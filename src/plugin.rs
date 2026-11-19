//! Live DiffFDN host adapter. Networks are prepared off the audio thread.

use std::sync::{
    Arc, Mutex,
    atomic::{AtomicU32, Ordering},
};

use egui_plugin as egui;
use nice_plug::prelude::*;
use nice_plug_core::context::gui::GuiContext;
use nice_plug_egui::{
    EguiEditor, EguiEditorState, EguiNiceSettings, Frame, NiceEguiApp, RepaintNotifier,
    create_egui_editor,
    widgets::{ParamButton, ParamComboBox, ParamEnumButton},
};

use crate::{DelayMode, Matrix, difffdn::DiffFdn};

mod dual_knob;
#[path = "../vn_plugin/src/file_browser.rs"]
mod file_browser;
#[path = "../vn_plugin/src/limiter.rs"]
#[allow(dead_code)] // Shared limiter also exposes a gain meter for Algo VN.
mod live_limiter;
mod peak_space;
mod seed_import;
mod slice_repeat;

#[derive(Enum, Clone, Copy, PartialEq, Eq, Debug)]
enum RoomPreset {
    #[id = "salon"]
    #[name = "French Salon"]
    Salon,
    #[id = "damped"]
    #[name = "Damped Large Room"]
    Damped,
}

fn room_text(room: RoomPreset) -> &'static str {
    match room {
        RoomPreset::Salon => include_str!("../resources/difffdn/salon.fdn"),
        RoomPreset::Damped => include_str!("../resources/difffdn/damped.fdn"),
    }
}

struct LiveDiffFdn {
    rooms: [peak_space::Space; 2],
    transition_samples: u32,
    position: u32,
}

// Fixed, wet-only reference trims: approximately unity RMS at 48 kHz with
// default controls and the log-spaced multitone regression below. Not AGC or
// a perceptual loudness guarantee; feedback edits and sample rate affect gain.
const DIFFFDN_WET_GAINS: [f64; 2] = [0.053_703_179_637_025_27, 0.084_139_514_164_519_51]; // -25.4 / -21.5 dB

fn difffdn_mix_gains(wet: f32) -> (f32, f32) {
    if wet <= 0.0 {
        return (1.0, 0.0);
    }
    if wet >= 1.0 {
        return (0.0, 1.0);
    }
    let (wet, dry) = (wet * std::f32::consts::FRAC_PI_2).sin_cos();
    (dry, wet)
}

impl LiveDiffFdn {
    fn new(rate: u32, room: RoomPreset) -> Option<Self> {
        let load = |r| {
            let load = || {
                DiffFdn::parse(room_text(r))
                    .and_then(|f| f.at_sample_rate(rate))
                    .ok()
            };
            Some(peak_space::Space::new([load()?, load()?]))
        };
        let transition_samples = (0.15 * rate as f64).round() as u32;
        Some(Self {
            rooms: [load(RoomPreset::Salon)?, load(RoomPreset::Damped)?],
            transition_samples,
            position: if room == RoomPreset::Salon {
                0
            } else {
                transition_samples
            },
        })
    }
    fn reset(&mut self, room: RoomPreset) {
        for f in &mut self.rooms {
            f.reset();
        }
        self.position = if room == RoomPreset::Salon {
            0
        } else {
            self.transition_samples
        };
    }
    fn process(&mut self, input: f32, room: RoomPreset) -> [f32; 2] {
        // Inactive networks are cleared at fade completion, so restarting them
        // never replays a frozen tail. Reversal preserves both running states.
        let target = if room == RoomPreset::Salon {
            0
        } else {
            self.transition_samples
        };
        if self.position == target {
            let index = usize::from(room == RoomPreset::Damped);
            let wet =
                (DIFFFDN_WET_GAINS[index] * self.rooms[index].process_sample(input as f64)) as f32;
            return [wet; 2];
        }
        if self.position < target {
            self.position += 1;
        } else {
            self.position -= 1;
        }
        let blend = self.position as f64 / self.transition_samples as f64;
        let a = self.rooms[0].process_sample(input as f64);
        let b = self.rooms[1].process_sample(input as f64);
        let wet =
            ((1.0 - blend) * DIFFFDN_WET_GAINS[0] * a + blend * DIFFFDN_WET_GAINS[1] * b) as f32;
        if self.position == target {
            self.rooms[usize::from(room == RoomPreset::Salon)].reset();
        }
        [wet; 2]
    }
}

fn damping_frequency(name: &str, default: f32, min: f32, max: f32) -> FloatParam {
    FloatParam::new(
        name,
        default,
        FloatRange::Skewed {
            min,
            max,
            factor: FloatRange::skew_factor(-2.0),
        },
    )
    .with_unit(" Hz")
    .with_value_to_string(Arc::new(|value| format!("{value:.0}")))
}

#[derive(Enum, Clone, Copy, PartialEq, Eq)]
enum DelayReadMode {
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
    fn from(mode: DelayReadMode) -> Self {
        match mode {
            DelayReadMode::Static => Self::Static,
            DelayReadMode::RandomLinear => Self::RandomLinear,
            DelayReadMode::RandomAllpass => Self::RandomAllpass,
        }
    }
}

#[derive(Enum, Clone, Copy, PartialEq, Eq)]
enum MatrixMode {
    #[id = "householder"]
    Householder,
    #[id = "hadamard"]
    Hadamard,
    #[id = "hybrid"]
    #[name = "Householder -> Hadamard"]
    Hybrid,
}

impl From<MatrixMode> for Matrix {
    fn from(mode: MatrixMode) -> Self {
        match mode {
            MatrixMode::Householder => Self::Householder,
            MatrixMode::Hadamard => Self::Hadamard,
            MatrixMode::Hybrid => Self::Hybrid,
        }
    }
}

#[derive(Enum, Clone, Copy, PartialEq, Eq)]
enum DuckSource {
    #[id = "internal"]
    Internal,
    #[id = "external"]
    External,
}

#[derive(Enum, Clone, Copy, PartialEq, Eq)]
enum DuckDetection {
    #[id = "level"]
    Level,
    #[id = "transient"]
    Transient,
}

/// Peak-envelope novelty detector. Its held target feeds the existing gain
/// envelope, so Attack and Release retain their original time-constant meaning.
#[derive(Default)]
struct DuckTransient {
    fast: f32,
    slow: f32,
    armed: bool,
    remaining: u32,
    held: f32,
}

impl DuckTransient {
    fn detect(&mut self, level: f32, threshold: f32, amount: f32, timing: &DuckTiming) -> f32 {
        self.fast = level.max(self.fast * (1.0 - timing.fast));
        let novelty = (self.fast - self.slow).max(0.0);
        // A relative floor rejects ordinary waveform ripple on sustained bass
        // notes, even when the absolute Threshold is set very low.
        let onset_threshold = threshold.max(self.slow * 0.5);
        let slow_speed = if self.fast > self.slow {
            timing.slow_attack
        } else {
            timing.slow_release
        };
        self.slow += slow_speed * (self.fast - self.slow);
        if novelty < onset_threshold * 0.5 {
            self.armed = true;
        }
        if self.armed && novelty > onset_threshold {
            self.armed = false;
            self.remaining = timing.hold;
            self.held = 0.0;
        }
        if self.remaining > 0 {
            self.held = self
                .held
                .max((20.0 * (novelty.max(1e-20) / threshold).log10()).clamp(0.0, amount));
            self.remaining -= 1;
            self.held.min(amount)
        } else {
            0.0
        }
    }
}

struct DuckTiming {
    fast: f32,
    slow_attack: f32,
    slow_release: f32,
    hold: u32,
}

#[derive(Params)]
struct ReverbParams {
    #[persist = "seed_wav_location_v1"]
    seed_wav_location: Mutex<(std::path::PathBuf, std::path::PathBuf)>,
    #[persist = "rust_stereo_seed_v1"]
    seed_state: Mutex<(String, String)>,
    #[id = "matrix"]
    matrix: EnumParam<MatrixMode>,
    #[id = "diffusion"]
    diffusion: FloatParam,
    #[id = "t60"]
    t60: FloatParam,
    #[id = "damping"]
    damping: FloatParam,
    #[id = "damping_low"]
    damping_low: FloatParam,
    #[id = "damping_mid"]
    damping_mid: FloatParam,
    #[id = "wet"]
    wet: FloatParam,
    #[id = "output_db"]
    output_db: FloatParam,
    #[id = "bypass"]
    bypass: BoolParam,
    #[id = "duck_enabled"]
    duck_enabled: BoolParam,
    #[id = "duck_source"]
    duck_source: EnumParam<DuckSource>,
    #[id = "duck_threshold"]
    duck_threshold: FloatParam,
    #[id = "duck_amount"]
    duck_amount: FloatParam,
    #[id = "duck_attack"]
    duck_attack: FloatParam,
    #[id = "duck_release"]
    duck_release: FloatParam,
    #[id = "loop_diffusion"]
    loop_diffusion: FloatParam,
    #[id = "delay_mode"]
    delay_mode: EnumParam<DelayReadMode>,
    #[id = "mod_depth"]
    mod_depth: FloatParam,
    #[id = "mod_rate"]
    mod_rate: FloatParam,
    #[id = "damping_low_hz"]
    damping_low_hz: FloatParam,
    #[id = "damping_mid_hz"]
    damping_mid_hz: FloatParam,
    #[id = "damping_mid_q"]
    damping_mid_q: FloatParam,
    #[id = "duck_detection"]
    duck_detection: EnumParam<DuckDetection>,
    #[id = "difffdn_room"]
    room: EnumParam<RoomPreset>,
    #[id = "difffdn_damping"]
    extra_damping: FloatParam,
    #[id = "seed_enabled"]
    seed_enabled: BoolParam,
    #[id = "space_enabled"]
    space_enabled: BoolParam,
    #[id = "space_threshold"]
    space_threshold: FloatParam,
    #[id = "space_decay"]
    space_decay: FloatParam,
    #[id = "space_swell"]
    space_swell: FloatParam,
    #[id = "space_release"]
    space_release: FloatParam,
    #[id = "space_steps"]
    space_steps: FloatParam,
    #[id = "repeat_enabled"]
    repeat_enabled: BoolParam,
    #[id = "repeat_mode"]
    repeat_mode: EnumParam<slice_repeat::Mode>,
    #[id = "repeat_division"]
    repeat_division: EnumParam<slice_repeat::Division>,
    #[id = "repeat_nudge"]
    repeat_nudge: FloatParam,
    #[id = "repeat_feedback"]
    repeat_feedback: FloatParam,
    #[id = "repeat_mix"]
    repeat_mix: FloatParam,
    #[id = "repeat_high_cut"]
    repeat_high_cut: FloatParam,
    #[id = "repeat_high_pass"]
    repeat_high_pass: FloatParam,
}

fn repeat_frequency(name: &'static str, default: f32, min: f32, max: f32) -> FloatParam {
    FloatParam::new(
        name,
        default,
        FloatRange::Skewed {
            min,
            max,
            factor: FloatRange::skew_factor(-2.),
        },
    )
    .with_unit(" Hz")
    .with_value_to_string(Arc::new(|v| format!("{v:.0}")))
}

fn space_time(name: &'static str, default: f32) -> FloatParam {
    FloatParam::new(
        name,
        default,
        FloatRange::Skewed {
            min: 10.,
            max: 3000.,
            factor: FloatRange::skew_factor(-2.),
        },
    )
    .with_unit(" ms")
    .with_value_to_string(Arc::new(|v| format!("{v:.0}")))
}

impl Default for ReverbParams {
    fn default() -> Self {
        Self {
            repeat_enabled: BoolParam::new("Slice Repeat", false),
            repeat_mode: EnumParam::new("Repeat Mode", slice_repeat::Mode::Space),
            repeat_division: EnumParam::new("Division", slice_repeat::Division::Eighth),
            repeat_nudge: FloatParam::new(
                "Groove Offset",
                0.,
                FloatRange::Linear {
                    min: -50.,
                    max: 50.,
                },
            )
            .with_unit(" ms")
            .with_value_to_string(Arc::new(|v| format!("{v:+.2}"))),
            repeat_feedback: FloatParam::new(
                "Feedback",
                0.6,
                FloatRange::Linear { min: 0., max: 0.95 },
            )
            .with_value_to_string(Arc::new(|v| format!("{:.0}%", v * 100.))),
            repeat_mix: FloatParam::new(
                "Repeat Mix",
                0.65,
                FloatRange::Linear { min: 0., max: 1. },
            )
            .with_value_to_string(Arc::new(|v| format!("{:.0}%", v * 100.))),
            repeat_high_cut: repeat_frequency("High Cut", 6000., 500., 20000.),
            repeat_high_pass: repeat_frequency("High Pass", 120., 20., 4000.),
            seed_state: Mutex::default(),
            seed_wav_location: Mutex::default(),
            seed_enabled: BoolParam::new("Use IR seed", false),
            space_enabled: BoolParam::new("Peak Space", false),
            space_threshold: FloatParam::new(
                "Sensitivity",
                -24.,
                FloatRange::Linear { min: -60., max: 0. },
            )
            .with_unit(" dBFS"),
            space_decay: space_time("Space Decay", 150.),
            space_swell: space_time("Swell", 500.),
            space_release: space_time("Space Release", 300.),
            space_steps: FloatParam::new(
                "Swell Steps",
                0.,
                FloatRange::Linear { min: 0., max: 8. },
            )
            .with_step_size(1.)
            .with_value_to_string(Arc::new(|v| {
                if v < 0.5 {
                    "Smooth".into()
                } else {
                    format!("{v:.0}")
                }
            })),
            room: EnumParam::new("Room", RoomPreset::Salon),
            extra_damping: FloatParam::new(
                "Damping",
                0.0,
                FloatRange::Linear { min: 0.0, max: 1.0 },
            )
            .with_value_to_string(Arc::new(|value| format!("{:.0}%", value * 100.0))),
            matrix: EnumParam::new("Matrix", MatrixMode::Hybrid),
            damping_low_hz: damping_frequency("Low Frequency", 250.0, 20.0, 2000.0),
            damping_mid_hz: damping_frequency("Mid Frequency", 1000.0, 100.0, 12000.0),
            damping_mid_q: FloatParam::new(
                "Mid Q",
                std::f32::consts::FRAC_1_SQRT_2,
                FloatRange::Linear { min: 0.2, max: 8.0 },
            )
            .with_value_to_string(Arc::new(|value| format!("{value:.2}"))),
            delay_mode: EnumParam::new("Delay Mode", DelayReadMode::Static),
            mod_depth: FloatParam::new("Mod Depth", 0.2, FloatRange::Linear { min: 0.0, max: 1.0 })
                .with_unit(" ms")
                .with_value_to_string(Arc::new(|value| format!("{value:.2}"))),
            mod_rate: FloatParam::new(
                "Mod Rate",
                0.3,
                FloatRange::Linear {
                    min: 0.05,
                    max: 2.0,
                },
            )
            .with_unit(" Hz")
            .with_value_to_string(Arc::new(|value| format!("{value:.2}"))),
            loop_diffusion: FloatParam::new(
                "Loop Diffusion",
                0.0,
                FloatRange::Linear { min: 0.0, max: 1.0 },
            )
            .with_value_to_string(Arc::new(|value| format!("{value:.2}"))),
            diffusion: FloatParam::new(
                "Input diffusion",
                0.0,
                FloatRange::Linear { min: 0.0, max: 1.0 },
            )
            .with_value_to_string(Arc::new(|value| format!("{value:.2}"))),
            t60: FloatParam::new(
                "T60",
                2.5,
                FloatRange::Skewed {
                    min: 0.1,
                    max: 20.0,
                    factor: FloatRange::skew_factor(-2.0),
                },
            )
            .with_unit(" s")
            .with_value_to_string(Arc::new(|value| format!("{value:.2}"))),
            damping: FloatParam::new(
                "High Damping",
                6000.0,
                FloatRange::Skewed {
                    min: 100.0,
                    max: 20_000.0,
                    factor: FloatRange::skew_factor(-2.0),
                },
            )
            .with_unit(" Hz")
            .with_value_to_string(Arc::new(|value| format!("{value:.0}"))),
            damping_low: FloatParam::new(
                "Low Damping",
                0.0,
                FloatRange::Linear {
                    min: 0.0,
                    max: 24.0,
                },
            )
            .with_unit(" dB")
            .with_value_to_string(Arc::new(|value| format!("{value:.1}"))),
            damping_mid: FloatParam::new(
                "Mid Damping",
                0.0,
                FloatRange::Linear {
                    min: 0.0,
                    max: 24.0,
                },
            )
            .with_unit(" dB")
            .with_value_to_string(Arc::new(|value| format!("{value:.1}"))),
            wet: FloatParam::new("Dry / Wet", 0.35, FloatRange::Linear { min: 0.0, max: 1.0 })
                .with_value_to_string(Arc::new(|value| format!("{value:.2}"))),
            output_db: FloatParam::new(
                "Output",
                0.0,
                FloatRange::Linear {
                    min: -24.0,
                    max: 6.0,
                },
            )
            .with_unit(" dB")
            .with_value_to_string(Arc::new(|value| format!("{value:.1}"))),
            bypass: BoolParam::new("Bypass", false).make_bypass(),
            duck_enabled: BoolParam::new("Ducking", false),
            duck_source: EnumParam::new("Source", DuckSource::Internal),
            duck_detection: EnumParam::new("Ducking Detection", DuckDetection::Level),
            duck_threshold: FloatParam::new(
                "Threshold",
                -24.0,
                FloatRange::Linear {
                    min: -60.0,
                    max: 0.0,
                },
            )
            .with_unit(" dBFS")
            .with_value_to_string(Arc::new(|value| format!("{value:.1}"))),
            duck_amount: FloatParam::new(
                "Amount",
                12.0,
                FloatRange::Linear {
                    min: 0.0,
                    max: 36.0,
                },
            )
            .with_unit(" dB")
            .with_value_to_string(Arc::new(|value| format!("{value:.1}"))),
            duck_attack: FloatParam::new(
                "Attack",
                10.0,
                FloatRange::Skewed {
                    min: 0.1,
                    max: 200.0,
                    factor: FloatRange::skew_factor(-2.0),
                },
            )
            .with_unit(" ms")
            .with_value_to_string(Arc::new(|value| format!("{value:.1}"))),
            duck_release: FloatParam::new(
                "Release",
                300.0,
                FloatRange::Skewed {
                    min: 10.0,
                    max: 3000.0,
                    factor: FloatRange::skew_factor(-2.0),
                },
            )
            .with_unit(" ms")
            .with_value_to_string(Arc::new(|value| format!("{value:.1}"))),
        }
    }
}

fn apply_live_controls(f: &mut DiffFdn, p: &ReverbParams) {
    f.set_live_controls(
        p.t60.value(),
        p.damping.value(),
        p.matrix.value().into(),
        p.diffusion.value(),
        p.loop_diffusion.value(),
        p.damping_low.value(),
        p.damping_mid.value(),
        p.damping_low_hz.value(),
        p.damping_mid_hz.value(),
        p.damping_mid_q.value(),
        p.delay_mode.value().into(),
        p.mod_depth.value(),
        p.mod_rate.value(),
    );
}

struct PluginEngine {
    repeat: slice_repeat::Repeat,
    seed: seed_import::Playback,
    reverb: LiveDiffFdn,
    sample_rate: u32,
    wet: f32,
    gain: f32,
    bypass: f32,
    smoothing: f32,
    duck_reduction: f32,
    limiter: live_limiter::Limiter,
    wet_limiter: live_limiter::Limiter,
    duck_transient: DuckTransient,
    duck_detection: DuckDetection,
    duck_source: DuckSource,
}

impl PluginEngine {
    fn new(sample_rate: f32, params: &ReverbParams) -> Option<Self> {
        // The DSP uses integer sample rates; don't silently detune fractional rates.
        if !sample_rate.is_finite() || sample_rate.fract() != 0.0 {
            return None;
        }
        let sample_rate = sample_rate as u32;
        let mut reverb = LiveDiffFdn::new(sample_rate, params.room.value())?;
        for f in &mut reverb.rooms {
            f.set_damping(params.extra_damping.value() as f64);
        }
        for f in &mut reverb.rooms {
            f.update(params);
        }
        reverb.reset(params.room.value());
        let mut limiter = live_limiter::Limiter::default();
        limiter.configure(sample_rate);
        let mut wet_limiter = live_limiter::Limiter::default();
        wet_limiter.configure(sample_rate);
        Some(Self {
            repeat: slice_repeat::Repeat::new(sample_rate),
            seed: seed_import::Playback::restore(sample_rate, params),
            reverb,
            sample_rate,
            wet: params.wet.value(),
            gain: util::db_to_gain(params.output_db.value()),
            bypass: if params.bypass.value() { 1.0 } else { 0.0 },
            smoothing: 1.0 - (-1.0 / (0.01 * sample_rate as f32)).exp(),
            duck_reduction: 0.0,
            limiter,
            wet_limiter,
            duck_transient: DuckTransient {
                armed: true,
                ..Default::default()
            },
            duck_detection: params.duck_detection.value(),
            duck_source: params.duck_source.value(),
        })
    }

    fn update(&mut self, params: &ReverbParams) {
        self.repeat.update(params);
        self.seed.update(params);
        for f in &mut self.reverb.rooms {
            f.update(params);
            f.set_damping(params.extra_damping.value() as f64);
        }
    }

    fn reset(&mut self, params: &ReverbParams) {
        self.repeat.reset();
        self.update(params);
        self.reverb.reset(params.room.value());
        self.seed.reset();
        self.limiter.reset();
        self.wet_limiter.reset();
        // Host reset restores the current parameter state without allocating or
        // carrying coefficient ramps over from the previous transport position.
        self.wet = params.wet.value();
        self.gain = util::db_to_gain(params.output_db.value());
        self.bypass = if params.bypass.value() { 1.0 } else { 0.0 };
        self.duck_reduction = 0.0;
        self.duck_transient = DuckTransient {
            armed: true,
            ..Default::default()
        };
    }

    #[cfg(test)]
    fn process(
        &mut self,
        buffer: &mut Buffer,
        params: &ReverbParams,
        sidechain: Option<&Buffer>,
    ) -> f32 {
        self.process_with_wet_output(buffer, params, sidechain, None)
    }

    fn process_with_wet_output(
        &mut self,
        buffer: &mut Buffer,
        params: &ReverbParams,
        sidechain: Option<&Buffer>,
        mut wet_output: Option<&mut Buffer>,
    ) -> f32 {
        if let Some(output) = wet_output.as_mut() {
            for channel in output.as_slice() {
                channel.fill(0.);
            }
        }
        self.update(params);
        let target_wet = params.wet.value();
        let target_gain = util::db_to_gain(params.output_db.value());
        let target_bypass = if params.bypass.value() { 1.0 } else { 0.0 };
        let mut peak = 0.0_f32;
        let coefficient = |ms: f32| 1.0 - (-1.0 / (0.001 * ms * self.sample_rate as f32)).exp();
        let attack = coefficient(params.duck_attack.value());
        let release = coefficient(params.duck_release.value());
        let timing = DuckTiming {
            fast: coefficient(30.0),
            slow_attack: coefficient(30.0),
            slow_release: coefficient(100.0),
            // Three attack time constants give even slow attacks time to reach
            // about 95% of the target. No extra user-facing Hold control.
            hold: ((params.duck_attack.value() * 3.0).max(20.0) * 0.001 * self.sample_rate as f32)
                .ceil() as u32,
        };
        let detection = params.duck_detection.value();
        let source = params.duck_source.value();
        if detection != self.duck_detection || source != self.duck_source {
            self.duck_transient = DuckTransient {
                armed: true,
                ..Default::default()
            };
            self.duck_detection = detection;
            self.duck_source = source;
        }
        let threshold = util::db_to_gain(params.duck_threshold.value());
        let sidechain = sidechain.map(Buffer::as_slice_immutable);
        for (index, mut frame) in buffer.iter_samples().enumerate() {
            // Reject invalid host samples before they can poison the feedback state.
            let finite = |x: f32| if x.is_finite() { x } else { 0.0 };
            let dry = [
                finite(*frame.get_mut(0).unwrap()),
                finite(*frame.get_mut(1).unwrap()),
            ];
            let detector_input = dry;
            let input = dry[0] * 0.5 + dry[1] * 0.5;
            let input = if self.repeat.mode() == slice_repeat::Mode::Input {
                self.repeat.process([input; 2])[0]
            } else {
                input
            };
            let blend = self.seed.advance(params.seed_enabled.value());
            let mut wet = if blend < 1.0 {
                self.reverb.process(input, params.room.value())
            } else {
                [0.0; 2]
            };
            let custom = self.seed.process(input);
            for c in 0..2 {
                wet[c] = wet[c] * (1.0 - blend) + custom[c] * blend;
            }
            if self.seed.just_selected() {
                self.reverb.reset(params.room.value());
            }
            if self.repeat.mode() == slice_repeat::Mode::Space {
                wet = self.repeat.process(wet);
            }
            let detector = match params.duck_source.value() {
                DuckSource::Internal => detector_input[0].abs().max(detector_input[1].abs()),
                DuckSource::External => sidechain.map_or(0.0, |channels| {
                    channels
                        .iter()
                        .filter_map(|channel| channel.get(index))
                        .fold(0.0_f32, |peak, &sample| peak.max(finite(sample).abs()))
                }),
            };
            // One dB of reduction per dB above threshold, capped by Amount.
            let transient_target = self.duck_transient.detect(
                detector,
                threshold,
                params.duck_amount.value(),
                &timing,
            );
            let target_reduction = if !params.duck_enabled.value() {
                self.duck_transient.remaining = 0;
                0.0
            } else if detection == DuckDetection::Transient {
                transient_target
            } else {
                (20.0 * detector.max(1e-20).log10() - params.duck_threshold.value())
                    .clamp(0.0, params.duck_amount.value())
            };
            let speed = if target_reduction > self.duck_reduction {
                attack
            } else {
                release
            };
            self.duck_reduction += speed * (target_reduction - self.duck_reduction);
            let duck_gain = util::db_to_gain(-self.duck_reduction);
            self.wet += self.smoothing * (target_wet - self.wet);
            self.gain += self.smoothing * (target_gain - self.gain);
            self.bypass += self.smoothing * (target_bypass - self.bypass);
            let (dry_gain, wet_gain) = difffdn_mix_gains(self.wet);
            let wet_only = self
                .wet_limiter
                .process(wet.map(|x| self.gain * duck_gain * x), true);
            if let Some(output) = wet_output.as_mut() {
                for (c, channel) in output.as_slice().iter_mut().take(2).enumerate() {
                    if let Some(sample) = channel.get_mut(index) {
                        *sample = wet_only[c] * (1. - self.bypass);
                    }
                }
            }
            let effected = std::array::from_fn(|channel| {
                self.gain * (dry_gain * dry[channel] + wet_gain * duck_gain * wet[channel])
            });
            let effected = self.limiter.process(effected, true);
            for channel in 0..2 {
                // True bypass crossfade: unity dry, not the audition app's gain-retaining bypass.
                let output = (1.0 - self.bypass) * effected[channel] + self.bypass * dry[channel];
                *frame.get_mut(channel).unwrap() = output;
                peak = peak.max(output.abs());
            }
        }
        peak
    }
}

#[derive(Default)]
struct AlgoReverb {
    seed_import: Arc<seed_import::Shared>,
    params: Arc<ReverbParams>,
    engine: Option<PluginEngine>,
    peak: Arc<AtomicU32>,
    reduction: Arc<AtomicU32>,
}

impl Plugin for AlgoReverb {
    const NAME: &'static str = "Algo Reverb";
    const VENDOR: &'static str = "Algo Reverb";
    const URL: &'static str = "";
    const EMAIL: &'static str = "";
    const VERSION: &'static str = env!("CARGO_PKG_VERSION");
    const AUDIO_IO_LAYOUTS: &'static [AudioIOLayout] = &[AudioIOLayout {
        main_input_channels: NonZeroU32::new(2),
        main_output_channels: NonZeroU32::new(2),
        aux_input_ports: &[new_nonzero_u32(2)],
        aux_output_ports: &[new_nonzero_u32(2)],
        names: PortNames {
            aux_inputs: &["Sidechain"],
            aux_outputs: &["Wet Out"],
            ..PortNames::const_default()
        },
        ..AudioIOLayout::const_default()
    }];
    const SAMPLE_ACCURATE_AUTOMATION: bool = true;
    type Editor = EguiEditor<ReverbEditor>;
    type SysExMessage = ();
    type BackgroundTask = ();

    fn params(&self) -> Arc<dyn Params> {
        self.params.clone()
    }

    fn editor(&mut self, _executor: AsyncExecutor<Self>) -> Option<Self::Editor> {
        create_egui_editor(
            EguiEditorState::from_size(
                nice_plug_core::editor::dpi::LogicalSize::new(520.0, 800.0),
                1.0,
            ),
            RepaintNotifier::new(),
            EguiNiceSettings::default(),
            ReverbEditor {
                params: self.params.clone(),
                peak: self.peak.clone(),
                reduction: self.reduction.clone(),
                context: None,
                held_peak: 0.0,
                seed_import: self.seed_import.clone(),
                seed_browser: seed_import::Browser::default(),
            },
        )
    }

    fn activate(
        &mut self,
        layout: &AudioIOLayout,
        config: &BufferConfig,
        context: &mut impl ActivateContext<Self>,
    ) -> bool {
        if layout.main_input_channels != NonZeroU32::new(2)
            || layout.main_output_channels != NonZeroU32::new(2)
        {
            return false;
        }
        self.engine = PluginEngine::new(config.sample_rate, &self.params);
        // Activation may restore a different session; discard obsolete handoffs here.
        self.seed_import.clear_pending();
        self.seed_import
            .rate
            .store(config.sample_rate as u32, Ordering::Release);
        context.set_latency_samples(0);
        self.engine.is_some()
    }

    fn reset(&mut self) {
        if let Some(engine) = &mut self.engine {
            engine.reset(&self.params);
        }
        self.peak.store(0, Ordering::Relaxed);
        self.reduction.store(0, Ordering::Relaxed);
    }

    fn deactivate(&mut self) {
        self.seed_import.rate.store(0, Ordering::Release);
    }

    fn process(
        &mut self,
        buffer: &mut Buffer,
        aux: &mut AuxiliaryBuffers,
        context: &mut impl ProcessContext<Self>,
    ) -> ProcessStatus {
        let Some(engine) = &mut self.engine else {
            return ProcessStatus::Error("Plugin is not activated");
        };
        if buffer.channels() != 2 {
            return ProcessStatus::Error("Stereo I/O required");
        }
        engine.seed.receive(&self.seed_import, engine.sample_rate);
        let transport = context.transport();
        engine.repeat.transport(
            transport.tempo,
            transport.pos_beats(),
            transport.playing,
            buffer.samples(),
        );
        let peak = engine.process_with_wet_output(
            buffer,
            &self.params,
            aux.inputs.first(),
            aux.outputs.first_mut(),
        );
        self.reduction
            .store(engine.duck_reduction.to_bits(), Ordering::Relaxed);
        // Nonnegative finite floats have the same ordering as their bit patterns.
        self.peak.fetch_max(peak.to_bits(), Ordering::Relaxed);
        // Conservative fixed tail accommodates the fitted rooms and live controls.
        // This is a host scheduling hint, not an assertion of exact silence at this time.
        // At 20 BPM / quarter notes / 95% feedback, -120 dB takes about 810 s,
        // in addition to the original reverb tail. Leave conservative headroom.
        ProcessStatus::Tail(
            engine.sample_rate
                * if self.params.repeat_enabled.value() && self.params.space_enabled.value() {
                    960
                } else {
                    61
                },
        )
    }
}

impl ClapPlugin for AlgoReverb {
    const CLAP_ID: &'static str = "com.algo.algo-reverb";
    const CLAP_DESCRIPTION: Option<&'static str> =
        Some("IR-derived eight-line DiffFDN with passive damping");
    const CLAP_MANUAL_URL: Option<&'static str> = None;
    const CLAP_SUPPORT_URL: Option<&'static str> = None;
    const CLAP_FEATURES: &'static [ClapFeature] = &[
        ClapFeature::AudioEffect,
        ClapFeature::Reverb,
        ClapFeature::Stereo,
    ];
}

impl Vst3Plugin for AlgoReverb {
    const VST3_CLASS_ID: [u8; 16] = *b"pnkAlgoReverb001";
    const VST3_SUBCATEGORIES: &'static [Vst3SubCategory] =
        &[Vst3SubCategory::Fx, Vst3SubCategory::Reverb];
}

fn knob_label(param: &FloatParam) -> &str {
    match param.name() {
        "Input diffusion" => "Diffusion",
        "Loop Diffusion" => "Loop Diff",
        "High Damping" => "High Damp",
        "Low Damping" => "Low Damp",
        "Mid Damping" => "Mid Damp",
        name => name,
    }
}

/// Use the same four-column grid for every row; shorter rows start at the left.
fn knob_row(ui: &mut egui::Ui, params: &[&FloatParam], setter: &ParamSetter<'_>) {
    let spacing = ui.spacing().item_spacing.x;
    let cell_width = ((ui.available_width() - spacing * 3.0) / 4.0).min(116.0);
    // Zero-height cells grow as their contents are laid out. Align their tops:
    // vertical centering would move each later cell down by the previous growth.
    ui.with_layout(egui::Layout::left_to_right(egui::Align::Min), |ui| {
        for param in params {
            ui.allocate_ui_with_layout(
                egui::vec2(cell_width, 0.0),
                egui::Layout::top_down(egui::Align::Center),
                |ui| {
                    ui.label(knob_label(param)).on_hover_text(param.name());
                    ui.add_space(4.0);
                    dual_knob::single_knob(ui, param, setter);
                },
            );
        }
    });
    ui.add_space(10.0);
}

fn editor_fonts() -> egui::FontDefinitions {
    let mut fonts = egui::FontDefinitions::default();
    fonts.font_data.insert(
        "JetBrains Mono Medium".into(),
        Arc::new(egui::FontData::from_static(include_bytes!(
            "../resources/JetBrainsMono-Medium.ttf"
        ))),
    );
    for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
        fonts
            .families
            .entry(family)
            .or_default()
            .insert(0, "JetBrains Mono Medium".into());
    }
    fonts
}

struct ReverbEditor {
    seed_browser: seed_import::Browser,
    seed_import: Arc<seed_import::Shared>,
    params: Arc<ReverbParams>,
    peak: Arc<AtomicU32>,
    reduction: Arc<AtomicU32>,
    context: Option<GuiContext>,
    held_peak: f32,
}

impl NiceEguiApp for ReverbEditor {
    fn build(
        &mut self,
        ctx: egui::Context,
        context: GuiContext,
        _frame: &mut Frame,
    ) -> Result<(), nice_plug_egui::baseview::HandlerError> {
        ctx.set_fonts(editor_fonts());
        for theme in [egui::Theme::Dark, egui::Theme::Light] {
            ctx.style_mut_of(theme, |style| {
                let target_size = 14.0;
                for text_style in style.text_styles.values_mut() {
                    text_style.size = target_size;
                }
            });
        }
        self.context = Some(context);
        Ok(())
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut Frame) {
        self.draw(ui);
    }

    fn editor_closed(&mut self) {
        self.context = None;
    }
}

impl ReverbEditor {
    fn draw(&mut self, ui: &mut egui::Ui) {
        egui::Frame::new().inner_margin(16.0).show(ui, |ui| {
            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .max_height((ui.available_height() - 88.0).max(100.0))
                .show(ui, |ui| self.draw_contents(ui));
            self.draw_footer(ui);
        });
    }

    fn draw_footer(&mut self, ui: &mut egui::Ui) {
        let peak = f32::from_bits(self.peak.swap(0, Ordering::Relaxed));
        self.held_peak = self.held_peak.max(peak);
        ui.horizontal(|ui| {
            let peak_text = egui::RichText::new(format!(
                "Peak since reset: {:.1} dBFS",
                20.0 * self.held_peak.max(1e-6).log10()
            ));
            if self.held_peak > 1.0 {
                ui.label(peak_text.color(egui::Color32::YELLOW))
                    .on_hover_text("Above 0 dBFS — lower input/Output. Bypass is unprotected.");
            } else {
                ui.label(peak_text);
            }
            if ui.button("Reset peak").clicked() {
                self.held_peak = 0.0;
            }
        });
        ui.small("Stereo dry · mono-summed wet · use 100% wet on a send");
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(50));
    }

    fn draw_contents(&mut self, ui: &mut egui::Ui) {
        let Some(context) = &self.context else {
            return;
        };
        let setter = context.param_setter();
        ui.horizontal(|ui| {
            ui.heading("Algo Reverb");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.add(
                    ParamButton::for_param(&self.params.bypass, &setter)
                        .with_label("Bypass (unity dry)"),
                );
            });
        });
        ui.add_space(8.0);
        ui.separator();
        self.seed_browser
            .draw(ui, &self.params, &self.seed_import, &setter);
        {
            let modulation_id = ui.make_persistent_id("delay_modulation_open");
            let mut expanded =
                ui.data_mut(|data| data.get_temp::<bool>(modulation_id).unwrap_or(false));
            #[cfg(test)]
            if ui.data_mut(|d| {
                d.get_temp::<bool>(egui::Id::unique("test_modulation_open"))
                    .unwrap_or(false)
            }) {
                expanded = true;
            }
            let header = ui.horizontal(|ui| {
                let arrow = ui.small_button(if expanded { "▾" } else { "▸" });
                let label = ui.selectable_label(expanded, "Delay Modulation");
                arrow.clicked() || label.clicked()
            });
            if header.inner {
                expanded = !expanded;
                ui.data_mut(|data| data.insert_temp(modulation_id, expanded));
                ui.ctx().request_repaint();
            }
            if expanded {
                ui.indent(modulation_id, |ui| {
                    ui.horizontal(|ui| {
                        ui.label("Delay");
                        ui.add(
                            ParamComboBox::for_param(&self.params.delay_mode, &setter)
                                .with_width(260.0),
                        );
                    });
                    knob_row(
                        ui,
                        &[&self.params.mod_depth, &self.params.mod_rate],
                        &setter,
                    );
                    if self.params.delay_mode.value() == DelayReadMode::Static {
                        ui.small(
                            "Static: modulation off. Select Random / Linear or Random / Allpass.",
                        );
                    }
                });
            }
            for row in [
                &self.params.diffusion,
                &self.params.t60,
                &self.params.loop_diffusion,
                &self.params.wet,
                &self.params.output_db,
                &self.params.extra_damping,
            ]
            .chunks(3)
            {
                knob_row(ui, row, &setter);
            }
            ui.label("Damping · feedback attenuation");
            let spacing = ui.spacing().item_spacing.x;
            let width = ((ui.available_width() - spacing * 3.0) / 4.0).min(116.0);
            ui.with_layout(egui::Layout::left_to_right(egui::Align::Min), |ui| {
                for (amount, frequency) in [
                    (&self.params.damping_low, &self.params.damping_low_hz),
                    (&self.params.damping_mid, &self.params.damping_mid_hz),
                ] {
                    ui.allocate_ui_with_layout(
                        egui::vec2(width, 0.0),
                        egui::Layout::top_down(egui::Align::Center),
                        |ui| {
                            ui.label(knob_label(amount));
                            ui.add_space(4.0);
                            dual_knob::dual_knob(ui, amount, frequency, &setter);
                            ui.small(format!(
                                "{:.1} dB · {:.0} Hz",
                                amount.value(),
                                frequency.value()
                            ));
                        },
                    );
                }
                ui.allocate_ui_with_layout(
                    egui::vec2(width, 0.0),
                    egui::Layout::top_down(egui::Align::Center),
                    |ui| {
                        ui.label("High Damp");
                        ui.add_space(4.0);
                        dual_knob::single_knob(ui, &self.params.damping, &setter);
                    },
                );
                ui.allocate_ui_with_layout(
                    egui::vec2(width, 0.0),
                    egui::Layout::top_down(egui::Align::Center),
                    |ui| {
                        ui.label("Mid Q");
                        ui.add_space(20.0);
                        dual_knob::q_value(ui, &self.params.damping_mid_q, &setter);
                    },
                );
            });
            ui.small("Low / Mid: left drag = dB · right drag = Hz · Shift = fine");
            ui.small("DiffFDN · leveled Wet · safety -1 dBFS")
                .on_hover_text(
                    "T60 adds loop loss to the fitted absorption. Bypass is unprotected.",
                );
        }

        ui.group(|ui| {
            ui.heading("Peak Space");
            ui.add(
                ParamButton::for_param(&self.params.space_enabled, &setter)
                    .with_label("Enable Peak Space"),
            );
            ui.small("Onset reset → dip → swell · previous space releases. +5 ms wet predelay.");
            knob_row(
                ui,
                &[
                    &self.params.space_threshold,
                    &self.params.space_decay,
                    &self.params.space_swell,
                    &self.params.space_release,
                ],
                &setter,
            );
            knob_row(ui, &[&self.params.space_steps], &setter);
            ui.small("Swell Steps: Smooth or 1–8 rises · softened edges, same total Swell time.");
            ui.separator();
            ui.horizontal(|ui| {
                ui.add(
                    ParamButton::for_param(&self.params.repeat_enabled, &setter)
                        .with_label("Slice Repeat"),
                );
                ui.add(ParamEnumButton::cycling(&self.params.repeat_mode, &setter));
                ui.add(ParamEnumButton::cycling(
                    &self.params.repeat_division,
                    &setter,
                ));
            });
            knob_row(
                ui,
                &[
                    &self.params.repeat_feedback,
                    &self.params.repeat_mix,
                    &self.params.repeat_high_cut,
                    &self.params.repeat_high_pass,
                ],
                &setter,
            );
            ui.small("Requires Peak Space · tempo sync (20–400 BPM; fallback 120).");
            knob_row(ui, &[&self.params.repeat_nudge], &setter);
            ui.small("Groove: fixed ±ms shift · interval unchanged · negative = ahead, positive = behind.");
            ui.small("Extra stereo Wet Out: after ducking/output, independent of Dry/Wet.");
            ui.small("Space: repeat wet slices · Input: repeat slices into the space.");
            ui.small("High Cut / High Pass filter every repeat · 2 ms softened slice edges.");
        });
        ui.separator();
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.add(
                ParamButton::for_param(&self.params.duck_enabled, &setter)
                    .with_label("Ducking (wet only)"),
            );
            ui.add_space(12.0);
            ui.label("Source");
            ui.add(ParamEnumButton::cycling(&self.params.duck_source, &setter));
            ui.add(ParamEnumButton::cycling(&self.params.duck_detection, &setter))
                .on_hover_text("Level: follow loudness. Transient: duck on onsets, then recover during sustained notes. Threshold controls onset sensitivity; Attack/Release shape the reduction.");
        });
        ui.add_space(12.0);
        knob_row(
            ui,
            &[
                &self.params.duck_threshold,
                &self.params.duck_amount,
                &self.params.duck_attack,
                &self.params.duck_release,
            ],
            &setter,
        );
        ui.label(format!(
            "Gain reduction: {:.1} dB",
            f32::from_bits(self.reduction.load(Ordering::Relaxed))
        ));
        ui.label("External: route a track to the DAW sidechain input.");
        ui.add_space(8.0);
        ui.separator();
        ui.add_space(8.0);
    }
}

nice_export_clap!(AlgoReverb);
nice_export_vst3!(AlgoReverb);

#[cfg(test)]
mod tests {
    use super::*;
    use nice_plug::params::InternalParamMut;

    struct LayoutHost;

    impl nice_plug_core::context::gui::GuiContextInner for LayoutHost {
        fn plugin_api(&self) -> nice_plug_core::context::PluginApi {
            nice_plug_core::context::PluginApi::Clap
        }
        unsafe fn raw_begin_set_parameter(&self, _: nice_plug_core::params::internals::ParamPtr) {
            panic!("layout must not edit parameters");
        }
        unsafe fn raw_set_parameter_normalized(
            &self,
            _: nice_plug_core::params::internals::ParamPtr,
            _: f32,
        ) {
            panic!("layout must not edit parameters");
        }
        unsafe fn raw_end_set_parameter(&self, _: nice_plug_core::params::internals::ParamPtr) {
            panic!("layout must not edit parameters");
        }
        fn get_state(&self) -> nice_plug_core::plugin::PluginState {
            panic!("not used in layout tests");
        }
        fn set_state(&self, _: nice_plug_core::plugin::PluginState) {
            panic!("not used in layout tests");
        }
        fn request_restart(&self) {}
    }

    #[test]
    fn editor_contents_fit_default_window_even_with_peak_warning() {
        assert_editor_contents_fit_default_window(2.0, false);
    }

    #[test]
    fn editor_contents_fit_default_window_without_peak_warning() {
        assert_editor_contents_fit_default_window(0.0, false);
    }

    #[test]
    fn difffdn_long_ducking_labels_fit_default_window() {
        assert_editor_contents_fit_default_window(2.0, true);
    }

    fn assert_editor_contents_fit_default_window(peak: f32, open_modulation: bool) {
        let mut editor = ReverbEditor {
            seed_browser: seed_import::Browser::default(),
            seed_import: Arc::default(),
            params: Arc::new(ReverbParams::default()),
            peak: Arc::new(AtomicU32::new(peak.to_bits())),
            reduction: Arc::new(AtomicU32::new(0)),
            context: Some(GuiContext::new(Arc::new(LayoutHost))),
            held_peak: 0.0,
        };
        if open_modulation {
            // Exercise the longest ducking labels in the compact fixed window.
            set(&editor.params.duck_detection, DuckDetection::Transient);
            set(&editor.params.duck_source, DuckSource::External);
        }
        let ctx = egui::Context::default();
        ctx.set_fonts(editor_fonts());
        for theme in [egui::Theme::Dark, egui::Theme::Light] {
            ctx.style_mut_of(theme, |style| {
                for font in style.text_styles.values_mut() {
                    font.size = 14.0;
                }
            });
        }
        let window_size = egui::vec2(520.0, 800.0);
        for frame in 0..9 {
            let mut bounds = egui::Rect::NOTHING;
            let mut output = ctx.run_ui(
                egui::RawInput {
                    time: Some(frame as f64 / 60.0),
                    screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, window_size)),
                    ..Default::default()
                },
                |ui| {
                    ui.data_mut(|data| {
                        data.insert_temp(egui::Id::unique("test_modulation_open"), open_modulation)
                    });
                    editor.draw(ui);
                    bounds = ui.min_rect();
                },
            );
            output.textures_delta.clear();
            for viewport in output.viewport_output.values() {
                assert!(
                    !viewport
                        .commands
                        .iter()
                        .any(|command| matches!(command, egui::ViewportCommand::InnerSize(_))),
                    "opening and closing modulation must never request a host resize"
                );
            }
            let labels: Vec<_> = output
                .shapes
                .iter()
                .filter_map(|shape| match &shape.shape {
                    egui::epaint::Shape::Text(text) => Some(text),
                    _ => None,
                })
                .collect();
            {
                for hidden in ["Room", "Matrix"] {
                    assert!(!labels.iter().any(|t| t.galley.job.text == hidden));
                }
                for visible in ["Impulse Response", "Browse…", "◀", "▶", "Refresh"] {
                    assert!(
                        labels.iter().any(|t| t.galley.job.text == visible),
                        "missing {visible}"
                    );
                }
                assert!(labels.iter().any(|t| t.galley.job.text == "Damping"));
                for restored in [
                    "Delay Modulation",
                    "T60",
                    "Diffusion",
                    "Low Damp",
                    "Mid Damp",
                    "High Damp",
                ] {
                    assert!(
                        labels.iter().any(|t| t.galley.job.text == restored),
                        "missing {restored}"
                    );
                }
                if open_modulation {
                    for name in ["Mod Depth", "Mod Rate"] {
                        assert!(
                            labels.iter().any(|t| t.galley.job.text == name),
                            "missing {name}"
                        );
                    }
                }
            }
            assert!(bounds.right() <= window_size.x, "{bounds:?}");
            let peak_label = output
                .shapes
                .iter()
                .find_map(|shape| match &shape.shape {
                    egui::epaint::Shape::Text(text)
                        if text.galley.job.text.starts_with("Peak since reset:") =>
                    {
                        Some(text)
                    }
                    _ => None,
                })
                .expect("peak label must remain visible");
            for section in &peak_label.galley.job.sections {
                assert_eq!(
                    section.format.color == egui::Color32::YELLOW,
                    peak > 1.0,
                    "peak warning color must reflect the held peak"
                );
            }
            assert!(
                bounds.right() <= window_size.x && bounds.bottom() <= window_size.y,
                "{bounds:?}"
            );
        }
    }

    #[test]
    fn knob_rows_are_level_left_aligned_and_compact() {
        let params = ReverbParams::default();
        let setter = ParamSetter::new(&LayoutHost);
        for width in [480.0, 520.0, 640.0] {
            for row in [
                vec![&params.mod_depth, &params.mod_rate],
                vec![&params.wet, &params.output_db, &params.loop_diffusion],
                vec![&params.diffusion, &params.t60, &params.damping],
                vec![
                    &params.duck_threshold,
                    &params.duck_amount,
                    &params.duck_attack,
                    &params.duck_release,
                ],
            ] {
                let ctx = egui::Context::default();
                ctx.set_fonts(editor_fonts());
                for theme in [egui::Theme::Dark, egui::Theme::Light] {
                    ctx.style_mut_of(theme, |style| {
                        for font in style.text_styles.values_mut() {
                            font.size = 14.0;
                        }
                    });
                }
                for _ in 0..3 {
                    let mut height = 0.0;
                    let mut first_column_center = 0.0;
                    let mut column_pitch = 0.0;
                    let mut output = ctx.run_ui(
                        egui::RawInput {
                            screen_rect: Some(egui::Rect::from_min_size(
                                egui::Pos2::ZERO,
                                egui::vec2(width, 660.0),
                            )),
                            ..Default::default()
                        },
                        |ui| {
                            let spacing = ui.spacing().item_spacing.x;
                            let cell_width =
                                ((ui.available_width() - spacing * 3.0) / 4.0).min(116.0);
                            first_column_center = ui.cursor().left() + cell_width * 0.5;
                            column_pitch = cell_width + spacing;
                            knob_row(ui, &row, &setter);
                            height = ui.min_rect().height();
                        },
                    );
                    output.textures_delta.clear();
                    let labels: Vec<_> = output
                        .shapes
                        .iter()
                        .filter_map(|shape| match &shape.shape {
                            egui::epaint::Shape::Text(text)
                                if row.iter().any(|p| knob_label(p) == text.galley.job.text) =>
                            {
                                assert_eq!(text.galley.rows.len(), 1, "wrapped knob label");
                                // Use layout height: glyph ink cap heights differ by label.
                                let mut bounds = text.visual_bounding_rect();
                                bounds.min.y = text.pos.y;
                                bounds.max.y = text.pos.y + text.galley.size().y;
                                Some(bounds)
                            }
                            _ => None,
                        })
                        .collect();
                    assert_eq!(labels.len(), row.len());
                    let knobs: Vec<_> = output
                        .shapes
                        .iter()
                        .filter_map(|shape| match &shape.shape {
                            egui::epaint::Shape::Path(path) if path.points.len() == 49 => {
                                Some(path.visual_bounding_rect())
                            }
                            _ => None,
                        })
                        .collect();
                    assert_eq!(
                        knobs.len(),
                        row.len() * 2,
                        "background and value arcs per knob"
                    );
                    for knob in knobs {
                        let label = labels
                            .iter()
                            .min_by(|a, b| {
                                (a.center().x - knob.center().x)
                                    .abs()
                                    .total_cmp(&(b.center().x - knob.center().x).abs())
                            })
                            .unwrap();
                        let gap = knob.top() - label.bottom();
                        assert!(gap >= 4.0, "label is too close to knob: {gap}");
                    }
                    for label in &labels {
                        assert!(
                            (label.top() - labels[0].top()).abs() <= 1.0,
                            "staggered labels: {labels:?}"
                        );
                    }
                    for (column, label) in labels.iter().enumerate() {
                        let expected = first_column_center + column as f32 * column_pitch;
                        assert!(
                            (label.center().x - expected).abs() <= 2.0,
                            "expected {expected}, pitch {column_pitch}: {labels:?}"
                        );
                    }
                    assert!(height < 124.0, "row is too tall: {height}");
                }
            }
        }
    }

    #[test]
    fn input_label_is_short_but_parameter_name_is_preserved() {
        let params = ReverbParams::default();
        assert_eq!(knob_label(&params.diffusion), "Diffusion");
        assert_eq!(params.diffusion.name(), "Input diffusion");
        assert_eq!(knob_label(&params.loop_diffusion), "Loop Diff");
        assert_eq!(params.loop_diffusion.name(), "Loop Diffusion");
        assert_eq!(knob_label(&params.damping), "High Damp");
        assert_eq!(knob_label(&params.damping_low), "Low Damp");
        assert_eq!(knob_label(&params.damping_mid), "Mid Damp");
    }

    #[test]
    fn knob_value_displays_are_compact_and_keep_units() {
        let params = ReverbParams::default();
        set(&params.t60, 6.87);
        set(&params.damping, 3472.0);
        set(&params.output_db, 5.9);
        assert_eq!(params.t60.to_string(), "6.87 s");
        assert_eq!(params.damping.to_string(), "3472 Hz");
        assert_eq!(params.output_db.to_string(), "5.9 dB");
        assert_eq!(params.duck_threshold.to_string(), "-24.0 dBFS");
        assert_eq!(params.duck_attack.to_string(), "10.0 ms");
        assert_eq!(params.wet.to_string(), "0.35");
        assert_eq!(params.mod_depth.to_string(), "0.20 ms");
        assert_eq!(params.mod_rate.to_string(), "0.30 Hz");
    }

    #[test]
    fn output_range_and_default_support_fine_level_adjustments() {
        let params = ReverbParams::default();
        assert_eq!(params.output_db.default_plain_value(), 0.0);
        assert_eq!(params.output_db.value(), 0.0);
        assert_eq!(params.output_db.preview_plain(0.0), -24.0);
        assert_eq!(params.output_db.preview_plain(1.0), 6.0);
        assert_eq!(params.output_db.preview_plain(0.5), -9.0);
    }

    fn set<P: Param + InternalParamMut>(param: &P, value: P::Plain) {
        // Test-only host emulation, with valid values and no concurrent access.
        unsafe {
            param._internal_set_plain_value(value);
        }
    }

    fn process(
        engine: &mut PluginEngine,
        params: &ReverbParams,
        left: &mut [f32],
        right: &mut [f32],
    ) {
        process_sidechain(engine, params, left, right, None);
    }

    fn process_sidechain(
        engine: &mut PluginEngine,
        params: &ReverbParams,
        left: &mut [f32],
        right: &mut [f32],
        sidechain: Option<&Buffer>,
    ) {
        assert_eq!(left.len(), right.len());
        let mut buffer = Buffer::default();
        // Both equally-sized slices outlive the buffer, which is dropped here.
        unsafe {
            buffer.set_slices(left.len(), |slices| {
                slices.push(left);
                slices.push(right);
            });
        }
        engine.process(&mut buffer, params, sidechain);
    }

    #[test]
    fn wet_output_is_independent_protected_and_does_not_change_main() {
        fn render(wet: f32, bypass: bool, output_db: f32, attached: bool) -> [Vec<f32>; 4] {
            let p = ReverbParams::default();
            set(&p.wet, wet);
            set(&p.bypass, bypass);
            set(&p.output_db, output_db);
            set(&p.space_enabled, true);
            set(&p.repeat_enabled, true);
            set(&p.repeat_nudge, -5.);
            let mut engine = PluginEngine::new(8000., &p).unwrap();
            let mut left: Vec<f32> = (0..8000)
                .map(|i| {
                    if i < 300 {
                        (i as f32 * 0.21).sin() * 0.1
                    } else {
                        0.
                    }
                })
                .collect();
            let mut right = left.clone();
            let mut wl = vec![123.; left.len()];
            let mut wr = wl.clone();
            let mut main = Buffer::default();
            let mut aux = Buffer::default();
            unsafe {
                main.set_slices(left.len(), |s| {
                    s.push(&mut left);
                    s.push(&mut right);
                });
                aux.set_slices(wl.len(), |s| {
                    s.push(&mut wl);
                    s.push(&mut wr);
                });
            }
            engine.process_with_wet_output(&mut main, &p, None, attached.then_some(&mut aux));
            [left, right, wl, wr]
        }
        let dry = render(0., false, 0., true);
        let wet = render(1., false, 0., true);
        let no_aux = render(0., false, 0., false);
        assert_eq!(&dry[..2], &no_aux[..2]);
        assert_eq!(&dry[2..], &wet[2..]);
        assert_eq!(&wet[..2], &wet[2..]); // Same processing order at 100% Wet.
        assert!(dry[2].iter().any(|v| v.abs() > 1e-6));
        let boosted = render(0., false, 6., true);
        for (a, b) in dry[2].iter().zip(&boosted[2]) {
            assert!((b - a * util::db_to_gain(6.)).abs() < 1e-6);
        }
        let bypass = render(0.5, true, 0., true);
        assert!(bypass[2..].iter().flatten().all(|v| *v == 0.));
        let hot = render(0., false, 6., true);
        assert!(
            hot[2..]
                .iter()
                .flatten()
                .all(|v| v.is_finite() && v.abs() <= live_limiter::CEILING + 1e-6)
        );
        assert_eq!(
            AlgoReverb::AUDIO_IO_LAYOUTS[0].aux_output_ports,
            &[new_nonzero_u32(2)]
        );
        assert_eq!(
            AlgoReverb::AUDIO_IO_LAYOUTS[0].names.aux_outputs,
            &["Wet Out"]
        );
    }

    #[test]
    fn slice_repeat_modes_are_distinct_block_independent_limited_and_resettable() {
        let mut outputs = Vec::new();
        for mode in [slice_repeat::Mode::Space, slice_repeat::Mode::Input] {
            let params = ReverbParams::default();
            set(&params.space_enabled, true);
            set(&params.repeat_enabled, true);
            set(&params.repeat_mode, mode);
            set(&params.repeat_feedback, 0.95);
            set(&params.repeat_nudge, -5.25);
            set(&params.wet, 1.);
            let mut a = PluginEngine::new(8000., &params).unwrap();
            let mut b = PluginEngine::new(8000., &params).unwrap();
            let source: Vec<f32> = (0..24000)
                .map(|i| {
                    if i % 4000 < 700 {
                        (i as f32 * 0.21).sin() * 0.8
                    } else {
                        0.
                    }
                })
                .collect();
            let mut left = source.clone();
            let mut right = source.clone();
            let mut bl = source.clone();
            let mut br = source;
            process(&mut a, &params, &mut left, &mut right);
            for (l, r) in bl.chunks_mut(37).zip(br.chunks_mut(37)) {
                process(&mut b, &params, l, r);
            }
            assert_eq!(left, bl);
            assert_eq!(right, br);
            assert!(left.iter().any(|x| x.abs() > 1e-5));
            assert!(
                left.iter()
                    .chain(&right)
                    .all(|x| x.is_finite() && x.abs() <= live_limiter::CEILING + 1e-6)
            );
            a.reset(&params);
            let mut l = [0.; 4096];
            let mut r = l;
            process(&mut a, &params, &mut l, &mut r);
            assert_eq!(l, [0.; 4096]);
            outputs.push(left);
        }
        assert!(
            outputs[0]
                .iter()
                .zip(&outputs[1])
                .map(|(a, b)| (a - b).abs())
                .sum::<f32>()
                > 0.1
        );
    }

    #[test]
    fn disabled_slice_repeat_preserves_audio_and_parameter_defaults() {
        let a = ReverbParams::default();
        let b = ReverbParams::default();
        assert!(!a.repeat_enabled.value());
        assert_eq!(a.repeat_division.value(), slice_repeat::Division::Eighth);
        assert_eq!(a.repeat_mode.value(), slice_repeat::Mode::Space);
        set(&b.repeat_enabled, true); // Peak Space is still OFF.
        set(&b.repeat_mode, slice_repeat::Mode::Input);
        let mut ea = PluginEngine::new(8000., &a).unwrap();
        let mut eb = PluginEngine::new(8000., &b).unwrap();
        let mut al: Vec<f32> = (0..8000).map(|i| (i as f32 * 0.23).sin() * 0.2).collect();
        let mut ar = al.clone();
        let mut bl = al.clone();
        let mut br = al.clone();
        process(&mut ea, &a, &mut al, &mut ar);
        process(&mut eb, &b, &mut bl, &mut br);
        assert_eq!((al, ar), (bl, br));
    }

    #[test]
    fn peak_space_plugin_is_block_independent_and_limited() {
        for steps in [0., 1., 4., 8.] {
            check_peak_space_blocks(steps);
        }
    }

    fn check_peak_space_blocks(steps: f32) {
        let params = ReverbParams::default();
        assert!(!params.space_enabled.value());
        assert_eq!(params.space_steps.value(), 0.);
        assert_eq!(params.space_steps.to_string(), "Smooth");
        set(&params.space_steps, steps);
        set(&params.space_enabled, true);
        set(&params.wet, 1.);
        let mut a = PluginEngine::new(8000., &params).unwrap();
        let mut b = PluginEngine::new(8000., &params).unwrap();
        let mut left: Vec<f32> = (0..16000)
            .map(|i| if i % 800 == 0 { 2. } else { 0. })
            .collect();
        let mut right = left.clone();
        let mut bl = left.clone();
        let mut br = right.clone();
        process(&mut a, &params, &mut left, &mut right);
        for (l, r) in bl.chunks_mut(37).zip(br.chunks_mut(37)) {
            process(&mut b, &params, l, r);
        }
        assert_eq!(left, bl);
        assert_eq!(right, br);
        assert!(left.iter().any(|x| x.abs() > 1e-6));
        assert!(
            left.iter()
                .chain(&right)
                .all(|x| x.is_finite() && x.abs() <= 0.892)
        );
        a.reset(&params);
        let (mut l, mut r) = ([0.; 1000], [0.; 1000]);
        process(&mut a, &params, &mut l, &mut r);
        assert!(l.iter().chain(&r).all(|&x| x == 0.));
    }

    #[test]
    fn external_ducking_scales_only_wet_and_preserves_tail_state() {
        let params = ReverbParams::default();
        set(&params.wet, 1.0);
        set(&params.output_db, 0.0);
        set(&params.duck_source, DuckSource::External);
        set(&params.duck_attack, 0.1);
        let mut reference = PluginEngine::new(48000.0, &params).unwrap();
        let mut ducked = PluginEngine::new(48000.0, &params).unwrap();
        let mut l = vec![0.0; 12000];
        l[0] = 1.0;
        let mut r = l.clone();
        let (mut dl, mut dr) = (l.clone(), r.clone());
        // Opposite polarity must still trigger full stereo-linked reduction.
        let mut sc_l = vec![1.0; l.len()];
        let mut sc_r = vec![-1.0; l.len()];
        let mut sc = Buffer::default();
        // Equally-sized, live slices; the buffer is dropped before these vectors.
        unsafe {
            sc.set_slices(sc_l.len(), |s| {
                s.push(&mut sc_l);
                s.push(&mut sc_r);
            });
        }
        process_sidechain(&mut reference, &params, &mut l, &mut r, Some(&sc));
        set(&params.duck_enabled, true);
        process_sidechain(&mut ducked, &params, &mut dl, &mut dr, Some(&sc));
        let gain = util::db_to_gain(-12.0);
        for ((&left, &right), (&duck_l, &duck_r)) in
            l.iter().zip(&r).zip(dl.iter().zip(&dr)).skip(1000)
        {
            assert!((duck_l - left * gain).abs() < 1e-6);
            assert!((duck_r - right * gain).abs() < 1e-6);
        }
        assert!((ducked.duck_reduction - 12.0).abs() < 1e-4);
        // Ducking changes no delay/filter state: removing reduction reveals the same tail.
        ducked.duck_reduction = 0.0;
        set(&params.duck_enabled, false);
        let (mut l, mut r, mut dl, mut dr) = ([0.0; 4096], [0.0; 4096], [0.0; 4096], [0.0; 4096]);
        process(&mut reference, &params, &mut l, &mut r);
        process(&mut ducked, &params, &mut dl, &mut dr);
        assert_eq!((l, r), (dl, dr));
    }

    #[test]
    fn ducking_attack_release_missing_sidechain_and_reset() {
        let params = ReverbParams::default();
        set(&params.duck_enabled, true);
        let mut engine = PluginEngine::new(48000.0, &params).unwrap();
        // Attack/Release are one-pole time constants: about 63% after one time constant.
        process(&mut engine, &params, &mut [1.0; 480], &mut [-1.0; 480]);
        assert!((engine.duck_reduction - 12.0 * (1.0 - (-1.0_f32).exp())).abs() < 0.01);
        let before = engine.duck_reduction;
        set(&params.duck_source, DuckSource::External);
        process(&mut engine, &params, &mut [1.0; 14400], &mut [1.0; 14400]);
        assert!((engine.duck_reduction - before * (-1.0_f32).exp()).abs() < 0.01);
        engine.reset(&params);
        assert_eq!(engine.duck_reduction, 0.0);
        // No external audio must not fall back to loud internal input.
        process(&mut engine, &params, &mut [1.0; 100], &mut [1.0; 100]);
        assert_eq!(engine.duck_reduction, 0.0);
        set(&params.duck_source, DuckSource::Internal);
        set(&params.duck_amount, 0.0);
        process(&mut engine, &params, &mut [1.0; 100], &mut [1.0; 100]);
        assert_eq!(engine.duck_reduction, 0.0);
    }

    #[test]
    fn external_ducking_is_block_independent_and_sanitizes_trigger() {
        assert_external_ducking_is_block_independent(DuckDetection::Level);
        assert_external_ducking_is_block_independent(DuckDetection::Transient);
    }

    fn assert_external_ducking_is_block_independent(detection: DuckDetection) {
        let params = ReverbParams::default();
        set(&params.duck_enabled, true);
        set(&params.duck_source, DuckSource::External);
        set(&params.duck_detection, detection);
        let mut a = PluginEngine::new(48000.0, &params).unwrap();
        let mut b = PluginEngine::new(48000.0, &params).unwrap();
        let mut l = vec![0.1; 8192];
        let mut r = l.clone();
        let (mut bl, mut br) = (l.clone(), r.clone());
        let mut trigger: Vec<_> = (0..8192)
            .map(|i| if i % 1000 < 500 { 1.0 } else { 0.0 })
            .collect();
        trigger[0] = f32::NAN;
        trigger[1] = f32::INFINITY;
        let mut split_trigger = trigger.clone();
        let mut sc = Buffer::default();
        // Mono is also handled safely even though the advertised bus is stereo.
        unsafe {
            sc.set_slices(trigger.len(), |s| s.push(&mut trigger));
        }
        process_sidechain(&mut a, &params, &mut l, &mut r, Some(&sc));
        for ((l, r), trigger) in bl
            .chunks_mut(37)
            .zip(br.chunks_mut(37))
            .zip(split_trigger.chunks_mut(37))
        {
            let mut sc = Buffer::default();
            // The chunk remains live until after this buffer is dropped.
            unsafe {
                sc.set_slices(trigger.len(), |s| s.push(trigger));
            }
            process_sidechain(&mut b, &params, l, r, Some(&sc));
        }
        assert_eq!((l, r), (bl, br));
        assert_eq!(a.duck_reduction, b.duck_reduction);
        assert!(a.duck_reduction.is_finite());
    }

    fn duck_constant(engine: &mut PluginEngine, params: &ReverbParams, value: f32, ms: f32) {
        let count = (ms * 0.001 * engine.sample_rate as f32).round() as usize;
        process(
            engine,
            params,
            &mut vec![value; count],
            &mut vec![-value; count],
        );
    }

    #[test]
    fn transient_ducking_recovers_on_sustain_and_retriggers_at_multiple_rates() {
        for rate in [44100.0, 48000.0, 96000.0] {
            let params = ReverbParams::default();
            set(&params.duck_enabled, true);
            set(&params.duck_detection, DuckDetection::Transient);
            set(&params.duck_release, 50.0);
            let mut engine = PluginEngine::new(rate, &params).unwrap();
            duck_constant(&mut engine, &params, 1.0, 10.0);
            assert!((engine.duck_reduction - 12.0 * (1.0 - (-1.0_f32).exp())).abs() < 0.02);
            duck_constant(&mut engine, &params, 1.0, 20.0);
            assert!(engine.duck_reduction > 11.3);
            duck_constant(&mut engine, &params, 1.0, 700.0);
            assert!(
                engine.duck_reduction < 0.001,
                "sustain must not hold reduction"
            );
            duck_constant(&mut engine, &params, 2.0, 10.0);
            assert!(
                engine.duck_reduction > 7.0,
                "a new onset over sustain must retrigger"
            );
            engine.reset(&params);
            assert_eq!(engine.duck_transient.remaining, 0);
            assert_eq!(engine.duck_transient.fast, 0.0);
            assert_eq!(engine.duck_reduction, 0.0);
            duck_constant(&mut engine, &params, 1.0, 10.0);
            assert!(engine.duck_reduction > 7.0);
        }
    }

    #[test]
    fn transient_attack_hold_release_threshold_and_amount_remain_effective() {
        let params = ReverbParams::default();
        set(&params.duck_enabled, true);
        set(&params.duck_detection, DuckDetection::Transient);
        set(&params.duck_attack, 200.0);
        set(&params.duck_release, 100.0);
        set(&params.duck_amount, 6.0);
        let mut engine = PluginEngine::new(48000.0, &params).unwrap();
        duck_constant(&mut engine, &params, 1.0, 10.0);
        assert!(engine.duck_reduction > 0.2 && engine.duck_reduction < 0.4);
        duck_constant(&mut engine, &params, 1.0, 590.0);
        assert!((engine.duck_reduction - 6.0 * (1.0 - (-3.0_f32).exp())).abs() < 0.01);
        let before = engine.duck_reduction;
        duck_constant(&mut engine, &params, 1.0, 100.0);
        assert!((engine.duck_reduction - before * (-1.0_f32).exp()).abs() < 0.01);
        engine.reset(&params);
        duck_constant(&mut engine, &params, 0.01, 100.0);
        assert_eq!(
            engine.duck_reduction, 0.0,
            "below-threshold onset must be ignored"
        );
        set(&params.duck_amount, 0.0);
        duck_constant(&mut engine, &params, 1.0, 100.0);
        assert_eq!(engine.duck_reduction, 0.0);
        set(&params.duck_amount, 6.0);
        set(&params.duck_source, DuckSource::External);
        duck_constant(&mut engine, &params, 1.0, 100.0);
        assert_eq!(
            engine.duck_reduction, 0.0,
            "missing external input must not use internal audio"
        );
    }

    #[test]
    fn transient_detector_does_not_retrigger_on_a_steady_tone() {
        let params = ReverbParams::default();
        set(&params.duck_enabled, true);
        set(&params.duck_detection, DuckDetection::Transient);
        set(&params.duck_release, 10.0);
        for hz in [40.0, 1000.0, 10000.0] {
            let mut engine = PluginEngine::new(48000.0, &params).unwrap();
            let mut left: Vec<_> = (0..48000)
                .map(|i| (std::f32::consts::TAU * hz * i as f32 / 48000.0).sin())
                .collect();
            let mut right = left.iter().map(|v| -v).collect::<Vec<_>>();
            process(&mut engine, &params, &mut left, &mut right);
            assert!(engine.duck_reduction < 0.001, "steady {hz} Hz must settle");
        }
    }

    #[test]
    fn transient_disabled_and_mode_changes_do_not_replay_a_held_trigger() {
        let params = ReverbParams::default();
        set(&params.duck_detection, DuckDetection::Transient);
        let mut engine = PluginEngine::new(48000.0, &params).unwrap();
        duck_constant(&mut engine, &params, 1.0, 100.0);
        assert_eq!(engine.duck_reduction, 0.0);
        set(&params.duck_enabled, true);
        duck_constant(&mut engine, &params, 1.0, 100.0);
        assert_eq!(
            engine.duck_reduction, 0.0,
            "enabling during sustain must not replay an onset"
        );
        duck_constant(&mut engine, &params, 2.0, 10.0);
        assert!(engine.duck_reduction > 7.0);
        set(&params.duck_detection, DuckDetection::Level);
        set(&params.duck_source, DuckSource::External);
        let before = engine.duck_reduction;
        duck_constant(&mut engine, &params, 1.0, 10.0);
        assert!(engine.duck_reduction < before);
        assert_eq!(engine.duck_transient.remaining, 0);
        set(&params.duck_detection, DuckDetection::Transient);
        let before = engine.duck_reduction;
        duck_constant(&mut engine, &params, 1.0, 10.0);
        assert!(engine.duck_reduction < before);
    }

    #[test]
    fn plugin_uses_embedded_jetbrains_mono_medium_before_fallback_fonts() {
        let fonts = editor_fonts();
        assert_eq!(
            fonts.font_data["JetBrains Mono Medium"]
                .font
                .as_ref()
                .as_ref(),
            include_bytes!("../resources/JetBrainsMono-Medium.ttf").as_slice()
        );
        for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
            assert_eq!(fonts.families[&family][0], "JetBrains Mono Medium");
            assert!(fonts.families[&family].len() > 1);
        }
    }

    #[test]
    fn plugin_embeds_default_fonts() {
        let fonts = egui::FontDefinitions::default();
        for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
            let names = &fonts.families[&family];
            assert!(!names.is_empty(), "Missing fonts for {family:?}");
            for name in names {
                assert!(!fonts.font_data[name].font.as_ref().as_ref().is_empty());
            }
        }
    }

    #[test]
    fn default_engine_matches_difffdn_after_reset() {
        let params = ReverbParams::default();
        let mut engine = PluginEngine::new(48_000.0, &params).unwrap();
        for _ in 0..2 {
            engine.reset(&params);
            let mut reference = DiffFdn::parse(room_text(RoomPreset::Salon)).unwrap();
            reference.enable_live_controls();
            apply_live_controls(&mut reference, &params);
            reference.reset();
            for i in 0..48000 {
                let input = if i == 0 { 1.0 } else { 0.0 };
                let wet = engine.reverb.process(input, RoomPreset::Salon);
                assert_eq!(
                    wet,
                    [(DIFFFDN_WET_GAINS[0] * reference.process_sample(input as f64)) as f32; 2]
                );
            }
        }
    }

    #[test]
    fn difffdn_single_room_matches_continuously_running_reference() {
        for room in [RoomPreset::Salon, RoomPreset::Damped] {
            let params = ReverbParams::default();
            set(&params.room, room);
            let mut engine = PluginEngine::new(48000.0, &params).unwrap();
            let mut reference = PluginEngine::new(48000.0, &params).unwrap();
            let index = usize::from(room == RoomPreset::Damped);
            for i in 0..48000 {
                if i == 12000 {
                    set(&params.extra_damping, 0.7);
                    set(&params.mod_depth, 0.8);
                    set(&params.delay_mode, DelayReadMode::RandomAllpass);
                    engine.update(&params);
                    reference.update(&params);
                }
                let input = (i as f32 * 0.031).sin() * 0.1;
                let both: [f64; 2] =
                    std::array::from_fn(|j| reference.reverb.rooms[j].process_sample(input as f64));
                assert_eq!(
                    engine.reverb.process(input, room),
                    [(both[index] * DIFFFDN_WET_GAINS[index]) as f32; 2]
                );
            }
            // The unselected room has received no input and has no hidden tail.
            for _ in 0..12000 {
                assert_eq!(engine.reverb.rooms[1 - index].process_sample(0.0), 0.0);
            }
        }
    }

    #[test]
    fn difffdn_room_fade_reverses_continuously_and_clears_stopped_tail() {
        let params = ReverbParams::default();
        let mut engine = PluginEngine::new(48000.0, &params).unwrap();
        let mut reference = PluginEngine::new(48000.0, &params).unwrap();
        let n = engine.reverb.transition_samples;
        let mut position = 0_u32;
        for i in 0..24000 {
            let room = if i < 1000 || (i >= 2000 && i < 2500) {
                RoomPreset::Salon
            } else {
                RoomPreset::Damped
            };
            let target = if room == RoomPreset::Salon { 0 } else { n };
            let before = position;
            position = if position < target {
                position + 1
            } else if position > target {
                position - 1
            } else {
                position
            };
            let blend = position as f64 / n as f64;
            let input = (i as f32 * 0.031).sin() * 0.1;
            let a = reference.reverb.rooms[0].process_sample(input as f64);
            let b = reference.reverb.rooms[1].process_sample(input as f64);
            let actual = engine.reverb.process(input, room)[0];
            // Destination is cold until the first switch. Before it is audible,
            // the warm reference must be reset to that same initial state.
            if i < 1000 {
                reference.reverb.rooms[1].reset();
            }
            let expected = if position == n {
                (b * DIFFFDN_WET_GAINS[1]) as f32
            } else {
                ((1.0 - blend) * DIFFFDN_WET_GAINS[0] * a + blend * DIFFFDN_WET_GAINS[1] * b) as f32
            };
            assert!((actual - expected).abs() < 1e-7);
            assert!(position.abs_diff(before) <= 1);
            assert_eq!(engine.reverb.position, position);
        }
        assert_eq!(position, n);
        for _ in 0..12000 {
            assert_eq!(engine.reverb.rooms[0].process_sample(0.0), 0.0);
        }
        // Inactive parameter changes apply before the room is resumed.
        set(&params.extra_damping, 0.8);
        set(&params.matrix, MatrixMode::Hadamard);
        engine.update(&params);
        for _ in 0..n {
            assert!(engine.reverb.process(0.0, RoomPreset::Salon)[0].is_finite());
        }
        assert_eq!(engine.reverb.process(0.0, RoomPreset::Salon), [0.0; 2]);
        engine.reverb.reset(RoomPreset::Damped);
        assert_eq!(engine.reverb.position, n);
        assert_eq!(engine.reverb.process(0.0, RoomPreset::Damped), [0.0; 2]);
    }

    #[test]
    fn difffdn_rapid_room_automation_preserves_safety_ceiling() {
        let params = ReverbParams::default();
        set(&params.wet, 0.7);
        set(&params.output_db, 6.0);
        let mut engine = PluginEngine::new(48000.0, &params).unwrap();
        for block in 0..800 {
            set(
                &params.room,
                if (block / 80) % 2 == 0 || block % 7 == 0 {
                    RoomPreset::Salon
                } else {
                    RoomPreset::Damped
                },
            );
            let mut left = [2.0; 128];
            let mut right = [1.5; 128];
            process(&mut engine, &params, &mut left, &mut right);
            assert!(
                left.iter()
                    .chain(&right)
                    .all(|x| x.is_finite() && x.abs() <= live_limiter::CEILING + 1e-6)
            );
        }
    }

    #[test]
    fn difffdn_parameter_snapshot_restore_matches_fresh_engine() {
        let saved = ReverbParams::default();
        set(&saved.room, RoomPreset::Damped);
        set(&saved.extra_damping, 0.73);
        set(&saved.wet, 0.61);
        set(&saved.output_db, -8.0);
        // Exercise the same public parameter pointers used by the host wrapper.
        // This is a parameter/state-field roundtrip, not a DAW chunk-format test.
        let snapshot: Vec<_> = saved
            .param_map()
            .into_iter()
            .map(|(id, ptr, _)| (id, unsafe { ptr.unmodulated_normalized_value() }))
            .collect();
        let restored = ReverbParams::default();
        for (id, ptr, _) in restored.param_map() {
            let value = snapshot.iter().find(|(key, _)| key == &id).unwrap().1;
            unsafe {
                ptr._internal_set_normalized_value(value);
            }
        }
        restored.deserialize_fields(&saved.serialize_fields());
        assert_eq!(restored.room.value(), RoomPreset::Damped);
        assert_eq!(restored.extra_damping.value(), saved.extra_damping.value());
        for rate in [44100.0, 48000.0, 96000.0] {
            let mut a = PluginEngine::new(rate, &saved).unwrap();
            let mut b = PluginEngine::new(rate, &restored).unwrap();
            for _ in 0..2 {
                let mut al = [0.0; 4096];
                let mut ar = al;
                al[0] = 0.5;
                let mut bl = al;
                let mut br = ar;
                process(&mut a, &saved, &mut al, &mut ar);
                process(&mut b, &restored, &mut bl, &mut br);
                assert_eq!((al, ar), (bl, br));
                a.reset(&saved);
                b.reset(&restored);
            }
        }
        for (id, ptr, _) in restored.param_map() {
            let flags = unsafe { ptr.flags() };
            if [
                "matrix",
                "diffusion",
                "t60",
                "damping",
                "damping_low",
                "damping_mid",
                "loop_diffusion",
                "delay_mode",
                "mod_depth",
                "mod_rate",
                "damping_low_hz",
                "damping_mid_hz",
                "damping_mid_q",
            ]
            .contains(&id.as_str())
            {
                assert!(!flags.contains(ParamFlags::HIDDEN));
            }
        }
    }

    #[test]
    fn custom_seed_plugin_is_stereo_limited_and_restores_without_wav() {
        let saved = ReverbParams::default();
        let mut text = String::from("rust-stereo-seed-v1");
        for x in [1.0; 7]
            .into_iter()
            .chain([0.0; 7])
            .chain([0.35; 8])
            .chain([0.35, -0.35, 0.35, -0.35, 0.35, -0.35, 0.35, -0.35])
        {
            text.push_str(&format!(" {x}"));
        }
        *saved.seed_state.lock().unwrap() = ("missing-source.wav".into(), text);
        set(&saved.seed_enabled, true);
        set(&saved.wet, 1.0);
        set(&saved.output_db, 6.0);
        let restored = ReverbParams::default();
        restored.deserialize_fields(&saved.serialize_fields());
        set(&restored.seed_enabled, true);
        set(&restored.wet, 1.0);
        set(&restored.output_db, 6.0);
        let mut a = PluginEngine::new(48000.0, &saved).unwrap();
        let mut b = PluginEngine::new(48000.0, &restored).unwrap();
        let mut difference = 0.0;
        for block in 0..400 {
            let mut al = [0.0; 128];
            for (i, x) in al.iter_mut().enumerate() {
                *x = ((block * 128 + i) as f32 * 0.172).sin() * 3.0;
            }
            let mut ar = al;
            let mut bl = al;
            let mut br = ar;
            process(&mut a, &saved, &mut al, &mut ar);
            process(&mut b, &restored, &mut bl, &mut br);
            assert_eq!((al, ar), (bl, br));
            for i in 0..128 {
                assert!(al[i].is_finite() && ar[i].is_finite());
                assert!(al[i].abs().max(ar[i].abs()) <= live_limiter::CEILING + 1e-6);
                difference += (al[i] - ar[i]).abs();
            }
        }
        assert!(difference > 1.0);
        assert_eq!(a.seed.advance(true), 1.0);
    }

    #[test]
    fn plugin_parameter_ids_and_default_matrix_are_stable() {
        let params = ReverbParams::default();
        let ids: Vec<_> = params
            .param_map()
            .into_iter()
            .map(|(id, _, _)| id)
            .collect();
        assert_eq!(
            ids,
            [
                "matrix",
                "diffusion",
                "t60",
                "damping",
                "damping_low",
                "damping_mid",
                "wet",
                "output_db",
                "bypass",
                "duck_enabled",
                "duck_source",
                "duck_threshold",
                "duck_amount",
                "duck_attack",
                "duck_release",
                "loop_diffusion",
                "delay_mode",
                "mod_depth",
                "mod_rate",
                "damping_low_hz",
                "damping_mid_hz",
                "damping_mid_q",
                "duck_detection",
                "difffdn_room",
                "difffdn_damping",
                "seed_enabled",
                "space_enabled",
                "space_threshold",
                "space_decay",
                "space_swell",
                "space_release",
                "space_steps",
                "repeat_enabled",
                "repeat_mode",
                "repeat_division",
                "repeat_nudge",
                "repeat_feedback",
                "repeat_mix",
                "repeat_high_cut",
                "repeat_high_pass"
            ]
        );
        assert!(params.matrix.value() == MatrixMode::Hybrid);
        assert!(params.delay_mode.value() == DelayReadMode::Static);
        assert_eq!(
            DelayReadMode::ids().unwrap(),
            ["static", "random_linear", "random_allpass"]
        );
        assert_eq!(params.mod_depth.value(), 0.2);
        assert_eq!(params.mod_rate.value(), 0.3);
        assert_eq!(params.diffusion.default_plain_value(), 0.0);
        assert_eq!(params.diffusion.value(), 0.0);
        assert!(!params.duck_enabled.value());
        assert_eq!(params.loop_diffusion.default_plain_value(), 0.0);
        assert_eq!(params.loop_diffusion.value(), 0.0);
        assert!(params.duck_source.value() == DuckSource::Internal);
        assert!(params.duck_detection.value() == DuckDetection::Level);
        assert_eq!(DuckDetection::ids().unwrap(), ["level", "transient"]);
        assert_eq!(DuckSource::ids().unwrap(), ["internal", "external"]);
        assert_eq!(
            AlgoReverb::AUDIO_IO_LAYOUTS[0].aux_input_ports,
            &[new_nonzero_u32(2)]
        );
        assert_eq!(
            MatrixMode::ids().unwrap(),
            ["householder", "hadamard", "hybrid"]
        );
        assert!(params.bypass.flags().contains(ParamFlags::BYPASS));
    }

    #[test]
    fn plugin_dry_is_protected_but_bypass_preserves_unity_stereo() {
        for bypass in [false, true] {
            let params = ReverbParams::default();
            set(&params.wet, 0.0);
            set(&params.output_db, if bypass { -12.0 } else { 0.0 });
            set(&params.bypass, bypass);
            set(&params.duck_enabled, true);
            set(&params.duck_attack, 0.1);
            let mut engine = PluginEngine::new(48000.0, &params).unwrap();
            let mut left = [1.5, -0.5, 0.25];
            let mut right = [-0.25, 0.75, -1.5];
            let expected = (left, right);
            process(&mut engine, &params, &mut left, &mut right);
            if bypass {
                assert_eq!((left, right), expected);
            } else {
                for i in 0..left.len() {
                    assert!(left[i].abs().max(right[i].abs()) <= live_limiter::CEILING + 1e-6);
                    assert!((left[i] * expected.1[i] - right[i] * expected.0[i]).abs() < 1e-6);
                }
            }
        }
    }

    #[test]
    fn classic_controls_change_the_difffdn_response() {
        let render = |p: &ReverbParams| {
            let mut engine = PluginEngine::new(48000.0, p).unwrap();
            (0..24000)
                .map(|i| {
                    engine
                        .reverb
                        .process(if i == 0 { 1.0 } else { 0.0 }, p.room.value())[0]
                })
                .collect::<Vec<_>>()
        };
        let baseline = render(&ReverbParams::default());
        for control in [
            "t60",
            "damping",
            "diffusion",
            "loop_diffusion",
            "damping_low",
            "damping_mid",
            "damping_low_hz",
            "damping_mid_hz",
            "damping_mid_q",
            "delay_mode",
            "mod_depth",
            "mod_rate",
            "matrix",
        ] {
            let p = ReverbParams::default();
            // Shape knobs need nonzero band loss; modulation knobs need an active mode.
            if control.starts_with("damping_") {
                set(&p.damping_low, 8.0);
                set(&p.damping_mid, 8.0);
            }
            if control.starts_with("mod_") {
                set(&p.delay_mode, DelayReadMode::RandomLinear);
            }
            let reference = if control.starts_with("damping_") || control.starts_with("mod_") {
                render(&p)
            } else {
                baseline.clone()
            };
            match control {
                "t60" => set(&p.t60, 0.15),
                "damping" => set(&p.damping, 300.0),
                "diffusion" => set(&p.diffusion, 1.0),
                "loop_diffusion" => set(&p.loop_diffusion, 1.0),
                "damping_low" => set(&p.damping_low, 24.0),
                "damping_mid" => set(&p.damping_mid, 24.0),
                "damping_low_hz" => set(&p.damping_low_hz, 1500.0),
                "damping_mid_hz" => set(&p.damping_mid_hz, 3000.0),
                "damping_mid_q" => set(&p.damping_mid_q, 4.0),
                "delay_mode" => set(&p.delay_mode, DelayReadMode::RandomAllpass),
                "mod_depth" => set(&p.mod_depth, 1.0),
                "mod_rate" => set(&p.mod_rate, 2.0),
                "matrix" => set(&p.matrix, MatrixMode::Hadamard),
                _ => unreachable!(),
            }
            let changed = render(&p);
            let difference: f64 = changed
                .iter()
                .zip(&reference)
                .map(|(a, b)| f64::from(a - b).powi(2))
                .sum();
            assert!(difference > 1e-12, "inert control: {control}, {difference}");
        }
    }

    #[test]
    #[ignore = "manual release-mode CPU measurement, not a timing assertion"]
    fn difffdn_cpu_measurement() {
        for mode in [DelayReadMode::Static, DelayReadMode::RandomAllpass] {
            let p = ReverbParams::default();
            set(&p.delay_mode, mode);
            let mut engine = PluginEngine::new(48000.0, &p).unwrap();
            let mut input = [0.0_f32; 128];
            for (i, x) in input.iter_mut().enumerate() {
                *x = (i as f32 * 0.13).sin() * 0.1;
            }
            for _ in 0..375 {
                let mut left = input;
                let mut right = input;
                process(&mut engine, &p, &mut left, &mut right);
            }
            let start = std::time::Instant::now();
            let mut checksum = 0.0_f64;
            for _ in 0..3750 {
                let mut left = input;
                let mut right = input;
                process(&mut engine, &p, &mut left, &mut right);
                checksum += f64::from(std::hint::black_box(left[127]));
            }
            eprintln!(
                "CPU static={}: 10 s audio / {:?} processing; checksum={checksum:.12}",
                mode == DelayReadMode::Static,
                start.elapsed()
            );
        }
    }

    #[test]
    fn difffdn_leveled_mix_preserves_power_and_exact_endpoints() {
        for (gain, db) in DIFFFDN_WET_GAINS.into_iter().zip([-25.4, -21.5]) {
            assert!((20.0 * gain.log10() - db).abs() < 1e-10);
        }
        assert_eq!(difffdn_mix_gains(0.0), (1.0, 0.0));
        assert_eq!(difffdn_mix_gains(1.0), (0.0, 1.0));
        for i in 0..=100 {
            let (dry, wet) = difffdn_mix_gains(i as f32 / 100.0);
            assert!((dry * dry + wet * wet - 1.0).abs() < 2e-7);
        }
        let params = ReverbParams::default();
        set(&params.wet, 0.0);
        set(&params.output_db, 0.0);
        let mut engine = PluginEngine::new(48000.0, &params).unwrap();
        let mut left = [0.125; 512];
        let mut right = [-0.25; 512];
        process(&mut engine, &params, &mut left, &mut right);
        assert_eq!(left, [0.125; 512]);
        assert_eq!(right, [-0.25; 512]);
    }

    #[test]
    fn difffdn_bypass_relative_level_measurement() {
        let rate = 48000;
        // Log-spaced equal-amplitude tones approximate equal power per octave.
        // Keep peaks low so the safety limiter does not hide the linear gain.
        let input: Vec<f32> = (0..rate * 6)
            .map(|i| {
                (0..48)
                    .map(|band| {
                        let hz = 30.0 * (16000.0_f64 / 30.0).powf(band as f64 / 47.0);
                        let phase = band as f64 * (band + 1) as f64 * 0.37;
                        (std::f64::consts::TAU * hz * i as f64 / rate as f64 + phase).sin()
                    })
                    .sum::<f64>() as f32
                    * 0.002
            })
            .collect();
        let power = |samples: &[f32]| {
            samples[rate * 2..]
                .iter()
                .map(|&x| f64::from(x).powi(2))
                .sum::<f64>()
        };
        let dry_power = power(&input);
        for room in [RoomPreset::Salon, RoomPreset::Damped] {
            for wet in [0.0, 0.35, 0.7, 1.0] {
                for output_db in [0.0, 6.0] {
                    let params = ReverbParams::default();
                    set(&params.room, room);
                    set(&params.wet, wet);
                    set(&params.output_db, output_db);
                    let mut engine = PluginEngine::new(rate as f32, &params).unwrap();
                    let mut left = input.clone();
                    let mut right = input.clone();
                    process(&mut engine, &params, &mut left, &mut right);
                    assert_eq!(left, right);
                    assert!(left.iter().all(|x| x.is_finite()));
                    let db = 10.0 * (power(&left) / dry_power).log10();
                    let peak = left.iter().fold(0.0_f32, |a, x| a.max(x.abs()));
                    eprintln!(
                        "{room:?}: wet={wet:.2}, output={output_db:+.0} dB, bypass-relative RMS={db:+.2} dB, peak={peak:.4}"
                    );
                    if wet == 0.0 {
                        assert!((db - f64::from(output_db)).abs() < 0.001);
                    } else {
                        assert!(
                            (db - f64::from(output_db)).abs() < 1.0,
                            "{room:?}: wet={wet}, output={output_db}, relative RMS={db}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    #[ignore = "manual hot-input limiter diagnostic"]
    fn difffdn_hot_input_level_measurement() {
        for room in [RoomPreset::Salon, RoomPreset::Damped] {
            for amplitude in [0.1, 1.0, 2.0] {
                let params = ReverbParams::default();
                set(&params.room, room);
                set(&params.output_db, 6.0);
                let mut engine = PluginEngine::new(48000.0, &params).unwrap();
                let input: Vec<f32> = (0..96000)
                    .map(|i| amplitude * (std::f32::consts::TAU * 997.0 * i as f32 / 48000.0).sin())
                    .collect();
                let mut left = input.clone();
                let mut right = input.clone();
                process(&mut engine, &params, &mut left, &mut right);
                let power = |x: &[f32]| {
                    x[48000..]
                        .iter()
                        .map(|&s| f64::from(s).powi(2))
                        .sum::<f64>()
                };
                eprintln!(
                    "{room:?}: input peak={amplitude}, wet=0.35, output=+6 dB, bypass-relative RMS={:+.2} dB, final limiter reduction={:.2} dB",
                    10.0 * (power(&left) / power(&input)).log10(),
                    -20.0 * engine.limiter.gain().log10()
                );
                assert!(
                    left.iter()
                        .all(|x| x.is_finite() && x.abs() <= live_limiter::CEILING + 1e-6)
                );
            }
        }
    }

    #[test]
    fn difffdn_headroom_for_impulses_and_sustained_signals() {
        for rate in [8000, 44100, 48000, 96000, 192000, 384000] {
            for room in [RoomPreset::Salon, RoomPreset::Damped] {
                let p = ReverbParams::default();
                set(&p.room, room);
                set(&p.wet, 1.0);
                let mut engine = PluginEngine::new(rate as f32, &p).unwrap();
                let mut peak = 0.0_f32;
                let mut energy = 0.0_f64;
                for i in 0..rate * 2 {
                    let t = i as f64 / rate as f64;
                    // Impulse, DC, low tone, swept tone, then tail.
                    let input = if i == 0 {
                        1.0
                    } else if t < 0.25 {
                        0.0
                    } else if t < 0.5 {
                        0.5
                    } else if t < 0.75 {
                        (std::f64::consts::TAU * 80.0 * t).sin() as f32 * 0.5
                    } else if t < 1.5 {
                        (std::f64::consts::TAU
                            * (100.0 * t + 0.15 * rate as f64 * (t - 0.75).powi(2)))
                        .sin() as f32
                            * 0.5
                    } else {
                        0.0
                    };
                    let wet = engine.reverb.process(input, room)[0];
                    assert!(wet.is_finite());
                    peak = peak.max(wet.abs());
                    energy += f64::from(wet).powi(2);
                }
                eprintln!(
                    "headroom: {rate} {room:?}: wet peak {peak:.6} ({:.2} dBFS), RMS {:.2} dBFS",
                    20.0 * peak.log10(),
                    10.0 * (energy / f64::from(rate * 2)).log10()
                );
                assert!(
                    peak < 0.8
                        * DIFFFDN_WET_GAINS[if room == RoomPreset::Salon { 0 } else { 1 }] as f32
                        / 0.025_118_864,
                    "{rate} {room:?}: excessive pre-limiter wet peak {peak}"
                );
                let mut left = [20.0; 512];
                let mut right = left;
                process(&mut engine, &p, &mut left, &mut right);
                assert!(
                    left.iter()
                        .chain(&right)
                        .all(|x| x.is_finite() && x.abs() <= live_limiter::CEILING + 1e-6)
                );
            }
        }
    }

    #[test]
    fn plugin_wet_impulse_is_dual_mono_and_reset_is_reproducible() {
        let params = ReverbParams::default();
        set(&params.delay_mode, DelayReadMode::RandomAllpass);
        set(&params.loop_diffusion, 0.8);
        set(&params.wet, 1.0);
        set(&params.output_db, 0.0);
        set(&params.diffusion, 1.0);
        let mut engine = PluginEngine::new(48000.0, &params).unwrap();
        let render = |engine: &mut PluginEngine| {
            let mut left = vec![0.0; 12000];
            let mut right = vec![0.0; 12000];
            left[0] = 1.0;
            right[0] = 1.0;
            process(engine, &params, &mut left, &mut right);
            (left, right)
        };
        let first = render(&mut engine);
        assert_eq!(first.0[0], 0.0);
        assert!(first.0.iter().any(|x| x.abs() > 1e-5));
        assert_eq!(first.0, first.1);
        engine.reset(&params);
        assert_eq!(render(&mut engine), first);
    }

    #[test]
    fn plugin_processing_is_independent_of_host_block_size() {
        let params = ReverbParams::default();
        set(&params.delay_mode, DelayReadMode::RandomAllpass);
        set(&params.duck_enabled, true);
        set(&params.loop_diffusion, 0.7);
        let mut a = PluginEngine::new(44100.0, &params).unwrap();
        let mut b = PluginEngine::new(44100.0, &params).unwrap();
        let mut left: Vec<_> = (0..8192).map(|i| (i as f32 * 0.1).sin() * 0.1).collect();
        let mut right = left.clone();
        let (mut split_l, mut split_r) = (left.clone(), right.clone());
        process(&mut a, &params, &mut left, &mut right);
        for (l, r) in split_l.chunks_mut(37).zip(split_r.chunks_mut(37)) {
            process(&mut b, &params, l, r);
        }
        assert_eq!((left, right), (split_l, split_r));
    }

    #[test]
    fn plugin_live_controls_and_sample_rates_stay_finite() {
        let params = ReverbParams::default();
        for rate in [8000.0, 44100.0, 48000.0, 96000.0, 192000.0, 384000.0] {
            let mut engine = PluginEngine::new(rate, &params).unwrap();
            for i in 0..100 {
                set(
                    &params.room,
                    if i % 2 == 0 {
                        RoomPreset::Salon
                    } else {
                        RoomPreset::Damped
                    },
                );
                set(&params.extra_damping, (i % 2) as f32);
                set(&params.matrix, MatrixMode::from_index(i % 3));
                set(&params.damping, if i % 2 == 0 { 20000.0 } else { 100.0 });
                set(&params.damping_low, if i % 2 == 0 { 24.0 } else { 0.0 });
                set(&params.damping_mid, if i % 3 == 0 { 24.0 } else { 0.0 });
                set(
                    &params.damping_low_hz,
                    if i % 2 == 0 { 20.0 } else { 2000.0 },
                );
                set(
                    &params.damping_mid_hz,
                    if i % 2 == 0 { 12000.0 } else { 100.0 },
                );
                set(&params.damping_mid_q, if i % 2 == 0 { 8.0 } else { 0.2 });
                set(&params.t60, if i % 2 == 0 { 20.0 } else { 0.1 });
                set(&params.diffusion, (i % 2) as f32);
                set(&params.loop_diffusion, (i % 2) as f32);
                set(
                    &params.delay_mode,
                    [
                        DelayReadMode::Static,
                        DelayReadMode::RandomLinear,
                        DelayReadMode::RandomAllpass,
                    ][i % 3],
                );
                set(&params.mod_depth, (i % 2) as f32);
                set(&params.mod_rate, if i % 2 == 0 { 0.05 } else { 2.0 });
                set(&params.wet, (i % 2) as f32);
                set(&params.bypass, i % 3 == 0);
                set(&params.duck_enabled, i % 2 == 0);
                set(
                    &params.duck_source,
                    if i % 3 == 0 {
                        DuckSource::External
                    } else {
                        DuckSource::Internal
                    },
                );
                set(&params.duck_attack, if i % 2 == 0 { 0.1 } else { 200.0 });
                set(&params.duck_release, if i % 2 == 0 { 10.0 } else { 3000.0 });
                let mut left = [0.1; 128];
                let mut right = [-0.05; 128];
                if i == 1 {
                    left[0] = f32::NAN;
                    right[0] = f32::INFINITY;
                }
                process(&mut engine, &params, &mut left, &mut right);
                assert!(
                    left.iter()
                        .chain(&right)
                        .all(|x| x.is_finite() && x.abs() < 4.0)
                );
                assert_eq!(engine.reverb.rooms[0].sample_rate(), rate as u32);
                assert!((0.0..=36.0).contains(&engine.duck_reduction));
            }
        }
        for rate in [0.0, 7999.0, 384001.0, 48000.5, f32::NAN, f32::INFINITY] {
            assert!(PluginEngine::new(rate, &params).is_none());
        }
    }
}
