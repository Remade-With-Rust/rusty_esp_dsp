//! The seam a PIE twin plugs into: one trait per kernel family, the scalar
//! implementation as the default and the oracle, and nothing else.
//!
//! A function package takes `impl PixelKernels` (or `SampleKernels`,
//! `BlockKernels`) and is handed [`Scalar`] by default; a firmware that runs
//! on a chip with a twin hands it that twin instead, in one place. No
//! `#[cfg]` ever appears inside a function package, and the twin is gated
//! byte-identical against [`Scalar`] over the same corpus the scalar code
//! was gated against the copies it replaced.
//!
//! The twins live in `rusty_esp_dsp-esp` (`PieS3` behind `pie-s3`, `PieP4`
//! behind `pie-p4`), which is where the family's only fenced `unsafe` for
//! this crate will be. Until a twin exists for a kernel, the `-esp` types
//! delegate to [`Scalar`] — the seam is in place before the speed is.

use rusty_esp_core::error::Result;
use rusty_esp_core::frame::Geometry;

use crate::{block, pixel, sample};

/// The packed-pixel conversions and downscales of [`pixel`].
pub trait PixelKernels {
    /// [`pixel::yuyv_to_rgb888`].
    fn yuyv_to_rgb888(&self, src: &[u8], dst: &mut [u8]) -> Result<usize>;
    /// [`pixel::yuyv_to_rgb565`].
    fn yuyv_to_rgb565(&self, src: &[u8], dst: &mut [u8]) -> Result<usize>;
    /// [`pixel::yuyv_to_gray8`].
    fn yuyv_to_gray8(&self, src: &[u8], dst: &mut [u8]) -> Result<usize>;
    /// [`pixel::rgb565_to_rgb888`].
    fn rgb565_to_rgb888(&self, src: &[u8], dst: &mut [u8]) -> Result<usize>;
    /// [`pixel::rgb888_to_rgb565`].
    fn rgb888_to_rgb565(&self, src: &[u8], dst: &mut [u8]) -> Result<usize>;
    /// [`pixel::downscale2x_gray8`].
    fn downscale2x_gray8(
        &self,
        src: &[u8],
        width: u32,
        height: u32,
        dst: &mut [u8],
    ) -> Result<Geometry>;
    /// [`pixel::downscale2x_rgb565`].
    fn downscale2x_rgb565(
        &self,
        src: &[u8],
        width: u32,
        height: u32,
        dst: &mut [u8],
    ) -> Result<Geometry>;
}

/// The i16 reductions of [`sample`].
pub trait SampleKernels {
    /// [`sample::dot_i16`].
    fn dot_i16(&self, a: &[i16], b: &[i16]) -> i64;
    /// [`sample::sum_sq_i16`].
    fn sum_sq_i16(&self, a: &[i16]) -> i64;
    /// [`sample::peak_abs_i16`].
    fn peak_abs_i16(&self, a: &[i16]) -> u16;
}

/// The H.264 block costs of [`block`].
pub trait BlockKernels {
    /// [`block::sad_8x8`].
    fn sad_8x8(&self, a: &[u8], sa: usize, b: &[u8], sb: usize) -> Result<u32>;
    /// [`block::sad_16x16`].
    fn sad_16x16(&self, a: &[u8], sa: usize, b: &[u8], sb: usize) -> Result<u32>;
    /// [`block::satd_4x4_sum`].
    fn satd_4x4_sum(&self, blocks: &[[i32; 16]]) -> i64;
}

/// The scalar implementation of every family: always present, always the
/// oracle, and the default a function package is handed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Scalar;

impl PixelKernels for Scalar {
    fn yuyv_to_rgb888(&self, src: &[u8], dst: &mut [u8]) -> Result<usize> {
        pixel::yuyv_to_rgb888(src, dst)
    }

    fn yuyv_to_rgb565(&self, src: &[u8], dst: &mut [u8]) -> Result<usize> {
        pixel::yuyv_to_rgb565(src, dst)
    }

    fn yuyv_to_gray8(&self, src: &[u8], dst: &mut [u8]) -> Result<usize> {
        pixel::yuyv_to_gray8(src, dst)
    }

    fn rgb565_to_rgb888(&self, src: &[u8], dst: &mut [u8]) -> Result<usize> {
        pixel::rgb565_to_rgb888(src, dst)
    }

    fn rgb888_to_rgb565(&self, src: &[u8], dst: &mut [u8]) -> Result<usize> {
        pixel::rgb888_to_rgb565(src, dst)
    }

    fn downscale2x_gray8(
        &self,
        src: &[u8],
        width: u32,
        height: u32,
        dst: &mut [u8],
    ) -> Result<Geometry> {
        pixel::downscale2x_gray8(src, width, height, dst)
    }

    fn downscale2x_rgb565(
        &self,
        src: &[u8],
        width: u32,
        height: u32,
        dst: &mut [u8],
    ) -> Result<Geometry> {
        pixel::downscale2x_rgb565(src, width, height, dst)
    }
}

impl SampleKernels for Scalar {
    fn dot_i16(&self, a: &[i16], b: &[i16]) -> i64 {
        sample::dot_i16(a, b)
    }

    fn sum_sq_i16(&self, a: &[i16]) -> i64 {
        sample::sum_sq_i16(a)
    }

    fn peak_abs_i16(&self, a: &[i16]) -> u16 {
        sample::peak_abs_i16(a)
    }
}

impl BlockKernels for Scalar {
    fn sad_8x8(&self, a: &[u8], sa: usize, b: &[u8], sb: usize) -> Result<u32> {
        block::sad_8x8(a, sa, b, sb)
    }

    fn sad_16x16(&self, a: &[u8], sa: usize, b: &[u8], sb: usize) -> Result<u32> {
        block::sad_16x16(a, sa, b, sb)
    }

    fn satd_4x4_sum(&self, blocks: &[[i32; 16]]) -> i64 {
        block::satd_4x4_sum(blocks)
    }
}

/// Run every kernel of `twin` and of [`Scalar`] over the same generated
/// corpus and demand identical outputs — the gate a twin passes before it
/// is allowed anywhere. `seed` picks the corpus; `rounds` its size. Returns
/// the number of comparisons made, so a caller can see the gate ran.
///
/// Available with `std` (it allocates the corpus); the same function is
/// what the board-side gate calls with the twin compiled for the chip.
#[cfg(feature = "std")]
pub fn twin_matches_scalar<T>(twin: &T, seed: u64, rounds: usize) -> usize
where
    T: PixelKernels + SampleKernels + BlockKernels,
{
    let mut state = seed;
    let mut next = move || {
        state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        state
    };
    let mut bytes =
        |n: usize| -> std::vec::Vec<u8> { (0..n).map(|_| (next() >> 56) as u8).collect() };
    let mut comparisons = 0usize;
    let scalar = Scalar;
    for _ in 0..rounds {
        // an even number of pixels, widths and heights of at least two
        let w = 2 * (1 + (bytes(1)[0] as u32 % 40));
        let h = 2 * (1 + (bytes(1)[0] as u32 % 30));
        let px = (w * h) as usize;
        let yuyv = bytes(px * 2);
        let rgb = bytes(px * 3);
        let p565 = bytes(px * 2);
        let (mut a, mut b) = (std::vec![0u8; px * 3], std::vec![0u8; px * 3]);
        assert_eq!(
            twin.yuyv_to_rgb888(&yuyv, &mut a),
            scalar.yuyv_to_rgb888(&yuyv, &mut b)
        );
        assert_eq!(a, b, "yuyv_to_rgb888");
        let (mut a, mut b) = (std::vec![0u8; px * 2], std::vec![0u8; px * 2]);
        assert_eq!(
            twin.yuyv_to_rgb565(&yuyv, &mut a),
            scalar.yuyv_to_rgb565(&yuyv, &mut b)
        );
        assert_eq!(a, b, "yuyv_to_rgb565");
        let (mut a, mut b) = (std::vec![0u8; px], std::vec![0u8; px]);
        assert_eq!(
            twin.yuyv_to_gray8(&yuyv, &mut a),
            scalar.yuyv_to_gray8(&yuyv, &mut b)
        );
        assert_eq!(a, b, "yuyv_to_gray8");
        let (mut a, mut b) = (std::vec![0u8; px * 3], std::vec![0u8; px * 3]);
        assert_eq!(
            twin.rgb565_to_rgb888(&p565, &mut a),
            scalar.rgb565_to_rgb888(&p565, &mut b)
        );
        assert_eq!(a, b, "rgb565_to_rgb888");
        let (mut a, mut b) = (std::vec![0u8; px * 2], std::vec![0u8; px * 2]);
        assert_eq!(
            twin.rgb888_to_rgb565(&rgb, &mut a),
            scalar.rgb888_to_rgb565(&rgb, &mut b)
        );
        assert_eq!(a, b, "rgb888_to_rgb565");
        let gray = bytes(px);
        let (ow, oh) = ((w / 2) as usize, (h / 2) as usize);
        let (mut a, mut b) = (std::vec![0u8; ow * oh], std::vec![0u8; ow * oh]);
        assert_eq!(
            twin.downscale2x_gray8(&gray, w, h, &mut a),
            scalar.downscale2x_gray8(&gray, w, h, &mut b)
        );
        assert_eq!(a, b, "downscale2x_gray8");
        let (mut a, mut b) = (std::vec![0u8; ow * oh * 2], std::vec![0u8; ow * oh * 2]);
        assert_eq!(
            twin.downscale2x_rgb565(&p565, w, h, &mut a),
            scalar.downscale2x_rgb565(&p565, w, h, &mut b)
        );
        assert_eq!(a, b, "downscale2x_rgb565");
        comparisons += 7;

        let n = 1 + (bytes(1)[0] as usize % 500);
        let s1: std::vec::Vec<i16> = bytes(n * 2)
            .chunks_exact(2)
            .map(|c| i16::from_le_bytes([c[0], c[1]]))
            .collect();
        let s2: std::vec::Vec<i16> = bytes(n * 2)
            .chunks_exact(2)
            .map(|c| i16::from_le_bytes([c[0], c[1]]))
            .collect();
        assert_eq!(twin.dot_i16(&s1, &s2), scalar.dot_i16(&s1, &s2), "dot_i16");
        assert_eq!(twin.sum_sq_i16(&s1), scalar.sum_sq_i16(&s1), "sum_sq_i16");
        assert_eq!(
            twin.peak_abs_i16(&s1),
            scalar.peak_abs_i16(&s1),
            "peak_abs_i16"
        );
        comparisons += 3;

        let plane_a = bytes(32 * 32);
        let plane_b = bytes(32 * 32);
        assert_eq!(
            twin.sad_8x8(&plane_a, 32, &plane_b, 32),
            scalar.sad_8x8(&plane_a, 32, &plane_b, 32)
        );
        assert_eq!(
            twin.sad_16x16(&plane_a, 32, &plane_b, 32),
            scalar.sad_16x16(&plane_a, 32, &plane_b, 32)
        );
        let blocks: std::vec::Vec<[i32; 16]> = (0..(1 + bytes(1)[0] as usize % 9))
            .map(|_| core::array::from_fn(|_| (bytes(1)[0] as i32) - 128))
            .collect();
        assert_eq!(
            twin.satd_4x4_sum(&blocks),
            scalar.satd_4x4_sum(&blocks),
            "satd_4x4_sum"
        );
        comparisons += 3;
    }
    comparisons
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame_path<K: PixelKernels>(k: &K) -> usize {
        let yuyv = [128u8; 16 * 2];
        let mut gray = [0u8; 16];
        k.yuyv_to_gray8(&yuyv, &mut gray).unwrap()
    }

    #[test]
    fn a_caller_takes_the_trait_and_gets_scalar_by_default() {
        assert_eq!(frame_path(&Scalar), 16);
        assert_eq!(Scalar.dot_i16(&[3, 4], &[5, 6]), 39);
        assert_eq!(Scalar.satd_4x4_sum(&[[1; 16]]), 16);
    }

    #[test]
    fn scalar_is_its_own_twin() {
        // the gate the twins pass, run on the oracle itself: a corpus of
        // 64 rounds, 13 comparisons each
        assert_eq!(twin_matches_scalar(&Scalar, 0x5EA3_0001, 64), 64 * 13);
    }
}
