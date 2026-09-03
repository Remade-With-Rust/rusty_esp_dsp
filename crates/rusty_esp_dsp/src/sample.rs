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
#[must_use]
pub fn dot_i16(a: &[i16], b: &[i16]) -> i64 {
    a.iter()
        .zip(b)
        .map(|(&x, &y)| i64::from(x) * i64::from(y))
        .sum()
}

/// `Σ a[i]²`, exact in `i64`.
#[must_use]
pub fn sum_sq_i16(a: &[i16]) -> i64 {
    a.iter().map(|&x| i64::from(x) * i64::from(x)).sum()
}

/// Sum of squares and sample count over little-endian `i16` bytes; a
/// trailing odd byte is ignored.
#[must_use]
pub fn sum_sq_i16_le(samples: &[u8]) -> (i64, usize) {
    let mut acc: i64 = 0;
    let mut n: usize = 0;
    for s in samples.chunks_exact(2) {
        let v = i64::from(i16::from_le_bytes([s[0], s[1]]));
        acc += v * v;
        n += 1;
    }
    (acc, n)
}

/// The largest magnitude in the block (`32 768` for `i16::MIN`).
#[must_use]
pub fn peak_abs_i16(a: &[i16]) -> u16 {
    a.iter().map(|x| x.unsigned_abs()).max().unwrap_or(0)
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
