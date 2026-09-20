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
//!
//! # The second instruction tier, also read off the silicon
//!
//! | form | what it does |
//! |---|---|
//! | `ssai N` + `ee.vsr.32 qd, qs` | ARITHMETIC right shift, four 32-bit lanes, amount from SAR |
//! | `ssai N` + `ee.vsl.32 qd, qs` | left shift, four 32-bit lanes, wrapping |
//! | `ee.vmul.s16 qd, qa, qb` | eight lanes, low half, **WRAPS** -- 32767 x 3 reads 32765 |
//! | `ee.vadds.s32` / `ee.vsubs.s32` | four 32-bit lanes, saturating |
//! | `ee.vcmp.lt.s16` / `.gt` / `.eq` | eight lanes, all-ones where true, all-zeros where false |
//! | `ee.vzip.32` / `ee.vunzip.32` | interleave / deinterleave 32-bit lanes across the pair |
//! | `ee.notq qd, qs` | bitwise complement |
//! | `rur.accx_0` / `rur.accx_1` | ACCX is FORTY bits; `accx_1` is bits 32..=39, ZERO-extended |
//!
//! Three consequences shape the kernels below.
//!
//! **There is no 16-bit shift.** Dividing i16 lanes by a power of two means
//! widening to 32 bits, shifting, and narrowing back.
//!
//! **`ee.vmul.s16` cannot be used where the scalar saturates**, because it
//! truncates to the low half instead. A saturating multiply has to go
//! through 32-bit lanes, or through QACC and `ee.srcmb.s16.qacc`.
//!
//! **Sign-extending i16 -> i32 costs two instructions.**
//! `ee.vcmp.lt.s16 qs, qv, qzero` puts all-ones in every negative lane,
//! which IS the high half of the sign-extended value, and `ee.vzip.16 qv, qs`
//! interleaves the two into correct 32-bit lanes. Narrowing back is
//! `ee.vunzip.16`, which collects the low halves. Where the values are known
//! non-negative, zip against a ZERO register instead and skip the compare.

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
    let body = pixels / 32 * 32;
    let use_simd = body != 0 && aligned16(src.as_ptr()) && aligned16(dst.as_ptr());

    let done = if use_simd {
        simd_even_bytes(&src[..body * 2], &mut dst[..body]);
        body
    } else if aligned16(dst.as_ptr()) && pixels >= 32 {
        // The SOURCE may sit at any offset -- a cropped sub-region of a
        // frame starts wherever its left edge does -- while the destination
        // is a buffer this code allocated and so is aligned. That case used
        // to take the oracle in full; the unaligned idiom reaches it.
        //
        // Sixteen pixels short of the end, because producing the last
        // window reads the aligned block containing its last byte.
        let ub = (pixels - 16) / 32 * 32;
        simd_even_bytes_unaligned_src(&src[..ub * 2], &mut dst[..ub]);
        ub
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
/// both 16-byte aligned, and `dst.len()` a multiple of 32.
///
/// The loop lives INSIDE the asm block and runs 32 output pixels a trip.
/// It used to be a Rust `for` around a four-instruction block, which is the
/// worst of both: the block was re-entered every sixteen pixels and paid the
/// Rust counter and branch on TOP of its own four instructions. Same
/// arithmetic, same bytes out.
#[allow(unsafe_code)]
fn simd_even_bytes(src: &[u8], dst: &mut [u8]) {
    debug_assert_eq!(src.len(), dst.len() * 2);
    debug_assert_eq!(dst.len() % 32, 0);
    debug_assert!(aligned16(src.as_ptr()) && aligned16(dst.as_mut_ptr()));

    let mut s = src.as_ptr();
    let mut d = dst.as_mut_ptr();
    let mut trips = dst.len() / 32;
    if trips == 0 {
        return;
    }
    // SAFETY: each trip advances `s` by 64 and `d` by 32, exactly
    // `dst.len() / 32` times, so the reads stay inside `src` (twice as long)
    // and the writes inside `dst`. Both are 16-byte aligned, which is what
    // `ee.vld/vst.128` require. q0-q3 only.
    unsafe {
        core::arch::asm!(
            "26:",
            "ee.vld.128.ip q0, {s}, 16",
            "ee.vld.128.ip q1, {s}, 16",
            "ee.vunzip.8 q0, q1",
            "ee.vst.128.ip q0, {d}, 16",
            "ee.vld.128.ip q2, {s}, 16",
            "ee.vld.128.ip q3, {s}, 16",
            "ee.vunzip.8 q2, q3",
            "ee.vst.128.ip q2, {d}, 16",
            "addi {n}, {n}, -1",
            "bnez {n}, 26b",
            s = inout(reg) s,
            d = inout(reg) d,
            n = inout(reg) trips => _,
            options(nostack),
        );
    }
    let _ = (s, d);
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
    // SIXTEEN samples a trip; see the note on `sum_sq_i16`. Here the gap
    // was larger still -- the unaligned arm read 13,735 against this one's
    // 16,288 ps/sample -- because the body is three instructions of work
    // against two of loop.
    let body = a.len() / 16 * 16;
    if body == 0 {
        return rusty_esp_dsp::sample::peak_abs_i16(a);
    }
    if !aligned16(a.as_ptr().cast::<u8>()) {
        // Not the oracle any more: the unaligned idiom reaches this buffer.
        return peak_abs_i16_unaligned(a);
    }

    // Eight i16 lanes a trip: one load, one max, one min.
    #[repr(align(16))]
    struct Q([i16; 8]);
    let mut hi = Q([i16::MIN; 8]);
    let mut lo = Q([i16::MAX; 8]);
    let p = a.as_ptr().cast::<u8>();
    let trips = body / 16;
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
            "ee.vld.128.ip q3, {p}, 16",
            "ee.vmax.s16 q1, q1, q0",
            "ee.vmin.s16 q2, q2, q0",
            "ee.vmax.s16 q1, q1, q3",
            "ee.vmin.s16 q2, q2, q3",
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
        // Not the oracle any more: the unaligned idiom reaches a block at
        // ANY position, which in a motion search is all of them.
        return Ok(sad_16x16_unaligned(a, sa, b, sb));
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
            // `ee.vld.128.xp` loads AND advances by a register-valued
            // stride, which is exactly what a row walk wants -- it replaces
            // the load plus the `add` that followed it, two instructions a
            // row per stream.
            "ee.vld.128.xp q0, {pa}, {sa}",
            "ee.vld.128.xp q1, {pb}, {sb}",
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
        // Not the oracle any more: the unaligned idiom reaches a block at
        // ANY position, which in a motion search is all of them.
        return Ok(sad_8x8_unaligned(a, sa, b, sb));
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
            // As in `sad_16x16`: load and stride in one instruction.
            "ee.vld.128.xp q0, {pa}, {sa}",
            "ee.vld.128.xp q1, {pb}, {sb}",
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
    // NOT the fused form, and that is MEASURED. `ee.vmulas.s16.accx.ld.ip`
    // does the load and the eight-lane multiply-accumulate in one slot and
    // it is SLOWER here -- see the ledger's P6 refutation. Instruction count
    // went down and cycles went up in all three kernels tried.
    let body = a.len() / 16 * 16;
    if body == 0 {
        return rusty_esp_dsp::sample::sum_sq_i16(a);
    }
    if !aligned16(a.as_ptr().cast::<u8>()) {
        // Not the oracle any more: the unaligned idiom reaches this buffer.
        return sum_sq_i16_unaligned(a);
    }

    let mut total: i64 = 0;
    let mut p = a.as_ptr().cast::<u8>();
    let mut left = body / 16; // trips still to do, two MACs each
    while left > 0 {
        // EIGHT trips = sixteen multiply-accumulates = 128 products of at
        // most 2^30, which peaks at 2^37 inside the 40-bit accumulator.
        let batch = left.min(8);
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
                "ee.vld.128.ip q1, {p}, 16",
                "ee.vmulas.s16.accx q0, q0",
                "ee.vmulas.s16.accx q1, q1",
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
        // ACCX is FORTY bits, and `rur.accx_1` delivers bits 32..=39
        // ZERO-extended into a 32-bit register -- an accumulator of -1 reads
        // back `accx_1 = 0x0000_00ff`, not `0xffff_ffff` (measured, see the
        // module table). So the two halves compose into 40 bits and the sign
        // has to be put back by hand. `sum_sq_i16` is exempt only because a
        // sum of squares is never negative; copy THIS one, not that one.
        let raw = (u64::from(hi & 0xff) << 32) | u64::from(lo);
        total += ((raw << 24) as i64) >> 24;
    }

    // The tail the vector body did not cover, by the oracle's own arithmetic.
    for &x in &a[body..] {
        let v = i32::from(x);
        total += i64::from(v * v);
    }
    total
}

/// `dot_i16` on the PIE unit, by the same accumulator as [`sum_sq_i16`].
///
/// Two loads and one multiply-accumulate per eight samples. The one
/// difference that matters: a dot product can be NEGATIVE, so the two halves
/// of the accumulator are recomposed as a signed 64-bit value rather than an
/// unsigned one — and the flush batch is sized for the magnitude either way,
/// since `|a*b| <= 2^30` and sixteen instructions add 128 of them.
#[allow(unsafe_code)]
#[must_use]
pub fn dot_i16(a: &[i16], b: &[i16]) -> i64 {
    let n = a.len().min(b.len());
    // NOT the fused form, and that is MEASURED. `ee.vmulas.s16.accx.ld.ip`
    // does the load and the eight-lane multiply-accumulate in one slot and
    // it is SLOWER here -- see the ledger's P6 refutation. Instruction count
    // went down and cycles went up in all three kernels tried.
    let body = n / 16 * 16;
    if body == 0 {
        return rusty_esp_dsp::sample::dot_i16(a, b);
    }
    if !aligned16(a.as_ptr().cast::<u8>()) || !aligned16(b.as_ptr().cast::<u8>()) {
        return dot_i16_unaligned(a, b);
    }

    let mut total: i64 = 0;
    let mut pa = a.as_ptr().cast::<u8>();
    let mut pb = b.as_ptr().cast::<u8>();
    let mut left = body / 16; // trips, two MACs each
    while left > 0 {
        // 16 instructions x 8 lanes = 128 products of at most 2^30, so the
        // partial peaks at 2^37 inside a 40-bit accumulator -- 4x headroom.
        // EIGHT trips = sixteen MACs = 128 products, peaking at 2^37 in
        // the 40-bit accumulator.
        let batch = left.min(8);
        left -= batch;
        let (lo, hi): (u32, u32);
        // SAFETY: the loop advances each pointer by 16 exactly `batch` times
        // and `batch` never exceeds what remains of `body`, so both stay
        // inside their slices. Both are 16-byte aligned. q0/q1 and ACCX only.
        unsafe {
            core::arch::asm!(
                "ee.zero.accx",
                "2:",
                "ee.vld.128.ip q0, {pa}, 16",
                "ee.vld.128.ip q1, {pb}, 16",
                "ee.vld.128.ip q2, {pa}, 16",
                "ee.vld.128.ip q3, {pb}, 16",
                "ee.vmulas.s16.accx q0, q1",
                "ee.vmulas.s16.accx q2, q3",
                "addi {n}, {n}, -1",
                "bnez {n}, 2b",
                "rur.accx_0 {l}",
                "rur.accx_1 {h}",
                pa = inout(reg) pa,
                pb = inout(reg) pb,
                n = inout(reg) batch => _,
                l = out(reg) lo,
                h = out(reg) hi,
                options(nostack),
            );
        }
        // ACCX is FORTY bits, and `rur.accx_1` delivers bits 32..=39
        // ZERO-extended into a 32-bit register -- an accumulator of -1 reads
        // back `accx_1 = 0x0000_00ff`, not `0xffff_ffff` (measured on the
        // part). So the halves compose into 40 bits and the sign has to be
        // put back by hand. `sum_sq_i16` got away without this only because
        // a sum of squares is never negative.
        let raw = (u64::from(hi & 0xff) << 32) | u64::from(lo);
        total += ((raw << 24) as i64) >> 24;
    }

    for k in body..n {
        total += i64::from(i32::from(a[k]) * i32::from(b[k]));
    }
    total
}

/// `sum_sq_i16_le` on the PIE unit: the same reduction over little-endian
/// bytes, which is the form `rms_dbfs_i16` (and therefore the VAD and the
/// AGC) actually calls.
///
/// The byte buffer IS a run of `i16`s, so when it views as one this is
/// [`sum_sq_i16`] and nothing else.
#[must_use]
pub fn sum_sq_i16_le(samples: &[u8]) -> (i64, usize) {
    // No alignment test: `sum_sq_i16` now has an unaligned arm of its own,
    // so the only question left is whether the bytes ARE samples.
    match rusty_esp_core::pcm::as_i16(samples) {
        Some(v) => (sum_sq_i16(v), v.len()),
        None => rusty_esp_dsp::sample::sum_sq_i16_le(samples),
    }
}

/// Saturating sum of two i16 streams -- the arithmetic `mix_i16` does.
///
/// `ee.vadds.s16` IS this operation: eight lanes, clamped to the i16 range,
/// one instruction. The scalar arm spends a widen, an add, two compares and
/// a narrow per sample to reach the same eight values.
#[allow(unsafe_code)]
pub fn mix_i16(a: &[i16], b: &[i16], out: &mut [i16]) {
    let n = a.len().min(b.len()).min(out.len());
    // NOT the fused `ee.vadds.s16.ld.incp`, which measured +64.6% here --
    // the worst of the three fused forms tried. See the P6 refutation.
    let body = n / 16 * 16;
    let vectorable = body > 0
        && aligned16(a.as_ptr().cast::<u8>())
        && aligned16(b.as_ptr().cast::<u8>())
        && aligned16(out.as_ptr().cast::<u8>());

    if vectorable {
        let mut pa = a.as_ptr().cast::<u8>();
        let mut pb = b.as_ptr().cast::<u8>();
        let mut po = out.as_mut_ptr().cast::<u8>();
        let mut left = body / 16;
        // SAFETY: each pointer advances by 32 exactly `body / 16` times and
        // `body` is a multiple of 8 samples no longer than the shortest
        // slice, so every access stays in bounds. All three are 16-byte
        // aligned. q0/q1/q2 only.
        unsafe {
            core::arch::asm!(
                "3:",
                "ee.vld.128.ip q0, {pa}, 16",
                "ee.vld.128.ip q1, {pb}, 16",
                "ee.vld.128.ip q2, {pa}, 16",
                "ee.vld.128.ip q3, {pb}, 16",
                "ee.vadds.s16 q0, q0, q1",
                "ee.vadds.s16 q2, q2, q3",
                "ee.vst.128.ip q0, {po}, 16",
                "ee.vst.128.ip q2, {po}, 16",
                "addi {n}, {n}, -1",
                "bnez {n}, 3b",
                pa = inout(reg) pa,
                pb = inout(reg) pb,
                po = inout(reg) po,
                n = inout(reg) left => _,
                options(nostack),
            );
        }
        let _ = (pa, pb, po);
    }

    let start = if vectorable {
        body
    } else {
        // Sources at any offset, destination aligned -- the shape a block
        // handed over by a pipeline or a ring buffer actually has.
        mix_i16_unaligned_src(a, b, out)
    };
    for k in start..n {
        let s = i32::from(a[k]) + i32::from(b[k]);
        out[k] = s.clamp(i32::from(i16::MIN), i32::from(i16::MAX)) as i16;
    }
}

/// Mono to stereo: every sample twice, in place of one load and two stores.
///
/// `ee.vzip.16` interleaves two registers into the pair -- so zipping a
/// register with a SECOND LOAD OF THE SAME ADDRESS interleaves it with
/// itself, which is duplication. Eight input samples become sixteen output
/// samples in five instructions, against the scalar arm's eight loads and
/// sixteen stores.
#[allow(unsafe_code)]
pub fn mono_to_stereo_i16(src: &[i16], dst: &mut [i16]) {
    let n = src.len().min(dst.len() / 2);
    // SIXTEEN a trip.
    let body = n / 16 * 16;
    let vectorable = body > 0
        && aligned16(src.as_ptr().cast::<u8>())
        && aligned16(dst.as_ptr().cast::<u8>());

    if vectorable {
        let mut ps = src.as_ptr().cast::<u8>();
        let mut pd = dst.as_mut_ptr().cast::<u8>();
        let mut left = body / 16;
        // SAFETY: the source advances 32 bytes and the destination 64 per
        // trip, `body / 8` times; `body <= n` and `dst` holds `2 * n`
        // samples, so both stay in bounds. Both are 16-byte aligned.
        unsafe {
            core::arch::asm!(
                "4:",
                "ee.vld.128.ip q0, {ps}, 0",  // the SAME sixteen bytes into
                "ee.vld.128.ip q1, {ps}, 16", // both halves of the zip pair
                "ee.vzip.16 q0, q1",
                "ee.vst.128.ip q0, {pd}, 16",
                "ee.vst.128.ip q1, {pd}, 16",
                "ee.vld.128.ip q2, {ps}, 0",
                "ee.vld.128.ip q3, {ps}, 16",
                "ee.vzip.16 q2, q3",
                "ee.vst.128.ip q2, {pd}, 16",
                "ee.vst.128.ip q3, {pd}, 16",
                "addi {n}, {n}, -1",
                "bnez {n}, 4b",
                ps = inout(reg) ps,
                pd = inout(reg) pd,
                n = inout(reg) left => _,
                options(nostack),
            );
        }
        let _ = (ps, pd);
    }

    let start = if vectorable {
        body
    } else {
        // Sources at any offset, destination aligned -- the shape a block
        // handed over by a pipeline or a ring buffer actually has.
        mono_to_stereo_i16_unaligned_src(src, dst)
    };
    for k in start..n {
        dst[k * 2] = src[k];
        dst[k * 2 + 1] = src[k];
    }
}

/// `rms_dbfs_i16` with the reduction on the PIE unit.
///
/// The f64 tail is character-for-character the scalar's, operating on the
/// same `i64` and the same count, so this is bit-identical by construction
/// and the whole win is [`sum_sq_i16_le`]'s. It is worth its own entry
/// because this -- not the reduction -- is the function the shipping
/// firmware's per-block loop calls.
#[must_use]
pub fn rms_dbfs_i16(samples: &[u8]) -> f32 {
    let (acc, n) = sum_sq_i16_le(samples);
    if n == 0 || acc == 0 {
        return -120.0;
    }
    let mean = acc as f64 / n as f64;
    let rms = libm::sqrt(mean) / 32768.0;
    (20.0 * libm::log10(rms)) as f32
}

/// Stereo to mono: `(l + r) >> 1` per frame, the arithmetic `StereoToMono`
/// does.
///
/// The obvious `ee.vadds.s16` is WRONG here and quietly so: two samples near
/// full scale sum past the i16 range, the instruction clamps to 32767, and
/// the shifted result is half what the scalar reports. The sum has to happen
/// in 32-bit lanes, which is what the sign-extending widen is for --
/// `ee.vcmp.lt.s16` against zero gives each lane's sign, `ee.vzip.16`
/// interleaves value with sign into correct i32 lanes, `ee.vadds.s32` cannot
/// saturate at these magnitudes, `ee.vsr.32` is an arithmetic shift and so
/// matches Rust's `>>` on a negative value, and `ee.vunzip.16` takes the low
/// halves back down. Eight frames a trip.
///
/// The block saves and restores SAR. `ssai` writes it, inline `asm!` on
/// Xtensa has no way to declare it clobbered, and the shift the compiler
/// emits on the other side of this block is entitled to assume it survived.
#[allow(unsafe_code)]
pub fn stereo_to_mono_i16(src: &[i16], dst: &mut [i16]) {
    let frames = (src.len() / 2).min(dst.len());
    let body = frames / 8 * 8;
    let vectorable = body > 0
        && aligned16(src.as_ptr().cast::<u8>())
        && aligned16(dst.as_ptr().cast::<u8>());

    if vectorable {
        let mut ps = src.as_ptr().cast::<u8>();
        let mut pd = dst.as_mut_ptr().cast::<u8>();
        let mut left = body / 8;
        // SAFETY: each trip reads 32 source bytes (8 frames) and writes 16,
        // `body / 8` times; `body <= src.len() / 2` and `body <= dst.len()`,
        // so both stay in bounds. Both are 16-byte aligned. q0-q3 and q6 are
        // the only vector registers touched, and SAR is restored.
        unsafe {
            core::arch::asm!(
                "rsr.sar {sar}",
                "ee.zero.q q6",              // the zero the sign compare needs
                "ssai 1",
                "5:",
                "ee.vld.128.ip q0, {ps}, 16",
                "ee.vld.128.ip q1, {ps}, 16",
                "ee.vunzip.16 q0, q1",       // q0 = the eight L, q1 = the eight R
                "ee.vcmp.lt.s16 q2, q0, q6", // all-ones where L is negative
                "ee.vcmp.lt.s16 q3, q1, q6",
                "ee.vzip.16 q0, q2",         // L widened to i32, low then high
                "ee.vzip.16 q1, q3",
                "ee.vadds.s32 q0, q0, q1",   // cannot saturate: |l+r| <= 65536
                "ee.vadds.s32 q2, q2, q3",
                "ee.vsr.32 q0, q0",          // arithmetic, so it floors like >>
                "ee.vsr.32 q2, q2",
                "ee.vunzip.16 q0, q2",       // low halves: the eight mono samples
                "ee.vst.128.ip q0, {pd}, 16",
                "addi {n}, {n}, -1",
                "bnez {n}, 5b",
                "wsr.sar {sar}",
                ps = inout(reg) ps,
                pd = inout(reg) pd,
                n = inout(reg) left => _,
                sar = out(reg) _,
                options(nostack),
            );
        }
        let _ = (ps, pd);
    }

    let start = if vectorable {
        body
    } else {
        // Sources at any offset, destination aligned -- the shape a block
        // handed over by a pipeline or a ring buffer actually has.
        stereo_to_mono_i16_unaligned_src(src, dst)
    };
    for k in start..frames {
        let l = i32::from(src[k * 2]);
        let r = i32::from(src[k * 2 + 1]);
        dst[k] = ((l + r) >> 1) as i16;
    }
}

/// 2x box downscale of a gray8 image: `(a + b + c + d + 2) / 4` per output
/// pixel.
///
/// Sixteen output pixels a trip, which is what makes the tail a 16-byte
/// store rather than a half-store of eight. The shape is: widen bytes to i16
/// against zero (`ee.vzip.8`, exact because the values are 0..=255), add the
/// two source rows, then `ee.vunzip.16` splits even columns from odd so that
/// one `ee.vadds.s16` finishes the horizontal pair. Sums reach 1020 and
/// cannot saturate an i16 lane.
///
/// The divide is the awkward part, because **there is no 16-bit shift**. The
/// sums widen again to 32-bit lanes -- against ZERO rather than a sign mask,
/// since a sum of four bytes is never negative -- take `+2`, shift right by
/// two, and come back through `ee.vunzip.16` and then `ee.vunzip.8`, whose
/// even bytes are the low byte of each result.
///
/// The constant 2 is BUILT rather than loaded: `ee.vcmp.eq.s16 q7, q6, q6`
/// is all-ones, `0 - (-1)` is one per 32-bit lane, and one left shift makes
/// it two. That avoids depending on `ee.vldbc.32`, whose semantics this
/// module has not measured.
#[allow(unsafe_code)]
pub fn downscale2x_gray8(src: &[u8], width: u32, height: u32, dst: &mut [u8]) -> Result<(), ()> {
    let (w, h) = (width as usize, height as usize);
    let (ow, oh) = (w / 2, h / 2);
    if src.len() < w * h || dst.len() < ow * oh {
        return Err(());
    }

    for oy in 0..oh {
        let r0 = &src[(2 * oy) * w..(2 * oy) * w + ow * 2];
        let r1 = &src[(2 * oy + 1) * w..(2 * oy + 1) * w + ow * 2];
        let drow = &mut dst[oy * ow..oy * ow + ow];

        // Per ROW, because a row's alignment depends on the stride and the
        // caller's base together. A row that does not qualify takes the
        // scalar arm; the image does not have to be all one or all the other.
        let aligned =
            aligned16(r0.as_ptr()) && aligned16(r1.as_ptr()) && aligned16(drow.as_ptr());
        // A row that is not aligned is no longer the oracle's: the unaligned
        // idiom reaches it, writes into `drow` itself, and reports how far it
        // got so the scalar tail can finish from there.
        let body = if aligned {
            ow / 16 * 16
        } else {
            downscale2x_gray8_unaligned_row(r0, r1, drow)
        };

        if aligned && body > 0 {
            let mut p0 = r0.as_ptr();
            let mut p1 = r1.as_ptr();
            let mut pd = drow.as_mut_ptr();
            let mut left = body / 16;
            // SAFETY: each trip reads 32 bytes from each source row and
            // writes 16 to the destination, `body / 16` times. `body <= ow`
            // and each source row is sliced to exactly `ow * 2` bytes, so
            // every access is inside the three slices. All three are 16-byte
            // aligned. q0-q7 are the only vector registers used, and SAR is
            // restored.
            unsafe {
                core::arch::asm!(
                    "rsr.sar {sar}",
                    "ee.zero.q q6",
                    "ee.vcmp.eq.s16 q7, q6, q6", // every lane equal -> all ones
                    "ee.vsubs.s32 q7, q6, q7",   // 0 - (-1) = 1 per 32-bit lane
                    "ssai 1",
                    "ee.vsl.32 q7, q7",          // ... and now 2, the round term
                    "ssai 2",
                    "6:",
                    // columns 0..=15 of the row pair -> outputs 0..=7
                    "ee.vld.128.ip q0, {p0}, 16",
                    "ee.vld.128.ip q1, {p1}, 16",
                    "ee.zero.q q2",
                    "ee.vzip.8 q0, q2",          // q0 = a0..a7, q2 = a8..a15
                    "ee.zero.q q3",
                    "ee.vzip.8 q1, q3",
                    "ee.vadds.s16 q0, q0, q1",   // the two rows, columns 0..=7
                    "ee.vadds.s16 q2, q2, q3",   // columns 8..=15
                    "ee.vunzip.16 q0, q2",       // even columns / odd columns
                    "ee.vadds.s16 q4, q0, q2",   // eight horizontal pairs <= 1020
                    // columns 16..=31 -> outputs 8..=15
                    "ee.vld.128.ip q0, {p0}, 16",
                    "ee.vld.128.ip q1, {p1}, 16",
                    "ee.zero.q q2",
                    "ee.vzip.8 q0, q2",
                    "ee.zero.q q3",
                    "ee.vzip.8 q1, q3",
                    "ee.vadds.s16 q0, q0, q1",
                    "ee.vadds.s16 q2, q2, q3",
                    "ee.vunzip.16 q0, q2",
                    "ee.vadds.s16 q5, q0, q2",
                    // (sum + 2) >> 2, in 32-bit lanes because there is no
                    // 16-bit shift; the sums are non-negative so ZERO, not a
                    // sign mask, is the correct high half
                    "ee.zero.q q0",
                    "ee.vzip.16 q4, q0",
                    "ee.zero.q q1",
                    "ee.vzip.16 q5, q1",
                    "ee.vadds.s32 q4, q4, q7",
                    "ee.vadds.s32 q0, q0, q7",
                    "ee.vadds.s32 q5, q5, q7",
                    "ee.vadds.s32 q1, q1, q7",
                    "ee.vsr.32 q4, q4",
                    "ee.vsr.32 q0, q0",
                    "ee.vsr.32 q5, q5",
                    "ee.vsr.32 q1, q1",
                    "ee.vunzip.16 q4, q0",       // back to i16: outputs 0..=7
                    "ee.vunzip.16 q5, q1",       // outputs 8..=15
                    "ee.vunzip.8 q4, q5",        // low byte of each: the pixels
                    "ee.vst.128.ip q4, {pd}, 16",
                    "addi {n}, {n}, -1",
                    "bnez {n}, 6b",
                    "wsr.sar {sar}",
                    p0 = inout(reg) p0,
                    p1 = inout(reg) p1,
                    pd = inout(reg) pd,
                    n = inout(reg) left => _,
                    sar = out(reg) _,
                    options(nostack),
                );
            }
            let _ = (p0, p1, pd);
        }

        for ox in body..ow {
            let s = u32::from(r0[2 * ox])
                + u32::from(r0[2 * ox + 1])
                + u32::from(r1[2 * ox])
                + u32::from(r1[2 * ox + 1]);
            drow[ox] = ((s + 2) / 4) as u8;
        }
    }
    Ok(())
}

/// The number of trailing samples an unaligned arm must leave to the scalar
/// tail.
///
/// `ee.ld.128.usar.ip` loads the 16-byte-aligned block CONTAINING its
/// address, so producing the last unaligned window reads up to one whole
/// block past the last sample the kernel consumes. Reading past the end of
/// a slice is undefined behaviour whatever the hardware does with it, so
/// every unaligned body stops eight samples (sixteen bytes) short and the
/// scalar loop finishes the job.
const UNALIGNED_TAIL: usize = 8;

/// `sum_sq_i16` for a buffer that is NOT 16-byte aligned.
///
/// Until now any such buffer took the scalar path in full. The unaligned
/// idiom is two loads and a funnel: `ee.ld.128.usar.ip` loads the aligned
/// block containing the address AND sets SAR_BYTE from the low four bits,
/// and `ee.src.q` shifts the pair by that amount. Measured on the part,
/// offset 3 yields bytes 3..=18 and offset 7 yields 7..=22.
///
/// The loop is unrolled by two so the block loaded for one window is the
/// low half of the next, which is what avoids needing a register move: the
/// roles of the two block registers simply swap each half-trip.
///
/// Sixteen samples a trip, eight trips to a flush -- 128 products of at most
/// 2^30 peak at 2^37 inside the 40-bit accumulator.
#[allow(unsafe_code)]
fn sum_sq_i16_unaligned(a: &[i16]) -> i64 {
    let n = a.len();
    if n < UNALIGNED_TAIL + 16 {
        return rusty_esp_dsp::sample::sum_sq_i16(a);
    }
    // Widened, and with DISJOINT registers for the two halves. Reusing the
    // same window register would make the second `ee.src.q` wait for the
    // first half's read of it -- a false dependency, which is the shape that
    // made `gain_i16` slower when it was widened.
    let body = (n - UNALIGNED_TAIL) / 32 * 32;
    let mut total: i64 = 0;
    let mut p = a.as_ptr().cast::<u8>();
    let mut left = body / 32;

    while left > 0 {
        // FOUR trips = sixteen MACs = 128 products, peaking at 2^37.
        let batch = left.min(4);
        left -= batch;
        let (lo, hi): (u32, u32);
        // SAFETY: the loop consumes 64 bytes a trip and reads at most one
        // 16-byte block beyond them; `body` stops `UNALIGNED_TAIL` samples
        // short of the slice end precisely so that block is still inside it.
        // q0-q2 and ACCX only, and SAR is restored.
        unsafe {
            core::arch::asm!(
                "rsr.sar {sar}",
                "ee.zero.accx",
                "ee.ld.128.usar.ip q0, {p}, 16",
                "7:",
                "ee.ld.128.usar.ip q1, {p}, 16",
                "ee.src.q q2, q0, q1",
                "ee.vmulas.s16.accx q2, q2",
                "ee.ld.128.usar.ip q0, {p}, 16",
                "ee.src.q q3, q1, q0",
                "ee.vmulas.s16.accx q3, q3",
                "ee.ld.128.usar.ip q1, {p}, 16",
                "ee.src.q q5, q0, q1",
                "ee.vmulas.s16.accx q5, q5",
                "ee.ld.128.usar.ip q0, {p}, 16",
                "ee.src.q q6, q1, q0",
                "ee.vmulas.s16.accx q6, q6",
                "addi {n}, {n}, -1",
                "bnez {n}, 7b",
                "rur.accx_0 {l}",
                "rur.accx_1 {h}",
                "wsr.sar {sar}",
                p = inout(reg) p,
                n = inout(reg) batch => _,
                l = out(reg) lo,
                h = out(reg) hi,
                sar = out(reg) _,
                options(nostack),
            );
        }
        // Non-negative, so the 40-bit composition needs no sign extension.
        total += (((u64::from(hi) & 0xff) << 32) | u64::from(lo)) as i64;
        // Each trip consumed 32 bytes but left the pointer one block ahead.
        p = unsafe { a.as_ptr().cast::<u8>().add((body / 32 - left) * 64) };
    }

    for &x in &a[body..] {
        total += i64::from(i32::from(x) * i32::from(x));
    }
    total
}

/// `peak_abs_i16` for a buffer that is NOT 16-byte aligned.
///
/// The same unaligned idiom in front of the same lane-wise MAX and MIN that
/// the aligned twin uses -- the minimum is carried because negating through
/// `ee.vsubs.s16` saturates at `-32768` and would report 32767 where the
/// oracle reports 32768.
#[allow(unsafe_code)]
fn peak_abs_i16_unaligned(a: &[i16]) -> u16 {
    let n = a.len();
    if n < UNALIGNED_TAIL + 16 {
        return rusty_esp_dsp::sample::peak_abs_i16(a);
    }
    // Widened, and with DISJOINT registers for the two halves. Reusing the
    // same window register would make the second `ee.src.q` wait for the
    // first half's read of it -- a false dependency, which is the shape that
    // made `gain_i16` slower when it was widened.
    let body = (n - UNALIGNED_TAIL) / 32 * 32;
    let mut maxes = [0i16; 8];
    let mut mins = [0i16; 8];
    let mut p = a.as_ptr().cast::<u8>();
    let mut left = body / 32;
    // SAFETY: as for `sum_sq_i16_unaligned` -- 32 bytes consumed a trip and
    // at most one block read beyond, which `UNALIGNED_TAIL` reserves. The
    // two output buffers are 16 bytes each. q0-q4 only; SAR is restored.
    unsafe {
        core::arch::asm!(
            "rsr.sar {sar}",
            "ee.zero.q q3",                 // running lane-wise maximum
            "ee.zero.q q4",                 // running lane-wise minimum
            "ee.ld.128.usar.ip q0, {p}, 16",
            "8:",
            "ee.ld.128.usar.ip q1, {p}, 16",
            "ee.src.q q2, q0, q1",
            "ee.vmax.s16 q3, q3, q2",
            "ee.vmin.s16 q4, q4, q2",
            "ee.ld.128.usar.ip q0, {p}, 16",
            "ee.src.q q5, q1, q0",
            "ee.vmax.s16 q3, q3, q5",
            "ee.vmin.s16 q4, q4, q5",
            "ee.ld.128.usar.ip q1, {p}, 16",
            "ee.src.q q6, q0, q1",
            "ee.vmax.s16 q3, q3, q6",
            "ee.vmin.s16 q4, q4, q6",
            "ee.ld.128.usar.ip q0, {p}, 16",
            "ee.src.q q7, q1, q0",
            "ee.vmax.s16 q3, q3, q7",
            "ee.vmin.s16 q4, q4, q7",
            "addi {n}, {n}, -1",
            "bnez {n}, 8b",
            "ee.vst.128.ip q3, {mx}, 0",
            "ee.vst.128.ip q4, {mn}, 0",
            "wsr.sar {sar}",
            p = inout(reg) p,
            n = inout(reg) left => _,
            mx = inout(reg) maxes.as_mut_ptr() => _,
            mn = inout(reg) mins.as_mut_ptr() => _,
            sar = out(reg) _,
            options(nostack),
        );
    }

    let mut best: u16 = 0;
    for k in 0..8 {
        best = best.max(maxes[k].unsigned_abs());
        best = best.max(mins[k].unsigned_abs());
    }
    for &x in &a[body..] {
        best = best.max(x.unsigned_abs());
    }
    best
}

/// `dot_i16` for operands that are NOT both 16-byte aligned.
///
/// Two independent unaligned streams need two different SAR_BYTE values, and
/// they get them for free: `ee.ld.128.usar.ip` sets SAR_BYTE as a side
/// effect of every load, so as long as each `ee.src.q` immediately follows
/// its own stream's load, each funnel shifts by its own offset.
#[allow(unsafe_code)]
fn dot_i16_unaligned(a: &[i16], b: &[i16]) -> i64 {
    let n = a.len().min(b.len());
    if n < UNALIGNED_TAIL + 8 {
        return rusty_esp_dsp::sample::dot_i16(a, b);
    }
    // SIXTEEN a trip, as the aligned arm now is.
    let body = (n - UNALIGNED_TAIL) / 16 * 16;
    let mut total: i64 = 0;
    let mut done = 0usize;

    while done < body {
        let batch = (body - done) / 16;
        // EIGHT trips = sixteen MACs = 128 products, peaking at 2^37.
        let batch = batch.min(8);
        let (lo, hi): (u32, u32);
        let mut pa = unsafe { a.as_ptr().cast::<u8>().add(done * 2) };
        let mut pb = unsafe { b.as_ptr().cast::<u8>().add(done * 2) };
        // SAFETY: each trip consumes 16 bytes of each operand and reads at
        // most one 16-byte block beyond, which `UNALIGNED_TAIL` reserves in
        // both slices. q0-q4 and ACCX only; SAR is restored.
        unsafe {
            core::arch::asm!(
                "rsr.sar {sar}",
                "ee.zero.accx",
                "ee.ld.128.usar.ip q0, {pa}, 16",
                "ee.ld.128.usar.ip q2, {pb}, 16",
                "9:",
                "ee.ld.128.usar.ip q1, {pa}, 0",
                "ee.src.q q4, q0, q1",       // SAR_BYTE is a's, set just above
                "ee.ld.128.usar.ip q3, {pb}, 0",
                "ee.src.q q0, q2, q3",       // and now b's
                "ee.vmulas.s16.accx q4, q0",
                "ee.ld.128.usar.ip q0, {pa}, 16",
                "ee.ld.128.usar.ip q2, {pb}, 16",
                "ee.ld.128.usar.ip q1, {pa}, 0",
                "ee.src.q q4, q0, q1",       // SAR_BYTE is a's, set just above
                "ee.ld.128.usar.ip q3, {pb}, 0",
                "ee.src.q q0, q2, q3",       // and now b's
                "ee.vmulas.s16.accx q4, q0",
                "ee.ld.128.usar.ip q0, {pa}, 16",
                "ee.ld.128.usar.ip q2, {pb}, 16",
                "addi {n}, {n}, -1",
                "bnez {n}, 9b",
                "rur.accx_0 {l}",
                "rur.accx_1 {h}",
                "wsr.sar {sar}",
                pa = inout(reg) pa,
                pb = inout(reg) pb,
                n = inout(reg) batch => _,
                l = out(reg) lo,
                h = out(reg) hi,
                sar = out(reg) _,
                options(nostack),
            );
        }
        let _ = (pa, pb);
        // Forty bits, and a dot product can be negative: mask and sign-extend.
        let raw = (u64::from(hi & 0xff) << 32) | u64::from(lo);
        total += ((raw << 24) as i64) >> 24;
        done += batch * 16;
    }

    for k in done..n {
        total += i64::from(i32::from(a[k]) * i32::from(b[k]));
    }
    total
}

/// `sad_16x16` for blocks at ARBITRARY positions -- which, in a motion
/// search, is all of them.
///
/// The aligned twin demands `stride % 16 == 0` and both bases 16-byte
/// aligned. A current block at a macroblock boundary usually satisfies that;
/// a reference block at a candidate motion vector essentially never does, so
/// the search half of the work has been taking the oracle in full.
///
/// Each row is one unaligned window: two `ee.ld.128.usar.ip` and one
/// `ee.src.q`, then the pointer steps by `stride - 16` because the loads
/// already advanced it by 16. The two streams interleave safely because
/// every `ee.src.q` sits immediately after its own stream's load, and so
/// funnels by its own SAR_BYTE.
///
/// The arithmetic is the aligned twin's, unchanged: widen against zero with
/// `ee.vzip.8` (exact for `0..=255`), subtract in `s16` where the difference
/// cannot saturate, and take the magnitude as `max(d, 0 - d)` where `-255..=255`
/// cannot saturate either.
///
/// **Requires 16 bytes of slack past the oracle's bound.** Producing the
/// last row's window reads the aligned block CONTAINING its last byte, which
/// can end 15 bytes beyond it. Reading past a slice is undefined behaviour
/// however the hardware behaves, so a block without that slack -- the last
/// one in a buffer sized exactly to the oracle's bound -- takes the scalar
/// arm.
#[allow(unsafe_code)]
fn sad_16x16_unaligned(a: &[u8], sa: usize, b: &[u8], sb: usize) -> u32 {
    if a.len() < 15 * sa + 32 || b.len() < 15 * sb + 32 {
        return rusty_esp_dsp::block::sad_16x16(a, sa, b, sb).unwrap_or(0);
    }
    #[repr(align(16))]
    struct Q([i16; 8]);
    let mut acc_lo = Q([0; 8]);
    let mut acc_hi = Q([0; 8]);
    let mut pa = a.as_ptr();
    let mut pb = b.as_ptr();
    let adja = sa.wrapping_sub(16);
    let adjb = sb.wrapping_sub(16);
    let mut rows = 16u32;
    // SAFETY: the bound checked above guarantees `15*stride + 32` readable
    // bytes from each base, and the loop touches at most `15*stride + 31`.
    // The unaligned idiom needs no alignment of either base or stride.
    // q0-q7 are the only vector registers used, and SAR is restored.
    unsafe {
        core::arch::asm!(
            "rsr.sar {sar}",
            "ee.zero.q q4",                  // accumulator, low eight lanes
            "ee.zero.q q5",                  // accumulator, high eight lanes
            "10:",
            "ee.ld.128.usar.ip q0, {pa}, 16",
            "ee.ld.128.usar.ip q1, {pa}, 0",
            "ee.src.q q6, q0, q1",           // sixteen bytes of a, any offset
            "add {pa}, {pa}, {adja}",
            "ee.ld.128.usar.ip q0, {pb}, 16",
            "ee.ld.128.usar.ip q1, {pb}, 0",
            "ee.src.q q7, q0, q1",           // and of b, at its own offset
            "add {pb}, {pb}, {adjb}",
            "ee.zero.q q2",
            "ee.vzip.8 q6, q2",              // a widened: low eight, high eight
            "ee.zero.q q3",
            "ee.vzip.8 q7, q3",
            "ee.vsubs.s16 q6, q6, q7",       // exact: 0..=255 minus 0..=255
            "ee.vsubs.s16 q2, q2, q3",
            "ee.zero.q q7",
            "ee.vsubs.s16 q1, q7, q6",       // -d, exact on -255..=255
            "ee.vmax.s16 q6, q6, q1",
            "ee.vsubs.s16 q3, q7, q2",
            "ee.vmax.s16 q2, q2, q3",
            "ee.vadds.s16 q4, q4, q6",       // 16 rows x 255 = 4080, fits s16
            "ee.vadds.s16 q5, q5, q2",
            "addi {n}, {n}, -1",
            "bnez {n}, 10b",
            "ee.vst.128.ip q4, {lo}, 0",
            "ee.vst.128.ip q5, {hi}, 0",
            "wsr.sar {sar}",
            pa = inout(reg) pa,
            pb = inout(reg) pb,
            adja = in(reg) adja,
            adjb = in(reg) adjb,
            n = inout(reg) rows,
            lo = inout(reg) acc_lo.0.as_mut_ptr() => _,
            hi = inout(reg) acc_hi.0.as_mut_ptr() => _,
            sar = out(reg) _,
            options(nostack),
        );
    }
    let _ = (pa, pb, rows);
    let mut total = 0u32;
    for k in 0..8 {
        total += u32::from(acc_lo.0[k] as u16) + u32::from(acc_hi.0[k] as u16);
    }
    total
}

/// `sad_8x8` at an arbitrary position, by the same idiom.
///
/// Eight bytes a row, so only the LOW half of each widened window carries
/// data: `ee.vzip.8 q6, q2` puts the first eight bytes in `q6` and the eight
/// that were read past the row in `q2`, and `q2` is simply never read. The
/// junk costs one instruction and no correctness.
#[allow(unsafe_code)]
fn sad_8x8_unaligned(a: &[u8], sa: usize, b: &[u8], sb: usize) -> u32 {
    if a.len() < 7 * sa + 32 || b.len() < 7 * sb + 32 {
        return rusty_esp_dsp::block::sad_8x8(a, sa, b, sb).unwrap_or(0);
    }
    #[repr(align(16))]
    struct Q([i16; 8]);
    let mut acc = Q([0; 8]);
    let mut pa = a.as_ptr();
    let mut pb = b.as_ptr();
    let adja = sa.wrapping_sub(16);
    let adjb = sb.wrapping_sub(16);
    let mut rows = 8u32;
    // SAFETY: the bound checked above guarantees `7*stride + 32` readable
    // bytes from each base; the loop touches at most `7*stride + 31`.
    // q0-q7 only, and SAR is restored.
    unsafe {
        core::arch::asm!(
            "rsr.sar {sar}",
            "ee.zero.q q4",
            "11:",
            "ee.ld.128.usar.ip q0, {pa}, 16",
            "ee.ld.128.usar.ip q1, {pa}, 0",
            "ee.src.q q6, q0, q1",
            "add {pa}, {pa}, {adja}",
            "ee.ld.128.usar.ip q0, {pb}, 16",
            "ee.ld.128.usar.ip q1, {pb}, 0",
            "ee.src.q q7, q0, q1",
            "add {pb}, {pb}, {adjb}",
            "ee.zero.q q2",
            "ee.vzip.8 q6, q2",              // q2 holds the bytes past the row
            "ee.zero.q q3",
            "ee.vzip.8 q7, q3",
            "ee.vsubs.s16 q6, q6, q7",
            "ee.zero.q q7",
            "ee.vsubs.s16 q1, q7, q6",
            "ee.vmax.s16 q6, q6, q1",
            "ee.vadds.s16 q4, q4, q6",       // 8 rows x 255 = 2040, fits s16
            "addi {n}, {n}, -1",
            "bnez {n}, 11b",
            "ee.vst.128.ip q4, {lo}, 0",
            "wsr.sar {sar}",
            pa = inout(reg) pa,
            pb = inout(reg) pb,
            adja = in(reg) adja,
            adjb = in(reg) adjb,
            n = inout(reg) rows,
            lo = inout(reg) acc.0.as_mut_ptr() => _,
            sar = out(reg) _,
            options(nostack),
        );
    }
    let _ = (pa, pb, rows);
    let mut total = 0u32;
    for k in 0..8 {
        total += u32::from(acc.0[k] as u16);
    }
    total
}

/// Gather the even bytes of an UNALIGNED `src` into an aligned `dst`.
///
/// The load side is the unaligned idiom and the store side is unchanged,
/// which is the shape every kernel in this module can take: `ee.src.q`
/// reaches any source offset for three instructions per window, while an
/// unaligned STORE has no equivalent on this unit -- `ee.vst.128` requires
/// alignment and the alternatives are a lane extract and a byte store each.
/// So a kernel whose destination is misaligned still belongs to the oracle;
/// one whose source alone is misaligned does not.
///
/// The caller has already reserved sixteen pixels of tail. Two windows a
/// trip, which is what `ee.vunzip.8` needs to fill a whole 16-byte store.
#[allow(unsafe_code)]
fn simd_even_bytes_unaligned_src(src: &[u8], dst: &mut [u8]) {
    debug_assert_eq!(src.len(), dst.len() * 2);
    debug_assert_eq!(dst.len() % 16, 0);
    debug_assert!(aligned16(dst.as_mut_ptr()));

    let mut s = src.as_ptr();
    let mut d = dst.as_mut_ptr();
    // Widened, and with DISJOINT registers for the two halves. Reusing the
    // same window register would make the second `ee.src.q` wait for the
    // first half's read of it -- a false dependency, which is the shape that
    // made `gain_i16` slower when it was widened.
    let mut left = dst.len() / 32;
    if left == 0 {
        return;
    }
    // SAFETY: each trip consumes 32 source bytes and writes 16, `left`
    // times. The caller sliced `src` to exactly `2 * dst.len()` and reserved
    // sixteen pixels beyond it, which covers the one aligned block this
    // idiom reads past the last byte consumed. `dst` is 16-byte aligned.
    // q0-q3 only, and SAR is restored.
    unsafe {
        core::arch::asm!(
            "rsr.sar {sar}",
            "ee.ld.128.usar.ip q0, {s}, 16",
            "12:",
            "ee.ld.128.usar.ip q1, {s}, 16",
            "ee.src.q q2, q0, q1",        // source bytes 0..=15 of the trip
            "ee.ld.128.usar.ip q0, {s}, 16",
            "ee.src.q q3, q1, q0",        // and 16..=31
            "ee.vunzip.8 q2, q3",         // q2 = the sixteen even bytes: luma
            "ee.vst.128.ip q2, {d}, 16",
            "ee.ld.128.usar.ip q1, {s}, 16",
            "ee.src.q q4, q0, q1",
            "ee.ld.128.usar.ip q0, {s}, 16",
            "ee.src.q q5, q1, q0",
            "ee.vunzip.8 q4, q5",
            "ee.vst.128.ip q4, {d}, 16",
            "addi {n}, {n}, -1",
            "bnez {n}, 12b",
            "wsr.sar {sar}",
            s = inout(reg) s,
            d = inout(reg) d,
            n = inout(reg) left => _,
            sar = out(reg) _,
            options(nostack),
        );
    }
    let _ = (s, d);
}

/// `mix_i16` with UNALIGNED sources and an aligned destination.
///
/// The destination is the caller's output block, which is allocated and so
/// aligned; the two inputs are whatever the pipeline handed over, and a
/// `PcmBlock` pointing part-way into a ring buffer is not aligned to
/// anything. Two streams at two different offsets, each `ee.src.q` placed
/// immediately after its own stream's load so each funnels by its own
/// SAR_BYTE.
#[allow(unsafe_code)]
fn mix_i16_unaligned_src(a: &[i16], b: &[i16], out: &mut [i16]) -> usize {
    let n = a.len().min(b.len()).min(out.len());
    if n < UNALIGNED_TAIL + 16 || !aligned16(out.as_ptr().cast::<u8>()) {
        return 0;
    }
    // SIXTEEN a trip, as the aligned arm now is.
    let body = (n - UNALIGNED_TAIL) / 16 * 16;
    let mut pa = a.as_ptr().cast::<u8>();
    let mut pb = b.as_ptr().cast::<u8>();
    let mut po = out.as_mut_ptr().cast::<u8>();
    let mut left = body / 16;
    // SAFETY: each trip consumes 16 bytes of each source and writes 16, and
    // reads at most one 16-byte block beyond what it consumes -- which is
    // what `UNALIGNED_TAIL` reserves in both inputs. `out` is 16-byte
    // aligned and `body <= out.len()`. q0-q5 only; SAR is restored.
    unsafe {
        core::arch::asm!(
            "rsr.sar {sar}",
            "ee.ld.128.usar.ip q0, {pa}, 16",
            "ee.ld.128.usar.ip q2, {pb}, 16",
            "13:",
            "ee.ld.128.usar.ip q1, {pa}, 0",
            "ee.src.q q4, q0, q1",           // a's window, at a's offset
            "ee.ld.128.usar.ip q3, {pb}, 0",
            "ee.src.q q5, q2, q3",           // b's window, at b's offset
            "ee.vadds.s16 q4, q4, q5",
            "ee.vst.128.ip q4, {po}, 16",
            "ee.ld.128.usar.ip q0, {pa}, 16",
            "ee.ld.128.usar.ip q2, {pb}, 16",
            "ee.ld.128.usar.ip q1, {pa}, 0",
            "ee.src.q q4, q0, q1",           // a's window, at a's offset
            "ee.ld.128.usar.ip q3, {pb}, 0",
            "ee.src.q q5, q2, q3",           // b's window, at b's offset
            "ee.vadds.s16 q4, q4, q5",
            "ee.vst.128.ip q4, {po}, 16",
            "ee.ld.128.usar.ip q0, {pa}, 16",
            "ee.ld.128.usar.ip q2, {pb}, 16",
            "addi {n}, {n}, -1",
            "bnez {n}, 13b",
            "wsr.sar {sar}",
            pa = inout(reg) pa,
            pb = inout(reg) pb,
            po = inout(reg) po,
            n = inout(reg) left => _,
            sar = out(reg) _,
            options(nostack),
        );
    }
    let _ = (pa, pb, po);
    body
}

/// `mono_to_stereo_i16` from an UNALIGNED source.
///
/// `ee.vzip.16` needs the same data in both halves of the pair, and this
/// unit has no register-move instruction -- so the copy is made by running
/// `ee.src.q` **twice on the same pair**, which recomputes the identical
/// window into a second register for one instruction. Cheaper than a
/// round trip through memory and it needs no scratch.
#[allow(unsafe_code)]
fn mono_to_stereo_i16_unaligned_src(src: &[i16], dst: &mut [i16]) -> usize {
    let n = src.len().min(dst.len() / 2);
    if n < UNALIGNED_TAIL + 8 || !aligned16(dst.as_ptr().cast::<u8>()) {
        return 0;
    }
    // Widened, and with DISJOINT registers for the two halves. Reusing the
    // same window register would make the second `ee.src.q` wait for the
    // first half's read of it -- a false dependency, which is the shape that
    // made `gain_i16` slower when it was widened.
    let body = (n - UNALIGNED_TAIL) / 16 * 16;
    let mut ps = src.as_ptr().cast::<u8>();
    let mut pd = dst.as_mut_ptr().cast::<u8>();
    let mut left = body / 16;
    // SAFETY: each trip consumes 16 source bytes and writes 32, reading at
    // most one block beyond -- reserved by `UNALIGNED_TAIL`. `dst` holds
    // `2 * n` samples and is 16-byte aligned. q0-q3 only; SAR is restored.
    unsafe {
        core::arch::asm!(
            "rsr.sar {sar}",
            "ee.ld.128.usar.ip q0, {ps}, 16",
            "14:",
            "ee.ld.128.usar.ip q1, {ps}, 0",
            "ee.src.q q2, q0, q1",
            "ee.src.q q3, q0, q1",           // the same window again: a copy
            "ee.vzip.16 q2, q3",             // every lane twice
            "ee.vst.128.ip q2, {pd}, 16",
            "ee.vst.128.ip q3, {pd}, 16",
            "ee.ld.128.usar.ip q0, {ps}, 16",
            "ee.ld.128.usar.ip q1, {ps}, 0",
            "ee.src.q q4, q0, q1",
            "ee.src.q q5, q0, q1",
            "ee.vzip.16 q4, q5",
            "ee.vst.128.ip q4, {pd}, 16",
            "ee.vst.128.ip q5, {pd}, 16",
            "ee.ld.128.usar.ip q0, {ps}, 16",
            "addi {n}, {n}, -1",
            "bnez {n}, 14b",
            "wsr.sar {sar}",
            ps = inout(reg) ps,
            pd = inout(reg) pd,
            n = inout(reg) left => _,
            sar = out(reg) _,
            options(nostack),
        );
    }
    let _ = (ps, pd);
    body
}

/// `stereo_to_mono_i16` from an UNALIGNED source.
///
/// Two windows a trip because a frame is two samples, then the same
/// 32-bit-lane downmix the aligned twin uses -- `ee.vadds.s16` would clamp
/// two near-full-scale samples and report half the right answer.
#[allow(unsafe_code)]
fn stereo_to_mono_i16_unaligned_src(src: &[i16], dst: &mut [i16]) -> usize {
    let frames = (src.len() / 2).min(dst.len());
    if frames < UNALIGNED_TAIL + 8 || !aligned16(dst.as_ptr().cast::<u8>()) {
        return 0;
    }
    let body = (frames - UNALIGNED_TAIL) / 8 * 8;
    let mut ps = src.as_ptr().cast::<u8>();
    let mut pd = dst.as_mut_ptr().cast::<u8>();
    let mut left = body / 8;
    // SAFETY: each trip consumes 32 source bytes (8 frames) and writes 16,
    // reading at most one block beyond -- reserved by `UNALIGNED_TAIL`.
    // `dst` is 16-byte aligned. q0-q6 only; SAR is restored.
    unsafe {
        core::arch::asm!(
            "rsr.sar {sar}",
            "ee.zero.q q6",
            "ee.ld.128.usar.ip q0, {ps}, 16",
            "15:",
            "ee.ld.128.usar.ip q1, {ps}, 16",
            "ee.src.q q2, q0, q1",           // frames 0..=3 interleaved
            "ee.ld.128.usar.ip q0, {ps}, 16",
            "ee.src.q q3, q1, q0",           // frames 4..=7
            "ee.vunzip.16 q2, q3",           // q2 = the eight L, q3 = the eight R
            "ee.vcmp.lt.s16 q4, q2, q6",
            "ee.vcmp.lt.s16 q5, q3, q6",
            "ee.vzip.16 q2, q4",             // L sign-extended into i32 lanes
            "ee.vzip.16 q3, q5",
            "ee.vadds.s32 q2, q2, q3",
            "ee.vadds.s32 q4, q4, q5",
            "ssai 1",
            "ee.vsr.32 q2, q2",              // arithmetic: floors like `>>`
            "ee.vsr.32 q4, q4",
            "ee.vunzip.16 q2, q4",           // low halves: eight mono samples
            "ee.vst.128.ip q2, {pd}, 16",
            "addi {n}, {n}, -1",
            "bnez {n}, 15b",
            "wsr.sar {sar}",
            ps = inout(reg) ps,
            pd = inout(reg) pd,
            n = inout(reg) left => _,
            sar = out(reg) _,
            options(nostack),
        );
    }
    let _ = (ps, pd);
    body
}

/// `downscale2x_gray8` for source rows at ANY offset, destination aligned.
///
/// The row-alignment test in the aligned twin depends on the caller's base
/// AND the stride together, so a frame whose width is not a multiple of 32
/// fails it on every second row. The unaligned idiom removes the question.
///
/// The register budget is the reason this is a separate body rather than a
/// flag: the aligned twin already uses all eight, and three more are needed
/// for the two funnels. The allocation that fits is q4/q5 for the two halves
/// of eight sums, q7 for the round term, and q0-q3 plus q6 recycled as load
/// scratch, widening scratch and zero.
#[allow(unsafe_code)]
fn downscale2x_gray8_unaligned_row(r0: &[u8], r1: &[u8], drow: &mut [u8]) -> usize {
    let ow = drow.len();
    if ow < 32 || !aligned16(drow.as_ptr()) {
        return 0;
    }
    // Sixteen output pixels a trip; leave one trip's worth of slack so the
    // last window's trailing block stays inside the row slices.
    let body = (ow - 16) / 16 * 16;
    if body == 0 {
        return 0;
    }
    let mut p0 = r0.as_ptr();
    let mut p1 = r1.as_ptr();
    let mut pd = drow.as_mut_ptr();
    let mut left = body / 16;
    // SAFETY: each trip consumes 32 bytes of each source row and writes 16,
    // and reads at most one 16-byte block past what it consumes -- which the
    // 16-pixel reservation above covers, since each row slice is `2 * ow`
    // bytes. `drow` is 16-byte aligned. q0-q7 only; SAR is restored.
    unsafe {
        core::arch::asm!(
            "rsr.sar {sar}",
            "ee.zero.q q6",
            "ee.vcmp.eq.s16 q7, q6, q6", // all ones
            "ee.vsubs.s32 q7, q6, q7",   // one per 32-bit lane
            "ssai 1",
            "ee.vsl.32 q7, q7",          // two: the round term
            "16:",
            // ---- columns 0..=15 -> outputs 0..=7 ----
            "ee.ld.128.usar.ip q0, {p0}, 16",
            "ee.ld.128.usar.ip q1, {p0}, 0",
            "ee.src.q q6, q0, q1",       // sixteen bytes of row 0
            "ee.ld.128.usar.ip q0, {p1}, 16",
            "ee.ld.128.usar.ip q1, {p1}, 0",
            "ee.src.q q2, q0, q1",       // and of row 1
            "ee.zero.q q3",
            "ee.vzip.8 q6, q3",          // row0 widened: low eight, high eight
            "ee.zero.q q0",
            "ee.vzip.8 q2, q0",          // row1 widened
            "ee.vadds.s16 q6, q6, q2",   // vertical sum, columns 0..=7
            "ee.vadds.s16 q3, q3, q0",   // columns 8..=15
            "ee.vunzip.16 q6, q3",       // even columns / odd columns
            "ee.vadds.s16 q4, q6, q3",   // eight horizontal pairs, <= 1020
            // ---- columns 16..=31 -> outputs 8..=15 ----
            "ee.ld.128.usar.ip q0, {p0}, 16",
            "ee.ld.128.usar.ip q1, {p0}, 0",
            "ee.src.q q6, q0, q1",
            "ee.ld.128.usar.ip q0, {p1}, 16",
            "ee.ld.128.usar.ip q1, {p1}, 0",
            "ee.src.q q2, q0, q1",
            "ee.zero.q q3",
            "ee.vzip.8 q6, q3",
            "ee.zero.q q0",
            "ee.vzip.8 q2, q0",
            "ee.vadds.s16 q6, q6, q2",
            "ee.vadds.s16 q3, q3, q0",
            "ee.vunzip.16 q6, q3",
            "ee.vadds.s16 q5, q6, q3",
            // ---- (sum + 2) >> 2 in 32-bit lanes; sums are non-negative ----
            "ee.zero.q q0",
            "ee.vzip.16 q4, q0",
            "ee.zero.q q1",
            "ee.vzip.16 q5, q1",
            "ee.vadds.s32 q4, q4, q7",
            "ee.vadds.s32 q0, q0, q7",
            "ee.vadds.s32 q5, q5, q7",
            "ee.vadds.s32 q1, q1, q7",
            "ssai 2",
            "ee.vsr.32 q4, q4",
            "ee.vsr.32 q0, q0",
            "ee.vsr.32 q5, q5",
            "ee.vsr.32 q1, q1",
            "ee.vunzip.16 q4, q0",
            "ee.vunzip.16 q5, q1",
            "ee.vunzip.8 q4, q5",        // low byte of each: the sixteen pixels
            "ee.vst.128.ip q4, {pd}, 16",
            "addi {n}, {n}, -1",
            "bnez {n}, 16b",
            "wsr.sar {sar}",
            p0 = inout(reg) p0,
            p1 = inout(reg) p1,
            pd = inout(reg) pd,
            n = inout(reg) left => _,
            sar = out(reg) _,
            options(nostack),
        );
    }
    let _ = (p0, p1, pd);
    body
}

/// `Gain`'s per-sample arithmetic: `(x * g + (1 << 14)) >> 15`, clamped to
/// i16.
///
/// This is a lane-wise fixed-point multiply, which on this unit is QACC's
/// job and not `ee.vmul.s16`'s — that instruction keeps the low half and
/// **wraps**, so it is wrong everywhere the scalar clamps.
///
/// The whole expression is three instructions for eight samples:
/// `ee.vmulas.s16.qacc q, qG` accumulates `x * g` into eight 40-bit lanes,
/// a second MAC of `16384 x 1` adds the rounding term to every lane, and
/// `ee.srcmb.s16.qacc` shifts right by 15 and clamps to i16 in one go. It
/// **floors**, which is what Rust's `>>` does on a negative value, so the
/// rounding matches at every sign — measured on the part, not assumed.
///
/// **Domain: `|q15| <= 32767`.** The multiplier has to live in an i16 lane.
/// The scalar's narrow path admits `|g| < 65536`, so gains above unity by
/// more than a hair take the oracle; attenuation, which is what an AGC
/// spends its life doing, is entirely inside the domain. Above 32767 the
/// caller gets the scalar arm and the same bytes.
///
/// In-domain no clamp can ever fire: `|x| <= 32768` and `|g| <= 32767` give
/// `|x*g| <= 2^30`, and `(2^30 + 2^14) >> 15 < 32768`. The clamp is kept
/// because it is free — `ee.srcmb` does it as part of the shift.
#[allow(unsafe_code)]
pub fn gain_i16(src: &[i16], q15: i32, dst: &mut [i16]) {
    let n = src.len().min(dst.len());
    if q15.unsigned_abs() > 32767 {
        for k in 0..n {
            let y = (i32::from(src[k]) * q15 + (1 << 14)) >> 15;
            dst[k] = y.clamp(-32768, 32767) as i16;
        }
        return;
    }

    // The three broadcast sources, adjacent so one register plus an offset
    // reaches them all.
    #[repr(align(16))]
    struct C([i16; 4]);
    let c = C([q15 as i16, 16384, 1, 0]);

    // EIGHT a trip, and MEASURED so. Sixteen read 27,329 against this
    // arm's 24,297 ps/sample -- +12.5% WORSE -- because QACC is a single
    // resource: the second half's `ee.zero.qacc` waits on the first half's
    // `ee.srcmb`, so widening doubles a serial dependency chain to save two
    // instructions of loop. The unrolling that paid in seven other kernels
    // does not pay where the body is a chain through one accumulator.
    let body = n / 8 * 8;
    let aligned = body > 0
        && aligned16(src.as_ptr().cast::<u8>())
        && aligned16(dst.as_ptr().cast::<u8>());

    let done = if aligned {
        let mut ps = src.as_ptr().cast::<u8>();
        let mut pd = dst.as_mut_ptr().cast::<u8>();
        let mut left = body / 8;
        // SAFETY: eight samples a trip, `body / 8` times, and `body <= n <=`
        // both slice lengths; both are 16-byte aligned. `c` is a live local
        // read two bytes at a time at offsets 0, 2 and 4 of four. q0/q1 and
        // q5-q7 only.
        unsafe {
            core::arch::asm!(
                "ee.vldbc.16 q5, {pg}",       // the gain, in every lane
                "addi {pg}, {pg}, 2",
                "ee.vldbc.16 q6, {pg}",       // 16384, the rounding term
                "addi {pg}, {pg}, 2",
                "ee.vldbc.16 q7, {pg}",       // 1, its multiplicand
                "17:",
                "ee.zero.qacc",
                "ee.vld.128.ip q0, {ps}, 16",
                "ee.vmulas.s16.qacc q0, q5",  // x * g, eight 40-bit lanes
                "ee.vmulas.s16.qacc q6, q7",  // + (1 << 14) in every lane
                "ee.srcmb.s16.qacc q1, {sh}, 0", // >> 15, floored, clamped
                "ee.vst.128.ip q1, {pd}, 16",
                "addi {n}, {n}, -1",
                "bnez {n}, 17b",
                pg = inout(reg) c.0.as_ptr() => _,
                ps = inout(reg) ps,
                pd = inout(reg) pd,
                sh = in(reg) 15u32,
                n = inout(reg) left => _,
                options(nostack),
            );
        }
        let _ = (ps, pd);
        body
    } else {
        gain_i16_unaligned_src(src, &c.0, dst)
    };

    for k in done..n {
        let y = (i32::from(src[k]) * q15 + (1 << 14)) >> 15;
        dst[k] = y.clamp(-32768, 32767) as i16;
    }
}

/// `gain_i16` for a source at any offset with an aligned destination.
#[allow(unsafe_code)]
fn gain_i16_unaligned_src(src: &[i16], c: &[i16; 4], dst: &mut [i16]) -> usize {
    let n = src.len().min(dst.len());
    if n < UNALIGNED_TAIL + 8 || !aligned16(dst.as_ptr().cast::<u8>()) {
        return 0;
    }
    let body = (n - UNALIGNED_TAIL) / 8 * 8;
    let mut ps = src.as_ptr().cast::<u8>();
    let mut pd = dst.as_mut_ptr().cast::<u8>();
    let mut left = body / 8;
    // SAFETY: eight samples a trip, reading at most one 16-byte block past
    // what is consumed -- reserved by `UNALIGNED_TAIL`. `dst` is 16-byte
    // aligned. q0-q2 and q5-q7 only; SAR is restored.
    unsafe {
        core::arch::asm!(
            "rsr.sar {sar}",
            "ee.vldbc.16 q5, {pg}",
            "addi {pg}, {pg}, 2",
            "ee.vldbc.16 q6, {pg}",
            "addi {pg}, {pg}, 2",
            "ee.vldbc.16 q7, {pg}",
            "ee.ld.128.usar.ip q0, {ps}, 16",
            "18:",
            "ee.zero.qacc",
            "ee.ld.128.usar.ip q1, {ps}, 0",
            "ee.src.q q2, q0, q1",
            "ee.vmulas.s16.qacc q2, q5",
            "ee.vmulas.s16.qacc q6, q7",
            "ee.srcmb.s16.qacc q2, {sh}, 0",
            "ee.vst.128.ip q2, {pd}, 16",
            "ee.ld.128.usar.ip q0, {ps}, 16",
            "addi {n}, {n}, -1",
            "bnez {n}, 18b",
            "wsr.sar {sar}",
            pg = inout(reg) c.as_ptr() => _,
            ps = inout(reg) ps,
            pd = inout(reg) pd,
            sh = in(reg) 15u32,
            n = inout(reg) left => _,
            sar = out(reg) _,
            options(nostack),
        );
    }
    let _ = (ps, pd);
    body
}

/// `convert` I16 -> I32 (and I16 -> I24In32, which is the same rule).
///
/// The scalar writes `(i32::from(x)) << 16`. On a little-endian machine that
/// value's four bytes are `[0, 0, lo(x), hi(x)]` -- which is exactly `x`
/// interleaved with a ZERO halfword in the low position. So the shift, the
/// sign-extension and the widening are all one `ee.vzip.16` against a zeroed
/// register, and no shift instruction is involved at all.
///
/// `ee.vzip.16` writes BOTH registers of the pair, so one zip produces all
/// eight results: the zeroed register comes back holding the first four and
/// the source register the second four. Five instructions for eight samples,
/// against a scalar arm that marshals two bytes in and four bytes out each.
#[allow(unsafe_code)]
pub fn convert_i16_to_i32(src: &[i16], dst: &mut [i32]) {
    let n = src.len().min(dst.len());
    // SIXTEEN a trip, not the width this was first written at. The body was
    // mostly LOOP: see the `sum_sq_i16` note and `codec-measurement` 2b --
    // the unaligned arms had been written wider and were measuring faster
    // despite doing more work per element.
    let body = n / 16 * 16;
    let vectorable = body > 0
        && aligned16(src.as_ptr().cast::<u8>())
        && aligned16(dst.as_ptr().cast::<u8>());

    if vectorable {
        let mut ps = src.as_ptr().cast::<u8>();
        let mut pd = dst.as_mut_ptr().cast::<u8>();
        let mut left = body / 16;
        // SAFETY: sixteen samples a trip -- 32 bytes read, 64 written --
        // `body / 8` times, and `body` is within both slices. Both are
        // 16-byte aligned. q0 and q1 only.
        unsafe {
            core::arch::asm!(
                "19:",
                "ee.vld.128.ip q0, {ps}, 16",
                "ee.vld.128.ip q2, {ps}, 16",
                "ee.zero.q q1",
                "ee.zero.q q3",
                "ee.vzip.16 q1, q0",         // [0,x0,0,x1,..] = x<<16 per i32
                "ee.vzip.16 q3, q2",
                "ee.vst.128.ip q1, {pd}, 16",
                "ee.vst.128.ip q0, {pd}, 16",
                "ee.vst.128.ip q3, {pd}, 16",
                "ee.vst.128.ip q2, {pd}, 16",
                "addi {n}, {n}, -1",
                "bnez {n}, 19b",
                ps = inout(reg) ps,
                pd = inout(reg) pd,
                n = inout(reg) left => _,
                options(nostack),
            );
        }
        let _ = (ps, pd);
    }

    let start = if vectorable {
        body
    } else {
        // Source at any offset, destination aligned.
        convert_i16_to_i32_unaligned_src(src, dst)
    };
    for k in start..n {
        dst[k] = i32::from(src[k]) << 16;
    }
}

/// `convert` I32 -> I16 (and I24In32 -> I16).
///
/// The scalar writes `(x >> 16) as i16`, an ARITHMETIC shift followed by a
/// truncation -- which together are simply the HIGH halfword of each i32.
/// `ee.vunzip.16` splits a register pair into even and odd 16-bit lanes, and
/// on a little-endian machine the odd lanes ARE the high halves. One
/// instruction, no shift, and the sign comes along because it was never
/// separated from the value.
#[allow(unsafe_code)]
pub fn convert_i32_to_i16(src: &[i32], dst: &mut [i16]) {
    let n = src.len().min(dst.len());
    // SIXTEEN a trip, not the width this was first written at. The body was
    // mostly LOOP: see the `sum_sq_i16` note and `codec-measurement` 2b --
    // the unaligned arms had been written wider and were measuring faster
    // despite doing more work per element.
    let body = n / 16 * 16;
    let vectorable = body > 0
        && aligned16(src.as_ptr().cast::<u8>())
        && aligned16(dst.as_ptr().cast::<u8>());

    if vectorable {
        let mut ps = src.as_ptr().cast::<u8>();
        let mut pd = dst.as_mut_ptr().cast::<u8>();
        let mut left = body / 16;
        // SAFETY: sixteen samples a trip -- 64 bytes read, 32 written --
        // `body / 8` times, within both slices, both 16-byte aligned.
        // q0 and q1 only.
        unsafe {
            core::arch::asm!(
                "20:",
                "ee.vld.128.ip q0, {ps}, 16",
                "ee.vld.128.ip q1, {ps}, 16",
                "ee.vld.128.ip q2, {ps}, 16",
                "ee.vld.128.ip q3, {ps}, 16",
                "ee.vunzip.16 q0, q1",       // q1 = the eight high halfwords
                "ee.vunzip.16 q2, q3",
                "ee.vst.128.ip q1, {pd}, 16",
                "ee.vst.128.ip q3, {pd}, 16",
                "addi {n}, {n}, -1",
                "bnez {n}, 20b",
                ps = inout(reg) ps,
                pd = inout(reg) pd,
                n = inout(reg) left => _,
                options(nostack),
            );
        }
        let _ = (ps, pd);
    }

    let start = if vectorable {
        body
    } else {
        // Source at any offset, destination aligned.
        convert_i32_to_i16_unaligned_src(src, dst)
    };
    for k in start..n {
        dst[k] = (src[k] >> 16) as i16;
    }
}

/// `convert` I32 -> I24In32: keep the top three bytes, zero the low one.
///
/// The scalar writes `[0, i[1], i[2], i[3]]`, which is a mask. The mask
/// itself is BUILT rather than loaded: all-ones shifted left by eight in
/// 32-bit lanes is `0xffff_ff00`, and both halves of that are instructions
/// this module has measured.
#[allow(unsafe_code)]
pub fn convert_i32_to_i24in32(src: &[i32], dst: &mut [i32]) {
    let n = src.len().min(dst.len());
    // SIXTEEN a trip, not the width this was first written at. The body was
    // mostly LOOP: see the `sum_sq_i16` note and `codec-measurement` 2b --
    // the unaligned arms had been written wider and were measuring faster
    // despite doing more work per element.
    // This one was the narrowest of all at FOUR: a load, an and and a
    // store against two instructions of loop -- forty per cent overhead.
    let body = n / 16 * 16;
    let vectorable = body > 0
        && aligned16(src.as_ptr().cast::<u8>())
        && aligned16(dst.as_ptr().cast::<u8>());

    if vectorable {
        let mut ps = src.as_ptr().cast::<u8>();
        let mut pd = dst.as_mut_ptr().cast::<u8>();
        let mut left = body / 16;
        // SAFETY: four samples a trip -- 16 bytes read and written --
        // `body / 4` times, within both slices, both 16-byte aligned.
        // q0, q6 and q7 only; SAR is restored.
        unsafe {
            core::arch::asm!(
                "rsr.sar {sar}",
                "ee.zero.q q6",
                "ee.vcmp.eq.s16 q7, q6, q6", // all ones
                "ssai 8",
                "ee.vsl.32 q7, q7",          // 0xffff_ff00 per 32-bit lane
                "21:",
                "ee.vld.128.ip q0, {ps}, 16",
                "ee.vld.128.ip q1, {ps}, 16",
                "ee.vld.128.ip q2, {ps}, 16",
                "ee.vld.128.ip q3, {ps}, 16",
                "ee.andq q0, q0, q7",
                "ee.andq q1, q1, q7",
                "ee.andq q2, q2, q7",
                "ee.andq q3, q3, q7",
                "ee.vst.128.ip q0, {pd}, 16",
                "ee.vst.128.ip q1, {pd}, 16",
                "ee.vst.128.ip q2, {pd}, 16",
                "ee.vst.128.ip q3, {pd}, 16",
                "addi {n}, {n}, -1",
                "bnez {n}, 21b",
                "wsr.sar {sar}",
                ps = inout(reg) ps,
                pd = inout(reg) pd,
                n = inout(reg) left => _,
                sar = out(reg) _,
                options(nostack),
            );
        }
        let _ = (ps, pd);
    }

    let start = if vectorable {
        body
    } else {
        // Source at any offset, destination aligned.
        convert_i32_to_i24in32_unaligned_src(src, dst)
    };
    for k in start..n {
        let b = src[k].to_le_bytes();
        dst[k] = i32::from_le_bytes([0, b[1], b[2], b[3]]);
    }
}

/// `convert_i16_to_i32` from an unaligned source.
#[allow(unsafe_code)]
fn convert_i16_to_i32_unaligned_src(src: &[i16], dst: &mut [i32]) -> usize {
    let n = src.len().min(dst.len());
    if n < UNALIGNED_TAIL + 8 || !aligned16(dst.as_ptr().cast::<u8>()) {
        return 0;
    }
    // Widened, and with DISJOINT registers for the two halves. Reusing the
    // same window register would make the second `ee.src.q` wait for the
    // first half's read of it -- a false dependency, which is the shape that
    // made `gain_i16` slower when it was widened.
    let body = (n - UNALIGNED_TAIL) / 16 * 16;
    let mut ps = src.as_ptr().cast::<u8>();
    let mut pd = dst.as_mut_ptr().cast::<u8>();
    let mut left = body / 16;
    // SAFETY: sixteen samples a trip -- 32 source bytes consumed, 64 written --
    // reading at most one 16-byte block beyond, reserved by `UNALIGNED_TAIL`.
    // `dst` is 16-byte aligned. q0-q3 only; SAR is restored.
    unsafe {
        core::arch::asm!(
            "rsr.sar {sar}",
            "ee.ld.128.usar.ip q0, {ps}, 16",
            "22:",
            "ee.ld.128.usar.ip q1, {ps}, 0",
            "ee.src.q q2, q0, q1",
            "ee.zero.q q3",
            "ee.vzip.16 q3, q2",
            "ee.vst.128.ip q3, {pd}, 16",
            "ee.vst.128.ip q2, {pd}, 16",
            "ee.ld.128.usar.ip q0, {ps}, 16",
            "ee.ld.128.usar.ip q1, {ps}, 0",
            "ee.src.q q4, q0, q1",
            "ee.zero.q q5",
            "ee.vzip.16 q5, q4",
            "ee.vst.128.ip q5, {pd}, 16",
            "ee.vst.128.ip q4, {pd}, 16",
            "ee.ld.128.usar.ip q0, {ps}, 16",
            "addi {n}, {n}, -1",
            "bnez {n}, 22b",
            "wsr.sar {sar}",
            ps = inout(reg) ps,
            pd = inout(reg) pd,
            n = inout(reg) left => _,
            sar = out(reg) _,
            options(nostack),
        );
    }
    let _ = (ps, pd);
    body
}

/// `convert_i32_to_i16` from an unaligned source.
#[allow(unsafe_code)]
fn convert_i32_to_i16_unaligned_src(src: &[i32], dst: &mut [i16]) -> usize {
    let n = src.len().min(dst.len());
    if n < UNALIGNED_TAIL + 8 || !aligned16(dst.as_ptr().cast::<u8>()) {
        return 0;
    }
    // Widened, and with DISJOINT registers for the two halves. Reusing the
    // same window register would make the second `ee.src.q` wait for the
    // first half's read of it -- a false dependency, which is the shape that
    // made `gain_i16` slower when it was widened.
    let body = (n - UNALIGNED_TAIL) / 16 * 16;
    let mut ps = src.as_ptr().cast::<u8>();
    let mut pd = dst.as_mut_ptr().cast::<u8>();
    let mut left = body / 16;
    // SAFETY: sixteen samples a trip -- 64 source bytes consumed, 32 written --
    // reading at most one block beyond, reserved by `UNALIGNED_TAIL`. `dst`
    // is 16-byte aligned. q0-q3 only; SAR is restored.
    unsafe {
        core::arch::asm!(
            "rsr.sar {sar}",
            "ee.ld.128.usar.ip q0, {ps}, 16",
            "23:",
            "ee.ld.128.usar.ip q1, {ps}, 16",
            "ee.src.q q2, q0, q1",
            "ee.ld.128.usar.ip q0, {ps}, 16",
            "ee.src.q q3, q1, q0",
            "ee.vunzip.16 q2, q3",       // q3 = the eight high halfwords
            "ee.vst.128.ip q3, {pd}, 16",
            "ee.ld.128.usar.ip q1, {ps}, 16",
            "ee.src.q q4, q0, q1",
            "ee.ld.128.usar.ip q0, {ps}, 16",
            "ee.src.q q5, q1, q0",
            "ee.vunzip.16 q4, q5",
            "ee.vst.128.ip q5, {pd}, 16",
            "addi {n}, {n}, -1",
            "bnez {n}, 23b",
            "wsr.sar {sar}",
            ps = inout(reg) ps,
            pd = inout(reg) pd,
            n = inout(reg) left => _,
            sar = out(reg) _,
            options(nostack),
        );
    }
    let _ = (ps, pd);
    body
}

/// `convert_i32_to_i24in32` from an unaligned source.
#[allow(unsafe_code)]
fn convert_i32_to_i24in32_unaligned_src(src: &[i32], dst: &mut [i32]) -> usize {
    let n = src.len().min(dst.len());
    if n < UNALIGNED_TAIL + 16 || !aligned16(dst.as_ptr().cast::<u8>()) {
        return 0;
    }
    // SIXTEEN a trip: this body was five instructions against two of loop.
    let body = (n - UNALIGNED_TAIL) / 16 * 16;
    let mut ps = src.as_ptr().cast::<u8>();
    let mut pd = dst.as_mut_ptr().cast::<u8>();
    let mut left = body / 16;
    // SAFETY: sixteen samples a trip -- 64 bytes consumed and written --
    // reading at most one block beyond, reserved by `UNALIGNED_TAIL`. `dst`
    // is 16-byte aligned. q0-q2 and q6/q7 only; SAR is restored.
    unsafe {
        core::arch::asm!(
            "rsr.sar {sar}",
            "ee.zero.q q6",
            "ee.vcmp.eq.s16 q7, q6, q6",
            "ssai 8",
            "ee.vsl.32 q7, q7",          // 0xffff_ff00 per 32-bit lane
            "ee.ld.128.usar.ip q0, {ps}, 16",
            "24:",
            "ee.ld.128.usar.ip q1, {ps}, 0",
            "ee.src.q q2, q0, q1",
            "ee.andq q2, q2, q7",
            "ee.vst.128.ip q2, {pd}, 16",
            "ee.ld.128.usar.ip q0, {ps}, 16",
            "ee.ld.128.usar.ip q1, {ps}, 0",
            "ee.src.q q2, q0, q1",
            "ee.andq q2, q2, q7",
            "ee.vst.128.ip q2, {pd}, 16",
            "ee.ld.128.usar.ip q0, {ps}, 16",
            "ee.ld.128.usar.ip q1, {ps}, 0",
            "ee.src.q q2, q0, q1",
            "ee.andq q2, q2, q7",
            "ee.vst.128.ip q2, {pd}, 16",
            "ee.ld.128.usar.ip q0, {ps}, 16",
            "ee.ld.128.usar.ip q1, {ps}, 0",
            "ee.src.q q2, q0, q1",
            "ee.andq q2, q2, q7",
            "ee.vst.128.ip q2, {pd}, 16",
            "ee.ld.128.usar.ip q0, {ps}, 16",
            "addi {n}, {n}, -1",
            "bnez {n}, 24b",
            "wsr.sar {sar}",
            ps = inout(reg) ps,
            pd = inout(reg) pd,
            n = inout(reg) left => _,
            sar = out(reg) _,
            options(nostack),
        );
    }
    let _ = (ps, pd);
    body
}

/// `rotate90_gray8`: `dst[x * h + (h - 1 - y)] = src[y * w + x]`.
///
/// A transpose with the destination column REVERSED, and the reversal is
/// free: this unit has no byte-reverse instruction, but loading the eight
/// source rows of a tile BOTTOM-TO-TOP puts the transposed bytes out in
/// exactly the order the destination run wants them, so the write is
/// contiguous and forward. The `h - 1 - y` term never appears in the kernel.
///
/// The 8x8 byte transpose is eight instructions. `ee.vzip.8/16/32` perform a
/// perfect shuffle across the register PAIR, which is precisely the three
/// stages of a transpose at 1-, 2- and 4-byte granularity:
///
/// ```text
///   stage 1   zip.8  (r0,r1) (r2,r3) (r4,r5) (r6,r7)
///   stage 2   zip.16 (A,B)   (C,D)
///   stage 3   zip.32 (A2,C2) (B2,D2)
/// ```
///
/// After stage 3 each register holds two whole destination runs back to
/// back, so the stores are `ee.vst.l.64` and `ee.vst.h.64` -- one half of a
/// register each, which is the shape a 64-bit run needs.
///
/// **Precondition: `w % 8 == 0`, `h % 8 == 0`, and both bases 16-byte
/// aligned.** Then every 64-bit access this makes is 8-byte aligned: a
/// source row starts at `y*w + x0` with `x0` a multiple of 8, and a
/// destination run at `x*h + (h - 8 - y0)` with `y0` a multiple of 8.
/// Anything else takes the oracle — and the A/B prints the preconditions
/// beside the verdict so a buffer that quietly failed them could not be
/// mistaken for a measurement of this code.
#[allow(unsafe_code)]
pub fn rotate90_gray8(src: &[u8], width: u32, height: u32, dst: &mut [u8]) -> Result<(), ()> {
    let (w, h) = (width as usize, height as usize);
    if src.len() < w * h || dst.len() < w * h {
        return Err(());
    }
    if w == 0 || h == 0 {
        return Ok(());
    }
    if w % 8 != 0 || h % 8 != 0 || !aligned16(src.as_ptr()) || !aligned16(dst.as_ptr()) {
        // The oracle's own loop, byte for byte.
        for x in 0..w {
            for y in 0..h {
                dst[x * h + (h - 1 - y)] = src[y * w + x];
            }
        }
        return Ok(());
    }

    // The tile loop stays in RUST, and that is a MEASURED choice.
    //
    // Moving it into the asm -- which is what made `yuyv_to_gray8` 25.6%
    // faster -- made this kernel 9.3% SLOWER (10,199 against 9,332 ps/px),
    // and the reason is the opposite of a saving. Rust recomputes both
    // cursors from `x0` and `y0` for every tile, so a tile's first load
    // depends on nothing the previous tile did. An in-asm loop has to walk
    // them incrementally, so the next tile's first load waits on the
    // previous tile's last store.
    //
    // The general form: removing a host loop wins when the address is
    // already a running cursor (`yuyv_to_gray8`, whose pointer only ever
    // increments), and loses when the host was computing an INDEPENDENT
    // address each trip. Redundant-looking arithmetic can be breaking a
    // dependency chain.
    let mut x0 = 0usize;
    while x0 < w {
        let mut y0 = 0usize;
        while y0 < h {
            // The BOTTOM row of the tile, walked upward.
            let mut ps = unsafe { src.as_ptr().add((y0 + 7) * w + x0) };
            let mut pd = unsafe { dst.as_mut_ptr().add(x0 * h + (h - 8 - y0)) };
            let back = w.wrapping_neg();
            // SAFETY: `w % 8 == 0` and `h % 8 == 0` with `x0 < w`, `y0 < h`
            // stepping by 8, so the eight source rows `(y0+7-k)*w + x0` and
            // the eight destination runs `(x0+x)*h + (h-8-y0)` are all fully
            // inside the `w * h` bytes checked above. Every address is
            // 8-byte aligned because both bases are 16-byte aligned and both
            // strides are multiples of 8. q0-q7 only.
            unsafe {
                core::arch::asm!(
                    // eight rows, bottom to top, into the low half of each
                    "ee.vld.l.64.ip q0, {ps}, 0",
                    "add {ps}, {ps}, {back}",
                    "ee.vld.l.64.ip q1, {ps}, 0",
                    "add {ps}, {ps}, {back}",
                    "ee.vld.l.64.ip q2, {ps}, 0",
                    "add {ps}, {ps}, {back}",
                    "ee.vld.l.64.ip q3, {ps}, 0",
                    "add {ps}, {ps}, {back}",
                    "ee.vld.l.64.ip q4, {ps}, 0",
                    "add {ps}, {ps}, {back}",
                    "ee.vld.l.64.ip q5, {ps}, 0",
                    "add {ps}, {ps}, {back}",
                    "ee.vld.l.64.ip q6, {ps}, 0",
                    "add {ps}, {ps}, {back}",
                    "ee.vld.l.64.ip q7, {ps}, 0",
                    // the transpose: three stages of perfect shuffle
                    "ee.vzip.8 q0, q1",
                    "ee.vzip.8 q2, q3",
                    "ee.vzip.8 q4, q5",
                    "ee.vzip.8 q6, q7",
                    "ee.vzip.16 q0, q2",
                    "ee.vzip.16 q4, q6",
                    "ee.vzip.32 q0, q4",   // q0 = cols 0,1   q4 = cols 2,3
                    "ee.vzip.32 q2, q6",   // q2 = cols 4,5   q6 = cols 6,7
                    // eight destination runs, one per column, stride h
                    "ee.vst.l.64.ip q0, {pd}, 0",
                    "add {pd}, {pd}, {hh}",
                    "ee.vst.h.64.ip q0, {pd}, 0",
                    "add {pd}, {pd}, {hh}",
                    "ee.vst.l.64.ip q4, {pd}, 0",
                    "add {pd}, {pd}, {hh}",
                    "ee.vst.h.64.ip q4, {pd}, 0",
                    "add {pd}, {pd}, {hh}",
                    "ee.vst.l.64.ip q2, {pd}, 0",
                    "add {pd}, {pd}, {hh}",
                    "ee.vst.h.64.ip q2, {pd}, 0",
                    "add {pd}, {pd}, {hh}",
                    "ee.vst.l.64.ip q6, {pd}, 0",
                    "add {pd}, {pd}, {hh}",
                    "ee.vst.h.64.ip q6, {pd}, 0",
                    ps = inout(reg) ps,
                    pd = inout(reg) pd,
                    back = in(reg) back,
                    hh = in(reg) h,
                    options(nostack),
                );
            }
            let _ = (ps, pd);
            y0 += 8;
        }
        x0 += 8;
    }
    Ok(())
}
