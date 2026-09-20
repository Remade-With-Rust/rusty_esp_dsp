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

/// `sad_16x16` on the PIE unit.
///
/// The S3 has **no SAD instruction and no unsigned compare or subtract** — the
/// assembler rejects `ee.vsad.u8`, `ee.vmax.u8` and `ee.vsubs.u8` alike — so
/// the absolute difference of two bytes has to be built in signed 16-bit:
///
/// 1. `ee.vzip.8 qa, qzero` ZERO-EXTENDS sixteen bytes into two vectors of
///    eight `u16` lanes (the probe showed `vzip` interleaves, so interleaving
///    with zero is exactly a widen).
/// 2. `ee.vsubs.s16` is then exact: both operands are `0..=255`, so the
///    difference is `-255..=255` and nothing saturates.
/// 3. `ee.vmax.s16(d, 0 - d)` is the magnitude, and negating `-255..=255`
///    does not saturate either. This is what stands in for the missing
///    `ee.vabs.s16`.
///
/// The accumulators stay in `s16` because they can: sixteen rows of at most
/// 255 is 4 080 per lane, well inside the type. The sixteen lanes are summed
/// in scalar once per call, where the total can reach 65 280.
///
/// Every row must start 16-byte aligned for `ee.vld.128`, which needs the
/// base AND the stride aligned; anything else is the oracle's.
#[allow(unsafe_code)]
pub fn sad_16x16(a: &[u8], sa: usize, b: &[u8], sb: usize) -> Result<u32> {
    // The oracle's own bounds, in its order.
    expect_len(a, 15 * sa + 16)?;
    expect_len(b, 15 * sb + 16)?;
    if sa % 16 != 0
        || sb % 16 != 0
        || !aligned16(a.as_ptr())
        || !aligned16(b.as_ptr())
    {
        return rusty_esp_dsp::block::sad_16x16(a, sa, b, sb);
    }

    #[repr(align(16))]
    struct Q([i16; 8]);
    let mut acc_lo = Q([0; 8]);
    let mut acc_hi = Q([0; 8]);
    let pa = a.as_ptr();
    let pb = b.as_ptr();
    // SAFETY: `expect_len` above guarantees 15*stride+16 readable bytes from
    // each base, and the loop reads exactly 16 bytes at `base + r*stride` for
    // r in 0..16. Both bases and both strides are 16-byte aligned, which is
    // what `ee.vld.128` requires. q0-q6 are the only vector registers used.
    unsafe {
        core::arch::asm!(
            "ee.zero.q q4",              // accumulator, low eight lanes
            "ee.zero.q q5",              // accumulator, high eight lanes
            "ee.zero.q q6",              // a constant zero, for the negations
            "2:",
            "ee.vld.128.ip q0, {pa}, 0",
            "ee.vld.128.ip q1, {pb}, 0",
            "add {pa}, {pa}, {sa}",
            "add {pb}, {pb}, {sb}",
            // widen both rows: 16 bytes -> 2 x 8 u16 lanes
            "ee.zero.q q2",
            "ee.vzip.8 q0, q2",
            "ee.zero.q q3",
            "ee.vzip.8 q1, q3",
            // exact differences, then magnitudes via max(d, -d)
            "ee.vsubs.s16 q0, q0, q1",
            "ee.vsubs.s16 q2, q2, q3",
            "ee.vsubs.s16 q1, q6, q0",
            "ee.vmax.s16 q0, q0, q1",
            "ee.vsubs.s16 q3, q6, q2",
            "ee.vmax.s16 q2, q2, q3",
            "ee.vadds.s16 q4, q4, q0",
            "ee.vadds.s16 q5, q5, q2",
            "addi {n}, {n}, -1",
            "bnez {n}, 2b",
            "ee.vst.128.ip q4, {lo}, 0",
            "ee.vst.128.ip q5, {hi}, 0",
            pa = inout(reg) pa => _,
            pb = inout(reg) pb => _,
            sa = in(reg) sa,
            sb = in(reg) sb,
            n = inout(reg) 16usize => _,
            lo = in(reg) acc_lo.0.as_mut_ptr(),
            hi = in(reg) acc_hi.0.as_mut_ptr(),
            options(nostack),
        );
    }

    // 16 lanes of at most 4 080; the total can reach 65 280, so sum in u32.
    let mut total: u32 = 0;
    for k in 0..8 {
        total += u32::from(acc_lo.0[k] as u16);
        total += u32::from(acc_hi.0[k] as u16);
    }
    Ok(total)
}

/// `sad_8x8` on the PIE unit, by the same construction as [`sad_16x16`].
///
/// A row here is eight bytes, half a vector. That costs nothing: `ee.vzip.8`
/// puts the FIRST eight bytes of its operand into the first result register
/// as `u16` lanes, so a 16-byte load whose upper half is ignored lands the
/// eight wanted bytes exactly where they are needed. The upper half is
/// discarded rather than computed.
///
/// The load reads sixteen bytes where the kernel's contract promises eight,
/// so the SIMD arm is taken only when the slice genuinely has that slack;
/// otherwise the oracle runs. Eight rows of at most 255 is 2 040 a lane.
#[allow(unsafe_code)]
pub fn sad_8x8(a: &[u8], sa: usize, b: &[u8], sb: usize) -> Result<u32> {
    expect_len(a, 7 * sa + 8)?;
    expect_len(b, 7 * sb + 8)?;
    // The vector load takes 16 bytes from the last row's start, which the
    // contract does not promise; require the slack explicitly.
    if sa % 16 != 0
        || sb % 16 != 0
        || !aligned16(a.as_ptr())
        || !aligned16(b.as_ptr())
        || a.len() < 7 * sa + 16
        || b.len() < 7 * sb + 16
    {
        return rusty_esp_dsp::block::sad_8x8(a, sa, b, sb);
    }

    #[repr(align(16))]
    struct Q([i16; 8]);
    let mut acc = Q([0; 8]);
    let pa = a.as_ptr();
    let pb = b.as_ptr();
    // SAFETY: the guard above proves 7*stride+16 readable bytes from each
    // base, and the loop reads exactly 16 at `base + r*stride` for r in 0..8.
    // Both bases and strides are 16-byte aligned. q0-q4, q6 only.
    unsafe {
        core::arch::asm!(
            "ee.zero.q q4",
            "ee.zero.q q6",
            "2:",
            "ee.vld.128.ip q0, {pa}, 0",
            "ee.vld.128.ip q1, {pb}, 0",
            "add {pa}, {pa}, {sa}",
            "add {pb}, {pb}, {sb}",
            // only the low eight bytes matter; q2/q3 take the ignored halves
            "ee.zero.q q2",
            "ee.vzip.8 q0, q2",
            "ee.zero.q q3",
            "ee.vzip.8 q1, q3",
            "ee.vsubs.s16 q0, q0, q1",
            "ee.vsubs.s16 q1, q6, q0",
            "ee.vmax.s16 q0, q0, q1",
            "ee.vadds.s16 q4, q4, q0",
            "addi {n}, {n}, -1",
            "bnez {n}, 2b",
            "ee.vst.128.ip q4, {out}, 0",
            pa = inout(reg) pa => _,
            pb = inout(reg) pb => _,
            sa = in(reg) sa,
            sb = in(reg) sb,
            n = inout(reg) 8usize => _,
            out = in(reg) acc.0.as_mut_ptr(),
            options(nostack),
        );
    }
    let mut total: u32 = 0;
    for k in 0..8 {
        total += u32::from(acc.0[k] as u16);
    }
    Ok(total)
}

/// `sum_sq_i16` on the PIE unit.
///
/// `ee.vmulas.s16.accx` sums the products of **all eight** `s16` lanes into a
/// single accumulator — the probe fed it lanes `1..=8` against ones and read
/// back `36`, which is `1+2+..+8` and not `1+2+3+4`. That makes it the
/// instruction this kernel is shaped like: one load and one multiply-
/// accumulate per eight samples, where the scalar version needs four
/// widening multiplies and four adds.
///
/// **The accumulator is finite, so the sum is FLUSHED.** A square is at most
/// `32768^2 = 2^30` and one instruction adds eight of them, so sixteen of
/// them can reach `2^37`. Draining into an `i64` every sixteen keeps every
/// partial well inside the accumulator, and integer addition is associative,
/// so the total is the same `i64` the oracle computes.
#[allow(unsafe_code)]
#[must_use]
pub fn sum_sq_i16(a: &[i16]) -> i64 {
    let body = a.len() / 8 * 8;
    if body == 0 || !aligned16(a.as_ptr().cast::<u8>()) {
        return rusty_esp_dsp::sample::sum_sq_i16(a);
    }

    let mut total: i64 = 0;
    let mut p = a.as_ptr().cast::<u8>();
    let mut left = body / 8; // multiply-accumulates still to do
    while left > 0 {
        let batch = left.min(16);
        left -= batch;
        let (lo, hi): (u32, u32);
        // SAFETY: `batch <= left` and the loop advances `p` by 16 per trip,
        // so the reads stay inside the first `body` elements of `a`. `p` is
        // 16-byte aligned as `ee.vld.128` requires. q0 and ACCX only.
        unsafe {
            core::arch::asm!(
                "ee.zero.accx",
                "2:",
                "ee.vld.128.ip q0, {p}, 16",
                "ee.vmulas.s16.accx q0, q0",
                "addi {n}, {n}, -1",
                "bnez {n}, 2b",
                "rur.accx_0 {l}",
                "rur.accx_1 {h}",
                p = inout(reg) p,
                n = inout(reg) batch => _,
                l = out(reg) lo,
                h = out(reg) hi,
                options(nostack),
            );
        }
        // A sum of squares is never negative, so the two halves compose
        // without sign extension.
        total += ((u64::from(hi) << 32) | u64::from(lo)) as i64;
    }

    // The tail the vector body did not cover, by the oracle's own arithmetic.
    for &x in &a[body..] {
        let v = i32::from(x);
        total += i64::from(v * v);
    }
    total
}
