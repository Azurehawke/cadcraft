//! The file browser CADCraft shows on Android for Open, Save As, Export, Plot to file, Import,
//! Attach and the other file choices: Android has no file dialog an app can wait on in a frame,
//! so the UI asks for files asynchronously (`Services::pick_async`) and this answers.
//!
//! Shortcuts to the storage places, the folder's subfolders and files (drawings, or every file),
//! a name field when saving, and a note with an Allow access… button while CADCraft may not read
//! shared storage ("All files access", granted once in Settings). Rows are tall enough for a
//! finger.

use std::path::{Path, PathBuf};

use cadcraft_ui_egui::PickRequest;
use egui::{Align2, RichText, vec2};

/// Drawings CADCraft opens; the "All files" choice shows the rest (linetype files, scripts…).
const DRAWINGS: &[&str] = &["dwg", "dxf", "dwt", "svg", "svgz"];
const MAX_LISTED: usize = 5000;
const MAX_SCANNED: usize = 100_000;
const ROW: f32 = 40.0;

#[derive(Clone, Debug, PartialEq)]
struct Entry {
    name: String,
    path: PathBuf,
    dir: bool,
}

/// What a frame of the browser decided.
pub enum Outcome {
    Open,
    /// Closed with this answer (`None`: cancelled).
    Done(Option<String>),
}

pub struct Browser {
    save: bool,
    dir: PathBuf,
    /// The name field (Save) or the highlighted file's name (Open).
    name: String,
    all_files: bool,
    listing: Result<(Vec<Entry>, bool), String>,
    selected: Option<PathBuf>,
    /// Save picked an existing file: OK again replaces it.
    replace: Option<PathBuf>,
    /// Shared storage was readable last frame.
    granted: Option<bool>,
    access_error: Option<String>,
}

fn has_ext(path: &Path, exts: &[&str]) -> bool {
    path.extension().and_then(|e| e.to_str()).is_some_and(|e| exts.iter().any(|x| x.eq_ignore_ascii_case(e)))
}

/// A folder's subfolders (first) and files of the wanted types, by name; the flag says more
/// were there than listed.
fn list(dir: &Path, all: bool) -> Result<(Vec<Entry>, bool), String> {
    let rd = std::fs::read_dir(dir).map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    let mut truncated = false;
    for e in rd.take(MAX_SCANNED).flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue;
        }
        let path = e.path();
        let dir = path.is_dir();
        if !dir && !all && !has_ext(&path, DRAWINGS) {
            continue;
        }
        if out.len() >= MAX_LISTED {
            truncated = true;
            break;
        }
        out.push(Entry { name, path, dir });
    }
    out.sort_by(|a, b| b.dir.cmp(&a.dir).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())));
    Ok((out, truncated))
}

impl Browser {
    pub fn new(req: &PickRequest, start: PathBuf) -> Self {
        let (save, name) = match req {
            PickRequest::Open => (false, String::new()),
            PickRequest::Save { name } => (true, name.clone()),
        };
        // Saving something that isn't a drawing (a PDF, PNG, SVG export): list every file, so
        // earlier exports show.
        let all_files = save && !has_ext(Path::new(&name), DRAWINGS);
        let mut b = Browser {
            save,
            dir: start.clone(),
            name,
            all_files,
            listing: Ok((Vec::new(), false)),
            selected: None,
            replace: None,
            granted: None,
            access_error: None,
        };
        b.navigate(start);
        b
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn navigate(&mut self, dir: PathBuf) {
        self.listing = list(&dir, self.all_files);
        self.dir = dir;
        self.selected = None;
        self.replace = None;
        if !self.save {
            self.name.clear();
        }
    }

    /// OK: the typed or highlighted file. Save adds the suggested extension to a bare name and
    /// asks once more before replacing a file.
    fn accept(&mut self) -> Option<String> {
        let typed = self.name.trim();
        if typed.is_empty() {
            return None;
        }
        let mut path = if Path::new(typed).is_absolute() { PathBuf::from(typed) } else { self.dir.join(typed) };
        if path.is_dir() {
            self.navigate(path);
            return None;
        }
        if !self.save {
            return path.is_file().then(|| path.to_string_lossy().into_owned());
        }
        if path.extension().is_none() {
            path.set_extension("dwg");
        }
        if path.exists() && self.replace.as_ref() != Some(&path) {
            self.replace = Some(path);
            return None;
        }
        Some(path.to_string_lossy().into_owned())
    }

    /// The access note and the storage shortcuts.
    fn storage_bar(&mut self, ui: &mut egui::Ui) {
        let granted = crate::file_access::granted();
        if self.granted == Some(false) && granted {
            // Access was just turned on in Settings: list the folder again.
            self.access_error = None;
            self.navigate(self.dir.clone());
        }
        self.granted = Some(granted);
        if !granted {
            egui::Frame::group(ui.style()).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label("To open and save drawings in your folders, allow CADCraft to manage all files. Without it, CADCraft can only use its own app folder.");
                if ui.add(egui::Button::new("Allow access…").min_size(vec2(0.0, 36.0))).clicked() {
                    self.access_error = crate::file_access::request().err();
                }
                if let Some(e) = &self.access_error {
                    ui.colored_label(
                        ui.visuals().error_fg_color,
                        format!("Settings didn't open ({e}). Turn it on in Settings › Apps › Special access › All files access › CADCraft."),
                    );
                }
            });
            ui.add_space(6.0);
        }
        ui.horizontal_wrapped(|ui| {
            for (label, dir) in crate::places() {
                let here = self.dir == dir;
                if ui.add(egui::Button::selectable(here, label).min_size(vec2(0.0, 34.0))).clicked() {
                    self.navigate(dir);
                }
            }
        });
    }

    /// Draw a frame of the browser.
    pub fn show(&mut self, ctx: &egui::Context) -> Outcome {
        let mut outcome = Outcome::Open;
        let screen = ctx.content_rect();
        let width = (screen.width() - 32.0).clamp(280.0, 720.0);
        let height = (screen.height() * 0.5).clamp(160.0, 520.0);
        let mut open = true;
        egui::Window::new(if self.save { "Save" } else { "Open" })
            .id(egui::Id::new("cadcraft_android_browser"))
            .collapsible(false)
            .resizable(false)
            .anchor(Align2::CENTER_CENTER, vec2(0.0, 0.0))
            .open(&mut open)
            .show(ctx, |ui| {
                ui.set_width(width);
                self.storage_bar(ui);
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    let up = self.dir.parent().map(Path::to_path_buf);
                    if ui.add_enabled(up.is_some(), egui::Button::new("⬆ Up").min_size(vec2(64.0, 34.0))).clicked()
                        && let Some(up) = up
                    {
                        self.navigate(up);
                    }
                    ui.label(RichText::new(self.dir.display().to_string()).weak());
                });
                let mut go = None;
                let mut pick = false;
                egui::Frame::group(ui.style()).show(ui, |ui| {
                    egui::ScrollArea::vertical().max_height(height).min_scrolled_height(height).auto_shrink([false, false]).show(
                        ui,
                        |ui| match &self.listing {
                            Err(e) => {
                                ui.colored_label(ui.visuals().error_fg_color, format!("This folder can't be read: {e}"));
                            }
                            Ok((entries, truncated)) => {
                                if entries.is_empty() {
                                    ui.label(RichText::new("No drawings here.").weak());
                                }
                                for e in entries {
                                    let label = if e.dir { format!("📁  {}", e.name) } else { format!("📄  {}", e.name) };
                                    let sel = self.selected.as_ref() == Some(&e.path);
                                    let r = ui.add_sized([ui.available_width(), ROW], egui::Button::selectable(sel, label));
                                    if r.clicked() {
                                        if e.dir {
                                            go = Some(e.path.clone());
                                        } else {
                                            self.selected = Some(e.path.clone());
                                            self.name = e.name.clone();
                                            self.replace = None;
                                        }
                                    }
                                    if r.double_clicked() && !e.dir {
                                        pick = true;
                                    }
                                }
                                if *truncated {
                                    ui.label(RichText::new(format!("Showing the first {MAX_LISTED} items.")).weak());
                                }
                            }
                        },
                    );
                });
                if let Some(dir) = go {
                    self.navigate(dir);
                }
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.label("File name:");
                    let r = ui.add(egui::TextEdit::singleline(&mut self.name).desired_width(ui.available_width() - 150.0));
                    if r.changed() {
                        self.replace = None;
                    }
                    if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                        pick = true;
                    }
                    if ui.checkbox(&mut self.all_files, "All files").changed() {
                        self.navigate(self.dir.clone());
                    }
                });
                if let Some(p) = &self.replace {
                    let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                    ui.colored_label(ui.visuals().warn_fg_color, format!("{name} already exists. Tap Save again to replace it."));
                }
                ui.add_space(6.0);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.add(egui::Button::new("Cancel").min_size(vec2(96.0, 38.0))).clicked() || ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                        outcome = Outcome::Done(None);
                    }
                    if ui.add(egui::Button::new(if self.save { "Save" } else { "Open" }).min_size(vec2(96.0, 38.0))).clicked() {
                        pick = true;
                    }
                });
                if pick && let Some(path) = self.accept() {
                    outcome = Outcome::Done(Some(path));
                }
            });
        if !open {
            outcome = Outcome::Done(None);
        }
        outcome
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_drawings_and_folders_and_saves_with_an_extension() {
        let dir = std::env::temp_dir().join(format!("cadcraft-android-browser-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::write(dir.join("plan.DWG"), b"x").unwrap();
        std::fs::write(dir.join("notes.txt"), b"x").unwrap();
        let (entries, _) = list(&dir, false).unwrap();
        let names: Vec<_> = entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["sub", "plan.DWG"]);
        assert_eq!(list(&dir, true).unwrap().0.len(), 3);

        let mut b = Browser::new(&PickRequest::Save { name: "Drawing1".into() }, dir.clone());
        assert_eq!(b.accept().as_deref(), Some(dir.join("Drawing1.dwg").to_string_lossy().as_ref()));
        b.name = "plan.DWG".into();
        assert_eq!(b.accept(), None, "asks before replacing");
        assert!(b.accept().is_some(), "replaces the second time");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
