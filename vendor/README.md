# Patched egui crates

`epaint`, `egui_glow` and `eframe` 0.37.0, copied from crates.io and wired in
through `[patch.crates-io]` in the root `Cargo.toml`. `epaint` and `egui_glow`
give text subpixel (LCD) anti-aliasing like the rest of the desktop (egui only
does grayscale, which looks jagged next to other apps at 1x). `eframe` keeps
the in-game overlay responsive on Wayland.

## eframe

- `native/run.rs` `MISSED_REDRAW_TIMEOUT`: 20 ms instead of 250 ms. On Wayland
  a window behind a fullscreen game gets no frame callbacks, so eframe only
  runs `App::logic` once a requested redraw is this late. Aurora runs the
  in-game Shift+Tab overlay there, so this is its response time.
- `native/glow_integration.rs` `run_viewport`: when `ViewportId::ROOT` is in
  `PassMode::LogicOnly` (e.g. occluded on another Wayland/Hyprland workspace)
  with `ActionRequested::PaintWhileHidden` queued, keep `is_visible = false`
  so ROOT's occluded Wayland surface is not cleared/swapped, set
  `show_ui = true` so `App::ui` still runs to create/update/remove deferred
  child viewports (such as the fallback Shift+Tab overlay), and drain
  `ActionRequested::PaintWhileHidden` in the `!is_visible` branch.

## epaint

- `FontData::lcd` (+ `FontData::lcd(bool)` builder): rasterize this font per
  RGB subpixel. Passed through `FaceStore::install` → `FontFace::new`.
- `text/font_face.rs` `rasterize_lcd`: renders the glyph at 3× horizontal
  resolution, applies FreeType's default LCD filter, and stores coverage as
  `(r, g, b, 0)`. Premultiplied colors never have color without alpha, so the
  renderer can recognize these texels.
- `FontColorTransferFunction::to_atlas_color` applies the transfer function to
  each channel of such texels.

## egui_glow

- Fragment shader: on desktop OpenGL ≥ 3.3, built as GLSL 330 with a second
  output for dual-source blending (`GL_ONE_MINUS_SRC1_COLOR`). LCD texels, only
  while drawing with the font atlas (`u_font_atlas`), blend per channel; every
  other pixel blends exactly as before.
- Linux only: on Windows and macOS the app keeps upstream's shader and
  grayscale text. On Linux, if dual-source blending is unavailable or that
  shader fails to build, the original shader is used and LCD texels are drawn
  as grayscale.

## Upgrading egui

Copy the new `epaint`, `egui_glow` and `eframe` from
`~/.cargo/registry/src/*/` and reapply the changes above, or drop the patch
and stop calling `ui::install_fonts_with(ctx, true)` (and accept a slower
in-game overlay on Wayland).

# SDL3 (`vendor/sdl3/`)

`SDL3.dll` (x64) from the official `libsdl-org/SDL` 3.4.18 Windows release
(`SDL3-3.4.18-win32-x64.zip`, SHA-256
`75c2c0fc74e7d1206aaedc22893a35887332f4f85f5c8b74fddc10d177985f2c`),
distributed unmodified under the zlib license (`vendor/sdl3/LICENSE.txt`,
copied as `SDL3-LICENSE.txt` in packaged builds).
