//! The ESP32-S3 PIE twins: 128-bit `ee.*` kernels against the scalar oracle.
//!
//! **Every lane layout used here was read off the silicon, not off a manual.**
//! The probe firmware runs each instruction over known byte patterns and
//! prints all sixteen result bytes, which settles semantics more exactly than
//! prose can and needs no document to be on hand. What it established, for
//! the instructions these kernels use:
//!
//! | instruction | behaviour |
//! |---|---|
//! | `ee.vld.128.ip q, a, imm` | 16 aligned bytes into `q`, then `a += imm` |
//! | `ee.vst.128.ip q, a, imm` | the reverse |
//! | `ee.vunzip.8 qa, qb` | `qa` <- the EVEN bytes of `[qa‖qb]`, `qb` <- the ODD ones, **both written in place** |
//!
//! `ee.vld.128.ip` requires 16-byte alignment; an unaligned buffer would need
//! `ee.ld.128.usar.ip` plus `ee.src.q`, which costs more than it saves here.
//! So every kernel below checks alignment once and hands anything else to the
//! oracle — the same shape the byte and aligned arms take everywhere else in
//! this family.

use rusty_esp_core::error::{Error, Result};
use rusty_esp_dsp::expect_len_pub as expect_len;

/// True when `p` sits on a 16-byte boundary, which `ee.vld/vst.128` require.
#[inline]
fn aligned16(p: *const u8) -> bool {
    (p as usize) % 16 == 0
}

/// `yuyv_to_gray8` on the 128-bit PIE unit.
///
/// YUYV is `Y0 U Y1 V`, so the luma bytes are exactly the EVEN indices of the
/// stream — and `ee.vunzip.8` gathers the even bytes of two loaded vectors
/// into the first. **Thirty-two source bytes become sixteen luma bytes in one
/// instruction**, where the scalar kernel needs sixteen byte loads and
/// sixteen byte stores to select the same bytes.
///
/// Byte-identical to [`rusty_esp_dsp::pixel::yuyv_to_gray8`] by construction:
/// it selects the same bytes and does no arithmetic. The gate is
/// `twin_matches_scalar` over a generated corpus, on the board.
pub fn yuyv_to_gray8(src: &[u8], dst: &mut [u8]) -> Result<usize> {
    // The oracle's validation, in the oracle's order, so the errors match.
    if src.len() % 2 != 0 {
        return Err(Error::InvalidGeometry);
    }
    let pixels = src.len() / 2;
    expect_len(dst, pixels)?;

    // 32 source bytes -> 16 output bytes per trip, and both sides have to be
    // 16-byte aligned for the vector load and store.
    let body = pixels / 16 * 16;
    let use_simd = body != 0 && aligned16(src.as_ptr()) && aligned16(dst.as_ptr());

    let done = if use_simd {
        simd_even_bytes(&src[..body * 2], &mut dst[..body]);
        body
    } else {
        0
    };

    // The tail — and the whole thing when the buffers are not aligned — is
    // the oracle's own loop, byte for byte.
    for (s, d) in src[done * 2..pixels * 2]
        .chunks_exact(2)
        .zip(dst[done..pixels].iter_mut())
    {
        *d = s[0];
    }
    Ok(pixels)
}

/// Gather the even bytes of `src` into `dst`; `src.len() == 2 * dst.len()`,
/// both 16-byte aligned, and `dst.len()` a multiple of 16.
#[allow(unsafe_code)]
fn simd_even_bytes(src: &[u8], dst: &mut [u8]) {
    debug_assert_eq!(src.len(), dst.len() * 2);
    debug_assert_eq!(dst.len() % 16, 0);
    debug_assert!(aligned16(src.as_ptr()) && aligned16(dst.as_mut_ptr()));

    let mut s = src.as_ptr();
    let mut d = dst.as_mut_ptr();
    let trips = dst.len() / 16;
    for _ in 0..trips {
        // SAFETY: `trips` is `dst.len() / 16` and the loop advances `s` by 32
        // and `d` by 16 exactly once each, so the reads stay inside `src`
        // (which is twice as long) and the writes inside `dst`. Both pointers
        // are 16-byte aligned, which is what `ee.vld/vst.128` require, and
        // the asm touches only q0 and q1, which nothing else holds live.
        unsafe {
            core::arch::asm!(
                "ee.vld.128.ip q0, {s}, 16",
                "ee.vld.128.ip q1, {s}, 16",
                "ee.vunzip.8 q0, q1",
                "ee.vst.128.ip q0, {d}, 16",
                s = inout(reg) s,
                d = inout(reg) d,
                options(nostack),
            );
        }
    }
}

/// `peak_abs_i16` on the PIE unit: the largest magnitude in the block.
///
/// There is **no `ee.vabs.*`** on this part — the assembler rejects it — so
/// the magnitude has to be built. Negating with `ee.vsubs.s16` from zero is
/// SATURATING, and that is exactly wrong at one input: `-32768` negates to
/// `32767`, where the scalar kernel reports `32768`. Tracking the lane-wise
/// MINIMUM alongside the maximum settles it exactly and costs one instruction
/// a trip: if any lane held `i16::MIN` the answer is `32768`, otherwise it is
/// `max(maxima, -minima)`, and both are decided once at the end.
///
/// Byte-identical to [`rusty_esp_dsp::sample::peak_abs_i16`] for every input
/// including `i16::MIN`, which is the whole reason the minimum is carried.
#[allow(unsafe_code)]
#[must_use]
pub fn peak_abs_i16(a: &[i16]) -> u16 {
    let body = a.len() / 8 * 8;
    if body == 0 || !aligned16(a.as_ptr().cast::<u8>()) {
        return rusty_esp_dsp::sample::peak_abs_i16(a);
    }

    // Eight i16 lanes a trip: one load, one max, one min.
    #[repr(align(16))]
    struct Q([i16; 8]);
    let mut hi = Q([i16::MIN; 8]);
    let mut lo = Q([i16::MAX; 8]);
    let p = a.as_ptr().cast::<u8>();
    let trips = body / 8;
    // SAFETY: `trips * 8` elements is `body`, which is at most `a.len()`, so
    // the loads stay inside `a`; `p` is 16-byte aligned as `ee.vld.128`
    // requires and advances by exactly 16 per trip. q0-q2 are the only
    // vector registers touched and nothing else holds them live.
    unsafe {
        core::arch::asm!(
            // q1 = running maxima, q2 = running minima.
            "ee.vld.128.ip q1, {hi}, 0",
            "ee.vld.128.ip q2, {lo}, 0",
            "2:",
            "ee.vld.128.ip q0, {p}, 16",
            "ee.vmax.s16 q1, q1, q0",
            "ee.vmin.s16 q2, q2, q0",
            "addi {n}, {n}, -1",
            "bnez {n}, 2b",
            "ee.vst.128.ip q1, {hi}, 0",
            "ee.vst.128.ip q2, {lo}, 0",
            p = inout(reg) p => _,
            n = inout(reg) trips => _,
            hi = in(reg) hi.0.as_mut_ptr(),
            lo = in(reg) lo.0.as_mut_ptr(),
            options(nostack),
        );
    }

    // Horizontal reduce, once per call, in scalar.
    let mut best: u16 = 0;
    let mut saw_min = false;
    for k in 0..8 {
        let h = hi.0[k];
        let l = lo.0[k];
        if l == i16::MIN {
            saw_min = true;
        } else if l.unsigned_abs() > best {
            best = l.unsigned_abs();
        }
        if h > 0 && (h as u16) > best {
            best = h as u16;
        }
    }
    // And the tail the vector body did not cover.
    for &x in &a[body..] {
        if x == i16::MIN {
            saw_min = true;
        } else if x.unsigned_abs() > best {
            best = x.unsigned_abs();
        }
    }
    if saw_min { 32768 } else { best }
}
