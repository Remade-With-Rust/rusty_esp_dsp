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
    let mut x = v;
    let mut y = x.div_ceil(2);
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
        // the squares themselves, and one below each
        for r in [256u32, 1_000, 4_096, 65_535] {
            assert_eq!(isqrt(r * r), r);
            assert_eq!(isqrt(r * r - 1), r - 1);
        }
    }
}
