//! RIR2FDN transposed scattering inference, using offline-designed coefficients.
//! No FIR convolution: the paraunitary matrix is executed as delayed stages.

#[cfg_attr(not(feature = "plugin"), allow(dead_code))]
mod live;
#[cfg(feature = "ir-analysis")]
pub mod seed;

#[derive(Clone)]
struct Delay {
    data: Vec<f64>,
    pos: usize,
}
impl Delay {
    fn new(samples: usize) -> Self {
        Self {
            data: vec![0.0; samples],
            pos: 0,
        }
    }
    fn tick(&mut self, x: f64) -> f64 {
        if self.data.is_empty() {
            return x;
        }
        let y = self.data[self.pos];
        self.data[self.pos] = x;
        self.pos += 1;
        if self.pos == self.data.len() {
            self.pos = 0;
        }
        y
    }
    fn reset(&mut self) {
        self.data.fill(0.0);
        self.pos = 0;
    }
}

struct Sos {
    b: [f64; 3],
    a: [f64; 2],
    state: [f64; 2],
}
impl Sos {
    fn tick(&mut self, x: f64) -> f64 {
        let y = self.b[0] * x + self.state[0];
        self.state[0] = self.b[1] * x - self.a[0] * y + self.state[1];
        self.state[1] = self.b[2] * x - self.a[1] * y;
        y
    }
}

/// Shared scattering FDN. Legacy profiles are mono; Rust seeds add stereo readouts.
pub struct DiffFdn {
    controls: Option<live::Controls>,
    rate: u32,
    main: Vec<Delay>,
    left: Vec<Delay>,
    right: Vec<Delay>,
    stages: Vec<(Vec<f64>, Vec<Delay>)>,
    input: Vec<f64>,
    output: Vec<f64>,
    stereo: Option<(Vec<f64>, Vec<Sos>)>,
    absorption: Vec<Vec<Sos>>,
    tone: Vec<Sos>,
    work: Vec<f64>,
    scratch: Vec<f64>,
    damping_state: Vec<f64>,
    damping: f64,
    damping_target: f64,
    damping_smoothing: f64,
    damping_lowpass: f64,
}

struct Reader<'a> {
    tokens: std::str::SplitWhitespace<'a>,
}
impl Reader<'_> {
    fn number(&mut self) -> Result<f64, String> {
        let x: f64 = self
            .tokens
            .next()
            .ok_or("truncated DiffFDN profile")?
            .parse()
            .map_err(|_| "invalid number")?;
        if !x.is_finite() {
            return Err("nonfinite coefficient".into());
        }
        Ok(x)
    }
    fn count(&mut self, low: usize, high: usize) -> Result<usize, String> {
        let x = self.number()?;
        if x.fract() != 0.0 || x < low as f64 || x > high as f64 {
            return Err("invalid count or delay".into());
        }
        Ok(x as usize)
    }
    fn vector(&mut self, n: usize) -> Result<Vec<f64>, String> {
        (0..n).map(|_| self.number()).collect()
    }
    fn delays(&mut self, n: usize, low: usize) -> Result<Vec<Delay>, String> {
        (0..n)
            .map(|_| self.count(low, 192000).map(Delay::new))
            .collect()
    }
    fn sos(&mut self) -> Result<Vec<Sos>, String> {
        let n = self.count(1, 64)?;
        (0..n)
            .map(|_| {
                let v = self.vector(6)?;
                if v[3] == 0.0 {
                    return Err("zero SOS denominator".into());
                }
                let a = [v[4] / v[3], v[5] / v[3]];
                // Strict second-order Jury test (also accepts first-order sections).
                if a[1].abs() >= 1.0 || 1.0 + a[0] + a[1] <= 0.0 || 1.0 - a[0] + a[1] <= 0.0 {
                    return Err("unstable SOS".into());
                }
                let b = [v[0] / v[3], v[1] / v[3], v[2] / v[3]];
                if b.iter().chain(&a).any(|x| !x.is_finite()) {
                    return Err("invalid normalized SOS".into());
                }
                Ok(Sos {
                    b,
                    a,
                    state: [0.0; 2],
                })
            })
            .collect()
    }
}

impl DiffFdn {
    /// Load trusted offline fits. Stable SOS and orthogonal stages are checked;
    /// SOS stability alone does not certify a passive closed loop. Validate renders.
    pub fn parse(text: &str) -> Result<Self, String> {
        let mut r = Reader {
            tokens: text.split_whitespace(),
        };
        if r.tokens.next() != Some("difffdn-scattering-v1") {
            return Err("unsupported DiffFDN profile".into());
        }
        let rate = r.count(8000, 192000)? as u32;
        let n = r.count(2, 32)?;
        let k = r.count(4, 4)?;
        let main = r.delays(n, 1)?;
        let left = r.delays(n, 0)?;
        let right = r.delays(n, 0)?;
        let input = r.vector(n)?;
        let output = r.vector(n)?;
        let mut stages = Vec::new();
        for _ in 0..k {
            let matrix = r.vector(n * n)?;
            for i in 0..n {
                for j in 0..n {
                    let dot: f64 = (0..n).map(|p| matrix[p * n + i] * matrix[p * n + j]).sum();
                    if (dot - if i == j { 1.0 } else { 0.0 }).abs() > 2e-5 {
                        return Err("nonorthogonal scattering stage".into());
                    }
                }
            }
            stages.push((matrix, r.delays(n, 0)?));
        }
        let absorption = (0..n).map(|_| r.sos()).collect::<Result<Vec<_>, _>>()?;
        let tone = r.sos()?;
        if r.tokens.next().is_some() {
            return Err("extra DiffFDN data".into());
        }
        Ok(Self {
            controls: None,
            rate,
            main,
            left,
            right,
            stages,
            input,
            output,
            stereo: None,
            absorption,
            tone,
            work: vec![0.0; n],
            scratch: vec![0.0; n],
            damping_state: vec![0.0; n],
            damping: 0.0,
            damping_target: 0.0,
            damping_smoothing: 1.0 - (-1.0 / (0.02 * rate as f64)).exp(),
            damping_lowpass: 1.0 - (-std::f64::consts::TAU * 2500.0 / rate as f64).exp(),
        })
    }
    /// Retarget the trusted fit off the audio thread. Bilinear substitution keeps
    /// stable poles and maps the entire unit circle (and thus passive filters).
    /// Delay rounding and frequency warping mean this is not a new room fit.
    pub fn at_sample_rate(mut self, rate: u32) -> Result<Self, String> {
        if self.controls.is_some() {
            return Err("retarget before enabling live controls".into());
        }
        if !(8000..=384000).contains(&rate) {
            return Err("unsupported sample rate".into());
        }
        if rate == self.rate {
            return Ok(self);
        }
        let ratio = rate as f64 / self.rate as f64;
        let a = (self.rate as f64 - rate as f64) / (self.rate as f64 + rate as f64);
        let transform = |c: [f64; 3]| {
            [
                c[0] + c[1] * a + c[2] * a * a,
                2.0 * a * (c[0] + c[2]) + c[1] * (1.0 + a * a),
                c[0] * a * a + c[1] * a + c[2],
            ]
        };
        for s in self.absorption.iter_mut().flatten().chain(&mut self.tone) {
            let b = transform(s.b);
            let d = transform([1.0, s.a[0], s.a[1]]);
            s.b = b.map(|x| x / d[0]);
            s.a = [d[1] / d[0], d[2] / d[0]];
        }
        if let Some((_, tone)) = &mut self.stereo {
            for s in tone {
                let b = transform(s.b);
                let d = transform([1.0, s.a[0], s.a[1]]);
                s.b = b.map(|x| x / d[0]);
                s.a = [d[1] / d[0], d[2] / d[0]];
            }
        }
        for d in self
            .main
            .iter_mut()
            .chain(&mut self.left)
            .chain(&mut self.right)
            .chain(self.stages.iter_mut().flat_map(|(_, delays)| delays))
        {
            let len = if d.data.is_empty() {
                0
            } else {
                ((d.data.len() as f64 * ratio).round() as usize).max(1)
            };
            *d = Delay::new(len);
        }
        self.rate = rate;
        self.damping_smoothing = 1.0 - (-1.0 / (0.02 * rate as f64)).exp();
        self.damping_lowpass = 1.0 - (-std::f64::consts::TAU * 2500.0 / rate as f64).exp();
        self.reset();
        Ok(self)
    }

    /// Additional passive HF loss in every loop, not an output tone control.
    /// 0 is bit-identical to the fit; 1 inserts a 2.5 kHz one-pole lowpass.
    pub fn set_damping(&mut self, amount: f64) {
        self.damping_target = if amount.is_finite() {
            amount.clamp(0.0, 1.0)
        } else {
            0.0
        };
    }
    pub fn sample_rate(&self) -> u32 {
        self.rate
    }
    /// Activate Classic-style editing after sample-rate conversion, off-thread.
    #[cfg_attr(not(feature = "plugin"), allow(dead_code))]
    pub(crate) fn enable_live_controls(&mut self) {
        let lengths: Vec<_> = self.main.iter().map(|d| d.data.len()).collect();
        self.controls = Some(live::Controls::new(self.rate, &lengths));
    }

    /// Additional loop loss and orthogonal scattering edits, not a refit of T60.
    #[cfg_attr(not(feature = "plugin"), allow(dead_code))]
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn set_live_controls(
        &mut self,
        t60: f32,
        high: f32,
        matrix: crate::Matrix,
        diffusion: f32,
        loop_diffusion: f32,
        low: f32,
        mid: f32,
        low_hz: f32,
        mid_hz: f32,
        q: f32,
        mode: crate::DelayMode,
        depth: f32,
        rate: f32,
    ) {
        if let Some(c) = &mut self.controls {
            c.update(
                t60,
                high,
                matrix,
                diffusion,
                loop_diffusion,
                low,
                mid,
                low_hz,
                mid_hz,
                q,
                mode,
                depth,
                rate,
            );
        }
    }
    pub fn reset(&mut self) {
        if let Some(c) = &mut self.controls {
            c.reset();
        }
        for d in self
            .main
            .iter_mut()
            .chain(&mut self.left)
            .chain(&mut self.right)
        {
            d.reset();
        }
        for (_, delays) in &mut self.stages {
            for d in delays {
                d.reset();
            }
        }
        for s in self.absorption.iter_mut().flatten().chain(&mut self.tone) {
            s.state = [0.0; 2];
        }
        if let Some((_, tone)) = &mut self.stereo {
            for s in tone {
                s.state = [0.0; 2];
            }
        }
        self.work.fill(0.0);
        self.scratch.fill(0.0);
        self.damping_state.fill(0.0);
        self.damping = self.damping_target;
    }
    /// Allocation-free per sample. Finite input and validated passive fits required.
    pub fn process_sample(&mut self, input: f64) -> f64 {
        self.process_stereo(input)[0]
    }
    /// One shared feedback network, with optional independently weighted outputs.
    pub fn process_stereo(&mut self, input: f64) -> [f64; 2] {
        let input = self.controls.as_mut().map_or(input, |c| c.input(input));
        self.damping += self.damping_smoothing * (self.damping_target - self.damping);
        let lowpass = self.damping_lowpass;
        let n = self.work.len();
        // main.pos points to the sample that will be overwritten below.
        for i in 0..n {
            let feedback = if let Some(c) = &mut self.controls {
                c.read(i)
            } else {
                self.main[i].data[self.main[i].pos]
            };
            self.work[i] = self.right[i].tick(self.input[i] * input + feedback);
        }
        for (matrix, delays) in &mut self.stages {
            for (sum, row) in self.scratch.iter_mut().zip(matrix.chunks_exact(n)) {
                *sum = row.iter().zip(&self.work).map(|(a, b)| a * b).sum();
            }
            for (i, d) in delays.iter_mut().enumerate() {
                self.work[i] = d.tick(self.scratch[i]);
            }
        }
        if let Some(c) = &self.controls {
            c.mix(&mut self.work);
        }
        let mut output = 0.0;
        let mut other = 0.0;
        for i in 0..n {
            let mut x = self.left[i].tick(self.work[i]);
            for s in &mut self.absorption[i] {
                x = s.tick(x);
            }
            self.damping_state[i] += lowpass * (x - self.damping_state[i]);
            x += self.damping * (self.damping_state[i] - x);
            if let Some(c) = &mut self.controls {
                x = c.write(i, x);
            } else {
                self.main[i].tick(x);
            }
            output += self.output[i] * x;
            if let Some((weights, _)) = &self.stereo {
                other += weights[i] * x;
            }
        }
        for s in &mut self.tone {
            output = s.tick(output);
        }
        if let Some((_, tone)) = &mut self.stereo {
            for s in tone {
                other = s.tick(other);
            }
        } else {
            other = output;
        }
        [output, other]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_rooms_rates_damping_and_long_tail() {
        for text in [
            include_str!("../resources/difffdn/salon.fdn"),
            include_str!("../resources/difffdn/damped.fdn"),
        ] {
            for rate in [8000, 44100, 48000, 88200, 96000, 192000, 384000] {
                let mut energies = [0.0; 2];
                for (setting, energy) in energies.iter_mut().enumerate() {
                    let mut f = DiffFdn::parse(text).unwrap().at_sample_rate(rate).unwrap();
                    for s in f.absorption.iter().flatten().chain(&f.tone) {
                        assert!(s.a[1].abs() < 1.0);
                        assert!(1.0 + s.a[0] + s.a[1] > 0.0);
                        assert!(1.0 - s.a[0] + s.a[1] > 0.0);
                    }
                    f.set_damping(setting as f64);
                    f.reset();
                    let mut tail = 0.0;
                    let mut onset = None;
                    for i in 0..rate * 12 {
                        let y = f.process_sample(if i == 0 { 1.0 } else { 0.0 });
                        assert!(y.is_finite() && y.abs() < 100.0, "rate={rate}, y={y}");
                        *energy += y * y;
                        if y.abs() > 1e-12 {
                            onset.get_or_insert(i);
                        }
                        if i >= rate * 11 {
                            tail += y * y;
                        }
                    }
                    assert!(onset.unwrap() > 0, "wet must not contain a direct impulse");
                    assert!(
                        tail < *energy * 1e-12,
                        "rate={rate}, tail={tail}, total={energy}"
                    );
                }
                assert!(
                    energies[1] < energies[0],
                    "damping must reduce impulse energy"
                );
            }
        }
    }

    #[test]
    fn damping_smooths_and_resets_to_target() {
        let mut f = DiffFdn::parse(&profile()).unwrap();
        f.set_damping(1.0);
        f.process_sample(1.0);
        assert!(f.damping > 0.0 && f.damping < 0.01);
        for _ in 0..4800 {
            f.process_sample(0.0);
        }
        assert!((f.damping - 1.0).abs() < 0.01);
        f.reset();
        assert_eq!(f.damping, 1.0);
        assert!(f.damping_state.iter().all(|x| *x == 0.0));
        for bad in [f64::NAN, f64::INFINITY] {
            f.set_damping(bad);
            assert_eq!(f.damping_target, 0.0);
        }
        assert!(
            DiffFdn::parse(&profile())
                .unwrap()
                .at_sample_rate(7999)
                .is_err()
        );
    }

    #[test]
    fn added_damping_is_passive_and_high_frequency_selective() {
        for rate in [8000, 44100, 48000, 96000, 384000] {
            let f = DiffFdn::parse(&profile())
                .unwrap()
                .at_sample_rate(rate)
                .unwrap();
            let p = 1.0 - f.damping_lowpass;
            for amount in [0.0, 0.25, 0.5, 0.75, 1.0] {
                let magnitude = |w: f64| {
                    let denominator = 1.0 - 2.0 * p * w.cos() + p * p;
                    let re = f.damping_lowpass * (1.0 - p * w.cos()) / denominator;
                    let im = -f.damping_lowpass * p * w.sin() / denominator;
                    ((1.0 - amount + amount * re).powi(2) + (amount * im).powi(2)).sqrt()
                };
                for i in 0..1025 {
                    assert!(magnitude(std::f64::consts::PI * i as f64 / 1024.0) <= 1.0 + 1e-12);
                }
                let low = magnitude(std::f64::consts::TAU * 100.0 / rate as f64);
                let high = magnitude(
                    std::f64::consts::TAU * (rate as f64 * 0.4).min(10000.0) / rate as f64,
                );
                assert!(high <= low + 1e-12);
                if amount > 0.0 {
                    assert!(high < low);
                }
            }
        }
    }
    fn profile() -> String {
        let mut s = "difffdn-scattering-v1 48000 2 4 3 5 0 0 0 0 1 0 1 1 ".to_string();
        for _ in 0..4 {
            s.push_str("0 1 1 0 0 0 ");
        }
        s.push_str("1 .5 0 0 1 0 0 1 .4 0 0 1 0 0 1 2 0 0 1 0 0");
        s
    }
    #[test]
    fn transposed_onset_decay_reset_and_validation() {
        let p = profile();
        let mut f = DiffFdn::parse(&p).unwrap();
        let y: Vec<_> = (0..48000)
            .map(|i| f.process_sample(if i == 0 { 1.0 } else { 0.0 }))
            .collect();
        assert_eq!(y[0], 1.0); // output before main delays, not at sample 3
        assert_eq!(y[3], 0.5);
        assert_eq!(y[6], 0.25);
        assert!(y[24000..].iter().all(|x| x.abs() < 1e-20));
        f.reset();
        for (i, expected) in y.iter().enumerate() {
            assert_eq!(f.process_sample(if i == 0 { 1.0 } else { 0.0 }), *expected);
        }
        for bad in [
            format!("{p} 0"),
            p.replace("48000", "NaN"),
            p.replace("0 1 1 0", "1 1 1 0"),
            p.replace("1 0 0 1 2", "1 0 1 1 2"),
        ] {
            assert!(DiffFdn::parse(&bad).is_err());
        }
    }
    #[test]
    fn stage_and_external_delays_are_causal() {
        let p = profile().replacen("3 5 0 0 0 0", "3 5 2 2 4 4", 1);
        let mut f = DiffFdn::parse(&p).unwrap();
        let y: Vec<_> = (0..20)
            .map(|i| f.process_sample(if i == 0 { 1.0 } else { 0.0 }))
            .collect();
        assert!(y[..6].iter().all(|x| *x == 0.0));
        assert_eq!(y[6], 1.0);
    }
}
