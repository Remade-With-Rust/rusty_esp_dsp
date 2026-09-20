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
//! **Nine of the seam's thirteen kernels now run real `ee.*` twins on an
//! ESP32-S3** -- `yuyv_to_gray8`, `downscale2x_gray8`, `dot_i16`,
//! `sum_sq_i16`, `peak_abs_i16`, `sad_8x8`, `sad_16x16`, `yuyv_to_rgb565`
//! and `downscale2x_rgb565`. The rest
//! delegate to the oracle, either because no twin exists yet or because one
//! was built and MEASURED WORSE (`satd_4x4_sum`; see the ledger's P4 and P6
//! entries) or is impossible on this unit (`rgb565_to_rgb888` and
//! `rgb888_to_rgb565` need a 3-way byte deinterleave the ISA does not have).
//!
//! The twins are `cfg(target_arch = "xtensa")`. Off-chip -- a host build, a
//! host test, a RISC-V target -- every method is the scalar oracle, so this
//! crate builds everywhere and needs nightly only for the Xtensa build.
//!
//! `PieP4` still delegates everything: D4 has no board yet.

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

// Only `PieP4` is generated now; `PieS3` writes its impls out so that
// which kernels are accelerated is readable rather than inferred.
#[cfg(feature = "pie-p4")]
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

/// The ESP32-S3 kernel set (`ee.*`, 128-bit PIE). Seven of the thirteen
/// seam kernels run a real twin on an S3; the impls below say which, and
/// why each of the rest does not.
#[cfg(feature = "pie-s3")]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PieS3;

/// `PieS3` does NOT use `delegate_to_scalar!`: each method either calls its
/// twin or states that it is deliberately the oracle. Written out rather
/// than generated so that "which kernels are actually accelerated" is
/// readable in one place -- the defect this crate previously had was a
/// fully-delegating seam sitting in front of a module full of working
/// twins, where every gate passed and no caller ever reached one.
#[cfg(feature = "pie-s3")]
impl PixelKernels for PieS3 {
    /// Oracle: no twin. 3 bytes a pixel needs a 3-way deinterleave and the
    /// PIE unit has only 2-way `zip`/`unzip` with no general byte permute.
    fn yuyv_to_rgb888(&self, src: &[u8], dst: &mut [u8]) -> Result<usize> {
        Scalar.yuyv_to_rgb888(src, dst)
    }

    /// TWIN (backlog B2, -63.2%).
    fn yuyv_to_rgb565(&self, src: &[u8], dst: &mut [u8]) -> Result<usize> {
        #[cfg(target_arch = "xtensa")]
        {
            pie_s3::yuyv_to_rgb565(src, dst)
        }
        #[cfg(not(target_arch = "xtensa"))]
        {
            Scalar.yuyv_to_rgb565(src, dst)
        }
    }

    /// TWIN.
    fn yuyv_to_gray8(&self, src: &[u8], dst: &mut [u8]) -> Result<usize> {
        #[cfg(target_arch = "xtensa")]
        {
            pie_s3::yuyv_to_gray8(src, dst)
        }
        #[cfg(not(target_arch = "xtensa"))]
        {
            Scalar.yuyv_to_gray8(src, dst)
        }
    }

    /// Oracle: impossible on this unit, as for `yuyv_to_rgb888`.
    fn rgb565_to_rgb888(&self, src: &[u8], dst: &mut [u8]) -> Result<usize> {
        Scalar.rgb565_to_rgb888(src, dst)
    }

    /// Oracle: impossible on this unit, as for `yuyv_to_rgb888`.
    fn rgb888_to_rgb565(&self, src: &[u8], dst: &mut [u8]) -> Result<usize> {
        Scalar.rgb888_to_rgb565(src, dst)
    }

    /// TWIN. The twin reports only success or a length failure, so the
    /// geometry the seam owes its caller is built here; a length failure
    /// goes to the oracle, which produces the exact error.
    fn downscale2x_gray8(
        &self,
        src: &[u8],
        width: u32,
        height: u32,
        dst: &mut [u8],
    ) -> Result<Geometry> {
        #[cfg(target_arch = "xtensa")]
        {
            match pie_s3::downscale2x_gray8(src, width, height, dst) {
                Ok(()) => Geometry::new(
                    width / 2,
                    height / 2,
                    rusty_esp_core::frame::PixelFormat::Gray8,
                ),
                Err(()) => Scalar.downscale2x_gray8(src, width, height, dst),
            }
        }
        #[cfg(not(target_arch = "xtensa"))]
        {
            Scalar.downscale2x_gray8(src, width, height, dst)
        }
    }

    /// TWIN (backlog B1, -65.5%). As for `downscale2x_gray8`, the twin
    /// reports only success or a length failure, so the geometry the seam
    /// owes its caller is built here.
    fn downscale2x_rgb565(
        &self,
        src: &[u8],
        width: u32,
        height: u32,
        dst: &mut [u8],
    ) -> Result<Geometry> {
        #[cfg(target_arch = "xtensa")]
        {
            match pie_s3::downscale2x_rgb565(src, width, height, dst) {
                Ok(()) => Geometry::new(
                    width / 2,
                    height / 2,
                    rusty_esp_core::frame::PixelFormat::Rgb565,
                ),
                Err(()) => Scalar.downscale2x_rgb565(src, width, height, dst),
            }
        }
        #[cfg(not(target_arch = "xtensa"))]
        {
            Scalar.downscale2x_rgb565(src, width, height, dst)
        }
    }
}

#[cfg(feature = "pie-s3")]
impl SampleKernels for PieS3 {
    /// TWIN.
    fn dot_i16(&self, a: &[i16], b: &[i16]) -> i64 {
        #[cfg(target_arch = "xtensa")]
        {
            pie_s3::dot_i16(a, b)
        }
        #[cfg(not(target_arch = "xtensa"))]
        {
            Scalar.dot_i16(a, b)
        }
    }

    /// TWIN.
    fn sum_sq_i16(&self, a: &[i16]) -> i64 {
        #[cfg(target_arch = "xtensa")]
        {
            pie_s3::sum_sq_i16(a)
        }
        #[cfg(not(target_arch = "xtensa"))]
        {
            Scalar.sum_sq_i16(a)
        }
    }

    /// TWIN.
    fn peak_abs_i16(&self, a: &[i16]) -> u16 {
        #[cfg(target_arch = "xtensa")]
        {
            pie_s3::peak_abs_i16(a)
        }
        #[cfg(not(target_arch = "xtensa"))]
        {
            Scalar.peak_abs_i16(a)
        }
    }
}

#[cfg(feature = "pie-s3")]
impl BlockKernels for PieS3 {
    /// TWIN.
    fn sad_8x8(&self, a: &[u8], sa: usize, b: &[u8], sb: usize) -> Result<u32> {
        #[cfg(target_arch = "xtensa")]
        {
            pie_s3::sad_8x8(a, sa, b, sb)
        }
        #[cfg(not(target_arch = "xtensa"))]
        {
            Scalar.sad_8x8(a, sa, b, sb)
        }
    }

    /// TWIN.
    fn sad_16x16(&self, a: &[u8], sa: usize, b: &[u8], sb: usize) -> Result<u32> {
        #[cfg(target_arch = "xtensa")]
        {
            pie_s3::sad_16x16(a, sa, b, sb)
        }
        #[cfg(not(target_arch = "xtensa"))]
        {
            Scalar.sad_16x16(a, sa, b, sb)
        }
    }

    /// ORACLE BY MEASUREMENT, not by omission. A twin was built twice --
    /// once with the 4x4 transpose going through memory (+25.4%) and once
    /// with it entirely in registers (+7.6%) -- and lost both times. Ledger
    /// P4 and P6.
    fn satd_4x4_sum(&self, blocks: &[[i32; 16]]) -> i64 {
        Scalar.satd_4x4_sum(blocks)
    }
}

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
