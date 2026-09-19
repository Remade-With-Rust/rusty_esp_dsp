//! Reductions over 16-bit PCM — the loops an AGC, a VAD or a level meter
//! spends its time in — and, under [`pcm`], sample-format conversion.
//!
//! [`rms_dbfs_i16`] moved verbatim from `rusty_esp_audio-core` (D0,
//! 2026-09-02); its integer accumulation is now [`sum_sq_i16_le`], which the
//! oracle test in `tests/moved.rs` compares with the original over a
//! generated corpus. The float tail uses `libm` so a level reads the same on
//! the host and on a chip.

pub mod pcm;

/// `Σ a[i] · b[i]` over the shorter of the two slices, exact in `i64`.
///
/// The PRODUCT is formed in `i32` and only then widened: two `i16`s multiply
/// to at most `32 768 · 32 768 = 2^30`, which is exact in `i32`, so the value
/// is identical — but a 32-bit core does it in one `mull` instead of the
/// multi-instruction 64×64 sequence `i64 · i64` compiles to. The ACCUMULATOR
/// stays `i64`, which is where the range is genuinely needed.
/// Four independent accumulators, because a single `acc += ...` chain is a
/// loop-carried dependency: each add waits on the previous one, and an
/// in-order core stalls on it. Integer addition is associative and the i64
/// accumulators cannot overflow here, so the total is identical.
#[must_use]
pub fn dot_i16(a: &[i16], b: &[i16]) -> i64 {
    let n = a.len().min(b.len());
    let (a, b) = (&a[..n], &b[..n]);
    let (mut a0, mut a1, mut a2, mut a3) = (0i64, 0i64, 0i64, 0i64);
    let mut ca = a.chunks_exact(4);
    let mut cb = b.chunks_exact(4);
    for (x, y) in ca.by_ref().zip(cb.by_ref()) {
        a0 += i64::from(i32::from(x[0]) * i32::from(y[0]));
        a1 += i64::from(i32::from(x[1]) * i32::from(y[1]));
        a2 += i64::from(i32::from(x[2]) * i32::from(y[2]));
        a3 += i64::from(i32::from(x[3]) * i32::from(y[3]));
    }
    let mut tail = 0i64;
    for (&x, &y) in ca.remainder().iter().zip(cb.remainder()) {
        tail += i64::from(i32::from(x) * i32::from(y));
    }
    (a0 + a1) + (a2 + a3) + tail
}

/// `Σ a[i]²`, exact in `i64`. The square is formed in `i32` (at most `2^30`,
/// see [`dot_i16`]) and widened for the accumulate.
#[must_use]
pub fn sum_sq_i16(a: &[i16]) -> i64 {
    let (mut a0, mut a1, mut a2, mut a3) = (0i64, 0i64, 0i64, 0i64);
    let mut c = a.chunks_exact(4);
    for x in c.by_ref() {
        let (v0, v1) = (i32::from(x[0]), i32::from(x[1]));
        let (v2, v3) = (i32::from(x[2]), i32::from(x[3]));
        a0 += i64::from(v0 * v0);
        a1 += i64::from(v1 * v1);
        a2 += i64::from(v2 * v2);
        a3 += i64::from(v3 * v3);
    }
    let mut tail = 0i64;
    for &x in c.remainder() {
        let v = i32::from(x);
        tail += i64::from(v * v);
    }
    (a0 + a1) + (a2 + a3) + tail
}

/// Sum of squares and sample count over little-endian `i16` bytes; a
/// trailing odd byte is ignored.
#[must_use]
pub fn sum_sq_i16_le(samples: &[u8]) -> (i64, usize) {
    // FAST ARM: the samples ARE i16s; reassembling each from two bytes costs
    // a second load, a shift and an or. The byte arm below is the oracle and
    // takes a misaligned or odd-length buffer.
    if let Some(v) = rusty_esp_core::pcm::as_i16(samples) {
        // The aligned arm IS `sum_sq_i16` -- same four accumulators, same
        // u32 square, same tail. It was written out here first, which made it
        // a hand-rolled twin of a kernel three lines up; calling it instead
        // means one loop to optimise and one to gate, and it gives
        // `sum_sq_i16` the production caller it did not have.
        return (sum_sq_i16(v), v.len());
    }

    // Four samples (eight bytes) per trip into four independent
    // accumulators: the square is exact in i32 (at most 2^30) and the
    // accumulator chain is broken, for the same reasons as `sum_sq_i16`.
    let (mut a0, mut a1, mut a2, mut a3) = (0i64, 0i64, 0i64, 0i64);
    let mut n: usize = 0;
    let mut c = samples.chunks_exact(8);
    for s in c.by_ref() {
        let v0 = i32::from(i16::from_le_bytes([s[0], s[1]]));
        let v1 = i32::from(i16::from_le_bytes([s[2], s[3]]));
        let v2 = i32::from(i16::from_le_bytes([s[4], s[5]]));
        let v3 = i32::from(i16::from_le_bytes([s[6], s[7]]));
        a0 += i64::from(v0 * v0);
        a1 += i64::from(v1 * v1);
        a2 += i64::from(v2 * v2);
        a3 += i64::from(v3 * v3);
        n += 4;
    }
    let mut tail = 0i64;
    for s in c.remainder().chunks_exact(2) {
        let v = i32::from(i16::from_le_bytes([s[0], s[1]]));
        tail += i64::from(v * v);
        n += 1;
    }
    ((a0 + a1) + (a2 + a3) + tail, n)
}

/// The largest magnitude in the block (`32 768` for `i16::MIN`).
///
/// Four independent running maxima for the same reason as [`dot_i16`]'s four
/// accumulators: `max` is associative and commutative, so the answer is
/// identical, but one running maximum is a loop-carried dependency.
#[must_use]
pub fn peak_abs_i16(a: &[i16]) -> u16 {
    // EIGHT lanes: eight u16 maxima fit the register window where eight i64
    // accumulators do not, so this wins (-9.0%) where the same widening LOSES
    // on dot_i16 and sum_sq_i16.
    let mut m = [0u16; 8];
    let mut c = a.chunks_exact(8);
    for x in c.by_ref() {
        for k in 0..8 {
            m[k] = m[k].max(x[k].unsigned_abs());
        }
    }
    let mut best = m.iter().copied().fold(0u16, u16::max);
    for &x in c.remainder() {
        best = best.max(x.unsigned_abs());
    }
    best
}

/// Level of an interleaved i16 block in dBFS (RMS over all channels).
/// Digital silence returns `-120.0`.
#[must_use]
pub fn rms_dbfs_i16(samples: &[u8]) -> f32 {
    let (acc, n) = sum_sq_i16_le(samples);
    if n == 0 || acc == 0 {
        return -120.0;
    }
    // Exact in f64 up to 2^53; the final division and log are the only rounding.
    let mean = acc as f64 / n as f64;
    let rms = libm::sqrt(mean) / 32768.0;
    (20.0 * libm::log10(rms)) as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dbfs_of_full_scale_square_is_zero() {
        let mut buf = [0u8; 64];
        for (i, s) in buf.chunks_exact_mut(2).enumerate() {
            let v: i16 = if i % 2 == 0 { 32767 } else { -32767 };
            s.copy_from_slice(&v.to_le_bytes());
        }
        let db = rms_dbfs_i16(&buf);
        assert!(db.abs() < 0.001, "{db}");
        assert_eq!(rms_dbfs_i16(&[0u8; 8]), -120.0);
        assert_eq!(rms_dbfs_i16(&[]), -120.0);
        // half scale is −6.02 dB
        let mut half = [0u8; 8];
        for s in half.chunks_exact_mut(2) {
            s.copy_from_slice(&16384i16.to_le_bytes());
        }
        assert!((rms_dbfs_i16(&half) + 6.0206).abs() < 0.001);
    }

    #[test]
    fn reductions_are_exact_at_the_extremes() {
        let ext = [i16::MIN, i16::MAX, i16::MIN, 0];
        assert_eq!(sum_sq_i16(&ext), 2 * 32768 * 32768 + 32767 * 32767);
        assert_eq!(dot_i16(&ext, &ext), sum_sq_i16(&ext));
        assert_eq!(dot_i16(&ext, &ext[..2]), 32768 * 32768 + 32767 * 32767);
        assert_eq!(dot_i16(&[3, -4], &[-2, 5]), -26);
        assert_eq!(peak_abs_i16(&ext), 32768);
        assert_eq!(peak_abs_i16(&[]), 0);
        let mut bytes = [0u8; 9];
        bytes[..8].copy_from_slice(&[0x00, 0x80, 0xFF, 0x7F, 0x00, 0x80, 0, 0]);
        assert_eq!(sum_sq_i16_le(&bytes), (sum_sq_i16(&ext), 4));
    }
}
