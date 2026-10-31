//! Asynchronous image loading with a disk cache, off-thread decoding,
//! animated GIF/WebP support and automatic texture eviction.

use std::collections::HashMap;
use std::io::Cursor;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use egui::{ColorImage, TextureHandle, TextureOptions, Vec2};
use image::AnimationDecoder;
use parking_lot::Mutex;
use sha2::{Digest, Sha256};
use tokio::sync::Semaphore;

const MAX_FRAMES: usize = 150;
const MAX_DIM: u32 = 2048;
const EVICT_AFTER: Duration = Duration::from_secs(90);

enum Entry {
    Loading,
    Ready(Frames),
    Failed,
}

struct Frames {
    frames: Vec<(TextureHandle, Duration)>,
    total: Duration,
    size: Vec2,
    last_used: Instant,
}

/// A drawable image frame.
#[derive(Clone)]
pub struct Img {
    pub texture: egui::TextureId,
    pub size: Vec2,
}

impl Img {
    pub fn sized(&self, max: Vec2) -> egui::load::SizedTexture {
        let scale = (max.x / self.size.x).min(max.y / self.size.y).min(1.0);
        egui::load::SizedTexture::new(self.texture, self.size * scale)
    }
}

#[derive(Clone)]
pub struct ImageCache {
    inner: Arc<Inner>,
}

struct Inner {
    ctx: egui::Context,
    http: reqwest::Client,
    rt: tokio::runtime::Handle,
    dir: PathBuf,
    entries: Mutex<HashMap<String, Entry>>,
    permits: Arc<Semaphore>,
    last_evict: Mutex<Instant>,
    animate: Mutex<bool>,
}

impl ImageCache {
    pub fn new(ctx: egui::Context, http: reqwest::Client, rt: tokio::runtime::Handle, dir: PathBuf) -> Self {
        let _ = std::fs::create_dir_all(&dir);
        Self {
            inner: Arc::new(Inner {
                ctx,
                http,
                rt,
                dir,
                entries: Mutex::new(HashMap::new()),
                permits: Arc::new(Semaphore::new(8)),
                last_evict: Mutex::new(Instant::now()),
                animate: Mutex::new(true),
            }),
        }
    }

    pub fn set_animate(&self, on: bool) {
        *self.inner.animate.lock() = on;
    }

    /// Returns the image (current animation frame) if loaded; starts loading otherwise.
    pub fn get(&self, url: &str) -> Option<Img> {
        let mut entries = self.inner.entries.lock();
        match entries.get_mut(url) {
            Some(Entry::Ready(f)) => {
                f.last_used = Instant::now();
                let (tex, delay_left) = if f.frames.len() > 1 && *self.inner.animate.lock() && f.total > Duration::ZERO {
                    let now = self.inner.ctx.input(|i| i.time);
                    let t = Duration::from_secs_f64(now % f.total.as_secs_f64());
                    let mut acc = Duration::ZERO;
                    let mut pick = (&f.frames[0].0, f.frames[0].1);
                    for (tex, d) in &f.frames {
                        acc += *d;
                        if t < acc {
                            pick = (tex, acc - t);
                            break;
                        }
                    }
                    (pick.0, Some(pick.1))
                } else {
                    (&f.frames[0].0, None)
                };
                if let Some(d) = delay_left {
                    self.inner.ctx.request_repaint_after(d);
                }
                Some(Img { texture: tex.id(), size: f.size })
            }
            Some(Entry::Loading) | Some(Entry::Failed) => None,
            None if url.starts_with(DEMO_SCHEME) => {
                // Offline demo pictures are drawn locally (no network in --demo or tests).
                drop(entries);
                let tex = self.inner.ctx.load_texture(url, demo_picture(url), TextureOptions::LINEAR);
                let size = tex.size_vec2();
                let id = tex.id();
                self.inner.entries.lock().insert(
                    url.to_string(),
                    Entry::Ready(Frames { frames: vec![(tex, Duration::ZERO)], total: Duration::ZERO, size, last_used: Instant::now() }),
                );
                Some(Img { texture: id, size })
            }
            None => {
                entries.insert(url.to_string(), Entry::Loading);
                drop(entries);
                self.spawn_load(url.to_string());
                None
            }
        }
    }

    pub fn is_failed(&self, url: &str) -> bool {
        matches!(self.inner.entries.lock().get(url), Some(Entry::Failed))
    }

    /// Drops textures that haven't been drawn recently. Call once per frame.
    pub fn maintain(&self) {
        let mut last = self.inner.last_evict.lock();
        if last.elapsed() < Duration::from_secs(15) {
            return;
        }
        *last = Instant::now();
        self.inner.entries.lock().retain(|_, e| match e {
            Entry::Ready(f) => f.last_used.elapsed() < EVICT_AFTER,
            Entry::Failed => true,
            Entry::Loading => true,
        });
    }

    /// Inserts an image built locally (e.g. a QR code) under a synthetic key.
    pub fn insert_local(&self, key: &str, img: ColorImage) {
        let size = Vec2::new(img.size[0] as f32, img.size[1] as f32);
        let tex = self.inner.ctx.load_texture(key, img, TextureOptions::NEAREST);
        self.inner
            .entries
            .lock()
            .insert(key.to_string(), Entry::Ready(Frames { frames: vec![(tex, Duration::ZERO)], total: Duration::ZERO, size, last_used: Instant::now() }));
    }

    fn cache_path(&self, url: &str) -> PathBuf {
        let h = Sha256::digest(url.as_bytes());
        let name: String = h.iter().take(16).map(|b| format!("{b:02x}")).collect();
        self.inner.dir.join(&name[..2]).join(name)
    }

    fn spawn_load(&self, url: String) {
        let this = self.clone();
        self.inner.rt.spawn(async move {
            let _permit = this.inner.permits.clone().acquire_owned().await;
            let path = this.cache_path(&url);
            let bytes = match tokio::fs::read(&path).await {
                Ok(b) => Some(b),
                Err(_) => match this.inner.http.get(&url).send().await {
                    Ok(r) if r.status().is_success() => match r.bytes().await {
                        Ok(b) => {
                            let b = b.to_vec();
                            if let Some(dir) = path.parent() {
                                let _ = tokio::fs::create_dir_all(dir).await;
                            }
                            let _ = tokio::fs::write(&path, &b).await;
                            Some(b)
                        }
                        Err(_) => None,
                    },
                    _ => None,
                },
            };
            let ctx = this.inner.ctx.clone();
            let key = url.clone();
            let decoded = match bytes {
                Some(b) => tokio::task::spawn_blocking(move || decode(&ctx, &key, &b)).await.ok().flatten(),
                None => None,
            };
            let entry = match decoded {
                Some(f) => Entry::Ready(f),
                None => {
                    tracing::debug!(%url, "image failed to load");
                    Entry::Failed
                }
            };
            this.inner.entries.lock().insert(url, entry);
            this.inner.ctx.request_repaint();
        });
    }
}

fn to_color_image(img: &image::RgbaImage) -> ColorImage {
    ColorImage::from_rgba_unmultiplied([img.width() as usize, img.height() as usize], img.as_raw())
}

fn shrink(img: image::RgbaImage) -> image::RgbaImage {
    if img.width() <= MAX_DIM && img.height() <= MAX_DIM {
        return img;
    }
    let scale = MAX_DIM as f32 / img.width().max(img.height()) as f32;
    image::imageops::resize(&img, (img.width() as f32 * scale) as u32, (img.height() as f32 * scale) as u32, image::imageops::FilterType::Triangle)
}

fn decode(ctx: &egui::Context, key: &str, bytes: &[u8]) -> Option<Frames> {
    let fmt = image::guess_format(bytes).ok()?;
    let opts = TextureOptions::LINEAR;
    let frames: Vec<(image::RgbaImage, Duration)> = match fmt {
        image::ImageFormat::Gif => {
            let dec = image::codecs::gif::GifDecoder::new(Cursor::new(bytes)).ok()?;
            collect_frames(dec.into_frames())
        }
        image::ImageFormat::WebP => {
            let dec = image::codecs::webp::WebPDecoder::new(Cursor::new(bytes)).ok()?;
            if dec.has_animation() {
                collect_frames(dec.into_frames())
            } else {
                vec![(image::load_from_memory_with_format(bytes, fmt).ok()?.to_rgba8(), Duration::ZERO)]
            }
        }
        _ => vec![(image::load_from_memory_with_format(bytes, fmt).ok()?.to_rgba8(), Duration::ZERO)],
    };
    let first = frames.first()?;
    let size = Vec2::new(first.0.width() as f32, first.0.height() as f32);
    let mut total = Duration::ZERO;
    let mut out = Vec::with_capacity(frames.len());
    for (i, (img, d)) in frames.into_iter().enumerate() {
        let img = shrink(img);
        // Browsers clamp very small GIF delays to 100ms; do the same.
        let d = if d < Duration::from_millis(20) { Duration::from_millis(100) } else { d };
        total += d;
        out.push((ctx.load_texture(format!("{key}#{i}"), to_color_image(&img), opts), d));
    }
    if out.len() == 1 {
        total = Duration::ZERO;
    }
    Some(Frames { frames: out, total, size, last_used: Instant::now() })
}

fn collect_frames(frames: image::Frames<'_>) -> Vec<(image::RgbaImage, Duration)> {
    frames
        .take(MAX_FRAMES)
        .filter_map(Result::ok)
        .map(|f| {
            let (n, d) = f.delay().numer_denom_ms();
            let ms = n.checked_div(d).unwrap_or(100);
            (f.into_buffer(), Duration::from_millis(ms as u64))
        })
        .collect()
}

/// URL scheme of the generated pictures used by `--demo`.
pub const DEMO_SCHEME: &str = "demo://";

/// A soft sunset landscape, sized from `width=`/`height=` in the URL (as requested
/// through the media proxy), so the demo shows real-looking image attachments.
fn demo_picture(url: &str) -> ColorImage {
    let param = |k: &str| url.split(['?', '&']).find_map(|p| p.strip_prefix(k)).and_then(|v| v.parse::<usize>().ok());
    let (w, h) = (param("width=").unwrap_or(800).clamp(8, 1600), param("height=").unwrap_or(450).clamp(8, 1600));
    let mut px = Vec::with_capacity(w * h);
    let lerp = |a: f32, b: f32, t: f32| a + (b - a) * t;
    for y in 0..h {
        let fy = y as f32 / h as f32;
        for x in 0..w {
            let fx = x as f32 / w as f32;
            // Sky: deep blue to warm orange towards the horizon.
            let t = (fy / 0.62).min(1.0);
            let (mut r, mut g, mut b) = (lerp(38.0, 250.0, t), lerp(52.0, 150.0, t * t), lerp(110.0, 90.0, t));
            // Sun.
            let d = ((fx - 0.62).powi(2) * 3.0 + (fy - 0.55).powi(2) * 9.0).sqrt();
            let glow = (1.0 - d * 2.2).clamp(0.0, 1.0);
            r = lerp(r, 255.0, glow);
            g = lerp(g, 214.0, glow);
            b = lerp(b, 140.0, glow);
            // Hills.
            let hill = 0.66 + 0.06 * (fx * 7.0).sin() + 0.03 * (fx * 17.0 + 1.0).sin();
            if fy > hill {
                let k = ((fy - hill) * 4.0).min(1.0);
                r = lerp(70.0, 25.0, k);
                g = lerp(40.0, 20.0, k);
                b = lerp(70.0, 40.0, k);
            }
            px.push(egui::Color32::from_rgb(r as u8, g as u8, b as u8));
        }
    }
    ColorImage::new([w, h], px)
}
