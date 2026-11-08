//! Host adapter. Device handling and WAV loading remain in the audition app.

use std::sync::{
    Arc,
    atomic::{AtomicU32, Ordering},
};

use egui_plugin as egui;
use nice_plug::prelude::*;
use nice_plug_core::context::gui::GuiContext;
use nice_plug_egui::{
    EguiEditor, EguiEditorState, EguiNiceSettings, Frame, NiceEguiApp, RepaintNotifier,
    create_egui_editor,
    widgets::{ParamButton, ParamComboBox, ParamEnumButton, ParamKnob},
};

use crate::{Config, DelayMode, Matrix, Reverb};

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

#[derive(Params)]
struct ReverbParams {
    #[id = "matrix"]
    matrix: EnumParam<MatrixMode>,
    #[id = "diffusion"]
    diffusion: FloatParam,
    #[id = "t60"]
    t60: FloatParam,
    #[id = "damping"]
    damping: FloatParam,
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
}

impl Default for ReverbParams {
    fn default() -> Self {
        Self {
            matrix: EnumParam::new("Matrix", MatrixMode::Hybrid),
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
                "Damping",
                6000.0,
                FloatRange::Skewed {
                    min: 100.0,
                    max: 20_000.0,
                    factor: FloatRange::skew_factor(-2.0),
                },
            )
            .with_unit(" Hz")
            .with_value_to_string(Arc::new(|value| format!("{value:.0}"))),
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

#[derive(Clone, Copy, PartialEq)]
struct Targets {
    t60: f32,
    damping: f32,
    matrix: Matrix,
    diffusion: f32,
    loop_diffusion: f32,
    delay_mode: DelayMode,
    mod_depth: f32,
    mod_rate: f32,
}

impl ReverbParams {
    fn targets(&self, sample_rate: u32) -> Targets {
        Targets {
            t60: self.t60.value(),
            damping: self.damping.value().min(sample_rate as f32 * 0.5),
            matrix: self.matrix.value().into(),
            diffusion: self.diffusion.value(),
            loop_diffusion: self.loop_diffusion.value(),
            delay_mode: self.delay_mode.value().into(),
            mod_depth: self.mod_depth.value(),
            mod_rate: self.mod_rate.value(),
        }
    }
}

struct PluginEngine {
    reverb: Reverb,
    sample_rate: u32,
    targets: Targets,
    wet: f32,
    gain: f32,
    bypass: f32,
    smoothing: f32,
    duck_reduction: f32,
}

impl PluginEngine {
    fn new(sample_rate: f32, params: &ReverbParams) -> Option<Self> {
        // The DSP uses integer sample rates; don't silently detune fractional rates.
        if !sample_rate.is_finite() || sample_rate.fract() != 0.0 {
            return None;
        }
        let sample_rate = sample_rate as u32;
        let targets = params.targets(sample_rate);
        let mut reverb = Reverb::new(Config {
            sample_rate,
            t60_seconds: targets.t60,
            damping_hz: targets.damping,
            matrix: targets.matrix,
        })
        .ok()?;
        reverb.set_diffusion(targets.diffusion).ok()?;
        reverb.diffusion = targets.diffusion;
        reverb.set_loop_diffusion(targets.loop_diffusion).ok()?;
        reverb.loop_diffusion = targets.loop_diffusion;
        reverb
            .set_delay_modulation(targets.delay_mode, targets.mod_depth, targets.mod_rate)
            .ok()?;
        reverb.delay_weights = reverb.target_delay_weights;
        reverb.mod_depth_ms = targets.mod_depth;
        reverb.mod_rate_hz = targets.mod_rate;
        reverb.reset();
        Some(Self {
            reverb,
            sample_rate,
            targets,
            wet: params.wet.value(),
            gain: util::db_to_gain(params.output_db.value()),
            bypass: if params.bypass.value() { 1.0 } else { 0.0 },
            smoothing: 1.0 - (-1.0 / (0.01 * sample_rate as f32)).exp(),
            duck_reduction: 0.0,
        })
    }

    fn update(&mut self, params: &ReverbParams) {
        let next = params.targets(self.sample_rate);
        if next != self.targets {
            // Host parameters are range checked, damping is clamped to Nyquist.
            let _ = self
                .reverb
                .set_parameters(next.t60, next.damping, next.matrix);
            let _ = self.reverb.set_diffusion(next.diffusion);
            let _ = self.reverb.set_loop_diffusion(next.loop_diffusion);
            let _ =
                self.reverb
                    .set_delay_modulation(next.delay_mode, next.mod_depth, next.mod_rate);
            self.targets = next;
        }
    }

    fn reset(&mut self, params: &ReverbParams) {
        self.update(params);
        self.reverb.reset();
        // Host reset restores the current parameter state without allocating or
        // carrying coefficient ramps over from the previous transport position.
        self.reverb.matrix_weights = self.reverb.target_matrix_weights;
        self.reverb.damping_coefficient = self.reverb.target_damping_coefficient;
        self.reverb.diffusion = self.reverb.target_diffusion;
        self.reverb.loop_diffusion = self.reverb.target_loop_diffusion;
        self.reverb.delay_weights = self.reverb.target_delay_weights;
        self.reverb.mod_depth_ms = self.reverb.target_mod_depth_ms;
        self.reverb.mod_rate_hz = self.reverb.target_mod_rate_hz;
        for delay in &mut self.reverb.delays {
            delay.gain = delay.target_gain;
        }
        self.wet = params.wet.value();
        self.gain = util::db_to_gain(params.output_db.value());
        self.bypass = if params.bypass.value() { 1.0 } else { 0.0 };
        self.duck_reduction = 0.0;
    }

    fn process(
        &mut self,
        buffer: &mut Buffer,
        params: &ReverbParams,
        sidechain: Option<&Buffer>,
    ) -> f32 {
        self.update(params);
        let target_wet = params.wet.value();
        let target_gain = util::db_to_gain(params.output_db.value());
        let target_bypass = if params.bypass.value() { 1.0 } else { 0.0 };
        let mut peak = 0.0_f32;
        let coefficient = |ms: f32| 1.0 - (-1.0 / (0.001 * ms * self.sample_rate as f32)).exp();
        let attack = coefficient(params.duck_attack.value());
        let release = coefficient(params.duck_release.value());
        let sidechain = sidechain.map(Buffer::as_slice_immutable);
        for (index, mut frame) in buffer.iter_samples().enumerate() {
            // Reject invalid host samples before they can poison the feedback state.
            let finite = |x: f32| if x.is_finite() { x } else { 0.0 };
            let dry = [
                finite(*frame.get_mut(0).unwrap()),
                finite(*frame.get_mut(1).unwrap()),
            ];
            let wet = self.reverb.process_sample(dry[0] * 0.5 + dry[1] * 0.5);
            // Stereo peak linking avoids cancellation of anti-phase trigger signals.
            // Missing/disconnected external input means silence, never internal fallback.
            let detector = match params.duck_source.value() {
                DuckSource::Internal => dry[0].abs().max(dry[1].abs()),
                DuckSource::External => sidechain.map_or(0.0, |channels| {
                    channels
                        .iter()
                        .filter_map(|channel| channel.get(index))
                        .fold(0.0_f32, |peak, &sample| peak.max(finite(sample).abs()))
                }),
            };
            // One dB of reduction per dB above threshold, capped by Amount.
            let target_reduction = if params.duck_enabled.value() {
                (20.0 * detector.max(1e-20).log10() - params.duck_threshold.value())
                    .clamp(0.0, params.duck_amount.value())
            } else {
                0.0
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
            for channel in 0..2 {
                let effected = self.gain
                    * ((1.0 - self.wet) * dry[channel] + self.wet * duck_gain * wet[channel]);
                // True bypass crossfade: unity dry, not the audition app's gain-retaining bypass.
                let output = (1.0 - self.bypass) * effected + self.bypass * dry[channel];
                *frame.get_mut(channel).unwrap() = output;
                peak = peak.max(output.abs());
            }
        }
        peak
    }
}

#[derive(Default)]
struct AlgoReverb {
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
        names: PortNames {
            aux_inputs: &["Sidechain"],
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
                nice_plug_core::editor::dpi::LogicalSize::new(520.0, 700.0),
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
            },
        )
    }

    fn activate(
        &mut self,
        layout: &AudioIOLayout,
        config: &BufferConfig,
        _context: &mut impl ActivateContext<Self>,
    ) -> bool {
        if layout.main_input_channels != NonZeroU32::new(2)
            || layout.main_output_channels != NonZeroU32::new(2)
        {
            return false;
        }
        self.engine = PluginEngine::new(config.sample_rate, &self.params);
        self.engine.is_some()
    }

    fn reset(&mut self) {
        if let Some(engine) = &mut self.engine {
            engine.reset(&self.params);
        }
        self.peak.store(0, Ordering::Relaxed);
        self.reduction.store(0, Ordering::Relaxed);
    }

    fn process(
        &mut self,
        buffer: &mut Buffer,
        aux: &mut AuxiliaryBuffers,
        _context: &mut impl ProcessContext<Self>,
    ) -> ProcessStatus {
        let Some(engine) = &mut self.engine else {
            return ProcessStatus::Error("Plugin is not activated");
        };
        if buffer.channels() != 2 {
            return ProcessStatus::Error("Stereo I/O required");
        }
        let peak = engine.process(buffer, &self.params, aux.inputs.first());
        self.reduction
            .store(engine.duck_reduction.to_bits(), Ordering::Relaxed);
        // Nonnegative finite floats have the same ordering as their bit patterns.
        self.peak.fetch_max(peak.to_bits(), Ordering::Relaxed);
        // Conservative fixed tail avoids cutting off a long tail when T60 is automated down.
        // This is a host scheduling hint, not an assertion of exact silence at this time.
        ProcessStatus::Tail(engine.sample_rate * 61)
    }
}

impl ClapPlugin for AlgoReverb {
    const CLAP_ID: &'static str = "com.algo.algo-reverb";
    const CLAP_DESCRIPTION: Option<&'static str> =
        Some("Eight-line FDN with hybrid mixing and input diffusion");
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
                    ui.add(ParamKnob::for_param(*param, setter).with_size(48.0));
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
        egui::Frame::new()
            .inner_margin(16.0)
            .show(ui, |ui| self.draw_contents(ui));
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
        ui.label("8-line FDN · Householder -> Hadamard");
        ui.add_space(8.0);
        ui.separator();
        ui.horizontal(|ui| {
            ui.label("Matrix");
            ui.add(ParamComboBox::for_param(&self.params.matrix, &setter).with_width(260.0));
        });
        let modulation = egui::CollapsingHeader::new("Delay Modulation")
            .default_open(false)
            .show(ui, |ui| {
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
            });
        // Keep enough room throughout the collapse animation. Send only on a
        // state change, rather than repeatedly asking a rejecting host to resize.
        let expanded = modulation.body_returned.is_some();
        let resize_id = ui.scope_id().with("modulation_window_expanded");
        let previous = ui.data_mut(|data| {
            let previous = data.get_temp::<bool>(resize_id);
            data.insert_temp(resize_id, expanded);
            previous
        });
        if previous != Some(expanded) {
            ui.send_viewport_cmd(egui::ViewportCommand::InnerSize(egui::vec2(
                520.0,
                if expanded { 820.0 } else { 700.0 },
            )));
        }
        for row in [
            &self.params.diffusion,
            &self.params.t60,
            &self.params.damping,
            &self.params.wet,
            &self.params.output_db,
            &self.params.loop_diffusion,
        ]
        .chunks(3)
        {
            knob_row(ui, row, &setter);
        }
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
        let peak = f32::from_bits(self.peak.swap(0, Ordering::Relaxed));
        self.held_peak = self.held_peak.max(peak);
        ui.horizontal(|ui| {
            let peak_text = egui::RichText::new(format!(
                "Peak since reset: {:.1} dBFS",
                20.0 * self.held_peak.max(1e-6).log10()
            ));
            if self.held_peak > 1.0 {
                ui.label(peak_text.color(egui::Color32::YELLOW))
                    .on_hover_text("Above 0 dBFS — lower output. No limiter.");
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
    fn delay_modulation_opens_and_fits_default_window() {
        assert_editor_contents_fit_default_window(2.0, true);
    }

    fn assert_editor_contents_fit_default_window(peak: f32, open_modulation: bool) {
        let mut editor = ReverbEditor {
            params: Arc::new(ReverbParams::default()),
            peak: Arc::new(AtomicU32::new(peak.to_bits())),
            reduction: Arc::new(AtomicU32::new(0)),
            context: Some(GuiContext::new(Arc::new(LayoutHost))),
            held_peak: 0.0,
        };
        let ctx = egui::Context::default();
        for theme in [egui::Theme::Dark, egui::Theme::Light] {
            ctx.style_mut_of(theme, |style| style.animation_time = 0.0);
        }
        ctx.set_fonts(editor_fonts());
        for theme in [egui::Theme::Dark, egui::Theme::Light] {
            ctx.style_mut_of(theme, |style| {
                for font in style.text_styles.values_mut() {
                    font.size = 14.0;
                }
            });
        }
        let mut header_position = egui::Pos2::ZERO;
        let mut window_size = egui::vec2(520.0, 700.0);
        let mut requested_sizes = Vec::new();
        for frame in 0..9 {
            let previous_window_size = window_size;
            let mut bounds = egui::Rect::NOTHING;
            let mut output = ctx.run_ui(
                egui::RawInput {
                    time: Some(frame as f64 * 0.1),
                    events: if open_modulation && [1, 2, 5, 6].contains(&frame) {
                        vec![
                            egui::Event::PointerMoved(header_position),
                            egui::Event::PointerButton {
                                pos: header_position,
                                button: egui::PointerButton::Primary,
                                pressed: frame == 1 || frame == 5,
                                modifiers: egui::Modifiers::NONE,
                            },
                        ]
                    } else {
                        vec![]
                    },
                    screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, window_size)),
                    ..Default::default()
                },
                |ui| {
                    editor.draw(ui);
                    bounds = ui.min_rect();
                },
            );
            output.textures_delta.clear();
            for viewport in output.viewport_output.values() {
                for command in &viewport.commands {
                    if let egui::ViewportCommand::InnerSize(size) = command {
                        requested_sizes.push(*size);
                        window_size = *size;
                    }
                }
            }
            // The host applies a viewport resize after this frame. Expanded
            // contents may be clipped until the following frame arrives.
            if window_size != previous_window_size {
                continue;
            }
            let labels: Vec<_> = output
                .shapes
                .iter()
                .filter_map(|shape| match &shape.shape {
                    egui::epaint::Shape::Text(text) => Some(text),
                    _ => None,
                })
                .collect();
            let header = labels
                .iter()
                .find(|text| text.galley.job.text == "Delay Modulation")
                .expect("modulation header must remain visible");
            header_position = header.pos + header.galley.size() * 0.5;
            if frame == 0 || frame == 4 || frame == 8 {
                assert_eq!(
                    labels
                        .iter()
                        .any(|text| text.galley.job.text == "Mod Depth"),
                    open_modulation && frame == 4
                );
                assert_eq!(
                    labels.iter().any(|text| text.galley.job.text == "Mod Rate"),
                    open_modulation && frame == 4
                );
            }
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
        assert_eq!(
            requested_sizes,
            if open_modulation {
                vec![
                    egui::vec2(520.0, 700.0),
                    egui::vec2(520.0, 820.0),
                    egui::vec2(520.0, 700.0),
                ]
            } else {
                vec![egui::vec2(520.0, 700.0)]
            }
        );
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
                            egui::epaint::Shape::Circle(circle) if circle.radius > 20.0 => {
                                Some(circle)
                            }
                            _ => None,
                        })
                        .collect();
                    assert!(!knobs.is_empty(), "expected visible knobs");
                    for knob in knobs {
                        let label = labels
                            .iter()
                            .min_by(|a, b| {
                                (a.center().x - knob.center.x)
                                    .abs()
                                    .total_cmp(&(b.center().x - knob.center.x).abs())
                            })
                            .unwrap();
                        let gap = knob.center.y - knob.radius - label.bottom();
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
        assert_eq!(knob_label(&params.damping), "Damping");
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
        assert_eq!(params.output_db.default_plain_value(), -6.0);
        assert_eq!(params.output_db.value(), -6.0);
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
        let params = ReverbParams::default();
        set(&params.duck_enabled, true);
        set(&params.duck_source, DuckSource::External);
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
                "mod_rate"
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
    fn plugin_dry_and_bypass_preserve_stereo_without_clamping() {
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
            assert_eq!((left, right), expected);
        }
    }

    #[test]
    fn plugin_wet_impulse_is_stereo_and_reset_is_reproducible() {
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
        assert_ne!(first.0, first.1);
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
                set(&params.matrix, MatrixMode::from_index(i % 3));
                set(&params.damping, if i % 2 == 0 { 20000.0 } else { 100.0 });
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
                assert!(engine.targets.damping <= rate * 0.5);
                assert!((0.0..=36.0).contains(&engine.duck_reduction));
            }
        }
        for rate in [0.0, 7999.0, 384001.0, 48000.5, f32::NAN, f32::INFINITY] {
            assert!(PluginEngine::new(rate, &params).is_none());
        }
    }
}
