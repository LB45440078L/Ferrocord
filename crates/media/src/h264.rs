//! H.264 encoding and decoding with Cisco's OpenH264 (built from the bundled source).
//!
//! ## WebRTC compatibility
//!
//! Discord's SFU forwards our stream to browsers, so the encoder is pinned to what every WebRTC endpoint
//! decodes: **Constrained Baseline** profile (no B-frames, CAVLC, no FMO/ASO), level 3.1 up to 1280×720
//! (`profile-level-id=42e01f`) and a higher level only for bigger frames, Annex-B output, SPS/PPS repeated in
//! front of every IDR so a PLI-triggered keyframe is self-contained, and size-limited slices (see
//! [`EncoderConfig::max_slice_bytes`]; slices larger than a packet are fragmented with FU-A,
//! packetization-mode=1). OpenH264 writes `constraint_set1_flag` for its
//! baseline profile, which is what makes it *constrained* baseline (`42e0..`); a unit test checks the SPS.
//!
//! ## Frame size changes
//!
//! The encoder output size is fixed by [`EncoderConfig`]; input frames of a different size are scaled to it
//! (bilinear / area averaging), which keeps the resolution the receivers see stable while the camera
//! renegotiates. Use [`H264Encoder::set_resolution`] to change the output size explicitly (forces an IDR).
//!
//! ## Licensing
//!
//! See the crate-level documentation: OpenH264 is BSD-2-Clause but H.264 is patent-encumbered when shipping
//! a self-compiled build. Disable the `h264` feature if that is a concern.

use std::sync::Arc;
use std::time::Instant;

use openh264::OpenH264API;
use openh264::decoder::{Decoder, DecoderConfig};
use openh264::encoder::{BitRate, Complexity, Encoder, FrameRate, FrameType, IntraFramePeriod, Level, Profile, RateControlMode, SpsPpsStrategy, UsageType};
use openh264::formats::YUVSource;

use crate::convert;
use crate::error::{MediaError, Result};
use crate::frame::{I420Frame, MAX_DIMENSION, VideoFrame};

/// Largest frame OpenH264 can encode (its level 5.2 limit), either orientation.
pub const MAX_ENCODE_WIDTH: u32 = 3840;
/// See [`MAX_ENCODE_WIDTH`].
pub const MAX_ENCODE_HEIGHT: u32 = 2160;

/// Smallest accepted [`EncoderConfig::max_slice_bytes`] (other non-zero values are raised to it).
pub const MIN_SLICE_BYTES: u32 = 420;

/// Encoder settings.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EncoderConfig {
    /// Output width in pixels (rounded down to even, min 16).
    pub width: u32,
    /// Output height in pixels (rounded down to even, min 16).
    pub height: u32,
    /// Target frame rate, used by rate control.
    pub fps: f32,
    /// Target bitrate in bits per second.
    pub bitrate_bps: u32,
    /// Emit an IDR every this many frames; `0` disables periodic keyframes (keyframes then only come from
    /// [`H264Encoder::force_keyframe`], i.e. on PLI/FIR).
    pub keyframe_interval: u32,
    /// Upper bound for a slice NAL unit in bytes (at least [`MIN_SLICE_BYTES`]); `0` encodes each frame as a
    /// single slice.
    ///
    /// Multiple slices are what lets OpenH264 use several threads (about 2.5x faster at 720p on a 12-core machine
    /// without NASM), and a value just below the RTP payload budget makes every slice a single-NAL packet, so
    /// one lost packet costs one slice instead of a whole FU-A chain. Slightly lower compression efficiency.
    pub max_slice_bytes: u32,
}

impl Default for EncoderConfig {
    /// 1280×720 @ 30 fps, 2.5 Mbit/s, an IDR every 10 s as a safety net for lost PLIs, slices of at most
    /// 1100 bytes (fits [`crate::rtp_h264::DEFAULT_MAX_PAYLOAD_SIZE`]).
    fn default() -> Self {
        Self { width: 1280, height: 720, fps: 30.0, bitrate_bps: 2_500_000, keyframe_interval: 300, max_slice_bytes: 1100 }
    }
}

impl EncoderConfig {
    fn normalized(mut self) -> Result<Self> {
        self.width = (self.width & !1).max(16);
        self.height = (self.height & !1).max(16);
        let (long, short) = (self.width.max(self.height), self.width.min(self.height));
        if long > MAX_ENCODE_WIDTH || short > MAX_ENCODE_HEIGHT {
            return Err(MediaError::InvalidInput(format!("encoder size {}x{} exceeds {MAX_ENCODE_WIDTH}x{MAX_ENCODE_HEIGHT}", self.width, self.height)));
        }
        if !(self.fps.is_finite() && self.fps > 0.0) {
            self.fps = 30.0;
        }
        self.fps = self.fps.min(240.0);
        self.bitrate_bps = self.bitrate_bps.clamp(10_000, i32::MAX as u32);
        if self.max_slice_bytes > 0 {
            // OpenH264 rejects NAL size limits below one worst-case macroblock (400 B) + 20 B of headers.
            self.max_slice_bytes = self.max_slice_bytes.max(MIN_SLICE_BYTES);
        }
        Ok(self)
    }

    /// Lowest H.264 level whose macroblock budget covers the frame size.
    fn level(&self) -> Level {
        let mbs = self.width.div_ceil(16) * self.height.div_ceil(16);
        let mbps = mbs as f32 * self.fps;
        match (mbs, mbps) {
            (..=3600, ..=108_000.0) => Level::Level_3_1,
            (..=5120, ..=216_000.0) => Level::Level_3_2,
            (..=8192, ..=245_760.0) => Level::Level_4_0,
            (..=8704, ..=522_240.0) => Level::Level_4_2,
            (..=22080, ..=589_824.0) => Level::Level_5_0,
            (..=36864, ..=983_040.0) => Level::Level_5_1,
            _ => Level::Level_5_2,
        }
    }
}

/// One encoded access unit.
#[derive(Clone, PartialEq, Eq)]
pub struct EncodedFrame {
    /// Annex-B bytes (every NAL unit prefixed with `00 00 00 01`). Keyframes start with SPS and PPS.
    pub data: Vec<u8>,
    /// Whether this is an IDR frame.
    pub keyframe: bool,
    /// Encoded width.
    pub width: u32,
    /// Encoded height.
    pub height: u32,
}

impl std::fmt::Debug for EncodedFrame {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EncodedFrame")
            .field("bytes", &self.data.len())
            .field("keyframe", &self.keyframe)
            .field("width", &self.width)
            .field("height", &self.height)
            .finish()
    }
}

/// I420 planes borrowed for OpenH264.
struct Planes<'a>(&'a I420Frame);

impl YUVSource for Planes<'_> {
    fn dimensions(&self) -> (usize, usize) {
        (self.0.width as usize, self.0.height as usize)
    }

    fn strides(&self) -> (usize, usize, usize) {
        let c = self.0.chroma_width();
        (self.0.width as usize, c, c)
    }

    fn y(&self) -> &[u8] {
        &self.0.y
    }

    fn u(&self) -> &[u8] {
        &self.0.u
    }

    fn v(&self) -> &[u8] {
        &self.0.v
    }
}

fn codec_err(e: openh264::Error) -> MediaError {
    MediaError::Codec(e.to_string())
}

fn build_encoder(config: &EncoderConfig) -> Result<Encoder> {
    let cfg = openh264::encoder::EncoderConfig::new()
        .usage_type(UsageType::CameraVideoRealTime)
        .profile(Profile::Baseline)
        .level(config.level())
        .complexity(Complexity::Medium)
        .rate_control_mode(RateControlMode::Bitrate)
        .bitrate(BitRate::from_bps(config.bitrate_bps))
        .max_frame_rate(FrameRate::from_hz(config.fps))
        .skip_frames(true)
        .sps_pps_strategy(SpsPpsStrategy::ConstantId)
        .intra_frame_period(IntraFramePeriod::from_num_frames(config.keyframe_interval))
        .scene_change_detect(false)
        .long_term_reference(false)
        // 0 = one thread per core; OpenH264 only parallelises across slices, see `max_slice_bytes`.
        .num_threads(0);
    let cfg = if config.max_slice_bytes > 0 { cfg.max_slice_len(config.max_slice_bytes) } else { cfg };
    Encoder::with_api_config(OpenH264API::from_source(), cfg).map_err(codec_err)
}

/// Real-time H.264 encoder (RGBA or I420 in, Annex-B out).
pub struct H264Encoder {
    config: EncoderConfig,
    encoder: Encoder,
    yuv: I420Frame,
    initialized: bool,
    pending_keyframe: bool,
    epoch: Option<Instant>,
}

impl std::fmt::Debug for H264Encoder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("H264Encoder").field("config", &self.config).finish_non_exhaustive()
    }
}

impl H264Encoder {
    /// Creates an encoder. Width/height are rounded down to even values.
    pub fn new(config: EncoderConfig) -> Result<Self> {
        let config = config.normalized()?;
        Ok(Self {
            encoder: build_encoder(&config)?,
            yuv: I420Frame::black(config.width, config.height)?,
            config,
            initialized: false,
            pending_keyframe: false,
            epoch: None,
        })
    }

    /// The effective (normalised) configuration.
    pub fn config(&self) -> &EncoderConfig {
        &self.config
    }

    /// Encodes an RGBA frame, scaling it to the configured size if necessary.
    ///
    /// Returns `Ok(None)` when rate control decided to skip the frame.
    pub fn encode(&mut self, frame: &VideoFrame) -> Result<Option<EncodedFrame>> {
        frame.validate()?;
        let mut yuv = std::mem::replace(&mut self.yuv, I420Frame { width: 0, height: 0, y: Vec::new(), u: Vec::new(), v: Vec::new() });
        let converted = if frame.width == self.config.width && frame.height == self.config.height {
            convert::rgba_to_i420(&frame.data, frame.width, frame.height, &mut yuv)
        } else {
            convert::scale_rgba(&frame.data, frame.width, frame.height, self.config.width, self.config.height)
                .and_then(|scaled| convert::rgba_to_i420(&scaled, self.config.width, self.config.height, &mut yuv))
        };
        let result = converted.and_then(|()| self.encode_i420(&yuv, frame.timestamp));
        self.yuv = yuv;
        result
    }

    /// Encodes an I420 frame. Its size must equal the configured size.
    pub fn encode_i420(&mut self, frame: &I420Frame, timestamp: Instant) -> Result<Option<EncodedFrame>> {
        frame.validate()?;
        if frame.width != self.config.width || frame.height != self.config.height {
            return Err(MediaError::InvalidInput(format!(
                "I420 frame is {}x{}, encoder is configured for {}x{}",
                frame.width, frame.height, self.config.width, self.config.height
            )));
        }
        if self.pending_keyframe && self.initialized {
            self.encoder.force_intra_frame();
        }
        self.pending_keyframe = false;
        let epoch = *self.epoch.get_or_insert(timestamp);
        let millis = timestamp.saturating_duration_since(epoch).as_millis() as u64;
        let stream = self.encoder.encode_at(&Planes(frame), openh264::Timestamp::from_millis(millis)).map_err(codec_err)?;
        self.initialized = true;
        let keyframe = match stream.frame_type() {
            FrameType::Skip | FrameType::Invalid => return Ok(None),
            FrameType::IDR => true,
            _ => false,
        };
        let data = stream.to_vec();
        if data.is_empty() {
            return Ok(None);
        }
        Ok(Some(EncodedFrame { data, keyframe, width: frame.width, height: frame.height }))
    }

    /// Makes the next encoded frame an IDR (with SPS/PPS). Call on RTCP PLI/FIR.
    pub fn force_keyframe(&mut self) {
        self.pending_keyframe = true;
    }

    /// Changes the target bitrate without interrupting the stream (no keyframe).
    pub fn set_bitrate(&mut self, bitrate_bps: u32) -> Result<()> {
        let bitrate_bps = bitrate_bps.clamp(10_000, i32::MAX as u32);
        let previous = std::mem::replace(&mut self.config.bitrate_bps, bitrate_bps);
        if !self.initialized {
            // OpenH264 is configured lazily on the first frame; just rebuild with the new settings.
            self.encoder = build_encoder(&self.config)?;
            return Ok(());
        }
        // OpenH264 verifies the (single) spatial layer's target against that layer's maximum, so raise the
        // maximum before the target when increasing, and lower it afterwards when decreasing.
        use openh264_sys2::{ENCODER_OPTION_BITRATE as TARGET, ENCODER_OPTION_MAX_BITRATE as MAX, SPATIAL_LAYER_0 as LAYER0, SPATIAL_LAYER_ALL as ALL};
        let steps = if bitrate_bps > previous { [(MAX, LAYER0), (MAX, ALL), (TARGET, ALL)] } else { [(TARGET, ALL), (MAX, LAYER0), (MAX, ALL)] };
        for (option, layer) in steps {
            let mut info = openh264_sys2::SBitrateInfo { iLayer: layer, iBitrate: bitrate_bps as i32 };
            // SAFETY: the encoder is initialized (a frame was encoded); both options take a pointer to an
            // SBitrateInfo that only has to live for the duration of the call, which `info` does.
            let rc = unsafe { self.encoder.raw_api().set_option(option, (&raw mut info).cast()) };
            if rc != 0 {
                self.config.bitrate_bps = previous;
                return Err(MediaError::Codec(format!("SetOption({option}) for {bitrate_bps} bps failed with {rc}")));
            }
        }
        Ok(())
    }

    /// Changes the output size. Re-creates the encoder; the next frame is an IDR.
    pub fn set_resolution(&mut self, width: u32, height: u32) -> Result<()> {
        let config = EncoderConfig { width, height, ..self.config }.normalized()?;
        if config.width == self.config.width && config.height == self.config.height {
            return Ok(());
        }
        self.encoder = build_encoder(&config)?;
        self.yuv = I420Frame::black(config.width, config.height)?;
        self.config = config;
        self.initialized = false;
        self.pending_keyframe = false;
        Ok(())
    }
}

/// H.264 decoder (Annex-B in, RGBA out).
///
/// Feed it complete access units, e.g. from [`crate::rtp_h264::H264Depacketizer`] (after DAVE decryption).
/// Corrupt input yields `Err`, never a panic; after an error, request a keyframe.
pub struct H264Decoder {
    decoder: Decoder,
    max_pixels: usize,
}

impl std::fmt::Debug for H264Decoder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("H264Decoder").finish_non_exhaustive()
    }
}

impl H264Decoder {
    /// Creates a decoder.
    pub fn new() -> Result<Self> {
        let decoder = Decoder::with_api_config(OpenH264API::from_source(), DecoderConfig::new()).map_err(codec_err)?;
        Ok(Self { decoder, max_pixels: MAX_DIMENSION as usize * MAX_DIMENSION as usize / 4 })
    }

    /// Decodes one access unit. Returns `Ok(None)` if it produced no picture (e.g. only SPS/PPS).
    pub fn decode(&mut self, annexb: &[u8]) -> Result<Option<VideoFrame>> {
        let mut out = Vec::new();
        Ok(self.decode_into(annexb, &mut out)?.map(|(width, height)| VideoFrame { width, height, data: Arc::new(out), timestamp: Instant::now() }))
    }

    /// Decodes one access unit into a reusable RGBA buffer and returns the picture size, if any.
    pub fn decode_into(&mut self, annexb: &[u8], rgba: &mut Vec<u8>) -> Result<Option<(u32, u32)>> {
        if annexb.is_empty() {
            return Ok(None);
        }
        let max_pixels = self.max_pixels;
        let Some(yuv) = self.decoder.decode(annexb).map_err(codec_err)? else { return Ok(None) };
        let (w, h) = yuv.dimensions();
        if w == 0 || h == 0 || w > MAX_DIMENSION as usize || h > MAX_DIMENSION as usize || w * h > max_pixels {
            return Err(MediaError::Codec(format!("decoded picture has unsupported size {w}x{h}")));
        }
        let (ys, us, vs) = yuv.strides();
        let (y, u, v) = (yuv.y(), yuv.u(), yuv.v());
        let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
        let plane_ok = |len: usize, stride: usize, row: usize, rows: usize| stride >= row && len >= stride * (rows - 1) + row;
        if !plane_ok(y.len(), ys, w, h) || !plane_ok(u.len(), us, cw, ch) || !plane_ok(v.len(), vs, cw, ch) {
            return Err(MediaError::Codec("decoder returned inconsistent plane sizes".into()));
        }
        rgba.clear();
        rgba.resize(w * h * 4, 0);
        yuv.write_rgba8(rgba);
        Ok(Some((w as u32, h as u32)))
    }
}
