//! Single-tank Classic reverb.
use crate::{Config, ConfigError, DelayMode, Matrix, ReverbTank};

/// Mono-in, wet-only stereo reverb using one eight-line Classic FDN.
/// Construct outside the audio callback. Input samples must be finite.
pub struct Reverb {
    pub(crate) tank: ReverbTank,
}

impl Reverb {
    pub fn new(config: Config) -> Result<Self, ConfigError> {
        Ok(Self {
            tank: ReverbTank::new(config)?,
        })
    }

    pub fn set_parameters(
        &mut self,
        t60: f32,
        damping: f32,
        matrix: Matrix,
    ) -> Result<(), ConfigError> {
        self.tank.set_parameters(t60, damping, matrix)
    }

    pub fn set_diffusion(&mut self, amount: f32) -> Result<(), ConfigError> {
        self.tank.set_diffusion(amount)
    }

    pub fn set_loop_diffusion(&mut self, amount: f32) -> Result<(), ConfigError> {
        self.tank.set_loop_diffusion(amount)
    }

    pub fn set_band_damping(&mut self, low: f32, mid: f32) -> Result<(), ConfigError> {
        self.tank.set_band_damping(low, mid)
    }

    pub fn set_damping_shape(&mut self, low: f32, mid: f32, q: f32) -> Result<(), ConfigError> {
        self.tank.set_damping_shape(low, mid, q)
    }

    pub fn set_delay_modulation(
        &mut self,
        mode: DelayMode,
        depth: f32,
        rate: f32,
    ) -> Result<(), ConfigError> {
        self.tank.set_delay_modulation(mode, depth, rate)
    }

    /// Clear histories and immediately apply current target parameters.
    pub fn reset(&mut self) {
        self.tank.reset();
        self.tank.snap_parameters();
    }

    /// No allocation, locking, or system calls. Input must be finite.
    pub fn process_sample(&mut self, input: f32) -> [f32; 2] {
        self.tank.process_sample(input)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classic_matches_original_tank_sample_for_sample() {
        for sample_rate in [8000, 48000, 96000] {
            for matrix in [Matrix::Householder, Matrix::Hadamard, Matrix::Hybrid] {
                let config = Config {
                    sample_rate,
                    matrix,
                    damping_hz: 3000.0,
                    ..Config::default()
                };
                let mut original = ReverbTank::new(config).unwrap();
                let mut reverb = Reverb::new(config).unwrap();
                for i in 0..sample_rate {
                    if i == sample_rate / 2 {
                        original
                            .set_parameters(0.7, 2000.0, Matrix::Hybrid)
                            .unwrap();
                        reverb.set_parameters(0.7, 2000.0, Matrix::Hybrid).unwrap();
                        original.set_diffusion(0.3).unwrap();
                        reverb.set_diffusion(0.3).unwrap();
                        original.set_loop_diffusion(0.2).unwrap();
                        reverb.set_loop_diffusion(0.2).unwrap();
                        original.set_band_damping(2.0, 3.0).unwrap();
                        reverb.set_band_damping(2.0, 3.0).unwrap();
                        original.set_damping_shape(300.0, 1200.0, 1.0).unwrap();
                        reverb.set_damping_shape(300.0, 1200.0, 1.0).unwrap();
                        original
                            .set_delay_modulation(DelayMode::RandomAllpass, 0.5, 0.7)
                            .unwrap();
                        reverb
                            .set_delay_modulation(DelayMode::RandomAllpass, 0.5, 0.7)
                            .unwrap();
                    }
                    let input = if i % (sample_rate / 4) == 0 { 1.0 } else { 0.0 };
                    assert_eq!(reverb.process_sample(input), original.process_sample(input));
                }
                reverb.reset();
                original.reset();
                original.snap_parameters();
                for i in 0..sample_rate / 4 {
                    let input = if i == 0 { 1.0 } else { 0.0 };
                    assert_eq!(reverb.process_sample(input), original.process_sample(input));
                }
            }
        }
    }
}
