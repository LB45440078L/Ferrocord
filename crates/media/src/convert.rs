//! Pixel format conversion and scaling.
//!
//! Everything here is plain safe Rust written so that LLVM can vectorise the inner loops (fixed-point
//! arithmetic, chunked iteration, no per-pixel bounds checks in the hot paths). That is fast enough for
//! 720p30 on one core; see the crate's benchmark test for numbers.
//!
//! ## Colour space
//!
//! YUV ↔ RGB uses **BT.601 limited range** (Y ∈ 16..=235, Cb/Cr ∈ 16..=240), with the classic 8-bit
//! fixed-point coefficients. This matches what OpenH264 assumes and what browser decoders do for an H.264
//! stream without a VUI colour description, so a frame encoded here looks the same in Chrome as in Ferrocord.
//! Chroma is subsampled by averaging each 2×2 block (odd edges average the available pixels).
//!
//! ## Camera formats
//!
//! [`nv12_to_rgba`], [`yuyv_to_rgba`], [`rgb24_to_rgba`], [`bgr24_to_rgba`] and [`gray_to_rgba`] cover what
//! MediaFoundation, V4L2 and AVFoundation hand out besides MJPEG (which the camera module decodes with
//! `zune-jpeg`). All of them assume tightly packed rows and return an error instead of panicking when the
//! buffer is too short.

use crate::error::{MediaError, Result};
use crate::frame::{I420Frame, checked_pixels};

#[inline(always)]
fn clamp_u8(v: i32) -> u8 {
    v.clamp(0, 255) as u8
}

#[inline(always)]
fn rgb_to_y(r: i32, g: i32, b: i32) -> u8 {
    (((66 * r + 129 * g + 25 * b + 128) >> 8) + 16) as u8
}

#[inline(always)]
fn rgb_to_uv(r: i32, g: i32, b: i32) -> (u8, u8) {
    let u = ((-38 * r - 74 * g + 112 * b + 128) >> 8) + 128;
    let v = ((112 * r - 94 * g - 18 * b + 128) >> 8) + 128;
    (u as u8, v as u8)
}

/// Pre-computed chroma contribution for a pair of pixels sharing the same U/V sample.
#[derive(Clone, Copy)]
struct Chroma {
    r: i32,
    g: i32,
    b: i32,
}

impl Chroma {
    #[inline(always)]
    fn new(u: u8, v: u8) -> Self {
        let d = u as i32 - 128;
        let e = v as i32 - 128;
        Self { r: 409 * e, g: -100 * d - 208 * e, b: 516 * d }
    }

    #[inline(always)]
    fn write(self, y: u8, out: &mut [u8; 4]) {
        let c = (y as i32 - 16) * 298 + 128;
        *out = [clamp_u8((c + self.r) >> 8), clamp_u8((c + self.g) >> 8), clamp_u8((c + self.b) >> 8), 255];
    }

    #[inline(always)]
    fn write_pair(self, y: &[u8; 2], out: &mut [u8; 8]) {
        let (pixels, _) = out.as_chunks_mut::<4>();
        self.write(y[0], &mut pixels[0]);
        self.write(y[1], &mut pixels[1]);
    }
}

/// Writes one RGBA row from a luma row and one chroma sample per pixel pair (the last one may be single).
#[inline(always)]
fn write_row(y_row: &[u8], out_row: &mut [u8], mut chroma: impl Iterator<Item = Chroma>) {
    let (y_pairs, y_tail) = y_row.as_chunks::<2>();
    let (out_pairs, out_tail) = out_row.as_chunks_mut::<8>();
    for ((y, out), c) in y_pairs.iter().zip(out_pairs.iter_mut()).zip(&mut chroma) {
        c.write_pair(y, out);
    }
    if let (Some(&y), Some(out), Some(c)) = (y_tail.first(), out_tail.as_chunks_mut::<4>().0.first_mut(), chroma.next()) {
        c.write(y, out);
    }
}

#[inline(always)]
fn rgb(px: &[u8]) -> (i32, i32, i32) {
    (px[0] as i32, px[1] as i32, px[2] as i32)
}

fn too_short(what: &str, got: usize, need: usize, width: u32, height: u32) -> MediaError {
    MediaError::InvalidInput(format!("{what} buffer is {got} bytes, need at least {need} for {width}x{height}"))
}

/// Converts tightly packed RGBA8 to I420, resizing `out` as needed (its allocation is reused).
///
/// The alpha channel is ignored.
pub fn rgba_to_i420(rgba: &[u8], width: u32, height: u32, out: &mut I420Frame) -> Result<()> {
    let pixels = checked_pixels(width, height)?;
    if rgba.len() != pixels * 4 {
        return Err(MediaError::InvalidInput(format!("RGBA buffer is {} bytes, expected {} for {width}x{height}", rgba.len(), pixels * 4)));
    }
    out.reshape(width, height)?;
    let w = width as usize;
    let h = height as usize;
    let cw = out.chroma_width();
    let mut odd_row_scratch = if h % 2 == 1 { vec![0u8; w] } else { Vec::new() };

    let (y_plane, u_plane, v_plane) = (&mut out.y, &mut out.u, &mut out.v);
    for (cy, ((y_pair, u_row), v_row)) in y_plane.chunks_mut(2 * w).zip(u_plane.chunks_exact_mut(cw)).zip(v_plane.chunks_exact_mut(cw)).enumerate() {
        let row0 = 2 * cy;
        let row1 = (row0 + 1).min(h - 1);
        let src0 = &rgba[row0 * w * 4..(row0 + 1) * w * 4];
        let src1 = &rgba[row1 * w * 4..(row1 + 1) * w * 4];
        let (y0, y1): (&mut [u8], &mut [u8]) = if y_pair.len() == 2 * w {
            y_pair.split_at_mut(w)
        } else {
            // Odd height: the last luma row has no partner; write the duplicate into scratch.
            (y_pair, odd_row_scratch.as_mut_slice())
        };

        let (pairs0, tail0) = src0.as_chunks::<8>();
        let (pairs1, tail1) = src1.as_chunks::<8>();
        let (y0_pairs, y0_tail) = y0.as_chunks_mut::<2>();
        let (y1_pairs, y1_tail) = y1.as_chunks_mut::<2>();
        let pairs = pairs0.iter().zip(pairs1).zip(y0_pairs.iter_mut().zip(y1_pairs.iter_mut())).zip(u_row.iter_mut().zip(v_row.iter_mut()));
        for (((a, b), (ya, yb)), (u, v)) in pairs {
            let (ra, ga, ba) = rgb(&a[..4]);
            let (rb, gb, bb) = rgb(&a[4..]);
            let (rc, gc, bc) = rgb(&b[..4]);
            let (rd, gd, bd) = rgb(&b[4..]);
            *ya = [rgb_to_y(ra, ga, ba), rgb_to_y(rb, gb, bb)];
            *yb = [rgb_to_y(rc, gc, bc), rgb_to_y(rd, gd, bd)];
            (*u, *v) = rgb_to_uv((ra + rb + rc + rd + 2) >> 2, (ga + gb + gc + gd + 2) >> 2, (ba + bb + bc + bd + 2) >> 2);
        }
        // Odd width: the last column pairs only vertically.
        if let (Some(y0), Some(y1)) = (y0_tail.first_mut(), y1_tail.first_mut())
            && tail0.len() >= 4
            && tail1.len() >= 4
        {
            let (ra, ga, ba) = rgb(tail0);
            let (rc, gc, bc) = rgb(tail1);
            *y0 = rgb_to_y(ra, ga, ba);
            *y1 = rgb_to_y(rc, gc, bc);
            let uv = rgb_to_uv((ra + rc + 1) >> 1, (ga + gc + 1) >> 1, (ba + bc + 1) >> 1);
            if let (Some(u), Some(v)) = (u_row.last_mut(), v_row.last_mut()) {
                (*u, *v) = uv;
            }
        }
    }
    Ok(())
}

/// Converts an I420 frame to tightly packed RGBA8 (alpha = 255), replacing the contents of `out`.
pub fn i420_to_rgba(frame: &I420Frame, out: &mut Vec<u8>) -> Result<()> {
    frame.validate()?;
    let w = frame.width as usize;
    let cw = frame.chroma_width();
    out.clear();
    out.resize(w * frame.height as usize * 4, 0);
    for (row, out_row) in out.chunks_exact_mut(w * 4).enumerate() {
        let y_row = &frame.y[row * w..(row + 1) * w];
        let c = (row / 2) * cw;
        let u_row = &frame.u[c..c + cw];
        let v_row = &frame.v[c..c + cw];
        write_row(y_row, out_row, u_row.iter().zip(v_row).map(|(&u, &v)| Chroma::new(u, v)));
    }
    Ok(())
}

/// Converts NV12 (Y plane + interleaved CbCr plane) to RGBA8.
///
/// Rows must be tightly packed. MediaFoundation sometimes pads the luma plane to a multiple of 16 rows
/// (1080 → 1088); that layout is detected from the buffer length and handled.
pub fn nv12_to_rgba(src: &[u8], width: u32, height: u32, out: &mut Vec<u8>) -> Result<()> {
    let pixels = checked_pixels(width, height)?;
    let w = width as usize;
    let h = height as usize;
    let cw = w.div_ceil(2);
    let ch = h.div_ceil(2);
    let need = pixels + cw * 2 * ch;
    if src.len() < need {
        return Err(too_short("NV12", src.len(), need, width, height));
    }
    let h16 = h.next_multiple_of(16);
    let uv_offset = if h16 != h && src.len() == w * h16 + cw * 2 * (h16 / 2) { w * h16 } else { pixels };
    let (y_plane, uv_plane) = (&src[..pixels], &src[uv_offset..uv_offset + cw * 2 * ch]);

    out.clear();
    out.resize(pixels * 4, 0);
    for (row, out_row) in out.chunks_exact_mut(w * 4).enumerate() {
        let y_row = &y_plane[row * w..(row + 1) * w];
        let uv_row = &uv_plane[(row / 2) * cw * 2..(row / 2 + 1) * cw * 2];
        write_row(y_row, out_row, uv_row.as_chunks::<2>().0.iter().map(|uv| Chroma::new(uv[0], uv[1])));
    }
    Ok(())
}

/// Converts packed YUYV 4:2:2 (a.k.a. YUY2: `Y0 U Y1 V`) to RGBA8.
pub fn yuyv_to_rgba(src: &[u8], width: u32, height: u32, out: &mut Vec<u8>) -> Result<()> {
    let pixels = checked_pixels(width, height)?;
    let w = width as usize;
    let stride = w.div_ceil(2) * 4;
    let need = stride * height as usize;
    if src.len() < need {
        return Err(too_short("YUYV", src.len(), need, width, height));
    }
    out.clear();
    out.resize(pixels * 4, 0);
    for (src_row, out_row) in src.chunks_exact(stride).zip(out.chunks_exact_mut(w * 4)) {
        let (quads, _) = src_row.as_chunks::<4>();
        let (out_pairs, out_tail) = out_row.as_chunks_mut::<8>();
        for (q, out) in quads.iter().zip(out_pairs.iter_mut()) {
            Chroma::new(q[1], q[3]).write_pair(&[q[0], q[2]], out);
        }
        if let (Some(q), Some(out)) = (quads.get(out_pairs.len()), out_tail.as_chunks_mut::<4>().0.first_mut()) {
            Chroma::new(q[1], q[3]).write(q[0], out);
        }
    }
    Ok(())
}

fn packed3_to_rgba(src: &[u8], width: u32, height: u32, out: &mut Vec<u8>, swap: bool, what: &str) -> Result<()> {
    let pixels = checked_pixels(width, height)?;
    if src.len() < pixels * 3 {
        return Err(too_short(what, src.len(), pixels * 3, width, height));
    }
    out.clear();
    out.resize(pixels * 4, 255);
    for (s, d) in src[..pixels * 3].as_chunks::<3>().0.iter().zip(out.as_chunks_mut::<4>().0) {
        if swap {
            d[0] = s[2];
            d[2] = s[0];
        } else {
            d[0] = s[0];
            d[2] = s[2];
        }
        d[1] = s[1];
    }
    Ok(())
}

/// Converts packed RGB24 to RGBA8.
pub fn rgb24_to_rgba(src: &[u8], width: u32, height: u32, out: &mut Vec<u8>) -> Result<()> {
    packed3_to_rgba(src, width, height, out, false, "RGB24")
}

/// Converts packed BGR24 to RGBA8.
pub fn bgr24_to_rgba(src: &[u8], width: u32, height: u32, out: &mut Vec<u8>) -> Result<()> {
    packed3_to_rgba(src, width, height, out, true, "BGR24")
}

/// Converts 8-bit greyscale to RGBA8.
pub fn gray_to_rgba(src: &[u8], width: u32, height: u32, out: &mut Vec<u8>) -> Result<()> {
    let pixels = checked_pixels(width, height)?;
    if src.len() < pixels {
        return Err(too_short("GRAY", src.len(), pixels, width, height));
    }
    out.clear();
    out.resize(pixels * 4, 255);
    for (&g, d) in src[..pixels].iter().zip(out.as_chunks_mut::<4>().0) {
        *d = [g, g, g, 255];
    }
    Ok(())
}

/// Largest size that fits in `max_width × max_height` while keeping the aspect ratio of `width × height`.
///
/// Never upscales and never returns a zero dimension.
pub fn fit_within(width: u32, height: u32, max_width: u32, max_height: u32) -> (u32, u32) {
    let (max_width, max_height) = (max_width.max(1), max_height.max(1));
    if width <= max_width && height <= max_height {
        return (width.max(1), height.max(1));
    }
    // Compare width/max_width against height/max_height without floating point.
    let (w, h, mw, mh) = (width as u64, height as u64, max_width as u64, max_height as u64);
    if w * mh >= h * mw { (max_width, ((h * mw + w / 2) / w).clamp(1, mh) as u32) } else { (((w * mh + h / 2) / h).clamp(1, mw) as u32, max_height) }
}

/// Scales tightly packed RGBA8 from `src_w × src_h` to `dst_w × dst_h`.
///
/// Reductions of 2× or more in both directions use area averaging (no aliasing on thumbnails); everything
/// else uses bilinear interpolation with 8-bit fixed-point weights.
pub fn scale_rgba(src: &[u8], src_w: u32, src_h: u32, dst_w: u32, dst_h: u32) -> Result<Vec<u8>> {
    let src_pixels = checked_pixels(src_w, src_h)?;
    let dst_pixels = checked_pixels(dst_w, dst_h)?;
    if src.len() != src_pixels * 4 {
        return Err(MediaError::InvalidInput(format!("RGBA buffer is {} bytes, expected {} for {src_w}x{src_h}", src.len(), src_pixels * 4)));
    }
    if src_w == dst_w && src_h == dst_h {
        return Ok(src.to_vec());
    }
    let mut out = vec![0u8; dst_pixels * 4];
    if dst_w * 2 <= src_w && dst_h * 2 <= src_h {
        scale_area(src, src_w as usize, src_h as usize, &mut out, dst_w as usize, dst_h as usize);
    } else {
        scale_bilinear(src, src_w as usize, src_h as usize, &mut out, dst_w as usize, dst_h as usize);
    }
    Ok(out)
}

fn scale_area(src: &[u8], sw: usize, sh: usize, out: &mut [u8], dw: usize, dh: usize) {
    let spans = |s: usize, d: usize| -> Vec<(usize, usize)> { (0..d).map(|i| (i * s / d, ((i + 1) * s / d).max(i * s / d + 1).min(s))).collect() };
    let xs = spans(sw, dw);
    let ys = spans(sh, dh);
    let mut acc = vec![0u32; dw * 4];
    for (out_row, &(y0, y1)) in out.chunks_exact_mut(dw * 4).zip(&ys) {
        acc.iter_mut().for_each(|a| *a = 0);
        for row in src[y0 * sw * 4..y1 * sw * 4].chunks_exact(sw * 4) {
            for (a, &(x0, x1)) in acc.as_chunks_mut::<4>().0.iter_mut().zip(&xs) {
                for px in row[x0 * 4..x1 * 4].as_chunks::<4>().0 {
                    a[0] += px[0] as u32;
                    a[1] += px[1] as u32;
                    a[2] += px[2] as u32;
                    a[3] += px[3] as u32;
                }
            }
        }
        let rows = (y1 - y0) as u32;
        for ((o, a), &(x0, x1)) in out_row.as_chunks_mut::<4>().0.iter_mut().zip(acc.as_chunks::<4>().0).zip(&xs) {
            let n = rows * (x1 - x0) as u32;
            for c in 0..4 {
                o[c] = ((a[c] + n / 2) / n) as u8;
            }
        }
    }
}

fn scale_bilinear(src: &[u8], sw: usize, sh: usize, out: &mut [u8], dw: usize, dh: usize) {
    // Map destination pixel centres onto the source grid in 16.16 fixed point; keep 8 bits of weight.
    let map = |s: usize, d: usize| -> Vec<(usize, usize, u32)> {
        let step = ((s as u64) << 16) / d as u64;
        (0..d)
            .map(|i| {
                let pos = ((i as i64 * 2 + 1) * step as i64 / 2 - (1 << 15)).max(0) as u64;
                let i0 = ((pos >> 16) as usize).min(s - 1);
                let i1 = (i0 + 1).min(s - 1);
                (i0, i1, ((pos >> 8) & 0xff) as u32)
            })
            .collect()
    };
    let xs = map(sw, dw);
    let ys = map(sh, dh);
    let stride = sw * 4;
    for (out_row, &(y0, y1, fy)) in out.chunks_exact_mut(dw * 4).zip(&ys) {
        let r0 = &src[y0 * stride..(y0 + 1) * stride];
        let r1 = &src[y1 * stride..(y1 + 1) * stride];
        for (o, &(x0, x1, fx)) in out_row.as_chunks_mut::<4>().0.iter_mut().zip(&xs) {
            let (a, b, c, d) = (&r0[x0 * 4..x0 * 4 + 4], &r0[x1 * 4..x1 * 4 + 4], &r1[x0 * 4..x0 * 4 + 4], &r1[x1 * 4..x1 * 4 + 4]);
            for ch in 0..4 {
                let top = a[ch] as u32 * (256 - fx) + b[ch] as u32 * fx;
                let bottom = c[ch] as u32 * (256 - fx) + d[ch] as u32 * fx;
                o[ch] = ((top * (256 - fy) + bottom * fy + (1 << 15)) >> 16) as u8;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid(w: u32, h: u32, rgb: [u8; 3]) -> Vec<u8> {
        (0..w * h).flat_map(|_| [rgb[0], rgb[1], rgb[2], 255]).collect()
    }

    #[test]
    fn rgba_i420_round_trip_is_close() {
        for (w, h) in [(16, 16), (17, 9), (1, 1), (3, 2)] {
            let rgba: Vec<u8> = (0..w * h).flat_map(|i| [(i * 7 % 256) as u8, 128, (255 - i * 3 % 256) as u8, 255]).collect();
            let mut yuv = I420Frame::black(1, 1).unwrap();
            rgba_to_i420(&rgba, w, h, &mut yuv).unwrap();
            yuv.validate().unwrap();
            let mut back = Vec::new();
            i420_to_rgba(&yuv, &mut back).unwrap();
            assert_eq!(back.len(), rgba.len());
        }
        for color in [[255, 0, 0], [0, 255, 0], [0, 0, 255], [255, 255, 255], [0, 0, 0], [90, 160, 30]] {
            let rgba = solid(8, 8, color);
            let mut yuv = I420Frame::black(1, 1).unwrap();
            rgba_to_i420(&rgba, 8, 8, &mut yuv).unwrap();
            let mut back = Vec::new();
            i420_to_rgba(&yuv, &mut back).unwrap();
            for (a, b) in rgba.iter().zip(&back) {
                assert!((*a as i32 - *b as i32).abs() <= 3, "{color:?}: {a} vs {b}");
            }
        }
    }

    #[test]
    fn camera_formats_reject_short_buffers() {
        let mut out = Vec::new();
        assert!(nv12_to_rgba(&[0; 10], 4, 4, &mut out).is_err());
        assert!(yuyv_to_rgba(&[0; 10], 4, 4, &mut out).is_err());
        assert!(rgb24_to_rgba(&[0; 10], 4, 4, &mut out).is_err());
        assert!(gray_to_rgba(&[0; 10], 4, 4, &mut out).is_err());
        assert!(nv12_to_rgba(&[], 0, 4, &mut out).is_err());
    }

    #[test]
    fn camera_formats_produce_grey() {
        let mut out = Vec::new();
        let (w, h) = (6u32, 4u32);
        let nv12: Vec<u8> = vec![126; (w * h) as usize].into_iter().chain(vec![128; (w * h / 2) as usize]).collect();
        nv12_to_rgba(&nv12, w, h, &mut out).unwrap();
        assert!(out.chunks(4).all(|p| p[0] == p[1] && p[1] == p[2] && p[3] == 255));
        let yuyv: Vec<u8> = (0..w * h / 2).flat_map(|_| [126, 128, 126, 128]).collect();
        let mut out2 = Vec::new();
        yuyv_to_rgba(&yuyv, w, h, &mut out2).unwrap();
        assert_eq!(out, out2);
        // MediaFoundation-style 16-row padded NV12.
        let (w, h) = (4u32, 6u32);
        let padded: Vec<u8> = vec![126; (w * 16) as usize].into_iter().chain(vec![128; (w * 8) as usize]).collect();
        nv12_to_rgba(&padded, w, h, &mut out).unwrap();
        assert_eq!(out.len(), (w * h * 4) as usize);
        let mut bgr = Vec::new();
        bgr24_to_rgba(&[1, 2, 3], 1, 1, &mut bgr).unwrap();
        assert_eq!(bgr, [3, 2, 1, 255]);
    }

    #[test]
    fn scaling_preserves_solid_colour_and_fits() {
        let src = solid(64, 36, [10, 200, 30]);
        for (w, h) in [(16, 9), (100, 50), (63, 35), (1, 1), (128, 72)] {
            let out = scale_rgba(&src, 64, 36, w, h).unwrap();
            assert_eq!(out.len(), (w * h * 4) as usize);
            assert!(out.chunks(4).all(|p| p == [10, 200, 30, 255]), "{w}x{h}");
        }
        assert_eq!(fit_within(1920, 1080, 320, 320), (320, 180));
        assert_eq!(fit_within(1080, 1920, 320, 320), (180, 320));
        assert_eq!(fit_within(100, 50, 320, 320), (100, 50));
        assert_eq!(fit_within(5000, 1, 10, 10), (10, 1));
        assert!(scale_rgba(&src, 64, 36, 0, 10).is_err());
        assert!(scale_rgba(&src[1..], 64, 36, 10, 10).is_err());
    }
}
