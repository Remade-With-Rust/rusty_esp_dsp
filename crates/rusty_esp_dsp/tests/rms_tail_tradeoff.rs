//! What the `rms_dbfs_i16` float tail would cost in ACCURACY if it stopped
//! being bit-exact.
//!
//! `tests/moved.rs` pins this function with `assert_eq!` against the version
//! that moved out of `rusty_esp_audio-core`, which is a deliberate exactness
//! contract. On an ESP32-S3 that tail is ~48% of the function, because the
//! chip's FPU is SINGLE precision and every `f64` operation below is a
//! software routine:
//!
//!     let mean = acc as f64 / n as f64;
//!     let rms  = libm::sqrt(mean) / 32768.0;
//!     (20.0 * libm::log10(rms)) as f32
//!
//! This test does not change anything. It measures the error of the two
//! cheaper forms over a wide corpus so the trade can be decided with a number
//! rather than a feeling. Run it with `--ignored` for the full sweep.

use rusty_esp_dsp::sample::{rms_dbfs_i16, sum_sq_i16_le};

/// The shipped tail, for reference.
fn tail_f64(acc: i64, n: usize) -> f32 {
    let mean = acc as f64 / n as f64;
    let rms = libm::sqrt(mean) / 32768.0;
    (20.0 * libm::log10(rms)) as f32
}

/// Same arithmetic, single precision throughout.
fn tail_f32(acc: i64, n: usize) -> f32 {
    let mean = acc as f32 / n as f32;
    let rms = libm::sqrtf(mean) / 32768.0;
    20.0 * libm::log10f(rms)
}

/// `20*log10(sqrt(m)/k)` is `10*log10(m) - 20*log10(k)`, which drops the
/// square root entirely and keeps f64.
fn tail_f64_nosqrt(acc: i64, n: usize) -> f32 {
    const K: f64 = 90.308_998_699_194_36; // 20*log10(32768)
    let mean = acc as f64 / n as f64;
    (10.0 * libm::log10(mean) - K) as f32
}

fn corpus() -> Vec<(i64, usize)> {
    let mut v = Vec::new();
    // every block length a real path uses, against every amplitude decade
    for &n in &[1usize, 2, 16, 160, 256, 320, 512, 1024, 4096, 16000] {
        for shift in 0..16u32 {
            let amp = 1i64 << shift; // 1 .. 32768
            v.push((amp * amp * n as i64, n));
        }
        // and the extremes: full-scale, and a single loud sample in silence
        v.push((32768i64 * 32768 * n as i64, n));
        v.push((1, n));
        v.push((32768i64 * 32768, n));
    }
    v
}

#[test]
fn the_two_cheaper_tails_are_within_a_millionth_of_a_decibel() {
    let (mut worst32, mut worst_ns) = (0.0f64, 0.0f64);
    let (mut at32, mut at_ns) = ((0i64, 0usize), (0i64, 0usize));
    for (acc, n) in corpus() {
        let base = f64::from(tail_f64(acc, n));
        let e32 = (f64::from(tail_f32(acc, n)) - base).abs();
        let ens = (f64::from(tail_f64_nosqrt(acc, n)) - base).abs();
        if e32 > worst32 {
            worst32 = e32;
            at32 = (acc, n);
        }
        if ens > worst_ns {
            worst_ns = ens;
            at_ns = (acc, n);
        }
    }
    println!("TRADEOFF f32 max_err={worst32:e} dB at acc={} n={}", at32.0, at32.1);
    println!("TRADEOFF nosqrt max_err={worst_ns:e} dB at acc={} n={}", at_ns.0, at_ns.1);

    // A VAD threshold is set in whole decibels and a log line prints one
    // decimal. Both forms must be orders of magnitude inside that.
    assert!(worst32 < 1e-3, "f32 tail error {worst32:e} dB is too large");
    assert!(worst_ns < 1e-3, "no-sqrt tail error {worst_ns:e} dB is too large");
}

/// The published behaviours must survive either form.
#[test]
fn the_documented_levels_hold_for_every_form() {
    let mut full = [0u8; 640];
    for (i, s) in full.chunks_exact_mut(2).enumerate() {
        let v: i16 = if i % 2 == 0 { 32767 } else { -32767 };
        s.copy_from_slice(&v.to_le_bytes());
    }
    let (acc, n) = sum_sq_i16_le(&full);
    for (name, db) in [
        ("shipped", rms_dbfs_i16(&full)),
        ("f32", tail_f32(acc, n)),
        ("nosqrt", tail_f64_nosqrt(acc, n)),
    ] {
        assert!(db.abs() < 0.001, "{name}: full scale square is {db} dBFS");
    }

    let mut half = [0u8; 640];
    for s in half.chunks_exact_mut(2) {
        s.copy_from_slice(&16384i16.to_le_bytes());
    }
    let (acc, n) = sum_sq_i16_le(&half);
    for (name, db) in [
        ("shipped", rms_dbfs_i16(&half)),
        ("f32", tail_f32(acc, n)),
        ("nosqrt", tail_f64_nosqrt(acc, n)),
    ] {
        assert!((db + 6.0206).abs() < 0.001, "{name}: half scale is {db} dBFS");
    }
}

/// ★ REFUTED, and worth keeping for how it was refuted.
///
/// On the structured corpus above, `tail_f64_nosqrt` reported max error
/// **exactly 0** -- bit-identical at all 190 points -- which would have made
/// it shippable under `moved.rs`'s `assert_eq!`. It is not: over 3.46 MILLION
/// points, **32 037 of them (0.9%) differ**, the first at `acc=1073741823,
/// n=1` where the two forms give -4.0446824e-9 and -4.0446793e-9 dB.
///
/// A corpus can only ever fail to refute a claim about ROUNDING. 190 points
/// found nothing; 3.5 million found one in a hundred and ten. If a cheaper
/// float form is ever proposed here again, sweep before believing it.
#[test]
#[ignore = "millions of points; run with --release --ignored"]
fn neither_cheaper_tail_is_bit_identical_but_both_are_far_inside_a_millidecibel() {
    let mut checked: u64 = 0;
    let (mut d_ns, mut d_32) = (0u64, 0u64);
    let (mut w_ns, mut w_32) = (0.0f64, 0.0f64);
    let mut note = |acc: i64, n: usize| {
        if n == 0 || acc <= 0 {
            return None;
        }
        let a = tail_f64(acc, n);
        Some((a, tail_f64_nosqrt(acc, n), tail_f32(acc, n)))
    };
    let mut feed = |acc: i64, n: usize, checked: &mut u64, d_ns: &mut u64, d_32: &mut u64,
                    w_ns: &mut f64, w_32: &mut f64| {
        if let Some((a, ns, f3)) = note(acc, n) {
            *checked += 1;
            if a.to_bits() != ns.to_bits() {
                *d_ns += 1;
            }
            if a.to_bits() != f3.to_bits() {
                *d_32 += 1;
            }
            let base = f64::from(a);
            *w_ns = w_ns.max((f64::from(ns) - base).abs());
            *w_32 = w_32.max((f64::from(f3) - base).abs());
        }
    };

    for &n in &[1usize, 2, 4, 16, 64, 160, 256, 320, 512, 1024, 4096, 8000, 16000, 48000] {
        let max = (n as i64).saturating_mul(32768).saturating_mul(32768);
        let mut acc = 1i64;
        while acc < max {
            feed(acc, n, &mut checked, &mut d_ns, &mut d_32, &mut w_ns, &mut w_32);
            feed(max - acc, n, &mut checked, &mut d_ns, &mut d_32, &mut w_ns, &mut w_32);
            acc += (acc / 512).max(1);
        }
        feed(max, n, &mut checked, &mut d_ns, &mut d_32, &mut w_ns, &mut w_32);
    }
    let mut lcg: u64 = 0x2545_F491_4F6C_DD1D;
    for _ in 0..3_000_000u32 {
        lcg = lcg.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
        let n = ((lcg >> 40) as usize % 48_000) + 1;
        let max = (n as i64).saturating_mul(32768).saturating_mul(32768);
        let acc = ((lcg >> 8) as i64).rem_euclid(max) + 1;
        feed(acc, n, &mut checked, &mut d_ns, &mut d_32, &mut w_ns, &mut w_32);
    }

    println!("TAILSWEEP checked={checked} nosqrt_differ={d_ns} nosqrt_max={w_ns:e} f32_differ={d_32} f32_max={w_32:e}");
    assert!(w_ns < 1e-3, "no-sqrt error {w_ns:e} dB");
    assert!(w_32 < 1e-3, "f32 error {w_32:e} dB");
}
