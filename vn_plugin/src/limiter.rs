//! Zero-latency stereo-linked sample-peak safety limiter, not a loudness matcher.
//! Instant attack can distort transients. No true-peak/intersample guarantee.
pub const CEILING: f32 = 0.891_250_9; // -1 dBFS

pub struct Limiter {
    gain: f32,
    release: f32,
}

impl Default for Limiter {
    fn default() -> Self {
        Self {
            gain: 1.0,
            release: 0.0,
        }
    }
}

impl Limiter {
    pub fn configure(&mut self, rate: u32) {
        self.release = 1.0 - (-1.0 / (0.1 * rate as f32)).exp();
        self.reset();
    }

    pub fn reset(&mut self) {
        self.gain = 1.0;
    }

    pub fn process(&mut self, frame: [f32; 2], enabled: bool) -> [f32; 2] {
        // Numerical safety must remain active even when peak limiting is off.
        let frame = frame.map(|v| if v.is_finite() { v } else { 0.0 });
        if !enabled {
            self.reset();
            return frame;
        }
        let peak = frame[0].abs().max(frame[1].abs());
        let required = if peak > CEILING { CEILING / peak } else { 1.0 };
        self.gain = (self.gain + self.release * (1.0 - self.gain)).min(required);
        frame.map(|v| v * self.gain)
    }

    pub fn gain(&self) -> f32 {
        self.gain
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalid_samples_are_safe_with_limiting_disabled_and_do_not_latch_silence() {
        for enabled in [false, true] {
            let mut limiter = Limiter::default();
            limiter.configure(48000);
            assert_eq!(
                limiter.process([f32::NAN, f32::INFINITY], enabled),
                [0.0; 2]
            );
            assert_eq!(limiter.process([0.1, -0.2], enabled), [0.1, -0.2]);
        }
    }

    #[test]
    fn caps_peaks_preserves_stereo_ratio_and_releases() {
        for rate in [8000, 44100, 192000] {
            let mut limiter = Limiter::default();
            limiter.configure(rate);
            assert_eq!(limiter.process([0.1, -0.2], true), [0.1, -0.2]);
            let out = limiter.process([20.0, -10.0], true);
            assert!(out[0] <= CEILING + 1e-7);
            assert_eq!(out[0], -2.0 * out[1]);
            for _ in 0..rate {
                limiter.process([0.0; 2], true);
            }
            assert!(limiter.gain() > 0.999);
            assert_eq!(limiter.process([20.0; 2], false), [20.0; 2]);
            assert_eq!(limiter.gain(), 1.0);
            limiter.process([20.0; 2], true);
            limiter.reset();
            assert_eq!(limiter.gain(), 1.0);
            assert!(
                limiter
                    .process([f32::NAN, f32::INFINITY], true)
                    .iter()
                    .all(|x| x.is_finite())
            );
        }
    }
}
