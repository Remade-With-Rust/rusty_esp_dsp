//! The D1 share table: where the time goes, kernel by kernel, in a QVGA raw
//! frame path and in one second of 16 kHz PCM, in-process, best-of-N — plus
//! the null arm (the same kernel timed as arm A and arm B, alternating) that
//! is this run's noise floor.
//!
//! Run it through `bench/share.ps1`, which pins the process to one core at
//! High priority and prints the method line; a bare `cargo run --example
//! share --release` gives a table without a pin and says so.
//!
//! What this is for: the ceiling probe. A twin is only worth writing for a
//! kernel whose share of its path, times the speedup a PIE twin can buy,
//! clears the null floor. The table says which kernels those are; the floor
//! says what "clears" means on this machine, today.
//!
//! What this is not: a speed claim. Every number here is a best-of-N of a
//! single call, on a host CPU, of scalar code. The board rows (D2) are the
//! numbers that matter, and they are measured there.

use std::hint::black_box;
use std::time::Instant;

use rusty_esp_core::pcm::{PcmBlock, PcmFormat, SampleFormat};
use rusty_esp_core::time::Micros;
use rusty_esp_dsp::block::{residual_4x4, sad_16x16, satd_4x4_sum};
use rusty_esp_dsp::pixel::{
    downscale2x_gray8, downscale2x_rgb565, rgb565_to_rgb888, rgb888_to_rgb565, yuyv_to_gray8,
    yuyv_to_rgb565, yuyv_to_rgb888,
};
use rusty_esp_dsp::probe::Work;
use rusty_esp_dsp::sample::pcm::convert;
use rusty_esp_dsp::sample::{dot_i16, rms_dbfs_i16, sum_sq_i16};

const W: usize = 320;
const H: usize = 240;
const PIXELS: usize = W * H;
const SAMPLES: usize = 16_000;

struct Lcg(u64);

impl Lcg {
    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        self.0
    }

    fn bytes(&mut self, n: usize) -> Vec<u8> {
        (0..n).map(|_| (self.next() >> 56) as u8).collect()
    }
}

struct Row {
    name: &'static str,
    work: Work,
    unit: &'static str,
    units: u64,
    best_ns: u128,
}

/// Best of `reps` single calls, in nanoseconds. One call is the whole frame
/// or the whole second, so a call is well above the timer's resolution.
fn best_of<F: FnMut()>(reps: u32, mut f: F) -> u128 {
    let mut best = u128::MAX;
    for _ in 0..reps {
        let t = Instant::now();
        f();
        best = best.min(t.elapsed().as_nanos());
    }
    best
}

fn main() {
    let reps: u32 = std::env::args()
        .nth(1)
        .and_then(|a| a.parse().ok())
        .unwrap_or(31);
    let mut rng = Lcg(0x5EED_5EED);
    let yuyv = rng.bytes(PIXELS * 2);
    let mut gray = vec![0u8; PIXELS];
    let mut rgb565 = vec![0u8; PIXELS * 2];
    let mut rgb888 = vec![0u8; PIXELS * 3];
    let mut rgb565b = vec![0u8; PIXELS * 2];
    let mut half_gray = vec![0u8; PIXELS / 4];
    let mut half_565 = vec![0u8; PIXELS / 2];
    yuyv_to_gray8(&yuyv, &mut gray).unwrap();
    yuyv_to_rgb565(&yuyv, &mut rgb565).unwrap();
    yuyv_to_rgb888(&yuyv, &mut rgb888).unwrap();
    let shifted: Vec<u8> = gray
        .iter()
        .skip(W + 1)
        .copied()
        .chain([0u8; W + 1])
        .collect();

    let mut frame: Vec<Row> = Vec::new();
    let push = |rows: &mut Vec<Row>, name, work, unit, units, best_ns| {
        rows.push(Row {
            name,
            work,
            unit,
            units,
            best_ns,
        });
    };

    // ---- the raw frame path
    push(
        &mut frame,
        "yuyv_to_gray8",
        Work::pixels(PIXELS as u64),
        "px",
        PIXELS as u64,
        best_of(reps, || {
            black_box(yuyv_to_gray8(black_box(&yuyv), &mut gray).unwrap());
        }),
    );
    push(
        &mut frame,
        "yuyv_to_rgb565",
        Work::pixels(PIXELS as u64),
        "px",
        PIXELS as u64,
        best_of(reps, || {
            black_box(yuyv_to_rgb565(black_box(&yuyv), &mut rgb565).unwrap());
        }),
    );
    push(
        &mut frame,
        "yuyv_to_rgb888",
        Work::pixels(PIXELS as u64),
        "px",
        PIXELS as u64,
        best_of(reps, || {
            black_box(yuyv_to_rgb888(black_box(&yuyv), &mut rgb888).unwrap());
        }),
    );
    push(
        &mut frame,
        "rgb565_to_rgb888",
        Work::pixels(PIXELS as u64),
        "px",
        PIXELS as u64,
        best_of(reps, || {
            black_box(rgb565_to_rgb888(black_box(&rgb565), &mut rgb888).unwrap());
        }),
    );
    push(
        &mut frame,
        "rgb888_to_rgb565",
        Work::pixels(PIXELS as u64),
        "px",
        PIXELS as u64,
        best_of(reps, || {
            black_box(rgb888_to_rgb565(black_box(&rgb888), &mut rgb565b).unwrap());
        }),
    );
    push(
        &mut frame,
        "downscale2x_gray8",
        Work::pixels((PIXELS / 4) as u64),
        "px out",
        (PIXELS / 4) as u64,
        best_of(reps, || {
            black_box(
                downscale2x_gray8(black_box(&gray), W as u32, H as u32, &mut half_gray).unwrap(),
            );
        }),
    );
    push(
        &mut frame,
        "downscale2x_rgb565",
        Work::pixels((PIXELS / 4) as u64),
        "px out",
        (PIXELS / 4) as u64,
        best_of(reps, || {
            black_box(
                downscale2x_rgb565(black_box(&rgb565), W as u32, H as u32, &mut half_565).unwrap(),
            );
        }),
    );
    let mb = (W / 16) * (H / 16);
    push(
        &mut frame,
        "sad_16x16 (whole frame vs shifted)",
        Work::blocks(mb as u64),
        "blocks",
        mb as u64,
        best_of(reps, || {
            let mut total = 0u64;
            for by in 0..H / 16 {
                for bx in 0..W / 16 {
                    let o = by * 16 * W + bx * 16;
                    total += u64::from(sad_16x16(&gray[o..], W, &shifted[o..], W).unwrap());
                }
            }
            black_box(total);
        }),
    );
    let blocks4 = (W / 4) * (H / 4);
    let mut residuals: Vec<[i32; 16]> = Vec::with_capacity(blocks4);
    push(
        &mut frame,
        "residual_4x4 + satd_4x4_sum (whole frame)",
        Work::blocks(blocks4 as u64),
        "blocks",
        blocks4 as u64,
        best_of(reps, || {
            residuals.clear();
            for by in 0..H / 4 {
                for bx in 0..W / 4 {
                    let o = by * 4 * W + bx * 4;
                    residuals.push(residual_4x4(&gray[o..], W, &shifted[o..], W).unwrap());
                }
            }
            black_box(satd_4x4_sum(black_box(&residuals)));
        }),
    );

    // ---- one second of 16 kHz mono i16
    let pcm_bytes = rng.bytes(SAMPLES * 2);
    let pcm: Vec<i16> = pcm_bytes
        .chunks_exact(2)
        .map(|s| i16::from_le_bytes([s[0], s[1]]))
        .collect();
    let format = PcmFormat::new(16_000, 1, SampleFormat::I16).unwrap();
    let mut f32_bytes = vec![0u8; SAMPLES * 4];
    let mut back = vec![0u8; SAMPLES * 2];
    let mut second: Vec<Row> = Vec::new();
    push(
        &mut second,
        "rms_dbfs_i16",
        Work::samples(SAMPLES as u64),
        "samples",
        SAMPLES as u64,
        best_of(reps, || {
            black_box(rms_dbfs_i16(black_box(&pcm_bytes)));
        }),
    );
    push(
        &mut second,
        "sum_sq_i16",
        Work::samples(SAMPLES as u64),
        "samples",
        SAMPLES as u64,
        best_of(reps, || {
            black_box(sum_sq_i16(black_box(&pcm)));
        }),
    );
    push(
        &mut second,
        "dot_i16",
        Work::samples(SAMPLES as u64),
        "samples",
        SAMPLES as u64,
        best_of(reps, || {
            black_box(dot_i16(black_box(&pcm), black_box(&pcm)));
        }),
    );
    push(
        &mut second,
        "pcm convert I16 -> F32",
        Work::samples(SAMPLES as u64),
        "samples",
        SAMPLES as u64,
        best_of(reps, || {
            let block = PcmBlock::new(format, Micros::ZERO, black_box(&pcm_bytes)).unwrap();
            black_box(convert(block, SampleFormat::F32, &mut f32_bytes).unwrap());
        }),
    );
    let f32_format = PcmFormat::new(16_000, 1, SampleFormat::F32).unwrap();
    push(
        &mut second,
        "pcm convert F32 -> I16",
        Work::samples(SAMPLES as u64),
        "samples",
        SAMPLES as u64,
        best_of(reps, || {
            let block = PcmBlock::new(f32_format, Micros::ZERO, black_box(&f32_bytes)).unwrap();
            black_box(convert(block, SampleFormat::I16, &mut back).unwrap());
        }),
    );

    // ---- the null arm: the same kernel as arm A and arm B, ABBA, best-of-N
    // each; the spread between two identical arms is what this run cannot
    // resolve. Two kernels: the cheapest and the dearest of the frame path.
    let null = |reps: u32, mut f: Box<dyn FnMut()>| -> (u128, u128) {
        let (mut a, mut b) = (u128::MAX, u128::MAX);
        for i in 0..reps {
            let (first, second) = if i % 2 == 0 {
                (&mut a, &mut b)
            } else {
                (&mut b, &mut a)
            };
            let t = Instant::now();
            f();
            *first = (*first).min(t.elapsed().as_nanos());
            let t = Instant::now();
            f();
            *second = (*second).min(t.elapsed().as_nanos());
        }
        (a, b)
    };
    let mut g2 = vec![0u8; PIXELS];
    let (na, nb) = null(
        reps,
        Box::new(|| {
            black_box(yuyv_to_gray8(black_box(&yuyv), &mut g2).unwrap());
        }),
    );
    let mut r2 = vec![0u8; PIXELS * 3];
    let (ma, mb2) = null(
        reps,
        Box::new(|| {
            black_box(yuyv_to_rgb888(black_box(&yuyv), &mut r2).unwrap());
        }),
    );
    let floor = |a: u128, b: u128| -> u128 { (a.abs_diff(b) * 1000) / a.min(b).max(1) };
    let floor_permille = floor(na, nb).max(floor(ma, mb2));

    // ---- print
    println!(
        "method: in-process best-of-{reps} per kernel, one whole frame / second per call, ABBA null arm; pin and priority are the wrapper's (bench/share.ps1), unpinned if run bare"
    );
    println!();
    for (title, rows) in [
        ("QVGA raw frame path (320x240 YUYV in)", &frame),
        ("one second of 16 kHz mono i16", &second),
    ] {
        let total: u128 = rows.iter().map(|r| r.best_ns).sum();
        println!("### {title}");
        println!();
        println!("| kernel | work | best of {reps} | per unit | share of path |");
        println!("|---|---:|---:|---:|---:|");
        for r in rows {
            let per = r.best_ns as f64 / r.units as f64;
            println!(
                "| `{}` | {} {} | {:.3} ms | {:.1} ns/{} | {:.1} % |",
                r.name,
                r.units,
                r.unit,
                r.best_ns as f64 / 1e6,
                per,
                r.unit.split(' ').next().unwrap_or(r.unit),
                100.0 * r.best_ns as f64 / total as f64
            );
            debug_assert!(r.work.parity(&r.work));
        }
        println!(
            "| **path** | | **{:.3} ms** | | 100 % |",
            total as f64 / 1e6
        );
        println!();
    }
    println!(
        "null arm: yuyv_to_gray8 A {:.1} us vs B {:.1} us; yuyv_to_rgb888 A {:.1} us vs B {:.1} us; **floor {} permille** (the larger spread of best-of-N minima)",
        na as f64 / 1e3,
        nb as f64 / 1e3,
        ma as f64 / 1e3,
        mb2 as f64 / 1e3,
        floor_permille
    );
    println!(
        "work counters: frame path {} px + {} blocks; second {} samples",
        frame.iter().map(|r| r.work.pixels).sum::<u64>(),
        frame.iter().map(|r| r.work.blocks).sum::<u64>(),
        second.iter().map(|r| r.work.samples).sum::<u64>()
    );
}
