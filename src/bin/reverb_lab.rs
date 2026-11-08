use algo_reverb::Matrix;
use algo_reverb::preview::{Clip, Controls, PREVIEW_RATE, PreviewSource, Settings, load_wav};
use eframe::egui;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::sync::mpsc::{self, Receiver};
use std::time::Duration;

fn main() -> eframe::Result {
    eframe::run_native(
        "Reverb Lab",
        eframe::NativeOptions {
            viewport: egui::ViewportBuilder::default().with_inner_size([660.0, 560.0]),
            ..Default::default()
        },
        Box::new(|_| Ok(Box::new(Lab::new()))),
    )
}

type LoadResult = Result<(PathBuf, Clip), String>;

struct Lab {
    // Retain the device stream for the lifetime of playback.
    stream: Option<rodio::OutputStream>,
    sink: Option<rodio::Sink>,
    controls: Arc<Controls>,
    settings: Settings,
    clip: Option<Arc<Clip>>,
    name: String,
    error: Option<String>,
    loading: Option<Receiver<LoadResult>>,
}

impl Lab {
    fn new() -> Self {
        let mut app = Self {
            stream: None,
            sink: None,
            controls: Arc::new(Controls::default()),
            settings: Settings::default(),
            clip: None,
            name: "No WAV loaded".into(),
            error: None,
            loading: None,
        };
        app.open_audio();
        app
    }

    fn open_audio(&mut self) {
        match rodio::OutputStreamBuilder::open_default_stream() {
            Ok(stream) => {
                self.stream = Some(stream);
                self.error = None;
            }
            Err(error) => {
                self.error = Some(format!("Audio output unavailable: {error}"));
            }
        }
    }

    fn start_load(&mut self, path: PathBuf, ctx: egui::Context) {
        let (sender, receiver) = mpsc::sync_channel(1);
        self.loading = Some(receiver);
        self.error = None;
        std::thread::spawn(move || {
            let result = load_wav(&path).map(|clip| (path, clip));
            let _ = sender.send(result);
            ctx.request_repaint();
        });
    }

    fn attach_clip(&mut self) {
        let (Some(stream), Some(clip)) = (&self.stream, &self.clip) else {
            return;
        };
        if let Some(old) = self.sink.take() {
            old.stop();
        }
        // New controls prevent the old source publishing stale transport state.
        let controls = Arc::new(Controls::default());
        controls.publish(self.settings);
        let source = PreviewSource::new(clip.clone(), controls.clone());
        let sink = rodio::Sink::connect_new(stream.mixer());
        sink.append(source);
        self.controls = controls;
        self.sink = Some(sink);
    }

    fn poll_load(&mut self) {
        let Some(receiver) = &self.loading else {
            return;
        };
        match receiver.try_recv() {
            Ok(result) => {
                self.loading = None;
                match result {
                    Ok((path, clip)) => {
                        self.name = path.display().to_string();
                        self.clip = Some(Arc::new(clip));
                        self.attach_clip();
                    }
                    Err(error) => self.error = Some(error),
                }
            }
            Err(mpsc::TryRecvError::Disconnected) => {
                self.loading = None;
                self.error = Some("WAV loading worker stopped unexpectedly".into());
            }
            Err(mpsc::TryRecvError::Empty) => {}
        }
    }
}

impl eframe::App for Lab {
    fn update(&mut self, ctx: &egui::Context, _: &mut eframe::Frame) {
        self.poll_load();
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading("Reverb Lab");
            ui.label("8-line FDN / realtime WAV audition");
            ui.separator();
            if ui.add_enabled(self.loading.is_none(), egui::Button::new("Open WAV…")).clicked()
                && let Some(path) = rfd::FileDialog::new().add_filter("WAV audio", &["wav"]).pick_file()
            {
                self.start_load(path, ctx.clone());
            }
            if self.loading.is_some() { ui.spinner(); ui.label("Decoding and resampling…"); }
            ui.label(&self.name);
            if let Some(clip) = &self.clip {
                ui.label(format!("{} Hz / {} channel(s) -> 48 kHz stereo / {:.2} s",
                    clip.original_rate, clip.original_channels,
                    clip.frames.len() as f64 / f64::from(PREVIEW_RATE)));
            }
            if self.stream.is_none() && ui.button("Retry audio output").clicked() {
                self.open_audio();
                self.attach_clip();
            }
            ui.add_enabled_ui(self.sink.is_some(), |ui| {
                ui.horizontal(|ui| {
                    let playing = self.controls.playing.load(Ordering::Relaxed);
                    if ui.button(if playing { "Pause" } else { "Play" }).clicked() {
                        if !playing && let Some(clip) = &self.clip
                            && self.controls.position.load(Ordering::Relaxed) >= clip.frames.len()
                        {
                            self.controls.rewind.store(true, Ordering::Relaxed);
                        }
                        self.controls.playing.store(!playing, Ordering::Relaxed);
                    }
                    if ui.button("Stop / rewind").clicked() {
                        self.controls.playing.store(false, Ordering::Relaxed);
                        self.controls.rewind.store(true, Ordering::Relaxed);
                    }
                    let mut looping = self.controls.looping.load(Ordering::Relaxed);
                    if ui.checkbox(&mut looping, "Loop").changed() {
                        self.controls.looping.store(looping, Ordering::Relaxed);
                    }
                });
            });
            let seconds = self.controls.position.load(Ordering::Relaxed) as f64 / f64::from(PREVIEW_RATE);
            ui.label(format!("Position: {seconds:.2} s (tail continues after file end)"));
            ui.separator();
            let before = self.settings;
            ui.horizontal(|ui| {
                ui.label("Matrix");
                ui.selectable_value(&mut self.settings.matrix, Matrix::Hadamard, "Hadamard");
                ui.selectable_value(&mut self.settings.matrix, Matrix::Householder, "Householder");
                ui.selectable_value(&mut self.settings.matrix, Matrix::Hybrid, "Householder -> Hadamard");
            });
            ui.add(egui::Slider::new(&mut self.settings.diffusion, 0.0..=1.0).text("Input diffusion"));
            ui.add(egui::Slider::new(&mut self.settings.t60, 0.1..=20.0).logarithmic(true).text("T60 / s"));
            ui.add(egui::Slider::new(&mut self.settings.damping, 100.0..=20_000.0).logarithmic(true).text("High damping / Hz"));
            ui.add(egui::Slider::new(&mut self.settings.damping_low, 0.0..=24.0).text("Low damping / dB"));
            ui.add(egui::Slider::new(&mut self.settings.damping_mid, 0.0..=24.0).text("Mid damping / dB"));
            egui::CollapsingHeader::new("Damping bands").show(ui, |ui| {
                ui.add(egui::Slider::new(&mut self.settings.damping_low_hz, 20.0..=2000.0).logarithmic(true).text("Low / Hz"));
                ui.add(egui::Slider::new(&mut self.settings.damping_mid_hz, 100.0..=12000.0).logarithmic(true).text("Mid / Hz"));
                ui.add(egui::Slider::new(&mut self.settings.damping_mid_q, 0.2..=8.0).logarithmic(true).text("Mid Q"));
            });
            ui.add(egui::Slider::new(&mut self.settings.wet, 0.0..=1.0).text("Dry / Wet"));
            ui.add(egui::Slider::new(&mut self.settings.gain_db, -60.0..=0.0).text("Output / dB"));
            ui.checkbox(&mut self.settings.bypass, "Bypass reverb (dry comparison, output gain retained)");
            if before != self.settings { self.controls.publish(self.settings); }
            let peak = f32::from_bits(self.controls.peak.load(Ordering::Relaxed));
            ui.label(format!("Output peak: {:.1} dBFS", 20.0 * peak.max(1e-6).log10()));
            if self.controls.clipped.load(Ordering::Relaxed) {
                ui.colored_label(egui::Color32::YELLOW, "Output clipped: lower the gain. Audition output is hard-clamped.");
                if ui.button("Clear clip indicator").clicked() {
                    self.controls.clipped.store(false, Ordering::Relaxed);
                }
            }
            ui.separator();
            ui.small("Wet input = (L + R) / 2. Dry retains stereo. Start with low listening volume.");
            ui.small("Pause lets the tail decay silently; rewind/loop retains it. Loop boundaries are not crossfaded.");
            ui.small("WAV only, up to 5 min. Non-48 kHz audio uses linear audition resampling.");
            if let Some(error) = &self.error {
                ui.colored_label(egui::Color32::LIGHT_RED, error);
            }
        });
        ctx.request_repaint_after(Duration::from_millis(33));
    }
}
