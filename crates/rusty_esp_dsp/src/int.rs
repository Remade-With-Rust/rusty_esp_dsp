//! Integer helpers shared by the fixed-point paths.
//!
//! `isqrt` moved verbatim from `rusty_esp_signal-core::radar::csi` (D0,
//! 2026-09-02), where it turns a CSI subcarrier's `i² + q²` into an
//! amplitude without a float.

/// Integer square root, `floor(sqrt(v))`.
#[must_use]
pub const fn isqrt(v: u32) -> u32 {
    if v < 2 {
        return v;
    }
    // Seed with a power of two just above sqrt(v) instead of with v itself.
    // Newton's iteration descends monotonically to floor(sqrt(v)) from ANY
    // start at or above sqrt(v), so the answer is unchanged -- but starting
    // at v spent roughly log2(v) iterations, each one a 32-bit DIVIDE, just
    // halving its way down to the right magnitude. `bits` significant bits
    // means v < 2^bits, so x0 = 2^ceil(bits/2) has x0^2 >= 2^bits > v.
    let bits = 32 - v.leading_zeros();
    let mut x = 1u32 << ((bits + 1) / 2);
    let mut y = (x + v / x) / 2;
    while y < x {
        x = y;
        y = (x + v / x) / 2;
    }
    x
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn isqrt_is_floor_sqrt() {
        for v in 0..70_000u32 {
            let r = isqrt(v);
            assert!(r * r <= v && (r + 1) * (r + 1) > v, "v={v} r={r}");
        }
        assert_eq!(isqrt(u32::MAX), 65_535);

        // The seed changed (2026-09-19), so sweep hard: a seed that is ever
        // BELOW sqrt(v) makes Newton converge to the wrong value, and it
        // would show up first at magnitude boundaries.
        for v in 0u32..200_000 {
            assert_eq!(isqrt(v), (f64::from(v)).sqrt().floor() as u32, "v={v}");
        }
        for b in 0..32u32 {
            for d in 0..3u32 {
                for v in [(1u32 << b).saturating_add(d), (1u32 << b).saturating_sub(d)] {
                    assert_eq!(isqrt(v), (f64::from(v)).sqrt().floor() as u32, "v={v}");
                }
            }
        }
        // the squares themselves, and one below each
        for r in [256u32, 1_000, 4_096, 65_535] {
            assert_eq!(isqrt(r * r), r);
            assert_eq!(isqrt(r * r - 1), r - 1);
        }
    }
}
