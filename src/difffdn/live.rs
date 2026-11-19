//! Classic controls around the learned scattering network. All storage is prepared
//! at activation. Decay controls add loss; they never invert the fitted absorption.
use crate::{Config, DelayMode, Matrix, ReverbTank};

pub(super) struct Controls {
    tank: ReverbTank,
    interval: usize,
    coefficient: f32,
    modulation_rate: f32,
    targets: Option<([f32; 11], Matrix, DelayMode)>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cached_targets_match_forced_updates_through_automation_and_reset() {
        let lengths = [149, 211, 263, 293, 337, 379, 421, 463];
        let mut cached = Controls::new(8000, &lengths);
        let mut forced = Controls::new(8000, &lengths);
        for sample in 0..16000 {
            if sample % 128 == 0 {
                let phase = sample / 2048;
                for (i, c) in [&mut cached, &mut forced].into_iter().enumerate() {
                    if i == 1 {
                        c.targets = None;
                    }
                    c.update(
                        0.5 + phase as f32 * 0.1,
                        3000.0,
                        Matrix::Hybrid,
                        0.7,
                        0.4,
                        3.0,
                        4.0,
                        250.0,
                        1000.0,
                        0.7,
                        DelayMode::RandomAllpass,
                        0.5,
                        0.1 + phase as f32 * 0.1,
                    );
                }
            }
            if sample == 8192 {
                cached.reset();
                forced.reset();
            }
            let input = (sample as f64 * 0.17).sin() * 0.1;
            assert_eq!(cached.input(input), forced.input(input));
            for line in 0..8 {
                assert_eq!(cached.read(line), forced.read(line));
                assert_eq!(cached.write(line, input), forced.write(line, input));
            }
        }
    }
}

impl Controls {
    pub fn new(rate: u32, lengths: &[usize]) -> Self {
        assert_eq!(lengths.len(), 8);
        let mut tank = ReverbTank::new(Config {
            sample_rate: rate,
            damping_hz: 6000.0_f32.min(rate as f32 * 0.45),
            ..Config::default()
        })
        .unwrap();
        for (delay, &length) in tank.delays.iter_mut().zip(lengths) {
            delay.length = length;
            delay.buffer = vec![0.0; length + (0.001 * rate as f32).ceil() as usize + 4];
        }
        Self {
            tank,
            interval: 1,
            coefficient: 0.0,
            modulation_rate: f32::NAN,
            targets: None,
        }
    }

    pub fn update(
        &mut self,
        t60: f32,
        high: f32,
        matrix: Matrix,
        diffusion: f32,
        loop_diffusion: f32,
        low: f32,
        mid: f32,
        low_hz: f32,
        mid_hz: f32,
        q: f32,
        mode: DelayMode,
        depth: f32,
        rate: f32,
    ) {
        let targets = (
            [
                t60,
                high,
                diffusion,
                loop_diffusion,
                low,
                mid,
                low_hz,
                mid_hz,
                q,
                depth,
                rate,
            ],
            matrix,
            mode,
        );
        if self.targets == Some(targets) {
            return;
        }
        let t = &mut self.tank;
        t.set_parameters(t60, high.min(t.sample_rate as f32 * 0.45), matrix)
            .unwrap();
        t.set_diffusion(diffusion).unwrap();
        t.set_loop_diffusion(loop_diffusion).unwrap();
        t.set_band_damping(low, mid).unwrap();
        t.set_damping_shape(low_hz, mid_hz, q).unwrap();
        t.set_delay_modulation(mode, depth, rate).unwrap();
        self.targets = Some(targets);
    }

    pub fn reset(&mut self) {
        self.tank.reset();
        self.tank.snap_parameters();
    }

    pub fn input(&mut self, input: f64) -> f64 {
        let t = &mut self.tank;
        let s = t.smoothing;
        t.diffusion += s * (t.target_diffusion - t.diffusion);
        t.loop_diffusion += s * (t.target_loop_diffusion - t.loop_diffusion);
        t.damping_coefficient += s * (t.target_damping_coefficient - t.damping_coefficient);
        for (x, target) in t.damping_amounts.iter_mut().zip(t.target_damping_amounts) {
            *x += s * (target - *x);
        }
        for (x, target) in t.matrix_weights.iter_mut().zip(t.target_matrix_weights) {
            *x += s * (target - *x);
        }
        let sum: f32 = t.matrix_weights.iter().sum();
        for x in &mut t.matrix_weights {
            *x /= sum;
        }
        for (x, target) in t.delay_weights.iter_mut().zip(t.target_delay_weights) {
            *x += s * (target - *x);
        }
        let sum: f32 = t.delay_weights.iter().sum();
        for x in &mut t.delay_weights {
            *x /= sum;
        }
        t.mod_depth_ms += s * (t.target_mod_depth_ms - t.mod_depth_ms);
        t.mod_rate_hz += s * (t.target_mod_rate_hz - t.mod_rate_hz);
        if self.modulation_rate != t.mod_rate_hz {
            self.modulation_rate = t.mod_rate_hz;
            self.interval = (t.sample_rate as f32 / (t.mod_rate_hz * 8.0))
                .round()
                .max(1.0) as usize;
            self.coefficient =
                1.0 - (-std::f32::consts::TAU * t.mod_rate_hz / t.sample_rate as f32).exp();
        }
        let mut diffused = input as f32;
        for stage in &mut t.diffusers {
            diffused = stage.process(diffused);
        }
        (1.0 - t.diffusion as f64) * input + t.diffusion as f64 * diffused as f64
    }

    pub fn read(&mut self, line: usize) -> f64 {
        let t = &mut self.tank;
        let reads = t.delays[line].read(
            t.mod_depth_ms * 0.001 * t.sample_rate as f32,
            self.interval,
            self.coefficient,
        );
        reads
            .into_iter()
            .zip(t.delay_weights)
            .map(|(x, w)| x * w)
            .sum::<f32>() as f64
    }

    pub fn mix(&self, work: &mut [f64]) {
        let original: [f32; 8] = std::array::from_fn(|i| work[i] as f32);
        work.fill(0.0);
        // Hybrid is the learned scattering fit. Other selections add the
        // corresponding orthogonal transform; smoothing is a contractive blend.
        for (matrix, weight) in [Some(Matrix::Householder), Some(Matrix::Hadamard), None]
            .into_iter()
            .zip(self.tank.matrix_weights)
        {
            if weight == 0.0 {
                continue;
            }
            let mut mixed = original;
            if let Some(matrix) = matrix {
                matrix.mix(&mut mixed);
            }
            for (sum, x) in work.iter_mut().zip(mixed) {
                *sum += (weight * x) as f64;
            }
        }
    }

    pub fn write(&mut self, line: usize, input: f64) -> f64 {
        let t = &mut self.tank;
        let d = &mut t.delays[line];
        d.gain += t.smoothing * (d.target_gain - d.gain);
        d.lowpass =
            (1.0 - t.damping_coefficient) * input as f32 + t.damping_coefficient * d.lowpass;
        let mut x = d.lowpass * d.gain;
        x = d.low_damping.process(x, t.damping_amounts[0]);
        x = d.mid_damping.process(x, t.damping_amounts[1]);
        let diffused = t.loop_diffusers[line].process(x);
        x = (1.0 - t.loop_diffusion) * x + t.loop_diffusion * diffused;
        d.buffer[d.position] = x;
        d.position += 1;
        if d.position == d.buffer.len() {
            d.position = 0;
        }
        x as f64
    }
}
