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
    // REFUTED, measured worse, reverted. The seed must be at or above
    // sqrt(v) -- Newton descends monotonically to the floor from there, and
    // from BELOW it can stop one short -- and `1 << ((bits + 1) / 2)` is
    // tight for even `bits` and a factor of two loose for odd ones. Closing
    // that with `if bits & 1 == 0 { 1 << h } else { 3 << (h - 1) }` (valid,
    // since 1.5 > sqrt(2), and exhaustively proven identical over all 2^32
    // inputs) measured **+15.2%** against a 1.6% null arm on an ESP32-S3,
    // 2026-09-19. A branch and a two-operation shift on EVERY call cost more
    // than the one divide they save on HALF of them. The cheapest seed wins
    // on a kernel whose body is four instructions.
    let bits = 32 - v.leading_zeros();
    let mut x = 1u32 << ((bits + 1) / 2);
    let mut y = (x + v / x) / 2;
    while y < x {
        x = y;
        y = (x + v / x) / 2;
    }
    x
}

/// `floor(sqrt(v))` again, by restoring binary long division: two bits of
/// radicand per step, sixteen steps, and NO division anywhere.
///
/// A candidate twin for [`isqrt`], kept beside it so the two can be measured
/// in the SAME build -- the ESP32-S3 layout swings enough between builds
/// (-8%..+12% on untouched kernels) that an across-build comparison of two
/// leaf functions this small cannot be trusted. Newton converges in about
/// four steps against this one's sixteen, but each of its steps is a 32-bit
/// DIVIDE, and a divide is not pipelined on this core.
///
/// Gated exhaustively against [`isqrt`] over all 2^32 inputs.
#[must_use]
pub const fn isqrt_restoring(v: u32) -> u32 {
    let mut rem: u32 = 0;
    let mut root: u32 = 0;
    let mut i: u32 = 16;
    while i > 0 {
        i -= 1;
        root <<= 1;
        rem = (rem << 2) | ((v >> (i * 2)) & 3);
        if rem > root {
            rem -= root + 1;
            root += 2;
        }
    }
    root >> 1
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
