//! Webcam enumeration and capture.
//!
//! Backed by [nokhwa](https://docs.rs/nokhwa) with the native backend per platform: MediaFoundation on
//! Windows, V4L2 on Linux, AVFoundation on macOS. On other targets every function returns
//! [`MediaError::Unsupported`] (or an empty list).
//!
//! ## Threading
//!
//! nokhwa's capture call blocks until the next frame, and MediaFoundation objects are bound to the COM
//! apartment of the thread that created them. Each [`CameraHandle`] therefore owns a dedicated thread that
//! opens the device, pulls frames, converts them to RGBA and publishes them. Consumers either poll
//! [`CameraHandle::latest`] (UI preview) or [`subscribe`](CameraHandle::subscribe) to a small bounded channel
//! (encoder). Slow subscribers lose frames instead of building up latency. [`list_cameras`] also runs on a
//! short-lived thread so the caller's COM apartment (e.g. an MTA audio thread) does not matter.
//!
//! ## Format negotiation
//!
//! The device's mode list is scored against the requested [`CaptureConfig`]: frame rate matters most (a
//! 720p mode that only does 10 fps loses to 480p30), then resolution distance, then pixel format cost
//! (NV12 < YUYV < MJPEG < RGB < grey). MJPEG is decoded with `zune-jpeg` (pure Rust, SIMD); the raw formats
//! use [`crate::convert`]. Frames arriving faster than the requested rate are dropped before conversion.
//!
//! ## Permissions
//!
//! On macOS, [`Camera::start`] first asks for camera access (the system prompt is shown once; the app bundle
//! needs `NSCameraUsageDescription`). [`request_permission`] can be called earlier, e.g. from settings.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender, TrySendError};
use std::thread::JoinHandle;
use std::time::Duration;

use parking_lot::Mutex;

use crate::error::{MediaError, Result};
use crate::frame::VideoFrame;

/// A camera found by [`list_cameras`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CameraInfo {
    /// Index to pass to [`Camera::start`] (backend-specific; on Linux it is the `/dev/videoN` number).
    pub index: u32,
    /// Human-readable device name.
    pub name: String,
    /// Backend-provided description (may be empty).
    pub description: String,
}

/// Requested capture parameters. The closest mode the camera supports is used; see
/// [`CameraHandle::format`] for what was negotiated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CaptureConfig {
    /// Desired width in pixels.
    pub width: u32,
    /// Desired height in pixels.
    pub height: u32,
    /// Desired frames per second; frames beyond this rate are dropped.
    pub fps: u32,
}

impl Default for CaptureConfig {
    fn default() -> Self {
        Self { width: 1280, height: 720, fps: 30 }
    }
}

/// Pixel format delivered by the camera (before conversion to RGBA).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum PixelFormat {
    /// Motion JPEG.
    Mjpeg,
    /// Packed YUV 4:2:2.
    Yuyv,
    /// Semi-planar YUV 4:2:0.
    Nv12,
    /// Packed 24-bit RGB.
    Rgb24,
    /// Packed 24-bit BGR.
    Bgr24,
    /// 8-bit greyscale.
    Gray,
}

/// The mode the camera actually runs in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ActiveFormat {
    /// Frame width.
    pub width: u32,
    /// Frame height.
    pub height: u32,
    /// Frame rate reported by the device.
    pub fps: u32,
    /// Wire pixel format.
    pub format: PixelFormat,
}

/// Lists the cameras attached to the system. Returns an empty list (and logs) on any backend error.
pub fn list_cameras() -> Vec<CameraInfo> {
    let spawned = std::thread::Builder::new().name("ferrocord-camera-query".into()).spawn(|| {
        imp::thread_init();
        std::panic::catch_unwind(imp::query)
    });
    match spawned.map(|handle| handle.join()) {
        Ok(Ok(Ok(Ok(list)))) => list,
        Ok(Ok(Ok(Err(e)))) => {
            tracing::warn!(error = %e, "camera enumeration failed");
            Vec::new()
        }
        _ => {
            tracing::warn!("camera enumeration panicked or could not start a thread");
            Vec::new()
        }
    }
}

/// Requests camera access from the OS. Blocks until the user answers the macOS prompt (up to 60 s), so call it
/// off the UI thread. Always `Ok` on Windows and Linux, where access is not gated by a prompt.
pub fn request_permission() -> Result<()> {
    imp::request_permission()
}

/// Entry point for starting a capture. See [`Camera::start`].
#[derive(Debug)]
pub struct Camera;

impl Camera {
    /// Opens camera `index` (from [`list_cameras`]) and starts capturing on a dedicated thread.
    ///
    /// Returns once the device is streaming, or with [`MediaError::NoCamera`], [`MediaError::PermissionDenied`]
    /// or [`MediaError::Camera`] if it cannot be opened. Capture stops when the handle is dropped.
    pub fn start(index: u32, config: CaptureConfig) -> Result<CameraHandle> {
        imp::request_permission()?;
        let config = CaptureConfig { width: config.width.max(1), height: config.height.max(1), fps: config.fps.clamp(1, 240) };
        let shared = Arc::new(Shared::default());
        let (ready_tx, ready_rx) = mpsc::sync_channel::<Result<ActiveFormat>>(1);
        let (exit_tx, exit_rx) = mpsc::channel::<()>();
        let thread_shared = shared.clone();
        let thread = std::thread::Builder::new()
            .name(format!("ferrocord-camera-{index}"))
            .spawn(move || {
                let _exit = ExitSignal(exit_tx, thread_shared.clone());
                imp::thread_init();
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| imp::run(index, config, &thread_shared, &ready_tx)));
                if result.is_err() {
                    thread_shared.set_error("camera backend panicked".into());
                    let _ = ready_tx.try_send(Err(MediaError::Camera("camera backend panicked".into())));
                }
            })
            .map_err(|e| MediaError::Camera(format!("failed to spawn capture thread: {e}")))?;
        let format = match ready_rx.recv() {
            Ok(Ok(format)) => format,
            Ok(Err(e)) => {
                let _ = thread.join();
                return Err(e);
            }
            Err(_) => {
                let _ = thread.join();
                return Err(MediaError::Camera(shared.error.lock().clone().unwrap_or_else(|| "capture thread exited".into())));
            }
        };
        Ok(CameraHandle { shared, thread: Some(thread), exited: exit_rx, format })
    }
}

/// A running capture. Dropping it stops the camera.
#[derive(Debug)]
pub struct CameraHandle {
    shared: Arc<Shared>,
    thread: Option<JoinHandle<()>>,
    exited: Receiver<()>,
    format: ActiveFormat,
}

impl CameraHandle {
    /// The negotiated camera mode.
    pub fn format(&self) -> ActiveFormat {
        self.format
    }

    /// The most recent frame, if any has been captured yet. Cheap (clones an `Arc`).
    pub fn latest(&self) -> Option<VideoFrame> {
        self.shared.latest.lock().clone()
    }

    /// Returns a receiver that gets every captured frame. The channel holds at most two frames; when the
    /// consumer falls behind, new frames are dropped for it. Dropping the receiver unsubscribes.
    pub fn subscribe(&self) -> Receiver<VideoFrame> {
        let (tx, rx) = mpsc::sync_channel(2);
        self.shared.subscribers.lock().push(tx);
        rx
    }

    /// Number of frames published so far.
    pub fn frames_captured(&self) -> u64 {
        self.shared.frames.load(Ordering::Relaxed)
    }

    /// `false` once the capture thread has stopped (device unplugged, repeated errors, or [`stop`](Self::stop)).
    pub fn is_running(&self) -> bool {
        self.shared.running.load(Ordering::Acquire)
    }

    /// The error that stopped the capture, if any.
    pub fn last_error(&self) -> Option<String> {
        self.shared.error.lock().clone()
    }

    /// Stops capturing and releases the device (same as dropping the handle).
    pub fn stop(self) {}
}

impl Drop for CameraHandle {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Release);
        // The capture thread notices the flag after its current (blocking) frame read. Don't hang the caller
        // forever if a driver never returns.
        match self.exited.recv_timeout(Duration::from_secs(3)) {
            Ok(()) | Err(mpsc::RecvTimeoutError::Disconnected) => {
                if let Some(thread) = self.thread.take() {
                    let _ = thread.join();
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => tracing::warn!("camera thread did not stop within 3 s; detaching it"),
        }
    }
}

#[derive(Debug)]
struct Shared {
    latest: Mutex<Option<VideoFrame>>,
    subscribers: Mutex<Vec<SyncSender<VideoFrame>>>,
    error: Mutex<Option<String>>,
    stop: AtomicBool,
    running: AtomicBool,
    frames: AtomicU64,
}

impl Default for Shared {
    fn default() -> Self {
        Self {
            latest: Mutex::new(None),
            subscribers: Mutex::new(Vec::new()),
            error: Mutex::new(None),
            stop: AtomicBool::new(false),
            running: AtomicBool::new(true),
            frames: AtomicU64::new(0),
        }
    }
}

impl Shared {
    fn publish(&self, frame: VideoFrame) {
        self.subscribers.lock().retain(|tx| !matches!(tx.try_send(frame.clone()), Err(TrySendError::Disconnected(_))));
        *self.latest.lock() = Some(frame);
        self.frames.fetch_add(1, Ordering::Relaxed);
    }

    fn set_error(&self, error: String) {
        tracing::warn!(%error, "camera capture stopped");
        *self.error.lock() = Some(error);
    }
}

/// Signals the owning handle (and clears `running`) when the capture thread ends, even by panic.
struct ExitSignal(mpsc::Sender<()>, Arc<Shared>);

impl Drop for ExitSignal {
    fn drop(&mut self) {
        self.1.running.store(false, Ordering::Release);
        let _ = self.0.send(());
    }
}

#[cfg(any(target_os = "windows", target_os = "linux", target_os = "macos"))]
mod imp {
    use std::sync::Arc;
    use std::sync::atomic::Ordering;
    use std::sync::mpsc::SyncSender;
    use std::time::{Duration, Instant};

    use nokhwa::Buffer;
    use nokhwa::utils::{ApiBackend, CameraFormat, CameraIndex, FrameFormat, RequestedFormat, RequestedFormatType};
    use zune_jpeg::JpegDecoder;
    use zune_jpeg::zune_core::bytestream::ZCursor;
    use zune_jpeg::zune_core::colorspace::ColorSpace;
    use zune_jpeg::zune_core::options::DecoderOptions;

    use super::{ActiveFormat, CameraInfo, CaptureConfig, PixelFormat, Shared};
    use crate::convert;
    use crate::error::{MediaError, Result};
    use crate::frame::{MAX_DIMENSION, VideoFrame};

    const FORMATS: &[FrameFormat] = &[FrameFormat::NV12, FrameFormat::YUYV, FrameFormat::MJPEG, FrameFormat::RAWRGB, FrameFormat::RAWBGR, FrameFormat::GRAY];

    /// Stop after this many consecutive capture errors (device unplugged).
    const MAX_CONSECUTIVE_ERRORS: u32 = 50;

    fn camera_err(e: nokhwa::NokhwaError) -> MediaError {
        MediaError::Camera(e.to_string())
    }

    /// Per-thread platform setup.
    pub(super) fn thread_init() {
        #[cfg(target_os = "windows")]
        {
            #[link(name = "ole32")]
            unsafe extern "system" {
                fn CoInitializeEx(reserved: *const core::ffi::c_void, coinit: u32) -> i32;
            }
            // COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE: the flags nokhwa uses. nokhwa only initialises COM
            // on the first thread that touches MediaFoundation (process-global flag), so every thread of ours does
            // it explicitly; nokhwa's own call then just returns S_FALSE. The thread exits soon after, which
            // releases the apartment.
            // SAFETY: plain Win32 call with a null reserved pointer, as documented.
            let _ = unsafe { CoInitializeEx(std::ptr::null(), 0x2 | 0x4) };
        }
    }

    pub(super) fn query() -> Result<Vec<CameraInfo>> {
        let devices = nokhwa::query(ApiBackend::Auto).map_err(camera_err)?;
        Ok(devices
            .into_iter()
            .enumerate()
            .map(|(pos, d)| CameraInfo { index: d.index().as_index().unwrap_or(pos as u32), name: d.human_name(), description: d.description().to_string() })
            .collect())
    }

    #[cfg(target_os = "macos")]
    pub(super) fn request_permission() -> Result<()> {
        if nokhwa::nokhwa_check() {
            return Ok(());
        }
        let (tx, rx) = std::sync::mpsc::channel();
        let tx = parking_lot::Mutex::new(tx);
        nokhwa::nokhwa_initialize(move |granted| {
            let _ = tx.lock().send(granted);
        });
        match rx.recv_timeout(Duration::from_secs(60)) {
            Ok(true) => Ok(()),
            _ => Err(MediaError::PermissionDenied),
        }
    }

    #[cfg(not(target_os = "macos"))]
    pub(super) fn request_permission() -> Result<()> {
        Ok(())
    }

    fn pixel_format(f: FrameFormat) -> PixelFormat {
        match f {
            FrameFormat::MJPEG => PixelFormat::Mjpeg,
            FrameFormat::YUYV => PixelFormat::Yuyv,
            FrameFormat::NV12 => PixelFormat::Nv12,
            FrameFormat::RAWRGB => PixelFormat::Rgb24,
            FrameFormat::RAWBGR => PixelFormat::Bgr24,
            FrameFormat::GRAY => PixelFormat::Gray,
        }
    }

    fn format_cost(f: FrameFormat) -> u32 {
        match f {
            FrameFormat::NV12 => 0,
            FrameFormat::YUYV => 1,
            FrameFormat::MJPEG => 2,
            FrameFormat::RAWRGB | FrameFormat::RAWBGR => 3,
            FrameFormat::GRAY => 4,
        }
    }

    /// Picks the mode closest to `want`, valuing frame rate over resolution.
    pub(super) fn pick_format(formats: &[CameraFormat], want: &CaptureConfig) -> Option<CameraFormat> {
        formats.iter().filter(|f| FORMATS.contains(&f.format()) && f.width() > 0 && f.height() > 0).copied().min_by_key(|f| {
            let res = f.width().abs_diff(want.width) + f.height().abs_diff(want.height);
            let fps = if f.frame_rate() >= want.fps { (f.frame_rate() - want.fps) * 2 } else { (want.fps - f.frame_rate()) * 60 };
            (res.saturating_add(fps), format_cost(f.format()))
        })
    }

    pub(super) fn run(index: u32, config: CaptureConfig, shared: &Arc<Shared>, ready: &SyncSender<Result<ActiveFormat>>) {
        let mut camera = match open(index, &config) {
            Ok(camera) => camera,
            Err(e) => {
                let _ = ready.send(Err(e));
                return;
            }
        };
        let fmt = camera.camera_format();
        let active = ActiveFormat { width: fmt.width(), height: fmt.height(), fps: fmt.frame_rate(), format: pixel_format(fmt.format()) };
        tracing::info!(index, ?active, "camera started");
        let _ = ready.send(Ok(active));

        let min_interval = Duration::from_secs_f64(0.85 / f64::from(config.fps));
        let mut last_published: Option<Instant> = None;
        let mut errors = 0u32;
        while !shared.stop.load(Ordering::Acquire) {
            match camera.frame() {
                Ok(buffer) => {
                    errors = 0;
                    let now = Instant::now();
                    if last_published.is_some_and(|last| now.duration_since(last) < min_interval) {
                        continue;
                    }
                    match to_frame(&buffer, now) {
                        Ok(frame) => {
                            last_published = Some(now);
                            shared.publish(frame);
                        }
                        Err(e) => tracing::debug!(error = %e, "dropping undecodable camera frame"),
                    }
                }
                Err(e) => {
                    errors += 1;
                    if errors >= MAX_CONSECUTIVE_ERRORS {
                        shared.set_error(e.to_string());
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(20));
                }
            }
        }
        if let Err(e) = camera.stop_stream() {
            tracing::debug!(error = %e, "stop_stream failed");
        }
    }

    fn open(index: u32, config: &CaptureConfig) -> Result<nokhwa::Camera> {
        match query() {
            Ok(list) if list.is_empty() => return Err(MediaError::NoCamera(None)),
            Ok(list) if !list.iter().any(|c| c.index == index) => return Err(MediaError::NoCamera(Some(format!("no camera with index {index}")))),
            Ok(_) => {}
            Err(e) => tracing::debug!(error = %e, "camera query failed; trying to open anyway"),
        }
        let mut camera =
            nokhwa::Camera::new(CameraIndex::Index(index), RequestedFormat::with_formats(RequestedFormatType::None, FORMATS)).map_err(camera_err)?;
        match camera.compatible_camera_formats() {
            Ok(formats) => {
                if let Some(best) = pick_format(&formats, config)
                    && let Err(e) = camera.set_camera_requset(RequestedFormat::with_formats(RequestedFormatType::Exact(best), FORMATS))
                {
                    tracing::warn!(error = %e, ?best, "camera rejected the preferred mode; using its default");
                }
            }
            Err(e) => tracing::debug!(error = %e, "camera does not list its modes; using its default"),
        }
        camera.open_stream().map_err(camera_err)?;
        Ok(camera)
    }

    fn to_frame(buffer: &Buffer, timestamp: Instant) -> Result<VideoFrame> {
        let resolution = buffer.resolution();
        let (w, h) = (resolution.width(), resolution.height());
        let src = buffer.buffer();
        let mut out = Vec::new();
        match buffer.source_frame_format() {
            FrameFormat::MJPEG => return decode_mjpeg(src, timestamp),
            FrameFormat::YUYV => convert::yuyv_to_rgba(src, w, h, &mut out)?,
            FrameFormat::NV12 => convert::nv12_to_rgba(src, w, h, &mut out)?,
            FrameFormat::RAWRGB => convert::rgb24_to_rgba(src, w, h, &mut out)?,
            FrameFormat::RAWBGR => convert::bgr24_to_rgba(src, w, h, &mut out)?,
            FrameFormat::GRAY => convert::gray_to_rgba(src, w, h, &mut out)?,
        }
        VideoFrame::with_timestamp(w, h, out, timestamp)
    }

    /// Decodes one MJPEG frame (webcams often omit the Huffman tables; zune-jpeg supplies the defaults).
    pub(super) fn decode_mjpeg(src: &[u8], timestamp: Instant) -> Result<VideoFrame> {
        let decoded = std::panic::catch_unwind(|| {
            let options = DecoderOptions::default()
                .jpeg_set_out_colorspace(ColorSpace::RGBA)
                .set_max_width(MAX_DIMENSION as usize)
                .set_max_height(MAX_DIMENSION as usize);
            let mut decoder = JpegDecoder::new_with_options(ZCursor::new(src), options);
            let pixels = decoder.decode().map_err(|e| MediaError::Camera(format!("MJPEG decode failed: {e:?}")))?;
            let info = decoder.info().ok_or_else(|| MediaError::Camera("MJPEG without header".into()))?;
            Ok::<_, MediaError>((u32::from(info.width), u32::from(info.height), pixels))
        })
        .map_err(|_| MediaError::Camera("MJPEG decoder panicked".into()))?;
        let (w, h, pixels) = decoded?;
        VideoFrame::with_timestamp(w, h, pixels, timestamp)
    }
}

#[cfg(not(any(target_os = "windows", target_os = "linux", target_os = "macos")))]
mod imp {
    use std::sync::Arc;
    use std::sync::mpsc::SyncSender;

    use super::{ActiveFormat, CameraInfo, CaptureConfig, Shared};
    use crate::error::{MediaError, Result};

    pub(super) fn thread_init() {}

    pub(super) fn query() -> Result<Vec<CameraInfo>> {
        Ok(Vec::new())
    }

    pub(super) fn request_permission() -> Result<()> {
        Ok(())
    }

    pub(super) fn run(_index: u32, _config: CaptureConfig, _shared: &Arc<Shared>, ready: &SyncSender<Result<ActiveFormat>>) {
        let _ = ready.send(Err(MediaError::Unsupported("camera capture is not available on this platform".into())));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn list_cameras_does_not_panic() {
        let cameras = list_cameras();
        for camera in &cameras {
            assert!(!camera.name.is_empty() || camera.description.is_empty() || camera.index < u32::MAX);
        }
        eprintln!("cameras: {cameras:?}");
    }

    #[test]
    fn missing_camera_is_an_error() {
        match Camera::start(9_999, CaptureConfig::default()) {
            Err(MediaError::NoCamera(_) | MediaError::Camera(_) | MediaError::Unsupported(_) | MediaError::PermissionDenied) => {}
            Err(e) => panic!("unexpected error {e}"),
            Ok(_) => panic!("camera 9999 should not exist"),
        }
    }

    #[cfg(any(target_os = "windows", target_os = "linux", target_os = "macos"))]
    #[test]
    fn mode_selection_prefers_frame_rate_then_size_then_cheap_formats() {
        use nokhwa::utils::{CameraFormat, FrameFormat};
        let modes = [
            CameraFormat::new_from(1280, 720, FrameFormat::YUYV, 10),
            CameraFormat::new_from(1280, 720, FrameFormat::MJPEG, 30),
            CameraFormat::new_from(640, 480, FrameFormat::YUYV, 30),
            CameraFormat::new_from(1920, 1080, FrameFormat::MJPEG, 30),
            CameraFormat::new_from(1280, 720, FrameFormat::NV12, 30),
        ];
        let want = CaptureConfig::default();
        assert_eq!(imp::pick_format(&modes, &want), Some(modes[4]));
        assert_eq!(imp::pick_format(&modes[..4], &want), Some(modes[1]));
        assert_eq!(imp::pick_format(&modes[..1], &want), Some(modes[0]));
        assert_eq!(imp::pick_format(&[], &want), None);
    }

    #[cfg(any(target_os = "windows", target_os = "linux", target_os = "macos"))]
    #[test]
    fn corrupt_mjpeg_is_an_error() {
        let now = std::time::Instant::now();
        assert!(imp::decode_mjpeg(&[], now).is_err());
        assert!(imp::decode_mjpeg(&[0xff, 0xd8, 0xff, 0xdb, 0, 3, 1], now).is_err());
        assert!(imp::decode_mjpeg(&[0x42; 1000], now).is_err());
    }

    /// Needs a real webcam: `cargo test -p ferrocord-media -- --ignored camera_captures_frames`.
    #[test]
    #[ignore = "requires camera hardware"]
    fn camera_captures_frames() {
        let cameras = list_cameras();
        let first = cameras.first().expect("no camera attached");
        let handle = Camera::start(first.index, CaptureConfig { width: 640, height: 480, fps: 30 }).expect("start camera");
        eprintln!("negotiated {:?}", handle.format());
        let rx = handle.subscribe();
        let frame = rx.recv_timeout(Duration::from_secs(5)).expect("no frame within 5 s");
        frame.validate().unwrap();
        assert!(handle.latest().is_some());
        eprintln!("got {}x{} frame; captured {}", frame.width, frame.height, handle.frames_captured());
        handle.stop();
    }
}
