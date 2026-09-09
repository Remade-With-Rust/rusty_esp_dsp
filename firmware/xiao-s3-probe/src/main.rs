//! The I5 ceiling probe, on the chip it is meant to price.
//!
//! The D1 share table was measured on a laptop, and its own ledger says a
//! host CPU says nothing about an S3. This firmware runs the same scalar
//! kernels on an ESP32-S3 and reports, per kernel, the work done and the
//! time it took — work first, because on a chip the box is always busy and a
//! deterministic count is the primary evidence while the clock is only
//! confirmation.
//!
//! It also reports what the chip says about its own memory, rather than what
//! the ELF says, and dumps a megabyte from the hardware random number
//! generator so an outside statistic can judge it.
//!
//! Every line is prefixed so a monitor can parse it without guessing:
//! `MEM`, `KERNEL`, `RNG`. Nothing here is a speed claim about anything but
//! this part at this clock.

#![no_std]
#![no_main]

extern crate alloc;

use alloc::vec;
use esp_backtrace as _;
use esp_hal::time::Instant;
use esp_println::println;
use rusty_esp_dsp::pixel;
use rusty_esp_dsp::probe::Work;

esp_bootloader_esp_idf::esp_app_desc!();

#[cfg(all(feature = "alloc-esp", feature = "alloc-rusty"))]
compile_error!("pick one allocator arm: alloc-esp or alloc-rusty, not both");
#[cfg(not(any(feature = "alloc-esp", feature = "alloc-rusty")))]
compile_error!("pick an allocator arm: alloc-esp (default) or alloc-rusty");

/// The heap both arms are given. Equal budgets, so neither is advantaged by
/// having more memory to walk.
const HEAP_BYTES: usize = 200_704;

/// 200,704 is not a round number, it is `good_region_size(220 * 1024)`.
///
/// A 220 KiB region is carved into whole 64 KiB segments plus one 4 KiB page,
/// so three segments fit and **24,576 bytes are stranded** -- measured on this
/// board on 2026-09-08, when it was three times the allocator's entire code
/// cost. rusty_alloc 2.0.3 added the arithmetic; this asserts the constant
/// against it rather than trusting a comment, and both arms take the same
/// number so the comparison stays like for like.
#[cfg(feature = "alloc-rusty")]
const _: () = assert!(
    HEAP_BYTES == rusty_esp_alloc::good_region_size(220 * 1024),
    "the heap is no longer the largest zero-waste region under 220 KiB"
);

/// Which allocator this build carries, printed so a log says what ran rather
/// than what a manifest said.
const ALLOCATOR: &str = if cfg!(feature = "alloc-rusty") {
    "rusty_alloc"
} else {
    "esp-alloc"
};

#[cfg(feature = "alloc-rusty")]
#[global_allocator]
static ALLOC: rusty_esp_alloc::Alloc = rusty_esp_alloc::Alloc;

/// The region rusty_alloc serves from. A chip has no operating system to ask
/// for memory, only what the linker reserved.
#[cfg(feature = "alloc-rusty")]
static HEAP: rusty_esp_alloc::Region<HEAP_BYTES> = rusty_esp_alloc::Region::new();

/// The frame every pixel kernel is measured over. Smaller than the host
/// table's QVGA so the buffers fit internal RAM without PSRAM bring-up; the
/// comparable figure is per unit, not per frame.
const W: u32 = 160;
const H: u32 = 120;
const PX: usize = (W * H) as usize;

/// Each kernel runs until it has spent at least this long, so a fast one is
/// not measured against the timer's own resolution.
const BUDGET_US: u64 = 100_000;

/// Bytes asked of the random number generator.
const RNG_BYTES: usize = 1024 * 1024;
/// Bytes per emitted line.
const RNG_LINE: usize = 64;

/// Run `f` until the budget is spent; report units done and microseconds.
fn measure(name: &str, unit: &str, units_per_rep: u64, mut f: impl FnMut() -> Work) -> Work {
    // one untimed pass: the first touch of a buffer pays for cache misses the
    // steady state does not
    let mut work = f();
    let start = Instant::now();
    let mut reps: u64 = 0;
    let mut elapsed;
    loop {
        work.add(f());
        reps += 1;
        elapsed = start.elapsed().as_micros();
        if elapsed >= BUDGET_US {
            break;
        }
    }
    let units = reps * units_per_rep;
    // integer arithmetic only: picoseconds per unit, printed as a decimal
    let ps_per_unit = if units == 0 {
        0
    } else {
        elapsed.saturating_mul(1_000_000) / units
    };
    println!(
        "KERNEL name={name} unit={unit} units={units} reps={reps} us={elapsed} ps_per_unit={ps_per_unit} work_px={} work_blocks={} work_bytes={}",
        work.pixels, work.blocks, work.bytes
    );
    work
}

/// What the allocator says about itself, in the same shape on both arms so a
/// ledger row can put them side by side.
fn report_memory(stage: &str) {
    #[cfg(feature = "alloc-esp")]
    let (used, free) = (esp_alloc::HEAP.used(), esp_alloc::HEAP.free());
    #[cfg(feature = "alloc-rusty")]
    let (used, free) = {
        let (u, f, _total) = rusty_esp_alloc::occupancy();
        (u, f)
    };
    println!(
        "MEM stage={stage} alloc={ALLOCATOR} used={used} free={free} total={} budget={HEAP_BYTES}",
        used + free
    );
    // the allocator's own view, one line, for the ledger's method column
    #[cfg(feature = "alloc-esp")]
    println!("MEM stage={stage} detail={:?}", esp_alloc::HEAP.stats());
}

#[cfg(feature = "rngdump")]
fn dump_random(trng: &mut esp_hal::rng::Trng) {
    println!("RNG begin bytes={RNG_BYTES} source=hardware-trng-adc");
    let mut line = [0u8; RNG_LINE];
    let mut hex = [0u8; RNG_LINE * 2];
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let start = Instant::now();
    let mut sent = 0usize;
    while sent < RNG_BYTES {
        // `random()` gives four bytes at a time
        for chunk in line.chunks_mut(4) {
            let word = trng.random().to_le_bytes();
            chunk.copy_from_slice(&word[..chunk.len()]);
        }
        for (i, b) in line.iter().enumerate() {
            hex[i * 2] = DIGITS[usize::from(b >> 4)];
            hex[i * 2 + 1] = DIGITS[usize::from(b & 0x0f)];
        }
        // SAFETY-free: every byte written above is an ASCII hex digit
        let text = core::str::from_utf8(&hex).unwrap_or("");
        println!("RNGDATA {text}");
        sent += line.len();
    }
    println!("RNG end bytes={sent} us={}", start.elapsed().as_micros());
}

#[esp_hal::main]
fn main() -> ! {
    let peripherals = esp_hal::init(esp_hal::Config::default());
    // Room for one source frame and one destination frame at the largest
    // format (three bytes a pixel), plus the small buffers. Both arms get the
    // same budget.
    #[cfg(feature = "alloc-esp")]
    esp_alloc::heap_allocator!(size: HEAP_BYTES);
    #[cfg(feature = "alloc-rusty")]
    HEAP.give()
        .expect("the heap region is given once, before any allocation");

    println!("== JANUS PROBE xiao-s3 ==");
    println!("PROBE allocator={ALLOCATOR} heap_bytes={HEAP_BYTES}");
    println!("PROBE frame={W}x{H} px={PX} budget_us={BUDGET_US}");
    report_memory("boot");

    let mut yuyv = vec![0u8; PX * 2];
    let mut rgb888 = vec![0u8; PX * 3];
    let mut rgb565 = vec![0u8; PX * 2];
    let mut gray = vec![0u8; PX];
    let mut small = vec![0u8; PX]; // holds either downscale output
                                   // a deterministic pattern: the same bytes the host arm uses, so the work
                                   // counts and the outputs are comparable rather than merely similar
    for (i, b) in yuyv.iter_mut().enumerate() {
        *b = (i as u32).wrapping_mul(2_654_435_761).to_le_bytes()[0];
    }
    for (i, b) in rgb565.iter_mut().enumerate() {
        *b = (i as u32).wrapping_mul(40_503).to_le_bytes()[1];
    }
    for (i, b) in rgb888.iter_mut().enumerate() {
        *b = (i as u32).wrapping_mul(2_246_822_519).to_le_bytes()[2];
    }
    for (i, b) in gray.iter_mut().enumerate() {
        *b = (i as u32).wrapping_mul(97).to_le_bytes()[0];
    }
    report_memory("buffers");

    let px = PX as u64;
    measure("yuyv_to_gray8", "px", px, || {
        let _ = pixel::yuyv_to_gray8(&yuyv, &mut gray);
        Work::pixels(px)
    });
    measure("yuyv_to_rgb565", "px", px, || {
        let _ = pixel::yuyv_to_rgb565(&yuyv, &mut rgb565);
        Work::pixels(px)
    });
    measure("yuyv_to_rgb888", "px", px, || {
        let _ = pixel::yuyv_to_rgb888(&yuyv, &mut rgb888);
        Work::pixels(px)
    });
    measure("rgb565_to_rgb888", "px", px, || {
        let _ = pixel::rgb565_to_rgb888(&rgb565, &mut rgb888);
        Work::pixels(px)
    });
    measure("rgb888_to_rgb565", "px", px, || {
        let _ = pixel::rgb888_to_rgb565(&rgb888, &mut rgb565);
        Work::pixels(px)
    });
    let out_px = px / 4;
    measure("downscale2x_gray8", "px_out", out_px, || {
        let _ = pixel::downscale2x_gray8(&gray, W, H, &mut small);
        Work::pixels(out_px)
    });
    measure("downscale2x_rgb565", "px_out", out_px, || {
        let _ = pixel::downscale2x_rgb565(&rgb565, W, H, &mut small);
        Work::pixels(out_px)
    });

    // one whole frame of 16x16 blocks against a shifted copy of itself
    let blocks_x = (W / 16) as usize;
    let blocks_y = (H / 16) as usize;
    let blocks = (blocks_x * blocks_y) as u64;
    measure("sad_16x16", "block", blocks, || {
        let stride = W as usize;
        let mut sum = 0u32;
        for by in 0..blocks_y {
            for bx in 0..blocks_x {
                let at = by * 16 * stride + bx * 16;
                let shifted = at + 1;
                if let Ok(v) =
                    rusty_esp_dsp::block::sad_16x16(&gray[at..], stride, &gray[shifted..], stride)
                {
                    sum = sum.wrapping_add(v);
                }
            }
        }
        core::hint::black_box(sum);
        Work {
            blocks,
            ..Work::ZERO
        }
    });

    report_memory("after_kernels");

    #[cfg(feature = "rngdump")]
    {
        let _source = esp_hal::rng::TrngSource::new(peripherals.RNG, peripherals.ADC1);
        match esp_hal::rng::Trng::try_new() {
            Ok(mut trng) => dump_random(&mut trng),
            Err(e) => println!("RNG unavailable: {e:?}"),
        }
    }
    #[cfg(not(feature = "rngdump"))]
    let _ = &peripherals;

    report_memory("end");
    println!("== DONE ==");
    loop {
        // nothing left to do; the numbers are on the wire
        esp_hal::delay::Delay::new().delay_millis(1000);
    }
}
