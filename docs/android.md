# CADCraft on Android (experimental)

CADCraft's desktop interface running on Android tablets, meant for use with an S Pen, a keyboard
and, if you like, a mouse. Built from the `android` branch by `.github/workflows/android.yml`
for arm64 devices running Android 11 or later, the same way as the PdfCraft and WordCraft ports
(a NativeActivity APK built without Gradle).

## Install

1. On the tablet, open the repository's **Releases** page and download `cadcraft.apk` from the
   `android-latest` prerelease.
2. Open the downloaded file. Android asks to allow installs from your browser or Files app the
   first time; allow it, then install. If Play Protect warns about an unknown app, choose
   *More details ▸ Install anyway*.
3. Give CADCraft file access. It is a special access, not one of the app's regular permissions:
   the first time you open or save a drawing, the file browser shows **Allow access…**, which
   opens the switch in Settings. Or turn it on yourself: **Settings ▸ Apps ▸ ⋮ ▸ Special access ▸
   All files access ▸ CADCraft**. Without it CADCraft can only use its own app folder.

## Updates

CADCraft updates itself from the GitHub repository it was built from. At start-up, and at most
once an hour after that, it asks GitHub for the `android-latest` release; when that is a newer
build, a card at the bottom right offers it. **Update** downloads it in the app; the first time,
Android asks you to allow CADCraft to install apps (turn on **Allow from this source**, come back
and tap **Install**); then Android's own install window asks you to confirm. CADCraft restarts on
the new build, so save your drawings first. **Later** hides the card until CADCraft next starts.
Android only installs an update signed with the same key as the installed app.

## How it differs from the desktop app

- **Drawing tabs** work as on the desktop: each drawing has a tab, and closing one leaves the
  app open.
- **Touch:** a finger drag pans the drawing (like dragging with the middle mouse button), two
  fingers pan and pinch to zoom, a finger tap clicks, and a finger held still for half a second
  right-clicks (the shortcut menu). Double-tap with the middle button isn't available by touch:
  use ZOOM Extents.
- **S Pen:** the precise pointer. It picks points and objects and drags selection windows, and a
  hovering pen moves the crosshair, so you can see where a point will land before touching.
  While the pen is on the screen, fingers and a resting palm are ignored.
- **Mouse and touchpad:** a paired mouse works as on a desktop (hover, right-click, wheel zoom,
  middle-drag pan), and so does a keyboard cover's touchpad.
- **Keyboard:** with a keyboard cover or Bluetooth keyboard attached, the command line works as
  on the desktop and the on-screen keyboard stays away; without one, it comes up when the
  command line or a text field is active.
- **Files:** Open, Save As, Export, Plot to file, Import, Attach and the other file choices use a
  file browser drawn in the app, with shortcuts to Internal storage, Documents, Download, SD
  cards and USB drives, and the app folder. **All files** shows files that aren't drawings
  (linetype files, scripts).
- **Open with:** CADCraft appears for DWG, DXF and SVG files in Files, My Files, Gmail, Drive and
  Chrome downloads. A drawing from your storage opens in place; one that only exists in another
  app (an attachment, a Drive file) is first saved as a copy in **Documents/CADCraft**.
- **Clipboard:** text and drawing copies (CADCraft copies drawings as DXF text) go on the system
  clipboard, and Ctrl+V pastes from it.
- **System bars:** the status bar and taskbar take the menu bar's colour, with icons drawn dark
  or light to stay readable in the light and dark themes.
- **Saving:** autosave is on, as on the desktop, and matters more here: Android may stop a
  CADCraft that is in the background. Drawing Recovery offers what it kept.
- **Not on Android:** printing to a printer (Plot writes PDF, PNG or SVG files), SpaceMouse, and
  the `--control` automation channel.

## Build it yourself

Needs the Android SDK and NDK, `rustup target add aarch64-linux-android` and
`cargo install cargo-ndk`:

```sh
ANDROID_HOME=~/Android/Sdk ANDROID_NDK_HOME=~/Android/Sdk/ndk/<version> packaging/android/build-apk.sh
adb install -r target/android-apk/cadcraft.apk
adb logcat -s CADCraft   # the app's log
```

The build log of every CI run is also pushed to the `ci-logs` branch (`build.log`, and the
compiler errors alone in `errors.txt`).

## Signing

The APK is signed with `packaging/android/debug.keystore` (password `android`), which is public,
like Android's own debug key. To use a private key instead, add the repository secrets
`ANDROID_KEYSTORE_B64` (the keystore, base64), `ANDROID_KEYSTORE_PASSWORD` and
`ANDROID_KEY_ALIAS`. Android won't install an update signed with a different key over an
installed app: uninstall the debug-signed one first.

## How it's put together

| Part | What it does |
|---|---|
| `apps/cadcraft-android` | The `cdylib` Android loads: `android_main` (I/O hooks, settings, autosave, GPU), `tablet.rs` (insets and bar colours, drafting-style touch routing, keyboard, clipboard sync), `browser.rs` (the file browser), `file_access.rs`, `incoming.rs`, `clipboard.rs`, `updater.rs`; the desktop's `workspace.rs` is reused as is |
| `crates/ui-egui` | Unchanged UI, plus `Services::pick_async` / `CadApp::finish_pick`: file choices a host answers on a later frame (the desktop's blocking dialogs work as before) |
| `crates/fonts`, `crates/ui-egui/src/theme.rs` | Android's font folders and its Roboto and Noto CJK fonts |
| `vendor/winit` | winit 0.30.13 with PdfCraft's Android patches (tool type per touch, mouse and touchpad as a mouse); see `vendor/README.md` |
| `packaging/android` | Manifest, the Java activity (intents, system bars, the updater's HTTP and install), signing key, `build-apk.sh` |
