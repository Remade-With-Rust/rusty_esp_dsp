//! Sample-format conversion with fixed, documented rules — the ones ffmpeg's
//! `swresample` uses, so the oracle test in `rusty_esp_audio-esp` can demand
//! byte identity.
//!
//! | from → to | rule |
//! |---|---|
//! | I16 → I32 / I24In32 | `x << 16` |
//! | I32 / I24In32 → I16 | `x >> 16` (truncate toward −∞) |
//! | I32 → I24In32 | clear the low 8 bits |
//! | I24In32 → I32 | bit copy |
//! | I16 → F32 | `x / 32768` |
//! | I32 / I24In32 → F32 | `x / 2^31` |
//! | F32 → I16 | `round_ties_even(x · 32768)` clamped to the i16 range |
//! | F32 → I32 | `round_ties_even(x · 2^31)` clamped (in f64) to the i32 range |
//! | F32 → I24In32 | as I32, then clear the low 8 bits |
//!
//! Moved verbatim from `rusty_esp_audio-core::codec::pcm` (D0, 2026-09-02).

use rusty_esp_core::error::{Error, Result};
use rusty_esp_core::pcm::{PcmBlock, SampleFormat};

/// Bytes `convert` writes for `input_bytes` of `from` samples going to `to`.
#[must_use]
pub const fn output_bytes(from: SampleFormat, to: SampleFormat, input_bytes: usize) -> usize {
    input_bytes / from.bytes() * to.bytes()
}

/// `1.5 · 2^23`. Adding it to an `f32` of magnitude under `2^22` lands in
/// `[2^23, 2^24)`, where one ulp is exactly 1.0, so the addition rounds the
/// value to the nearest integer — ties to even, the default mode — and the
/// subtraction gives that integer back. Two additions where `rintf` was a
/// libm call: the same bits for every input (`tests` below prove it over
/// the whole `f32` space, `--ignored`), and on a chip without a rounding
/// instruction the difference between a level meter and a stall.
const ROUND_F32: f32 = 12_582_912.0;
/// `1.5 · 2^52`, the same trick in `f64` for magnitudes under `2^51`.
const ROUND_F64: f64 = 6_755_399_441_055_744.0;

#[inline]
fn f32_to_i16(x: f32) -> i16 {
    let v = x * 32768.0;
    if v >= 32767.0 {
        i16::MAX
    } else if v <= -32768.0 {
        i16::MIN
    } else if v.is_nan() {
        // `rintf(NaN) as i16` is 0; say so rather than rely on the cast.
        0
    } else {
        ((v + ROUND_F32) - ROUND_F32) as i16
    }
}

#[inline]
fn f32_to_i32(x: f32) -> i32 {
    let v = f64::from(x) * 2_147_483_648.0;
    if v >= 2_147_483_647.0 {
        i32::MAX
    } else if v <= -2_147_483_648.0 {
        i32::MIN
    } else if v.is_nan() {
        0
    } else {
        ((v + ROUND_F64) - ROUND_F64) as i32
    }
}

/// Convert one sample given as its little-endian bytes.
#[inline]
fn convert_sample(from: SampleFormat, to: SampleFormat, i: &[u8], o: &mut [u8]) {
    use SampleFormat::{F32, I16, I24In32, I32};
    match (from, to) {
        (I16, I32) | (I16, I24In32) => {
            let v = i32::from(i16::from_le_bytes([i[0], i[1]])) << 16;
            o.copy_from_slice(&v.to_le_bytes());
        }
        (I32, I16) | (I24In32, I16) => {
            let v = i32::from_le_bytes([i[0], i[1], i[2], i[3]]) >> 16;
            o.copy_from_slice(&(v as i16).to_le_bytes());
        }
        (I32, I24In32) => {
            o.copy_from_slice(&[0, i[1], i[2], i[3]]);
        }
        (I24In32, I32) => {
            o.copy_from_slice(i);
        }
        // Dividing by a power of two is multiplying by its exact reciprocal,
        // bit for bit (no subnormal can arise from an integer this size), and
        // a multiply is what a chip's FPU has where a divide is a routine.
        (I16, F32) => {
            let v = f32::from(i16::from_le_bytes([i[0], i[1]])) * (1.0 / 32768.0);
            o.copy_from_slice(&v.to_le_bytes());
        }
        (I32, F32) | (I24In32, F32) => {
            let v = (i32::from_le_bytes([i[0], i[1], i[2], i[3]]) as f32) * (1.0 / 2_147_483_648.0);
            o.copy_from_slice(&v.to_le_bytes());
        }
        (F32, I16) => {
            let v = f32_to_i16(f32::from_le_bytes([i[0], i[1], i[2], i[3]]));
            o.copy_from_slice(&v.to_le_bytes());
        }
        (F32, I32) => {
            let v = f32_to_i32(f32::from_le_bytes([i[0], i[1], i[2], i[3]]));
            o.copy_from_slice(&v.to_le_bytes());
        }
        (F32, I24In32) => {
            let v = f32_to_i32(f32::from_le_bytes([i[0], i[1], i[2], i[3]])) & !0xFF;
            o.copy_from_slice(&v.to_le_bytes());
        }
        // Same format: a copy. `from`/`to` are `#[non_exhaustive]`, so a new
        // variant arriving in core lands here too and stays a copy only when
        // the sizes agree (checked by the caller).
        _ => o.copy_from_slice(i),
    }
}

/// Re-encode every sample of `input` as `to` into `out`; returns bytes
/// written. Same-format input is a byte copy.
pub fn convert(input: PcmBlock<'_>, to: SampleFormat, out: &mut [u8]) -> Result<usize> {
    let from = input.format.sample;
    let n = output_bytes(from, to, input.data.len());
    if out.len() < n {
        return Err(Error::BufferTooSmall { needed: n });
    }
    if from == to {
        out[..n].copy_from_slice(input.data);
        return Ok(n);
    }
    if from.bytes() != to.bytes() && !is_known_pair(from, to) {
        return Err(Error::Unsupported);
    }
    let (fb, tb) = (from.bytes(), to.bytes());
    // Resolve the format pair ONCE and run a loop that knows it, instead of
    // re-matching `(from, to)` for every sample. The per-sample arithmetic is
    // unchanged -- `convert_sample` stays as the single definition of each
    // pair and the oracle for this fast path -- only the dispatch moves out
    // of the loop.
    macro_rules! each {
        (|$i:ident, $o:ident| $body:block) => {{
            for ($i, $o) in input
                .data
                .chunks_exact(fb)
                .zip(out[..n].chunks_exact_mut(tb))
            {
                $body
            }
        }};
    }
    use SampleFormat::{F32, I16, I24In32, I32};
    match (from, to) {
        (I16, I32) | (I16, I24In32) => each!(|i, o| {
            let v = i32::from(i16::from_le_bytes([i[0], i[1]])) << 16;
            o.copy_from_slice(&v.to_le_bytes());
        }),
        (I32, I16) | (I24In32, I16) => each!(|i, o| {
            let v = i32::from_le_bytes([i[0], i[1], i[2], i[3]]) >> 16;
            o.copy_from_slice(&(v as i16).to_le_bytes());
        }),
        (I16, F32) => each!(|i, o| {
            let v = f32::from(i16::from_le_bytes([i[0], i[1]])) * (1.0 / 32768.0);
            o.copy_from_slice(&v.to_le_bytes());
        }),
        (I32, F32) | (I24In32, F32) => each!(|i, o| {
            let v =
                (i32::from_le_bytes([i[0], i[1], i[2], i[3]]) as f32) * (1.0 / 2_147_483_648.0);
            o.copy_from_slice(&v.to_le_bytes());
        }),
        (F32, I16) => each!(|i, o| {
            let v = f32_to_i16(f32::from_le_bytes([i[0], i[1], i[2], i[3]]));
            o.copy_from_slice(&v.to_le_bytes());
        }),
        (F32, I32) => each!(|i, o| {
            let v = f32_to_i32(f32::from_le_bytes([i[0], i[1], i[2], i[3]]));
            o.copy_from_slice(&v.to_le_bytes());
        }),
        // The rarer pairs keep the shared per-sample definition.
        _ => each!(|i, o| {
            convert_sample(from, to, i, o);
        }),
    }
    Ok(n)
}

/// Every pair the table above lists.
const fn is_known_pair(from: SampleFormat, to: SampleFormat) -> bool {
    use SampleFormat::{F32, I16, I24In32, I32};
    matches!(
        (from, to),
        (I16, I32)
            | (I16, I24In32)
            | (I16, F32)
            | (I32, I16)
            | (I32, I24In32)
            | (I32, F32)
            | (I24In32, I16)
            | (I24In32, I32)
            | (I24In32, F32)
            | (F32, I16)
            | (F32, I32)
            | (F32, I24In32)
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusty_esp_core::pcm::PcmFormat;
    use rusty_esp_core::time::Micros;

    fn block<'a>(sample: SampleFormat, data: &'a [u8]) -> PcmBlock<'a> {
        PcmBlock::new(
            PcmFormat::new(16_000, 1, sample).unwrap(),
            Micros::ZERO,
            data,
        )
        .unwrap()
    }

    #[test]
    fn integer_widths() {
        let mut out = [0u8; 8];
        let i16s = [0x01, 0x00, 0x00, 0x80]; // 1, -32768
        assert_eq!(
            convert(block(SampleFormat::I16, &i16s), SampleFormat::I32, &mut out).unwrap(),
            8
        );
        assert_eq!(out, [0, 0, 1, 0, 0, 0, 0, 0x80]);
        let mut back = [0u8; 4];
        convert(block(SampleFormat::I32, &out), SampleFormat::I16, &mut back).unwrap();
        assert_eq!(back, i16s);
        // Truncation: 0x0000_FFFF >> 16 = 0; 0xFFFF_0001 (negative) >> 16 = -1.
        let odd = [0xFF, 0xFF, 0x00, 0x00, 0x01, 0x00, 0xFF, 0xFF];
        convert(block(SampleFormat::I32, &odd), SampleFormat::I16, &mut back).unwrap();
        assert_eq!(back, [0, 0, 0xFF, 0xFF]);
        // I32 → I24In32 clears the low byte; I24In32 → I32 is a copy.
        let mut o24 = [0u8; 8];
        convert(
            block(SampleFormat::I32, &odd),
            SampleFormat::I24In32,
            &mut o24,
        )
        .unwrap();
        assert_eq!(o24, [0, 0xFF, 0, 0, 0, 0, 0xFF, 0xFF]);
        let mut o32 = [0u8; 8];
        convert(
            block(SampleFormat::I24In32, &o24),
            SampleFormat::I32,
            &mut o32,
        )
        .unwrap();
        assert_eq!(o32, o24);
    }

    #[test]
    fn float_rounds_ties_even_and_clamps() {
        let f = [1.0f32, -1.0, 0.5, 1.5 / 32768.0, 2.5 / 32768.0, -0.00001];
        let mut bytes = [0u8; 24];
        for (b, v) in bytes.chunks_exact_mut(4).zip(f) {
            b.copy_from_slice(&v.to_le_bytes());
        }
        let mut out = [0u8; 12];
        convert(
            block(SampleFormat::F32, &bytes),
            SampleFormat::I16,
            &mut out,
        )
        .unwrap();
        let v: [i16; 6] =
            core::array::from_fn(|i| i16::from_le_bytes([out[i * 2], out[i * 2 + 1]]));
        assert_eq!(v, [32767, -32768, 16384, 2, 2, 0]);
        // And back: exact for values that came from integers.
        let mut f_out = [0u8; 24];
        convert(
            block(SampleFormat::I16, &out),
            SampleFormat::F32,
            &mut f_out,
        )
        .unwrap();
        let back = f32::from_le_bytes([f_out[8], f_out[9], f_out[10], f_out[11]]);
        assert_eq!(back, 0.5);
        // F32 → I32 clamps at both ends; 0.5 is exactly 2^30.
        let mut o32 = [0u8; 24];
        convert(
            block(SampleFormat::F32, &bytes),
            SampleFormat::I32,
            &mut o32,
        )
        .unwrap();
        assert_eq!(
            i32::from_le_bytes([o32[0], o32[1], o32[2], o32[3]]),
            i32::MAX
        );
        assert_eq!(
            i32::from_le_bytes([o32[4], o32[5], o32[6], o32[7]]),
            i32::MIN
        );
        assert_eq!(
            i32::from_le_bytes([o32[8], o32[9], o32[10], o32[11]]),
            1 << 30
        );
    }

    /// The libm twins the rounding replaced, kept here as the oracle.
    fn libm_f32_to_i16(x: f32) -> i16 {
        let v = libm::rintf(x * 32768.0);
        if v >= 32767.0 {
            i16::MAX
        } else if v <= -32768.0 {
            i16::MIN
        } else {
            v as i16
        }
    }

    fn libm_f32_to_i32(x: f32) -> i32 {
        let v = libm::rint(f64::from(x) * 2_147_483_648.0);
        if v >= 2_147_483_647.0 {
            i32::MAX
        } else if v <= -2_147_483_648.0 {
            i32::MIN
        } else {
            v as i32
        }
    }

    const EDGES: [f32; 22] = [
        0.0,
        -0.0,
        f32::NAN,
        f32::INFINITY,
        f32::NEG_INFINITY,
        f32::MIN_POSITIVE,
        -f32::MIN_POSITIVE,
        1e-45,
        -1e-45,
        1.0,
        -1.0,
        0.5,
        -0.5,
        0.999_969_5,
        -0.999_969_5,
        0.999_984_74,
        1.5 / 32768.0,
        2.5 / 32768.0,
        -1.5 / 32768.0,
        -2.5 / 32768.0,
        32766.5 / 32768.0,
        -32767.5 / 32768.0,
    ];

    #[test]
    fn rounding_matches_libm_on_the_edges_and_a_corpus() {
        for &x in &EDGES {
            assert_eq!(f32_to_i16(x), libm_f32_to_i16(x), "{x}");
            assert_eq!(f32_to_i32(x), libm_f32_to_i32(x), "{x}");
        }
        // every bit pattern class: an LCG over the raw bits, ten million of them
        let mut state = 0x0F32_0F32_0F32_0F32u64;
        for _ in 0..10_000_000u32 {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            let x = f32::from_bits((state >> 32) as u32);
            assert_eq!(
                f32_to_i16(x),
                libm_f32_to_i16(x),
                "{x} ({:#x})",
                x.to_bits()
            );
            assert_eq!(
                f32_to_i32(x),
                libm_f32_to_i32(x),
                "{x} ({:#x})",
                x.to_bits()
            );
        }
        // the reciprocal multiplies are the divisions, bit for bit
        for v in i16::MIN..=i16::MAX {
            assert_eq!(
                (f32::from(v) * (1.0 / 32768.0)).to_bits(),
                (f32::from(v) / 32768.0).to_bits()
            );
        }
        let mut state = 0x1332_1332u64;
        for _ in 0..10_000_000u32 {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            let v = (state >> 32) as i32;
            assert_eq!(
                ((v as f32) * (1.0 / 2_147_483_648.0)).to_bits(),
                ((v as f32) / 2_147_483_648.0).to_bits()
            );
        }
    }

    /// Every `f32` there is, both conversions: `cargo test --release -p
    /// rusty_esp_dsp -- --ignored exhaustive`. Run once per change to the
    /// rounding; the ledger records the run.
    #[test]
    #[ignore]
    fn rounding_matches_libm_exhaustively() {
        for bits in 0..=u32::MAX {
            let x = f32::from_bits(bits);
            assert_eq!(f32_to_i16(x), libm_f32_to_i16(x), "{x} ({bits:#x})");
            assert_eq!(f32_to_i32(x), libm_f32_to_i32(x), "{x} ({bits:#x})");
        }
    }

    #[test]
    fn same_format_copies_and_small_out_is_reported() {
        let data = [1u8, 2, 3, 4];
        let mut out = [0u8; 4];
        assert_eq!(
            convert(block(SampleFormat::I16, &data), SampleFormat::I16, &mut out).unwrap(),
            4
        );
        assert_eq!(out, data);
        let mut tiny = [0u8; 2];
        assert_eq!(
            convert(
                block(SampleFormat::I16, &data),
                SampleFormat::I32,
                &mut tiny
            )
            .err(),
            Some(Error::BufferTooSmall { needed: 8 })
        );
    }
}
