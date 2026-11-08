//! An allocation-free-per-sample, eight-line feedback delay network.

use std::fmt;

#[cfg(feature = "gui")]
pub mod preview;

#[cfg(feature = "plugin")]
mod plugin;

pub const CHANNELS: usize = 8;
const INV_SQRT_8: f32 = 0.353_553_38;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Matrix {
    Householder,
    Hadamard,
    /// Householder first, then normalized Hadamard (A H).
    Hybrid,
}

impl Matrix {
    /// Orthogonal mixing, without constructing a dense matrix.
    pub fn mix(self, samples: &mut [f32; CHANNELS]) {
        match self {
            Self::Hybrid => {
                // A * (I - 2 uu^T) = diag(-1, 1, ..., 1) * A,
                // since A maps normalized all-ones u to the first basis vector.
                Self::Hadamard.mix(samples);
                samples[0] = -samples[0];
            }
            Self::Householder => {
                let correction = samples.iter().copied().sum::<f32>() * 0.25;
                for sample in samples {
                    *sample -= correction;
                }
            }
            Self::Hadamard => {
                // Explicit fixed-size butterfly stages give LLVM constant bounds.
                for chunk in samples.as_chunks_mut::<2>().0 {
                    let a = chunk[0];
                    let b = chunk[1];
                    chunk[0] = a + b;
                    chunk[1] = a - b;
                }
                for chunk in samples.as_chunks_mut::<4>().0 {
                    for i in 0..2 {
                        let a = chunk[i];
                        let b = chunk[i + 2];
                        chunk[i] = a + b;
                        chunk[i + 2] = a - b;
                    }
                }
                for i in 0..4 {
                    let a = samples[i];
                    let b = samples[i + 4];
                    samples[i] = a + b;
                    samples[i + 4] = a - b;
                }
                for sample in samples {
                    *sample *= INV_SQRT_8;
                }
            }
        }
    }
}

struct Allpass {
    buffer: Vec<f32>,
    position: usize,
}

impl Allpass {
    fn process(&mut self, input: f32) -> f32 {
        const G: f32 = 0.6;
        let delayed = self.buffer[self.position];
        let output = delayed - G * input;
        self.buffer[self.position] = input + G * output;
        self.position = (self.position + 1) % self.buffer.len();
        output
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Config {
    pub sample_rate: u32,
    /// Nominal low-frequency decay time in seconds, not a broadband guarantee.
    pub t60_seconds: f32,
    /// One-pole feedback low-pass cutoff parameter, in Hz.
    pub damping_hz: f32,
    pub matrix: Matrix,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            sample_rate: 48_000,
            t60_seconds: 2.5,
            damping_hz: 6_000.0,
            matrix: Matrix::Hadamard,
        }
    }
}

impl Config {
    fn validate(self) -> Result<(), ConfigError> {
        if !(8_000..=384_000).contains(&self.sample_rate) {
            return Err(ConfigError(
                "sample_rate must be between 8000 and 384000 Hz",
            ));
        }
        if !self.t60_seconds.is_finite() || self.t60_seconds <= 0.0 || self.t60_seconds > 60.0 {
            return Err(ConfigError("t60_seconds must be finite and in (0, 60]"));
        }
        if !self.damping_hz.is_finite()
            || self.damping_hz <= 0.0
            || self.damping_hz > self.sample_rate as f32 * 0.5
        {
            return Err(ConfigError("damping_hz must be finite and in (0, Nyquist]"));
        }
        Ok(())
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct ConfigError(&'static str);

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0)
    }
}

impl std::error::Error for ConfigError {}

/// Delay readout experiments; independent of feedback matrix and Loop Diffusion.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DelayMode {
    Static,
    RandomLinear,
    RandomAllpass,
}

impl DelayMode {
    fn weights(self) -> [f32; 3] {
        match self {
            Self::Static => [1.0, 0.0, 0.0],
            Self::RandomLinear => [0.0, 1.0, 0.0],
            Self::RandomAllpass => [0.0, 0.0, 1.0],
        }
    }
}

struct Delay {
    buffer: Vec<f32>,
    length: usize,
    position: usize,
    gain: f32,
    target_gain: f32,
    lowpass: f32,
    seed: u32,
    random: u32,
    random_target: f32,
    random_countdown: usize,
    random_low: f32,
    random_smooth: f32,
    allpass_input: f32,
    allpass_output: f32,
    allpass_tap: usize,
}

impl Delay {
    fn tap(&self, age: usize) -> f32 {
        self.buffer[(self.position + self.buffer.len() - age) % self.buffer.len()]
    }

    fn read(&mut self, depth: f32, interval: usize, coefficient: f32) -> [f32; 3] {
        // Independent sample-and-hold noise followed by two low-pass stages.
        // This is spectrally rolled off, not an ideal brick-wall band limit.
        if self.random_countdown == 0 {
            self.random ^= self.random << 13;
            self.random ^= self.random >> 17;
            self.random ^= self.random << 5;
            self.random_target = (self.random >> 8) as f32 / 8_388_607.5 - 1.0;
            self.random_countdown = interval;
        }
        self.random_countdown -= 1;
        self.random_low += coefficient * (self.random_target - self.random_low);
        self.random_smooth += coefficient * (self.random_low - self.random_smooth);
        let static_sample = self.tap(self.length);
        let age = self.length as f32 + depth * self.random_smooth;
        let integer = age.floor() as usize;
        let fraction = age - integer as f32;
        let linear = self.tap(integer) * (1.0 - fraction) + self.tap(integer + 1) * fraction;

        // First-order Thiran: keep the fractional part in [0.5, 1.5),
        // avoiding coefficients near +/-1 at integer-delay boundaries.
        let tap = (age - 0.5).floor() as usize;
        let fractional_delay = age - tap as f32;
        let a = (1.0 - fractional_delay) / (1.0 + fractional_delay);
        let input = self.tap(tap);
        if tap != self.allpass_tap {
            // Re-anchor history when the integer read head changes. This is
            // approximate, not an exactly lossless time-varying all-pass.
            let previous_age = age + 1.0;
            let previous_integer = previous_age.floor() as usize;
            self.allpass_input = self.tap(tap + 1);
            self.allpass_output = self.tap(previous_integer) * (1.0 - fraction)
                + self.tap(previous_integer + 1) * fraction;
            self.allpass_tap = tap;
        }
        let allpass = a * input + self.allpass_input - a * self.allpass_output;
        self.allpass_input = input;
        self.allpass_output = allpass;
        // Depth zero is a true integer read, without interpolation coloration.
        if depth == 0.0 {
            [static_sample; 3]
        } else {
            [static_sample, linear, allpass]
        }
    }
}

/// Mono input, wet-only stereo output. Construct outside the audio callback.
/// Input samples must be finite; no limiting or dry mixing is performed here.
pub struct Reverb {
    delays: [Delay; CHANNELS],
    sample_rate: u32,
    matrix_weights: [f32; 3],
    target_matrix_weights: [f32; 3],
    diffusers: [Allpass; 4],
    loop_diffusers: [Allpass; CHANNELS],
    loop_diffusion: f32,
    target_loop_diffusion: f32,
    diffusion: f32,
    target_diffusion: f32,
    damping_coefficient: f32,
    target_damping_coefficient: f32,
    smoothing: f32,
    delay_weights: [f32; 3],
    target_delay_weights: [f32; 3],
    mod_depth_ms: f32,
    target_mod_depth_ms: f32,
    mod_rate_hz: f32,
    target_mod_rate_hz: f32,
}

impl Reverb {
    pub fn new(config: Config) -> Result<Self, ConfigError> {
        config.validate()?;

        // Distinct prime lengths at 48 kHz, about 31 to 63 ms.
        let lengths = [1493, 1601, 1867, 2053, 2251, 2399, 2791, 3011];
        let delays = std::array::from_fn(|i| {
            let length =
                (lengths[i] as f64 * config.sample_rate as f64 / 48_000.0).round() as usize;
            let seconds = length as f32 / config.sample_rate as f32;
            Delay {
                buffer: vec![0.0; length + (0.001 * config.sample_rate as f32).ceil() as usize + 4],
                length,
                position: 0,
                gain: 10.0_f32.powf(-3.0 * seconds / config.t60_seconds),
                target_gain: 10.0_f32.powf(-3.0 * seconds / config.t60_seconds),
                lowpass: 0.0,
                seed: 0x9e37_79b9_u32.wrapping_mul(i as u32 + 1),
                random: 0x9e37_79b9_u32.wrapping_mul(i as u32 + 1),
                random_target: 0.0,
                random_countdown: 0,
                random_low: 0.0,
                random_smooth: 0.0,
                allpass_input: 0.0,
                allpass_output: 0.0,
                allpass_tap: usize::MAX,
            }
        });
        let damping_coefficient =
            (-std::f32::consts::TAU * config.damping_hz / config.sample_rate as f32).exp();
        let matrix_weights = Self::weights(config.matrix);
        Ok(Self {
            delays,
            sample_rate: config.sample_rate,
            matrix_weights,
            target_matrix_weights: matrix_weights,
            diffusers: [149, 211, 263, 293].map(|length| Allpass {
                buffer: vec![
                    0.0;
                    (length as f64 * config.sample_rate as f64 / 48_000.0).round() as usize
                ],
                position: 0,
            }),
            diffusion: 0.0,
            target_diffusion: 0.0,
            loop_diffusers: [127, 179, 233, 283, 337, 389, 443, 499].map(|length| Allpass {
                buffer: vec![
                    0.0;
                    (length as f64 * config.sample_rate as f64 / 48_000.0).round() as usize
                ],
                position: 0,
            }),
            loop_diffusion: 0.0,
            target_loop_diffusion: 0.0,
            damping_coefficient,
            target_damping_coefficient: damping_coefficient,
            smoothing: 1.0 - (-1.0 / (0.02 * config.sample_rate as f32)).exp(),
            delay_weights: DelayMode::Static.weights(),
            target_delay_weights: DelayMode::Static.weights(),
            mod_depth_ms: 0.2,
            target_mod_depth_ms: 0.2,
            mod_rate_hz: 0.3,
            target_mod_rate_hz: 0.3,
        })
    }

    fn weights(matrix: Matrix) -> [f32; 3] {
        match matrix {
            Matrix::Householder => [1.0, 0.0, 0.0],
            Matrix::Hadamard => [0.0, 1.0, 0.0],
            Matrix::Hybrid => [0.0, 0.0, 1.0],
        }
    }

    /// Blend the direct and four-stage all-pass input paths. This does not
    /// alter feedback decay gains. The intermediate blend can color the input.
    pub fn set_diffusion(&mut self, amount: f32) -> Result<(), ConfigError> {
        if !amount.is_finite() || !(0.0..=1.0).contains(&amount) {
            return Err(ConfigError("diffusion must be finite and in [0, 1]"));
        }
        self.target_diffusion = amount;
        Ok(())
    }

    /// Blend short per-line all-pass filters into the feedback path.
    /// Zero preserves the original path exactly. Intermediate blends can add
    /// frequency-dependent attenuation; added loop delay also affects decay.
    /// The amount is smoothed with a 20 ms time constant.
    pub fn set_loop_diffusion(&mut self, amount: f32) -> Result<(), ConfigError> {
        if !amount.is_finite() || !(0.0..=1.0).contains(&amount) {
            return Err(ConfigError("loop diffusion must be finite and in [0, 1]"));
        }
        self.target_loop_diffusion = amount;
        Ok(())
    }

    /// Select delay readout and low-pass random modulation. Depth is a maximum
    /// excursion in ms, rate controls noise update rate and low-pass cutoff.
    /// Parameters/mode blend are smoothed; no allocation or tail reset occurs.
    pub fn set_delay_modulation(
        &mut self,
        mode: DelayMode,
        depth_ms: f32,
        rate_hz: f32,
    ) -> Result<(), ConfigError> {
        if !depth_ms.is_finite()
            || !(0.0..=1.0).contains(&depth_ms)
            || !rate_hz.is_finite()
            || !(0.05..=2.0).contains(&rate_hz)
        {
            return Err(ConfigError(
                "modulation depth must be 0..1 ms and rate 0.05..2 Hz",
            ));
        }
        self.target_delay_weights = mode.weights();
        self.target_mod_depth_ms = depth_ms;
        self.target_mod_rate_hz = rate_hz;
        Ok(())
    }

    /// Update targets without allocation or resetting the tail. Coefficients
    /// approach their targets with a 20 ms time constant. Call at control rate,
    /// not every sample: this computes exponentials and powers.
    pub fn set_parameters(
        &mut self,
        t60_seconds: f32,
        damping_hz: f32,
        matrix: Matrix,
    ) -> Result<(), ConfigError> {
        Config {
            sample_rate: self.sample_rate,
            t60_seconds,
            damping_hz,
            matrix,
        }
        .validate()?;
        for delay in &mut self.delays {
            let seconds = delay.length as f32 / self.sample_rate as f32;
            delay.target_gain = 10.0_f32.powf(-3.0 * seconds / t60_seconds);
        }
        self.target_damping_coefficient =
            (-std::f32::consts::TAU * damping_hz / self.sample_rate as f32).exp();
        self.target_matrix_weights = Self::weights(matrix);
        Ok(())
    }

    /// Process one sample without allocation, locking, or system calls.
    pub fn process_sample(&mut self, input: f32) -> [f32; 2] {
        const INPUT: [f32; CHANNELS] = [1.0, -1.0, 1.0, 1.0, -1.0, 1.0, -1.0, -1.0];
        const LEFT: [f32; CHANNELS] = [1.0, 1.0, -1.0, 1.0, -1.0, -1.0, 1.0, -1.0];
        const RIGHT: [f32; CHANNELS] = [1.0, -1.0, 1.0, 1.0, 1.0, -1.0, -1.0, 1.0];
        let mut feedback = [0.0; CHANNELS];
        let mut output = [0.0; 2];
        self.damping_coefficient +=
            self.smoothing * (self.target_damping_coefficient - self.damping_coefficient);
        for (weight, target) in self
            .matrix_weights
            .iter_mut()
            .zip(self.target_matrix_weights)
        {
            *weight += self.smoothing * (target - *weight);
        }
        // Preserve a convex partition despite f32 smoothing roundoff near 1.
        let weight_sum: f32 = self.matrix_weights.iter().sum();
        for weight in &mut self.matrix_weights {
            *weight /= weight_sum;
        }
        if self
            .matrix_weights
            .iter()
            .zip(self.target_matrix_weights)
            .all(|(weight, target)| (target - weight).abs() < 1e-5)
        {
            self.matrix_weights = self.target_matrix_weights;
        }
        self.diffusion += self.smoothing * (self.target_diffusion - self.diffusion);
        self.loop_diffusion += self.smoothing * (self.target_loop_diffusion - self.loop_diffusion);
        // Above the f32 smoothing stall threshold even at 384 kHz.
        if (self.target_loop_diffusion - self.loop_diffusion).abs() < 1e-3 {
            self.loop_diffusion = self.target_loop_diffusion;
        }
        let mut diffused = input;
        for stage in &mut self.diffusers {
            diffused = stage.process(diffused);
        }
        let input = input + self.diffusion * (diffused - input);
        self.mod_depth_ms += self.smoothing * (self.target_mod_depth_ms - self.mod_depth_ms);
        self.mod_rate_hz += self.smoothing * (self.target_mod_rate_hz - self.mod_rate_hz);
        if (self.target_mod_depth_ms - self.mod_depth_ms).abs() < 1e-3 {
            self.mod_depth_ms = self.target_mod_depth_ms;
        }
        let random_interval = (self.sample_rate as f32 / self.mod_rate_hz).round() as usize;
        let random_coefficient =
            1.0 - (-std::f32::consts::TAU * self.mod_rate_hz / self.sample_rate as f32).exp();
        for (weight, target) in self.delay_weights.iter_mut().zip(self.target_delay_weights) {
            *weight += self.smoothing * (target - *weight);
        }
        let sum: f32 = self.delay_weights.iter().sum();
        for weight in &mut self.delay_weights {
            *weight /= sum;
        }
        if self
            .delay_weights
            .iter()
            .zip(self.target_delay_weights)
            .all(|(w, t)| (w - t).abs() < 1e-3)
        {
            self.delay_weights = self.target_delay_weights;
        }
        for (i, delay) in self.delays.iter_mut().enumerate() {
            delay.gain += self.smoothing * (delay.target_gain - delay.gain);
            let reads = delay.read(
                self.mod_depth_ms * 0.001 * self.sample_rate as f32,
                random_interval,
                random_coefficient,
            );
            let sample = if self.delay_weights == DelayMode::Static.weights() {
                reads[0]
            } else {
                reads
                    .into_iter()
                    .zip(self.delay_weights)
                    .map(|(x, w)| x * w)
                    .sum()
            };
            output[0] += sample * LEFT[i];
            output[1] += sample * RIGHT[i];
            delay.lowpass = (1.0 - self.damping_coefficient) * sample
                + self.damping_coefficient * delay.lowpass;
            // Avoid very small feedback states lingering as subnormal values.
            if delay.lowpass.abs() < 1.0e-20 {
                delay.lowpass = 0.0;
            }
            feedback[i] = delay.lowpass * delay.gain;
            // Keep filter history warm even at zero; bypass without arithmetic
            // on the original signal so the default response stays unchanged.
            let diffused = self.loop_diffusers[i].process(feedback[i]);
            if self.loop_diffusion != 0.0 {
                feedback[i] =
                    (1.0 - self.loop_diffusion) * feedback[i] + self.loop_diffusion * diffused;
            }
        }
        // Convex blending remains contractive during arbitrary mode changes.
        let original = feedback;
        feedback.fill(0.0);
        for (matrix, weight) in [Matrix::Householder, Matrix::Hadamard, Matrix::Hybrid]
            .into_iter()
            .zip(self.matrix_weights)
        {
            if weight != 0.0 {
                let mut mixed = original;
                matrix.mix(&mut mixed);
                for (sum, sample) in feedback.iter_mut().zip(mixed) {
                    *sum += weight * sample;
                }
            }
        }
        for (i, delay) in self.delays.iter_mut().enumerate() {
            delay.buffer[delay.position] = feedback[i] + input * INPUT[i] * INV_SQRT_8;
            delay.position += 1;
            if delay.position == delay.buffer.len() {
                delay.position = 0;
            }
        }
        [output[0] * INV_SQRT_8, output[1] * INV_SQRT_8]
    }

    pub fn reset(&mut self) {
        for stage in self.diffusers.iter_mut().chain(&mut self.loop_diffusers) {
            stage.buffer.fill(0.0);
            stage.position = 0;
        }
        for delay in &mut self.delays {
            delay.buffer.fill(0.0);
            delay.position = 0;
            delay.lowpass = 0.0;
            delay.random = delay.seed;
            delay.random_target = 0.0;
            delay.random_countdown = 0;
            delay.random_low = 0.0;
            delay.random_smooth = 0.0;
            delay.allpass_input = 0.0;
            delay.allpass_output = 0.0;
            delay.allpass_tap = usize::MAX;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MATRICES: [Matrix; 3] = [Matrix::Householder, Matrix::Hadamard, Matrix::Hybrid];

    #[test]
    fn live_parameters_validate_before_mutation_and_converge() {
        let mut reverb = Reverb::new(Config::default()).unwrap();
        let previous = reverb.delays[0].target_gain;
        assert!(
            reverb
                .set_parameters(f32::NAN, 1000.0, Matrix::Householder)
                .is_err()
        );
        assert_eq!(reverb.delays[0].target_gain, previous);
        reverb
            .set_parameters(0.2, 1000.0, Matrix::Householder)
            .unwrap();
        assert_eq!(reverb.matrix_weights, [0.0, 1.0, 0.0]);
        assert_eq!(reverb.target_matrix_weights, [1.0, 0.0, 0.0]);
        for _ in 0..48_000 {
            let output = reverb.process_sample(0.0);
            assert_eq!(output, [0.0; 2]);
        }
        assert_eq!(reverb.matrix_weights, [1.0, 0.0, 0.0]);
        assert!((reverb.delays[0].gain - reverb.delays[0].target_gain).abs() < 1e-4);
        assert!((reverb.damping_coefficient - reverb.target_damping_coefficient).abs() < 1e-4);
    }

    #[test]
    fn matrices_are_orthogonal_and_original_modes_self_inverse() {
        for matrix in MATRICES {
            let columns: [[f32; CHANNELS]; CHANNELS] = std::array::from_fn(|i| {
                let mut column = [0.0; CHANNELS];
                column[i] = 1.0;
                matrix.mix(&mut column);
                column
            });
            for i in 0..CHANNELS {
                for j in 0..CHANNELS {
                    let dot: f32 = columns[i].iter().zip(&columns[j]).map(|(a, b)| a * b).sum();
                    assert!((dot - if i == j { 1.0 } else { 0.0 }).abs() < 1e-6);
                }
            }
            // A product of reflections need not itself be self-inverse.
            if matrix == Matrix::Hybrid {
                continue;
            }
            let original = [0.2, -0.7, 0.5, 0.1, -0.3, 0.8, 0.9, -0.4];
            let mut samples = original;
            matrix.mix(&mut samples);
            matrix.mix(&mut samples);
            for (actual, expected) in samples.iter().zip(original) {
                assert!((actual - expected).abs() < 1e-6);
            }
        }
    }

    #[test]
    fn hybrid_matches_explicit_composition_and_preserves_energy() {
        for i in 0..CHANNELS {
            let mut expected = [0.0; CHANNELS];
            expected[i] = 1.0;
            let mut actual = expected;
            Matrix::Householder.mix(&mut expected);
            Matrix::Hadamard.mix(&mut expected);
            Matrix::Hybrid.mix(&mut actual);
            for (a, b) in actual.iter().zip(expected) {
                assert!((a - b).abs() < 1e-6);
            }
        }
    }

    #[test]
    fn allpass_impulse_preserves_energy() {
        let mut stage = Allpass {
            buffer: vec![0.0; 149],
            position: 0,
        };
        let mut energy = 0.0_f64;
        for i in 0..20_000 {
            let sample = stage.process(if i == 0 { 1.0 } else { 0.0 });
            energy += f64::from(sample).powi(2);
        }
        assert!((energy - 1.0).abs() < 1e-6);
    }

    #[test]
    fn diffusion_changes_response_decays_and_resets() {
        for matrix in MATRICES {
            let config = Config {
                matrix,
                t60_seconds: 0.3,
                ..Config::default()
            };
            let mut plain = Reverb::new(config).unwrap();
            let mut diffuse = Reverb::new(config).unwrap();
            for invalid in [f32::NAN, -0.1, 1.1] {
                assert!(diffuse.set_diffusion(invalid).is_err());
            }
            diffuse.set_diffusion(1.0).unwrap();
            // Settle smoothing before comparing impulse responses.
            for _ in 0..24_000 {
                diffuse.process_sample(0.0);
            }
            let mut difference = 0.0_f64;
            let mut late = 0.0_f64;
            for i in 0..96_000 {
                let input = if i == 0 { 1.0 } else { 0.0 };
                let a = plain.process_sample(input);
                let b = diffuse.process_sample(input);
                assert!(b.iter().all(|x| x.is_finite()));
                difference += f64::from(a[0] - b[0]).powi(2);
                if i > 72_000 {
                    late += f64::from(b[0]).powi(2);
                }
            }
            assert!(difference > 0.01);
            assert!(late < 1e-8);
            diffuse.reset();
            for _ in 0..8000 {
                assert_eq!(diffuse.process_sample(0.0), [0.0; 2]);
            }
        }
    }

    #[test]
    fn loop_diffusion_zero_preserves_original_path_and_validates_amount() {
        let mut plain = Reverb::new(Config::default()).unwrap();
        let mut bypassed = Reverb::new(Config::default()).unwrap();
        bypassed.set_loop_diffusion(0.5).unwrap();
        for invalid in [f32::NAN, f32::INFINITY, -0.1, 1.1] {
            assert!(bypassed.set_loop_diffusion(invalid).is_err());
            assert_eq!(bypassed.target_loop_diffusion, 0.5);
        }
        bypassed.set_loop_diffusion(0.0).unwrap();
        // Nonzero all-pass history must not leak into a bypassed loop.
        for stage in &mut bypassed.loop_diffusers {
            stage.buffer.fill(0.25);
        }
        for i in 0..16_000 {
            let input = if i == 0 { 1.0 } else { 0.0 };
            assert_eq!(plain.process_sample(input), bypassed.process_sample(input));
        }
        bypassed.set_loop_diffusion(1.0).unwrap();
        for _ in 0..24_000 {
            bypassed.process_sample(0.0);
        }
        assert_eq!(bypassed.loop_diffusion, 1.0);
        bypassed.set_loop_diffusion(0.0).unwrap();
        for _ in 0..24_000 {
            bypassed.process_sample(0.0);
        }
        assert_eq!(bypassed.loop_diffusion, 0.0);
    }

    #[test]
    fn loop_diffusion_changes_tail_decays_and_resets_at_multiple_rates() {
        for matrix in MATRICES {
            for sample_rate in [8_000, 44_100, 48_000, 96_000] {
                for amount in [0.5, 1.0] {
                    let config = Config {
                        sample_rate,
                        matrix,
                        t60_seconds: 0.3,
                        damping_hz: sample_rate as f32 * 0.125,
                    };
                    let mut plain = Reverb::new(config).unwrap();
                    let mut diffuse = Reverb::new(config).unwrap();
                    diffuse.set_loop_diffusion(amount).unwrap();
                    for _ in 0..sample_rate / 2 {
                        diffuse.process_sample(0.0);
                    }
                    let mut difference = 0.0_f64;
                    let mut early = 0.0_f64;
                    let mut late = 0.0_f64;
                    let mut first = Vec::new();
                    for i in 0..sample_rate * 2 {
                        let input = if i == 0 { 1.0 } else { 0.0 };
                        let a = plain.process_sample(input);
                        let b = diffuse.process_sample(input);
                        assert!(b.iter().all(|x| x.is_finite() && x.abs() < 4.0));
                        difference += f64::from(a[0] - b[0]).powi(2);
                        let energy = b.iter().map(|&x| f64::from(x).powi(2)).sum::<f64>();
                        if i < sample_rate / 2 {
                            early += energy;
                            first.push(b);
                        }
                        if i >= sample_rate * 3 / 2 {
                            late += energy;
                        }
                    }
                    assert!(difference > 0.001);
                    assert!(early > 0.01);
                    assert!(late < early * 1e-6);
                    diffuse.reset();
                    for _ in 0..sample_rate / 2 {
                        assert_eq!(diffuse.process_sample(0.0), [0.0; 2]);
                    }
                    for (i, expected) in first.into_iter().enumerate() {
                        assert_eq!(
                            diffuse.process_sample(if i == 0 { 1.0 } else { 0.0 }),
                            expected
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn static_read_matches_original_integer_ring_and_zero_depth() {
        let mut reverb = Reverb::new(Config::default()).unwrap();
        let delay = &mut reverb.delays[0];
        let mut original = vec![0.0; delay.length];
        let mut position = 0;
        for i in 0..delay.length * 5 {
            let reads = delay.read(0.0, 100, 0.001);
            assert_eq!(reads, [original[position]; 3]);
            let input = (i as f32 * 0.13).sin();
            original[position] = input;
            position = (position + 1) % original.len();
            delay.buffer[delay.position] = input;
            delay.position = (delay.position + 1) % delay.buffer.len();
        }
    }

    #[test]
    fn modulation_validates_before_mutation() {
        let mut reverb = Reverb::new(Config::default()).unwrap();
        for (depth, rate) in [
            (f32::NAN, 0.3),
            (-0.1, 0.3),
            (1.1, 0.3),
            (0.2, f32::INFINITY),
            (0.2, 0.0),
            (0.2, 2.1),
        ] {
            assert!(
                reverb
                    .set_delay_modulation(DelayMode::RandomAllpass, depth, rate)
                    .is_err()
            );
            assert_eq!(reverb.target_delay_weights, DelayMode::Static.weights());
            assert_eq!(reverb.target_mod_depth_ms, 0.2);
            assert_eq!(reverb.target_mod_rate_hz, 0.3);
        }
    }

    #[test]
    fn zero_depth_modes_match_static_response() {
        for mode in [DelayMode::RandomLinear, DelayMode::RandomAllpass] {
            let mut plain = Reverb::new(Config::default()).unwrap();
            let mut modulated = Reverb::new(Config::default()).unwrap();
            modulated.set_delay_modulation(mode, 0.0, 0.3).unwrap();
            modulated.mod_depth_ms = 0.0;
            modulated.delay_weights = mode.weights();
            for i in 0..24_000 {
                let input = if i == 0 { 1.0 } else { 0.0 };
                assert_eq!(plain.process_sample(input), modulated.process_sample(input));
            }
        }
    }

    #[test]
    fn allpass_random_modulation_remains_bounded_over_long_tail() {
        for matrix in MATRICES {
            let mut reverb = Reverb::new(Config {
                sample_rate: 8_000,
                t60_seconds: 20.0,
                damping_hz: 4_000.0,
                matrix,
            })
            .unwrap();
            reverb
                .set_delay_modulation(DelayMode::RandomAllpass, 1.0, 2.0)
                .unwrap();
            let mut early = 0.0_f64;
            let mut late = 0.0_f64;
            for i in 0..8_000 * 25 {
                let frame = reverb.process_sample(if i == 0 { 1.0 } else { 0.0 });
                assert!(frame.iter().all(|x| x.is_finite() && x.abs() < 4.0));
                let energy = frame.iter().map(|&x| f64::from(x).powi(2)).sum::<f64>();
                if i < 8_000 {
                    early += energy;
                }
                if i >= 8_000 * 24 {
                    late += energy;
                }
            }
            assert!(late < early * 1e-3, "{matrix:?}: {early} {late}");
        }
    }

    #[test]
    fn random_delay_modes_change_tail_decay_and_reset_reproducibly() {
        for mode in [DelayMode::RandomLinear, DelayMode::RandomAllpass] {
            for matrix in MATRICES {
                for sample_rate in [8_000, 48_000, 96_000] {
                    let config = Config {
                        sample_rate,
                        matrix,
                        t60_seconds: 0.5,
                        damping_hz: sample_rate as f32 * 0.125,
                    };
                    let mut reverb = Reverb::new(config).unwrap();
                    let mut plain = Reverb::new(config).unwrap();
                    reverb.set_delay_modulation(mode, 1.0, 2.0).unwrap();
                    reverb.delay_weights = mode.weights();
                    reverb.mod_depth_ms = 1.0;
                    reverb.mod_rate_hz = 2.0;
                    let mut first = Vec::new();
                    let mut difference = 0.0_f64;
                    let mut early = 0.0_f64;
                    let mut late = 0.0_f64;
                    for i in 0..sample_rate * 3 {
                        let input = if i == 0 { 1.0 } else { 0.0 };
                        let frame = reverb.process_sample(input);
                        let reference = plain.process_sample(input);
                        assert!(frame.iter().all(|x| x.is_finite() && x.abs() < 4.0));
                        difference += f64::from(frame[0] - reference[0]).powi(2);
                        let energy = frame.iter().map(|&x| f64::from(x).powi(2)).sum::<f64>();
                        if i < sample_rate / 2 {
                            early += energy;
                            first.push(frame);
                        }
                        if i >= sample_rate * 2 {
                            late += energy;
                        }
                    }
                    assert!(difference > 1e-5);
                    assert!(early > 0.01);
                    assert!(late < early * 1e-6, "{mode:?}: {early} {late}");
                    reverb.reset();
                    for (i, frame) in first.into_iter().enumerate() {
                        assert_eq!(reverb.process_sample(if i == 0 { 1.0 } else { 0.0 }), frame);
                    }
                    // Distinct per-line seeds avoid a shared chorus trajectory.
                    assert_ne!(
                        reverb.delays[0].random_smooth,
                        reverb.delays[1].random_smooth
                    );
                }
            }
        }
    }

    #[test]
    fn random_delay_switching_is_bounded_at_long_decay_and_extreme_rates() {
        for sample_rate in [8_000, 384_000] {
            let mut reverb = Reverb::new(Config {
                sample_rate,
                t60_seconds: 20.0,
                damping_hz: sample_rate as f32 * 0.5,
                ..Config::default()
            })
            .unwrap();
            for i in 0..sample_rate * 3 {
                if i % (sample_rate / 4) == 0 {
                    let modes = [
                        DelayMode::Static,
                        DelayMode::RandomLinear,
                        DelayMode::RandomAllpass,
                    ];
                    reverb
                        .set_delay_modulation(
                            modes[(i / (sample_rate / 4)) as usize % 3],
                            if (i / (sample_rate / 4)) % 2 == 0 {
                                1.0
                            } else {
                                0.0
                            },
                            2.0,
                        )
                        .unwrap();
                }
                let frame = reverb.process_sample(if i == 0 { 1.0 } else { 0.0 });
                assert!(frame.iter().all(|x| x.is_finite() && x.abs() < 4.0));
            }
        }
    }

    #[test]
    fn rejects_invalid_configurations() {
        for config in [
            Config {
                sample_rate: 0,
                ..Config::default()
            },
            Config {
                t60_seconds: 0.0,
                ..Config::default()
            },
            Config {
                t60_seconds: f32::NAN,
                ..Config::default()
            },
            Config {
                t60_seconds: f32::INFINITY,
                ..Config::default()
            },
            Config {
                damping_hz: -1.0,
                ..Config::default()
            },
            Config {
                damping_hz: f32::NAN,
                ..Config::default()
            },
            Config {
                damping_hz: 30_000.0,
                ..Config::default()
            },
        ] {
            assert!(Reverb::new(config).is_err());
        }
    }

    #[test]
    fn silence_and_reset_are_exact() {
        for matrix in MATRICES {
            let mut reverb = Reverb::new(Config {
                matrix,
                ..Config::default()
            })
            .unwrap();
            for _ in 0..4_000 {
                assert_eq!(reverb.process_sample(0.0), [0.0; 2]);
            }
            reverb.process_sample(1.0);
            for _ in 0..4_000 {
                reverb.process_sample(0.0);
            }
            reverb.reset();
            let mut fresh = Reverb::new(Config {
                matrix,
                ..Config::default()
            })
            .unwrap();
            for i in 0..8_000 {
                let input = if i == 0 { 1.0 } else { 0.0 };
                assert_eq!(reverb.process_sample(input), fresh.process_sample(input));
            }
        }
    }

    #[test]
    fn impulse_is_finite_stereo_and_decays_at_multiple_sample_rates() {
        for matrix in MATRICES {
            for sample_rate in [8_000, 44_100, 48_000, 96_000] {
                let mut reverb = Reverb::new(Config {
                    matrix,
                    sample_rate,
                    t60_seconds: 0.5,
                    damping_hz: sample_rate as f32 * 0.125,
                })
                .unwrap();
                let mut early_energy = 0.0_f64;
                let mut late_energy = 0.0_f64;
                let mut stereo_difference = 0.0_f64;
                for i in 0..sample_rate * 2 {
                    let frame = reverb.process_sample(if i == 0 { 1.0 } else { 0.0 });
                    assert!(frame.iter().all(|x| x.is_finite()));
                    let energy = frame.iter().map(|&x| f64::from(x).powi(2)).sum::<f64>();
                    if i < sample_rate / 2 {
                        early_energy += energy;
                        stereo_difference += f64::from(frame[0] - frame[1]).powi(2);
                    }
                    if i >= sample_rate * 3 / 2 {
                        late_energy += energy;
                    }
                }
                assert!(early_energy > 0.01);
                assert!(stereo_difference > 0.01);
                assert!(late_energy < early_energy * 1e-8);
            }
        }
    }
}
