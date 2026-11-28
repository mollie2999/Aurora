//! Reusable widgets. Everything takes the [`Theme`] explicitly so plugins,
//! extensions and built-in pages all look the same.

use crate::{Theme, WidgetStyle};
use crate::theme::{bold, medium, semibold};
use std::sync::atomic::{AtomicUsize, Ordering};
use egui::{
    Align2, Color32, CornerRadius, FontFamily, FontId, Frame, Margin, Rect, Response, RichText, Sense,
    Stroke, StrokeKind, Ui, Vec2, load::{Bytes, BytesPoll, LoadError, TextureLoadResult, TexturePoll}, pos2, vec2,
};

// ---------- focus ----------

/// Give a text field keyboard focus on the frame it appears (when `want`),
/// and not again while it stays on screen.
///
/// Never call `request_focus` on every frame, or until the field reports
/// focus: each call makes egui-winit recreate the X11 input context, winit
/// answers with IME events it handles in a loop, eframe repaints from inside
/// that loop, and the field asks again. winit then never reads X events again,
/// so on X11 the window ignores input and the window manager's ping ("not
/// responding").
pub fn autofocus(r: &Response, want: bool) {
    let key = r.id.with("autofocus-seen");
    let pass = r.ctx.cumulative_pass_nr();
    let last = r.ctx.data_mut(|d| {
        let last = d.get_temp::<u64>(key);
        d.insert_temp(key, pass);
        last
    });
    // Shown on the previous pass too: it has been on screen, leave focus alone.
    let shown_before = last.is_some_and(|l| l + 1 >= pass);
    if want && !shown_before {
        r.request_focus();
    }
}

// ---------- text ----------

pub fn text(s: impl Into<String>, size: f32, color: Color32) -> RichText {
    RichText::new(s).size(size).color(color)
}
pub fn text_medium(s: impl Into<String>, size: f32, color: Color32) -> RichText {
    RichText::new(s).size(size).color(color).family(medium())
}
pub fn text_semibold(s: impl Into<String>, size: f32, color: Color32) -> RichText {
    RichText::new(s).size(size).color(color).family(semibold())
}
pub fn text_bold(s: impl Into<String>, size: f32, color: Color32) -> RichText {
    RichText::new(s).size(size).color(color).family(bold())
}

pub fn font(size: f32) -> FontId {
    FontId::new(size, FontFamily::Proportional)
}
pub fn font_medium(size: f32) -> FontId {
    FontId::new(size, medium())
}
pub fn font_semibold(size: f32) -> FontId {
    FontId::new(size, semibold())
}
pub fn font_bold(size: f32) -> FontId {
    FontId::new(size, bold())
}

/// Blend premultiplied color channels, preserving translucent endpoints.
pub fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    let l = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgba_premultiplied(l(a.r(), b.r()), l(a.g(), b.g()), l(a.b(), b.b()), l(a.a(), b.a()))
}

// ---------- painting ----------

/// Four-corner gradient fill (no rounding).
pub fn gradient(painter: &egui::Painter, rect: Rect, a: Color32, b: Color32, horizontal: bool) {
    let mut mesh = egui::Mesh::default();
    let (c0, c1, c2, c3) = if horizontal { (a, b, b, a) } else { (a, a, b, b) };
    mesh.colored_vertex(rect.left_top(), c0);
    mesh.colored_vertex(rect.right_top(), c1);
    mesh.colored_vertex(rect.right_bottom(), c2);
    mesh.colored_vertex(rect.left_bottom(), c3);
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(0, 2, 3);
    painter.add(mesh);
}

/// Fade `rect` from transparent (top) to `color` (bottom), starting at `from` (0..1).
pub fn fade_down(ui: &Ui, rect: Rect, color: Color32, from: f32) {
    let top = rect.top() + rect.height() * from;
    let r = Rect::from_min_max(pos2(rect.left(), top), rect.max);
    gradient(&ui.painter_at(rect), r, Color32::TRANSPARENT, color, false);
}

/// Fade from `color` (left) to transparent at `to` (0..1).
pub fn fade_right(ui: &Ui, rect: Rect, color: Color32, to: f32) {
    let r = Rect::from_min_max(rect.min, pos2(rect.left() + rect.width() * to, rect.bottom()));
    gradient(&ui.painter_at(rect), r, color, Color32::TRANSPARENT, true);
}

/// A 256-step alpha ramp (opaque white -> transparent) along x or y,
/// made once per context. Clamped at the edges, so uv outside 0..1 holds
/// the end values.
fn ramp(ctx: &egui::Context, vertical: bool) -> egui::TextureId {
    let id = egui::Id::unique(("ui-fade-ramp", vertical));
    if let Some(tex) = ctx.data(|d| d.get_temp::<egui::TextureHandle>(id)) {
        return tex.id();
    }
    let pixels: Vec<Color32> = (0..256u32).map(|i| { let a = (255 - i) as u8; Color32::from_rgba_premultiplied(a, a, a, a) }).collect();
    let size = if vertical { [1, 256] } else { [256, 1] };
    let tex = ctx.load_texture("fade-ramp", egui::ColorImage::new(size, pixels), egui::TextureOptions::LINEAR);
    let tid = tex.id();
    ctx.data_mut(|d| d.insert_temp(id, tex));
    tid
}

/// [`fade_right`] that keeps `rect`'s rounded corners.
pub fn fade_right_rounded(ui: &Ui, rect: Rect, color: Color32, to: f32, radius: CornerRadius) {
    // u = 0 at the left edge, 1 at `to`; beyond that the ramp stays clear.
    let uv = Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0 / to.max(0.01), 1.0));
    let shape = egui::epaint::RectShape::filled(rect, radius, color).with_texture(ramp(ui.ctx(), false), uv);
    ui.painter().add(shape);
}

/// [`fade_down`] that keeps `rect`'s rounded corners.
pub fn fade_down_rounded(ui: &Ui, rect: Rect, color: Color32, from: f32, radius: CornerRadius) {
    // The ramp runs opaque -> clear, so it is read bottom-up: v = 0 at the
    // bottom edge, 1 at `from`, clear above.
    let span = (1.0 - from).max(0.01);
    let uv = Rect::from_min_max(pos2(0.0, 1.0 / span), pos2(1.0, 0.0));
    let shape = egui::epaint::RectShape::filled(rect, radius, color).with_texture(ramp(ui.ctx(), true), uv);
    ui.painter().add(shape);
}

/// How a remote image is doing.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ImageState {
    Loading,
    Ready,
    /// Failed for good (after a few retries), e.g. the image doesn't exist.
    Missing,
}

/// Pixel sizes still pictures are decoded at (the app's sized image
/// loader): a few steps, so a picture drawn at slightly different sizes
/// shares one texture.
const STEPS: [u32; 11] = [64, 96, 128, 192, 256, 384, 512, 768, 1024, 1536, 2048];
const SIZED: &str = "#sir-fit=";
pub const BLUR_BOTTOM_SUFFIX: &str = ",blur-bottom";
/// A sized picture blurred all over (a "frosted glass" backdrop).
pub const BLUR_SUFFIX: &str = ",blur";

fn step(px: f32) -> Option<u32> {
    STEPS.iter().copied().find(|s| *s as f32 >= px.ceil())
}

/// `url` with the size it's drawn at (`px`, in pixels), so the picture is
/// decoded at that size instead of in full: `<url>#sir-fit=<cover|contain>,<w>x<h>`.
/// Animations, unknown formats and very large draws keep the plain URL.
pub fn sized_url(url: &str, px: Vec2, cover: bool) -> String {
    let path = url.split(['?', '#']).next().unwrap_or(url).as_bytes();
    let still = [b".jpg".as_slice(), b".jpeg", b".png", b".webp"].iter().any(|extension| {
        path.len() >= extension.len() && path[path.len() - extension.len()..].eq_ignore_ascii_case(extension)
    });
    match (still, step(px.x), step(px.y)) {
        (true, Some(w), Some(h)) if url.starts_with("http") && !url.contains('#') => format!("{url}{SIZED}{},{w}x{h}", if cover { "cover" } else { "contain" }),
        _ => url.to_string(),
    }
}

/// The parts of a [`sized_url`]: (plain URL, cover, width, height).
pub const USE_ORIGINAL_IMAGE: &str = "load the original image for animation";

pub fn parse_sized(uri: &str) -> Option<(&str, bool, u32, u32)> {
    let (url, spec) = uri.split_once(SIZED)?;
    let (mode, size) = spec.split_once(',')?;
    let (w, h) = size.split_once('x')?;
    Some((url, mode == "cover", w.parse().ok()?, h.parse().ok()?))
}

/// Plain URLs known missing for good, and ones asked for (see [`image_state`]).
static SEEN: std::sync::Mutex<Option<std::collections::HashMap<String, ImageState>>> = std::sync::Mutex::new(None);

fn note_state(url: &str, state: ImageState) {
    let mut guard = SEEN.lock().unwrap();
    let map = guard.get_or_insert_with(Default::default);
    if map.get(url) != Some(&state) {
        if map.len() >= IMAGE_HISTORY_LIMIT && !map.contains_key(url) {
            map.clear();
        }
        map.insert(url.to_string(), state);
    }
}

/// Failed loads: url -> (attempts, when the last one failed).
static FAILED: std::sync::Mutex<Option<std::collections::HashMap<String, (u32, f64)>>> = std::sync::Mutex::new(None);
const RETRIES: u32 = 3;
const IMAGE_HISTORY_LIMIT: usize = 4096;

/// Load an image, retrying failed downloads a few times with a pause.
/// Steam's image servers sometimes drop a connection when many images load
/// at once; without this a dropped one would stay blank.
fn load(ui: &Ui, url: &str, size: Vec2, cover: bool) -> (ImageState, Option<egui::load::SizedTexture>) {
    let (state, texture) = load_sized(ui, url, size, cover);
    if !url.is_empty() {
        note_state(url, state);
    }
    (state, texture)
}

fn load_sized(ui: &Ui, url: &str, size: Vec2, cover: bool) -> (ImageState, Option<egui::load::SizedTexture>) {
    if url.is_empty() {
        return (ImageState::Missing, None);
    }
    let uri = sized_url(url, size * ui.ctx().pixels_per_point(), cover);
    load_uri(ui, url, &uri, size)
}

fn load_uri(ui: &Ui, url: &str, uri: &str, size: Vec2) -> (ImageState, Option<egui::load::SizedTexture>) {
    let result = if uri == url {
        egui::Image::new(url).load_for_size(ui.ctx(), size)
    } else {
        // The sized decoder must see its URI intact, including for WebP.
        ui.ctx().try_load_texture(uri, egui::TextureOptions::LINEAR, egui::load::SizeHint::default())
    };
    let result = match result {
        Err(egui::load::LoadError::Loading(reason)) if reason == USE_ORIGINAL_IMAGE => {
            egui::Image::new(url).load_for_size(ui.ctx(), size)
        }
        other => other,
    };
    match result {
        Ok(TexturePoll::Ready { texture }) => (ImageState::Ready, Some(texture)),
        Ok(TexturePoll::Pending { .. }) => (ImageState::Loading, None),
        // The picture doesn't exist (404 and friends): no point retrying,
        // and a fallback image can load right away.
        Err(egui::load::LoadError::Loading(e)) if e.starts_with("HTTP 4") => (ImageState::Missing, None),
        Err(_) => {
            let now = ui.input(|i| i.time);
            let mut guard = FAILED.lock().unwrap();
            let map = guard.get_or_insert_with(Default::default);
            if map.len() >= IMAGE_HISTORY_LIMIT && !map.contains_key(url) {
                map.clear();
            }
            let entry = map.entry(url.to_string()).or_insert((0, now));
            if entry.0 >= RETRIES {
                return (ImageState::Missing, None);
            }
            // Wait 2 s, 4 s, 6 s between attempts.
            if now - entry.1 > 2.0 * (entry.0 + 1) as f64 {
                entry.0 += 1;
                entry.1 = now;
                ui.ctx().forget_image(url);
                if uri != url {
                    ui.ctx().forget_image(uri);
                }
            }
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(500));
            (ImageState::Loading, None)
        }
    }
}

/// Whether an image loaded, is loading, or is missing. For a picture drawn
/// before, what that draw found (no extra texture at another size).
pub fn image_state(ui: &Ui, url: &str) -> ImageState {
    if let Some(state) = SEEN.lock().unwrap().as_ref().and_then(|m| m.get(url).copied()) {
        return state;
    }
    load(ui, url, Vec2::splat(64.0), false).0
}

/// Draw a remote image covering `rect` (center-cropped). False while loading.
pub fn paint_cover(ui: &Ui, rect: Rect, url: &str, radius: CornerRadius) -> bool {
    paint_cover_state(ui, rect, url, radius) == ImageState::Ready
}

fn paint_cover_state(ui: &Ui, rect: Rect, url: &str, radius: CornerRadius) -> ImageState {
    let (state, texture) = load(ui, url, rect.size(), true);
    if let Some(texture) = texture {
        paint_cover_texture(ui, rect, texture, radius);
    }
    state
}

fn paint_cover_texture(ui: &Ui, rect: Rect, texture: egui::load::SizedTexture, radius: CornerRadius) {
    let tex = texture.size;
    let (ta, ra) = (tex.x / tex.y, rect.width() / rect.height());
    let uv = if ta > ra {
        let w = ra / ta;
        Rect::from_min_max(pos2((1.0 - w) / 2.0, 0.0), pos2((1.0 + w) / 2.0, 1.0))
    } else {
        let h = ta / ra;
        Rect::from_min_max(pos2(0.0, (1.0 - h) / 2.0), pos2(1.0, (1.0 + h) / 2.0))
    };
    egui::Image::from_texture(texture).uv(uv).corner_radius(radius).paint_at(ui, rect);
}

/// Draw the first of `urls` that exists, covering `rect`. A later URL is only
/// fetched once the earlier ones failed for good, not while they're still
/// loading, so a fallback never competes with the image it stands in for.
pub fn paint_cover_first(ui: &Ui, rect: Rect, urls: &[&str], radius: CornerRadius) -> bool {
    for url in urls {
        match load(ui, url, rect.size(), true) {
            (_, Some(texture)) => {
                paint_cover_texture(ui, rect, texture, radius);
                return true;
            }
            (ImageState::Missing, _) => continue,
            _ => return false,
        }
    }
    false
}

/// Draw one cover with blur increasing toward the bottom. Return the sized
/// variant so the caller can release it when leaving the game page.
pub fn paint_blurred_cover_first(ui: &Ui, screen: Rect, rect: Rect, urls: &[&str]) -> Option<String> {
    let px = screen.size() * ui.ctx().pixels_per_point();
    // A viewport resize or zoom should not produce a new texture every frame.
    let px = px * (1536.0 / px.x.max(px.y).max(1.0)).min(1.0);
    for url in urls.iter().copied().filter(|url| !url.is_empty()) {
        let mut uri = sized_url(url, px, true);
        if uri != url {
            uri.push_str(BLUR_BOTTOM_SUFFIX);
        }
        match load_uri(ui, url, &uri, screen.size()) {
            (_, Some(texture)) => {
                paint_cover_texture(ui, rect, texture, CornerRadius::ZERO);
                return (uri != url).then_some(uri);
            }
            (ImageState::Missing, _) => continue,
            _ => return (uri != url).then_some(uri),
        }
    }
    None
}

/// A Steam profile background: drawn at its own size (CSS pixels = points),
/// centered on `center_x` from `top`, like Steam's `center top no-repeat`;
/// or `fill_width` wide when the profile uses "Full Screen". Returns where
/// it was drawn.
pub fn paint_page_background(ui: &Ui, center_x: f32, top: f32, url: &str, fill_width: Option<f32>) -> Option<Rect> {
    // Asked for at up to 4K so the decoder keeps the picture's own size.
    let (_, texture) = load(ui, url, vec2(3840.0, 2160.0) / ui.ctx().pixels_per_point(), false);
    let texture = texture?;
    let size = match fill_width {
        Some(w) => vec2(w, w * texture.size.y / texture.size.x.max(1.0)),
        None => texture.size,
    };
    let rect = Rect::from_min_size(pos2(center_x - size.x / 2.0, top), size);
    egui::Image::from_texture(texture).paint_at(ui, rect);
    Some(rect)
}

/// A blurred copy of a [`paint_page_background`] picture drawn at `rect`,
/// cut to `area`: Steam's frosted content on game profiles. A shape, so it
/// can go under content drawn before it. `None` while it's being made.
pub fn blurred_background_shape(ui: &Ui, rect: Rect, area: Rect, url: &str) -> Option<egui::Shape> {
    let part = rect.intersect(area);
    if !part.is_positive() {
        return None;
    }
    // Blurring a smaller copy is cheaper and looks the same.
    let px = (rect.size() * ui.ctx().pixels_per_point() * 0.5).max(vec2(16.0, 16.0));
    let mut uri = sized_url(url, px, false);
    if uri == url {
        return None;
    }
    uri.push_str(BLUR_SUFFIX);
    let Ok(TexturePoll::Ready { texture }) = ui.ctx().try_load_texture(&uri, egui::TextureOptions::LINEAR, egui::load::SizeHint::default()) else {
        return None;
    };
    let uv = |p: egui::Pos2| pos2((p.x - rect.left()) / rect.width(), (p.y - rect.top()) / rect.height());
    Some(egui::Shape::image(texture.id, part, Rect::from_min_max(uv(part.min), uv(part.max)), Color32::WHITE))
}

/// Start loading the image [`paint_cover_first`] would show, without drawing.
pub fn prefetch_first(ui: &Ui, urls: &[&str], size: Vec2) {
    for url in urls {
        if load(ui, url, size, true).0 != ImageState::Missing {
            return;
        }
    }
}

/// Draw a remote image as large as fits in `rect`, anchored by `align`.
pub fn paint_contain(ui: &Ui, rect: Rect, url: &str, align: Align2) -> bool {
    let (_, Some(texture)) = load(ui, url, rect.size(), false) else {
        return false;
    };
    paint_contain_texture(ui, rect, texture, align);
    true
}

fn paint_contain_texture(ui: &Ui, rect: Rect, texture: egui::load::SizedTexture, align: Align2) {
    let scale = (rect.width() / texture.size.x).min(rect.height() / texture.size.y);
    let target = align.align_size_within_rect(texture.size * scale, rect);
    egui::Image::from_texture(texture).paint_at(ui, target);
}

#[cfg(test)]
fn decode_test_gif(bytes: &[u8]) -> Option<(Vec<egui::ColorImage>, Vec<u32>)> {
    decode_animation(bytes, |_, _| {})
}

// ---------- animated pictures (avatars, avatar frames) ----------

/// Decoded APNG frames: Steam's animated avatar frames are APNGs, which
/// egui's loaders show as a still picture.
enum Apng {
    Decoding,
    Failed,
    Decoded(Vec<egui::ColorImage>, Vec<u32>, bool),
    Ready(std::sync::Arc<Vec<egui::TextureHandle>>, std::sync::Arc<Vec<u32>>, bool),
}

static APNGS: std::sync::Mutex<Option<std::collections::HashMap<String, Apng>>> = std::sync::Mutex::new(None);
/// Animations kept decoded at once.
const APNG_CACHE: usize = 24;
/// Frames kept per animation (longer ones skip frames) and their largest side.
const APNG_MAX_FRAMES: usize = 300;
const APNG_MAX_SIDE: u32 = 256;

const MAX_ANIMATION_DIMENSION: u32 = 4096;
const MAX_ANIMATION_ALLOC: u64 = 64 << 20;
const MAX_ANIMATION_DECODERS: usize = 4;
static ACTIVE_ANIMATION_DECODERS: AtomicUsize = AtomicUsize::new(0);

struct AnimationDecodeSlot;

impl AnimationDecodeSlot {
    fn acquire() -> Option<Self> {
        let slot = Self;
        (ACTIVE_ANIMATION_DECODERS.fetch_add(1, Ordering::Relaxed) < MAX_ANIMATION_DECODERS).then_some(slot)
    }
}

impl Drop for AnimationDecodeSlot {
    fn drop(&mut self) {
        ACTIVE_ANIMATION_DECODERS.fetch_sub(1, Ordering::Relaxed);
    }
}

fn is_apng(bytes: &[u8]) -> bool {
    let Some(mut chunks) = bytes.strip_prefix(b"\x89PNG\r\n\x1a\n") else { return false };
    while let Some(header) = chunks.get(..8) {
        match &header[4..] {
            b"acTL" => return true,
            b"IDAT" | b"IEND" => return false,
            _ => {}
        }
        let size = u32::from_be_bytes(header[..4].try_into().unwrap()) as usize;
        let Some(rest) = size.checked_add(12).and_then(|end| chunks.get(end..)) else { return false };
        chunks = rest;
    }
    false
}

fn is_animated_webp(bytes: &[u8]) -> bool {
    bytes.starts_with(b"RIFF")
        && bytes.get(8..16) == Some(b"WEBPVP8X")
        && bytes.get(20).is_some_and(|flags| flags & 0x02 != 0)
}

fn is_animation(bytes: &[u8]) -> bool {
    bytes.starts_with(b"GIF8") || is_animated_webp(bytes) || is_apng(bytes)
}

/// The chunk tag, payload and next chunk in a WebP RIFF stream.
fn webp_chunk(bytes: &[u8]) -> Option<(&[u8], &[u8], &[u8])> {
    let size = u32::from_le_bytes(bytes.get(4..8)?.try_into().ok()?) as usize;
    let (payload, rest) = bytes.get(8..)?.split_at_checked(size)?;
    Some((&bytes[..4], payload, rest.get(size % 2..)?))
}

/// The WebP decoder allocates lossy frame planes from the inner VP8
/// dimensions before checking them against the animation frame header.
fn webp_frame_dimensions_match(bytes: &[u8]) -> Option<()> {
    let mut chunks = bytes.get(12..)?;
    while chunks.len() >= 8 {
        let (tag, payload, rest) = webp_chunk(chunks)?;
        chunks = rest;
        if tag != b"ANMF" {
            continue;
        }
        // A frame header and its padded subchunks always have even length.
        if payload.len() % 2 != 0 {
            return None;
        }
        let (tag, frame, rest) = webp_chunk(payload.get(16..)?)?;
        let (tag, frame) = if tag == b"ALPH" {
            let (tag, frame, _) = webp_chunk(rest)?;
            (tag, frame)
        } else {
            (tag, frame)
        };
        if tag == b"VP8 " {
            let frame_size = |at| u32::from_le_bytes([payload[at], payload[at + 1], payload[at + 2], 0]) + 1;
            let header = frame.get(6..10)?;
            let vp8_size = |at| u32::from(u16::from_le_bytes([header[at], header[at + 1]]) & 0x3fff);
            if frame_size(6) != vp8_size(0) || frame_size(9) != vp8_size(2) {
                return None;
            }
        }
    }
    Some(())
}

#[cfg(test)]
fn decode_apng(bytes: &[u8]) -> Option<(Vec<egui::ColorImage>, Vec<u32>)> {
    decode_animation(bytes, |_, _| {})
}

fn decode_animation(bytes: &[u8], mut preview: impl FnMut(&[egui::ColorImage], &[u32])) -> Option<(Vec<egui::ColorImage>, Vec<u32>)> {
    use image::{AnimationDecoder, ImageDecoder};
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_ANIMATION_DIMENSION);
    limits.max_image_height = Some(MAX_ANIMATION_DIMENSION);
    limits.max_alloc = Some(MAX_ANIMATION_ALLOC);
    let cursor = std::io::Cursor::new(bytes);
    let frames = if bytes.starts_with(b"GIF8") {
        let mut decoder = image::codecs::gif::GifDecoder::new(cursor).ok()?;
        decoder.set_limits(limits).ok()?;
        decoder.into_frames()
    } else if is_animated_webp(bytes) {
        let mut decoder = image::codecs::webp::WebPDecoder::new(cursor).ok()?;
        decoder.set_limits(limits.clone()).ok()?;
        let (width, height) = decoder.dimensions();
        // WebP's set_limits only checks dimensions. Budget the frame, the
        // accumulated canvas, the output, and decoder scratch at RGBA size.
        limits.reserve(u64::from(width) * u64::from(height) * 16).ok()?;
        webp_frame_dimensions_match(bytes)?;
        decoder.into_frames()
    } else {
        image::codecs::png::PngDecoder::with_limits(cursor, limits).ok()?.apng().ok()?.into_frames()
    };
    let mut images = Vec::new();
    let mut delays: Vec<u32> = Vec::new();
    // Keep the whole timeline. If an exceptionally long animation exceeds
    // the frame budget, merge adjacent frames and retain their total delay.
    let mut stride = 1usize;
    let mut seen = 0usize;
    for frame in frames {
        // Some Steam uploads have malformed trailing metadata after valid
        // frames. Preserve the frames already decoded, as browsers do.
        let Ok(frame) = frame else { break };
        let (n, d) = frame.delay().numer_denom_ms();
        let delay = (n / d.max(1)).max(1);
        if seen % stride != 0 {
            if let Some(last) = delays.last_mut() { *last = last.saturating_add(delay); }
            seen += 1;
            if seen % 24 == 0 { preview(&images, &delays); }
            continue;
        }
        seen += 1;
        if images.len() == APNG_MAX_FRAMES {
            let mut compact_images = Vec::with_capacity(APNG_MAX_FRAMES / 2);
            let mut compact_delays: Vec<u32> = Vec::with_capacity(APNG_MAX_FRAMES / 2);
            for (i, (image, delay)) in images.drain(..).zip(delays.drain(..)).enumerate() {
                if i % 2 == 0 { compact_images.push(image); compact_delays.push(delay); }
                else if let Some(last) = compact_delays.last_mut() { *last = last.saturating_add(delay); }
            }
            images = compact_images;
            delays = compact_delays;
            stride *= 2;
        }
        let mut buf = frame.into_buffer();
        let (w, h) = buf.dimensions();
        if w.max(h) > APNG_MAX_SIDE {
            let k = APNG_MAX_SIDE as f32 / w.max(h) as f32;
            buf = image::imageops::resize(&buf, ((w as f32 * k) as u32).max(1), ((h as f32 * k) as u32).max(1), image::imageops::FilterType::Triangle);
        }
        images.push(egui::ColorImage::from_rgba_unmultiplied([buf.width() as usize, buf.height() as usize], buf.as_raw()));
        delays.push(delay);
        // Start playing while the rest of a long animation is decoded.
        if seen % 24 == 0 { preview(&images, &delays); }
    }
    (!images.is_empty()).then_some((images, delays))
}

/// The frame to show at `time` (seconds) and how long until the next one.
fn frame_at(delays: &[u32], time: f64) -> (usize, u64) {
    let total: u64 = delays.iter().map(|d| *d as u64).sum::<u64>().max(1);
    let now = (time * 1000.0) as u64 % total;
    let mut at = 0u64;
    for (i, d) in delays.iter().enumerate() {
        at += *d as u64;
        if now < at {
            return (i, at - now);
        }
    }
    (0, delays.first().copied().unwrap_or(100) as u64)
}

/// The current frame of an animation, decoding it in the background.
fn animation_texture(ctx: &egui::Context, url: &str, bytes: Bytes, animate: bool, max_fps: f32) -> TextureLoadResult {
    let mut guard = APNGS.lock().unwrap();
    let map = guard.get_or_insert_with(Default::default);
    if !map.contains_key(url) {
        let Some(slot) = AnimationDecodeSlot::acquire() else {
            ctx.request_repaint_after(std::time::Duration::from_millis(16));
            return Ok(TexturePoll::Pending { size: None });
        };
        if map.len() >= APNG_CACHE {
            // Active decoders also own their preview entries until they finish.
            map.retain(|_, entry| matches!(entry, Apng::Decoding | Apng::Decoded(_, _, false) | Apng::Ready(_, _, false)));
        }
        let worker_ctx = ctx.clone();
        let worker_url = url.to_string();
        std::thread::Builder::new().spawn(move || {
            let result = decode_animation(&bytes, |images, delays| {
                if let Some(map) = APNGS.lock().unwrap().as_mut() {
                    map.insert(worker_url.clone(), Apng::Decoded(images.to_vec(), delays.to_vec(), false));
                }
                worker_ctx.request_repaint();
            });
            if let Some(map) = APNGS.lock().unwrap().as_mut() {
                map.insert(worker_url, match result {
                    Some((images, delays)) => Apng::Decoded(images, delays, true),
                    None => Apng::Failed,
                });
            }
            drop(slot);
            worker_ctx.request_repaint();
        }).map_err(|err| LoadError::Loading(err.to_string()))?;
        map.insert(url.to_string(), Apng::Decoding);
        return Ok(TexturePoll::Pending { size: None });
    }
    let entry = map.get_mut(url).unwrap();
    if matches!(entry, Apng::Failed) {
        return Err(LoadError::Loading("Could not decode animation".into()));
    }
    if let Apng::Decoded(images, delays, complete) = entry {
        let textures: Vec<_> = images.drain(..).enumerate().map(|(i, img)| ctx.load_texture(format!("{url}#apng{i}"), img, egui::TextureOptions::LINEAR)).collect();
        *entry = Apng::Ready(std::sync::Arc::new(textures), std::sync::Arc::new(std::mem::take(delays)), *complete);
    }
    let Apng::Ready(textures, delays, complete) = entry else { return Ok(TexturePoll::Pending { size: None }) };
    let index = if animate {
        let now = ctx.input(|i| i.time);
        let clock_id = egui::Id::unique(("animation-clock", url));
        let total = delays.iter().map(|d| *d as f64).sum::<f64>() / 1000.0;
        let elapsed = ctx.data_mut(|data| {
            let (previous, elapsed) = data.get_temp::<(f64, f64)>(clock_id).unwrap_or((now, 0.0));
            let mut elapsed = elapsed + (now - previous).clamp(0.0, 0.25);
            if !*complete { elapsed = elapsed.min((total - 0.001).max(0.0)); }
            data.insert_temp(clock_id, (now, elapsed));
            elapsed
        });
        let (i, wait) = frame_at(delays, elapsed);
        let floor = (1000.0 / max_fps.max(1.0)) as u64;
        ctx.request_repaint_after(std::time::Duration::from_millis(wait.max(floor)));
        i
    } else {
        0
    };
    Ok(TexturePoll::Ready { texture: egui::load::SizedTexture::from_handle(&textures[index]) })
}

/// Like [`paint_cover`], but animated GIFs, WebPs and APNGs play, at most
/// `max_fps` frames a second and only while `rect` is on screen.
pub fn paint_cover_animated(ui: &Ui, rect: Rect, url: &str, radius: CornerRadius, max_fps: f32) -> bool {
    let ctx = ui.ctx();
    match ctx.try_load_bytes(url) {
        Ok(BytesPoll::Ready { bytes, .. }) if is_animation(&bytes) => {
            if let Ok(TexturePoll::Ready { texture }) = animation_texture(ctx, url, bytes, ui.is_rect_visible(rect), max_fps) {
                paint_cover_texture(ui, rect, texture, radius);
                return true;
            }
            false
        }
        Ok(BytesPoll::Pending { .. }) => false,
        _ => paint_cover(ui, rect, url, radius),
    }
}

/// Like [`paint_contain`], with animated GIF/WebP/APNG playback while visible.
pub fn paint_contain_animated(ui: &Ui, rect: Rect, url: &str, align: Align2, max_fps: f32) -> bool {
    let ctx = ui.ctx();
    match ctx.try_load_bytes(url) {
        Ok(BytesPoll::Ready { bytes, .. }) if is_animation(&bytes) => {
            if let Ok(TexturePoll::Ready { texture }) = animation_texture(ctx, url, bytes, ui.is_rect_visible(rect), max_fps) {
                paint_contain_texture(ui, rect, texture, align);
                return true;
            }
            false
        }
        Ok(BytesPoll::Pending { .. }) => false,
        _ => paint_contain(ui, rect, url, align),
    }
}

/// A remote picture `width` wide at its own shape (no cropping), at most
/// `max_height` tall. Animations play at most `max_fps` frames a second and
/// only while on screen. Shows a 16:9 placeholder until it loads.
pub fn picture_fit_width(ui: &mut Ui, t: &Theme, url: &str, width: f32, max_height: f32, max_fps: f32) -> Response {
    let ctx = ui.ctx();
    let original = parse_sized(url).map_or(url, |(original, _, _, _)| original);
    let ahead = Rect::from_min_size(ui.cursor().min, vec2(width, max_height));
    let texture = match ctx.try_load_bytes(original) {
        Ok(BytesPoll::Ready { bytes, .. }) if is_animation(&bytes) => {
            animation_texture(ctx, original, bytes, ui.is_rect_visible(ahead), max_fps)
        }
        Ok(BytesPoll::Pending { size }) => Ok(TexturePoll::Pending { size }),
        // Recognized animations always stay on the bounded decoder path.
        _ => egui::Image::new(url).load_for_size(ctx, vec2(width, max_height)),
    };
    match texture {
        Ok(egui::load::TexturePoll::Ready { texture }) if texture.size.x > 0.0 => {
            let mut size = vec2(width, width * texture.size.y / texture.size.x);
            if size.y > max_height {
                size = vec2(max_height * texture.size.x / texture.size.y, max_height);
            }
            let (outer, resp) = ui.allocate_exact_size(vec2(width, size.y), Sense::click());
            let rect = Align2::CENTER_TOP.align_size_within_rect(size, outer);
            egui::Image::from_texture(texture).paint_at(ui, rect);
            resp
        }
        Err(_) => {
            let (rect, resp) = ui.allocate_exact_size(vec2(width, width * 9.0 / 16.0), Sense::click());
            placeholder(ui, t, rect, "", CornerRadius::ZERO);
            resp
        }
        _ => {
            let (rect, resp) = ui.allocate_exact_size(vec2(width, width * 9.0 / 16.0), Sense::click());
            skeleton(ui, t, rect, CornerRadius::ZERO);
            resp
        }
    }
}

/// Animated loading placeholder.
pub fn skeleton(ui: &Ui, t: &Theme, rect: Rect, radius: CornerRadius) {
    let time = ui.input(|i| i.time) as f32;
    let k = (time * 2.2 + rect.left() * 0.004).sin() * 0.5 + 0.5;
    ui.painter().rect_filled(rect, radius, mix(t.surface, t.surface_hover, k));
    ui.ctx().request_repaint_after(std::time::Duration::from_millis(33));
}

/// Placeholder with a title, for apps that have no art.
pub fn placeholder(ui: &Ui, t: &Theme, rect: Rect, title: &str, radius: CornerRadius) {
    let p = ui.painter();
    p.rect_filled(rect, radius, t.surface_hover);
    if t.widget_style != WidgetStyle::Adwaita {
        gradient(&ui.painter_at(rect.shrink(1.0)), rect.shrink(1.0), t.accent.gamma_multiply(0.10), t.accent_2.gamma_multiply(0.05), false);
    }
    let galley = p.layout(title.to_string(), font_semibold(15.0), t.text_dim, rect.width() - 24.0);
    p.galley(rect.center() - galley.size() / 2.0, galley, t.text_dim);
}

/// Image if loaded, shimmer while loading, a plain tile if it's missing.
pub fn image_or_skeleton(ui: &Ui, t: &Theme, rect: Rect, url: &str, radius: CornerRadius) {
    image_or_title(ui, t, rect, url, "", radius);
}

/// Image if loaded; otherwise a titled placeholder once it's known missing.
pub fn image_or_title(ui: &Ui, t: &Theme, rect: Rect, url: &str, title: &str, radius: CornerRadius) {
    match paint_cover_state(ui, rect, url, radius) {
        ImageState::Ready => {}
        ImageState::Loading => skeleton(ui, t, rect, radius),
        ImageState::Missing => placeholder(ui, t, rect, title, radius),
    }
}

// ---------- containers ----------

/// A dialog (egui Modal) with room around its content: 28 px padding and
/// the large corner radius, instead of the tight popup-menu frame.
pub fn modal(ctx: &egui::Context, id: egui::Id) -> egui::Modal {
    let style = ctx.global_style();
    // The page color, not the control color, so buttons inside stand out.
    let frame = Frame::popup(&style)
        .fill(style.visuals.panel_fill)
        .inner_margin(Margin::same(28))
        .corner_radius(CornerRadius::same(style.visuals.window_corner_radius.nw.max(10)));
    egui::Modal::new(id).frame(frame)
}


pub fn card(t: &Theme) -> Frame {
    Frame::new()
        .fill(t.surface)
        .corner_radius(t.corner())
        .inner_margin(Margin::same(18))
}

pub fn panel(t: &Theme) -> Frame {
    Frame::new().fill(t.raised).corner_radius(t.corner_small()).inner_margin(Margin::same(12))
}

// ---------- buttons ----------

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Primary,
    Secondary,
    Ghost,
    Play,
    Danger,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Size {
    Sm,
    Md,
    Lg,
}

/// Adwaita uses a 34 px minimum target even for compact controls.
fn control_height(t: &Theme, height: f32) -> f32 {
    if t.widget_style == WidgetStyle::Adwaita { height.max(t.native_metrics.map(|m| m.min_height).unwrap_or(34.0)) } else { height }
}

fn focus_ring(ui: &Ui, t: &Theme, response: &Response, rect: Rect) {
    if t.widget_style == WidgetStyle::Adwaita && response.has_focus() && ui.is_enabled() {
        ui.painter().rect_stroke(rect.expand(2.0), t.corner_small(), Stroke::new(2.0, t.accent), StrokeKind::Outside);
    }
}

/// The one button. `icon` is a glyph from [`crate::icons`].
pub fn button(ui: &mut Ui, t: &Theme, icon: Option<&str>, label: &str, kind: Kind, size: Size) -> Response {
    let (h, fs, pad) = match size {
        Size::Sm => (28.0, 13.0, 12.0),
        Size::Md => (34.0, 13.5, 16.0),
        Size::Lg => (44.0, 15.0, 22.0),
    };
    let h = control_height(t, h);
    let pad = t.native_metrics.map(|m| m.padding[1].max(m.padding[3])).unwrap_or(pad);
    let text = match icon {
        Some(i) if !label.is_empty() => format!("{i}  {label}"),
        Some(i) => i.to_string(),
        None => label.to_string(),
    };
    let (bg, fg, border) = match kind {
        Kind::Primary => (t.accent, t.on_accent, Color32::TRANSPARENT),
        Kind::Play => (t.play, Color32::WHITE, Color32::TRANSPARENT),
        Kind::Danger => (t.danger.gamma_multiply(0.16), t.danger, Color32::TRANSPARENT),
        Kind::Secondary => (if t.widget_style == WidgetStyle::Adwaita { t.chrome } else { t.raised }, t.text, Color32::TRANSPARENT),
        Kind::Ghost => (Color32::TRANSPARENT, t.text_dim, Color32::TRANSPARENT),
    };
    let galley = ui.painter().layout_no_wrap(text, font_medium(fs), fg);
    let w = if label.is_empty() { h } else { galley.size().x + pad * 2.0 };
    let (rect, resp) = ui.allocate_exact_size(vec2(w, h), Sense::click());
    if ui.is_rect_visible(rect) {
        let enabled = ui.is_enabled();
        let hover = ui.ctx().animate_bool_with_time(resp.id, resp.hovered() && enabled, 0.12);
        let pressed = resp.is_pointer_button_down_on();
        let fill = match kind {
            Kind::Ghost => mix(Color32::TRANSPARENT, t.surface_hover, hover),
            Kind::Secondary => mix(bg, t.surface_hover, hover),
            _ => mix(bg, mix(bg, Color32::WHITE, 0.10), hover),
        };
        let fill = if pressed { mix(fill, Color32::BLACK, 0.12) } else { fill };
        let fill = if enabled { fill } else { fill.gamma_multiply(0.45) };
        let r = t.corner_small();
        // Flat: one fill, no drop shadow, no gloss.
        let p = ui.painter();
        let native = t.widget_style == WidgetStyle::Adwaita
            && kind == Kind::Secondary
            && crate::native_style::paint_button(ui, t.dark, rect, resp.hovered(), pressed, enabled);
        if !native { p.rect_filled(rect, r, fill); }
        if border != Color32::TRANSPARENT {
            p.rect_stroke(rect, r, Stroke::new(1.0, border), StrokeKind::Inside);
        }
        let fg = if enabled { mix(fg, if kind == Kind::Ghost { t.text } else { fg }, hover) } else { fg.gamma_multiply(0.6) };
        p.galley(rect.center() - galley.size() / 2.0, galley, fg);
        focus_ring(ui, t, &resp, rect);
    }
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Square icon button with optional red badge.
pub fn icon_button(ui: &mut Ui, t: &Theme, icon: &str, tooltip: &str, active: bool, badge: usize) -> Response {
    icon_button_sized(ui, t, icon, tooltip, active, badge, 32.0)
}

/// Wishlist toggle button with a heart icon.
pub fn heart_button(ui: &mut Ui, t: &Theme, on: bool, tooltip: &str) -> Response {
    icon_button(ui, t, crate::icons::HEART, tooltip, on, 0)
}

/// [`icon_button`] at another size (the header uses larger controls).
pub fn icon_button_sized(ui: &mut Ui, t: &Theme, icon: &str, tooltip: &str, active: bool, badge: usize, size: f32) -> Response {
    let size = control_height(t, size);
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(size), Sense::click());
    let enabled = ui.is_enabled();
    let hover = ui.ctx().animate_bool_with_time(resp.id, resp.hovered() && enabled, 0.12);
    let p = ui.painter();
    let base = if t.widget_style == WidgetStyle::Adwaita { Color32::TRANSPARENT } else { t.raised };
    let bg = if active { active_fill(t) } else { mix(base, t.surface_hover, hover) };
    let native = t.widget_style == WidgetStyle::Adwaita && !active && resp.hovered()
        && crate::native_style::paint_button(ui, t.dark, rect, true, resp.is_pointer_button_down_on(), enabled);
    if !native { p.rect_filled(rect, t.corner_small(), if enabled { bg } else { bg.gamma_multiply(0.5) }); }
    let color = if active { if t.widget_style == WidgetStyle::Adwaita { t.text } else { t.accent_text() } } else { mix(t.text_dim, t.text, hover) };
    let color = if enabled { color } else { color.gamma_multiply(0.45) };
    p.text(rect.center(), Align2::CENTER_CENTER, icon, font(size / 2.0), color);
    if badge > 0 {
        let c = rect.right_top() + vec2(-8.0, 8.0);
        let label = if badge > 99 { "99+".to_string() } else { badge.to_string() };
        let w = (label.len() as f32 * 6.0 + 8.0).max(16.0);
        let b = Rect::from_center_size(c, vec2(w, 16.0));
        p.rect_filled(b, CornerRadius::same(8), t.danger_fill());
        p.text(c, Align2::CENTER_CENTER, label, font_bold(10.0), Color32::WHITE);
    }
    focus_ring(ui, t, &resp, rect);
    resp.on_hover_text(tooltip).on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// A clickable row inside a dropdown menu or popup: icon and single-line label,
/// highlighted when hovered or `selected`. Safe for egui sizing passes.
pub fn menu_row(ui: &mut Ui, t: &Theme, icon: &str, label: &str, selected: bool) -> Response {
    menu_row_impl(ui, t, icon, label, None, false, selected)
}

/// Top navigation tab with an animated underline.
/// A row in a dropdown menu: icon and label, the full menu width, with a
/// highlight while hovered so it's clear which item a click will hit.
pub fn menu_item(ui: &mut Ui, t: &Theme, icon: &str, label: &str) -> Response {
    menu_item_tinted(ui, t, icon, label, None)
}

/// [`menu_item`] in its own color, e.g. red for "Uninstall…".
pub fn menu_item_tinted(ui: &mut Ui, t: &Theme, icon: &str, label: &str, tint: Option<Color32>) -> Response {
    menu_row_impl(ui, t, icon, label, tint, false, false)
}

/// A menu row that opens a submenu (a caret on the right), shown with
/// [`egui::containers::menu::SubMenu`].
pub fn submenu_item(ui: &mut Ui, t: &Theme, icon: &str, label: &str) -> Response {
    menu_row_impl(ui, t, icon, label, None, true, false)
}

/// A thin line between groups of menu rows.
pub fn menu_separator(ui: &mut Ui, t: &Theme) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.min_rect().width().max(1.0), 9.0), Sense::hover());
    let x = ui.max_rect().x_range();
    ui.painter().hline(x, rect.center().y, Stroke::new(1.0, t.border));
}

/// The tight row spacing of a popup menu (call at the top of its contents).
pub fn menu_spacing(ui: &mut Ui) {
    ui.spacing_mut().item_spacing.y = 2.0;
}

fn menu_row_impl(ui: &mut Ui, t: &Theme, icon: &str, label: &str, tint: Option<Color32>, sub: bool, selected: bool) -> Response {
    // As wide as the label needs during egui's sizing pass so popup menus do
    // not stretch to the whole screen; outside sizing passes, clamp to
    // available width so bounded dropdowns truncate long labels instead of overflowing.
    let text_w = ui.painter().layout_no_wrap(label.to_string(), font_medium(13.5), t.text).size().x;
    let text_left_pad = if icon.is_empty() { 12.0 } else { 36.0 };
    let natural = text_left_pad + text_w + if sub { 40.0 } else { 16.0 };
    let width = if ui.is_sizing_pass() { natural } else { ui.available_width().max(natural) };
    let (rect, resp) = ui.allocate_exact_size(vec2(width, control_height(t, 30.0)), Sense::click());
    if ui.is_rect_visible(rect) {
        let open = sub && egui::Popup::is_id_open(ui.ctx(), egui::containers::menu::SubMenu::id_from_widget_id(resp.id));
        let hover = ui.ctx().animate_bool_with_time(resp.id, resp.hovered() || open || selected, 0.08);
        let p = ui.painter();
        if hover > 0.0 {
            let fill = if selected { active_fill(t) } else { t.surface_hover };
            p.rect_filled(rect, t.corner_small(), fill.gamma_multiply(hover));
        }
        let y = rect.center().y;
        let (icon_c, text_c) = match tint {
            Some(c) => (c, c),
            None if selected => (t.accent, t.text),
            None => (mix(t.text_dim, t.text, hover), mix(t.text_dim, t.text, 0.6 + 0.4 * hover)),
        };
        if !icon.is_empty() {
            p.text(pos2(rect.left() + 18.0, y), egui::Align2::CENTER_CENTER, icon, font(14.0), icon_c);
        }
        let text_left = rect.left() + text_left_pad;
        let max_w = (rect.right() - if sub { 28.0 } else { 10.0 } - text_left).max(16.0);
        let mut job = egui::text::LayoutJob::single_section(label.to_string(), egui::TextFormat::simple(font_medium(13.5), text_c));
        job.wrap = egui::text::TextWrapping { max_width: max_w, max_rows: 1, break_anywhere: true, overflow_character: Some('…') };
        let galley = p.layout_job(job);
        p.galley(pos2(text_left, y - galley.size().y / 2.0), galley, text_c);
        if sub {
            p.text(pos2(rect.right() - 12.0, y), egui::Align2::CENTER_CENTER, crate::icons::CARET_RIGHT, font(12.0), icon_c);
        }
        focus_ring(ui, t, &resp, rect);
    }
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

pub fn nav_tab(ui: &mut Ui, t: &Theme, label: &str, selected: bool, height: f32) -> Response {
    nav_tab_icon(ui, t, None, label, selected, height)
}

/// Fill of an active control: the accent at low strength over the chrome.
pub fn active_fill(t: &Theme) -> Color32 {
    if t.widget_style == WidgetStyle::Adwaita {
        if t.dark { mix(t.chrome, Color32::WHITE, 0.2) } else { Color32::WHITE }
    } else {
        mix(t.chrome, t.accent, 0.15)
    }
}

/// Navigation button: a 32 px filled tab with an optional icon, centered in
/// a row `height` tall. The open page gets an accent-tinted fill and accent
/// text, the others the neutral control fill.
pub fn nav_tab_icon(ui: &mut Ui, t: &Theme, icon: Option<&str>, label: &str, selected: bool, height: f32) -> Response {
    // Laid out without a color, so the state color below applies.
    let fs = if height >= 48.0 { 13.5 } else { 13.0 };
    let galley = ui.painter().layout_no_wrap(label.to_string(), font_medium(fs), Color32::PLACEHOLDER);
    let icon_w = if icon.is_some() { 22.0 } else { 0.0 };
    let w = galley.size().x + 24.0 + icon_w;
    let (rect, resp) = ui.allocate_exact_size(vec2(w, height), Sense::click());
    let hover = ui.ctx().animate_bool_with_time(resp.id, resp.hovered(), 0.12);
    let sel = ui.ctx().animate_bool_with_time(resp.id.with("sel"), selected, 0.15);
    let pill = Rect::from_center_size(rect.center(), vec2(w, height.min(CONTROL_LG)));
    let fill = mix(mix(t.raised, t.surface_hover, hover), active_fill(t), sel);
    let selected_color = if t.widget_style == WidgetStyle::Adwaita { t.text } else { t.accent_text() };
    let color = mix(mix(t.text_dim, t.text, hover), selected_color, sel);
    let p = ui.painter();
    p.rect_filled(pill, t.corner_small(), fill);
    let mut x = pill.left() + 12.0;
    if let Some(i) = icon {
        p.text(pos2(x + 7.0, pill.center().y), Align2::CENTER_CENTER, i, font(15.0), color);
        x += icon_w;
    }
    p.galley(pos2(x, pill.center().y - galley.size().y / 2.0), galley, color);
    focus_ring(ui, t, &resp, pill);
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Rounded filter chip.
pub fn chip(ui: &mut Ui, t: &Theme, label: &str, selected: bool) -> Response {
    tinted_chip(ui, t, label, None, selected)
}

/// A chip in its own color, e.g. an item rarity: a colored dot and text,
/// and a tinted fill when selected.
pub fn tinted_chip(ui: &mut Ui, t: &Theme, label: &str, tint: Option<Color32>, selected: bool) -> Response {
    chip_impl(ui, t, label, tint, selected, None)
}

/// A chip stretched to `width`, its label centered (e.g. a full-row button
/// under a row of chips).
pub fn wide_chip(ui: &mut Ui, t: &Theme, label: &str, selected: bool, width: f32) -> Response {
    chip_impl(ui, t, label, None, selected, Some(width))
}

/// The width [`chip`] gives `label`.
pub fn chip_width(ui: &Ui, label: &str) -> f32 {
    ui.painter().layout_no_wrap(label.to_string(), font_medium(13.0), Color32::PLACEHOLDER).size().x + 24.0
}

fn chip_impl(ui: &mut Ui, t: &Theme, label: &str, tint: Option<Color32>, selected: bool, width: Option<f32>) -> Response {
    let galley = ui.painter().layout_no_wrap(label.to_string(), font_medium(13.0), Color32::PLACEHOLDER);
    let dot = if tint.is_some() { 14.0 } else { 0.0 };
    let natural = galley.size().x + 24.0 + dot;
    let (rect, resp) = ui.allocate_exact_size(vec2(width.map_or(natural, |w| w.max(natural)), control_height(t, 28.0)), Sense::click());
    let hover = ui.ctx().animate_bool_with_time(resp.id, resp.hovered(), 0.12);
    let p = ui.painter();
    let r = t.corner_small();
    let accent = tint.unwrap_or(t.accent);
    if selected {
        p.rect_filled(rect, r, if t.widget_style == WidgetStyle::Adwaita && tint.is_none() { active_fill(t) } else { mix(t.chrome, accent, 0.18) });
    } else {
        p.rect_filled(rect, r, mix(t.raised, t.surface_hover, hover));
    }
    let color = match (tint, selected) {
        (Some(c), _) => mix(c, t.text, if selected { 0.25 } else { 0.1 + hover * 0.2 }),
        (None, true) => if t.widget_style == WidgetStyle::Adwaita { t.text } else { t.accent_text() },
        (None, false) => mix(t.text_dim, t.text, hover),
    };
    if let Some(c) = tint {
        p.circle_filled(egui::pos2(rect.left() + 16.0, rect.center().y), 4.0, c);
    }
    let x = if width.is_some() { rect.center().x - galley.size().x / 2.0 } else { rect.left() + 12.0 + dot };
    let pos = egui::pos2(x, rect.center().y - galley.size().y / 2.0);
    p.galley(pos, galley, color);
    focus_ring(ui, t, &resp, rect);
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// An on/off chip with a checkmark, for quick filters.
pub fn toggle_chip(ui: &mut Ui, t: &Theme, label: &str, on: &mut bool) -> Response {
    let text = if *on { format!("{}  {label}", crate::icons::CHECK) } else { label.to_string() };
    let mut r = chip(ui, t, &text, *on);
    if r.clicked() {
        *on = !*on;
        r.mark_changed();
    }
    r
}

/// Small non-interactive label pill.
pub fn pill(ui: &mut Ui, text_: &str, fg: Color32, bg: Color32) {
    let galley = ui.painter().layout_no_wrap(text_.to_string(), font_semibold(11.0), fg);
    let (rect, _) = ui.allocate_exact_size(vec2(galley.size().x + 14.0, 20.0), Sense::hover());
    ui.painter().rect_filled(rect, CornerRadius::same(5), bg);
    ui.painter().galley(rect.center() - galley.size() / 2.0, galley, fg);
}

/// Segmented control: side-by-side buttons, the chosen one accent-tinted
/// (like NEVKO's Friends / Chats switch). Returns true if the selection changed.
pub fn segmented<T: PartialEq + Copy>(ui: &mut Ui, t: &Theme, value: &mut T, options: &[(T, &str)]) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        for (opt, label) in options {
            let on = *value == *opt;
            let galley = ui.painter().layout_no_wrap(label.to_string(), font_medium(13.0), Color32::PLACEHOLDER);
            let (rect, resp) = ui.allocate_exact_size(vec2(galley.size().x + 24.0, control_height(t, 30.0)), Sense::click());
            let hover = ui.ctx().animate_bool_with_time(resp.id, resp.hovered(), 0.1);
            let fill = if on { active_fill(t) } else { mix(t.raised, t.surface_hover, hover) };
            ui.painter().rect_filled(rect, t.corner_small(), fill);
            let c = if on { if t.widget_style == WidgetStyle::Adwaita { t.text } else { t.accent_text() } } else { mix(t.text_dim, t.text, hover) };
            ui.painter().galley(rect.center() - galley.size() / 2.0, galley, c);
            focus_ring(ui, t, &resp, rect);
            if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() && !on {
                *value = *opt;
                changed = true;
            }
        }
    });
    changed
}

/// Height of the header's controls (tabs, search, icon buttons).
pub const CONTROL_LG: f32 = 36.0;

/// Compact search field (32 px tall) with a clear button.
pub fn search(ui: &mut Ui, t: &Theme, value: &mut String, hint: &str, width: f32) -> Response {
    search_sized(ui, t, value, hint, width, 32.0)
}

/// [`search`] at another height.
pub fn search_sized(ui: &mut Ui, t: &Theme, value: &mut String, hint: &str, width: f32, height: f32) -> Response {
    let height = control_height(t, height);
    let (rect, _) = ui.allocate_exact_size(vec2(width, height), Sense::hover());
    let r = t.corner_small();
    let fill = if t.widget_style == WidgetStyle::Adwaita { ui.visuals().extreme_bg_color } else { t.raised };
    ui.painter().rect_filled(rect, r, fill);
    if t.widget_style == WidgetStyle::Adwaita {
        ui.painter().rect_stroke(rect, r, Stroke::new(1.0, t.border), StrokeKind::Inside);
    }
    ui.painter().text(rect.left_center() + vec2(15.0, 0.0), Align2::CENTER_CENTER, crate::icons::SEARCH, font(14.0), t.text_faint);
    let clear_w = if value.is_empty() { 0.0 } else { 26.0 };
    let field = Rect::from_min_max(pos2(rect.left() + 30.0, rect.top()), pos2(rect.right() - 12.0 - clear_w, rect.bottom()));
    let resp = ui.put(
        field,
        egui::TextEdit::singleline(value)
            .hint_text(text(hint, 13.5, t.text_faint))
            .font(font(13.5))
            .frame(Frame::NONE)
            .margin(vec2(0.0, 0.0))
            .align(Align2::LEFT_CENTER)
            .desired_width(field.width()),
    ).with_edit_menu();
    if t.widget_style == WidgetStyle::Adwaita {
        focus_ring(ui, t, &resp, rect);
    } else if resp.has_focus() {
        ui.painter().rect_stroke(rect, r, Stroke::new(1.0, t.accent.gamma_multiply(0.6)), StrokeKind::Inside);
    }
    if !value.is_empty() {
        let c = pos2(rect.right() - 20.0, rect.center().y);
        let x = ui.interact(Rect::from_center_size(c, Vec2::splat(20.0)), resp.id.with("clear"), Sense::click());
        ui.painter().text(c, Align2::CENTER_CENTER, crate::icons::X, font(12.0), if x.hovered() { t.text } else { t.text_faint });
        if x.clicked() {
            value.clear();
        }
    }
    resp
}

/// Labeled text input used on forms.
pub fn field(ui: &mut Ui, t: &Theme, label: &str, value: &mut String, password: bool) -> Response {
    ui.label(text_medium(label, 12.5, t.text_dim));
    ui.add_space(-2.0);
    let resp = ui.add(
        egui::TextEdit::singleline(value)
            .password(password)
            .font(font(15.0))
            .desired_width(f32::INFINITY)
            .margin(if t.widget_style == WidgetStyle::Adwaita { vec2(9.0, 7.0) } else { vec2(12.0, 10.0) }),
    ).with_edit_menu();
    focus_ring(ui, t, &resp, resp.rect);
    resp
}

// ---------- people ----------

/// Round avatar with an optional presence dot.
pub fn avatar(ui: &mut Ui, t: &Theme, url: &str, size: f32, status: Option<Color32>) -> Response {
    avatar_rounded(ui, t, url, size, status, CornerRadius::same((size / 2.0) as u8))
}

/// Like [`avatar`], with its own corner rounding (e.g. the theme's, to sit
/// inside a button).
pub fn avatar_rounded(ui: &mut Ui, t: &Theme, url: &str, size: f32, status: Option<Color32>, radius: CornerRadius) -> Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(size), Sense::click());
    if !ui.is_rect_visible(rect) {
        return resp;
    }
    if !paint_cover(ui, rect, url, radius) {
        ui.painter().rect_filled(rect, radius, t.surface_hover);
    }
    if let Some(c) = status {
        let r = (size * 0.14).max(4.0);
        let center = rect.right_bottom() - Vec2::splat(r * 0.9);
        ui.painter().circle_filled(center, r + 2.0, t.chrome);
        ui.painter().circle_filled(center, r, c);
    }
    resp
}

// ---------- games ----------

/// Library capsule (2:3) with hover lift and installed marker.
pub fn game_tile(ui: &mut Ui, t: &Theme, url: &str, fallback_url: &str, title: &str, installed: bool, size: Vec2) -> Response {
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    if !ui.is_rect_visible(rect) {
        // Load tiles up to a screen above and below ahead of time, so they
        // are ready when scrolled to (and stay loaded while close by).
        let clip = ui.clip_rect();
        if clip.expand2(vec2(0.0, clip.height())).intersects(rect) {
            prefetch_first(ui, &[url, fallback_url], size);
        }
        return resp;
    }
    let hover = ui.ctx().animate_bool_with_time(resp.id, resp.hovered(), 0.15);
    let r = t.corner_small();
    let img = rect.expand(hover * 3.0);
    if hover > 0.0 {
        ui.painter().rect_filled(img.translate(vec2(0.0, 8.0 * hover)), r, Color32::from_black_alpha((90.0 * hover) as u8));
    }
    if !paint_cover_first(ui, img, &[url, fallback_url], r) {
        placeholder(ui, t, img, title, r);
    }
    let p = ui.painter();
    if !installed {
        p.rect_filled(img, r, Color32::from_black_alpha(90));
    }
    if hover > 0.0 {
        p.rect_stroke(img, r, Stroke::new(1.5, t.accent.gamma_multiply(hover)), StrokeKind::Outside);
    }
    if installed {
        let c = img.left_bottom() + vec2(14.0, -14.0);
        p.circle_filled(c, 7.0, Color32::from_black_alpha(160));
        p.circle_filled(c, 4.0, t.play);
    }
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Price with an optional discount badge and struck-through original.
pub fn price_tag(ui: &mut Ui, t: &Theme, label: &str, original: Option<&str>, discount: u8) {
    if discount == 0 {
        ui.label(text_semibold(label, 14.0, t.text));
        return;
    }
    // Like Steam: a green "-10%" block joined to a dark block with the old
    // price struck through above the new one.
    let pct = ui.painter().layout_no_wrap(format!("-{discount}%"), font_bold(17.0), Color32::from_rgb(0xBE, 0xEE, 0x11));
    let new = ui.painter().layout_no_wrap(label.to_string(), font_semibold(14.0), t.text);
    let old = original.map(|o| ui.painter().layout_no_wrap(o.to_string(), font(11.0), t.text_faint));
    let h = 36.0;
    let pct_w = pct.size().x + 16.0;
    let price_w = new.size().x.max(old.as_ref().map(|g| g.size().x).unwrap_or(0.0)) + 16.0;
    let (rect, _) = ui.allocate_exact_size(vec2(pct_w + price_w, h), Sense::hover());
    if !ui.is_rect_visible(rect) {
        return;
    }
    let p = ui.painter();
    let left = Rect::from_min_size(rect.min, vec2(pct_w, h));
    let right = Rect::from_min_max(egui::pos2(left.right(), rect.top()), rect.max);
    let r = CornerRadius::same(3);
    p.rect_filled(left, CornerRadius { nw: r.nw, sw: r.sw, ne: 0, se: 0 }, t.discount);
    p.rect_filled(right, CornerRadius { nw: 0, sw: 0, ne: r.ne, se: r.se }, Color32::from_black_alpha(140));
    p.galley(left.center() - pct.size() / 2.0, pct, Color32::from_rgb(0xBE, 0xEE, 0x11));
    match old {
        Some(old) => {
            let x = right.right() - 8.0;
            let top = egui::pos2(x - old.size().x, right.top() + 3.0);
            let strike_y = top.y + old.size().y / 2.0;
            let old_w = old.size().x;
            p.galley(top, old, t.text_faint);
            p.line_segment([egui::pos2(top.x - 1.0, strike_y), egui::pos2(top.x + old_w + 1.0, strike_y)], Stroke::new(1.0, t.text_faint));
            p.galley(egui::pos2(x - new.size().x, right.bottom() - 3.0 - new.size().y), new, t.text);
        }
        None => {
            p.galley(right.center() - new.size() / 2.0, new, t.text);
        }
    }
}

// ---------- layout helpers ----------

/// A centered column at most `max` wide with at least `pad` on each side.
pub fn centered<R>(ui: &mut Ui, max: f32, pad: f32, add: impl FnOnce(&mut Ui) -> R) -> R {
    let avail = ui.available_width();
    let width = (avail - pad * 2.0).min(max).max(100.0);
    let left = ((avail - width) / 2.0).max(0.0);
    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        ui.add_space(left);
        ui.vertical(|ui| {
            ui.set_width(width);
            ui.spacing_mut().item_spacing = egui::vec2(8.0, 8.0);
            add(ui)
        })
        .inner
    })
    .inner
}

pub fn page_title(ui: &mut Ui, t: &Theme, title: &str, subtitle: Option<&str>) {
    ui.label(text_bold(title, 28.0, t.text));
    if let Some(s) = subtitle {
        ui.label(text(s, 14.0, t.text_dim));
    }
    ui.add_space(14.0);
}

/// Section header; returns true if the action link was clicked.
pub fn section(ui: &mut Ui, t: &Theme, title: &str, action: Option<&str>) -> bool {
    let mut clicked = false;
    ui.horizontal(|ui| {
        ui.label(text_bold(title, 18.0, t.text));
        if let Some(a) = action {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let r = ui.add(egui::Label::new(text_medium(a, 13.0, t.text_dim)).sense(Sense::click()));
                if r.hovered() {
                    ui.painter().hline(r.rect.x_range(), r.rect.bottom(), Stroke::new(1.0, t.text_dim));
                }
                clicked = r.on_hover_cursor(egui::CursorIcon::PointingHand).clicked();
            });
        }
    });
    ui.add_space(6.0);
    clicked
}

/// Label that heads a group in a list, NEVKO style: sentence case and a
/// middle dot before the count ("In game · 6"), never shouted in capitals.
pub fn caption(ui: &mut Ui, t: &Theme, s: &str) {
    ui.label(caption_text(t, s));
}

pub fn caption_text(t: &Theme, s: &str) -> RichText {
    text_semibold(sentence_case(&s.replace(" — ", " · ")), 12.5, t.text_dim)
}

/// "YOUR FRIEND CODE" -> "Your friend code". Text with any lowercase letter
/// is left alone, so names and mixed-case labels keep their spelling.
pub fn sentence_case(s: &str) -> String {
    if s.chars().any(char::is_lowercase) {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut first = true;
    for c in s.chars() {
        if first && c.is_alphabetic() {
            out.push(c);
            first = false;
        } else {
            out.extend(c.to_lowercase());
        }
    }
    out
}

pub fn stat(ui: &mut Ui, t: &Theme, label: &str, value: &str) {
    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing.y = 2.0;
        ui.label(text_medium(label, 12.0, t.text_faint));
        ui.label(text_semibold(value, 15.0, t.text));
    });
}

pub fn loading(ui: &mut Ui, t: &Theme, label: &str) {
    ui.vertical_centered(|ui| {
        ui.add_space(60.0);
        ui.add(egui::Spinner::new().size(28.0).color(t.accent));
        ui.add_space(10.0);
        ui.label(text(label, 14.0, t.text_dim));
    });
}

pub fn empty(ui: &mut Ui, t: &Theme, icon: &str, title: &str, body: &str) {
    ui.vertical_centered(|ui| {
        ui.add_space(70.0);
        ui.label(text(icon, 44.0, t.text_faint));
        ui.add_space(8.0);
        ui.label(text_semibold(title, 18.0, t.text));
        ui.label(text(body, 14.0, t.text_dim));
    });
}

/// Thin progress bar.
pub fn progress(ui: &mut Ui, t: &Theme, value: f32, width: f32) {
    // Never negative: egui aborts the whole app on a negative size.
    let width = width.max(0.0);
    let (rect, _) = ui.allocate_exact_size(vec2(width, 6.0), Sense::hover());
    ui.painter().rect_filled(rect, CornerRadius::same(3), t.raised);
    let fill = Rect::from_min_size(rect.min, vec2(rect.width() * value.clamp(0.0, 1.0), rect.height()));
    ui.painter().rect_filled(fill, CornerRadius::same(3), t.accent);
}

/// The Aurora mark, tinted with the theme's text color.
pub fn logo(ui: &mut Ui, t: &Theme, size: f32) -> Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(size), Sense::hover());
    egui::Image::from_bytes(crate::brand::LOGO_URI, crate::brand::LOGO_SVG).tint(t.text).paint_at(ui, rect);
    resp
}

// ---------- choices ----------

/// Indicator + label row shared by [`radio`] and [`check`].
fn choice_row(ui: &mut Ui, label: impl Into<egui::WidgetText>, paint: impl FnOnce(&egui::Painter, Rect, f32)) -> Response {
    let galley = label.into().into_galley(ui, Some(egui::TextWrapMode::Extend), f32::INFINITY, egui::TextStyle::Body);
    let size = vec2(18.0 + 10.0 + galley.size().x, galley.size().y.max(18.0) + 6.0);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    let hover = ui.ctx().animate_bool_with_time(resp.id, resp.hovered(), 0.1);
    let mark = Rect::from_center_size(egui::pos2(rect.left() + 9.0, rect.center().y), Vec2::splat(18.0));
    paint(ui.painter(), mark, hover);
    let color = ui.visuals().text_color();
    ui.painter().galley(egui::pos2(mark.right() + 10.0, rect.center().y - galley.size().y / 2.0), galley, color);
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// A clearly visible radio button.
pub fn radio(ui: &mut Ui, t: &Theme, selected: bool, label: impl Into<egui::WidgetText>) -> Response {
    choice_row(ui, label, |p, r, hover| {
        let ring = if selected { t.accent } else { mix(t.text_faint, t.text_dim, hover) };
        p.circle_filled(r.center(), 9.0, t.raised);
        p.circle_stroke(r.center(), 8.5, Stroke::new(1.6, ring));
        if selected {
            p.circle_filled(r.center(), 4.5, t.accent);
        }
    })
}

/// Radio button that sets `current` to `value` when clicked.
pub fn radio_value<V: PartialEq>(ui: &mut Ui, t: &Theme, current: &mut V, value: V, label: impl Into<egui::WidgetText>) -> Response {
    let mut r = radio(ui, t, *current == value, label);
    if r.clicked() && *current != value {
        *current = value;
        r.mark_changed();
    }
    r
}

/// A clearly visible checkbox.
pub fn check(ui: &mut Ui, t: &Theme, on: &mut bool, label: impl Into<egui::WidgetText>) -> Response {
    let state = *on;
    let mut r = choice_row(ui, label, |p, r, hover| {
        let rr = CornerRadius::same(4);
        if state {
            p.rect_filled(r, rr, t.accent);
            let s = Stroke::new(2.0, t.on_accent);
            let a = egui::pos2(r.left() + 4.5, r.center().y + 0.5);
            let b = egui::pos2(r.left() + 7.8, r.bottom() - 5.0);
            let c = egui::pos2(r.right() - 4.0, r.top() + 5.0);
            p.line_segment([a, b], s);
            p.line_segment([b, c], s);
        } else {
            p.rect_filled(r, rr, t.raised);
            p.rect_stroke(r, rr, Stroke::new(1.6, mix(t.text_faint, t.text_dim, hover)), StrokeKind::Inside);
        }
    });
    if r.clicked() {
        *on = !*on;
        r.mark_changed();
    }
    r
}

/// A chip with a small picture (a game icon, an avatar) before the label.
pub fn image_chip(ui: &mut Ui, t: &Theme, image: Option<&str>, label: &str, selected: bool) -> Response {
    // A 38 px control with a 28 px picture: big enough to tell games apart.
    let galley = ui.painter().layout_no_wrap(label.to_string(), font_medium(13.5), Color32::PLACEHOLDER);
    let (h, pic) = (38.0, 28.0);
    let lead = if image.is_some() { 5.0 + pic + 10.0 } else { 14.0 };
    let (rect, resp) = ui.allocate_exact_size(vec2(lead + galley.size().x + 14.0, h), Sense::click());
    let hover = ui.ctx().animate_bool_with_time(resp.id, resp.hovered(), 0.12);
    let r = t.corner_small();
    let p = ui.painter();
    let fill = if selected { active_fill(t) } else { mix(t.raised, t.surface_hover, hover) };
    p.rect_filled(rect, r, fill);
    if let Some(url) = image {
        let img = Rect::from_min_size(egui::pos2(rect.left() + 5.0, rect.center().y - pic / 2.0), Vec2::splat(pic));
        let ir = CornerRadius::same(r.nw.saturating_sub(2).max(3));
        if !paint_cover(ui, img, url, ir) {
            skeleton(ui, t, img, ir);
        }
    }
    let color = if selected { if t.widget_style == WidgetStyle::Adwaita { t.text } else { t.accent_text() } } else { mix(t.text_dim, t.text, hover) };
    ui.painter().galley(egui::pos2(rect.left() + lead, rect.center().y - galley.size().y / 2.0), galley, color);
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

// ---------- Steam markup ----------

/// Lay out Steam's mix of BBCode and simple HTML (item descriptions, bios):
/// `[color=#ff9800]…[/color]`, `[b]`, `[i]`, `[u]`, `[url=…]…[/url]`,
/// `<br>`, `<font color="…">`. Unknown tags are dropped, text is kept.
pub fn markup(text: &str, size: f32, color: Color32, wrap_width: f32) -> egui::text::LayoutJob {
    use egui::text::{LayoutJob, TextFormat};
    let mut job = LayoutJob::default();
    job.wrap.max_width = wrap_width;
    let mut colors: Vec<Color32> = vec![color];
    let (mut bold_on, mut italic, mut underline) = (0u32, 0u32, 0u32);
    let parse_color = |v: &str| -> Option<Color32> {
        let v = v.trim().trim_matches(|c| c == '"' || c == '\'').trim_start_matches('#');
        let n = u32::from_str_radix(v.get(..6)?, 16).ok()?;
        Some(Color32::from_rgb((n >> 16) as u8, (n >> 8) as u8, n as u8))
    };
    let push = |job: &mut LayoutJob, s: &str, colors: &[Color32], b: u32, i: u32, u: u32| {
        if s.is_empty() {
            return;
        }
        let c = *colors.last().unwrap_or(&color);
        let mut f = TextFormat { font_id: if b > 0 { font_semibold(size) } else { font(size) }, color: c, italics: i > 0, ..Default::default() };
        if u > 0 {
            f.underline = Stroke::new(1.0, c);
        }
        job.append(&decode_entities(s), 0.0, f);
    };
    let mut rest = text;
    while !rest.is_empty() {
        let next = rest.find(['[', '<']);
        let Some(at) = next else {
            push(&mut job, rest, &colors, bold_on, italic, underline);
            break;
        };
        push(&mut job, &rest[..at], &colors, bold_on, italic, underline);
        let close_ch = if rest[at..].starts_with('[') { ']' } else { '>' };
        let Some(end) = rest[at..].find(close_ch) else {
            push(&mut job, &rest[at..], &colors, bold_on, italic, underline);
            break;
        };
        let tag = &rest[at + 1..at + end];
        let lower = tag.to_ascii_lowercase();
        let name = lower.trim_start_matches('/').split(['=', ' ']).next().unwrap_or_default().to_string();
        let closing = lower.starts_with('/');
        let known = matches!(name.as_str(), "color" | "font" | "b" | "strong" | "i" | "em" | "u" | "url" | "a" | "br" | "p" | "h1" | "h2" | "h3" | "list" | "li" | "*" | "span" | "div" | "img" | "noparse" | "hr" | "strike" | "spoiler" | "quote" | "code");
        if !known {
            // Not a tag: keep the bracket as text.
            push(&mut job, &rest[at..at + 1], &colors, bold_on, italic, underline);
            rest = &rest[at + 1..];
            continue;
        }
        match (name.as_str(), closing) {
            ("color" | "font", false) => {
                let value = tag.split_once('=').map(|x| x.1).unwrap_or_default();
                let value = value.trim_start_matches("color=").trim_start_matches("\"").to_string();
                colors.push(parse_color(&value).unwrap_or(*colors.last().unwrap_or(&color)));
            }
            ("color" | "font", true) => {
                if colors.len() > 1 {
                    colors.pop();
                }
            }
            ("b" | "strong" | "h1" | "h2" | "h3", false) => bold_on += 1,
            ("b" | "strong", true) => bold_on = bold_on.saturating_sub(1),
            ("h1" | "h2" | "h3", true) => {
                bold_on = bold_on.saturating_sub(1);
                push(&mut job, "\n", &colors, 0, 0, 0);
            }
            ("i" | "em", false) => italic += 1,
            ("i" | "em", true) => italic = italic.saturating_sub(1),
            ("u", false) => underline += 1,
            ("u", true) => underline = underline.saturating_sub(1),
            ("br" | "p" | "li" | "*" | "hr", _) | ("div", true) => push(&mut job, "\n", &colors, 0, 0, 0),
            _ => {}
        }
        rest = &rest[at + end + 1..];
    }
    job
}

fn decode_entities(s: &str) -> String {
    s.replace("&quot;", "\"").replace("&#39;", "'").replace("&apos;", "'").replace("&lt;", "<").replace("&gt;", ">").replace("&nbsp;", " ").replace("&amp;", "&")
}

/// Show Steam markup as a wrapped label.
pub fn markup_label(ui: &mut Ui, text: &str, size: f32, color: Color32) -> Response {
    let job = markup(text, size, color, ui.available_width());
    ui.label(job)
}

#[cfg(test)]
mod markup_tests {

    #[test]
    fn animation_decoder_slots_are_bounded_and_reusable() {
        let mut slots: Vec<_> = (0..4).map(|_| super::AnimationDecodeSlot::acquire().unwrap()).collect();
        assert!(super::AnimationDecodeSlot::acquire().is_none());
        drop(slots.pop());
        assert!(super::AnimationDecodeSlot::acquire().is_some());
    }

    #[test]
    fn animation_decodes_lossy_webp_frames() {
        // Two 8x8 frames, red and blue, encoded as VP8 chunks.
        let bytes = b"\
            \x52\x49\x46\x46\xc0\x00\x00\x00\x57\x45\x42\x50\x56\x50\x38\x58\
            \x0a\x00\x00\x00\x02\x00\x00\x00\x07\x00\x00\x07\x00\x00\x41\x4e\
            \x49\x4d\x06\x00\x00\x00\xff\xff\xff\xff\x00\x00\x41\x4e\x4d\x46\
            \x48\x00\x00\x00\x00\x00\x00\x00\x00\x00\x07\x00\x00\x07\x00\x00\
            \x64\x00\x00\x02\x56\x50\x38\x20\x30\x00\x00\x00\xd0\x01\x00\x9d\
            \x01\x2a\x08\x00\x08\x00\x02\x00\x34\x25\xa0\x02\x74\xba\x01\xf8\
            \x00\x03\xb0\x00\xfe\xf0\xe8\xf7\xff\x20\xb9\x61\x75\xc8\xd7\xff\
            \x20\x3f\xe4\x07\xfc\x80\xff\xf8\xf2\x00\x00\x00\x41\x4e\x4d\x46\
            \x44\x00\x00\x00\x00\x00\x00\x00\x00\x00\x07\x00\x00\x07\x00\x00\
            \x64\x00\x00\x00\x56\x50\x38\x20\x2c\x00\x00\x00\x94\x01\x00\x9d\
            \x01\x2a\x08\x00\x08\x00\x00\x00\x34\x25\xa0\x02\x74\xba\x00\x03\
            \x98\x00\xfe\xf9\x93\x6f\xff\x90\x1f\xff\x90\x1f\xff\x90\x1f\xff\
            \x20\x3f\xe2\x17\x7b\x20\x30\x00";
        assert!(super::is_animation(bytes));
        let (frames, delays) = super::decode_apng(bytes).unwrap();
        assert_eq!(delays, [100, 100]);
        assert!(frames[0].pixels[0].r() > 200);
        assert!(frames[1].pixels[0].b() > 200);
    }

    #[test]
    fn animation_rejects_oversized_canvas() {
        let mut bytes = Vec::new();
        {
            let mut encoder = image::codecs::gif::GifEncoder::new(&mut bytes);
            let image = image::RgbaImage::from_pixel(4097, 1, image::Rgba([0, 0, 0, 255]));
            encoder.encode_frame(image::Frame::new(image)).unwrap();
        }
        assert!(super::decode_apng(&bytes).is_none());
    }
    #[test]
    fn animation_state_repaints_and_loops_after_completion() {
        use std::sync::Arc;
        let ctx = egui::Context::default();
        let url = "test-animation-loop-state";
        let textures: Vec<_> = [egui::Color32::RED, egui::Color32::BLUE].into_iter().map(|color| {
            ctx.load_texture("loop-frame", egui::ColorImage::filled([2, 2], color), egui::TextureOptions::LINEAR)
        }).collect();
        let ids: Vec<_> = textures.iter().map(|t| t.id()).collect();
        super::APNGS.lock().unwrap().get_or_insert_with(Default::default).insert(url.into(), super::Apng::Ready(Arc::new(textures), Arc::new(vec![100, 100]), true));
        let mut seen = Vec::new();
        for time in [0.0, 0.1, 0.2, 0.3] {
            let input = egui::RawInput { time: Some(time), ..Default::default() };
            let mut output = ctx.run_ui(input, |_ui| {
                let Ok(egui::load::TexturePoll::Ready { texture }) = super::animation_texture(&ctx, url, egui::load::Bytes::Static(&[]), true, 240.0) else {
                    panic!("expected a ready animation frame");
                };
                seen.push(texture.id);
            });
            output.textures_delta.clear();
        }
        assert_eq!(seen, [ids[0], ids[1], ids[0], ids[1]]);
        super::APNGS.lock().unwrap().as_mut().unwrap().remove(url);
    }

    #[test]
    fn animation_keeps_valid_frames_before_a_damaged_trailer() {
        let mut bytes = Vec::new();
        {
            let mut encoder = image::codecs::gif::GifEncoder::new(&mut bytes);
            for i in 0..10 {
                let image = image::RgbaImage::from_pixel(4, 4, image::Rgba([i * 20, 0, 0, 255]));
                encoder.encode_frame(image::Frame::from_parts(image, 0, 0, image::Delay::from_numer_denom_ms(30, 1))).unwrap();
            }
        }
        bytes.truncate(bytes.len() - 5);
        let (images, delays) = super::decode_apng(&bytes).expect("valid earlier frames should survive");
        assert!(images.len() >= 8);
        assert!(delays.iter().sum::<u32>() >= 240);
    }

    #[test]
    fn animation_loops_only_after_full_duration() {
        let delays = [100, 200, 300];
        assert_eq!(super::frame_at(&delays, 0.0).0, 0);
        assert_eq!(super::frame_at(&delays, 0.15).0, 1);
        assert_eq!(super::frame_at(&delays, 0.599).0, 2);
        assert_eq!(super::frame_at(&delays, 0.6).0, 0);
        assert_eq!(super::frame_at(&delays, 0.75).0, 1);
    }

    #[test]
    fn long_gif_preserves_full_duration() {
        let mut bytes = Vec::new();
        {
            let mut encoder = image::codecs::gif::GifEncoder::new(&mut bytes);
            for i in 0..320 {
                let image = image::RgbaImage::from_pixel(2, 2, image::Rgba([i as u8, 0, 0, 255]));
                encoder.encode_frame(image::Frame::from_parts(image, 0, 0, image::Delay::from_numer_denom_ms(10, 1))).unwrap();
            }
        }
        let mut preview_frames = 0;
        let (frames, delays) = super::decode_animation(&bytes, |frames, _| preview_frames = frames.len()).unwrap();
        assert_eq!(preview_frames, 156);
        assert!(frames.len() <= super::APNG_MAX_FRAMES);
        assert_eq!(delays.iter().sum::<u32>(), 3200);
        assert!(frames.last().unwrap().pixels[0].r() > 40);
    }

    #[test]
    fn workshop_gif_decodes_bounded_animated_frames() {
        let mut bytes = Vec::new();
        {
            let mut encoder = image::codecs::gif::GifEncoder::new(&mut bytes);
            for color in [[255, 0, 0, 255], [0, 0, 255, 255]] {
                let image = image::RgbaImage::from_pixel(300, 400, image::Rgba(color));
                encoder.encode_frame(image::Frame::from_parts(image, 0, 0, image::Delay::from_numer_denom_ms(100, 1))).unwrap();
            }
        }
        let (frames, delays) = super::decode_apng(&bytes).unwrap();
        assert_eq!(frames.len(), 2);
        assert_ne!(frames[0].pixels[0], frames[1].pixels[0]);
        assert!(frames[0].size[0].max(frames[0].size[1]) <= super::APNG_MAX_SIDE as usize);
        assert_eq!(delays, [100, 100]);
    }

    #[test]
    fn mix_preserves_translucent_endpoints_and_premultiplied_midpoint() {
        let a = egui::Color32::from_rgba_premultiplied(20, 40, 60, 80);
        let b = egui::Color32::from_rgba_premultiplied(100, 120, 140, 160);
        assert_eq!(super::mix(a, b, 0.0), a);
        assert_eq!(super::mix(a, b, 1.0), b);
        assert_eq!(super::mix(a, b, 0.5), egui::Color32::from_rgba_premultiplied(60, 80, 100, 120));
        assert_eq!(super::mix(egui::Color32::from_rgb(20, 40, 60), egui::Color32::from_rgb(100, 120, 140), 0.5), egui::Color32::from_rgb(60, 80, 100));
    }

    #[test]
    fn adwaita_notification_badge_uses_destructive_fill() {
        let ctx = egui::Context::default();
        crate::install_fonts(&ctx);
        let theme = crate::Theme::adwaita(true);
        theme.apply(&ctx);
        let mut output = ctx.run_ui(Default::default(), |ui| {
            super::icon_button(ui, &theme, crate::icons::CHECK, "Notifications", false, 1);
        });
        output.textures_delta.clear();
        assert!(output.shapes.iter().any(|shape| matches!(&shape.shape, egui::Shape::Rect(rect) if rect.rect.height() == 16.0 && rect.fill == egui::Color32::from_rgb(0xc0, 0x1c, 0x28))));
    }

    #[test]
    fn adwaita_shared_controls_have_native_minimum_height() {
        let ctx = egui::Context::default();
        crate::install_fonts(&ctx);
        let theme = crate::Theme::adwaita(false);
        theme.apply(&ctx);
        let mut value = String::new();
        let mut enabled = false;
        let mut output = ctx.run_ui(Default::default(), |ui| {
            assert_eq!(super::button(ui, &theme, None, "OK", super::Kind::Secondary, super::Size::Sm).rect.height(), 34.0);
            assert_eq!(super::icon_button(ui, &theme, crate::icons::CHECK, "OK", false, 0).rect.height(), 34.0);
            assert_eq!(super::toggle_chip(ui, &theme, "Enabled", &mut enabled).rect.height(), 34.0);
            assert_eq!(super::search(ui, &theme, &mut value, "Search", 200.0).rect.height(), 34.0);
        });
        output.textures_delta.clear();
    }

    #[test]
    fn reads_steam_markup() {
        let job = super::markup("Skin for [color=#ffdba5]Jacket[/color]. [b]Breaks[/b] into 1 x Cloth<br>Tradable", 12.0, egui::Color32::WHITE, 300.0);
        assert_eq!(job.text, "Skin for Jacket. Breaks into 1 x Cloth\nTradable");
        assert!(job.sections.iter().any(|s| s.format.color == egui::Color32::from_rgb(0xff, 0xdb, 0xa5)));
        // Brackets that aren't tags stay.
        assert_eq!(super::markup("[x] and [y]", 12.0, egui::Color32::WHITE, 100.0).text, "[x] and [y]");
    }
}

/// Right-click menu for text fields, like Steam's: Cut, Copy, Paste.
/// They act on the field through egui's own clipboard commands, so
/// selections and the cursor behave exactly as with Ctrl+X/C/V.
pub trait EditMenu {
    fn with_edit_menu(self) -> Self;
}

impl EditMenu for Response {
    fn with_edit_menu(self) -> Self {
        if self.secondary_clicked() {
            // The commands go to the focused field: this one.
            self.request_focus();
        }
        self.context_menu(|ui| {
            ui.set_min_width(160.0);
            let mut cmd = None;
            if ui.button(i18n::tr("Cut")).clicked() {
                cmd = Some(egui::ViewportCommand::RequestCut);
            }
            if ui.button(i18n::tr("Copy")).clicked() {
                cmd = Some(egui::ViewportCommand::RequestCopy);
            }
            if ui.button(i18n::tr("Paste")).clicked() {
                cmd = Some(egui::ViewportCommand::RequestPaste);
            }
            if let Some(c) = cmd {
                self.request_focus();
                ui.ctx().send_viewport_cmd(c);
                ui.close();
            }
        });
        self
    }
}

/// Reserve space for a fixed-height row outside the viewport and its
/// one-screen prefetch margin.
pub fn skip_offscreen_row(ui: &mut Ui, height: f32) -> bool {
    let clip = ui.clip_rect();
    let y = ui.next_widget_position().y;
    if y > clip.bottom() + clip.height() || y + height < clip.top() - clip.height() {
        ui.allocate_space(egui::vec2(ui.available_width(), height));
        true
    } else {
        false
    }
}
