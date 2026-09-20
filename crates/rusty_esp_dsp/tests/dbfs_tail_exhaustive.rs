//! The `rms_dbfs_i16` float tail, swept over its ENTIRE input domain.
//!
//! The tail consumes one number — the mean square — so its domain is the set
//! of positive `f32` values a block of `i16` samples can produce, which is
//! `(0, 32768²]`. That is enumerable, so this is not a corpus and it cannot
//! merely fail to refute: it is every input the function will ever see.
//!
//! That distinction is the whole reason this file exists. Ledger R2 records a
//! 190-point corpus reporting a max error of EXACTLY ZERO for a form that
//! 3.46 M points later showed disagreeing on 0.9% of cases. A sampled bound
//! on a rounding claim is not evidence. An exhaustive one is.
//!
//! Run the full sweep with:
//!   cargo test --release --test dbfs_tail_exhaustive -- --ignored --nocapture

/// The shipped f64 tail as it stood before R2, and the reference here.
fn tail_f64(mean: f64) -> f32 {
    let rms = libm::sqrt(mean) / 32768.0;
    (20.0 * libm::log10(rms)) as f32
}

/// Candidate A: the same arithmetic in single precision.
fn tail_f32_sqrt(mean: f32) -> f32 {
    let rms = libm::sqrtf(mean) / 32768.0;
    20.0 * libm::log10f(rms)
}

/// Candidate B, and what actually ships: `20·log10(√m / k)` is
/// `10·log10(m) − 20·log10(k)`, which drops the square root entirely.
///
/// This calls the SHIPPED function rather than restating it. A copy here
/// would prove a bound for the copy — the same mistake as gating a kernel
/// against a paraphrase of itself.
fn tail_f32_nosqrt(mean: f32) -> f32 {
    rusty_esp_dsp::sample::dbfs_from_mean_square(mean)
}

/// The largest mean square an i16 block can produce: 32768².
const MEAN_MAX: f32 = 1_073_741_824.0;

/// The smallest it can produce, with room to spare.
///
/// `mean = acc as f32 / n as f32` where `acc` is an integer sum of squares
/// and the `acc == 0` case returns the silence floor before reaching here.
/// So the smallest non-zero mean is `1 / n`, and this floor of `2⁻³²` allows
/// n up to 4.29e9 samples — about three years of 16 kHz audio in one block.
///
/// Below this lie the subnormals, where `log10f` loses its relative accuracy
/// and both candidates read 3.05e-5 dB. Excluding them is not cherry-picking:
/// reaching one would need n near 1e38.
const MEAN_MIN: f32 = 2.328_306_4e-10;

/// Walk every positive `f32` bit pattern in `[MEAN_MIN, MEAN_MAX]`, `step`
/// apart.
fn sweep(step: u32, label: &str) -> (f64, f64) {
    let hi = MEAN_MAX.to_bits();
    let lo = MEAN_MIN.to_bits();
    let mut worst_a = 0.0f64;
    let mut worst_b = 0.0f64;
    let mut at_a = 0f32;
    let mut at_b = 0f32;
    let mut bits = lo;
    let mut n = 0u64;
    while bits <= hi {
        let m = f32::from_bits(bits);
        if m.is_finite() && m > 0.0 {
            let r = tail_f64(f64::from(m));
            let ea = (f64::from(tail_f32_sqrt(m)) - f64::from(r)).abs();
            let eb = (f64::from(tail_f32_nosqrt(m)) - f64::from(r)).abs();
            if ea > worst_a {
                worst_a = ea;
                at_a = m;
            }
            if eb > worst_b {
                worst_b = eb;
                at_b = m;
            }
            n += 1;
        }
        bits = bits.saturating_add(step);
        if step == 0 {
            break;
        }
    }
    println!("{label}: {n} points over (0, 2^30]");
    println!("  A  sqrt+log10f  max |err| = {worst_a:.3e} dB   at mean={at_a:e}");
    println!("  B  log10f only  max |err| = {worst_b:.3e} dB   at mean={at_b:e}");
    (worst_a, worst_b)
}

#[test]
fn a_strided_sweep_bounds_both_candidates() {
    // ~524k points: fast enough for every `cargo test`, and enough to catch a
    // form that is wrong rather than merely imprecise.
    let (a, b) = sweep(4096, "strided");
    assert!(a < 1.0e-3, "candidate A drifted: {a}");
    assert!(b < 1.0e-3, "candidate B drifted: {b}");
}

#[test]
#[ignore = "exhaustive: ~1.3e9 points, minutes in release"]
fn every_input_the_tail_can_ever_see() {
    let (a, b) = sweep(1, "EXHAUSTIVE");
    // The bound this crate publishes for the shipped form. Tightened to the
    // measured value once the sweep has run; a VAD threshold is whole dB and
    // a log line prints one decimal, so anything here is orders below use.
    assert!(b < 1.0e-4, "candidate B exceeded the published bound: {b}");
    assert!(a < 1.0e-4, "candidate A exceeded the published bound: {a}");
}
