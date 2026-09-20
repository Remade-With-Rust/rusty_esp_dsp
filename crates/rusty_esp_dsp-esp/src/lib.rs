#![cfg_attr(not(feature = "std"), no_std)]
#![deny(unsafe_code)]
//! `rusty_esp_dsp-esp` — the chip half of the kernel home: the PIE twins.
//!
//! [`PieS3`] (feature `pie-s3`) and [`PieP4`] (feature `pie-p4`) implement
//! the seam's three traits. A kernel with a twin runs the chip's SIMD; a
//! kernel without one runs the scalar oracle under the same name, so a
//! firmware selects its type in one place and nothing else changes as
//! twins arrive. Every twin is gated byte-identical against
//! [`rusty_esp_dsp::seam::Scalar`] over the corpus
//! [`rusty_esp_dsp::seam::twin_matches_scalar`] generates — on the host with
//! the intrinsics stubbed to scalar, and on the board with them live (D2's
//! board row).
//!
//! **Today no twin exists** (D2 waits for an S3 on the bench): both types
//! delegate everything to the scalar oracle, the crate is `deny(unsafe_code)`
//! with no fenced block yet, and it compiles for `xtensa-esp32s3-none-elf`
//! and `riscv32imafc-unknown-none-elf` so the first twin lands in a crate
//! that already builds for its chip.

// `ee.*` reaches Rust only through inline asm, and Xtensa asm is still
// experimental; the `esp` toolchain is nightly, so this is available. Gated
// on the target so the host build of this crate (where the twins are the
// scalar oracle) needs no nightly at all.
#![cfg_attr(
    all(feature = "pie-s3", target_arch = "xtensa"),
    feature(asm_experimental_arch)
)]

/// The ESP32-S3 twins.
#[cfg(all(feature = "pie-s3", target_arch = "xtensa"))]
pub mod pie_s3;

#[cfg(any(feature = "pie-s3", feature = "pie-p4"))]
use rusty_esp_core::error::Result;
#[cfg(any(feature = "pie-s3", feature = "pie-p4"))]
use rusty_esp_core::frame::Geometry;
use rusty_esp_dsp::seam::Scalar;
#[cfg(any(feature = "pie-s3", feature = "pie-p4"))]
use rusty_esp_dsp::seam::{BlockKernels, PixelKernels, SampleKernels};

#[cfg(any(feature = "pie-s3", feature = "pie-p4"))]
macro_rules! delegate_to_scalar {
    ($ty:ident) => {
        impl PixelKernels for $ty {
            fn yuyv_to_rgb888(&self, src: &[u8], dst: &mut [u8]) -> Result<usize> {
                Scalar.yuyv_to_rgb888(src, dst)
            }

            fn yuyv_to_rgb565(&self, src: &[u8], dst: &mut [u8]) -> Result<usize> {
                Scalar.yuyv_to_rgb565(src, dst)
            }

            fn yuyv_to_gray8(&self, src: &[u8], dst: &mut [u8]) -> Result<usize> {
                Scalar.yuyv_to_gray8(src, dst)
            }

            fn rgb565_to_rgb888(&self, src: &[u8], dst: &mut [u8]) -> Result<usize> {
                Scalar.rgb565_to_rgb888(src, dst)
            }

            fn rgb888_to_rgb565(&self, src: &[u8], dst: &mut [u8]) -> Result<usize> {
                Scalar.rgb888_to_rgb565(src, dst)
            }

            fn downscale2x_gray8(
                &self,
                src: &[u8],
                width: u32,
                height: u32,
                dst: &mut [u8],
            ) -> Result<Geometry> {
                Scalar.downscale2x_gray8(src, width, height, dst)
            }

            fn downscale2x_rgb565(
                &self,
                src: &[u8],
                width: u32,
                height: u32,
                dst: &mut [u8],
            ) -> Result<Geometry> {
                Scalar.downscale2x_rgb565(src, width, height, dst)
            }
        }

        impl SampleKernels for $ty {
            fn dot_i16(&self, a: &[i16], b: &[i16]) -> i64 {
                Scalar.dot_i16(a, b)
            }

            fn sum_sq_i16(&self, a: &[i16]) -> i64 {
                Scalar.sum_sq_i16(a)
            }

            fn peak_abs_i16(&self, a: &[i16]) -> u16 {
                Scalar.peak_abs_i16(a)
            }
        }

        impl BlockKernels for $ty {
            fn sad_8x8(&self, a: &[u8], sa: usize, b: &[u8], sb: usize) -> Result<u32> {
                Scalar.sad_8x8(a, sa, b, sb)
            }

            fn sad_16x16(&self, a: &[u8], sa: usize, b: &[u8], sb: usize) -> Result<u32> {
                Scalar.sad_16x16(a, sa, b, sb)
            }

            fn satd_4x4_sum(&self, blocks: &[[i32; 16]]) -> i64 {
                Scalar.satd_4x4_sum(blocks)
            }
        }
    };
}

/// The ESP32-S3 kernel set (`ee.*`, 128-bit PIE). Every kernel is the
/// scalar oracle until its twin lands; the first candidates, by the D1
/// share table, are `yuyv_to_rgb888`, `yuyv_to_rgb565` and `satd_4x4_sum`.
#[cfg(feature = "pie-s3")]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PieS3;

#[cfg(feature = "pie-s3")]
delegate_to_scalar!(PieS3);

/// The ESP32-P4 kernel set (`esp.*`, 128-bit). Every kernel is the scalar
/// oracle until D4.
#[cfg(feature = "pie-p4")]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PieP4;

#[cfg(feature = "pie-p4")]
delegate_to_scalar!(PieP4);

/// The kernel set a firmware for `chip` should hold: the chip's twins when
/// the feature is on, the scalar oracle otherwise. Written as a function so
/// the choice is one line in a firmware and nowhere else.
#[must_use]
pub fn default_kernels() -> Kernels {
    Kernels::default()
}

/// What [`default_kernels`] returns under the crate's features.
#[cfg(feature = "pie-s3")]
pub type Kernels = PieS3;
/// What [`default_kernels`] returns under the crate's features.
#[cfg(all(feature = "pie-p4", not(feature = "pie-s3")))]
pub type Kernels = PieP4;
/// What [`default_kernels`] returns under the crate's features.
#[cfg(not(any(feature = "pie-s3", feature = "pie-p4")))]
pub type Kernels = Scalar;

#[cfg(all(test, feature = "std"))]
mod tests {
    use super::*;
    use rusty_esp_dsp::seam::twin_matches_scalar;

    #[test]
    fn the_chip_kernel_set_matches_the_oracle() {
        // trivially, today: the gate that will hold the real twins to it
        assert_eq!(
            twin_matches_scalar(&default_kernels(), 0x5EA3_0002, 32),
            32 * 13
        );
    }

    #[cfg(feature = "pie-s3")]
    #[test]
    fn pie_s3_is_selected_and_matches() {
        assert_eq!(twin_matches_scalar(&PieS3, 0x5EA3_0003, 32), 32 * 13);
    }
}
