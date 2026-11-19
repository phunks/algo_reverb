//! Background import; the callback only try-locks and swaps prepared networks.
use super::peak_space::Space;
use super::*;
use crate::difffdn::seed::{Seed, analyze};
use crate::ir_analysis::Impulse;
use std::sync::atomic::AtomicBool;

#[derive(Default)]
struct Handoff {
    next: Option<Space>,
    retired: Option<Space>,
}

#[derive(Default)]
pub(super) struct Shared {
    pub rate: AtomicU32,
    busy: AtomicBool,
    generation: AtomicU32,
    pending: Mutex<Handoff>,
    status: Mutex<String>,
}

impl Shared {
    pub fn clear_pending(&self) {
        let mut pending = self.pending.lock().unwrap();
        self.generation.fetch_add(1, Ordering::AcqRel);
        *pending = Handoff::default();
    }
}

pub(super) struct Playback {
    network: Option<Space>,
    position: u32,
    duration: u32,
    replacing: bool,
    selected: bool,
}

impl Playback {
    pub fn restore(rate: u32, params: &ReverbParams) -> Self {
        let state = params.seed_state.lock().unwrap();
        let network = Seed::decode(&state.1)
            .and_then(|s| {
                let mut space = Space::new([s.build(rate)?, s.build(rate)?]);
                space.update(params);
                space.reset();
                Ok(space)
            })
            .ok();
        Self {
            network,
            position: 0,
            duration: (rate as f64 * 0.15).round() as u32,
            replacing: false,
            selected: false,
        }
    }
    pub fn reset(&mut self) {
        if let Some(f) = &mut self.network {
            f.reset();
        }
        self.position = 0;
        self.replacing = false;
    }
    pub fn receive(&mut self, shared: &Shared, rate: u32) {
        let Ok(mut pending) = shared.pending.try_lock() else {
            return;
        };
        if pending.next.is_none() {
            return;
        }
        self.replacing = true;
        if self.position == 0 && pending.retired.is_none() {
            if pending
                .next
                .as_ref()
                .is_some_and(|f| f.sample_rate() == rate)
            {
                std::mem::swap(&mut self.network, &mut pending.next);
                pending.retired = pending.next.take();
            } else {
                pending.retired = pending.next.take();
            }
            self.replacing = false;
        }
    }
    pub fn advance(&mut self, enabled: bool) -> f32 {
        let target = enabled && !self.replacing && self.network.is_some();
        self.selected = false;
        if target && self.position < self.duration {
            self.position += 1;
            self.selected = self.position == self.duration;
        } else if !target && self.position > 0 {
            self.position -= 1;
            if self.position == 0
                && let Some(f) = &mut self.network
            {
                f.reset();
            }
        }
        self.position as f32 / self.duration as f32
    }
    pub fn just_selected(&self) -> bool {
        self.selected
    }
    pub fn update(&mut self, params: &ReverbParams) {
        if let Some(f) = &mut self.network {
            f.update(params);
            f.set_damping(params.extra_damping.value() as f64);
        }
    }
    pub fn process(&mut self, input: f32) -> [f32; 2] {
        if self.position == 0 {
            return [0.; 2];
        }
        self.network.as_mut().map_or([0.; 2], |f| {
            f.process_stereo(input as f64).map(|x| x as f32)
        })
    }
}

fn start_import(path: std::path::PathBuf, params: &Arc<ReverbParams>, shared: &Arc<Shared>) {
    if shared.rate.load(Ordering::Acquire) == 0 || shared.busy.swap(true, Ordering::AcqRel) {
        return;
    }
    let shared = shared.clone();
    let params = params.clone();
    let generation = shared.generation.load(Ordering::Acquire);
    *shared.status.lock().unwrap() = "Analyzing WAV…".into();
    let worker = shared.clone();
    let launched = std::thread::Builder::new().name("reverb-seed-import".into()).spawn(move || {
        let shared = worker;
        drop(shared.pending.lock().unwrap().retired.take());
        let result = (|| -> Result<String, String> {
            let started = std::time::Instant::now();
            let ir = Impulse::load(&path)?;
            let report = analyze(&ir)?;
            let rate = shared.rate.load(Ordering::Acquire);
            let mut network = Space::new([report.seed.build(rate)?, report.seed.build(rate)?]);
            network.update(&params);
            network.reset();
            let name = path.file_name().unwrap_or_default().to_string_lossy().into_owned();
            let mut pending = shared.pending.lock().unwrap();
            if rate != shared.rate.load(Ordering::Acquire)
                || generation != shared.generation.load(Ordering::Acquire) {
                return Err("Plugin reactivated; import again".into());
            }
            drop(pending.retired.take());
            *params.seed_state.lock().unwrap() = (name.clone(), report.seed.encode());
            *params.seed_wav_location.lock().unwrap() = (
                path.parent().unwrap_or_else(|| std::path::Path::new("")).to_path_buf(), path.clone());
            pending.next = Some(network);
            Ok(format!("{name}: {:.2}s total ({:.2}s analysis), {}/7 decay bands, corr {:.2}. Enable Use IR seed.",
                started.elapsed().as_secs_f64(), report.seconds, report.reliable_bands, report.target_correlation))
        })();
        *shared.status.lock().unwrap() = result.unwrap_or_else(|e| format!("Import failed (previous seed retained): {e}"));
        shared.busy.store(false, Ordering::Release);
    });
    if let Err(error) = launched {
        *shared.status.lock().unwrap() = format!("Cannot start import: {error}");
        shared.busy.store(false, Ordering::Release);
    }
}

#[derive(Default)]
pub(super) struct Browser {
    picker: Option<file_browser::FileBrowser>,
    folder: file_browser::WavFolder,
}

impl Browser {
    pub(super) fn draw(
        &mut self,
        ui: &mut egui::Ui,
        params: &Arc<ReverbParams>,
        shared: &Arc<Shared>,
        setter: &ParamSetter<'_>,
    ) {
        // Reclaim callback-retired storage on the UI thread, even after a fast
        // sequence of imports.
        let retired = shared.pending.lock().unwrap().retired.take();
        drop(retired);
        let (directory, selected_path) = params.seed_wav_location.lock().unwrap().clone();
        self.folder.update(&directory, false);
        let ready =
            shared.rate.load(Ordering::Acquire) != 0 && !shared.busy.load(Ordering::Acquire);
        let mut selected = None;
        ui.label("Impulse Response");
        ui.horizontal(|ui| {
            ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate);
            if ui
                .add_enabled(ready, egui::Button::new("Browse…"))
                .clicked()
            {
                self.picker = Some(file_browser::FileBrowser::new(
                    (!directory.as_os_str().is_empty()).then_some(directory.as_path()),
                ));
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .add_enabled(
                        !directory.as_os_str().is_empty() && !self.folder.loading(),
                        egui::Button::new("Refresh"),
                    )
                    .on_hover_text("Rescan the remembered WAV folder")
                    .clicked()
                {
                    self.folder.update(&directory, true);
                }
                for (label, forward, hint) in [
                    ("▶", true, "Import next WAV"),
                    ("◀", false, "Import previous WAV"),
                ] {
                    let path = self.folder.adjacent(&selected_path, forward);
                    if ui
                        .add_enabled(ready && path.is_some(), egui::Button::new(label))
                        .on_hover_text(hint)
                        .clicked()
                    {
                        selected = path.cloned();
                    }
                }
                ui.add_enabled_ui(ready && !self.folder.files.is_empty(), |ui| {
                    egui::ComboBox::from_id_salt("seed-wav-folder")
                        .selected_text(
                            selected_path
                                .file_name()
                                .map(|n| n.to_string_lossy().into_owned())
                                .unwrap_or_else(|| {
                                    if self.folder.loading() {
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
                            for path in &self.folder.files {
                                if ui
                                    .selectable_label(
                                        *path == selected_path,
                                        path.file_name().unwrap_or_default().to_string_lossy(),
                                    )
                                    .on_hover_text(path.to_string_lossy())
                                    .clicked()
                                {
                                    selected = Some(path.clone());
                                }
                            }
                        })
                        .response
                        .on_hover_text(directory.to_string_lossy());
                });
            });
        });
        if let Some(path) = self
            .picker
            .as_mut()
            .and_then(|picker| picker.show(ui.ctx()))
        {
            selected = Some(path);
        }
        if let Some(path) = selected {
            start_import(path, params, shared);
        }
        ui.add(ParamButton::for_param(&params.seed_enabled, setter));
        if let Some(error) = &self.folder.error {
            ui.small(error);
        }
        if let Ok(state) = params.seed_state.try_lock()
            && !state.0.is_empty()
        {
            ui.small(format!("Seed: {}", state.0));
        }
        if let Ok(status) = shared.status.try_lock()
            && !status.is_empty()
        {
            ui.add(egui::Label::new(egui::RichText::new(status.as_str()).small()).truncate())
                .on_hover_text(status.as_str());
        }
        ui.small(
            "Rust feature fit • synthetic stereo tail • no IR playback / exact early reflections",
        );
        if shared.busy.load(Ordering::Acquire) || self.folder.loading() {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(100));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nice_plug::params::InternalParamMut;

    fn state() -> String {
        let mut text = String::from("rust-stereo-seed-v2");
        for x in [1.; 7]
            .into_iter()
            .chain([0.; 7])
            .chain([0.35; 8])
            .chain([0.35, -0.35, 0.35, -0.35, 0.35, -0.35, 0.35, -0.35])
        {
            text.push_str(&format!(" {x}"));
        }
        text.push_str(" .017 .021 .025 .029 .033 .037 .041 .047 .002 .65");
        text
    }

    #[test]
    fn peak_space_seed_restore_retrigger_and_import_are_deterministic() {
        let params = ReverbParams::default();
        *params.seed_state.lock().unwrap() = ("test.wav".into(), state());
        unsafe {
            params.space_enabled._internal_set_plain_value(true);
        }
        let mut a = Playback::restore(8000, &params);
        let mut b = Playback::restore(8000, &params);
        a.update(&params);
        b.update(&params);
        let mut energy = 0.;
        for i in 0..16000 {
            assert_eq!(a.advance(true), b.advance(true));
            let x = if i % 800 == 0 { 0.5 } else { 0. };
            let output = a.process(x);
            assert_eq!(output, b.process(x));
            assert!(output.iter().all(|v| v.is_finite()));
            energy += output[0] * output[0];
        }
        assert!(energy > 0.);
        let shared = Shared::default();
        shared.pending.lock().unwrap().next = Some(Space::new(std::array::from_fn(|_| {
            Seed::decode(&state()).unwrap().build(8000).unwrap()
        })));
        a.receive(&shared, 8000);
        for _ in 0..a.duration {
            a.advance(true);
            a.process(0.);
        }
        a.receive(&shared, 8000);
        a.update(&params);
        a.reset();
        for _ in 0..2000 {
            a.advance(true);
            assert_eq!(a.process(0.), [0.; 2]);
        }
    }

    #[test]
    fn persisted_seed_restores_and_replacement_waits_for_fade() {
        let params = ReverbParams::default();
        *params.seed_state.lock().unwrap() = ("test.wav".into(), state());
        *params.seed_wav_location.lock().unwrap() =
            ("/missing/irs".into(), "/missing/irs/test.wav".into());
        let restored = ReverbParams::default();
        restored.deserialize_fields(&params.serialize_fields());
        assert_eq!(
            *params.seed_wav_location.lock().unwrap(),
            *restored.seed_wav_location.lock().unwrap()
        );
        assert_eq!(
            *params.seed_state.lock().unwrap(),
            *restored.seed_state.lock().unwrap()
        );
        let mut a = Playback::restore(8000, &params);
        let mut b = Playback::restore(8000, &restored);
        for i in 0..a.duration * 2 {
            assert_eq!(a.advance(true), b.advance(true));
            assert_eq!(
                a.process(if i == 0 { 1. } else { 0. }),
                b.process(if i == 0 { 1. } else { 0. })
            );
        }
        let shared = Shared::default();
        shared.pending.lock().unwrap().next = Some(Space::new(std::array::from_fn(|_| {
            Seed::decode(&state()).unwrap().build(8000).unwrap()
        })));
        a.receive(&shared, 8000);
        assert!(a.replacing);
        assert!(shared.pending.lock().unwrap().next.is_some());
        for _ in 0..a.duration {
            a.advance(true);
            a.process(0.);
        }
        assert_eq!(a.position, 0);
        a.receive(&shared, 8000);
        assert!(!a.replacing);
        assert!(shared.pending.lock().unwrap().retired.is_some());
        assert!(shared.pending.lock().unwrap().next.is_none());
        a.advance(true);
        assert_eq!(a.process(0.), [0.; 2]);
        a.reset();
        assert_eq!(a.process(0.), [0.; 2]);
    }

    #[test]
    fn custom_seed_modulation_changes_audio_only_in_random_modes() {
        let render = |mode, depth, rate| {
            let params = ReverbParams::default();
            *params.seed_state.lock().unwrap() = ("test.wav".into(), state());
            // Test-only host emulation, with valid values and no concurrent access.
            unsafe {
                params.delay_mode._internal_set_plain_value(mode);
                params.mod_depth._internal_set_plain_value(depth);
                params.mod_rate._internal_set_plain_value(rate);
            }
            let mut playback = Playback::restore(8000, &params);
            playback.update(&params);
            for _ in 0..8000 {
                playback.advance(true);
                playback.process(0.0);
            }
            (0..16000)
                .map(|i| playback.process(if i == 0 { 1.0 } else { 0.0 }))
                .collect::<Vec<_>>()
        };
        assert_eq!(
            render(DelayReadMode::Static, 0.2, 0.3),
            render(DelayReadMode::Static, 1.0, 2.0)
        );
        for mode in [DelayReadMode::RandomLinear, DelayReadMode::RandomAllpass] {
            let reference = render(mode, 0.2, 0.3);
            for changed in [render(mode, 1.0, 0.3), render(mode, 0.2, 2.0)] {
                let difference: f64 = reference
                    .iter()
                    .flatten()
                    .zip(changed.iter().flatten())
                    .map(|(a, b)| f64::from(a - b).powi(2))
                    .sum();
                assert!(difference > 1e-12, "custom seed modulation is inert");
            }
        }
    }

    #[test]
    fn failed_import_preserves_seed_and_browser_selection() {
        let params = Arc::new(ReverbParams::default());
        *params.seed_state.lock().unwrap() = ("previous.wav".into(), state());
        let location = ("/previous".into(), "/previous/previous.wav".into());
        *params.seed_wav_location.lock().unwrap() = location;
        let previous = params.serialize_fields();
        let shared = Arc::new(Shared::default());
        shared.rate.store(8000, Ordering::Release);
        let missing = std::env::temp_dir()
            .join(format!("missing-seed-{}", std::process::id()))
            .join("missing.wav");
        start_import(missing, &params, &shared);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while shared.busy.load(Ordering::Acquire) {
            assert!(
                std::time::Instant::now() < deadline,
                "import worker timed out"
            );
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        assert_eq!(params.serialize_fields(), previous);
        assert!(shared.pending.lock().unwrap().next.is_none());
        assert!(
            shared
                .status
                .lock()
                .unwrap()
                .starts_with("Import failed (previous seed retained):")
        );
    }

    #[test]
    fn pending_rate_mismatch_and_contention_do_not_replace() {
        let shared = Shared::default();
        let mut a = Playback::restore(8000, &ReverbParams::default());
        shared.pending.lock().unwrap().next = Some(Space::new(std::array::from_fn(|_| {
            Seed::decode(&state()).unwrap().build(48000).unwrap()
        })));
        let guard = shared.pending.lock().unwrap();
        a.receive(&shared, 8000); // must not block
        drop(guard);
        a.receive(&shared, 8000);
        assert!(a.network.is_none());
        assert!(shared.pending.lock().unwrap().retired.is_some());
    }
}
