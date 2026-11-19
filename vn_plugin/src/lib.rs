//! Zero-added-latency IR playback with IR-age damping and live Wet tone filtering.
mod autolevel;
#[path = "zero_convolution.rs"]
mod convolution;
mod damping;
#[path = "../../src/plugin/dual_knob.rs"]
mod dual_knob;
mod file_browser;
mod limiter;

use algo_reverb::ir_analysis::Impulse;
use convolution::{Convolver, LATENCY, resample};
use egui_plugin as egui;
use nice_plug::prelude::*;
use nice_plug_core::context::gui::GuiContext;
use nice_plug_egui::{
    EguiEditor, EguiEditorState, EguiNiceSettings, Frame, NiceEguiApp, RepaintNotifier,
    create_egui_editor, widgets::ParamButton,
};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicU32, Ordering},
};

// Store the source IR, not a /tmp filename: sessions survive moved/deleted WAVs.
type StoredIr = (String, u32, Vec<[f64; 2]>);
#[derive(Params)]
struct VnParams {
    #[nested]
    damping: damping::DampingParams,
    #[persist = "ir_damping_applied_v1"]
    ir_damping_applied: Mutex<Option<[f32; 8]>>,
    #[id = "wet"]
    wet: FloatParam,
    #[id = "output_db"]
    output: FloatParam,
    #[id = "bypass"]
    bypass: BoolParam,
    #[id = "safety_limiter"]
    safety_limiter: BoolParam,
    #[id = "auto_level"]
    auto_level: BoolParam,
    #[persist = "calibration_reference_v1"]
    calibration_reference: Mutex<Option<(u32, Vec<[f64; 2]>)>>,
    #[persist = "source_ir"]
    source: Mutex<StoredIr>,
    // UI location only. Playback always restores the embedded source, not this path.
    #[persist = "wav_location_v1"]
    wav_location: Mutex<(std::path::PathBuf, std::path::PathBuf)>,
}
impl Default for VnParams {
    fn default() -> Self {
        Self {
            damping: damping::DampingParams::default(),
            ir_damping_applied: Mutex::new(None),
            wet: FloatParam::new("Dry / Wet", 0.35, FloatRange::Linear { min: 0.0, max: 1.0 }),
            output: FloatParam::new(
                "Output",
                -12.0,
                FloatRange::Linear {
                    min: -60.0,
                    max: 6.0,
                },
            )
            .with_unit(" dB"),
            bypass: BoolParam::new("Bypass", false).make_bypass(),
            safety_limiter: BoolParam::new("Safety Limiter", true),
            auto_level: BoolParam::new("Pink Auto Level", false),
            calibration_reference: Mutex::new(None),
            source: Mutex::new((String::new(), 48000, Vec::new())),
            wav_location: Mutex::default(),
        }
    }
}

#[derive(Default)]
struct Pending {
    engine: Option<Convolver>,
    ready: bool,
}

#[derive(Default)]
struct VnPlugin {
    wet_filter: damping::WetFilter,
    params: Arc<VnParams>,
    pending: Arc<Mutex<Pending>>,
    rate: Arc<AtomicU32>,
    engine: Option<Convolver>,
    mix: f32,
    gain: f32,
    bypass: f32,
    smoothing: f32,
    wet_gain: f32,
    limiter: limiter::Limiter,
    limiter_meter: Arc<AtomicU32>,
}

impl Plugin for VnPlugin {
    const NAME: &'static str = "Algo VN";
    const VENDOR: &'static str = "Algo Reverb";
    const URL: &'static str = "";
    const EMAIL: &'static str = "";
    const VERSION: &'static str = env!("CARGO_PKG_VERSION");
    const AUDIO_IO_LAYOUTS: &'static [AudioIOLayout] = &[AudioIOLayout {
        main_input_channels: NonZeroU32::new(2),
        main_output_channels: NonZeroU32::new(2),
        ..AudioIOLayout::const_default()
    }];
    const SAMPLE_ACCURATE_AUTOMATION: bool = true;
    type Editor = EguiEditor<VnEditor>;
    type SysExMessage = ();
    type BackgroundTask = ();

    fn params(&self) -> Arc<dyn Params> {
        self.params.clone()
    }
    fn editor(&mut self, _executor: AsyncExecutor<Self>) -> Option<Self::Editor> {
        create_egui_editor(
            EguiEditorState::from_size(
                nice_plug_core::editor::dpi::LogicalSize::new(520.0, 735.0),
                1.0,
            ),
            RepaintNotifier::new(),
            EguiNiceSettings::default(),
            VnEditor {
                params: self.params.clone(),
                pending: self.pending.clone(),
                rate: self.rate.clone(),
                context: None,
                status: String::new(),
                limiter_meter: self.limiter_meter.clone(),
                ir_job: None,
                ir_observed: None,
                file_browser: None,
                wav_folder: file_browser::WavFolder::default(),
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
            || !config.sample_rate.is_finite()
            || config.sample_rate.fract() != 0.0
            || !(8000.0..=192000.0).contains(&config.sample_rate)
        {
            return false;
        }
        let rate = config.sample_rate as u32;
        // Activation is outside processing; prepare everything synchronously for offline renders.
        let source = self.params.source.lock().unwrap();
        let frames = if source.2.is_empty() {
            vec![[0.0; 2]]
        } else {
            match resample(&source.2, source.1, rate) {
                Ok(v) => v,
                Err(_) => return false,
            }
        };
        let processed = self
            .params
            .ir_damping_applied
            .lock()
            .unwrap()
            .map(|v| damping::render(&frames, rate, damping::Settings::unpack(v)));
        self.engine = match Convolver::new(processed.as_deref().unwrap_or(&frames), rate) {
            Ok(v) => Some(v),
            Err(_) => return false,
        };
        let reference = self.params.calibration_reference.lock().unwrap();
        let calibration = match reference.as_ref() {
            Some((source_rate, ir)) => match resample(ir, *source_rate, rate) {
                Ok(v) => v,
                Err(_) => return false,
            },
            None => frames,
        };
        self.engine.as_mut().unwrap().calibration_gain = autolevel::gain(&calibration, rate);
        drop(reference);
        self.rate.store(rate, Ordering::Release);
        drop(source);
        self.smoothing = 1.0 - (-1.0 / (0.02 * rate as f32)).exp();
        self.limiter.configure(rate);
        context.set_latency_samples(LATENCY);
        self.reset();
        true
    }
    fn reset(&mut self) {
        if let Some(engine) = &mut self.engine {
            engine.reset();
        }
        self.mix = self.params.wet.value();
        self.gain = util::db_to_gain(self.params.output.value());
        self.bypass = if self.params.bypass.value() { 1.0 } else { 0.0 };
        self.wet_gain = if self.params.auto_level.value() {
            self.engine.as_ref().map_or(1.0, |v| v.calibration_gain)
        } else {
            1.0
        };
        self.limiter.reset();
        self.wet_filter
            .reset(self.rate.load(Ordering::Acquire), self.params.damping.wet());
        self.limiter_meter
            .store(1.0_f32.to_bits(), Ordering::Relaxed);
    }
    fn process(
        &mut self,
        buffer: &mut Buffer,
        _aux: &mut AuxiliaryBuffers,
        _context: &mut impl ProcessContext<Self>,
    ) -> ProcessStatus {
        if buffer.channels() != 2 {
            return ProcessStatus::Error("Stereo I/O required");
        }
        // Nonblocking handoff. The retired engine stays in Pending and is freed by
        // the GUI on the next load (or during teardown), never on the audio thread.
        if let Ok(mut pending) = self.pending.try_lock()
            && pending.ready
            && pending
                .engine
                .as_ref()
                .is_some_and(|v| v.rate == self.rate.load(Ordering::Acquire))
        {
            std::mem::swap(&mut self.engine, &mut pending.engine);
            pending.ready = false;
        }
        let Some(engine) = &mut self.engine else {
            return ProcessStatus::Error("Not activated");
        };
        let target_mix = self.params.wet.value();
        let target_gain = util::db_to_gain(self.params.output.value());
        let target_bypass = if self.params.bypass.value() { 1.0 } else { 0.0 };
        let limit = self.params.safety_limiter.value();
        let target_wet_gain = if self.params.auto_level.value() {
            engine.calibration_gain
        } else {
            1.0
        };
        let mut minimum_gain = 1.0_f32;
        self.wet_filter.update(self.params.damping.wet());
        for mut frame in buffer.iter_samples() {
            let input = [*frame.get_mut(0).unwrap(), *frame.get_mut(1).unwrap()]
                .map(|v| if v.is_finite() { v } else { 0.0 });
            let (dry, wet) = engine.process(input);
            self.mix += self.smoothing * (target_mix - self.mix);
            self.gain += self.smoothing * (target_gain - self.gain);
            self.wet_gain += self.smoothing * (target_wet_gain - self.wet_gain);
            self.bypass += self.smoothing * (target_bypass - self.bypass);
            let wet = self.wet_filter.process(wet).map(|v| v * self.wet_gain);
            let effect = std::array::from_fn(|ch| {
                self.gain * ((1.0 - self.mix) * dry[ch] + self.mix * wet[ch])
            });
            let effect = self.limiter.process(effect, limit);
            minimum_gain = minimum_gain.min(self.limiter.gain());
            for ch in 0..2 {
                *frame.get_mut(ch).unwrap() =
                    (1.0 - self.bypass) * effect[ch] + self.bypass * dry[ch];
            }
        }
        self.limiter_meter
            .store(minimum_gain.to_bits(), Ordering::Relaxed);
        ProcessStatus::Tail((engine.frames as u32).saturating_add(self.wet_filter.tail_samples()))
    }
}

impl ClapPlugin for VnPlugin {
    const CLAP_ID: &'static str = "com.algo.algo-vn";
    const CLAP_DESCRIPTION: Option<&'static str> =
        Some("Zero-added-latency IR playback with IR time damping and Wet tone filtering");
    const CLAP_MANUAL_URL: Option<&'static str> = None;
    const CLAP_SUPPORT_URL: Option<&'static str> = None;
    const CLAP_FEATURES: &'static [ClapFeature] = &[
        ClapFeature::AudioEffect,
        ClapFeature::Reverb,
        ClapFeature::Stereo,
    ];
}
impl Vst3Plugin for VnPlugin {
    const VST3_CLASS_ID: [u8; 16] = *b"pnkAlgoVNconv001";
    const VST3_SUBCATEGORIES: &'static [Vst3SubCategory] =
        &[Vst3SubCategory::Fx, Vst3SubCategory::Reverb];
}

struct VnEditor {
    params: Arc<VnParams>,
    pending: Arc<Mutex<Pending>>,
    rate: Arc<AtomicU32>,
    context: Option<GuiContext>,
    status: String,
    limiter_meter: Arc<AtomicU32>,
    ir_job: Option<std::thread::JoinHandle<Result<IrDampingResult, String>>>,
    ir_observed: Option<([f32; 8], Option<std::time::Instant>)>,
    file_browser: Option<file_browser::FileBrowser>,
    wav_folder: file_browser::WavFolder,
}

struct IrDampingResult {
    engine: Convolver,
    source: StoredIr,
    reference: Option<(u32, Vec<[f64; 2]>)>,
    rate: u32,
    settings: [f32; 8],
}

const AUTO_REBUILD_DELAY: std::time::Duration = std::time::Duration::from_millis(300);

// Disabled damping always means the untouched IR, regardless of knob positions.
// None is the legacy/default state: activation uses the untouched IR in that case.
fn ir_settings_match(applied: Option<[f32; 8]>, selected: [f32; 8]) -> bool {
    if selected[0] == 0.0 {
        applied.is_none_or(|settings| settings[0] == 0.0)
    } else {
        applied == Some(selected)
    }
}

impl VnEditor {
    fn load_selected_wav(&mut self, path: &std::path::Path) {
        self.status = match self.load(path) {
            Ok(()) => {
                *self.params.wav_location.lock().unwrap() = (
                    path.parent().unwrap_or(path).to_path_buf(),
                    path.to_path_buf(),
                );
                "IR loaded. Previous tail cleared.".into()
            }
            Err(e) => format!("Load failed (previous IR retained): {e}"),
        };
    }

    fn apply_ir_damping(&mut self) -> Result<(), String> {
        if self.ir_job.is_some() {
            return Err("Wait for the current IR damping job".into());
        }
        let source = self.params.source.lock().unwrap().clone();
        if source.2.is_empty() {
            return Err("Load a WAV first".into());
        }
        let rate = self.rate.load(Ordering::Acquire);
        if rate == 0 {
            return Err("Activate the plugin first".into());
        }
        let settings = self.params.damping.ir();
        let reference = self.params.calibration_reference.lock().unwrap().clone();
        self.ir_job = Some(std::thread::spawn(move || {
            let converted = resample(&source.2, source.1, rate)?;
            let mut engine = Convolver::new(&damping::render(&converted, rate, settings), rate)?;
            // Calibrate against the untouched reference, never undo attenuation.
            engine.calibration_gain = match reference.as_ref() {
                Some((r, frames)) => autolevel::gain(&resample(frames, *r, rate)?, rate),
                None => autolevel::gain(&converted, rate),
            };
            Ok(IrDampingResult {
                engine,
                source,
                reference,
                rate,
                settings: settings.pack(),
            })
        }));
        self.status = "Preparing IR damping in background…".into();
        Ok(())
    }

    fn finish_ir_damping(&mut self) -> Result<(), String> {
        let result = self
            .ir_job
            .take()
            .unwrap()
            .join()
            .map_err(|_| "IR damping worker panicked")??;
        let source = self.params.source.lock().unwrap();
        let reference = self.params.calibration_reference.lock().unwrap();
        let mut pending = self.pending.lock().unwrap();
        if result.rate != self.rate.load(Ordering::Acquire)
            || !ir_settings_match(Some(result.settings), self.params.damping.ir().pack())
            || result.source != *source
            || result.reference != *reference
        {
            self.ir_observed = None;
            return Err("IR source, rate or settings changed; discarded stale damping".into());
        }
        *self.params.ir_damping_applied.lock().unwrap() = Some(result.settings);
        *pending = Pending {
            engine: Some(result.engine),
            ready: true,
        };
        self.status = "IR damping applied automatically; convolution history cleared.".into();
        Ok(())
    }

    fn poll_ir_damping(&mut self, now: std::time::Instant) {
        if self.params.source.lock().unwrap().2.is_empty() || self.rate.load(Ordering::Acquire) == 0
        {
            self.ir_observed = None;
            return;
        }
        let selected = self.params.damping.ir().pack();
        if ir_settings_match(*self.params.ir_damping_applied.lock().unwrap(), selected) {
            self.ir_observed = None;
            return;
        }
        // OFF knob edits must not restart the debounce while restoring an undamped IR.
        let selected = if selected[0] == 0.0 {
            [0.0; 8]
        } else {
            selected
        };
        let (observed, changed) = *self.ir_observed.get_or_insert((selected, Some(now)));
        if observed != selected {
            self.ir_observed = Some((selected, Some(now)));
        } else if changed.is_some_and(|at| now.duration_since(at) >= AUTO_REBUILD_DELAY)
            && self.ir_job.is_none()
        {
            // Mark this attempt; do not retry failures every UI frame.
            self.ir_observed = Some((selected, None));
            if let Err(e) = self.apply_ir_damping() {
                self.status = e;
            }
        }
    }
    fn load(&mut self, path: &std::path::Path) -> Result<(), String> {
        self.load_calibrated(path, None)
    }

    fn load_calibrated(
        &mut self,
        path: &std::path::Path,
        reference: Option<(u32, Vec<[f64; 2]>)>,
    ) -> Result<(), String> {
        if self.ir_job.is_some() {
            return Err("Wait for the current IR damping job".into());
        }
        let ir = Impulse::load(path)?;
        let rate = self.rate.load(Ordering::Acquire);
        if rate == 0 {
            return Err("Activate the plugin before loading an IR".into());
        }
        let converted = resample(&ir.frames, ir.sample_rate, rate)?;
        let selected = self.params.damping.ir();
        let processed = damping::render(&converted, rate, selected);
        let mut engine = Convolver::new(&processed, rate)?;
        engine.calibration_gain = if let Some((source_rate, frames)) = &reference {
            autolevel::gain(&resample(frames, *source_rate, rate)?, rate)
        } else {
            autolevel::gain(&converted, rate)
        };
        // Lock ordering agrees with activation. No processing thread takes source.
        let mut source = self.params.source.lock().unwrap();
        let mut stored_reference = self.params.calibration_reference.lock().unwrap();
        let mut pending = self.pending.lock().unwrap();
        if rate != self.rate.load(Ordering::Acquire) {
            return Err("Sample rate changed; please load again".into());
        }
        *source = (
            path.file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned(),
            ir.sample_rate,
            ir.frames,
        );
        *pending = Pending {
            engine: Some(engine),
            ready: true,
        };
        *stored_reference = reference;
        *self.params.ir_damping_applied.lock().unwrap() = Some(selected.pack());
        self.ir_observed = None;
        Ok(())
    }
}
fn configure_editor(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    fonts.font_data.insert(
        "JetBrains Mono Medium".into(),
        Arc::new(egui::FontData::from_static(include_bytes!(
            "../../resources/JetBrainsMono-Medium.ttf"
        ))),
    );
    for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
        fonts
            .families
            .entry(family)
            .or_default()
            .insert(0, "JetBrains Mono Medium".into());
    }
    ctx.set_fonts(fonts);
    for theme in [egui::Theme::Dark, egui::Theme::Light] {
        ctx.style_mut_of(theme, |style| {
            for font in style.text_styles.values_mut() {
                font.size = 14.0;
            }
        });
    }
}

fn tone_row(ui: &mut egui::Ui, p: &damping::DampingParams, ir: bool, setter: &ParamSetter<'_>) {
    let width = ((ui.available_width() - ui.spacing().item_spacing.x * 3.0) / 4.0).min(116.0);
    let bands = if ir {
        [
            Some((&p.ir_low, &p.ir_low_hz)),
            Some((&p.ir_mid, &p.ir_mid_hz)),
            Some((&p.ir_high, &p.ir_high_hz)),
        ]
    } else {
        [
            Some((&p.wet_low, &p.wet_low_hz)),
            Some((&p.wet_mid, &p.wet_mid_hz)),
            None,
        ]
    };
    ui.with_layout(egui::Layout::left_to_right(egui::Align::Min), |ui| {
        for (label, band) in ["Low", "Mid", "High"].into_iter().zip(bands) {
            ui.allocate_ui_with_layout(
                egui::vec2(width, 0.0),
                egui::Layout::top_down(egui::Align::Center),
                |ui| {
                    ui.label(label);
                    ui.add_space(4.0);
                    if let Some((amount, frequency)) = band {
                        dual_knob::dual_knob(ui, amount, frequency, setter);
                        ui.small(format!(
                            "{:.1} {}",
                            amount.value(),
                            if ir { "dB/s" } else { "dB" }
                        ));
                        ui.small(format!("{:.0} Hz", frequency.value()));
                    } else {
                        dual_knob::single_knob_without_value(ui, &p.wet_high_hz, setter);
                        // Single rings are 48 px tall, dual rings 56 px. Keep
                        // both text rows aligned and use the same small font.
                        ui.add_space(8.0);
                        ui.small(p.wet_high_hz.to_string());
                        ui.small("Low-pass");
                    }
                },
            );
        }
        ui.allocate_ui_with_layout(
            egui::vec2(width, 0.0),
            egui::Layout::top_down(egui::Align::Center),
            |ui| {
                ui.label("Mid Q");
                ui.add_space(20.0);
                dual_knob::q_value(ui, if ir { &p.ir_q } else { &p.wet_q }, setter);
            },
        );
    });
}

impl VnEditor {
    fn show(&mut self, ui: &mut egui::Ui) {
        if self.ir_job.as_ref().is_some_and(|job| job.is_finished())
            && let Err(e) = self.finish_ir_damping()
        {
            self.status = format!("IR damping not applied: {e}");
        }
        let (directory, selected_path) = self.params.wav_location.lock().unwrap().clone();
        self.wav_folder.update(&directory, false);
        egui::ScrollArea::vertical().show(ui, |ui| {
            ui.heading("Algo VN");
            ui.small("IR convolution · Zero added latency");
            ui.separator();
            ui.label("Impulse Response");
            ui.horizontal(|ui| {
                // Long filenames must not expand the row or move Refresh offscreen.
                ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate);
                if ui
                    .add_enabled(self.ir_job.is_none(), egui::Button::new("Browse…"))
                    .clicked()
                {
                    let initial =
                        (!directory.as_os_str().is_empty()).then_some(directory.as_path());
                    self.file_browser = Some(file_browser::FileBrowser::new(initial));
                }
                let mut selected = None;
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .add_enabled(
                            !directory.as_os_str().is_empty() && !self.wav_folder.loading(),
                            egui::Button::new("Refresh"),
                        )
                        .on_hover_text("Rescan the remembered WAV folder")
                        .clicked()
                    {
                        self.wav_folder.update(&directory, true);
                    }
                    for (label, forward, hint) in [
                        ("▶", true, "Load next WAV (stop transport first)"),
                        ("◀", false, "Load previous WAV (stop transport first)"),
                    ] {
                        let path = self.wav_folder.adjacent(&selected_path, forward);
                        if ui
                            .add_enabled(
                                self.ir_job.is_none() && path.is_some(),
                                egui::Button::new(label),
                            )
                            .on_hover_text(hint)
                            .clicked()
                        {
                            selected = path.cloned();
                        }
                    }
                    ui.add_enabled_ui(
                        self.ir_job.is_none() && !self.wav_folder.files.is_empty(),
                        |ui| {
                            egui::ComboBox::from_id_salt("wav-folder")
                                .selected_text(
                                    selected_path
                                        .file_name()
                                        .map(|name| name.to_string_lossy().into_owned())
                                        .unwrap_or_else(|| {
                                            if self.wav_folder.loading() {
                                                "Reading folder…".into()
                                            } else {
                                                "Choose WAV".into()
                                            }
                                        }),
                                )
                                .width(ui.available_width())
                                .truncate()
                                .show_ui(ui, |ui| {
                                    ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate);
                                    for path in &self.wav_folder.files {
                                        let name =
                                            path.file_name().unwrap_or_default().to_string_lossy();
                                        if ui
                                            .selectable_label(*path == selected_path, name)
                                            .on_hover_text(path.to_string_lossy())
                                            .clicked()
                                        {
                                            selected = Some(path.clone());
                                        }
                                    }
                                })
                                .response
                                .on_hover_text(directory.to_string_lossy());
                        },
                    );
                });
                if let Some(path) = selected {
                    self.load_selected_wav(&path);
                }
            });
            if let Some(error) = &self.wav_folder.error {
                ui.small(format!("{error} (embedded IR retained)"));
            } else if !directory.as_os_str().is_empty()
                && !self.wav_folder.loading()
                && self.wav_folder.files.is_empty()
            {
                ui.small("No WAV files in this folder. Use Browse… or Refresh.");
            }
            {
                let source = self.params.source.lock().unwrap();
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(if source.2.is_empty() {
                            "No IR: Wet output is silent"
                        } else {
                            &source.0
                        })
                        .small(),
                    )
                    .truncate(),
                )
                .on_hover_text(&source.0);
            }
            if let Some(context) = &self.context {
                let setter = context.param_setter();
                let p = &self.params.damping;
                ui.separator();
                ui.add(ParamButton::for_param(&p.ir_enable, &setter));
                tone_row(ui, p, true, &setter);
                ui.small("IR age decay · 0–120 dB/s · Auto-apply after 300 ms");
                // Keep one line reserved so background preparation never shifts the controls.
                ui.small(if self.ir_job.is_some() {
                    "Preparing IR damping…"
                } else {
                    " "
                });
                ui.separator();
                ui.add(
                    ParamButton::for_param(&p.wet_enable, &setter).with_label("Wet Tone Filter"),
                );
                tone_row(ui, p, false, &setter);
                ui.small("Live Wet-only filtering · 20 ms smoothing");
                ui.small("Dual rings: left = amount · right = Hz · Shift = fine");
                ui.separator();
                ui.with_layout(egui::Layout::left_to_right(egui::Align::Min), |ui| {
                    for (label, param) in [
                        ("Dry / Wet", &self.params.wet),
                        ("Output", &self.params.output),
                    ] {
                        ui.allocate_ui_with_layout(
                            egui::vec2(116.0, 0.0),
                            egui::Layout::top_down(egui::Align::Center),
                            |ui| {
                                ui.label(label);
                                ui.add_space(4.0);
                                dual_knob::single_knob(ui, param, &setter);
                            },
                        );
                    }
                    ui.vertical(|ui| {
                        ui.add(ParamButton::for_param(&self.params.bypass, &setter));
                        ui.add(ParamButton::for_param(&self.params.auto_level, &setter));
                        ui.add(ParamButton::for_param(&self.params.safety_limiter, &setter));
                    });
                });
            }
            let gain = f32::from_bits(self.limiter_meter.load(Ordering::Relaxed));
            let reduction = -20.0 * gain.clamp(1e-12, 1.0).log10();
            ui.small(format!(
                "Safety: -1 dBFS sample peak · reduction {reduction:.1} dB"
            ));
            ui.small("Stop transport before loading / IR damping edits (tail resets).");
            ui.small("Pink: Wet only, ±24 dB · Bypass is unprotected · Max IR 30 s");
            ui.add(
                egui::Label::new(
                    egui::RichText::new(if self.status.is_empty() {
                        " "
                    } else {
                        &self.status
                    })
                    .small(),
                )
                .truncate(),
            )
            .on_hover_text(&self.status);
        });
        let selected = self
            .file_browser
            .as_mut()
            .and_then(|browser| browser.show(ui.ctx()));
        if let Some(path) = selected {
            self.load_selected_wav(&path);
        }
        if !self
            .file_browser
            .as_ref()
            .is_some_and(|browser| browser.open)
        {
            self.poll_ir_damping(std::time::Instant::now());
        }
        if !self.params.source.lock().unwrap().2.is_empty()
            || self.ir_job.is_some()
            || self.wav_folder.loading()
        {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(50));
        }
    }
}

impl NiceEguiApp for VnEditor {
    fn build(
        &mut self,
        ctx: egui::Context,
        context: GuiContext,
        _frame: &mut Frame,
    ) -> Result<(), nice_plug_egui::baseview::HandlerError> {
        configure_editor(&ctx);
        self.context = Some(context);
        Ok(())
    }
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut Frame) {
        self.show(ui);
    }
    fn editor_closed(&mut self) {
        self.context = None;
        self.file_browser = None;
    }
}

nice_export_clap!(VnPlugin);
nice_export_vst3!(VnPlugin);

#[cfg(test)]
mod tests {
    use super::*;
    use nice_plug_core::context::process::SendEventError;

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
    fn wet_tone_high_readouts_align_with_dual_knob_readouts() {
        let editor = ir_editor(48000);
        let context = GuiContext::new(Arc::new(LayoutHost));
        let ctx = egui::Context::default();
        configure_editor(&ctx);
        for frame in 0..3 {
            let mut output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(520.0, 300.0),
                    )),
                    time: Some(frame as f64 / 60.0),
                    ..Default::default()
                },
                |ui| tone_row(ui, &editor.params.damping, false, &context.param_setter()),
            );
            output.textures_delta.clear(); // Headless test: no renderer consumes texture uploads.
            let position = |label: &str| {
                output
                    .shapes
                    .iter()
                    .find_map(|shape| match &shape.shape {
                        egui::Shape::Text(text) if text.galley.job.text == label => {
                            Some(text.pos.y)
                        }
                        _ => None,
                    })
                    .unwrap_or_else(|| panic!("Missing readout: {label}"))
            };
            let p = &editor.params.damping;
            assert_eq!(position(&p.wet_high_hz.to_string()), position("0.0 dB"));
            assert_eq!(
                position("Low-pass"),
                position(&format!("{:.0} Hz", p.wet_low_hz.value()))
            );
            assert_eq!(
                position("Low-pass"),
                position(&format!("{:.0} Hz", p.wet_mid_hz.value()))
            );
        }
    }

    #[test]
    fn compact_editor_shows_all_main_controls_without_scrolling() {
        let mut editor = ir_editor(48000);
        editor.context = Some(GuiContext::new(Arc::new(LayoutHost)));
        // A very long remembered filename must not displace Refresh or arrows.
        editor.params.wav_location.lock().unwrap().1 =
            std::path::PathBuf::from(format!("{}.wav", "long_filename_".repeat(60)));
        let ctx = egui::Context::default();
        configure_editor(&ctx);
        let size = egui::vec2(520.0, 735.0);
        let mut idle_positions = Vec::new();
        let (release, wait) = std::sync::mpsc::channel();
        let mut wait = Some(wait);
        for frame in 0..9 {
            if frame == 3 {
                let wait = wait.take().unwrap();
                editor.ir_job = Some(std::thread::spawn(move || {
                    wait.recv().unwrap();
                    Err("layout test worker".into())
                }));
                editor.status = "Preparing IR damping in background…".into();
            } else if frame == 6 {
                release.send(()).unwrap();
                assert!(editor.ir_job.take().unwrap().join().unwrap().is_err());
                editor.status =
                    "IR damping applied automatically; convolution history cleared.".into();
            }
            let mut output = ctx.run_ui(
                egui::RawInput {
                    time: Some(frame as f64 / 60.0),
                    screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
                    ..Default::default()
                },
                |ui| editor.show(ui),
            );
            output.textures_delta.clear(); // Headless test: no renderer consumes texture uploads.
            if frame % 3 == 2 {
                let labels: Vec<_> = output
                    .shapes
                    .iter()
                    .filter_map(|shape| {
                        if let egui::Shape::Text(text) = &shape.shape {
                            Some(text)
                        } else {
                            None
                        }
                    })
                    .collect();
                for expected in [
                    "Impulse Response",
                    "Browse…",
                    "Refresh",
                    "◀",
                    "▶",
                    "IR Time Damping",
                    "Wet Tone Filter",
                    "Dry / Wet",
                    "Output",
                    "Pink Auto Level",
                    "Safety Limiter",
                ] {
                    let label = labels
                        .iter()
                        .find(|text| text.galley.job.text == expected)
                        .unwrap_or_else(|| panic!("Missing visible control: {expected}"));
                    let rect = egui::Rect::from_min_size(label.pos, label.galley.size());
                    assert!(
                        rect.right() <= size.x && rect.bottom() <= size.y,
                        "Control outside window: {expected} {rect:?}"
                    );
                }
                assert!(!labels.iter().any(|text| {
                    let label = &text.galley.job.text;
                    label.contains("Comparison")
                        || label.contains("Rendered FDN")
                        || label == "Load WAV"
                        || label == "WAV file path"
                }));
                let footer = labels
                    .iter()
                    .find(|text| text.galley.job.text.starts_with("Pink: Wet only"))
                    .expect("Footer must remain visible without scrolling");
                assert!(
                    footer.pos.y + footer.galley.size().y < size.y,
                    "Footer bottom: {}",
                    footer.pos.y + footer.galley.size().y
                );
                let status = labels.iter().find(|text| {
                    text.galley.job.text == editor.status && !editor.status.is_empty()
                });
                if let Some(status) = status {
                    assert!(status.pos.y + status.galley.size().y < size.y);
                }
                let positions: Vec<_> = [
                    "Browse…",
                    "Refresh",
                    "Wet Tone Filter",
                    "Dry / Wet",
                    "Output",
                ]
                .iter()
                .map(|expected| {
                    labels
                        .iter()
                        .find(|text| text.galley.job.text == *expected)
                        .unwrap()
                        .pos
                })
                .collect();
                assert_eq!(
                    positions[3].y, positions[4].y,
                    "Dry / Wet and Output labels must share the same top edge"
                );
                let value_positions: Vec<_> = [&editor.params.wet, &editor.params.output]
                    .iter()
                    .map(|param| {
                        labels
                            .iter()
                            .find(|text| text.galley.job.text == param.to_string())
                            .expect("Knob value must be visible")
                            .pos
                            .y
                    })
                    .collect();
                assert_eq!(
                    value_positions[0], value_positions[1],
                    "Dry / Wet and Output values must be aligned"
                );
                if frame == 2 {
                    idle_positions = positions;
                } else {
                    assert_eq!(
                        positions, idle_positions,
                        "Preparing/idle must not shift controls"
                    );
                }
            }
        }
    }

    struct TestProcessContext;
    impl ProcessContext<VnPlugin> for TestProcessContext {
        fn plugin_api(&self) -> PluginApi {
            PluginApi::Clap
        }
        fn execute_background(&self, _: ()) {
            panic!("unexpected task");
        }
        fn execute_gui(&self, _: ()) {
            panic!("unexpected task");
        }
        fn transport(&self) -> &Transport {
            panic!("unexpected transport query");
        }
        fn next_event(&mut self) -> Option<PluginNoteEvent<VnPlugin>> {
            None
        }
        fn try_send_event(
            &mut self,
            _: PluginNoteEvent<VnPlugin>,
        ) -> Result<(), (PluginNoteEvent<VnPlugin>, SendEventError)> {
            panic!("unexpected event");
        }
        fn set_latency_samples(&self, _: u32) {}
        fn request_restart(&self) {
            panic!("unexpected restart");
        }
        fn set_current_voice_capacity(&self, _: u32) {}
    }

    #[test]
    #[ignore = "manual check against locally available IMreverbs WAVs"]
    fn imreverbs_damping_worker_changes_actual_convolution_output() {
        use nice_plug::params::InternalParamMut;
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../resources/IMreverbs");
        let mut paths: Vec<_> = std::fs::read_dir(root)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .filter(|path| path.extension().is_some_and(|ext| ext == "wav"))
            .collect();
        paths.sort();
        assert!(!paths.is_empty(), "No local WAV fixtures found");
        for path in paths {
            let mut editor = ir_editor(48000);
            editor.load(&path).unwrap();
            let mut original = editor.pending.lock().unwrap().engine.take().unwrap();
            let frames = original.frames;
            unsafe {
                editor
                    .params
                    .damping
                    .ir_enable
                    ._internal_set_plain_value(true);
                editor
                    .params
                    .damping
                    .ir_low
                    ._internal_set_plain_value(120.0);
                editor
                    .params
                    .damping
                    .ir_mid
                    ._internal_set_plain_value(120.0);
                editor
                    .params
                    .damping
                    .ir_high
                    ._internal_set_plain_value(120.0);
            }
            let now = std::time::Instant::now();
            editor.poll_ir_damping(now);
            editor.poll_ir_damping(now + AUTO_REBUILD_DELAY);
            editor.finish_ir_damping().unwrap();
            let mut processed = editor.pending.lock().unwrap().engine.take().unwrap();
            assert_eq!(original.calibration_gain, processed.calibration_gain);
            let mut before = 0.0_f64;
            let mut after = 0.0_f64;
            let mut difference = 0.0_f64;
            for i in 0..frames {
                let input = if i == 0 { [1.0; 2] } else { [0.0; 2] };
                let a = original.process(input).1;
                let b = processed.process(input).1;
                for ch in 0..2 {
                    before += f64::from(a[ch]).powi(2);
                    after += f64::from(b[ch]).powi(2);
                    difference += f64::from(a[ch] - b[ch]).powi(2);
                }
            }
            assert!(before > 0.0 && difference > 0.0, "{}", path.display());
            if path.file_name().unwrap() == "Masonic Lodge.wav" {
                assert!(
                    10.0 * (after / before).log10() < -2.5,
                    "Maximum damping must make a substantial change on Masonic Lodge"
                );
            }
            eprintln!(
                "{}: duration={:.3}s energy change={:.2}dB difference/source={:.2}dB",
                path.file_name().unwrap().to_string_lossy(),
                frames as f64 / 48000.0,
                10.0 * (after / before).log10(),
                10.0 * (difference / before).log10()
            );
        }
    }

    #[test]
    fn automatic_ir_damping_reaches_plugin_audio_output_in_every_band() {
        use nice_plug::params::InternalParamMut;
        // Exercise the real worker, Pending swap and Plugin::process, not just render().
        for rate in [8000, 48000] {
            for (band, hz) in [(0, 40.0), (1, 1000.0), (2, 3000.0)] {
                for auto_level in [false, true] {
                    let n = rate as usize * 2;
                    let ir: Vec<_> = (0..n)
                        .map(|i| {
                            let x =
                                0.01 * (std::f64::consts::TAU * hz * i as f64 / rate as f64).sin();
                            [x, -x * 0.7]
                        })
                        .collect();
                    let mut editor = ir_editor(rate);
                    *editor.params.source.lock().unwrap() = ("probe".into(), rate, ir.clone());
                    let mut plugin = VnPlugin {
                        params: editor.params.clone(),
                        pending: editor.pending.clone(),
                        rate: editor.rate.clone(),
                        engine: Some(Convolver::new(&ir, rate).unwrap()),
                        smoothing: 1.0 - (-1.0 / (0.02 * rate as f32)).exp(),
                        ..VnPlugin::default()
                    };
                    plugin.engine.as_mut().unwrap().calibration_gain = autolevel::gain(&ir, rate);
                    unsafe {
                        plugin.params.wet._internal_set_plain_value(1.0);
                        plugin.params.output._internal_set_plain_value(0.0);
                        plugin
                            .params
                            .safety_limiter
                            ._internal_set_plain_value(false);
                        plugin
                            .params
                            .auto_level
                            ._internal_set_plain_value(auto_level);
                        plugin
                            .params
                            .damping
                            .ir_high_hz
                            ._internal_set_plain_value(500.0);
                    }
                    let capture = |plugin: &mut VnPlugin| {
                        plugin.reset();
                        let mut left = vec![0.0; n];
                        let mut right = vec![0.0; n];
                        left[0] = 1.0;
                        right[0] = 1.0;
                        let mut buffer = Buffer::default();
                        // Both equally sized channel buffers remain alive through process().
                        unsafe {
                            buffer.set_slices(n, |v| *v = vec![&mut left, &mut right]);
                        }
                        let status = plugin.process(
                            &mut buffer,
                            &mut AuxiliaryBuffers {
                                inputs: &mut [],
                                outputs: &mut [],
                            },
                            &mut TestProcessContext,
                        );
                        assert!(matches!(status, ProcessStatus::Tail(_)));
                        drop(buffer);
                        left.iter()
                            .zip(right)
                            .map(|(l, r)| [*l, r])
                            .collect::<Vec<_>>()
                    };
                    let dry_ir = capture(&mut plugin);
                    unsafe {
                        editor
                            .params
                            .damping
                            .ir_enable
                            ._internal_set_plain_value(true);
                        let amount = match band {
                            0 => &editor.params.damping.ir_low,
                            1 => &editor.params.damping.ir_mid,
                            _ => &editor.params.damping.ir_high,
                        };
                        amount._internal_set_plain_value(24.0);
                    }
                    let now = std::time::Instant::now();
                    editor.poll_ir_damping(now);
                    editor.poll_ir_damping(now + AUTO_REBUILD_DELAY);
                    editor.finish_ir_damping().unwrap();
                    let damped = capture(&mut plugin);
                    assert!(!plugin.pending.lock().unwrap().ready);
                    let energy = |frames: &[[f32; 2]]| {
                        frames[n * 3 / 4..]
                            .iter()
                            .flatten()
                            .map(|v| f64::from(*v).powi(2))
                            .sum::<f64>()
                    };
                    let db = 10.0 * (energy(&damped) / energy(&dry_ir)).log10();
                    eprintln!(
                        "rate={rate} band={band} auto_level={auto_level}: late output {db:.2} dB"
                    );
                    assert!(db < -10.0, "rate={rate} band={band}: {db} dB");
                    unsafe {
                        editor
                            .params
                            .damping
                            .ir_enable
                            ._internal_set_plain_value(false);
                    }
                    editor.poll_ir_damping(now + AUTO_REBUILD_DELAY * 2);
                    editor.poll_ir_damping(now + AUTO_REBUILD_DELAY * 3);
                    editor.finish_ir_damping().unwrap();
                    assert_eq!(capture(&mut plugin), dry_ir);
                }
            }
        }
    }

    #[test]
    fn disabled_ir_knob_edits_do_not_rebuild_or_invalidate_restore() {
        use nice_plug::params::InternalParamMut;
        let mut editor = ir_editor(8000);
        *editor.params.source.lock().unwrap() = ("probe".into(), 8000, vec![[0.1; 2]; 80]);
        let now = std::time::Instant::now();
        // The default/legacy None state already represents an undamped IR.
        unsafe {
            let value = 12.0;
            let p = &editor.params.damping;
            p.ir_high._internal_set_plain_value(value);
            p.ir_mid._internal_set_plain_value(value);
            p.ir_low._internal_set_plain_value(value);
            p.ir_high_hz._internal_set_plain_value(4000.0 + value);
            p.ir_mid_hz._internal_set_plain_value(1000.0 + value);
            p.ir_low_hz._internal_set_plain_value(200.0 + value);
            p.ir_q._internal_set_plain_value(1.0 + value / 100.0);
        }
        editor.poll_ir_damping(now);
        editor.poll_ir_damping(now + AUTO_REBUILD_DELAY);
        assert!(editor.ir_job.is_none());
        assert!(editor.ir_observed.is_none());

        unsafe {
            editor
                .params
                .damping
                .ir_enable
                ._internal_set_plain_value(true);
        }
        editor.poll_ir_damping(now + AUTO_REBUILD_DELAY * 2);
        editor.poll_ir_damping(now + AUTO_REBUILD_DELAY * 3);
        editor.finish_ir_damping().unwrap();
        unsafe {
            editor
                .params
                .damping
                .ir_enable
                ._internal_set_plain_value(false);
        }
        editor.poll_ir_damping(now + AUTO_REBUILD_DELAY * 4);
        // Knob edits while OFF must not postpone the one necessary restore.
        unsafe {
            editor
                .params
                .damping
                .ir_high
                ._internal_set_plain_value(24.0);
        }
        editor.poll_ir_damping(now + AUTO_REBUILD_DELAY * 5);
        assert!(editor.ir_job.is_some());
        unsafe {
            editor.params.damping.ir_mid._internal_set_plain_value(36.0);
        }
        editor.finish_ir_damping().unwrap();
        let mut pending = editor.pending.lock().unwrap();
        assert_eq!(
            pending.engine.as_mut().unwrap().process([1.0; 2]).1,
            [0.1; 2]
        );
        pending.ready = false;
        drop(pending);
        editor.poll_ir_damping(now + AUTO_REBUILD_DELAY * 6);
        editor.poll_ir_damping(now + AUTO_REBUILD_DELAY * 7);
        assert!(editor.ir_job.is_none());
        assert!(!editor.pending.lock().unwrap().ready);
        // Re-enabling uses the latest knob positions, not the previously applied ones.
        unsafe {
            editor
                .params
                .damping
                .ir_enable
                ._internal_set_plain_value(true);
        }
        editor.poll_ir_damping(now + AUTO_REBUILD_DELAY * 8);
        editor.poll_ir_damping(now + AUTO_REBUILD_DELAY * 9);
        editor.finish_ir_damping().unwrap();
        assert_eq!(
            *editor.params.ir_damping_applied.lock().unwrap(),
            Some(editor.params.damping.ir().pack())
        );
    }

    #[test]
    fn ir_damping_auto_applies_on_off_and_rejects_stale_settings_source_and_rate() {
        use nice_plug::params::InternalParamMut;
        let mut editor = ir_editor(8000);
        *editor.params.source.lock().unwrap() = ("test".into(), 8000, vec![[0.1; 2]; 800]);
        let original = editor.params.source.lock().unwrap().clone();
        let now = std::time::Instant::now();
        unsafe {
            editor
                .params
                .damping
                .ir_enable
                ._internal_set_plain_value(true);
            editor
                .params
                .damping
                .ir_high
                ._internal_set_plain_value(12.0);
        }
        editor.poll_ir_damping(now);
        assert!(editor.ir_job.is_none());
        editor.poll_ir_damping(now + AUTO_REBUILD_DELAY);
        assert!(editor.ir_job.is_some());
        assert!(editor.apply_ir_damping().is_err());
        // Coalesce another edit while preparing; never publish the old result.
        unsafe {
            editor.params.damping.ir_high._internal_set_plain_value(6.0);
        }
        assert!(editor.finish_ir_damping().is_err());
        assert!(editor.params.ir_damping_applied.lock().unwrap().is_none());
        editor.poll_ir_damping(now + AUTO_REBUILD_DELAY * 2);
        editor.poll_ir_damping(now + AUTO_REBUILD_DELAY * 3);
        editor.finish_ir_damping().unwrap();
        assert_eq!(
            *editor.params.ir_damping_applied.lock().unwrap(),
            Some(editor.params.damping.ir().pack())
        );
        assert_eq!(*editor.params.source.lock().unwrap(), original);
        unsafe {
            editor
                .params
                .damping
                .ir_enable
                ._internal_set_plain_value(false);
        }
        editor.poll_ir_damping(now + AUTO_REBUILD_DELAY * 4);
        editor.poll_ir_damping(now + AUTO_REBUILD_DELAY * 5);
        editor.finish_ir_damping().unwrap();
        assert_eq!(
            editor
                .pending
                .lock()
                .unwrap()
                .engine
                .as_mut()
                .unwrap()
                .process([1.0; 2]),
            ([1.0; 2], [0.1; 2])
        );
        for change in 0..2 {
            editor.apply_ir_damping().unwrap();
            match change {
                0 => editor.params.source.lock().unwrap().2[0][0] = 0.2,
                _ => editor.rate.store(16000, Ordering::Release),
            }
            assert!(editor.finish_ir_damping().is_err());
        }
        *editor.params.source.lock().unwrap() = (String::new(), 16000, Vec::new());
        editor.poll_ir_damping(now + AUTO_REBUILD_DELAY * 6);
        editor.poll_ir_damping(now + AUTO_REBUILD_DELAY * 7);
        assert!(editor.ir_job.is_none());
    }

    fn ir_editor(rate: u32) -> VnEditor {
        VnEditor {
            params: Arc::new(VnParams::default()),
            rate: Arc::new(AtomicU32::new(rate)),
            pending: Arc::default(),
            context: None,
            status: String::new(),
            limiter_meter: Arc::new(AtomicU32::new(1.0_f32.to_bits())),
            ir_job: None,
            ir_observed: None,
            file_browser: None,
            wav_folder: file_browser::WavFolder::default(),
        }
    }

    #[test]
    fn host_parameters_keep_ir_and_output_ids_without_fdn() {
        let params = VnParams::default();
        let map = params.param_map();
        let ids: std::collections::HashSet<_> = map.iter().map(|(id, _, _)| id.as_str()).collect();
        assert_eq!(ids.len(), map.len());
        assert!(!ids.iter().any(|id| id.starts_with("fdn_")));
        for id in [
            "wet",
            "output_db",
            "bypass",
            "safety_limiter",
            "auto_level",
            "ir_damp_enable",
            "ir_damp_high",
            "wet_damp_enable",
            "wet_damp_high_hz",
        ] {
            assert!(ids.contains(id), "Missing parameter: {id}");
        }
    }
    #[test]
    fn loading_valid_wav_then_invalid_wav_retains_embedded_ir_and_pending_engine() {
        let mut editor = ir_editor(48000);
        let path = std::env::temp_dir().join(format!("algo-vn-load-{}.wav", std::process::id()));
        // Two stereo PCM16 frames at 48 kHz.
        let mut bytes = Vec::from(&b"RIFF"[..]);
        bytes.extend(44_u32.to_le_bytes());
        bytes.extend(b"WAVEfmt ");
        bytes.extend(16_u32.to_le_bytes());
        bytes.extend(1_u16.to_le_bytes());
        bytes.extend(2_u16.to_le_bytes());
        bytes.extend(48000_u32.to_le_bytes());
        bytes.extend(192000_u32.to_le_bytes());
        bytes.extend(4_u16.to_le_bytes());
        bytes.extend(16_u16.to_le_bytes());
        bytes.extend(b"data");
        bytes.extend(8_u32.to_le_bytes());
        for sample in [8192_i16, -16384, 4096, -8192] {
            bytes.extend(sample.to_le_bytes());
        }
        std::fs::write(&path, &bytes).unwrap();
        editor.load_selected_wav(&path);
        assert_eq!(editor.status, "IR loaded. Previous tail cleared.");
        let location = editor.params.wav_location.lock().unwrap().clone();
        assert_eq!(
            location,
            (path.parent().unwrap().to_path_buf(), path.clone())
        );
        let stored = editor.params.source.lock().unwrap().clone();
        assert_eq!(stored.2, vec![[0.25, -0.5], [0.125, -0.25]]);
        assert!(editor.pending.lock().unwrap().ready);
        std::fs::write(&path, b"invalid wav").unwrap();
        editor.load_selected_wav(&path);
        assert!(
            editor
                .status
                .starts_with("Load failed (previous IR retained):")
        );
        assert_eq!(*editor.params.wav_location.lock().unwrap(), location);
        assert_eq!(*editor.params.source.lock().unwrap(), stored);
        let pending = editor.pending.lock().unwrap();
        assert!(pending.ready);
        assert_eq!(pending.engine.as_ref().unwrap().frames, 2);
        drop(pending);
        use nice_plug::params::InternalParamMut;
        unsafe {
            editor
                .params
                .damping
                .ir_enable
                ._internal_set_plain_value(true);
            editor
                .params
                .damping
                .ir_high
                ._internal_set_plain_value(12.0);
        }
        editor.apply_ir_damping().unwrap();
        editor.finish_ir_damping().unwrap();
        assert_eq!(*editor.params.source.lock().unwrap(), stored);
        let restored = VnParams::default();
        restored.deserialize_fields(&editor.params.serialize_fields());
        assert_eq!(
            *restored.ir_damping_applied.lock().unwrap(),
            Some(editor.params.damping.ir().pack())
        );
        // Applying repeatedly always starts from the original, not the edited IR.
        let first = editor
            .pending
            .lock()
            .unwrap()
            .engine
            .as_mut()
            .unwrap()
            .process([1.0; 2]);
        editor.apply_ir_damping().unwrap();
        editor.finish_ir_damping().unwrap();
        assert_eq!(
            editor
                .pending
                .lock()
                .unwrap()
                .engine
                .as_mut()
                .unwrap()
                .process([1.0; 2]),
            first
        );
        unsafe {
            editor
                .params
                .damping
                .ir_enable
                ._internal_set_plain_value(false);
        }
        editor.apply_ir_damping().unwrap();
        editor.finish_ir_damping().unwrap();
        assert_eq!(
            editor
                .pending
                .lock()
                .unwrap()
                .engine
                .as_mut()
                .unwrap()
                .process([1.0; 2])
                .1,
            [0.25, -0.5]
        );

        // Ordinary WAV loads clear a legacy comparison calibration reference.
        *editor.params.calibration_reference.lock().unwrap() = Some((48000, vec![[2.0, -2.0]]));
        std::fs::write(&path, &bytes).unwrap();
        editor.load(&path).unwrap();
        assert!(
            editor
                .params
                .calibration_reference
                .lock()
                .unwrap()
                .is_none()
        );
        std::fs::remove_file(&path).unwrap();
        assert!(editor.load(&path).is_err());
        assert_eq!(*editor.params.source.lock().unwrap(), stored);
        let restored = VnParams::default();
        restored.deserialize_fields(&editor.params.serialize_fields());
        assert_eq!(*restored.source.lock().unwrap(), stored);
        assert_eq!(*restored.wav_location.lock().unwrap(), location);
    }
    #[test]
    fn embedded_ir_round_trips_without_external_file() {
        let params = VnParams::default();
        assert!(!params.auto_level.value());
        *params.source.lock().unwrap() = ("test.wav".into(), 48000, vec![[0.2, -0.4], [0.1, 0.3]]);
        *params.calibration_reference.lock().unwrap() = Some((44100, vec![[2.0, -2.0]]));
        let state = params.serialize_fields();
        let restored = VnParams::default();
        restored.deserialize_fields(&state);
        assert_eq!(
            *params.source.lock().unwrap(),
            *restored.source.lock().unwrap()
        );
        let reference = restored.calibration_reference.lock().unwrap();
        let (rate, frames) = reference.as_ref().unwrap();
        assert_eq!(*rate, 44100);
        assert!((autolevel::gain(frames, *rate) - 0.5).abs() < 1e-6);
    }
}
