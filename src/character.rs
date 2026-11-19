//! Character banks are allocated at construction and stay warm during switching.
use crate::{Config, ConfigError, DelayMode, Matrix, ReverbTank};

/// Structural voicings, not simulations of specific rooms or plate hardware.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(usize)]
pub enum Character {
    #[default]
    Classic,
    Room,
    Chamber,
    Plate,
}

impl Character {
    pub const ALL: [Self; 4] = [Self::Classic, Self::Room, Self::Chamber, Self::Plate];

    pub fn name(self) -> &'static str {
        match self {
            Self::Classic => "Classic",
            Self::Room => "Room",
            Self::Chamber => "Chamber",
            Self::Plate => "Plate",
        }
    }

    pub(crate) fn lengths(self) -> [usize; 8] {
        match self {
            Self::Classic => [1493, 1601, 1867, 2053, 2251, 2399, 2791, 3011],
            Self::Room => [431, 571, 733, 887, 1061, 1217, 1429, 1699],
            Self::Chamber => [809, 1151, 1511, 1999, 2539, 3203, 4001, 4783],
            Self::Plate => [977, 1097, 1223, 1361, 1481, 1613, 1753, 1901],
        }
    }

    pub(crate) fn diffuser(self) -> ([usize; 4], f32) {
        match self {
            Self::Classic => ([149, 211, 263, 293], 0.6),
            Self::Room => ([43, 79, 113, 163], 0.45),
            Self::Chamber => ([173, 307, 449, 631], 0.65),
            Self::Plate => ([113, 173, 241, 337], 0.72),
        }
    }

    pub(crate) fn early_taps(self) -> [(f32, [f32; 2]); 6] {
        let times = match self {
            Self::Classic => [1.0; 6],
            Self::Room => [3.1, 5.7, 9.3, 13.7, 18.1, 24.3],
            Self::Chamber => [11.3, 19.7, 31.1, 43.7, 59.3, 79.1],
            Self::Plate => [2.3, 3.7, 5.3, 7.1, 11.3, 17.9],
        };
        let level = match self {
            Self::Classic => 0.0,
            Self::Room => 0.16,
            Self::Chamber => 0.11,
            Self::Plate => 0.045,
        };
        let gains = [
            [1.0, 0.2],
            [0.3, 0.9],
            [-0.65, 0.25],
            [0.2, -0.6],
            [0.45, 0.15],
            [-0.1, 0.4],
        ];
        std::array::from_fn(|i| (times[i], gains[i].map(|x| x * level)))
    }

    fn diffusion_floor(self) -> (f32, f32) {
        match self {
            Self::Classic => (0.0, 0.0),
            Self::Room => (0.2, 0.1),
            Self::Chamber => (0.45, 0.25),
            Self::Plate => (0.85, 0.65),
        }
    }
}

/// Mono-in, wet-only stereo reverb. Four preallocated, continuously warm tanks
/// allow tail-preserving Character automation without allocation or state resets.
/// This trades roughly four tanks' DSP/memory cost for predictable switching.
pub struct Reverb {
    pub(crate) tanks: [ReverbTank; 4],
    weights: [f32; 4],
    character: Character,
    smoothing: f32,
}

impl Reverb {
    pub fn new(config: Config) -> Result<Self, ConfigError> {
        // Validate before allocating any tank.
        config.validate()?;
        let mut tanks = Character::ALL.map(|character| {
            ReverbTank::new_character(config, character).expect("validated config")
        });
        for (tank, character) in tanks.iter_mut().zip(Character::ALL) {
            let (input, feedback) = character.diffusion_floor();
            tank.set_diffusion(input)?;
            tank.set_loop_diffusion(feedback)?;
            tank.snap_parameters();
        }
        Ok(Self {
            tanks,
            weights: [1.0, 0.0, 0.0, 0.0],
            character: Character::Classic,
            smoothing: 1.0 - (-1.0 / (0.05 * config.sample_rate as f32)).exp(),
        })
    }

    /// Crossfade the warm tanks; does not overwrite any other user parameter.
    pub fn set_character(&mut self, character: Character) {
        self.character = character;
    }

    pub fn set_parameters(
        &mut self,
        t60: f32,
        damping: f32,
        matrix: Matrix,
    ) -> Result<(), ConfigError> {
        for tank in &mut self.tanks {
            tank.set_parameters(t60, damping, matrix)?;
        }
        Ok(())
    }

    pub fn set_diffusion(&mut self, amount: f32) -> Result<(), ConfigError> {
        self.tanks[0].set_diffusion(amount)?;
        for (tank, character) in self.tanks.iter_mut().zip(Character::ALL).skip(1) {
            let floor = character.diffusion_floor().0;
            tank.set_diffusion(floor + (1.0 - floor) * amount)?;
        }
        Ok(())
    }

    pub fn set_loop_diffusion(&mut self, amount: f32) -> Result<(), ConfigError> {
        self.tanks[0].set_loop_diffusion(amount)?;
        for (tank, character) in self.tanks.iter_mut().zip(Character::ALL).skip(1) {
            let floor = character.diffusion_floor().1;
            tank.set_loop_diffusion(floor + (1.0 - floor) * amount)?;
        }
        Ok(())
    }

    pub fn set_band_damping(&mut self, low: f32, mid: f32) -> Result<(), ConfigError> {
        for tank in &mut self.tanks {
            tank.set_band_damping(low, mid)?;
        }
        Ok(())
    }

    pub fn set_damping_shape(&mut self, low: f32, mid: f32, q: f32) -> Result<(), ConfigError> {
        for tank in &mut self.tanks {
            tank.set_damping_shape(low, mid, q)?;
        }
        Ok(())
    }

    pub fn set_delay_modulation(
        &mut self,
        mode: DelayMode,
        depth: f32,
        rate: f32,
    ) -> Result<(), ConfigError> {
        for tank in &mut self.tanks {
            tank.set_delay_modulation(mode, depth, rate)?;
        }
        Ok(())
    }

    /// Clear all histories and start at current target parameters/Character.
    pub fn reset(&mut self) {
        for tank in &mut self.tanks {
            tank.reset();
            tank.snap_parameters();
        }
        self.weights = [0.0; 4];
        self.weights[self.character as usize] = 1.0;
    }

    /// No allocation, locking, or system calls. Input must be finite.
    pub fn process_sample(&mut self, input: f32) -> [f32; 2] {
        let mut output = [0.0; 2];
        for (i, weight) in self.weights.iter_mut().enumerate() {
            let target = if i == self.character as usize {
                1.0
            } else {
                0.0
            };
            *weight += self.smoothing * (target - *weight);
        }
        if self.weights.iter().enumerate().all(|(i, w)| {
            (*w - if i == self.character as usize {
                1.0
            } else {
                0.0
            })
            .abs()
                < 1e-3
        }) {
            self.weights = [0.0; 4];
            self.weights[self.character as usize] = 1.0;
        }
        let sum: f32 = self.weights.iter().sum();
        for (tank, weight) in self.tanks.iter_mut().zip(&mut self.weights) {
            *weight /= sum;
            let frame = tank.process_sample(input);
            if *weight != 0.0 {
                for ch in 0..2 {
                    output[ch] += *weight * frame[ch];
                }
            }
        }
        output
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classic_matches_original_tank_sample_for_sample() {
        let config = Config::default();
        let mut original = ReverbTank::new(config).unwrap();
        let mut bank = Reverb::new(config).unwrap();
        for i in 0..48_000 {
            let input = if i == 0 { 1.0 } else { 0.0 };
            assert_eq!(bank.process_sample(input), original.process_sample(input));
        }
    }

    #[test]
    fn switching_uses_warm_tail_and_preserves_controls() {
        let mut bank = Reverb::new(Config::default()).unwrap();
        bank.set_parameters(0.7, 3000.0, Matrix::Hybrid).unwrap();
        bank.set_diffusion(0.3).unwrap();
        bank.set_loop_diffusion(0.2).unwrap();
        bank.reset();
        for i in 0..4800 {
            bank.process_sample(if i == 0 { 1.0 } else { 0.0 });
        }
        let targets = bank.tanks.each_ref().map(|tank| tank.delays[0].target_gain);
        bank.set_character(Character::Plate);
        let mut energy = 0.0;
        for _ in 0..48_000 {
            let frame = bank.process_sample(0.0);
            energy += frame.iter().map(|x| x * x).sum::<f32>();
            assert!((bank.weights.iter().sum::<f32>() - 1.0).abs() < 1e-6);
            assert!(bank.weights.iter().all(|w| *w >= 0.0 && *w <= 1.0));
        }
        assert!(energy > 1e-5);
        assert_eq!(bank.weights, [0.0, 0.0, 0.0, 1.0]);
        assert_eq!(
            targets,
            bank.tanks.each_ref().map(|tank| tank.delays[0].target_gain)
        );
        bank.reset();
        for _ in 0..4800 {
            assert_eq!(bank.process_sample(0.0), [0.0; 2]);
        }
    }
}
