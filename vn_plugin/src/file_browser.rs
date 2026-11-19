//! In-editor WAV picker: no native dialogs, host window handles or nested event loops.
use egui_plugin as egui;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver};

/// Folder location entry accepts quoted and home-relative paths.
fn location_path(text: &str) -> Result<PathBuf, String> {
    let text = text.trim();
    let text = text
        .strip_prefix('"')
        .and_then(|s| s.strip_suffix('"'))
        .or_else(|| text.strip_prefix('\'').and_then(|s| s.strip_suffix('\'')))
        .unwrap_or(text);
    if let Some(rest) = text.strip_prefix("~/") {
        return std::env::var_os("HOME")
            .map(|home| PathBuf::from(home).join(rest))
            .ok_or_else(|| "HOME is unavailable; use an absolute folder path".into());
    }
    let path = PathBuf::from(text);
    if !path.is_absolute() {
        return Err("Use an absolute folder path".into());
    }
    Ok(path)
}

struct Entry {
    path: PathBuf,
    directory: bool,
}

fn entries(directory: &Path) -> Result<Vec<Entry>, String> {
    let mut entries = std::fs::read_dir(directory)
        .map_err(|e| format!("Cannot open folder: {e}"))?
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let path = entry.path();
            let directory = path.is_dir();
            (directory || is_wav(&path)).then_some(Entry { path, directory })
        })
        .collect::<Vec<_>>();
    entries.sort_by_cached_key(|entry| {
        (
            !entry.directory,
            entry
                .path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_lowercase(),
        )
    });
    Ok(entries)
}

fn is_wav(path: &Path) -> bool {
    path.extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("wav"))
}

/// Cached, non-recursive WAV list. Only the GUI polls results; no audio-thread I/O.
#[derive(Default)]
pub struct WavFolder {
    directory: Option<PathBuf>,
    pub files: Vec<PathBuf>,
    pub error: Option<String>,
    listing: Option<Receiver<Result<Vec<Entry>, String>>>,
}

impl WavFolder {
    /// Follow the sorted folder list without wrapping at either end.
    pub fn adjacent(&self, selected: &Path, forward: bool) -> Option<&PathBuf> {
        if self.loading() {
            return None;
        }
        match self.files.iter().position(|path| path == selected) {
            Some(index) if forward => self.files.get(index + 1),
            Some(index) => index.checked_sub(1).and_then(|index| self.files.get(index)),
            None if forward => self.files.first(),
            None => self.files.last(),
        }
    }

    pub fn update(&mut self, directory: &Path, refresh: bool) {
        if self.directory.as_deref() != Some(directory) || refresh {
            self.directory = Some(directory.to_path_buf());
            self.files.clear();
            self.error = None;
            self.listing = None;
            if directory.as_os_str().is_empty() {
                return;
            }
            let path = directory.to_path_buf();
            let (sender, receiver) = mpsc::channel();
            match std::thread::Builder::new()
                .name("vn-wav-list".into())
                .spawn(move || {
                    let _ = sender.send(entries(&path));
                }) {
                Ok(_) => self.listing = Some(receiver),
                Err(e) => self.error = Some(format!("Cannot read folder: {e}")),
            }
        }
        if let Some(receiver) = &self.listing {
            match receiver.try_recv() {
                Ok(result) => {
                    match result {
                        Ok(entries) => {
                            self.files = entries
                                .into_iter()
                                .filter(|entry| !entry.directory)
                                .map(|entry| entry.path)
                                .collect()
                        }
                        Err(e) => self.error = Some(e),
                    }
                    self.listing = None;
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.error = Some("Folder reader stopped; use Refresh".into());
                    self.listing = None;
                }
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }
    }

    pub fn loading(&self) -> bool {
        self.listing.is_some()
    }
}

pub struct FileBrowser {
    pub open: bool,
    directory: PathBuf,
    location: String,
    entries: Vec<Entry>,
    selected: Option<PathBuf>,
    listing: Option<Receiver<Result<Vec<Entry>, String>>>,
    error: Option<String>,
}

fn file_list_scroll_area() -> egui::ScrollArea {
    egui::ScrollArea::both()
        // Keep the viewport full-width even for short names or an empty folder.
        .auto_shrink([false, false])
        // Reserve both bars even for empty/short lists, so horizontal overflow
        // cannot change the dialog height or move the footer buttons.
        .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysVisible)
        .max_height(260.0)
        .min_scrolled_height(260.0)
}

impl FileBrowser {
    pub fn new(initial: Option<&Path>) -> Self {
        let directory = initial
            .and_then(|p| if p.is_dir() { Some(p) } else { p.parent() })
            .filter(|p| p.is_absolute())
            .map(Path::to_path_buf)
            .or_else(|| std::env::var_os("HOME").map(PathBuf::from))
            .unwrap_or_else(|| PathBuf::from("/"));
        let mut browser = Self {
            open: true,
            directory: directory.clone(),
            location: String::new(),
            entries: Vec::new(),
            selected: None,
            listing: None,
            error: None,
        };
        browser.navigate(directory);
        browser
    }

    fn navigate(&mut self, directory: PathBuf) {
        self.location = directory.to_string_lossy().into_owned();
        self.directory = directory.clone();
        self.entries.clear();
        self.selected = None;
        self.error = None;
        let (sender, receiver) = mpsc::channel();
        // Slow disks/network mounts must not block the host's UI event loop.
        match std::thread::Builder::new()
            .name("vn-wav-browser".into())
            .spawn(move || {
                let _ = sender.send(entries(&directory));
            }) {
            Ok(_) => self.listing = Some(receiver),
            Err(e) => {
                self.listing = None;
                self.error = Some(format!("Cannot read folder: {e}"));
            }
        }
    }

    pub fn show(&mut self, ctx: &egui::Context) -> Option<PathBuf> {
        if !self.open {
            return None;
        }
        if let Some(receiver) = &self.listing {
            match receiver.try_recv() {
                Ok(result) => {
                    match result {
                        Ok(entries) => self.entries = entries,
                        Err(e) => self.error = Some(e),
                    }
                    self.listing = None;
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.error = Some("Folder reader stopped; try Go again".into());
                    self.listing = None;
                }
                Err(mpsc::TryRecvError::Empty) => {
                    ctx.request_repaint_after(std::time::Duration::from_millis(50));
                }
            }
        }
        let mut chosen = None;
        let response = egui::Modal::new(egui::Id::unique("vn-wav-browser")).show(ctx, |ui| {
            ui.set_width(450.0);
            ui.heading("Choose WAV IR");
            ui.add(
                egui::Label::new(
                    egui::RichText::new(
                        "Stop transport before loading. Double-click a folder to open it.",
                    )
                    .small(),
                )
                .truncate(),
            );
            ui.horizontal(|ui| {
                let location =
                    ui.add(egui::TextEdit::singleline(&mut self.location).desired_width(330.0));
                if ui.button("Go").clicked()
                    || (location.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)))
                {
                    match location_path(&self.location) {
                        Ok(path) => self.navigate(path),
                        Err(e) => self.error = Some(e),
                    }
                }
                if ui
                    .add_enabled(self.directory.parent().is_some(), egui::Button::new("Up"))
                    .clicked()
                    && let Some(parent) = self.directory.parent()
                {
                    self.navigate(parent.to_path_buf());
                }
            });
            let mut navigate = None;
            file_list_scroll_area().show(ui, |ui| {
                ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
                if self.listing.is_some() {
                    ui.label("Reading folder…");
                } else if self.entries.is_empty() && self.error.is_none() {
                    ui.label("No WAV files or folders here");
                }
                for entry in &self.entries {
                    let name = entry.path.file_name().unwrap_or_default().to_string_lossy();
                    let label = if entry.directory {
                        format!("[Folder] {name}")
                    } else {
                        name.into_owned()
                    };
                    let response =
                        ui.selectable_label(self.selected.as_ref() == Some(&entry.path), label);
                    if response.clicked() {
                        self.selected = Some(entry.path.clone());
                    }
                    if response.double_clicked() {
                        if entry.directory {
                            navigate = Some(entry.path.clone());
                        } else {
                            chosen = Some(entry.path.clone());
                        }
                    }
                }
            });
            if let Some(path) = navigate {
                self.navigate(path);
            }
            // Reserve a single line even without an error: stable modal height.
            ui.add(
                egui::Label::new(
                    egui::RichText::new(self.error.as_deref().unwrap_or(" "))
                        .color(ui.visuals().error_fg_color),
                )
                .truncate(),
            )
            .on_hover_text(self.error.as_deref().unwrap_or(""));
            ui.horizontal(|ui| {
                let file_selected = self
                    .entries
                    .iter()
                    .any(|entry| !entry.directory && self.selected.as_ref() == Some(&entry.path));
                if ui
                    .add_enabled(file_selected, egui::Button::new("Open & Load"))
                    .clicked()
                {
                    chosen = self.selected.clone();
                }
                if ui.button("Cancel").clicked() {
                    self.open = false;
                }
            });
        });
        if response.should_close() {
            self.open = false;
            return None;
        }
        if chosen.is_some() {
            self.open = false;
        }
        chosen
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adjacent_wav_follows_list_without_wrapping_and_waits_for_scan() {
        let mut folder = WavFolder::default();
        let a = PathBuf::from("/IR/a.wav");
        let b = PathBuf::from("/IR/b.wav");
        let absent = Path::new("/IR/missing.wav");
        assert!(folder.adjacent(absent, true).is_none());
        folder.files = vec![a.clone(), b.clone()];
        assert_eq!(folder.adjacent(&a, true), Some(&b));
        assert_eq!(folder.adjacent(&b, false), Some(&a));
        assert!(folder.adjacent(&a, false).is_none());
        assert!(folder.adjacent(&b, true).is_none());
        assert_eq!(folder.adjacent(absent, true), Some(&a));
        assert_eq!(folder.adjacent(absent, false), Some(&b));
        folder.files = vec![a.clone()];
        assert!(folder.adjacent(&a, true).is_none());
        assert!(folder.adjacent(&a, false).is_none());
        let (_sender, receiver) = mpsc::channel();
        folder.listing = Some(receiver);
        assert!(folder.adjacent(absent, true).is_none());
    }

    #[test]
    fn browser_footer_stays_fixed_for_empty_loading_long_and_many_files() {
        let ctx = egui::Context::default();
        let mut browser = FileBrowser::new(None);
        browser.listing = None;
        browser.error = None;
        browser.entries.clear();
        let mut baseline = None;
        let (sender, receiver) = mpsc::channel();
        let mut receiver = Some(receiver);
        for scenario in 0..5 {
            browser.entries = match scenario {
                1 => vec![Entry {
                    path: PathBuf::from("a.wav"),
                    directory: false,
                }],
                2 => (0..80)
                    .map(|index| Entry {
                        path: PathBuf::from(format!("{index}-{}.wav", "long_name".repeat(80))),
                        directory: false,
                    })
                    .collect(),
                _ => Vec::new(),
            };
            browser.error = (scenario == 3).then(|| "Error ".repeat(100));
            if scenario == 4 {
                browser.listing = receiver.take();
            }
            for frame in 0..5 {
                let mut output = ctx.run_ui(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(
                            egui::Pos2::ZERO,
                            egui::vec2(520.0, 735.0),
                        )),
                        time: Some((scenario * 5 + frame) as f64),
                        ..Default::default()
                    },
                    |ui| {
                        browser.show(ui.ctx());
                    },
                );
                output.textures_delta.clear(); // Headless test: no renderer consumes texture uploads.
                if frame == 4 {
                    let positions: Vec<_> = ["Choose WAV IR", "Cancel"]
                        .iter()
                        .map(|label| {
                            output
                                .shapes
                                .iter()
                                .find_map(|shape| match &shape.shape {
                                    egui::Shape::Text(text) if text.galley.job.text == *label => {
                                        Some(text.pos)
                                    }
                                    _ => None,
                                })
                                .expect("Browser heading and footer must be visible")
                        })
                        .collect();
                    if let Some(baseline) = &baseline {
                        assert_eq!(&positions, baseline, "Dialog moved for scenario {scenario}");
                    } else {
                        baseline = Some(positions);
                    }
                }
            }
        }
        drop(sender);
    }

    #[test]
    fn file_list_viewport_stays_full_width_for_short_and_long_names() {
        for name in ["", "a.wav", "Reading folder…", &"long_name".repeat(80)] {
            let ctx = egui::Context::default();
            for frame in 0..4 {
                let input = egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(520.0, 800.0),
                    )),
                    time: Some(frame as f64),
                    ..Default::default()
                };
                let mut output = ctx.run_ui(input, |ui| {
                    egui::Modal::new(egui::Id::unique("scroll-width-test")).show(ui.ctx(), |ui| {
                        ui.set_width(450.0);
                        let available = ui.available_rect_before_wrap();
                        let output = file_list_scroll_area()
                            .horizontal_scroll_offset(100.0)
                            .show(ui, |ui| {
                                ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
                                // Enough rows to require a vertical scrollbar, except for an empty folder.
                                if !name.is_empty() {
                                    for _ in 0..40 {
                                        ui.selectable_label(false, name).on_hover_text(name);
                                    }
                                }
                            });
                        assert!(
                            (available.right() - output.inner_rect.right()).abs() < 20.0,
                            "Scrollbar must stay at the right edge: {:?}",
                            output.inner_rect
                        );
                        assert!(output.inner_rect.width() > 430.0);
                        assert!((output.inner_rect.height() - 260.0).abs() < 1.0);
                        if name.len() > 100 && frame > 1 {
                            assert!(output.content_size.x > output.inner_rect.width());
                            assert!(
                                output.state.clamped_offset().x > 0.0,
                                "Long names must remain horizontally scrollable"
                            );
                        }
                    });
                });
                output.textures_delta.clear(); // Headless test: no renderer consumes texture uploads.
            }
        }
    }

    #[test]
    fn folder_paths_accept_quotes_and_reject_relative_paths() {
        for text in ["/tmp/a room", "  \"/tmp/a room\"\n", "'/tmp/a room'"] {
            assert_eq!(location_path(text).unwrap(), PathBuf::from("/tmp/a room"));
        }
        assert!(location_path(" ").is_err());
        assert!(location_path("relative").is_err());
        if let Some(home) = std::env::var_os("HOME") {
            assert_eq!(
                location_path("~/room").unwrap(),
                PathBuf::from(home).join("room")
            );
        }
    }

    #[test]
    fn lists_only_wavs_and_directories_with_directories_first() {
        let directory = std::env::temp_dir().join(format!("vn-browser-{}", std::process::id()));
        std::fs::create_dir_all(directory.join("z folder")).unwrap();
        for name in ["a.WAV", "b.wav", "c.txt"] {
            std::fs::write(directory.join(name), []).unwrap();
        }
        let list = entries(&directory).unwrap();
        let names: Vec<_> = list
            .iter()
            .map(|e| e.path.file_name().unwrap().to_str().unwrap())
            .collect();
        assert_eq!(names, ["z folder", "a.WAV", "b.wav"]);
        let mut folder = WavFolder::default();
        let wait = |folder: &mut WavFolder, path: &Path| {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
            loop {
                folder.update(path, false);
                if !folder.loading() {
                    break;
                }
                assert!(
                    std::time::Instant::now() < deadline,
                    "folder scan timed out"
                );
                std::thread::sleep(std::time::Duration::from_millis(2));
            }
        };
        wait(&mut folder, &directory);
        assert_eq!(
            folder.files,
            [directory.join("a.WAV"), directory.join("b.wav")]
        );
        std::fs::write(directory.join("d.wav"), []).unwrap();
        folder.update(&directory, true);
        wait(&mut folder, &directory);
        assert_eq!(folder.files.len(), 3);
        wait(&mut folder, &directory.join("missing"));
        assert!(folder.files.is_empty());
        assert!(folder.error.is_some());
        let browser = FileBrowser::new(Some(&directory.join("a.WAV")));
        assert_eq!(browser.directory, directory);
        assert!(entries(&directory.join("missing")).is_err());
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn escape_cancels_without_selecting_a_file() {
        let mut browser = FileBrowser::new(None);
        let ctx = egui::Context::default();
        for frame in 0..3 {
            let mut input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(520.0, 800.0),
                )),
                ..Default::default()
            };
            if frame == 2 {
                input.events.push(egui::Event::Key {
                    key: egui::Key::Escape,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: Default::default(),
                });
            }
            let mut output = ctx.run_ui(input, |ui| assert!(browser.show(ui.ctx()).is_none()));
            output.textures_delta.clear(); // Headless test: no renderer consumes texture uploads.
        }
        assert!(!browser.open);
    }
}
