//! Pixel kernels — scalar, bounds-checked, and the oracle for every faster twin.
//!
//! These are the conversions a camera pipeline on an ESP32 actually needs:
//! what the sensor emits (YUYV, RGB565) into what an encoder, a model or an
//! LCD wants (RGB888, gray, a smaller frame). Every function takes borrowed
//! input and caller-owned output and returns the geometry it produced.
//!
//! Colour conversion is BT.601 full-range (the JFIF convention), fixed-point
//! with an 8-bit fraction — the same arithmetic on every platform.
//!
//! Moved verbatim from `rusty_esp_image-core::ops` (D0, 2026-09-02); the
//! oracle test in `tests/moved.rs` holds the copies this module replaced and
//! demands byte identity over a generated corpus.

use rusty_esp_core::error::{Error, Result};
use rusty_esp_core::frame::{Geometry, PixelFormat};

use crate::expect_len;

/// Pack an RGB888 pixel as little-endian RGB565.
#[must_use]
pub const fn pack_rgb565(r: u8, g: u8, b: u8) -> u16 {
    ((r as u16 & 0xF8) << 8) | ((g as u16 & 0xFC) << 3) | (b as u16 >> 3)
}

/// Unpack a little-endian RGB565 pixel to RGB888, replicating the top bits
/// into the low bits so white stays white.
#[must_use]
pub const fn unpack_rgb565(p: u16) -> [u8; 3] {
    let r5 = ((p >> 11) & 0x1F) as u8;
    let g6 = ((p >> 5) & 0x3F) as u8;
    let b5 = (p & 0x1F) as u8;
    [
        (r5 << 3) | (r5 >> 2),
        (g6 << 2) | (g6 >> 4),
        (b5 << 3) | (b5 >> 2),
    ]
}

/// One RGB565 channel widened to 8 bits, top bits replicated into the low
/// bits — exactly the per-channel arithmetic of [`unpack_rgb565`], but taken
/// straight from the packed pixel and already widened, so a caller that wants
/// one channel of several pixels never materialises the `[u8; 3]`s.
#[inline]
const fn r5to8(p: u16) -> u32 {
    let v = ((p >> 11) & 0x1F) as u32;
    (v << 3) | (v >> 2)
}

#[inline]
const fn g6to8(p: u16) -> u32 {
    let v = ((p >> 5) & 0x3F) as u32;
    (v << 2) | (v >> 4)
}

#[inline]
const fn b5to8(p: u16) -> u32 {
    let v = (p & 0x1F) as u32;
    (v << 3) | (v >> 2)
}

/// BT.601 full-range YCbCr → RGB888, fixed-point.
#[must_use]
pub fn yuv_to_rgb(y: u8, u: u8, v: u8) -> [u8; 3] {
    let y = i32::from(y);
    let cb = i32::from(u) - 128;
    let cr = i32::from(v) - 128;
    let r = y + ((359 * cr) >> 8);
    let g = y - ((88 * cb + 183 * cr) >> 8);
    let b = y + ((454 * cb) >> 8);
    [clamp(r), clamp(g), clamp(b)]
}

fn clamp(v: i32) -> u8 {
    v.clamp(0, 255) as u8
}

/// RGB565 (LE) → RGB888. `src` holds `pixels × 2` bytes, `dst` `pixels × 3`.
pub fn rgb565_to_rgb888(src: &[u8], dst: &mut [u8]) -> Result<usize> {
    if src.len() % 2 != 0 {
        return Err(Error::InvalidGeometry);
    }
    let pixels = src.len() / 2;
    expect_len(dst, pixels * 3)?;
    for (s, d) in src.chunks_exact(2).zip(dst.chunks_exact_mut(3)) {
        d.copy_from_slice(&unpack_rgb565(u16::from_le_bytes([s[0], s[1]])));
    }
    Ok(pixels)
}

/// RGB888 → RGB565 (LE).
pub fn rgb888_to_rgb565(src: &[u8], dst: &mut [u8]) -> Result<usize> {
    if src.len() % 3 != 0 {
        return Err(Error::InvalidGeometry);
    }
    let pixels = src.len() / 3;
    expect_len(dst, pixels * 2)?;
    for (s, d) in src.chunks_exact(3).zip(dst.chunks_exact_mut(2)) {
        d.copy_from_slice(&pack_rgb565(s[0], s[1], s[2]).to_le_bytes());
    }
    Ok(pixels)
}

/// YUYV (Y0 U Y1 V) → RGB888. `src` holds `pixels × 2` bytes (pixels even).
pub fn yuyv_to_rgb888(src: &[u8], dst: &mut [u8]) -> Result<usize> {
    if src.len() % 4 != 0 {
        return Err(Error::InvalidGeometry);
    }
    let pixels = src.len() / 2;
    expect_len(dst, pixels * 3)?;
    for (s, d) in src.chunks_exact(4).zip(dst.chunks_exact_mut(6)) {
        d[..3].copy_from_slice(&yuv_to_rgb(s[0], s[1], s[3]));
        d[3..].copy_from_slice(&yuv_to_rgb(s[2], s[1], s[3]));
    }
    Ok(pixels)
}

/// YUYV → RGB565 (LE).
pub fn yuyv_to_rgb565(src: &[u8], dst: &mut [u8]) -> Result<usize> {
    if src.len() % 4 != 0 {
        return Err(Error::InvalidGeometry);
    }
    let pixels = src.len() / 2;
    expect_len(dst, pixels * 2)?;
    for (s, d) in src.chunks_exact(4).zip(dst.chunks_exact_mut(4)) {
        let [r0, g0, b0] = yuv_to_rgb(s[0], s[1], s[3]);
        let [r1, g1, b1] = yuv_to_rgb(s[2], s[1], s[3]);
        d[..2].copy_from_slice(&pack_rgb565(r0, g0, b0).to_le_bytes());
        d[2..].copy_from_slice(&pack_rgb565(r1, g1, b1).to_le_bytes());
    }
    Ok(pixels)
}

/// YUYV → Gray8: the luma bytes.
pub fn yuyv_to_gray8(src: &[u8], dst: &mut [u8]) -> Result<usize> {
    if src.len() % 2 != 0 {
        return Err(Error::InvalidGeometry);
    }
    let pixels = src.len() / 2;
    expect_len(dst, pixels)?;
    for (s, d) in src.chunks_exact(2).zip(dst.iter_mut()) {
        *d = s[0];
    }
    Ok(pixels)
}

/// 2× box downscale of a Gray8 image; odd trailing rows/columns are dropped.
/// Returns the output geometry.
pub fn downscale2x_gray8(src: &[u8], width: u32, height: u32, dst: &mut [u8]) -> Result<Geometry> {
    let (w, h) = (width as usize, height as usize);
    expect_len(src, w * h)?;
    let (ow, oh) = (w / 2, h / 2);
    let out = Geometry::new(ow as u32, oh as u32, PixelFormat::Gray8)?;
    expect_len(dst, ow * oh)?;
    // Slice both source rows to exactly the span this row reads and slice the
    // destination row too, then walk them as fixed 2-byte chunks. Indexing
    // `r0[2 * ox + 1]` and `dst[oy * ow + ox]` left the compiler unable to
    // prove either index in range, so each output pixel carried its own
    // bounds checks. Same arithmetic, same order, identical output bytes.
    let span = ow * 2;
    for oy in 0..oh {
        let r0 = &src[(2 * oy) * w..(2 * oy) * w + span];
        let r1 = &src[(2 * oy + 1) * w..(2 * oy + 1) * w + span];
        let drow = &mut dst[oy * ow..oy * ow + ow];
        for ((a, b), d) in r0
            .chunks_exact(2)
            .zip(r1.chunks_exact(2))
            .zip(drow.iter_mut())
        {
            let sum =
                u32::from(a[0]) + u32::from(a[1]) + u32::from(b[0]) + u32::from(b[1]);
            *d = ((sum + 2) / 4) as u8;
        }
    }
    Ok(out)
}

/// 2× box downscale of an RGB565 (LE) image, per channel.
pub fn downscale2x_rgb565(src: &[u8], width: u32, height: u32, dst: &mut [u8]) -> Result<Geometry> {
    let (w, h) = (width as usize, height as usize);
    expect_len(src, w * h * 2)?;
    let (ow, oh) = (w / 2, h / 2);
    let out = Geometry::new(ow as u32, oh as u32, PixelFormat::Rgb565)?;
    expect_len(dst, ow * oh * 2)?;
    // Hoist the two source rows and the destination row per output row, then
    // walk them as fixed 4-byte (two-pixel) chunks. Indexing the whole `src`
    // through a closure that re-derived `(y * w + x) * 2` cost four
    // bounds-checked pairs of loads per output pixel; the gray8 twin has
    // always sliced its rows, which is why it ran ~6x faster for the same
    // shape of work. The arithmetic below is unchanged, so every output byte
    // is identical.
    let stride = w * 2;
    let span = ow * 4; // the two-pixel columns this row actually reads
    for oy in 0..oh {
        let r0 = &src[(2 * oy) * stride..(2 * oy) * stride + span];
        let r1 = &src[(2 * oy + 1) * stride..(2 * oy + 1) * stride + span];
        let drow = &mut dst[oy * ow * 2..oy * ow * 2 + ow * 2];
        for ((s0, s1), d) in r0
            .chunks_exact(4)
            .zip(r1.chunks_exact(4))
            .zip(drow.chunks_exact_mut(2))
        {
            // Keep the four PACKED pixels live and widen one channel at a
            // time. Unpacking all four into `[u8; 3]` first put twelve values
            // plus three iterators live at once, far past the usable Xtensa
            // register window, and the loop spilled: 30 stores per output
            // pixel where the algorithm needs two. Same per-channel
            // arithmetic and rounding, so the output bytes are identical.
            let q0 = u16::from_le_bytes([s0[0], s0[1]]);
            let q1 = u16::from_le_bytes([s0[2], s0[3]]);
            let q2 = u16::from_le_bytes([s1[0], s1[1]]);
            let q3 = u16::from_le_bytes([s1[2], s1[3]]);
            let r = ((r5to8(q0) + r5to8(q1) + r5to8(q2) + r5to8(q3) + 2) / 4) as u8;
            let g = ((g6to8(q0) + g6to8(q1) + g6to8(q2) + g6to8(q3) + 2) / 4) as u8;
            let b = ((b5to8(q0) + b5to8(q1) + b5to8(q2) + b5to8(q3) + 2) / 4) as u8;
            d.copy_from_slice(&pack_rgb565(r, g, b).to_le_bytes());
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rgb565_pack_unpack_extremes_and_round_trip() {
        assert_eq!(pack_rgb565(255, 255, 255), 0xFFFF);
        assert_eq!(pack_rgb565(0, 0, 0), 0);
        assert_eq!(unpack_rgb565(0xFFFF), [255, 255, 255]);
        assert_eq!(unpack_rgb565(0xF800), [255, 0, 0]);
        assert_eq!(unpack_rgb565(0x07E0), [0, 255, 0]);
        assert_eq!(unpack_rgb565(0x001F), [0, 0, 255]);
        // round trip within quantisation
        for &(r, g, b) in &[(10u8, 200u8, 77u8), (128, 128, 128), (1, 2, 3)] {
            let [r2, g2, b2] = unpack_rgb565(pack_rgb565(r, g, b));
            assert!((i32::from(r) - i32::from(r2)).abs() <= 7);
            assert!((i32::from(g) - i32::from(g2)).abs() <= 3);
            assert!((i32::from(b) - i32::from(b2)).abs() <= 7);
        }
    }

    #[test]
    fn yuv_known_points() {
        assert_eq!(yuv_to_rgb(128, 128, 128), [128, 128, 128]);
        assert_eq!(yuv_to_rgb(255, 128, 128), [255, 255, 255]);
        assert_eq!(yuv_to_rgb(0, 128, 128), [0, 0, 0]);
        // pure red in JFIF: Y=76, Cb=85, Cr=255
        let [r, g, b] = yuv_to_rgb(76, 85, 255);
        assert!(r >= 250 && g <= 3 && b <= 3, "{r} {g} {b}");
        // pure blue: Y=29, Cb=255, Cr=107
        let [r, g, b] = yuv_to_rgb(29, 255, 107);
        assert!(b >= 250 && r <= 3 && g <= 3, "{r} {g} {b}");
    }

    #[test]
    fn conversions_and_sizes() {
        let rgb = [255u8, 0, 0, 0, 255, 0];
        let mut p = [0u8; 4];
        assert_eq!(rgb888_to_rgb565(&rgb, &mut p).unwrap(), 2);
        assert_eq!(p, [0x00, 0xF8, 0xE0, 0x07]);
        let mut back = [0u8; 6];
        assert_eq!(rgb565_to_rgb888(&p, &mut back).unwrap(), 2);
        assert_eq!(back, rgb);
        let mut small = [0u8; 3];
        assert_eq!(
            rgb565_to_rgb888(&p, &mut small),
            Err(Error::BufferTooSmall { needed: 6 })
        );
        assert_eq!(
            rgb565_to_rgb888(&p[..3], &mut back),
            Err(Error::InvalidGeometry)
        );

        let yuyv = [128u8, 128, 255, 128];
        let mut rgb = [0u8; 6];
        assert_eq!(yuyv_to_rgb888(&yuyv, &mut rgb).unwrap(), 2);
        assert_eq!(rgb, [128, 128, 128, 255, 255, 255]);
        let mut g = [0u8; 2];
        yuyv_to_gray8(&yuyv, &mut g).unwrap();
        assert_eq!(g, [128, 255]);
        let mut p = [0u8; 4];
        yuyv_to_rgb565(&yuyv, &mut p).unwrap();
        assert_eq!(&p[2..], &[0xFF, 0xFF]);
    }

    #[test]
    fn downscale_drops_odd_edges_and_rounds_to_nearest() {
        // 4x2 gray
        let src = [0u8, 4, 8, 12, 4, 8, 12, 16];
        let mut dst = [0u8; 2];
        let g = downscale2x_gray8(&src, 4, 2, &mut dst).unwrap();
        assert_eq!((g.width, g.height), (2, 1));
        assert_eq!(dst, [4, 12]);
        // a 5x3 image loses its last column and row: 2x1 out of it
        let src = [10u8; 15];
        let mut dst = [0u8; 2];
        let g = downscale2x_gray8(&src, 5, 3, &mut dst).unwrap();
        assert_eq!((g.width, g.height), (2, 1));
        assert_eq!(dst, [10, 10]);
        // (1 + 2 + 3 + 4 + 2) / 4 = 3: nearest, not floor
        let src = [1u8, 2, 3, 4];
        let mut dst = [0u8; 1];
        downscale2x_gray8(&src, 2, 2, &mut dst).unwrap();
        assert_eq!(dst, [3]);

        // 2x2 rgb565 of one colour averages to itself
        let c = pack_rgb565(200, 100, 50).to_le_bytes();
        let src: std::vec::Vec<u8> = c.iter().copied().cycle().take(8).collect();
        let mut dst = [0u8; 2];
        downscale2x_rgb565(&src, 2, 2, &mut dst).unwrap();
        assert_eq!(dst, c);
        let mut tiny = [0u8; 1];
        assert_eq!(
            downscale2x_rgb565(&src, 2, 2, &mut tiny),
            Err(Error::BufferTooSmall { needed: 2 })
        );
    }
}
