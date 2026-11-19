//! Synthesis-only phase-compensated sequential Linkwitz–Riley crossover bank.
//! Analysis octave filters deliberately remain unchanged.
//! Biquad coefficients/state are intentionally separate from the analyzer to
//! prevent synthesis-bank refactors from changing reference analysis filters.

#[derive(Clone, Copy)]
struct Biquad {
    b: [f64; 3],
    a: [f64; 2],
    state: [[f64; 2]; 2],
}

impl Biquad {
    fn new(rate: u32, hz: f64, high: bool) -> Self {
        let w = std::f64::consts::TAU * hz / f64::from(rate);
        let alpha = w.sin() / 2.0_f64.sqrt();
        let norm = 1.0 / (1.0 + alpha);
        let c = w.cos();
        let b = if high {
            [(1.0 + c) * 0.5, -(1.0 + c), (1.0 + c) * 0.5]
        } else {
            [(1.0 - c) * 0.5, 1.0 - c, (1.0 - c) * 0.5]
        };
        Self {
            b: b.map(|v| v * norm),
            a: [-2.0 * c * norm, (1.0 - alpha) * norm],
            state: [[0.0; 2]; 2],
        }
    }

    fn process(&mut self, frame: [f64; 2]) -> [f64; 2] {
        std::array::from_fn(|ch| {
            let x = frame[ch];
            let y = self.b[0] * x + self.state[ch][0];
            self.state[ch][0] = self.b[1] * x - self.a[0] * y + self.state[ch][1];
            self.state[ch][1] = self.b[2] * x - self.a[1] * y;
            y
        })
    }
}

struct Split {
    low: [Biquad; 2],
    high: [Biquad; 2],
}

/// Stateful counterpart of `filter()`, prepared outside the audio callback.
pub(super) struct StreamingBank {
    bands: Vec<(Vec<Split>, f64)>,
}

impl StreamingBank {
    pub(super) fn new(rate: u32, gains: &[f64]) -> Self {
        let edges = edges(rate);
        assert_eq!(gains.len(), edges.len() + 1);
        Self {
            bands: gains
                .iter()
                .map(|&gain| (edges.iter().map(|&hz| Split::new(rate, hz)).collect(), gain))
                .collect(),
        }
    }

    pub(super) fn reset(&mut self) {
        for (splits, _) in &mut self.bands {
            for split in splits {
                for filter in split.low.iter_mut().chain(&mut split.high) {
                    filter.state = [[0.0; 2]; 2];
                }
            }
        }
    }

    pub(super) fn process(&mut self, frame: [f64; 2]) -> [f64; 2] {
        let mut sum = [0.0; 2];
        for (band, (splits, gain)) in self.bands.iter_mut().enumerate() {
            let mut x = frame;
            for (i, split) in splits.iter_mut().enumerate() {
                let (low, high) = split.process(x);
                x = if i < band {
                    high
                } else if i == band {
                    low
                } else {
                    [low[0] + high[0], low[1] + high[1]]
                };
            }
            for ch in 0..2 {
                sum[ch] += x[ch] * *gain;
            }
        }
        sum
    }
}

impl Split {
    fn new(rate: u32, hz: f64) -> Self {
        Self {
            low: [Biquad::new(rate, hz, false); 2],
            high: [Biquad::new(rate, hz, true); 2],
        }
    }

    fn process(&mut self, frame: [f64; 2]) -> ([f64; 2], [f64; 2]) {
        let low = self.low[0].process(frame);
        let low = self.low[1].process(low);
        let high = self.high[0].process(frame);
        let high = self.high[1].process(high);
        (low, high)
    }
}

/// Includes a low-pass residual below the first octave and a high-pass residual
/// above the last. Their unmodified coherent sum has unity magnitude, not unity
/// phase. Independent VN bands need not have flat summed power at crossovers.
pub(super) fn edges(rate: u32) -> Vec<f64> {
    let mut edges = vec![125.0 / 2.0_f64.sqrt()];
    edges.extend(
        super::band_centers(rate)
            .into_iter()
            .map(|f| f * 2.0_f64.sqrt()),
    );
    edges
}

pub(super) fn filter(frames: &[[f64; 2]], rate: u32, band: usize) -> Vec<[f64; 2]> {
    let edges = edges(rate);
    assert!(band <= edges.len());
    let mut splits: Vec<_> = edges.iter().map(|&hz| Split::new(rate, hz)).collect();
    frames
        .iter()
        .map(|&frame| {
            let mut x = frame;
            for (i, split) in splits.iter_mut().enumerate() {
                let (low, high) = split.process(x);
                x = if i < band {
                    high
                } else if i == band {
                    low
                } else {
                    [low[0] + high[0], low[1] + high[1]]
                };
            }
            x
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir_analysis::energy;

    #[test]
    fn streaming_matches_offline_bank_and_reset() {
        let rate = 8000;
        let gains: Vec<_> = (0..=edges(rate).len())
            .map(|i| 0.5 + i as f64 * 0.2)
            .collect();
        let frames: Vec<_> = (0..4000)
            .map(|i| if i % 97 == 0 { [0.8, -0.3] } else { [0.0; 2] })
            .collect();
        let mut expected = vec![[0.0; 2]; frames.len()];
        for (band, gain) in gains.iter().enumerate() {
            for (sum, x) in expected.iter_mut().zip(filter(&frames, rate, band)) {
                for ch in 0..2 {
                    sum[ch] += x[ch] * gain;
                }
            }
        }
        let mut live = StreamingBank::new(rate, &gains);
        for _ in 0..2 {
            for (&x, want) in frames.iter().zip(&expected) {
                assert_eq!(live.process(x), *want);
            }
            live.reset();
        }
    }

    #[test]
    fn coherent_sum_is_flat_and_residuals_cover_spectrum() {
        let rate = 44100;
        for hz in [30.0, 125.0, 500.0, 2000.0, 8000.0, 16000.0, 21000.0] {
            let tone: Vec<_> = (0..rate)
                .map(|i| [(std::f64::consts::TAU * hz * f64::from(i) / f64::from(rate)).sin(); 2])
                .collect();
            let bands: Vec<_> = (0..=edges(rate).len())
                .map(|b| filter(&tone, rate, b))
                .collect();
            let sum: Vec<_> = (0..tone.len())
                .map(|i| [bands.iter().map(|b| b[i][0]).sum::<f64>(); 2])
                .collect();
            let power = |v: &[[f64; 2]]| v[rate as usize / 2..].iter().map(energy).sum::<f64>();
            assert!((power(&sum) / power(&tone) - 1.0).abs() < 0.003, "{hz}");
            if hz == 30.0 {
                assert!(power(&bands[0]) / power(&tone) > 0.9);
            }
            if hz == 16000.0 {
                assert!(power(bands.last().unwrap()) / power(&tone) > 0.8);
            }
        }
    }
}
