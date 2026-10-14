//! CADCraft on Android, aimed at tablets with a keyboard, an S Pen and a mouse.
//!
//! Android starts the app through `CadCraftActivity`, a `NativeActivity` (packaging/android/java),
//! which loads this library and calls [`android_main`]. The UI is the desktop one
//! (`cadcraft-ui-egui`), drawing tabs included; what differs on Android:
//! - Files: Open, Save As, Export, Plot to file and the other file choices use an in-app file
//!   browser (`browser`) over shared storage, which needs "All files access" granted once in
//!   Settings (`file_access`). The UI asks for files asynchronously (`Services::pick_async`).
//! - Settings, the workspace, autosave files and startup LISP live in the app's data folder.
//! - Touch, S Pen, mouse, system bars and the keyboard cover (`tablet`); the system clipboard
//!   (`clipboard`); drawings from other apps ("Open with", `incoming`); new builds from the GitHub
//!   repository the APK was built from (`updater`).
//! - No printing to a printer (Plot writes PDF, PNG or SVG files), no SpaceMouse, no `--control`.
//!
//! The approach (NativeActivity, the patched winit, JNI for what Rust can't reach) follows
//! PdfCraft's and WordCraft's Android ports. Elsewhere the crate is empty, so `cargo build
//! --workspace` on a desktop still works.
#![cfg(target_os = "android")]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

mod browser;
mod clipboard;
mod file_access;
mod incoming;
mod tablet;
mod updater;
// The desktop app's workspace file (palettes, docking, Tool Sets), reused as is: it lives in
// CADCRAFT_CONFIG_DIR, which `android_main` points at the app's data folder.
#[path = "../../cadcraft/src/workspace.rs"]
#[allow(dead_code)]
mod workspace;

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::OnceLock;

use cadcraft_engine::Session;
use cadcraft_ui_egui::{CadApp, PickRequest, Services};
use winit::platform::android::activity::AndroidApp;

/// Shared storage as apps see it (the device's "Internal storage").
pub(crate) const SHARED: &str = "/storage/emulated/0";

/// Stencil bits of the window's render pass, as on the desktop (clipped layout viewports).
const STENCIL_BITS: u8 = 8;

/// The app's settings folder (`<app data>/cadcraft`): workspace, autosave, support, received.
static CONFIG_DIR: OnceLock<PathBuf> = OnceLock::new();

pub(crate) fn config_dir() -> Option<PathBuf> {
    CONFIG_DIR.get().cloned()
}

/// A file pick the UI asked for, for the host to show its browser (`tablet`).
pub(crate) type PickSlot = Rc<RefCell<Option<PickRequest>>>;

/// The file browser's shortcuts: the device's storage and its usual folders, SD cards and USB
/// drives that are plugged in, and CADCraft's own folder.
pub(crate) fn places() -> Vec<(String, PathBuf)> {
    let mut out = vec![
        ("Internal storage".to_string(), PathBuf::from(SHARED)),
        ("Documents".to_string(), PathBuf::from(SHARED).join("Documents")),
        ("Download".to_string(), PathBuf::from(SHARED).join("Download")),
    ];
    if let Ok(entries) = std::fs::read_dir("/storage") {
        let mut volumes: Vec<PathBuf> = entries
            .flatten()
            .filter(|e| {
                let name = e.file_name();
                let name = name.to_string_lossy();
                name != "emulated" && name != "self" && e.path().is_dir()
            })
            .map(|e| e.path())
            .collect();
        volumes.sort();
        for v in volumes {
            let name = v.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            out.push((format!("SD card or USB drive ({name})"), v));
        }
    }
    if let Some(dir) = config_dir() {
        out.push(("App folder".to_string(), dir));
    }
    out
}

/// File format hooks for the engine (it stays I/O-agnostic), as the desktop app installs them.
fn install_io() {
    cadcraft_engine::cmd::file::set_io(cadcraft_engine::cmd::file::IoHooks {
        read: |b, name| cadcraft_io::read(b, name).map_err(|e| e.to_string()),
        write: |d, name| cadcraft_io::write(d, name).map_err(|e| e.to_string()),
        plot: Some(|d, space, opts| cadcraft_io::plotter::plot(d, space, opts).map_err(|e| e.to_string())),
    });
    cadcraft_engine::cmd::file::set_write_as(|d, name, o| {
        cadcraft_io::write_with(d, name, &cadcraft_io::SaveOptions { version: o.version, binary: o.binary }).map_err(|e| e.to_string())
    });
    cadcraft_engine::cmd::file::set_entity_codec(cadcraft_engine::cmd::file::EntityCodec {
        groups: cadcraft_io::entity_groups,
        entity: cadcraft_io::entity_from_groups,
    });
}

/// The system's language (`Locale.getDefault()`, e.g. "en-US"), for the interface language.
fn system_language() -> Option<String> {
    file_access::with_activity("reading the system language", |env, _| {
        let locale = env.call_static_method("java/util/Locale", "getDefault", "()Ljava/util/Locale;", &[])?.l()?;
        let tag = env.call_method(&locale, "toLanguageTag", "()Ljava/lang/String;", &[])?.l()?;
        incoming::string_of(env, tag)
    })
    .ok()
    .flatten()
}

/// What the host gives the app on Android: an asynchronous file picker the host draws.
fn services(slot: &PickSlot) -> Services {
    let slot = Rc::clone(slot);
    Services { pick_async: Some(Box::new(move |req| *slot.borrow_mut() = Some(req))), ..Services::default() }
}

/// Entry point, called by android-activity on its own thread once the activity is created.
#[allow(unsafe_code)] // Exported by name: android-activity looks this symbol up.
#[unsafe(no_mangle)]
fn android_main(android: AndroidApp) {
    android_logger::init_once(android_logger::Config::default().with_max_level(log::LevelFilter::Info).with_tag("CADCraft"));
    std::panic::set_hook(Box::new(|info| {
        let trace = std::backtrace::Backtrace::force_capture();
        log::error!("internal error: {info}\n{trace}");
    }));

    let data = android.internal_data_path();
    if let Some(dir) = &data {
        let config = dir.join("cadcraft");
        let tmp = dir.join("tmp");
        let _ = std::fs::create_dir_all(&config);
        let _ = std::fs::create_dir_all(&tmp);
        let _ = CONFIG_DIR.set(config.clone());
        // Android sets none of these. The workspace goes to the app's private folder; `~` is
        // shared storage; temporary files go where the app may write. Nothing else runs yet: this
        // is the app's only thread besides Java's.
        #[allow(unsafe_code)]
        // SAFETY: called before eframe or the app start any thread that could read the environment.
        unsafe {
            std::env::set_var("CADCRAFT_CONFIG_DIR", &config);
            std::env::set_var("HOME", SHARED);
            std::env::set_var("TMPDIR", &tmp);
        }
        log::info!("CADCraft {} data folder {}", env!("CARGO_PKG_VERSION"), dir.display());
    }
    file_access::set_app(android.clone());
    log::info!("All files access: {}", if file_access::granted() { "granted" } else { "not granted" });
    install_io();
    let shell_android = android.clone();

    let surface = cadcraft_ui_egui::gpu::surface_config(None, false);
    let mut options = eframe::NativeOptions {
        android_app: Some(android),
        renderer: eframe::Renderer::Wgpu,
        wgpu_options: eframe::egui_wgpu::WgpuConfiguration::default().with_surface_config(surface),
        stencil_buffer: STENCIL_BITS,
        persist_window: false,
        persistence_path: data.map(|d| d.join("app.ron")),
        ..Default::default()
    };
    // Large selections' previews outgrow wgpu's default buffer cap, as on the desktop: ask for
    // what the GPU supports.
    if let eframe::egui_wgpu::WgpuSetup::CreateNew(create) = &mut options.wgpu_options.wgpu_setup {
        let base = std::sync::Arc::clone(&create.device_descriptor);
        create.device_descriptor = std::sync::Arc::new(move |adapter| {
            let mut d = base(adapter);
            d.required_limits.max_buffer_size = d.required_limits.max_buffer_size.max(adapter.limits().max_buffer_size);
            d
        });
    }
    let result = eframe::run_native(
        "CADCraft",
        options,
        Box::new(move |cc| {
            let slot = PickSlot::default();
            let mut app = CadApp::new(Session::empty(), services(&slot));
            let (workspace, restored, workspace_error) = workspace::Workspace::load();
            if let Some(ui) = restored {
                app.ui = ui;
            }
            if let Some(error) = &workspace_error {
                app.set_status(error);
            }
            app.system_languages = system_language().into_iter().collect();
            if let Some(profile) = cc.storage.and_then(|s| s.get_string(cadcraft_ui_egui::options::PROFILE_KEY)) {
                app.load_profile(&profile);
            }
            if let Some(prefs) = cc.storage.and_then(|s| s.get_string(cadcraft_ui_egui::PREFS_KEY)) {
                app.load_prefs(&prefs);
            }
            if let Some(prefs) = cc.storage.and_then(|s| s.get_string(cadcraft_ui_egui::autosave::PREFS_KEY)) {
                cadcraft_ui_egui::autosave::load_prefs(&prefs);
            }
            // Android may stop a backgrounded app without warning: autosave matters more here.
            if let Some(dir) = config_dir() {
                app.enable_autosave(&dir.join("autosave"));
                let support = dir.join("support");
                if let Err(e) = std::fs::create_dir_all(&support) {
                    log::warn!("no LISP support folder {}: {e}", support.display());
                }
                app.session.set_lisp_support_dir(Some(support));
            }
            if let Some(rs) = &cc.wgpu_render_state {
                let info = rs.adapter.get_info();
                log::info!("renderer: wgpu on {} ({:?}, {:?})", info.name, info.device_type, info.backend);
                app.set_wgpu(rs, eframe::egui_wgpu::depth_format_from_bits(0, STENCIL_BITS));
            }
            if app.session.docs.is_empty() {
                app.session.new_drawing(false);
            }
            Ok(Box::new(tablet::TabletApp::new(app, slot, workspace, shell_android)))
        }),
    );
    if let Err(e) = result {
        log::error!("CADCraft stopped: {e}");
    }
}
