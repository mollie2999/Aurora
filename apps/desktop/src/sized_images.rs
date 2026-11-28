//! Still pictures decoded at the size they're drawn at, on a few threads.
//!
//! egui's own image loader decodes every picture in full (a 600×900 library
//! capsule is 2.2 MB as a texture, drawn at ~190×280) and starts a thread
//! per picture, so opening a page full of art decoded dozens at once. The
//! widgets ask for `<url>#sir-fit=<cover|contain>,<w>x<h>` (see
//! `ui::widgets::sized_url`); this loader reads the picture's bytes through
//! the normal (disk-cached) bytes loader, decodes and shrinks it on a small
//! pool, and keeps the result only until it's on the GPU.

use egui::ColorImage;
use egui::load::{BytesPoll, ImageLoadResult, ImageLoader, ImagePoll, LoadError, SizeHint};
use std::collections::HashMap;
use std::sync::mpsc::{Receiver, SyncSender, TrySendError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Decoded pictures nobody picked up for this long are dropped.
const UNCLAIMED_FOR: Duration = Duration::from_secs(10);
const MAX_IMAGE_DIM: u32 = 32768;
const MAX_IMAGE_ALLOC: u64 = 64 << 20;
const MAX_QUEUED_JOBS: usize = 32;

fn image_limits() -> image::Limits {
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_IMAGE_DIM);
    limits.max_image_height = Some(MAX_IMAGE_DIM);
    limits.max_alloc = Some(MAX_IMAGE_ALLOC);
    limits
}

/// Cap inline rich-text/store image URLs through [`ui::widgets::sized_url`] (up to `1600×1200`)
/// so `SizedLoader` decodes and downscales them under [`image_limits`].
pub fn inline_url(url: &str, max_w: f32, max_h: f32, ppp: f32) -> String {
    let scale = if ppp.is_finite() && ppp > 0.0 { ppp } else { 1.0 };
    let w = if max_w.is_finite() { (max_w * scale).clamp(64.0, 1600.0) } else { 1600.0 };
    let h = if max_h.is_finite() { (max_h * scale).clamp(64.0, 1200.0) } else { 1200.0 };
    ui::widgets::sized_url(url, egui::vec2(w, h), false)
}

enum Entry {
    Pending,
    Ready(Arc<ColorImage>, Instant),
    Failed(String),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Blur {
    None,
    /// Sharp at the top, blurred toward the bottom (game page backdrops).
    Bottom,
    /// Blurred all over (frosted profile panels).
    All,
}

struct Job {
    uri: String,
    bytes: egui::load::Bytes,
    cover: bool,
    blur: Blur,
    w: u32,
    h: u32,
    ctx: egui::Context,
}

pub struct SizedLoader {
    entries: Arc<Mutex<HashMap<String, Entry>>>,
    jobs: Mutex<SyncSender<Job>>,
    swept: Mutex<Instant>,
}

impl SizedLoader {
    pub fn install(ctx: &egui::Context) {
        let entries: Arc<Mutex<HashMap<String, Entry>>> = Default::default();
        let (tx, rx) = std::sync::mpsc::sync_channel::<Job>(MAX_QUEUED_JOBS);
        let rx = Arc::new(Mutex::new(rx));
        // A few decoders: enough to keep up with scrolling, few enough that a
        // page of art doesn't hold dozens of full-size pictures at once.
        let threads = std::thread::available_parallelism().map(|n| (n.get() / 2).clamp(1, 3)).unwrap_or(2);
        for i in 0..threads {
            let (rx, entries) = (rx.clone(), entries.clone());
            let _ = std::thread::Builder::new().name(format!("picture-decode-{i}")).spawn(move || worker(&rx, &entries));
        }
        ctx.add_image_loader(Arc::new(SizedLoader { entries, jobs: Mutex::new(tx), swept: Mutex::new(Instant::now()) }));
    }

    /// Drop decoded pictures that were never uploaded (scrolled past while decoding).
    fn sweep(&self) {
        let mut swept = self.swept.lock().unwrap();
        if swept.elapsed() < Duration::from_secs(5) {
            return;
        }
        *swept = Instant::now();
        self.entries.lock().unwrap().retain(|_, e| !matches!(e, Entry::Ready(_, at) if at.elapsed() > UNCLAIMED_FOR));
    }
}

fn worker(rx: &Mutex<Receiver<Job>>, entries: &Mutex<HashMap<String, Entry>>) {
    let mut resizer = fast_image_resize::Resizer::new();
    loop {
        let Ok(job) = rx.lock().unwrap().recv() else { return };
        // Skip obsolete jobs whose entry was forgotten while waiting in the queue.
        if !matches!(entries.lock().unwrap().get(&job.uri), Some(Entry::Pending)) {
            continue;
        }
        let result = decode_with(&job.bytes, job.cover, job.w, job.h, &mut resizer).map(|mut image| {
            match job.blur {
                Blur::None => {}
                Blur::Bottom => blur_bottom(&mut image),
                Blur::All => blur_all(&mut image),
            }
            image
        });
        let mut map = entries.lock().unwrap();
        // Forgotten meanwhile (another page): don't keep it.
        if matches!(map.get(&job.uri), Some(Entry::Pending)) {
            map.insert(job.uri, match result {
                Ok(image) => Entry::Ready(Arc::new(image), Instant::now()),
                Err(e) => Entry::Failed(e),
            });
            drop(map);
            job.ctx.request_repaint();
        }
    }
}

/// Decode, then shrink so the picture still covers (or fits in) `w`×`h`.
#[cfg(test)]
fn decode(bytes: &[u8], cover: bool, w: u32, h: u32) -> Result<ColorImage, String> {
    let mut resizer = fast_image_resize::Resizer::new();
    decode_with(bytes, cover, w, h, &mut resizer)
}

fn decode_with(
    bytes: &[u8],
    cover: bool,
    w: u32,
    h: u32,
    resizer: &mut fast_image_resize::Resizer,
) -> Result<ColorImage, String> {
    use fast_image_resize::{FilterType, PixelType, ResizeAlg, ResizeOptions, images::{Image, ImageRef}};

    let mut reader = image::ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|e| e.to_string())?;
    if reader.format() == Some(image::ImageFormat::WebP)
        && image::codecs::webp::WebPDecoder::new(std::io::Cursor::new(bytes)).is_ok_and(|d| d.has_animation())
    {
        return Err(ui::widgets::USE_ORIGINAL_IMAGE.into());
    }
    reader.limits(image_limits());
    let img = reader.decode().map_err(|e| e.to_string())?;
    let (iw, ih) = (img.width().max(1), img.height().max(1));
    let (sx, sy) = (w as f32 / iw as f32, h as f32 / ih as f32);
    let scale = if cover { sx.max(sy) } else { sx.min(sy) };
    let rgba = img.into_rgba8();
    if scale < 0.9 {
        let (nw, nh) = (((iw as f32 * scale).ceil() as u32).max(1), ((ih as f32 * scale).ceil() as u32).max(1));
        if let Ok(src) = ImageRef::new(iw, ih, rgba.as_raw(), PixelType::U8x4) {
            let mut dst = Image::new(nw, nh, PixelType::U8x4);
            let opts = ResizeOptions::new()
                .resize_alg(ResizeAlg::Convolution(FilterType::Bilinear))
                .use_alpha(false);
            if resizer.resize(&src, &mut dst, &opts).is_ok() {
                return Ok(ColorImage::from_rgba_unmultiplied([nw as usize, nh as usize], dst.buffer()));
            }
        }
        let fallback = image::imageops::resize(&rgba, nw, nh, image::imageops::FilterType::Triangle);
        return Ok(ColorImage::from_rgba_unmultiplied([nw as usize, nh as usize], fallback.as_raw()));
    }
    Ok(ColorImage::from_rgba_unmultiplied([rgba.width() as usize, rgba.height() as usize], rgba.as_raw()))
}

/// Bake a smooth vertical blur into the existing artwork. Only the small
/// copy needs a Gaussian pass, and both samples use the same coordinates.
fn blur_bottom(image: &mut ColorImage) {
    use egui::Color32;
    use image::imageops::{self, FilterType};
    let [w, h] = image.size;
    let rgba = image::RgbaImage::from_fn(w as u32, h as u32, |x, y| {
        image::Rgba(image.pixels[y as usize * w + x as usize].to_array())
    });
    let small = imageops::resize(&rgba, (w as u32 / 4).max(1), (h as u32 / 4).max(1), FilterType::Triangle);
    let soft = imageops::resize(&imageops::blur(&small, 3.5), w as u32, h as u32, FilterType::Triangle);
    for (y, (sharp, blurred)) in image.pixels.chunks_exact_mut(w).zip(soft.as_raw().chunks_exact(w * 4)).enumerate() {
        let k = ((y as f32 / h as f32 - 0.34) / 0.60).clamp(0.0, 1.0);
        let weight = (256.0 * k * k * (3.0 - 2.0 * k)).round() as u16;
        if weight == 0 {
            continue;
        }
        for (pixel, other) in sharp.iter_mut().zip(blurred.chunks_exact(4)) {
            let from = pixel.to_array();
            let mix = |i: usize| ((u16::from(from[i]) * (256 - weight) + u16::from(other[i]) * weight) / 256) as u8;
            *pixel = Color32::from_rgba_premultiplied(mix(0), mix(1), mix(2), mix(3));
        }
    }
}

/// Blur the whole picture strongly (Steam's `backdrop-filter: blur`), on a
/// quarter-size copy scaled back up.
fn blur_all(image: &mut ColorImage) {
    use image::imageops::{self, FilterType};
    let [w, h] = image.size;
    let rgba = image::RgbaImage::from_fn(w as u32, h as u32, |x, y| image::Rgba(image.pixels[y as usize * w + x as usize].to_array()));
    let small = imageops::resize(&rgba, (w as u32 / 4).max(1), (h as u32 / 4).max(1), FilterType::Triangle);
    let soft = imageops::resize(&imageops::blur(&small, 6.0), w as u32, h as u32, FilterType::Triangle);
    for (pixel, other) in image.pixels.iter_mut().zip(soft.as_raw().chunks_exact(4)) {
        *pixel = egui::Color32::from_rgba_premultiplied(other[0], other[1], other[2], other[3]);
    }
}

impl ImageLoader for SizedLoader {
    fn id(&self) -> &str {
        egui::generate_loader_id!(SizedLoader)
    }

    fn load(&self, ctx: &egui::Context, uri: &str, _: SizeHint) -> ImageLoadResult {
        let (plain, blur) = match (uri.strip_suffix(ui::widgets::BLUR_BOTTOM_SUFFIX), uri.strip_suffix(ui::widgets::BLUR_SUFFIX)) {
            (Some(p), _) => (p, Blur::Bottom),
            (None, Some(p)) => (p, Blur::All),
            (None, None) => (uri, Blur::None),
        };
        let Some((url, cover, w, h)) = ui::widgets::parse_sized(plain) else {
            return Err(LoadError::NotSupported);
        };
        match self.entries.lock().unwrap().get(uri) {
            Some(Entry::Pending) => return Ok(ImagePoll::Pending { size: None }),
            Some(Entry::Ready(image, _)) => return Ok(ImagePoll::Ready { image: image.clone() }),
            Some(Entry::Failed(e)) => return Err(LoadError::Loading(e.clone())),
            None => {}
        }
        match ctx.try_load_bytes(url) {
            Ok(BytesPoll::Ready { bytes, .. }) => {
                let is_anim_webp = bytes.starts_with(b"GIF8") || bytes.windows(4).take(1024).any(|w| w == b"acTL") || bytes.get(8..12) == Some(b"WEBP")
                    && image::codecs::webp::WebPDecoder::new(std::io::Cursor::new(&bytes[..]))
                        .is_ok_and(|d| d.has_animation());
                let job = Job { uri: uri.to_string(), bytes: bytes.clone(), cover, blur, w, h, ctx: ctx.clone() };
                self.entries.lock().unwrap().insert(uri.to_string(), Entry::Pending);
                match self.jobs.lock().unwrap().try_send(job) {
                    Ok(()) => {
                        // The decoder has its own copy now; the disk cache has the file.
                        // Keep raw bytes in the bytes loader only for animated WebP so the
                        // fallback animated loader does not have to re-read from disk.
                        if !is_anim_webp {
                            for loader in ctx.loaders().bytes.lock().iter() {
                                loader.forget(url);
                            }
                        }
                    }
                    Err(TrySendError::Full(_)) => {
                        // Leave retryable on the next frame instead of piling up memory.
                        self.entries.lock().unwrap().remove(uri);
                        ctx.request_repaint_after(Duration::from_millis(33));
                    }
                    Err(TrySendError::Disconnected(_)) => {
                        self.entries.lock().unwrap().insert(uri.to_string(), Entry::Failed("no decoder".into()));
                    }
                }
                Ok(ImagePoll::Pending { size: None })
            }
            Ok(BytesPoll::Pending { size }) => Ok(ImagePoll::Pending { size }),
            Err(e) => Err(e),
        }
    }

    fn end_pass(&self, _pass_index: u64) {
        self.sweep();
    }

    fn forget(&self, uri: &str) {
        self.entries.lock().unwrap().remove(uri);
    }

    fn forget_all(&self) {
        self.entries.lock().unwrap().clear();
    }

    fn byte_size(&self) -> usize {
        self.entries.lock().unwrap().values().map(|e| if let Entry::Ready(i, _) = e { i.pixels.len() * 4 } else { 0 }).sum()
    }

    fn has_pending(&self) -> bool {
        self.entries.lock().unwrap().values().any(|e| matches!(e, Entry::Pending))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn jpeg(w: u32, h: u32) -> Vec<u8> {
        let img = image::RgbImage::from_fn(w, h, |x, y| image::Rgb([(x % 256) as u8, (y % 256) as u8, 128]));
        let mut out = std::io::Cursor::new(Vec::new());
        img.write_to(&mut out, image::ImageFormat::Jpeg).unwrap();
        out.into_inner()
    }

    #[test]
    fn backdrop_keeps_the_top_sharp_and_blurs_the_bottom_in_place() {
        let width = 128;
        let height = 128;
        let mut image = ColorImage::filled([width, height], egui::Color32::BLACK);
        for row in image.pixels.chunks_exact_mut(width) {
            for x in (0..width).step_by(2) {
                row[x] = egui::Color32::WHITE;
            }
        }
        let original = image.clone();
        blur_bottom(&mut image);
        assert_eq!(image.size, original.size);
        assert_eq!(&image.pixels[..width * 40], &original.pixels[..width * 40]);
        let contrast = |y| {
            let i: usize = y * width + width / 2;
            image.pixels[i].r().abs_diff(image.pixels[i + 1].r())
        };
        assert!(contrast(80) < contrast(40));
        assert!(contrast(120) < contrast(80));
    }

    #[test]
    fn resizes_static_webp_and_preserves_animation() {
        let mut encoded = std::io::Cursor::new(Vec::new());
        image::RgbImage::new(128, 64).write_to(&mut encoded, image::ImageFormat::WebP).unwrap();
        assert_eq!(decode(encoded.get_ref(), false, 32, 32).unwrap().size, [32, 16]);
        let animated = b"\
        \x52\x49\x46\x46\x84\x00\x00\x00\x57\x45\x42\x50\x56\x50\x38\x58\
        \x0a\x00\x00\x00\x02\x00\x00\x00\x07\x00\x00\x07\x00\x00\x41\x4e\
        \x49\x4d\x06\x00\x00\x00\x00\x00\x00\x00\x00\x00\x41\x4e\x4d\x46\
        \x28\x00\x00\x00\x00\x00\x00\x00\x00\x00\x07\x00\x00\x07\x00\x00\
        \x64\x00\x00\x02\x56\x50\x38\x4c\x10\x00\x00\x00\x2f\x07\xc0\x01\
        \x00\x07\x10\xe5\x2e\x45\xff\x03\x11\xd1\xff\x00\x41\x4e\x4d\x46\
        \x28\x00\x00\x00\x00\x00\x00\x00\x00\x00\x07\x00\x00\x07\x00\x00\
        \x64\x00\x00\x00\x56\x50\x38\x4c\x0f\x00\x00\x00\x2f\x07\xc0\x01\
        \x00\x07\x10\x51\xee\xfe\x07\x22\xa2\xff\x01\x00";
        assert_eq!(decode(animated, false, 4, 4).unwrap_err(), ui::widgets::USE_ORIGINAL_IMAGE);
    }

    #[test]
    fn shrinks_to_the_drawn_size() {
        let capsule = jpeg(600, 900);
        // Cover 192×384: the width needs 192, the height 384 → 256×384.
        assert_eq!(decode(&capsule, true, 192, 384).unwrap().size, [256, 384]);
        // Contain in 192×384: the width limits → 192×288.
        assert_eq!(decode(&capsule, false, 192, 384).unwrap().size, [192, 288]);
        // Drawn about as large as it is: kept whole.
        assert_eq!(decode(&capsule, true, 600, 900).unwrap().size, [600, 900]);
        assert_eq!(decode(&capsule, true, 2048, 2048).unwrap().size, [600, 900]);
    }

    #[test]
    fn sized_urls_round_trip() {
        let url = "https://shared.steamstatic.com/store_item_assets/steam/apps/10/library_600x900.jpg?t=1";
        let sized = ui::widgets::sized_url(url, egui::vec2(180.0, 270.0), true);
        assert_eq!(ui::widgets::parse_sized(&sized), Some((url, true, 192, 384)));
        // Animations and huge draws stay plain.
        assert_eq!(ui::widgets::sized_url("https://x/a.gif", egui::vec2(10.0, 10.0), true), "https://x/a.gif");
        assert_eq!(ui::widgets::sized_url(url, egui::vec2(4000.0, 100.0), true), url);
    }

    fn png(w: u32, h: u32) -> Vec<u8> {
        let img = image::RgbImage::new(w, h);
        let mut out = std::io::Cursor::new(Vec::new());
        img.write_to(&mut out, image::ImageFormat::Png).unwrap();
        out.into_inner()
    }

    #[test]
    fn decode_rejects_oversized_dimensions_and_alloc() {
        let ok = png(100, 100);
        assert_eq!(decode(&ok, false, 1600, 1200).unwrap().size, [100, 100]);

        // Narrow Workshop strips can exceed 8192 pixels and must be resized.
        let workshop_strip = png(150, 30_393);
        let resized = decode(&workshop_strip, false, 256, 512).unwrap();
        assert!(resized.size[1] <= 512);

        let wide = png(MAX_IMAGE_DIM + 1, 1);
        assert!(decode(&wide, false, 1600, 1200).is_err(), "expected width beyond the image limit to be rejected");

        let tall = png(1, MAX_IMAGE_DIM + 1);
        assert!(decode(&tall, false, 1600, 1200).is_err(), "expected height beyond the image limit to be rejected");

        // 5000x5000 RGB = 75 MB (> 64 MiB max_alloc) within the dimension limit.
        let bomb = png(5000, 5000);
        assert!(decode(&bomb, false, 1600, 1200).is_err(), "expected > 64 MiB allocation to be rejected");
    }

    #[test]
    fn inline_url_wraps_and_caps_through_sized_url() {
        let url = "https://cdn.example/post/banner.png?v=1";
        let wrapped = inline_url(url, 900.0, 700.0, 1.0);
        assert_eq!(ui::widgets::parse_sized(&wrapped), Some((url, false, 1024, 768)));

        // HiDPI scales stay capped at 1600x1200 (step -> 2048x1536) instead of dropping #sir-fit.
        let hidpi = inline_url(url, 900.0, 1200.0, 3.0);
        assert_eq!(ui::widgets::parse_sized(&hidpi), Some((url, false, 2048, 1536)));

        // Zero/tiny widths clamp cleanly without dropping #sir-fit.
        let zero = inline_url("https://cdn.example/shot.jpg", 0.0, 700.0, 1.0);
        assert_eq!(ui::widgets::parse_sized(&zero), Some(("https://cdn.example/shot.jpg", false, 64, 768)));

        // Animations and non-HTTP URIs remain untouched.
        assert_eq!(inline_url("https://cdn.example/anim.gif", 900.0, 700.0, 1.0), "https://cdn.example/anim.gif");
        assert_eq!(inline_url("file:///tmp/x.png", 900.0, 700.0, 1.0), "file:///tmp/x.png");
    }
}
