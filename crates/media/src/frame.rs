//! Frame types exchanged between capture, codec and UI.

use std::fmt;
use std::sync::Arc;
use std::time::Instant;

use crate::convert;
use crate::error::{MediaError, Result};

/// Largest width or height accepted anywhere in the crate.
///
/// Guards against absurd allocations from corrupt headers or caller bugs (8192×8192 RGBA is 256 MiB).
pub const MAX_DIMENSION: u32 = 8192;

/// Validates dimensions and returns `width * height` as `usize`.
pub(crate) fn checked_pixels(width: u32, height: u32) -> Result<usize> {
    if width == 0 || height == 0 || width > MAX_DIMENSION || height > MAX_DIMENSION {
        return Err(MediaError::InvalidInput(format!("unsupported frame size {width}x{height} (max {MAX_DIMENSION})")));
    }
    Ok(width as usize * height as usize)
}

/// A decoded/captured video frame in tightly packed RGBA8 (`width * height * 4` bytes, no row padding).
///
/// The pixel data sits behind an [`Arc`], so cloning a frame (e.g. to show it in a preview *and* feed it to the
/// encoder) does not copy pixels.
#[derive(Clone)]
pub struct VideoFrame {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// RGBA8 pixels, row-major, `width * 4` bytes per row.
    pub data: Arc<Vec<u8>>,
    /// When the frame was captured (or decoded). Used for encoder rate control and A/V sync.
    pub timestamp: Instant,
}

impl VideoFrame {
    /// Creates a frame stamped with [`Instant::now`], validating that `data` holds exactly `width * height * 4` bytes.
    pub fn new(width: u32, height: u32, data: Vec<u8>) -> Result<Self> {
        Self::with_timestamp(width, height, data, Instant::now())
    }

    /// Like [`VideoFrame::new`] with an explicit timestamp.
    pub fn with_timestamp(width: u32, height: u32, data: Vec<u8>, timestamp: Instant) -> Result<Self> {
        let expected = checked_pixels(width, height)? * 4;
        if data.len() != expected {
            return Err(MediaError::InvalidInput(format!("RGBA buffer is {} bytes, expected {expected} for {width}x{height}", data.len())));
        }
        Ok(Self { width, height, data: Arc::new(data), timestamp })
    }

    /// Returns an error if the public fields were set inconsistently (e.g. data length ≠ `width * height * 4`).
    pub fn validate(&self) -> Result<()> {
        let expected = checked_pixels(self.width, self.height)? * 4;
        if self.data.len() != expected {
            return Err(MediaError::InvalidInput(format!("RGBA buffer is {} bytes, expected {expected} for {}x{}", self.data.len(), self.width, self.height)));
        }
        Ok(())
    }

    /// Converts to planar I420 (BT.601 limited range).
    pub fn to_i420(&self) -> Result<I420Frame> {
        let mut out = I420Frame::black(1, 1)?;
        convert::rgba_to_i420(&self.data, self.width, self.height, &mut out)?;
        Ok(out)
    }

    /// Returns a copy scaled to exactly `width × height` (aspect ratio is not preserved).
    pub fn scaled(&self, width: u32, height: u32) -> Result<Self> {
        if width == self.width && height == self.height {
            self.validate()?;
            return Ok(self.clone());
        }
        let data = convert::scale_rgba(&self.data, self.width, self.height, width, height)?;
        Ok(Self { width, height, data: Arc::new(data), timestamp: self.timestamp })
    }

    /// Returns a copy that fits inside `max_width × max_height` while preserving the aspect ratio.
    ///
    /// Frames already small enough are returned as cheap clones. Uses area averaging, so large reductions
    /// (e.g. 1080p → 160×90 tiles) do not alias.
    pub fn thumbnail(&self, max_width: u32, max_height: u32) -> Result<Self> {
        let (w, h) = convert::fit_within(self.width, self.height, max_width, max_height);
        self.scaled(w, h)
    }
}

impl fmt::Debug for VideoFrame {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("VideoFrame")
            .field("width", &self.width)
            .field("height", &self.height)
            .field("bytes", &self.data.len())
            .field("timestamp", &self.timestamp)
            .finish()
    }
}

/// A planar YUV 4:2:0 frame (I420 / YU12): full-resolution Y plane followed by quarter-resolution U and V.
///
/// Planes are tightly packed: Y stride is `width`, chroma stride is `(width + 1) / 2`, chroma height is
/// `(height + 1) / 2`. Values use BT.601 limited ("studio") range.
#[derive(Clone, PartialEq, Eq)]
pub struct I420Frame {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// Luma plane, `width * height` bytes.
    pub y: Vec<u8>,
    /// Cb plane, `chroma_width() * chroma_height()` bytes.
    pub u: Vec<u8>,
    /// Cr plane, `chroma_width() * chroma_height()` bytes.
    pub v: Vec<u8>,
}

impl I420Frame {
    /// Allocates a black frame (Y = 16, U = V = 128).
    pub fn black(width: u32, height: u32) -> Result<Self> {
        let luma = checked_pixels(width, height)?;
        let chroma = Self::chroma_len(width, height);
        Ok(Self { width, height, y: vec![16; luma], u: vec![128; chroma], v: vec![128; chroma] })
    }

    /// Width of the chroma planes.
    pub fn chroma_width(&self) -> usize {
        (self.width as usize).div_ceil(2)
    }

    /// Height of the chroma planes.
    pub fn chroma_height(&self) -> usize {
        (self.height as usize).div_ceil(2)
    }

    pub(crate) fn chroma_len(width: u32, height: u32) -> usize {
        (width as usize).div_ceil(2) * (height as usize).div_ceil(2)
    }

    /// Resizes the planes for new dimensions (contents are unspecified afterwards).
    pub(crate) fn reshape(&mut self, width: u32, height: u32) -> Result<()> {
        let luma = checked_pixels(width, height)?;
        let chroma = Self::chroma_len(width, height);
        self.width = width;
        self.height = height;
        self.y.resize(luma, 16);
        self.u.resize(chroma, 128);
        self.v.resize(chroma, 128);
        Ok(())
    }

    /// Returns an error if plane sizes do not match the dimensions.
    pub fn validate(&self) -> Result<()> {
        let luma = checked_pixels(self.width, self.height)?;
        let chroma = Self::chroma_len(self.width, self.height);
        if self.y.len() != luma || self.u.len() != chroma || self.v.len() != chroma {
            return Err(MediaError::InvalidInput(format!(
                "I420 planes {}/{}/{} bytes do not match {}x{}",
                self.y.len(),
                self.u.len(),
                self.v.len(),
                self.width,
                self.height
            )));
        }
        Ok(())
    }

    /// Converts to an RGBA [`VideoFrame`] stamped with `timestamp`.
    pub fn to_rgba(&self, timestamp: Instant) -> Result<VideoFrame> {
        let mut rgba = Vec::new();
        convert::i420_to_rgba(self, &mut rgba)?;
        Ok(VideoFrame { width: self.width, height: self.height, data: Arc::new(rgba), timestamp })
    }
}

impl fmt::Debug for I420Frame {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("I420Frame").field("width", &self.width).field("height", &self.height).finish_non_exhaustive()
    }
}
