//! Live Classic branch. Construction allocates; update/reset/process do not.
#[cfg(test)]
use algo_reverb::{Config, ConfigError, Reverb};
use algo_reverb::{DelayMode, Matrix};
use nice_plug::prelude::*;

#[derive(Enum, Clone, Copy, PartialEq, Eq)]
pub enum MatrixMode {
    #[id = "householder"]
    Householder,
    #[id = "hadamard"]
    Hadamard,
    #[id = "hybrid"]
    Hybrid,
}
impl From<MatrixMode> for Matrix {
    fn from(value: MatrixMode) -> Self {
        match value {
            MatrixMode::Householder => Self::Householder,
            MatrixMode::Hadamard => Self::Hadamard,
            MatrixMode::Hybrid => Self::Hybrid,
        }
    }
}

#[derive(Enum, Clone, Copy, PartialEq, Eq)]
pub enum DelayReadMode {
    #[id = "static"]
    Static,
    #[id = "random_linear"]
    #[name = "Random / Linear"]
    RandomLinear,
    #[id = "random_allpass"]
    #[name = "Random / Allpass"]
    RandomAllpass,
}
impl From<DelayReadMode> for DelayMode {
    fn from(value: DelayReadMode) -> Self {
        match value {
            DelayReadMode::Static => Self::Static,
            DelayReadMode::RandomLinear => Self::RandomLinear,
            DelayReadMode::RandomAllpass => Self::RandomAllpass,
        }
    }
}

#[derive(Params)]
pub struct FdnParams {
    #[id = "fdn_balance"]
    pub balance: FloatParam,
    #[id = "fdn_matrix"]
    pub matrix: EnumParam<MatrixMode>,
    #[id = "fdn_t60"]
    pub t60: FloatParam,
    #[id = "fdn_diffusion"]
    pub diffusion: FloatParam,
    #[id = "fdn_loop_diffusion"]
    pub loop_diffusion: FloatParam,
    #[id = "fdn_damping"]
    pub damping: FloatParam,
    #[id = "fdn_damping_low"]
    pub damping_low: FloatParam,
    #[id = "fdn_damping_mid"]
    pub damping_mid: FloatParam,
    #[id = "fdn_damping_low_hz"]
    pub damping_low_hz: FloatParam,
    #[id = "fdn_damping_mid_hz"]
    pub damping_mid_hz: FloatParam,
    #[id = "fdn_damping_mid_q"]
    pub damping_mid_q: FloatParam,
    #[id = "fdn_delay_mode"]
    pub delay_mode: EnumParam<DelayReadMode>,
    #[id = "fdn_mod_depth"]
    pub mod_depth: FloatParam,
    #[id = "fdn_mod_rate"]
    pub mod_rate: FloatParam,
}

fn linear(name: &str, value: f32, min: f32, max: f32) -> FloatParam {
    FloatParam::new(name, value, FloatRange::Linear { min, max })
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
impl Default for FdnParams {
    fn default() -> Self {
        Self {
            balance: linear("Legacy Balance (unused)", 0.0, 0.0, 1.0).with_value_to_string(
                std::sync::Arc::new(|v| {
                    format!("IR {:.0}% / FDN {:.0}%", (1.0 - v) * 100.0, v * 100.0)
                }),
            ),
            matrix: EnumParam::new("FDN Matrix", MatrixMode::Hybrid),
            t60: FloatParam::new(
                "FDN T60",
                2.5,
                FloatRange::Skewed {
                    min: 0.01,
                    max: 60.0,
                    factor: FloatRange::skew_factor(-2.0),
                },
            )
            .with_unit(" s"),
            diffusion: linear("FDN Input Diffusion", 0.0, 0.0, 1.0),
            loop_diffusion: linear("FDN Loop Diffusion", 0.0, 0.0, 1.0),
            damping: frequency("FDN High Damping", 6000.0, 20.0, 20000.0),
            damping_low: linear("FDN Low Damping", 0.0, 0.0, 24.0).with_unit(" dB"),
            damping_mid: linear("FDN Mid Damping", 0.0, 0.0, 24.0).with_unit(" dB"),
            damping_low_hz: frequency("FDN Low Frequency", 250.0, 20.0, 2000.0),
            damping_mid_hz: frequency("FDN Mid Frequency", 1000.0, 100.0, 12000.0),
            damping_mid_q: linear("FDN Mid Q", std::f32::consts::FRAC_1_SQRT_2, 0.2, 8.0),
            delay_mode: EnumParam::new("FDN Delay Mode", DelayReadMode::Static),
            mod_depth: linear("FDN Mod Depth", 0.2, 0.0, 1.0).with_unit(" ms"),
            mod_rate: linear("FDN Mod Rate", 0.3, 0.05, 2.0).with_unit(" Hz"),
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
struct Targets {
    t60: f32,
    damping: f32,
    matrix: Matrix,
    diffusion: f32,
    loop_diffusion: f32,
    low: f32,
    mid: f32,
    low_hz: f32,
    mid_hz: f32,
    q: f32,
    delay: DelayMode,
    depth: f32,
    rate: f32,
}
impl FdnParams {
    pub fn tail_controls(&self, rate: u32) -> algo_reverb::ir_synthesis::TailControls {
        let t = self.targets(rate);
        algo_reverb::ir_synthesis::TailControls {
            t60: t.t60,
            damping: t.damping,
            matrix: t.matrix,
            diffusion: t.diffusion,
            loop_diffusion: t.loop_diffusion,
            low: t.low,
            mid: t.mid,
            low_hz: t.low_hz,
            mid_hz: t.mid_hz,
            q: t.q,
            delay: t.delay,
            depth: t.depth,
            rate: t.rate,
        }
    }

    pub fn set_tail_controls(
        &self,
        t: algo_reverb::ir_synthesis::TailControls,
        setter: &ParamSetter,
    ) {
        macro_rules! set {
            ($param:expr, $value:expr) => {{
                setter.begin_set_parameter($param);
                setter.set_parameter($param, $value);
                setter.end_set_parameter($param);
            }};
        }
        set!(&self.t60, t.t60);
        set!(&self.damping, t.damping);
        set!(&self.diffusion, t.diffusion);
        set!(&self.loop_diffusion, t.loop_diffusion);
        set!(&self.damping_low, t.low);
        set!(&self.damping_mid, t.mid);
        set!(&self.damping_low_hz, t.low_hz);
        set!(&self.damping_mid_hz, t.mid_hz);
        set!(&self.damping_mid_q, t.q);
        set!(&self.mod_depth, t.depth);
        set!(&self.mod_rate, t.rate);
        set!(
            &self.matrix,
            match t.matrix {
                Matrix::Householder => MatrixMode::Householder,
                Matrix::Hadamard => MatrixMode::Hadamard,
                Matrix::Hybrid => MatrixMode::Hybrid,
            }
        );
        set!(
            &self.delay_mode,
            match t.delay {
                DelayMode::Static => DelayReadMode::Static,
                DelayMode::RandomLinear => DelayReadMode::RandomLinear,
                DelayMode::RandomAllpass => DelayReadMode::RandomAllpass,
            }
        );
    }

    fn targets(&self, rate: u32) -> Targets {
        Targets {
            t60: self.t60.value(),
            damping: self.damping.value().min(rate as f32 * 0.5),
            matrix: self.matrix.value().into(),
            diffusion: self.diffusion.value(),
            loop_diffusion: self.loop_diffusion.value(),
            low: self.damping_low.value(),
            mid: self.damping_mid.value(),
            low_hz: self.damping_low_hz.value(),
            mid_hz: self.damping_mid_hz.value(),
            q: self.damping_mid_q.value(),
            delay: self.delay_mode.value().into(),
            depth: self.mod_depth.value(),
            rate: self.mod_rate.value(),
        }
    }
}
#[cfg(test)]
impl Targets {
    fn apply(self, reverb: &mut Reverb) -> Result<(), ConfigError> {
        reverb.set_parameters(self.t60, self.damping, self.matrix)?;
        reverb.set_diffusion(self.diffusion)?;
        reverb.set_loop_diffusion(self.loop_diffusion)?;
        reverb.set_band_damping(self.low, self.mid)?;
        reverb.set_damping_shape(self.low_hz, self.mid_hz, self.q)?;
        reverb.set_delay_modulation(self.delay, self.depth, self.rate)
    }
}

#[cfg(test)]
pub struct LiveFdn {
    reverb: Reverb,
    targets: Targets,
    sample_rate: u32,
    balance: f32,
    target_balance: f32,
    smoothing: f32,
    // Retain the largest decay used since reset, even when automation shortens T60.
    tail_frames: u32,
}
#[cfg(test)]
impl LiveFdn {
    pub fn new(sample_rate: u32, params: &FdnParams) -> Result<Self, ConfigError> {
        let targets = params.targets(sample_rate);
        let mut reverb = Reverb::new(Config {
            sample_rate,
            t60_seconds: targets.t60,
            damping_hz: targets.damping,
            matrix: targets.matrix,
        })?;
        targets.apply(&mut reverb)?;
        reverb.reset();
        Ok(Self {
            reverb,
            targets,
            sample_rate,
            balance: params.balance.value(),
            target_balance: params.balance.value(),
            smoothing: 1.0 - (-1.0 / (0.02 * sample_rate as f32)).exp(),
            tail_frames: Self::tail(sample_rate, targets.t60),
        })
    }
    fn tail(rate: u32, t60: f32) -> u32 {
        // Nominal estimate, not an exact cutoff: two T60s plus diffusion allowance.
        ((2.0 * t60 + 2.0) * rate as f32).ceil() as u32
    }
    pub fn tail_frames(&self) -> u32 {
        self.tail_frames
    }
    pub fn update(&mut self, params: &FdnParams) {
        let next = params.targets(self.sample_rate);
        if next != self.targets {
            // Host values have the same validated ranges as Classic.
            let result = next.apply(&mut self.reverb);
            debug_assert!(result.is_ok());
            self.targets = next;
        }
        self.tail_frames = self.tail_frames.max(Self::tail(self.sample_rate, next.t60));
        self.target_balance = params.balance.value();
    }
    pub fn reset(&mut self, params: &FdnParams) {
        self.update(params);
        self.reverb.reset();
        self.balance = self.target_balance;
        self.tail_frames = Self::tail(self.sample_rate, self.targets.t60);
    }
    pub fn process(&mut self, input: [f32; 2], ir: [f32; 2]) -> [f32; 2] {
        let input = input.map(|v| if v.is_finite() { v } else { 0.0 });
        let mut fdn = self.reverb.process_sample(input[0] * 0.5 + input[1] * 0.5);
        // A limiter can hide NaN/Inf, but cannot repair poisoned feedback memory.
        // Clear only the live branch; preserve the IR, balance and host controls.
        if fdn.iter().any(|v| !v.is_finite()) {
            self.reverb.reset();
            fdn = [0.0; 2];
        }
        self.balance += self.smoothing * (self.target_balance - self.balance);
        if (self.target_balance - self.balance).abs() < 0.0001 {
            self.balance = self.target_balance;
        }
        blend(ir, fdn, self.balance)
    }
}
#[cfg(test)]
fn blend(ir: [f32; 2], fdn: [f32; 2], balance: f32) -> [f32; 2] {
    // Exact endpoints, without residual contribution from the other branch.
    if balance <= 0.0 {
        ir
    } else if balance >= 1.0 {
        fdn
    } else {
        std::array::from_fn(|ch| (1.0 - balance) * ir[ch] + balance * fdn[ch])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nice_plug::params::InternalParamMut;
    fn set<P: Param + InternalParamMut>(param: &P, value: P::Plain) {
        // Test-only host emulation; no concurrent access.
        unsafe {
            param._internal_set_plain_value(value);
        }
    }

    #[test]
    fn blend_has_exact_endpoints_and_linear_midpoint() {
        assert_eq!(blend([1.0, -2.0], [3.0, 4.0], 0.0), [1.0, -2.0]);
        assert_eq!(blend([1.0, -2.0], [3.0, 4.0], 1.0), [3.0, 4.0]);
        assert_eq!(blend([1.0, -2.0], [3.0, 4.0], 0.5), [2.0, 1.0]);
    }

    #[test]
    fn fdn_endpoint_matches_classic_at_multiple_rates_with_live_controls() {
        for rate in [8000, 44100, 88200, 192000] {
            let params = FdnParams::default();
            set(&params.balance, 1.0);
            let mut live = LiveFdn::new(rate, &params).unwrap();
            let mut reference = Reverb::new(Config {
                sample_rate: rate,
                t60_seconds: 2.5,
                damping_hz: 6000.0_f32.min(rate as f32 * 0.5),
                matrix: Matrix::Hybrid,
            })
            .unwrap();
            params.targets(rate).apply(&mut reference).unwrap();
            reference.reset();
            let mut energy = 0.0;
            for i in 0..rate / 2 {
                if i == rate / 4 {
                    set(&params.t60, 0.7);
                    set(&params.matrix, MatrixMode::Householder);
                    set(&params.diffusion, 0.7);
                    set(&params.loop_diffusion, 0.6);
                    set(&params.damping, 2000.0);
                    set(&params.damping_low, 3.0);
                    set(&params.damping_mid, 6.0);
                    set(&params.damping_low_hz, 500.0);
                    set(&params.damping_mid_hz, 2000.0);
                    set(&params.damping_mid_q, 1.2);
                    set(&params.delay_mode, DelayReadMode::RandomAllpass);
                    set(&params.mod_depth, 0.5);
                    set(&params.mod_rate, 0.8);
                    live.update(&params);
                    params.targets(rate).apply(&mut reference).unwrap();
                }
                let input = if i % (rate / 4) == 0 { 1.0 } else { 0.0 };
                let actual = live.process([input; 2], [0.75, -0.5]);
                assert_eq!(actual, reference.process_sample(input));
                assert!(actual.iter().all(|x| x.is_finite()));
                energy += actual[0] * actual[0] + actual[1] * actual[1];
            }
            assert!(energy > 0.0);
            live.reset(&params);
            for _ in 0..rate / 4 {
                assert_eq!(live.process([0.0; 2], [1.0; 2]), [0.0; 2]);
            }
        }
    }

    #[test]
    fn poisoned_feedback_recovers_without_host_reset() {
        for balance in [0.0, 0.5, 1.0] {
            let params = FdnParams::default();
            set(&params.balance, balance);
            let mut live = LiveFdn::new(8000, &params).unwrap();
            // Simulate a numerical fault inside the tank, beyond input sanitation.
            live.reverb.process_sample(f32::NAN);
            let mut recovered_energy = 0.0;
            for i in 0..16000 {
                let input = (i as f32 * 0.071).sin() * 0.2;
                let frame = live.process([input; 2], [0.25, -0.5]);
                assert!(frame.iter().all(|v| v.is_finite()));
                if balance == 0.0 {
                    assert_eq!(frame, [0.25, -0.5]);
                }
                if i > 8000 {
                    recovered_energy += frame[0].abs() + frame[1].abs();
                }
            }
            assert!(recovered_energy > 1.0);
            assert!(
                live.process([f32::NAN, f32::INFINITY], [0.0; 2])
                    .iter()
                    .all(|v| v.is_finite())
            );
        }
    }

    #[test]
    fn live_control_sweep_remains_bounded() {
        for rate in [8000, 44100, 88200, 192000] {
            let params = FdnParams::default();
            set(&params.balance, 1.0);
            let mut live = LiveFdn::new(rate, &params).unwrap();
            let mut random = 123456789_u32;
            for i in 0..rate * 8 {
                if i % 128 == 0 {
                    random ^= random << 13;
                    random ^= random >> 17;
                    random ^= random << 5;
                    let v = (random >> 8) as f32 / 16777215.0;
                    set(&params.diffusion, v);
                    set(&params.loop_diffusion, 1.0 - v);
                    set(&params.t60, 0.1 + 19.9 * v);
                    set(&params.damping_low, 24.0 * v);
                    set(&params.damping_mid, 24.0 * (1.0 - v));
                    set(&params.damping_low_hz, 20.0 + 1980.0 * v);
                    set(&params.damping_mid_hz, 100.0 + 11900.0 * v);
                    set(&params.damping_mid_q, 0.2 + 7.8 * v);
                    set(&params.mod_depth, v);
                    set(&params.mod_rate, 0.1 + 1.9 * v);
                    set(
                        &params.delay_mode,
                        match random % 3 {
                            0 => DelayReadMode::Static,
                            1 => DelayReadMode::RandomLinear,
                            _ => DelayReadMode::RandomAllpass,
                        },
                    );
                    live.update(&params);
                }
                let input = (i as f32 * 0.071).sin() * 0.2;
                let frame = live.process([input; 2], [0.0; 2]);
                assert!(
                    frame.iter().all(|x| x.is_finite() && x.abs() < 32.0),
                    "rate={rate} sample={i} output={frame:?}"
                );
            }
        }
    }

    #[test]
    fn ir_endpoint_is_unchanged_and_balance_smooths_to_exact_endpoints() {
        let params = FdnParams::default();
        let mut live = LiveFdn::new(48000, &params).unwrap();
        for _ in 0..20000 {
            assert_eq!(live.process([0.1, 0.3], [0.25, -0.75]), [0.25, -0.75]);
        }
        set(&params.balance, 1.0);
        live.update(&params);
        live.process([0.0; 2], [0.25, -0.75]);
        assert!(live.balance > 0.0 && live.balance < 0.01);
        for _ in 0..20000 {
            live.process([0.0; 2], [0.25, -0.75]);
        }
        assert_eq!(live.balance, 1.0);
        set(&params.balance, 0.0);
        live.update(&params);
        for _ in 0..20000 {
            live.process([0.0; 2], [0.25, -0.75]);
        }
        assert_eq!(live.balance, 0.0);
    }

    #[test]
    fn tail_retains_previous_decay_until_reset_and_mid_balance_is_linear() {
        let params = FdnParams::default();
        set(&params.balance, 0.5);
        let mut half = LiveFdn::new(48000, &params).unwrap();
        set(&params.balance, 1.0);
        let mut full = LiveFdn::new(48000, &params).unwrap();
        for i in 0..12000 {
            let input = if i == 0 { [1.0; 2] } else { [0.0; 2] };
            let ir = [0.25, -0.5];
            let fdn = full.process(input, ir);
            assert_eq!(half.process(input, ir), blend(ir, fdn, 0.5));
        }
        set(&params.t60, 20.0);
        full.update(&params);
        assert_eq!(full.tail_frames(), 42 * 48000);
        set(&params.t60, 0.1);
        full.update(&params);
        assert_eq!(full.tail_frames(), 42 * 48000);
        full.reset(&params);
        assert_eq!(full.tail_frames(), (2.2_f32 * 48000.0).ceil() as u32);
    }
}
