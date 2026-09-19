//! A TOTAL proof that `isqrt_restoring` is `isqrt` for every one of the 2^32
//! `u32` inputs, and that both are `floor(sqrt(v))`.
//!
//! `#[ignore]`d because it is minutes in release and pointless in debug:
//!
//!   cargo test -p rusty_esp_dsp --release --test isqrt_exhaustive -- --ignored --nocapture
//!
//! The in-crate sweep covers 0..200_000 plus the power-of-two boundaries,
//! which is the right everyday gate. This one exists because adopting a
//! DIFFERENT ALGORITHM is not a range claim about the inputs in use -- it is
//! a claim about every input there is, and only every input can settle it.

use rusty_esp_dsp::int::{isqrt, isqrt_restoring};

#[test]
#[ignore = "sweeps all 2^32 u32 values; run with --release --ignored"]
fn restoring_equals_newton_everywhere() {
    let mut v: u32 = 0;
    loop {
        let (a, b) = (isqrt(v), isqrt_restoring(v));
        assert_eq!(a, b, "v={v}: newton={a} restoring={b}");
        if v == u32::MAX {
            break;
        }
        v += 1;
    }
    println!("isqrt: 2^32 inputs, both algorithms identical");
}

/// And that the shared answer really is the floor of the square root, checked
/// at the only places it can change: every perfect square and its neighbours.
#[test]
fn both_are_floor_sqrt_at_every_boundary() {
    for r in 0u32..=65_535 {
        let sq = r * r;
        assert_eq!(isqrt_restoring(sq), r, "r^2 = {sq}");
        assert_eq!(isqrt(sq), r, "r^2 = {sq}");
        if sq > 0 {
            assert_eq!(isqrt_restoring(sq - 1), r - 1, "r^2 - 1 = {}", sq - 1);
            assert_eq!(isqrt(sq - 1), r - 1);
        }
        if r < 65_535 {
            assert_eq!(isqrt_restoring(sq + r), r, "between squares");
            assert_eq!(isqrt(sq + r), r);
        }
    }
    assert_eq!(isqrt_restoring(u32::MAX), 65_535);
    assert_eq!(isqrt(u32::MAX), 65_535);
}
