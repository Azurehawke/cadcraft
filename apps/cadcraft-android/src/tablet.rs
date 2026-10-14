//! What a tablet needs around the desktop UI.
//!
//! - **System bars.** NativeActivity draws under the status bar and the taskbar. Android reports
//!   the part of the window they leave free (the content rect); the app is laid out inside it,
//!   egui is told the same insets, and the bars are coloured like the menu bar with icons that
//!   show on that colour.
//! - **Touch, drafting-style.** vendor/winit tags each touch with its tool (finger, S Pen, eraser,
//!   mouse, palm). egui-winit would turn whichever touch came first into a pressed mouse pointer,
//!   so a finger drag would draw a selection window and a resting palm would pick points. Here
//!   the pointer is rebuilt from the touches instead:
//!   - the S Pen (or a mouse) is the pointer: it picks points and objects and drags windows, and
//!     a hovering pen moves the crosshair; while it is down fingers and palms don't count;
//!   - a finger drag pans the drawing (sent as a middle-button drag, as a mouse pans), two
//!     fingers pan and pinch to zoom, a finger tap clicks, and a finger held still for half a
//!     second right-clicks (the shortcut menu);
//!   - a palm never counts.
//! - **Keyboard.** With a keyboard cover or Bluetooth keyboard attached, the command line and text
//!   fields don't ask for the on-screen keyboard.
//! - **Files** (`browser`), **the clipboard** (`clipboard`), **drawings from other apps**
//!   (`incoming`) and **updates** (`updater`).

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use cadcraft_ui_egui::CadApp;
use egui::{Event, Modifiers, PointerButton, Pos2, TouchPhase, Vec2};
use winit::platform::android::activity::AndroidApp;

use crate::browser::{Browser, Outcome};
use crate::incoming::Incoming;

/// A finger that moves further than this (points) before lifting pans instead of tapping.
const SLOP: f32 = 10.0;
/// A finger held this long (seconds) without moving right-clicks.
const LONG_PRESS: f64 = 0.5;

pub struct TabletApp {
    cad: CadApp,
    /// The UI's request for a file, and the browser answering it.
    pick: crate::PickSlot,
    browser: Option<Browser>,
    /// Where the browser was last (it opens there next).
    last_dir: Option<PathBuf>,
    workspace: crate::workspace::Workspace,
    android: AndroidApp,
    touches: TouchRouter,
    /// The insets last applied, in points.
    insets: egui::epaint::MarginF32,
    /// Look for a drawing sent by another app (`incoming`): at start-up and when the app comes
    /// back to the front.
    check_incoming: bool,
    focused: bool,
    updater: crate::updater::Updater,
    /// The system bars' colour last sent to Android.
    bar_color: Option<egui::Color32>,
}

impl TabletApp {
    pub fn new(cad: CadApp, pick: crate::PickSlot, workspace: crate::workspace::Workspace, android: AndroidApp) -> Self {
        Self {
            cad,
            pick,
            browser: None,
            last_dir: None,
            workspace,
            android,
            touches: TouchRouter::default(),
            insets: egui::epaint::MarginF32::default(),
            check_incoming: true,
            focused: true,
            updater: crate::updater::Updater::default(),
            bar_color: None,
        }
    }

    fn persist_workspace(&mut self) {
        if let Err(e) = self.workspace.save_if_changed(&self.cad.ui) {
            log::warn!("workspace not saved: {e}");
        }
    }

    /// Open the browser for the UI's file request, if there is one.
    fn start_pick(&mut self) {
        let Some(req) = self.pick.borrow_mut().take() else { return };
        let start = self
            .last_dir
            .clone()
            .filter(|d| d.is_dir())
            .or_else(|| {
                let docs = PathBuf::from(crate::SHARED).join("Documents");
                docs.is_dir().then_some(docs)
            })
            .or_else(crate::config_dir)
            .unwrap_or_else(|| PathBuf::from("/"));
        self.browser = Some(Browser::new(&req, start));
    }

    /// Open the drawing another app sent, if there is one. One that only exists in the other
    /// app (an attachment, a cloud file) is first kept as a file of CADCraft's own, so Save
    /// writes somewhere real. It opens in a drawing tab.
    fn open_incoming(&mut self) {
        let path = match crate::incoming::take() {
            None => return,
            Some(Incoming::Path(path)) => path,
            Some(Incoming::Bytes { name, bytes }) => match keep_copy(&name, &bytes) {
                Ok(path) => {
                    let msg = format!("Saved a copy of {name} in {}", path.parent().map_or_else(String::new, |p| p.display().to_string()));
                    self.cad.set_status(msg);
                    path.to_string_lossy().into_owned()
                }
                Err(e) => {
                    self.cad.set_status(format!("Couldn't open {name}: {e}"));
                    return;
                }
            },
            Some(Incoming::Failed(e)) => {
                self.cad.set_status(e);
                return;
            }
        };
        log::info!("opening {path}");
        self.cad.open_path(&path);
    }

    /// Colour the status bar and taskbar `color`, with dark icons on a light colour and light
    /// ones on a dark colour.
    fn colour_system_bars(&mut self, color: egui::Color32) {
        if self.bar_color == Some(color) {
            return;
        }
        self.bar_color = Some(color);
        let [r, g, b, _] = color.to_array();
        let argb = i32::from_ne_bytes(u32::from_be_bytes([0xff, r, g, b]).to_ne_bytes());
        let luminance = 0.2126 * f32::from(r) + 0.7152 * f32::from(g) + 0.0722 * f32::from(b);
        let light = luminance > 140.0;
        let _ = crate::file_access::with_activity("colouring the system bars", |env, activity| {
            env.call_method(activity, "ccSetSystemBars", "(IZ)V", &[jni::objects::JValue::Int(argb), jni::objects::JValue::Bool(u8::from(light))])?;
            Ok(())
        });
    }

    /// EXIT / QUIT: close the activity (after the UI dealt with unsaved drawings).
    fn finish_activity() {
        let _ = crate::file_access::with_activity("closing CADCraft", |env, activity| {
            env.call_method(activity, "finish", "()V", &[])?;
            Ok(())
        });
    }

    /// With a keyboard cover or Bluetooth keyboard attached, typing arrives as key presses, so a
    /// text field doesn't ask for the on-screen keyboard: Samsung shows its keyboard toolbar for
    /// that request, which takes room from the app and comes and goes as fields gain and lose
    /// focus. Without one, fields ask as usual.
    fn keep_soft_keyboard_away(&self, ctx: &egui::Context) {
        if matches!(self.android.config().keys_hidden(), ndk::configuration::KeysHidden::No) {
            ctx.output_mut(|o| o.ime = None);
        }
    }

    /// The window's edges covered by system bars, in points, from Android's content rect.
    fn system_bar_insets(&self, ctx: &egui::Context, raw: &egui::RawInput) -> Option<egui::epaint::MarginF32> {
        let ppp = ctx.pixels_per_point();
        let screen = raw.screen_rect?;
        if !(ppp.is_finite() && ppp > 0.0) {
            return None;
        }
        let r = self.android.content_rect();
        if r.right <= r.left || r.bottom <= r.top {
            return None;
        }
        let (w, h) = (screen.width() * ppp, screen.height() * ppp);
        let inset = |px: f32| (px / ppp).clamp(0.0, 400.0);
        Some(egui::epaint::MarginF32 {
            left: inset(r.left as f32),
            top: inset(r.top as f32),
            right: inset(w - r.right as f32),
            bottom: inset(h - r.bottom as f32),
        })
    }

    /// Ctrl+V pastes what other apps copied (and CADCraft's own drawing copies, as DXF text):
    /// egui-winit's own clipboard only knows this app's copies.
    fn system_paste(raw: &mut egui::RawInput) {
        let ctrl_v = raw.events.iter().any(|e| {
            matches!(e, Event::Key { key: egui::Key::V, pressed: true, modifiers, .. } if modifiers.command && !modifiers.alt && !modifiers.shift)
        });
        let has_paste = raw.events.iter().any(|e| matches!(e, Event::Paste(_)));
        if !(ctrl_v || has_paste) {
            return;
        }
        let Some(text) = crate::clipboard::text() else { return };
        if has_paste {
            for e in &mut raw.events {
                if let Event::Paste(p) = e {
                    *p = text.clone();
                }
            }
        } else {
            raw.events.push(Event::Paste(text));
        }
    }
}

impl eframe::App for TabletApp {
    fn raw_input_hook(&mut self, ctx: &egui::Context, raw: &mut egui::RawInput) {
        if let Some(insets) = self.system_bar_insets(ctx, raw) {
            if insets != self.insets {
                log::info!("system bar insets (points): {insets:?}");
            }
            self.insets = insets;
            raw.safe_area_insets = Some(egui::SafeAreaInsets(insets));
        }
        if raw.focused && !self.focused {
            // Back in front, perhaps with another drawing to open.
            self.check_incoming = true;
        }
        if !raw.focused && self.focused {
            // Android may stop a backgrounded app without warning.
            self.persist_workspace();
        }
        self.focused = raw.focused;
        self.touches.route(&mut raw.events, raw.time.unwrap_or(0.0));
        if self.touches.waiting() {
            // A lift held back for the next frame, or a finger that may become a long press.
            ctx.request_repaint_after(std::time::Duration::from_millis(50));
        }
        Self::system_paste(raw);
        self.cad.raw_input_hook(raw);
        cadcraft_ui_egui::cmdline::capture_tab(ctx, raw);
    }

    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if std::mem::take(&mut self.check_incoming) {
            self.open_incoming();
        }
        self.updater.maybe_check(ctx.input(|i| i.time));
        self.cad.logic(ctx);
        self.persist_workspace();
        if std::mem::take(&mut self.cad.quit_requested) {
            Self::finish_activity();
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let full = ui.max_rect();
        let m = self.insets;
        let mut inner = egui::Rect::from_min_max(full.min + egui::vec2(m.left, m.top), full.max - egui::vec2(m.right, m.bottom));
        if inner.width() < 200.0 || inner.height() < 200.0 {
            inner = full;
        }
        // Behind the status bar and taskbar: the menu bar's colour, so the bars read as part of
        // the app, and Android draws their icons dark or light to match.
        let bar = cadcraft_ui_egui::theme::Tokens::get().chrome;
        ui.painter().rect_filled(full, 0.0, bar);
        self.colour_system_bars(bar);
        let cad = &mut self.cad;
        ui.scope_builder(egui::UiBuilder::new().max_rect(inner), |ui| {
            ui.set_clip_rect(inner);
            cad.ui(ui);
        });
        // A file the UI asked for this frame: the browser answers it.
        self.start_pick();
        if let Some(b) = &mut self.browser
            && let Outcome::Done(path) = b.show(ui.ctx())
        {
            self.last_dir = Some(b.dir().to_path_buf());
            self.browser = None;
            self.cad.finish_pick(path);
            ui.ctx().request_repaint();
        }
        self.updater.show(ui.ctx(), inner);
        // What CADCraft copied this frame goes on the system clipboard too.
        let copied = ui.ctx().output(|o| {
            o.commands.iter().rev().find_map(|c| match c {
                egui::OutputCommand::CopyText(t) => Some(t.clone()),
                _ => None,
            })
        });
        if let Some(text) = copied {
            crate::clipboard::set_text(&text);
        }
        self.keep_soft_keyboard_away(ui.ctx());
        self.persist_workspace();
    }

    /// A normal exit: the workspace is saved, and this run's autosave files are not needed (a
    /// crash leaves them for recovery).
    fn on_exit(&mut self) {
        if let Err(e) = self.workspace.save_on_exit(&self.cad.ui) {
            log::warn!("workspace not saved: {e}");
        }
        self.cad.shutdown_autosave();
    }

    /// Preferences (profile, interface, autosave) survive restarts, as on the desktop.
    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        storage.set_string(cadcraft_ui_egui::options::PROFILE_KEY, self.cad.profile_json());
        storage.set_string(cadcraft_ui_egui::PREFS_KEY, self.cad.prefs_json());
        storage.set_string(cadcraft_ui_egui::autosave::PREFS_KEY, cadcraft_ui_egui::autosave::prefs_json());
    }

    fn persist_egui_memory(&self) -> bool {
        false
    }
}

/// Keep the bytes of a drawing that only exists in another app as a file CADCraft can open and
/// save: in Documents/CADCraft when shared storage is accessible, else in the app's own folder.
/// An existing file of that name is never replaced ("Plan (2).dwg").
fn keep_copy(name: &str, bytes: &[u8]) -> Result<PathBuf, String> {
    let dir = if crate::file_access::granted() {
        PathBuf::from(crate::SHARED).join("Documents/CADCraft")
    } else {
        crate::config_dir().map(|d| d.join("Received")).ok_or("no app folder")?
    };
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let path = free_name(&dir, &safe_file_name(name));
    std::fs::write(&path, bytes).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(path)
}

/// `name` without folders or characters a file name can't hold; never empty.
fn safe_file_name(name: &str) -> String {
    let base = name.rsplit(['/', '\\']).next().unwrap_or(name);
    let cleaned: String =
        base.chars().map(|c| if c.is_control() || matches!(c, ':' | '*' | '?' | '"' | '<' | '>' | '|') { '_' } else { c }).collect();
    let cleaned = cleaned.trim().trim_start_matches('.').to_string();
    if cleaned.is_empty() { "Drawing.dwg".to_string() } else { cleaned }
}

/// `dir/name`, or `dir/stem (2).ext` and so on when that file already exists.
fn free_name(dir: &Path, name: &str) -> PathBuf {
    let first = dir.join(name);
    if !first.exists() {
        return first;
    }
    let p = Path::new(name);
    let stem = p.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| name.to_string());
    let ext = p.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
    for n in 2..10_000 {
        let candidate = dir.join(format!("{stem} ({n}){ext}"));
        if !candidate.exists() {
            return candidate;
        }
    }
    first
}

/// Which tool a touch is, from the bits vendor/winit puts above the pointer id.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Tool {
    Finger,
    Pen,
    Mouse,
    Palm,
}

fn tool_of(id: u64) -> Tool {
    match (id >> 32) & 0xff {
        2 | 4 => Tool::Pen, // stylus, eraser end
        3 => Tool::Mouse,
        5 => Tool::Palm,
        _ => Tool::Finger, // finger, or unknown
    }
}

/// A touch that is down.
#[derive(Clone, Copy, Debug)]
struct Down {
    tool: Tool,
    device: egui::TouchDeviceId,
    pos: Pos2,
}

/// What fingers are doing (no pen or mouse down).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
enum Fingers {
    #[default]
    Idle,
    /// One finger down, not moved past [`SLOP`] yet: a tap, a long press or a pan.
    Pending { id: u64, start: Pos2, since: f64 },
    /// One finger panning: the middle button is down.
    Panning { id: u64 },
    /// Two or more fingers: pan by their centre (middle button down once `panning`), and egui
    /// pinch-zooms from the touches.
    Multi { panning: bool },
    /// The gesture was used up (a long press fired): ignore the rest until every finger lifts.
    Spent,
}

#[derive(Default)]
struct TouchRouter {
    /// Touches down now, by id (hidden ones too).
    down: HashMap<u64, Down>,
    /// Touches whose `Touch` events egui doesn't see (a palm; fingers while the pen is down).
    hidden: HashSet<u64>,
    /// The pen or mouse touch that drives egui's pointer.
    owner: Option<u64>,
    fingers: Fingers,
    /// Modifier keys as last reported (pointer events carry them).
    modifiers: Modifiers,
    /// Pointer events for the next frame, and for the one after. A quick tap often arrives in one
    /// frame (down, up), and egui finds what is under the pointer once per frame, after the
    /// frame's events: with the lift and the pointer's leaving in the same frame as the press,
    /// nothing was under it and the tap clicked nothing. So a lift waits for the next frame when
    /// its press came in this one, and the pointer leaves one frame after the lift.
    next: Vec<Event>,
    after: Vec<Event>,
    /// Touches that started in the frame being routed.
    started_now: HashSet<u64>,
}

impl TouchRouter {
    /// Something waits for a later frame: held-back pointer events, or a finger that may turn
    /// into a long press.
    fn waiting(&self) -> bool {
        !self.next.is_empty() || !self.after.is_empty() || matches!(self.fingers, Fingers::Pending { .. })
    }

    fn pen_down(&self) -> bool {
        self.down.values().any(|d| matches!(d.tool, Tool::Pen | Tool::Mouse))
    }

    fn finger_ids(&self) -> Vec<u64> {
        self.down.iter().filter(|(id, d)| d.tool == Tool::Finger && !self.hidden.contains(id)).map(|(id, _)| *id).collect()
    }

    fn centre(&self) -> Option<Pos2> {
        let ids = self.finger_ids();
        if ids.is_empty() {
            return None;
        }
        let sum = ids.iter().filter_map(|id| self.down.get(id)).fold(Vec2::ZERO, |s, d| s + d.pos.to_vec2());
        Some((sum / ids.len() as f32).to_pos2())
    }

    fn button(&self, out: &mut Vec<Event>, pos: Pos2, button: PointerButton, pressed: bool) {
        out.push(Event::PointerButton { pos, button, pressed, modifiers: self.modifiers });
    }

    /// A new press: the last touch's pointer doesn't leave after all.
    fn keep_pointer(&mut self, out: &mut Vec<Event>) {
        out.retain(|e| !matches!(e, Event::PointerGone));
        self.next.retain(|e| !matches!(e, Event::PointerGone));
        self.after.retain(|e| !matches!(e, Event::PointerGone));
    }

    /// A click of `button` at `pos`: the press now, the release next frame, and the pointer
    /// leaves a frame after that.
    fn click(&mut self, out: &mut Vec<Event>, pos: Pos2, button: PointerButton) {
        self.keep_pointer(out);
        out.push(Event::PointerMoved(pos));
        self.button(out, pos, button, true);
        self.next.push(Event::PointerButton { pos, button, pressed: false, modifiers: self.modifiers });
        self.after.push(Event::PointerGone);
    }

    /// The owner's lift at `pos`: now (or next frame if it was pressed in this one), and the
    /// pointer leaves a frame later.
    fn lift(&mut self, id: u64, pos: Pos2, out: &mut Vec<Event>) {
        let release = Event::PointerButton { pos, button: PointerButton::Primary, pressed: false, modifiers: self.modifiers };
        if self.started_now.contains(&id) {
            self.next.push(release);
            self.after.push(Event::PointerGone);
        } else {
            out.push(release);
            self.next.push(Event::PointerGone);
        }
    }

    /// Stop egui seeing a touch it has seen start: it ends as cancelled.
    fn hide(&mut self, id: u64, out: &mut Vec<Event>) {
        if !self.hidden.insert(id) {
            return;
        }
        if let Some(d) = self.down.get(&id) {
            out.push(Event::Touch { device_id: d.device, id: egui::TouchId(id), phase: TouchPhase::Cancel, pos: d.pos, force: None });
        }
    }

    /// End a pan in progress (the middle button comes up where the pointer is).
    fn end_pan(&mut self, out: &mut Vec<Event>, pos: Pos2) {
        if matches!(self.fingers, Fingers::Panning { .. } | Fingers::Multi { panning: true }) {
            self.button(out, pos, PointerButton::Middle, false);
            self.next.push(Event::PointerGone);
        }
    }

    /// Replace egui-winit's touch-to-pointer translation (module docs). `now` is the input time.
    fn route(&mut self, events: &mut Vec<Event>, now: f64) {
        // What earlier frames held back comes first.
        let mut out = std::mem::take(&mut self.next);
        self.next = std::mem::take(&mut self.after);
        self.started_now.clear();
        // egui-winit puts a touch's pointer events right after its `Touch` event: those are
        // rebuilt here. Other pointer events come from a mouse or a hovering pen (vendor/winit)
        // and stay.
        let mut after_touch = false;
        for event in std::mem::take(events) {
            match event {
                Event::Touch { device_id, id, phase, pos, force } => {
                    after_touch = true;
                    self.touch(device_id, id.0, phase, pos, now, &mut out);
                    let ended = matches!(phase, TouchPhase::End | TouchPhase::Cancel);
                    let hidden = if ended { self.hidden.remove(&id.0) } else { self.hidden.contains(&id.0) };
                    if !hidden {
                        out.push(Event::Touch { device_id, id, phase, pos, force });
                    }
                }
                Event::PointerMoved(_) | Event::PointerButton { .. } | Event::PointerGone if after_touch => {
                    if let Event::PointerButton { modifiers, .. } = event {
                        self.modifiers = modifiers;
                    }
                }
                other => {
                    after_touch = false;
                    if let Event::Key { modifiers, .. } | Event::PointerButton { modifiers, .. } = &other {
                        self.modifiers = *modifiers;
                    }
                    out.push(other);
                }
            }
        }
        // A finger held still long enough: a right-click where it rests (the shortcut menu).
        if let Fingers::Pending { start, since, .. } = self.fingers
            && now - since >= LONG_PRESS
        {
            self.click(&mut out, start, PointerButton::Secondary);
            self.fingers = Fingers::Spent;
        }
        *events = out;
    }

    fn touch(&mut self, device: egui::TouchDeviceId, id: u64, phase: TouchPhase, pos: Pos2, now: f64, out: &mut Vec<Event>) {
        let tool = tool_of(id);
        match phase {
            TouchPhase::Start => {
                let pen_was_down = self.pen_down();
                self.down.insert(id, Down { tool, device, pos });
                if tool == Tool::Palm || (tool == Tool::Finger && pen_was_down) {
                    // Not shown to egui at all: nothing to cancel.
                    self.hidden.insert(id);
                    return;
                }
                match tool {
                    Tool::Pen | Tool::Mouse => {
                        // The pen takes over: a pan ends, and fingers stop counting.
                        let at = self.centre().unwrap_or(pos);
                        self.end_pan(out, at);
                        self.fingers = Fingers::Idle;
                        for finger in self.finger_ids() {
                            self.hide(finger, out);
                        }
                        if self.owner.is_none() {
                            self.owner = Some(id);
                            self.started_now.insert(id);
                            self.keep_pointer(out);
                            out.push(Event::PointerMoved(pos));
                            self.button(out, pos, PointerButton::Primary, true);
                        }
                    }
                    Tool::Finger => {
                        let fingers = self.finger_ids().len();
                        self.fingers = match self.fingers {
                            Fingers::Idle if fingers == 1 => Fingers::Pending { id, start: pos, since: now },
                            Fingers::Spent => Fingers::Spent,
                            Fingers::Panning { .. } => {
                                // A second finger: the pan restarts from the fingers' centre.
                                let at = self.down.get(&id).map_or(pos, |d| d.pos);
                                self.end_pan(out, at);
                                Fingers::Multi { panning: false }
                            }
                            _ => Fingers::Multi { panning: false },
                        };
                    }
                    Tool::Palm => {}
                }
            }
            TouchPhase::Move => {
                if let Some(d) = self.down.get_mut(&id) {
                    d.pos = pos;
                }
                if self.hidden.contains(&id) {
                    return;
                }
                if self.owner == Some(id) {
                    out.push(Event::PointerMoved(pos));
                    return;
                }
                if tool != Tool::Finger {
                    return;
                }
                match self.fingers {
                    Fingers::Pending { id: f, start, .. } if f == id && (pos - start).length() > SLOP => {
                        // A pan: the middle button goes down where the finger started.
                        self.keep_pointer(out);
                        out.push(Event::PointerMoved(start));
                        self.button(out, start, PointerButton::Middle, true);
                        out.push(Event::PointerMoved(pos));
                        self.fingers = Fingers::Panning { id };
                    }
                    Fingers::Panning { id: f } if f == id => out.push(Event::PointerMoved(pos)),
                    Fingers::Multi { panning } => {
                        if let Some(c) = self.centre() {
                            if !panning {
                                self.keep_pointer(out);
                                out.push(Event::PointerMoved(c));
                                self.button(out, c, PointerButton::Middle, true);
                                self.fingers = Fingers::Multi { panning: true };
                            }
                            out.push(Event::PointerMoved(c));
                        }
                    }
                    _ => {}
                }
            }
            TouchPhase::End | TouchPhase::Cancel => {
                let was_hidden = self.hidden.contains(&id);
                let centre = self.centre().unwrap_or(pos);
                self.down.remove(&id);
                if self.owner == Some(id) {
                    self.owner = None;
                    if phase == TouchPhase::End {
                        self.lift(id, pos, out);
                    } else {
                        self.button(out, pos, PointerButton::Primary, false);
                        out.push(Event::PointerGone);
                    }
                    return;
                }
                if tool != Tool::Finger || was_hidden {
                    return;
                }
                let left = self.finger_ids().len();
                match self.fingers {
                    Fingers::Pending { id: f, start, .. } if f == id => {
                        self.fingers = Fingers::Idle;
                        if phase == TouchPhase::End {
                            self.click(out, start, PointerButton::Primary);
                        }
                    }
                    Fingers::Panning { id: f } if f == id => {
                        self.end_pan(out, pos);
                        self.fingers = Fingers::Idle;
                    }
                    Fingers::Multi { .. } if left == 0 => {
                        self.end_pan(out, centre);
                        self.fingers = Fingers::Idle;
                    }
                    // One finger of a pan lifted: the centre jumps, so the pan restarts from it.
                    Fingers::Multi { panning: true } => {
                        self.end_pan(out, centre);
                        self.fingers = Fingers::Multi { panning: false };
                    }
                    Fingers::Spent if left == 0 => self.fingers = Fingers::Idle,
                    _ => {}
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn touch(id: u64, phase: TouchPhase, x: f32, y: f32) -> Event {
        Event::Touch { device_id: egui::TouchDeviceId(1), id: egui::TouchId(id), phase, pos: Pos2::new(x, y), force: None }
    }

    const PEN: u64 = 2 << 32;

    fn has(ev: &[Event], button: PointerButton, pressed: bool) -> bool {
        ev.iter().any(|e| matches!(e, Event::PointerButton { button: b, pressed: p, .. } if *b == button && *p == pressed))
    }

    #[test]
    fn a_finger_drag_pans_with_the_middle_button_and_a_tap_clicks() {
        let mut r = TouchRouter::default();
        let mut ev = vec![touch(1, TouchPhase::Start, 10.0, 10.0), touch(1, TouchPhase::Move, 10.0, 60.0)];
        r.route(&mut ev, 0.0);
        assert!(has(&ev, PointerButton::Middle, true));
        assert!(!has(&ev, PointerButton::Primary, true));
        let mut ev = vec![touch(1, TouchPhase::End, 10.0, 60.0)];
        r.route(&mut ev, 0.1);
        assert!(has(&ev, PointerButton::Middle, false));

        let mut r = TouchRouter::default();
        let mut ev = vec![touch(1, TouchPhase::Start, 10.0, 10.0), touch(1, TouchPhase::End, 10.0, 10.0)];
        r.route(&mut ev, 0.0);
        assert!(has(&ev, PointerButton::Primary, true));
        let mut ev = vec![];
        r.route(&mut ev, 0.02);
        assert!(has(&ev, PointerButton::Primary, false));
    }

    #[test]
    fn a_held_finger_right_clicks() {
        let mut r = TouchRouter::default();
        let mut ev = vec![touch(1, TouchPhase::Start, 10.0, 10.0)];
        r.route(&mut ev, 0.0);
        let mut ev = vec![];
        r.route(&mut ev, 0.6);
        assert!(has(&ev, PointerButton::Secondary, true));
        let mut ev = vec![touch(1, TouchPhase::End, 10.0, 10.0)];
        r.route(&mut ev, 0.7);
        assert!(!has(&ev, PointerButton::Primary, true));
    }

    #[test]
    fn two_fingers_pan_from_their_centre() {
        let mut r = TouchRouter::default();
        let mut ev = vec![touch(1, TouchPhase::Start, 0.0, 0.0), touch(2, TouchPhase::Start, 100.0, 0.0), touch(1, TouchPhase::Move, 0.0, 20.0)];
        r.route(&mut ev, 0.0);
        assert!(has(&ev, PointerButton::Middle, true));
        assert!(ev.iter().any(|e| matches!(e, Event::PointerMoved(p) if p.x == 50.0)));
        // egui still sees the touches, for the pinch.
        assert_eq!(ev.iter().filter(|e| matches!(e, Event::Touch { .. })).count(), 3);
    }

    #[test]
    fn the_pen_picks_and_a_palm_is_ignored() {
        let mut r = TouchRouter::default();
        let mut ev =
            vec![touch(5 << 32, TouchPhase::Start, 50.0, 50.0), touch(PEN, TouchPhase::Start, 10.0, 10.0), touch(PEN, TouchPhase::Move, 30.0, 10.0)];
        r.route(&mut ev, 0.0);
        assert!(ev.iter().any(|e| matches!(e, Event::PointerButton { pressed: true, button: PointerButton::Primary, pos, .. } if pos.x == 10.0)));
        assert!(!ev.iter().any(|e| matches!(e, Event::Touch { id, .. } if id.0 == 5 << 32)));
        assert!(!has(&ev, PointerButton::Middle, true));
    }

    #[test]
    fn copies_of_incoming_drawings_get_free_names() {
        assert_eq!(safe_file_name("../a/b:c.dwg"), "b_c.dwg");
        assert_eq!(safe_file_name(""), "Drawing.dwg");
        let dir = std::env::temp_dir().join(format!("cadcraft-android-free-name-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("P.dwg"), b"x").unwrap();
        assert_eq!(free_name(&dir, "P.dwg"), dir.join("P (2).dwg"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
