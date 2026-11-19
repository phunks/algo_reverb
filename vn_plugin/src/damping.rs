//! Independent IR-age attenuation and causal Wet filtering. No added buffering.
use nice_plug::prelude::*;
use std::f64::consts::{PI, TAU};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Settings {
    pub enabled: bool,
    pub high_hz: f32,
    pub high: f32,
    pub low: f32,
    pub mid: f32,
    pub low_hz: f32,
    pub mid_hz: f32,
    pub q: f32,
}
impl Settings {
    pub fn pack(self) -> [f32; 8] {
        [
            if self.enabled { 1.0 } else { 0.0 },
            self.high_hz,
            self.high,
            self.low,
            self.mid,
            self.low_hz,
            self.mid_hz,
            self.q,
        ]
    }
    pub fn unpack(v: [f32; 8]) -> Self {
        Self {
            enabled: v[0] == 1.0,
            high_hz: v[1].clamp(20.0, 20000.0),
            high: v[2].clamp(0.0, 120.0),
            low: v[3].clamp(0.0, 120.0),
            mid: v[4].clamp(0.0, 120.0),
            low_hz: v[5].clamp(20.0, 2000.0),
            mid_hz: v[6].clamp(100.0, 12000.0),
            q: v[7].clamp(0.2, 8.0),
        }
    }
}

#[derive(Params)]
pub struct DampingParams {
    #[id = "ir_damp_enable"]
    pub ir_enable: BoolParam,
    #[id = "ir_damp_high_hz"]
    pub ir_high_hz: FloatParam,
    #[id = "ir_damp_high"]
    pub ir_high: FloatParam,
    #[id = "ir_damp_low"]
    pub ir_low: FloatParam,
    #[id = "ir_damp_mid"]
    pub ir_mid: FloatParam,
    #[id = "ir_damp_low_hz"]
    pub ir_low_hz: FloatParam,
    #[id = "ir_damp_mid_hz"]
    pub ir_mid_hz: FloatParam,
    #[id = "ir_damp_q"]
    pub ir_q: FloatParam,
    #[id = "wet_damp_enable"]
    pub wet_enable: BoolParam,
    #[id = "wet_damp_high_hz"]
    pub wet_high_hz: FloatParam,
    #[id = "wet_damp_low"]
    pub wet_low: FloatParam,
    #[id = "wet_damp_mid"]
    pub wet_mid: FloatParam,
    #[id = "wet_damp_low_hz"]
    pub wet_low_hz: FloatParam,
    #[id = "wet_damp_mid_hz"]
    pub wet_mid_hz: FloatParam,
    #[id = "wet_damp_q"]
    pub wet_q: FloatParam,
}

fn frequency(name: &str, value: f32, min: f32, max: f32) -> FloatParam {
    FloatParam::new(
        name,
        value,
        FloatRange::Skewed {
            min,
            max,
            factor: FloatRange::skew_factor(-2.0),
        },
    )
    .with_unit(" Hz")
}
fn amount(name: &str, unit: &'static str) -> FloatParam {
    attenuation(name, unit, 24.0)
}
fn attenuation(name: &str, unit: &'static str, max: f32) -> FloatParam {
    FloatParam::new(name, 0.0, FloatRange::Linear { min: 0.0, max }).with_unit(unit)
}
fn q(name: &str) -> FloatParam {
    FloatParam::new(
        name,
        std::f32::consts::FRAC_1_SQRT_2,
        FloatRange::Linear { min: 0.2, max: 8.0 },
    )
}
impl Default for DampingParams {
    fn default() -> Self {
        Self {
            ir_enable: BoolParam::new("IR Time Damping", false),
            ir_high_hz: frequency("IR High Frequency", 6000.0, 20.0, 20000.0),
            ir_high: attenuation("IR High Decay", " dB/s", 120.0),
            ir_low: attenuation("IR Low Decay", " dB/s", 120.0),
            ir_mid: attenuation("IR Mid Decay", " dB/s", 120.0),
            ir_low_hz: frequency("IR Low Frequency", 250.0, 20.0, 2000.0),
            ir_mid_hz: frequency("IR Mid Frequency", 1000.0, 100.0, 12000.0),
            ir_q: q("IR Mid Q"),
            wet_enable: BoolParam::new("Wet Tone Filter", false),
            wet_high_hz: frequency("Wet High Damping", 6000.0, 20.0, 20000.0),
            wet_low: amount("Wet Low Damping", " dB"),
            wet_mid: amount("Wet Mid Damping", " dB"),
            wet_low_hz: frequency("Wet Low Frequency", 250.0, 20.0, 2000.0),
            wet_mid_hz: frequency("Wet Mid Frequency", 1000.0, 100.0, 12000.0),
            wet_q: q("Wet Mid Q"),
        }
    }
}
impl DampingParams {
    pub fn ir(&self) -> Settings {
        Settings {
            enabled: self.ir_enable.value(),
            high_hz: self.ir_high_hz.value(),
            high: self.ir_high.value(),
            low: self.ir_low.value(),
            mid: self.ir_mid.value(),
            low_hz: self.ir_low_hz.value(),
            mid_hz: self.ir_mid_hz.value(),
            q: self.ir_q.value(),
        }
    }
    pub fn wet(&self) -> Settings {
        Settings {
            enabled: self.wet_enable.value(),
            high_hz: self.wet_high_hz.value(),
            high: 0.0,
            low: self.wet_low.value(),
            mid: self.wet_mid.value(),
            low_hz: self.wet_low_hz.value(),
            mid_hz: self.wet_mid_hz.value(),
            q: self.wet_q.value(),
        }
    }
}

// Same passive LP-subtraction and constant-peak BP-subtraction shapes as FDN.
#[derive(Default)]
struct Branch {
    x: [f64; 2],
    y: [f64; 2],
}
impl Branch {
    fn process(&mut self, x: f64, b: [f64; 3], a: [f64; 2]) -> f64 {
        let y =
            b[0] * x + b[1] * self.x[0] + b[2] * self.x[1] - a[0] * self.y[0] - a[1] * self.y[1];
        self.x = [x, self.x[0]];
        self.y = [if y.abs() < 1e-25 { 0.0 } else { y }, self.y[0]];
        y
    }
}
fn lp(rate: u32, hz: f32) -> ([f64; 3], [f64; 2]) {
    let k = (PI * f64::from(hz).min(f64::from(rate) * 0.45) / f64::from(rate)).tan();
    let b = k / (1.0 + k);
    ([b, b, 0.0], [(k - 1.0) / (k + 1.0), 0.0])
}
fn bp(rate: u32, hz: f32, q: f32) -> ([f64; 3], [f64; 2]) {
    let w = TAU * f64::from(hz).min(f64::from(rate) * 0.45) / f64::from(rate);
    let alpha = w.sin() / (2.0 * f64::from(q));
    let n = 1.0 / (1.0 + alpha);
    (
        [alpha * n, 0.0, -alpha * n],
        [-2.0 * w.cos() * n, (1.0 - alpha) * n],
    )
}

// RBJ cut-only shelves (slope=1) and peaking EQ. Unlike branch subtraction,
// their stop-band/center gain continues towards silence as IR age increases.
// Coefficients are prepared off-thread; WetFilter retains its existing shapes.
fn decay_eq(rate: u32, hz: f32, q: f32, db: f64, band: usize) -> ([f64; 3], [f64; 2]) {
    let w = TAU * f64::from(hz).min(f64::from(rate) * 0.45) / f64::from(rate);
    let c = w.cos();
    let a = 10.0_f64.powf(-db.min(120.0) / 40.0);
    let alpha = w.sin() / (2.0 * f64::from(q));
    let beta = 2.0 * a.sqrt() * w.sin() * std::f64::consts::FRAC_1_SQRT_2;
    let (b, d) = match band {
        0 => (
            [
                a * (a + 1.0 - (a - 1.0) * c + beta),
                2.0 * a * (a - 1.0 - (a + 1.0) * c),
                a * (a + 1.0 - (a - 1.0) * c - beta),
            ],
            [
                a + 1.0 + (a - 1.0) * c + beta,
                -2.0 * (a - 1.0 + (a + 1.0) * c),
                a + 1.0 + (a - 1.0) * c - beta,
            ],
        ),
        1 => (
            [1.0 + alpha * a, -2.0 * c, 1.0 - alpha * a],
            [1.0 + alpha / a, -2.0 * c, 1.0 - alpha / a],
        ),
        _ => (
            [
                a * (a + 1.0 + (a - 1.0) * c + beta),
                -2.0 * a * (a - 1.0 + (a + 1.0) * c),
                a * (a + 1.0 + (a - 1.0) * c - beta),
            ],
            [
                a + 1.0 - (a - 1.0) * c + beta,
                2.0 * (a - 1.0 - (a + 1.0) * c),
                a + 1.0 - (a - 1.0) * c - beta,
            ],
        ),
    };
    (b.map(|v| v / d[0]), [d[1] / d[0], d[2] / d[0]])
}

/// Cut depth grows with IR age, not host playback time, capped at 120 dB.
/// The original IR is retained. This is not an FDN feedback-loop model.
pub fn render(ir: &[[f64; 2]], rate: u32, s: Settings) -> Vec<[f64; 2]> {
    if !s.enabled || [s.high, s.low, s.mid] == [0.0; 3] {
        return ir.to_vec();
    }
    let mut low: [Branch; 2] = std::array::from_fn(|_| Branch::default());
    let mut mid: [Branch; 2] = std::array::from_fn(|_| Branch::default());
    let mut high: [Branch; 2] = std::array::from_fn(|_| Branch::default());
    ir.iter()
        .enumerate()
        .map(|(i, frame)| {
            let t = i as f64 / f64::from(rate);
            let (lb, la) = decay_eq(rate, s.low_hz, s.q, f64::from(s.low) * t, 0);
            let (mb, ma) = decay_eq(rate, s.mid_hz, s.q, f64::from(s.mid) * t, 1);
            let (hb, ha) = decay_eq(rate, s.high_hz, s.q, f64::from(s.high) * t, 2);
            std::array::from_fn(|ch| {
                let x = frame[ch];
                let x = if s.low > 0.0 {
                    low[ch].process(x, lb, la)
                } else {
                    x
                };
                let x = if s.mid > 0.0 {
                    mid[ch].process(x, mb, ma)
                } else {
                    x
                };
                if s.high > 0.0 {
                    high[ch].process(x, hb, ha)
                } else {
                    x
                }
            })
        })
        .collect()
}

#[derive(Default)]
pub struct WetFilter {
    low: [Branch; 2],
    mid: [Branch; 2],
    high: [f64; 2],
    current: [f64; 7],
    target: [f64; 7],
    smoothing: f64,
    rate: u32,
    low_coeff: ([f64; 3], [f64; 2]),
    mid_coeff: ([f64; 3], [f64; 2]),
    high_coeff: f64,
    active_low: ([f64; 3], [f64; 2]),
    active_mid: ([f64; 3], [f64; 2]),
    active_high: f64,
}
impl WetFilter {
    pub fn tail_samples(&self) -> u32 {
        // Include filter ring-out and enable smoothing in the host's tail budget.
        if self.current[0] != 0.0 || self.target[0] != 0.0 {
            self.rate * 2
        } else {
            0
        }
    }
    pub fn reset(&mut self, rate: u32, s: Settings) {
        *self = Self::default();
        self.rate = rate;
        self.smoothing = 1.0 - (-1.0 / (0.02 * f64::from(rate))).exp();
        self.update(s);
        self.current = self.target;
        self.active_low = self.low_coeff;
        self.active_mid = self.mid_coeff;
        self.active_high = self.high_coeff;
    }
    pub fn update(&mut self, s: Settings) {
        self.target = [
            if s.enabled { 1.0 } else { 0.0 },
            f64::from(s.high_hz),
            f64::from(s.low),
            f64::from(s.mid),
            f64::from(s.low_hz),
            f64::from(s.mid_hz),
            f64::from(s.q),
        ];
        self.low_coeff = lp(self.rate, s.low_hz);
        self.mid_coeff = bp(self.rate, s.mid_hz, s.q);
        self.high_coeff = (-TAU * f64::from(s.high_hz).min(f64::from(self.rate) * 0.5)
            / f64::from(self.rate))
        .exp();
    }
    pub fn process(&mut self, input: [f32; 2]) -> [f32; 2] {
        // Smooth coefficients (not frequencies) as in the existing feedback filters.
        for i in [0, 2, 3] {
            self.current[i] += self.smoothing * (self.target[i] - self.current[i]);
            if (self.target[i] - self.current[i]).abs() < 1e-9 {
                self.current[i] = self.target[i];
            }
        }
        for i in 0..3 {
            self.active_low.0[i] += self.smoothing * (self.low_coeff.0[i] - self.active_low.0[i]);
            self.active_mid.0[i] += self.smoothing * (self.mid_coeff.0[i] - self.active_mid.0[i]);
        }
        for i in 0..2 {
            self.active_low.1[i] += self.smoothing * (self.low_coeff.1[i] - self.active_low.1[i]);
            self.active_mid.1[i] += self.smoothing * (self.mid_coeff.1[i] - self.active_mid.1[i]);
        }
        self.active_high += self.smoothing * (self.high_coeff - self.active_high);
        // Fixed-size coefficient/state storage: no audio-thread allocation.
        let amounts = [self.current[2], self.current[3]].map(|db| 1.0 - 10.0_f64.powf(-db / 20.0));
        std::array::from_fn(|ch| {
            let x = f64::from(input[ch]);
            let low = self.low[ch].process(x, self.active_low.0, self.active_low.1);
            let v = x - amounts[0] * low;
            let mid = self.mid[ch].process(v, self.active_mid.0, self.active_mid.1);
            let v = v - amounts[1] * mid;
            self.high[ch] = (1.0 - self.active_high) * v + self.active_high * self.high[ch];
            if self.high[ch].abs() < 1e-25 {
                self.high[ch] = 0.0;
            }
            if self.current[0] == 0.0 {
                input[ch]
            } else {
                (x + self.current[0] * (self.high[ch] - x)) as f32
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disabled_and_zero_decay_preserve_ir_exactly() {
        let p = DampingParams::default();
        let ir = vec![[0.7, -0.4], [0.2, 0.1], [0.0; 2]];
        assert_eq!(render(&ir, 48000, p.ir()), ir);
        assert_eq!(
            render(
                &ir,
                48000,
                Settings {
                    enabled: true,
                    ..p.ir()
                }
            ),
            ir
        );
        assert_eq!(Settings::unpack(p.ir().pack()), p.ir());
    }

    fn energy(ir: &[[f64; 2]], start: usize, end: usize) -> f64 {
        ir[start..end].iter().map(|x| x[0] * x[0]).sum::<f64>() / (end - start) as f64
    }

    #[test]
    fn decay_eq_reaches_requested_band_gain_without_subtraction_floor() {
        let magnitude = |b: [f64; 3], a: [f64; 2], w: f64| {
            let power = |c: [f64; 3]| {
                let re = c[0] + c[1] * w.cos() + c[2] * (2.0 * w).cos();
                let im = -c[1] * w.sin() - c[2] * (2.0 * w).sin();
                re * re + im * im
            };
            10.0 * (power(b) / power([1.0, a[0], a[1]])).log10()
        };
        for band in 0..3 {
            for cut in [12.0, 48.0, 96.0] {
                let (b, a) = decay_eq(48000, 1000.0, 0.7, cut, band);
                let w = match band {
                    0 => 0.0,
                    1 => TAU * 1000.0 / 48000.0,
                    _ => PI,
                };
                assert!((magnitude(b, a, w) + cut).abs() < 0.001);
                if band != 1 {
                    let passband = if band == 0 { PI } else { 0.0 };
                    assert!(magnitude(b, a, passband).abs() < 0.001);
                }
            }
        }
    }

    #[test]
    fn strong_decay_shortens_short_ir_in_each_band_without_cross_channel_leakage() {
        for rate in [8000, 48000, 192000] {
            let n = rate as usize;
            for (band, hz) in [(0, 40.0), (1, 1000.0), (2, 3000.0)] {
                let ir: Vec<_> = (0..n / 4)
                    .map(|i| [(TAU * hz * i as f64 / rate as f64).sin(), 0.0])
                    .collect();
                let mut s = DampingParams::default().ir();
                s.enabled = true;
                s.high_hz = 500.0;
                let with_amount = |mut s: Settings, amount| {
                    match band {
                        0 => s.low = amount,
                        1 => s.mid = amount,
                        _ => s.high = amount,
                    }
                    s
                };
                let mild = render(&ir, rate, with_amount(s, 24.0));
                s = with_amount(s, 120.0);
                assert_eq!(Settings::unpack(s.pack()), s);
                let strong = render(&ir, rate, s);
                let db =
                    10.0 * (energy(&strong, n / 10, n / 5) / energy(&mild, n / 10, n / 5)).log10();
                assert!(db < -8.0, "rate={rate} band={band}: {db} dB");
                assert_eq!(strong[0], ir[0]);
                assert!(strong.iter().all(|v| v[0].is_finite() && v[1] == 0.0));
            }
        }
    }

    #[test]
    fn deep_decay_is_finite_at_frequency_and_q_extremes() {
        for rate in [8000, 48000, 192000] {
            for (low_hz, mid_hz, high_hz, q) in
                [(20.0, 100.0, 20.0, 0.2), (2000.0, 12000.0, 20000.0, 8.0)]
            {
                let ir: Vec<_> = (0..rate as usize * 2)
                    .map(|i| [(i as f64 * 0.3).sin(), (i as f64 * 0.7).cos()])
                    .collect();
                let out = render(
                    &ir,
                    rate,
                    Settings {
                        enabled: true,
                        low: 120.0,
                        mid: 120.0,
                        high: 120.0,
                        low_hz,
                        mid_hz,
                        high_hz,
                        q,
                    },
                );
                assert!(out.iter().flatten().all(|v| v.is_finite() && v.abs() < 4.0));
                assert_eq!(out[0], ir[0]);
            }
        }
    }

    #[test]
    fn each_band_attenuates_later_ir_more_than_early_ir() {
        for rate in [8000, 48000, 192000] {
            let n = rate as usize;
            for (band, hz) in [(0, 40.0), (1, 1000.0), (2, 3000.0)] {
                let ir: Vec<_> = (0..n * 2)
                    .map(|i| [(TAU * hz * i as f64 / f64::from(rate)).sin(); 2])
                    .collect();
                let mut s = DampingParams::default().ir();
                s.enabled = true;
                s.high_hz = 500.0;
                match band {
                    0 => s.low = 12.0,
                    1 => s.mid = 12.0,
                    _ => s.high = 12.0,
                }
                let out = render(&ir, rate, s);
                assert_eq!(out[0], ir[0]);
                assert!(
                    energy(&out, n * 3 / 2, n * 2) < energy(&out, n / 10, n / 5) * 0.5,
                    "rate={rate} band={band}"
                );
                assert!(out.iter().flatten().all(|v| v.is_finite()));
            }
        }
    }

    #[test]
    fn wet_filter_is_causal_exactly_bypassed_and_retains_stereo_separation() {
        let p = DampingParams::default();
        let mut filter = WetFilter::default();
        filter.reset(48000, p.wet());
        assert_eq!(filter.process([0.7, -0.2]), [0.7, -0.2]);
        assert_eq!(filter.tail_samples(), 0);
        filter.reset(
            48000,
            Settings {
                enabled: true,
                high_hz: 500.0,
                ..p.wet()
            },
        );
        let first = filter.process([1.0, 0.0]);
        assert_eq!(filter.tail_samples(), 96000);
        assert!(first[0] > 0.0 && first[0] < 1.0);
        assert_eq!(first[1], 0.0);
        for i in 0..48000 {
            if i % 128 == 0 {
                filter.update(Settings {
                    enabled: i % 256 == 0,
                    low: 24.0,
                    mid: 24.0,
                    low_hz: if i % 256 == 0 { 20.0 } else { 2000.0 },
                    mid_hz: if i % 256 == 0 { 100.0 } else { 12000.0 },
                    q: 8.0,
                    ..p.wet()
                });
            }
            let out = filter.process([(i as f32 * 0.3).sin(), 0.0]);
            assert!(out[0].is_finite() && out[0].abs() < 4.0);
            assert_eq!(out[1], 0.0);
        }
        filter.update(p.wet());
        for _ in 0..48000 {
            filter.process([0.0; 2]);
        }
        assert_eq!(filter.process([0.2, -0.7]), [0.2, -0.7]);
    }
}
