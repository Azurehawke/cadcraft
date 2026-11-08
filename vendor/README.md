# Vendored crates

Permissively licensed crates carried with small patches. Each directory keeps the upstream
licence files unchanged. Remove the vendored copy once upstream releases the fix. Vendoring
copyleft code is never allowed.

| Crate | Version | Licence | Patches | From |
|---|---|---|---|---|
| winit | a517b38 (storytold fork, the revision the root Cargo.toml pins for egui) | Apache-2.0 | **Android:** (1) each touch's tool type (finger, stylus, eraser, mouse, palm) is put in bits 32..40 of `Touch::id`, so the app can tell the S Pen from a finger and ignore a resting palm; (2) a mouse, or a touchpad in pointer mode, is delivered as cursor, button and wheel events instead of touches (hover, right-click, wheel); (3) a touchpad's two-finger swipe scrolls and its pinch zooms. Each patch is marked `PdfCraft patch:` in the source. Desktop builds use the same upstream revision without these patches. | Upstream winit at the revision above, with the Android patch from PdfCraft's Android port (`Azurehawke/pdfcraft`, `android` branch) applied on top. |

The Android patches are what `apps/cadcraft-android/src/tablet.rs` relies on: without them every
touch, the S Pen's included, would look like a finger.
