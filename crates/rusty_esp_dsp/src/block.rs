//! H.264 block costs, scalar: SAD over 4×4 / 8×8 / 16×16 luma blocks, the
//! 4×4 Hadamard and the SATD built on it.
//!
//! [`hadamard_4x4`] and [`satd_4x4_sum`] match `rusty_h264-common`'s
//! transform byte for byte (its Hadamard is the same butterfly; its SATD
//! sums four blocks at a time in SIMD lanes, integer math, so the totals are
//! identical) — `tests/h264_oracle.rs` pulls that crate and demands it over
//! a generated corpus. The S3 / P4 twins of these (D3) go upstream through
//! `rusty_h264`'s accel seam; this module is where their oracle lives.
//!
//! Planes are byte slices with a row stride; a block is `W` bytes of each of
//! `H` rows starting at the slice's first byte. Sizes are checked once, on
//! entry.

use rusty_esp_core::error::Result;

use crate::expect_len;

#[inline]
fn sad<const W: usize, const H: usize>(a: &[u8], sa: usize, b: &[u8], sb: usize) -> Result<u32> {
    expect_len(a, (H - 1) * sa + W)?;
    expect_len(b, (H - 1) * sb + W)?;
    let mut acc = 0u32;
    for r in 0..H {
        let ra = &a[r * sa..r * sa + W];
        let rb = &b[r * sb..r * sb + W];
        for (x, y) in ra.iter().zip(rb) {
            acc += u32::from(x.abs_diff(*y));
        }
    }
    Ok(acc)
}

/// Sum of absolute differences over a 4×4 block (at most 4 080).
pub fn sad_4x4(a: &[u8], sa: usize, b: &[u8], sb: usize) -> Result<u32> {
    sad::<4, 4>(a, sa, b, sb)
}

/// Sum of absolute differences over an 8×8 block (at most 16 320).
pub fn sad_8x8(a: &[u8], sa: usize, b: &[u8], sb: usize) -> Result<u32> {
    sad::<8, 8>(a, sa, b, sb)
}

/// Sum of absolute differences over a 16×16 block (at most 65 280).
pub fn sad_16x16(a: &[u8], sa: usize, b: &[u8], sb: usize) -> Result<u32> {
    sad::<16, 16>(a, sa, b, sb)
}

/// The 4×4 residual `a − b`, row-major, as the transform wants it.
pub fn residual_4x4(a: &[u8], sa: usize, b: &[u8], sb: usize) -> Result<[i32; 16]> {
    expect_len(a, 3 * sa + 4)?;
    expect_len(b, 3 * sb + 4)?;
    let mut out = [0i32; 16];
    for r in 0..4 {
        for c in 0..4 {
            out[r * 4 + c] = i32::from(a[r * sa + c]) - i32::from(b[r * sb + c]);
        }
    }
    Ok(out)
}

/// In-place 1D 4-point Hadamard (its own inverse up to scale).
#[inline]
const fn hadamard_1d(a: i32, b: i32, c: i32, d: i32) -> (i32, i32, i32, i32) {
    (a + b + c + d, a + b - c - d, a - b - c + d, a - b + c - d)
}

/// 4×4 Hadamard transform (rows then columns). Symmetric, so the same
/// routine serves forward and inverse; the I_16x16 luma DC transform and
/// the SATD cost both use it.
#[must_use]
pub fn hadamard_4x4(block: &[i32; 16]) -> [i32; 16] {
    let mut m = *block;
    for r in 0..4 {
        let (a, b, c, d) = hadamard_1d(m[r * 4], m[r * 4 + 1], m[r * 4 + 2], m[r * 4 + 3]);
        m[r * 4] = a;
        m[r * 4 + 1] = b;
        m[r * 4 + 2] = c;
        m[r * 4 + 3] = d;
    }
    for c in 0..4 {
        let (a, b, cc, d) = hadamard_1d(m[c], m[4 + c], m[8 + c], m[12 + c]);
        m[c] = a;
        m[4 + c] = b;
        m[8 + c] = cc;
        m[12 + c] = d;
    }
    m
}

/// SATD of one 4×4 residual block: `Σ|hadamard_4x4(block)|`.
#[must_use]
pub fn satd_4x4(block: &[i32; 16]) -> i64 {
    hadamard_4x4(block)
        .iter()
        .map(|&v| i64::from(v.unsigned_abs()))
        .sum()
}

/// SATD over a slice of 4×4 residual blocks — the motion-estimation and
/// mode-decision cost kernel.
#[must_use]
pub fn satd_4x4_sum(blocks: &[[i32; 16]]) -> i64 {
    blocks.iter().map(satd_4x4).sum()
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusty_esp_core::error::Error;

    #[test]
    fn sad_counts_absolute_differences_with_strides() {
        // a: rows of 0..20 with stride 20; b: all 5s with stride 16
        let mut a = [0u8; 20 * 16];
        for (i, v) in a.iter_mut().enumerate() {
            *v = (i % 20) as u8;
        }
        let b = [5u8; 16 * 16];
        // per row: Σ|x − 5| for x in 0..16 = (5+4+3+2+1+0) + (1+..+10) = 15 + 55 = 70
        assert_eq!(sad_16x16(&a, 20, &b, 16).unwrap(), 70 * 16);
        // 8x8: Σ|x − 5| for x in 0..8 = (5+4+3+2+1+0) + (1+2) = 18, times 8 rows
        assert_eq!(sad_8x8(&a, 20, &b, 16).unwrap(), 18 * 8);
        assert_eq!(sad_4x4(&a, 20, &b, 16).unwrap(), (5 + 4 + 3 + 2) * 4);
        assert_eq!(sad_4x4(&a, 20, &a, 20).unwrap(), 0);
        // the slice must reach the last row's last byte
        assert_eq!(
            sad_16x16(&a[..300], 20, &b, 16),
            Err(Error::BufferTooSmall {
                needed: 15 * 20 + 16
            })
        );
    }

    #[test]
    fn hadamard_is_its_own_inverse_up_to_sixteen_and_satd_is_the_abs_sum() {
        let block: [i32; 16] = core::array::from_fn(|i| (i as i32 * 7) % 11 - 5);
        let h = hadamard_4x4(&block);
        let back = hadamard_4x4(&h);
        for (x, y) in block.iter().zip(back) {
            assert_eq!(y, x * 16);
        }
        assert_eq!(satd_4x4(&[0; 16]), 0);
        // a DC-only block: every coefficient but the first is zero
        let dc = [3i32; 16];
        assert_eq!(hadamard_4x4(&dc)[0], 48);
        assert_eq!(satd_4x4(&dc), 48);
        assert_eq!(satd_4x4_sum(&[dc, block, dc]), 96 + satd_4x4(&block));
    }

    #[test]
    fn residual_is_a_minus_b() {
        let a = [
            10u8, 20, 30, 40, 11, 21, 31, 41, 12, 22, 32, 42, 13, 23, 33, 43,
        ];
        let b = [5u8; 16];
        let r = residual_4x4(&a, 4, &b, 4).unwrap();
        assert_eq!(r[0], 5);
        assert_eq!(r[15], 38);
        assert_eq!(
            residual_4x4(&a[..15], 4, &b, 4),
            Err(Error::BufferTooSmall { needed: 16 })
        );
    }
}
