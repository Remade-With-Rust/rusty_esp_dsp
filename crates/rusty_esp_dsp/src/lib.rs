#![cfg_attr(not(feature = "std"), no_std)]
#![forbid(unsafe_code)]
//! `rusty_esp_dsp` — ESP-DSP remade for the Janus family: the scalar kernel
//! home.
//!
//! Every function package in the family carries the hot loops it needs in
//! its own `-core`, scalar. The moment two packages carry the same loop, or a
//! loop is what a chip-side speedup would be spent on, it moves here — scalar
//! first — and the PIE twin (ESP32-S3 `ee.*`, ESP32-P4 `esp.*`) is written
//! against that scalar version, never against the caller. The scalar version
//! never leaves the tree: it is the oracle every twin is gated against, byte
//! for byte, on the host, in CI.
//!
//! What is here, and where it came from (D0, 2026-09-02):
//!
//! - [`pixel`]: the packed conversions a camera pipeline needs (YUYV / RGB565
//!   / RGB888 / Gray8) and the 2× box downscales, from `rusty_esp_image-core`.
//! - [`sample`]: the i16 reductions (dot, sum of squares, peak, dBFS) from
//!   `rusty_esp_audio-core`, and [`sample::pcm`], its sample-format conversion
//!   with `swresample`'s rules.
//! - [`block`]: the H.264 block costs (SAD, 4×4 Hadamard, SATD) written
//!   scalar to match `rusty_h264-common`'s transform byte for byte.
//! - [`int`]: the integer square root `rusty_esp_signal-core`'s CSI amplitude
//!   uses.
//! - [`probe`]: the work counters both arms of a measurement report, and the
//!   ceiling probe that says whether a twin is worth building at all.
//!
//! No allocator, no drivers, no product types, no FFT (nothing needs one
//! yet), no neural-network ops (those are FFai's). Borrowed slices in,
//! borrowed slices out; sizes are checked and `BufferTooSmall` names the need.
//!
//! Part of Janus (Remade With Rust). Plan: `docs/plans/rusty_esp_dsp.md`.

pub mod block;
pub mod int;
pub mod pixel;
pub mod probe;
pub mod sample;

pub use rusty_esp_core as esp_core;

use rusty_esp_core::error::{Error, Result};

/// Crate version, for capability manifests and logs.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// `Ok` when `buf` holds at least `needed` elements; the family's one way of
/// saying "your buffer is too small, and this is how big it has to be".
#[inline]
pub(crate) fn expect_len<T>(buf: &[T], needed: usize) -> Result<()> {
    if buf.len() < needed {
        Err(Error::BufferTooSmall { needed })
    } else {
        Ok(())
    }
}

/// The names a sketch or firmware wants in scope.
pub mod prelude {
    pub use crate::block::{hadamard_4x4, residual_4x4, sad_8x8, sad_16x16, satd_4x4};
    pub use crate::int::isqrt;
    pub use crate::pixel::{
        downscale2x_gray8, downscale2x_rgb565, pack_rgb565, rgb565_to_rgb888, rgb888_to_rgb565,
        unpack_rgb565, yuyv_to_gray8, yuyv_to_rgb565, yuyv_to_rgb888,
    };
    pub use crate::probe::{Verdict, Work};
    pub use crate::sample::{dot_i16, peak_abs_i16, rms_dbfs_i16, sum_sq_i16};
}
