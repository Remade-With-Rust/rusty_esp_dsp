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
// `ee.*` reaches Rust only through inline asm, and Xtensa asm is still
// experimental. The `esp` toolchain is nightly, so this is available.
#![cfg_attr(target_arch = "xtensa", feature(asm_experimental_arch))]

extern crate alloc;

use alloc::vec;
use esp_backtrace as _;
use esp_hal::time::Instant;
use esp_println::println;
use rusty_esp_dsp::pixel;
use rusty_esp_dsp::probe::Work;

// ---- PIE semantics prober -----------------------------------------------
//
// The ESP32-S3's 128-bit PIE unit is documented in the TRM, and the TRM is
// not on this machine. It does not need to be: the chip is, and an
// instruction run on known bytes reports its own semantics more exactly than
// prose does. Every lane layout below is READ OFF THE SILICON.
//
// The assembler accepts 30 of the 32 `ee.*` forms tried (no `ee.vabs.*`, so
// absolute value has to be built from max/min or compare-and-select). What
// this arm establishes is (a) that Rust's `asm!` reaches them at all, and
// (b) what each one does to each lane.
#[cfg(target_arch = "xtensa")]
mod pie {
    use esp_println::println;

    /// A 16-byte buffer at the alignment `ee.vld.128.ip` requires.
    #[repr(align(16))]
    #[derive(Clone, Copy)]
    pub struct Q(pub [u8; 16]);

    /// Load `a` into q0 and `b` into q1, run `$insn`, then report ALL THREE
    /// of q0, q1 and q2 -- several PIE instructions write their operands in
    /// place, and which ones do is exactly what is being measured.
    macro_rules! probe {
        ($name:literal, $insn:literal, $a:expr, $b:expr) => {{
            let a = Q($a);
            let b = Q($b);
            let mut o0 = Q([0u8; 16]);
            let mut o1 = Q([0u8; 16]);
            let mut o2 = Q([0u8; 16]);
            // SAFETY: three 16-byte, 16-byte-aligned buffers; the block reads
            // two and writes three, touches no memory beyond them, and uses
            // only q0-q2 which nothing else in this firmware holds live.
            unsafe {
                core::arch::asm!(
                    "ee.vld.128.ip q0, {pa}, 0",
                    "ee.vld.128.ip q1, {pb}, 0",
                    "ee.zero.q q2",
                    $insn,
                    "ee.vst.128.ip q0, {p0}, 0",
                    "ee.vst.128.ip q1, {p1}, 0",
                    "ee.vst.128.ip q2, {p2}, 0",
                    pa = inout(reg) a.0.as_ptr() => _,
                    pb = inout(reg) b.0.as_ptr() => _,
                    p0 = inout(reg) o0.0.as_mut_ptr() => _,
                    p1 = inout(reg) o1.0.as_mut_ptr() => _,
                    p2 = inout(reg) o2.0.as_mut_ptr() => _,
                    options(nostack),
                );
            }
            println!(
                "PIE {:<18} a={:02x?}",
                $name, a.0
            );
            println!("PIE {:<18} b={:02x?}", "", b.0);
            println!("PIE {:<18} q0={:02x?}", "", o0.0);
            println!("PIE {:<18} q1={:02x?}", "", o1.0);
            println!("PIE {:<18} q2={:02x?}", "", o2.0);
        }};
    }

    /// 0x00..0x0f, so every lane is identifiable by its own value.
    const RAMP: [u8; 16] = [
        0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c, 0x0d, 0x0e,
        0x0f,
    ];
    /// 0x10..0x1f, distinguishable from RAMP at a glance.
    const RAMP2: [u8; 16] = [
        0x10, 0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17, 0x18, 0x19, 0x1a, 0x1b, 0x1c, 0x1d, 0x1e,
        0x1f,
    ];
    /// Values that saturate both ways in s8 and are recognisable in s16.
    const SAT: [u8; 16] = [
        0x7f, 0x7f, 0x80, 0x80, 0x01, 0xff, 0x7f, 0x01, 0x80, 0xff, 0x00, 0x00, 0x40, 0x40, 0xc0,
        0xc0,
    ];

    pub fn run() {
        println!("PIE == semantics, read off the silicon ==");
        // 1. Does the mechanism work at all? q0 must come back as `a`.
        probe!("ld/st roundtrip", "", RAMP, RAMP2);
        // 2. The deinterleave a YUYV -> gray8 kernel needs.
        probe!("vunzip.8", "ee.vunzip.8 q0, q1", RAMP, RAMP2);
        probe!("vzip.8", "ee.vzip.8 q0, q1", RAMP, RAMP2);
        probe!("vunzip.16", "ee.vunzip.16 q0, q1", RAMP, RAMP2);
        // 3. Saturating arithmetic: which width, and does it saturate?
        probe!("vadds.s8", "ee.vadds.s8 q2, q0, q1", SAT, SAT);
        probe!("vsubs.s8", "ee.vsubs.s8 q2, q0, q1", RAMP2, RAMP);
        probe!("vadds.s16", "ee.vadds.s16 q2, q0, q1", SAT, SAT);
        // 4. Lane-wise max/min -- what `peak_abs_i16` would be built from.
        probe!("vmax.s8", "ee.vmax.s8 q2, q0, q1", SAT, RAMP);
        probe!("vmax.s16", "ee.vmax.s16 q2, q0, q1", SAT, RAMP);
        probe!("vmin.s8", "ee.vmin.s8 q2, q0, q1", SAT, RAMP);
        // 5. Bitwise, for masking and for building abs without ee.vabs.
        probe!("andq", "ee.andq q2, q0, q1", RAMP, SAT);
        probe!("xorq", "ee.xorq q2, q0, q1", RAMP, SAT);
        // The 40-bit-per-lane accumulator: what a dot product, a sum of
        // squares and a SAD would all be built on. `ee.zero.qacc` clears it,
        // `ee.vmulas.*.qacc` multiply-accumulates into it, and the two
        // `ee.st.qacc_*` forms read it back. How many lanes it holds and how
        // wide each is are exactly what this reports.
        {
            // a = i16 lanes 1..=8, b = all ones. Each accumulator then
            // holds the lane it was fed, which names the mapping outright.
            let a = Q([1, 0, 2, 0, 3, 0, 4, 0, 5, 0, 6, 0, 7, 0, 8, 0]);
            let b = Q([1, 0, 1, 0, 1, 0, 1, 0, 1, 0, 1, 0, 1, 0, 1, 0]);
            let mut lo = Q([0u8; 16]);
            let mut hi = Q([0u8; 16]);
            // SAFETY: two 16-byte aligned inputs read, two written; only
            // q0/q1 and the accumulator are touched.
            unsafe {
                core::arch::asm!(
                    "ee.vld.128.ip q0, {pa}, 0",
                    "ee.vld.128.ip q1, {pb}, 0",
                    "ee.zero.qacc",
                    "ee.vmulas.s16.qacc q0, q1",
                    "ee.st.qacc_l.l.128.ip {plo}, 0",
                    "ee.st.qacc_h.h.32.ip {phi}, 0",
                    pa = inout(reg) a.0.as_ptr() => _,
                    pb = inout(reg) b.0.as_ptr() => _,
                    plo = inout(reg) lo.0.as_mut_ptr() => _,
                    phi = inout(reg) hi.0.as_mut_ptr() => _,
                    options(nostack),
                );
            }
            println!("PIE qacc.s16 a={:02x?}", a.0);
            println!("PIE {:<18} b={:02x?}", "", b.0);
            println!("PIE {:<18} qacc_l={:02x?}", "", lo.0);
            println!("PIE {:<18} qacc_h={:02x?}", "", hi.0);
        }
        // The lane-shift and accumulator-readback forms. `ee.vmulas.*.qacc`
        // reaches only the first four lanes, so using all eight needs a way
        // to bring lanes 4-7 down; and an averaging kernel needs the
        // shift-round-clamp that reads the accumulator back.
        probe!("srci.2q 8", "ee.srci.2q q0, q1, 8", RAMP, RAMP2);
        probe!("slci.2q 8", "ee.slci.2q q0, q1, 8", RAMP, RAMP2);
        {
            // qacc <- lanes 0..4 of a, times one; then read back with a
            // shift of 2, which is what `(sum + 2) / 4` would need.
            let a = Q([0x10, 0, 0x20, 0, 0x30, 0, 0x40, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
            let b = Q([1, 0, 1, 0, 1, 0, 1, 0, 1, 0, 1, 0, 1, 0, 1, 0]);
            let mut o = Q([0u8; 16]);
            // SAFETY: two aligned inputs read, one written; q0/q1 and the
            // accumulator only.
            unsafe {
                core::arch::asm!(
                    "ee.vld.128.ip q0, {pa}, 0",
                    "ee.vld.128.ip q1, {pb}, 0",
                    "ee.zero.qacc",
                    "ee.vmulas.s16.qacc q0, q1",
                    "ee.srcmb.s16.qacc q2, {sh}, 0",
                    "ee.vst.128.ip q2, {po}, 0",
                    pa = inout(reg) a.0.as_ptr() => _,
                    pb = inout(reg) b.0.as_ptr() => _,
                    sh = in(reg) 2usize,
                    po = inout(reg) o.0.as_mut_ptr() => _,
                    options(nostack),
                );
            }
            println!("PIE srcmb.s16 in={:02x?}", a.0);
            println!("PIE {:<18} out(shift 2)={:02x?}", "", o.0);
        }
        {
            // ACCX: a separate accumulator from QACC. If `ee.vmulas.s16.accx`
            // sums ALL EIGHT lane products into one scalar it is exactly the
            // dot-product instruction; if it sums four it is not.
            // a = lanes 1..=8, b = all ones, so a full horizontal MAC reads
            // 36 and a four-lane one reads 10.
            let a = Q([1, 0, 2, 0, 3, 0, 4, 0, 5, 0, 6, 0, 7, 0, 8, 0]);
            let b = Q([1, 0, 1, 0, 1, 0, 1, 0, 1, 0, 1, 0, 1, 0, 1, 0]);
            let (mut lo, mut hi): (u32, u32) = (0, 0);
            // SAFETY: two aligned inputs read; q0/q1 and ACCX only.
            unsafe {
                core::arch::asm!(
                    "ee.vld.128.ip q0, {pa}, 0",
                    "ee.vld.128.ip q1, {pb}, 0",
                    "ee.zero.accx",
                    "ee.vmulas.s16.accx q0, q1",
                    "rur.accx_0 {l}",
                    "rur.accx_1 {h}",
                    pa = inout(reg) a.0.as_ptr() => _,
                    pb = inout(reg) b.0.as_ptr() => _,
                    l = out(reg) lo,
                    h = out(reg) hi,
                    options(nostack),
                );
            }
            println!("PIE accx.s16 lanes=1..8 xone -> accx_0={lo} accx_1={hi} (36=all eight, 10=four)");
        }
        println!("PIE == end ==");
    }
}

esp_bootloader_esp_idf::esp_app_desc!();

#[cfg(all(feature = "alloc-esp", feature = "alloc-rusty"))]
compile_error!("pick one allocator arm: alloc-esp or alloc-rusty, not both");
#[cfg(not(any(feature = "alloc-esp", feature = "alloc-rusty")))]
compile_error!("pick an allocator arm: alloc-esp (default) or alloc-rusty");

/// The heap both arms are given. Equal budgets, so neither is advantaged by
/// having more memory to walk.
const HEAP_BYTES: usize = 196_608;

/// Three whole 64 KiB segments, and `good_region_size` says so.
///
/// It was 200,704 until 2.0.4, when the sizing rules lost their `+ FIXED_PAGE`
/// -- the first heap's descriptor moved into the allocator's own statics, so a
/// region is now whole segments and nothing else. That change is why this
/// number moved, and the const assert below is what caught it: the old literal
/// stopped building, which is the point of pinning it to the arithmetic rather
/// than to a comment.
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
    #[cfg(target_arch = "xtensa")]
    pie::run();

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

    // ---- PIE twin vs scalar, same binary ---------------------------------
    // `ee.vld/vst.128` need 16-byte alignment and a `Vec<u8>` does not
    // promise it, so these buffers come from a `Vec<u128>`. Without that the
    // twin would quietly take its scalar fallback and the A/B would read
    // FLAT -- the "prove the fast path ran" trap, in its most inviting form.
    //
    // 2048 pixels, not a whole frame: the frame buffers above already hold
    // 172 800 of the 196 608-byte heap, and a per-pixel figure does not care
    // how many pixels it averaged over. The first attempt asked for a full
    // frame's worth and the allocator aborted the firmware.
    {
        const NPX: usize = 2048;
        let npx = NPX as u64;
        let mut ysrc: alloc::vec::Vec<u128> = alloc::vec![0; NPX * 2 / 16];
        let mut gdst: alloc::vec::Vec<u128> = alloc::vec![0; NPX / 16];
        let mut reference = vec![0u8; NPX];
        // SAFETY: a `Vec<u128>` is 16-byte aligned and its bytes are
        // initialised; viewing them as `u8` is a reinterpretation of POD.
        let (ys, gd) = unsafe {
            (
                core::slice::from_raw_parts_mut(ysrc.as_mut_ptr().cast::<u8>(), NPX * 2),
                core::slice::from_raw_parts_mut(gdst.as_mut_ptr().cast::<u8>(), NPX),
            )
        };
        ys.copy_from_slice(&yuyv[..NPX * 2]);

        // Byte-identity BEFORE either number is read, and the alignment the
        // fast arm needs printed beside it.
        let _ = pixel::yuyv_to_gray8(ys, &mut reference);
        let _ = rusty_esp_dsp_esp::pie_s3::yuyv_to_gray8(ys, gd);
        println!(
            "PIEKERNEL yuyv_to_gray8 identical={} src_align={} dst_align={}",
            reference[..] == gd[..],
            ys.as_ptr() as usize % 16,
            gd.as_ptr() as usize % 16
        );

        measure("yuyv_gray8_scalar", "px", npx, || {
            let _ = pixel::yuyv_to_gray8(ys, gd);
            Work { pixels: npx, ..Work::ZERO }
        });
        measure("yuyv_gray8_pie", "px", npx, || {
            let _ = rusty_esp_dsp_esp::pie_s3::yuyv_to_gray8(ys, gd);
            Work { pixels: npx, ..Work::ZERO }
        });
        // --- peak_abs_i16: PIE vs scalar, same buffers ---
        let mut isrc: alloc::vec::Vec<u128> = alloc::vec![0; NPX / 8];
        // SAFETY: a `Vec<u128>` is 16-byte aligned and initialised; viewing
        // its bytes as `i16` is a reinterpretation of POD.
        let iv = unsafe {
            core::slice::from_raw_parts_mut(isrc.as_mut_ptr().cast::<i16>(), NPX)
        };
        for (k, v) in iv.iter_mut().enumerate() {
            // reaches i16::MIN, which is the one input where a saturating
            // negate would give the wrong magnitude
            *v = match k % 5 {
                0 => i16::MIN,
                1 => i16::MAX,
                2 => 0,
                _ => (((k as i32) * 7919) % 65536 - 32768) as i16,
            };
        }
        let sref = rusty_esp_dsp::sample::peak_abs_i16(iv);
        let spie = rusty_esp_dsp_esp::pie_s3::peak_abs_i16(iv);
        println!(
            "PIEKERNEL peak_abs_i16 identical={} scalar={sref} pie={spie} align={}",
            sref == spie,
            iv.as_ptr() as usize % 16
        );
        measure("peak_abs_scalar", "sample", npx, || {
            core::hint::black_box(rusty_esp_dsp::sample::peak_abs_i16(iv));
            Work { samples: npx, ..Work::ZERO }
        });
        measure("peak_abs_pie", "sample", npx, || {
            core::hint::black_box(rusty_esp_dsp_esp::pie_s3::peak_abs_i16(iv));
            Work { samples: npx, ..Work::ZERO }
        });

        // --- sad_16x16: PIE vs scalar. Stride 16 makes the block
        // contiguous, and every row start is then 16-byte aligned, which is
        // what ee.vld.128 needs.
        let mut sadbuf: alloc::vec::Vec<u128> = alloc::vec![0; 32];
        // SAFETY: a `Vec<u128>` is 16-byte aligned and initialised; the two
        // 256-byte halves are a 16x16 block each at stride 16.
        let sb = unsafe {
            core::slice::from_raw_parts_mut(sadbuf.as_mut_ptr().cast::<u8>(), 512)
        };
        for (k, v) in sb.iter_mut().enumerate() {
            *v = (k.wrapping_mul(97) ^ (k >> 3)) as u8;
        }
        let (sa_blk, sb_blk) = sb.split_at(256);
        let rref = rusty_esp_dsp::block::sad_16x16(sa_blk, 16, sb_blk, 16).unwrap_or(0);
        let rpie = rusty_esp_dsp_esp::pie_s3::sad_16x16(sa_blk, 16, sb_blk, 16).unwrap_or(0);
        println!(
            "PIEKERNEL sad_16x16 identical={} scalar={rref} pie={rpie} align={}",
            rref == rpie,
            sa_blk.as_ptr() as usize % 16
        );
        measure("sad16_scalar", "block", 1, || {
            core::hint::black_box(rusty_esp_dsp::block::sad_16x16(sa_blk, 16, sb_blk, 16).ok());
            Work { blocks: 1, ..Work::ZERO }
        });
        measure("sad16_pie", "block", 1, || {
            core::hint::black_box(rusty_esp_dsp_esp::pie_s3::sad_16x16(sa_blk, 16, sb_blk, 16).ok());
            Work { blocks: 1, ..Work::ZERO }
        });

        let r8ref = rusty_esp_dsp::block::sad_8x8(sa_blk, 16, sb_blk, 16).unwrap_or(0);
        let r8pie = rusty_esp_dsp_esp::pie_s3::sad_8x8(sa_blk, 16, sb_blk, 16).unwrap_or(0);
        println!("PIEKERNEL sad_8x8 identical={} scalar={r8ref} pie={r8pie}", r8ref == r8pie);
        measure("sad8_scalar", "block", 1, || {
            core::hint::black_box(rusty_esp_dsp::block::sad_8x8(sa_blk, 16, sb_blk, 16).ok());
            Work { blocks: 1, ..Work::ZERO }
        });
        measure("sad8_pie", "block", 1, || {
            core::hint::black_box(rusty_esp_dsp_esp::pie_s3::sad_8x8(sa_blk, 16, sb_blk, 16).ok());
            Work { blocks: 1, ..Work::ZERO }
        });

        let qref = rusty_esp_dsp::sample::sum_sq_i16(iv);
        let qpie = rusty_esp_dsp_esp::pie_s3::sum_sq_i16(iv);
        println!("PIEKERNEL sum_sq_i16 identical={} scalar={qref} pie={qpie}", qref == qpie);
        measure("sumsq_scalar", "sample", npx, || {
            core::hint::black_box(rusty_esp_dsp::sample::sum_sq_i16(iv));
            Work { samples: npx, ..Work::ZERO }
        });
        measure("sumsq_pie", "sample", npx, || {
            core::hint::black_box(rusty_esp_dsp_esp::pie_s3::sum_sq_i16(iv));
            Work { samples: npx, ..Work::ZERO }
        });

        // --- third tier: the forms that remove the ALIGNMENT constraint --
        //
        // The assembler also accepts an unaligned-load idiom, 64-bit half
        // loads and stores, a lane insert from a general register, and the
        // accumulator's shift-round-clamp. Each of those would let a kernel
        // reach data the twelve twins so far hand to the scalar arm, so each
        // is read off the part before anything depends on it.
        {
            #[repr(align(16))]
            struct B([u8; 48]);
            let src = B({
                let mut a = [0u8; 48];
                let mut i = 0;
                while i < 48 {
                    a[i] = i as u8;
                    i += 1;
                }
                a
            });
            #[repr(align(16))]
            struct Q([u8; 16]);

            // 1. The unaligned load: `ee.ld.128.usar.ip` is documented to set
            //    SAR_BYTE from the address, and `ee.src.q` to funnel the pair.
            //    Reading from offset 3 must produce bytes 3..=18.
            for off in [0usize, 3, 7] {
                let mut o = Q([0u8; 16]);
                let base = unsafe { src.0.as_ptr().add(off) };
                // SAFETY: `off + 32 <= 48`, so both loads stay inside `src`;
                // the output is a 16-byte aligned buffer. q0-q2 only.
                unsafe {
                    core::arch::asm!(
                        "ee.ld.128.usar.ip q0, {p}, 16",
                        "ee.ld.128.usar.ip q1, {p}, 0",
                        "ee.src.q q2, q0, q1",
                        "ee.vst.128.ip q2, {o}, 0",
                        p = inout(reg) base => _,
                        o = inout(reg) o.0.as_mut_ptr() => _,
                        options(nostack),
                    );
                }
                println!("P3 unaligned off={off} q2={:02x?}", o.0);
            }

            // 2. The 64-bit half load: what happens to the OTHER half?
            {
                let mut lo = Q([0u8; 16]);
                let mut hi = Q([0u8; 16]);
                // SAFETY: two 8-byte reads inside `src`, two aligned writes.
                unsafe {
                    core::arch::asm!(
                        "ee.vcmp.eq.s16 q0, q0, q0",   // fill with all-ones first
                        "ee.vld.l.64.ip q0, {p}, 0",
                        "ee.vst.128.ip q0, {a}, 0",
                        "ee.vcmp.eq.s16 q1, q1, q1",
                        "ee.vld.h.64.ip q1, {p}, 0",
                        "ee.vst.128.ip q1, {b}, 0",
                        p = inout(reg) src.0.as_ptr() => _,
                        a = inout(reg) lo.0.as_mut_ptr() => _,
                        b = inout(reg) hi.0.as_mut_ptr() => _,
                        options(nostack),
                    );
                }
                println!("P3 vld.l.64        q0={:02x?}", lo.0);
                println!("P3 vld.h.64        q1={:02x?}", hi.0);
            }

            // 3. A lane insert from a general register -- the way a 4-byte
            //    block row could be gathered without any alignment at all.
            {
                let mut o = Q([0u8; 16]);
                // SAFETY: one aligned 16-byte write; q0 only.
                unsafe {
                    core::arch::asm!(
                        "ee.zero.q q0",
                        "ee.movi.32.q q0, {v}, 2",
                        "ee.vst.128.ip q0, {o}, 0",
                        v = in(reg) 0xdead_beefu32,
                        o = inout(reg) o.0.as_mut_ptr() => _,
                        options(nostack),
                    );
                }
                println!("P3 movi.32.q l2    q0={:02x?}", o.0);
            }

            // 4. QACC's shift-round-clamp. Does it ROUND or TRUNCATE? The
            //    products below are 6, 10, 14, 18; shifted right by two they
            //    are 1.5, 2.5, 3.5, 4.5, so truncation and round-half-up and
            //    round-half-even all print different answers.
            {
                #[repr(align(16))]
                struct S([i16; 8]);
                let a = S([3, 5, 7, 9, 0, 0, 0, 0]);
                let b = S([2, 2, 2, 2, 0, 0, 0, 0]);
                for sh in [0u32, 2] {
                    let mut o = Q([0u8; 16]);
                    // SAFETY: two aligned 16-byte reads, one aligned write.
                    unsafe {
                        core::arch::asm!(
                            "ee.zero.qacc",
                            "ee.vld.128.ip q0, {pa}, 0",
                            "ee.vld.128.ip q1, {pb}, 0",
                            "ee.vmulas.s16.qacc q0, q1",
                            "ee.srcmb.s16.qacc q2, {sh}, 0",
                            "ee.vst.128.ip q2, {o}, 0",
                            pa = inout(reg) a.0.as_ptr() => _,
                            pb = inout(reg) b.0.as_ptr() => _,
                            sh = in(reg) sh,
                            o = inout(reg) o.0.as_mut_ptr() => _,
                            options(nostack),
                        );
                    }
                    println!("P3 srcmb sh={sh}       q2={:02x?}", o.0);
                }
            }
            println!("P3 == end ==");
        }

        // --- semantics of the SECOND instruction tier, off the silicon ----
        //
        // The assembler accepts 31 more `ee.*` forms than the five patterns
        // the first ten twins used: broadcast loads, lane multiplies, 32-bit
        // lane arithmetic, lane shifts, compares. Every one of them would be
        // guessable and every guess would return a plausible wrong number,
        // so each is run on bytes whose answer is known before any kernel
        // depends on it.
        {
            #[repr(align(16))]
            struct Q([u8; 16]);
            macro_rules! p2 {
                ($name:literal, $pre:literal, $insn:literal, $a:expr, $b:expr) => {{
                    let a = Q($a);
                    let b = Q($b);
                    let mut o0 = Q([0u8; 16]);
                    let mut o1 = Q([0u8; 16]);
                    let mut o2 = Q([0u8; 16]);
                    // SAFETY: five 16-byte aligned buffers, two read and
                    // three written; q0-q2 are not live across this block.
                    unsafe {
                        core::arch::asm!(
                            "ee.vld.128.ip q0, {pa}, 0",
                            "ee.vld.128.ip q1, {pb}, 0",
                            "ee.zero.q q2",
                            $pre,
                            $insn,
                            "ee.vst.128.ip q0, {p0}, 0",
                            "ee.vst.128.ip q1, {p1}, 0",
                            "ee.vst.128.ip q2, {p2}, 0",
                            pa = inout(reg) a.0.as_ptr() => _,
                            pb = inout(reg) b.0.as_ptr() => _,
                            p0 = inout(reg) o0.0.as_mut_ptr() => _,
                            p1 = inout(reg) o1.0.as_mut_ptr() => _,
                            p2 = inout(reg) o2.0.as_mut_ptr() => _,
                            options(nostack),
                        );
                    }
                    println!("P2 {:<16} q0={:02x?}", $name, o0.0);
                    println!("P2 {:<16} q1={:02x?}", "", o1.0);
                    println!("P2 {:<16} q2={:02x?}", "", o2.0);
                }};
            }
            // i16 lanes 0x0100,0x0302,... so a lane's value names its place.
            const R: [u8; 16] = [
                0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15,
            ];
            // i16 lanes: 2, -2, 256, -256, 1, -1, 32767, -32768
            const V: [u8; 16] = [
                0x02, 0x00, 0xfe, 0xff, 0x00, 0x01, 0x00, 0xff,
                0x01, 0x00, 0xff, 0xff, 0xff, 0x7f, 0x00, 0x80,
            ];
            // i16 lanes all 3, so a multiply's scale is unmistakable
            const T: [u8; 16] = [
                3, 0, 3, 0, 3, 0, 3, 0, 3, 0, 3, 0, 3, 0, 3, 0,
            ];
            println!("P2 == second tier ==");
            // Does the shift take its amount from SAR, and is it arithmetic?
            p2!("vsr.32 sar=4", "ssai 4", "ee.vsr.32 q2, q0", V, R);
            p2!("vsl.32 sar=4", "ssai 4", "ee.vsl.32 q2, q0", V, R);
            p2!("vsr.32 sar=0", "ssai 0", "ee.vsr.32 q2, q0", V, R);
            // Lane multiply: what width, and does it saturate?
            p2!("vmul.s16", "", "ee.vmul.s16 q2, q0, q1", V, T);
            // 32-bit lanes -- the widening add `stereo_to_mono` needs
            p2!("vadds.s32", "", "ee.vadds.s32 q2, q0, q1", V, V);
            p2!("vsubs.s32", "", "ee.vsubs.s32 q2, q0, q1", V, R);
            // Compare masks: all-ones or a bitfield?
            p2!("vcmp.lt.s16", "", "ee.vcmp.lt.s16 q2, q0, q1", V, T);
            p2!("vcmp.gt.s16", "", "ee.vcmp.gt.s16 q2, q0, q1", V, T);
            // 32-bit lane interleave, for widening i16 -> i32
            p2!("vzip.32", "", "ee.vzip.32 q0, q1", R, V);
            p2!("vunzip.32", "", "ee.vunzip.32 q0, q1", R, V);
            p2!("notq", "", "ee.notq q2, q0", R, V);
            println!("P2 == end ==");
        }

        // --- how WIDE is ACCX, and what does `rur.accx_1` put above it? --
        //
        // `sum_sq_i16` never had to ask: a sum of squares is non-negative, so
        // recomposing the two halves as an unsigned 64-bit value is exact.
        // `dot_i16` read the low word right and the high word wrong, which
        // says the accumulator is NARROWER than 64 bits and `accx_1` is not
        // 32 bits of sign. Feed it accumulators whose value is KNOWN and read
        // the halves back -- the same method that produced the lane tables.
        {
            #[repr(align(16))]
            struct Q([i16; 8]);
            for (label, av, bv) in [
                ("-1", -1i16, 1i16),
                ("-2", -2, 1),
                ("+1", 1, 1),
                ("-32768*32767", i16::MIN, i16::MAX),
            ] {
                let a = Q([av, 0, 0, 0, 0, 0, 0, 0]);
                let b = Q([bv, 0, 0, 0, 0, 0, 0, 0]);
                let (lo, hi): (u32, u32);
                // SAFETY: two 16-byte aligned 16-byte buffers, read only;
                // q0/q1 and ACCX are not live across this block.
                unsafe {
                    core::arch::asm!(
                        "ee.zero.accx",
                        "ee.vld.128.ip q0, {pa}, 0",
                        "ee.vld.128.ip q1, {pb}, 0",
                        "ee.vmulas.s16.accx q0, q1",
                        "rur.accx_0 {l}",
                        "rur.accx_1 {h}",
                        pa = inout(reg) a.0.as_ptr() => _,
                        pb = inout(reg) b.0.as_ptr() => _,
                        l = out(reg) lo,
                        h = out(reg) hi,
                        options(nostack),
                    );
                }
                println!("ACCXW value={label} accx_0={lo:#010x} accx_1={hi:#010x}");
            }
        }

        // --- dot_i16: the one kernel here whose RESULT can be negative ---
        //
        // `dot_i16(iv, iv)` would be a sum of squares and could never test
        // the sign reconstruction, which is the only thing about this twin
        // that is not already proved by `sum_sq_i16`. So the second operand
        // is built to OPPOSE the first: the running total crosses zero and
        // the final answer is negative.
        let mut jsrc: alloc::vec::Vec<u128> = alloc::vec![0; NPX / 8];
        // SAFETY: as for `isrc` above -- a `Vec<u128>` is 16-byte aligned
        // and initialised, and `i16` is POD.
        let jv = unsafe {
            core::slice::from_raw_parts_mut(jsrc.as_mut_ptr().cast::<i16>(), NPX)
        };
        for (k, v) in jv.iter_mut().enumerate() {
            *v = match k % 4 {
                0 => i16::MAX,        // x i16::MIN -> the most negative product
                1 => i16::MIN,
                _ => (((k as i32) * 2731) % 65536 - 32768) as i16,
            };
        }
        let dref = rusty_esp_dsp::sample::dot_i16(iv, jv);
        let dpie = rusty_esp_dsp_esp::pie_s3::dot_i16(iv, jv);
        println!(
            "PIEKERNEL dot_i16 identical={} negative={} scalar={dref} pie={dpie}",
            dref == dpie,
            dref < 0
        );
        measure("dot_scalar", "sample", npx, || {
            core::hint::black_box(rusty_esp_dsp::sample::dot_i16(iv, jv));
            Work { samples: npx, ..Work::ZERO }
        });
        measure("dot_pie", "sample", npx, || {
            core::hint::black_box(rusty_esp_dsp_esp::pie_s3::dot_i16(iv, jv));
            Work { samples: npx, ..Work::ZERO }
        });

        // --- sum_sq_i16_le: the byte-slice form `rms_dbfs_i16` calls ---
        // SAFETY: `iv` is a live `[i16]`; viewing it as bytes is POD, and
        // the length is exactly twice as many.
        let ibytes = unsafe {
            core::slice::from_raw_parts(iv.as_ptr().cast::<u8>(), NPX * 2)
        };
        let lref = rusty_esp_dsp::sample::sum_sq_i16_le(ibytes);
        let lpie = rusty_esp_dsp_esp::pie_s3::sum_sq_i16_le(ibytes);
        println!("PIEKERNEL sum_sq_i16_le identical={} scalar={lref:?} pie={lpie:?}", lref == lpie);
        measure("sumsq_le_scalar", "sample", npx, || {
            core::hint::black_box(rusty_esp_dsp::sample::sum_sq_i16_le(ibytes));
            Work { samples: npx, ..Work::ZERO }
        });
        measure("sumsq_le_pie", "sample", npx, || {
            core::hint::black_box(rusty_esp_dsp_esp::pie_s3::sum_sq_i16_le(ibytes));
            Work { samples: npx, ..Work::ZERO }
        });

        // --- rms_dbfs_i16: the function the SHIPPING per-block loop calls --
        let rref = rusty_esp_dsp::sample::rms_dbfs_i16(ibytes);
        let rpie = rusty_esp_dsp_esp::pie_s3::rms_dbfs_i16(ibytes);
        println!(
            "PIEKERNEL rms_dbfs_i16 identical={} scalar={rref} pie={rpie}",
            rref == rpie
        );
        measure("rms_scalar", "sample", npx, || {
            core::hint::black_box(rusty_esp_dsp::sample::rms_dbfs_i16(ibytes));
            Work { samples: npx, ..Work::ZERO }
        });
        measure("rms_pie", "sample", npx, || {
            core::hint::black_box(rusty_esp_dsp_esp::pie_s3::rms_dbfs_i16(ibytes));
            Work { samples: npx, ..Work::ZERO }
        });

        // --- mix_i16: gated against the audio element, not a rewrite of it --
        // 512 samples, not NPX: by this point the heap is down to a few KiB
        // and these are two more buffers. The per-sample figure is what the
        // A/B reports, so a smaller working set says the same thing.
        const NMIX: usize = 512;
        let nmix = NMIX as u64;
        let mut msrc: alloc::vec::Vec<u128> = alloc::vec![0; NMIX / 8];
        let mut psrc: alloc::vec::Vec<u128> = alloc::vec![0; NMIX / 8];
        // SAFETY: `Vec<u128>` is 16-byte aligned and initialised; `i16` is POD.
        let (mref, mpie) = unsafe {
            (
                core::slice::from_raw_parts_mut(msrc.as_mut_ptr().cast::<u8>(), NMIX * 2),
                core::slice::from_raw_parts_mut(psrc.as_mut_ptr().cast::<u8>(), NMIX * 2),
            )
        };
        // SAFETY: as above -- the same bytes, viewed as the samples they are.
        let jbytes = unsafe {
            core::slice::from_raw_parts(jv.as_ptr().cast::<u8>(), NMIX * 2)
        };
        let mbytes = &ibytes[..NMIX * 2];
        rusty_esp_audio_core::elements::mix_i16(mbytes, jbytes, mref)
            .expect("mix scalar");
        rusty_esp_dsp_esp::pie_s3::mix_i16(&iv[..NMIX], &jv[..NMIX], unsafe {
            // SAFETY: `mpie` is the byte view of an aligned `Vec<u128>`.
            core::slice::from_raw_parts_mut(mpie.as_mut_ptr().cast::<i16>(), NMIX)
        });
        println!("PIEKERNEL mix_i16 identical={}", mref[..] == mpie[..]);
        measure("mix_scalar", "sample", nmix, || {
            let _ = rusty_esp_audio_core::elements::mix_i16(mbytes, jbytes, mref);
            Work { samples: nmix, ..Work::ZERO }
        });
        measure("mix_pie", "sample", nmix, || {
            // SAFETY: as above.
            let o = unsafe {
                core::slice::from_raw_parts_mut(mpie.as_mut_ptr().cast::<i16>(), NMIX)
            };
            rusty_esp_dsp_esp::pie_s3::mix_i16(&iv[..NMIX], &jv[..NMIX], o);
            Work { samples: nmix, ..Work::ZERO }
        });

        // --- mono_to_stereo: gated against the MonoToStereo element ---
        {
            use rusty_esp_audio_core::elements::MonoToStereo;
            use rusty_esp_audio_core::pipeline::Element;
            use rusty_esp_dsp::esp_core::pcm::{PcmBlock, PcmFormat, SampleFormat};
            use rusty_esp_dsp::esp_core::time::Micros;
            let f_mono = PcmFormat::new(16_000, 1, SampleFormat::I16).expect("fmt");
            let half = NMIX / 2; // so the doubled output fits the buffers above
            let halfu = half as u64;
            let mut e = MonoToStereo;
            // SAFETY: byte views of the two aligned scratch buffers.
            let (sref, spie) = unsafe {
                (
                    core::slice::from_raw_parts_mut(msrc.as_mut_ptr().cast::<u8>(), NMIX * 2),
                    core::slice::from_raw_parts_mut(psrc.as_mut_ptr().cast::<u8>(), NMIX * 2),
                )
            };
            let blk = PcmBlock::new(f_mono, Micros(0), &ibytes[..half * 2]).expect("blk");
            let _ = e.process(blk, sref).expect("upmix scalar");
            rusty_esp_dsp_esp::pie_s3::mono_to_stereo_i16(&iv[..half], unsafe {
                // SAFETY: `spie` is the byte view of an aligned `Vec<u128>`.
                core::slice::from_raw_parts_mut(spie.as_mut_ptr().cast::<i16>(), NMIX)
            });
            println!(
                "PIEKERNEL mono_to_stereo identical={}",
                sref[..half * 4] == spie[..half * 4]
            );
            measure("upmix_scalar", "sample", halfu, || {
                let blk = PcmBlock::new(f_mono, Micros(0), &ibytes[..half * 2]).expect("blk");
                let _ = e.process(blk, sref);
                Work { samples: halfu, ..Work::ZERO }
            });
            measure("upmix_pie", "sample", halfu, || {
                // SAFETY: as above.
                let o = unsafe {
                    core::slice::from_raw_parts_mut(spie.as_mut_ptr().cast::<i16>(), NMIX)
                };
                rusty_esp_dsp_esp::pie_s3::mono_to_stereo_i16(&iv[..half], o);
                Work { samples: halfu, ..Work::ZERO }
            });
        }

        // --- downscale2x_gray8: 64x32 over the gray bytes just produced ---
        //
        // `gd` already holds 2048 gray8 pixels from the yuyv arm, so the
        // downscale reads real sensor-derived bytes rather than a ramp. The
        // PIE destination is the FRONT OF `ys`, which is 16-byte aligned;
        // `reference` is a plain `Vec<u8>` and would have sent the twin down
        // its scalar arm silently, which is what a FLAT reading looks like.
        {
            const DW: u32 = 64;
            const DH: u32 = 32;
            let oute = (DW as usize / 2) * (DH as usize / 2);
            let _ = pixel::downscale2x_gray8(gd, DW, DH, &mut reference[..oute]);
            let _ = rusty_esp_dsp_esp::pie_s3::downscale2x_gray8(gd, DW, DH, &mut ys[..oute]);
            println!(
                "PIEKERNEL downscale2x_gray8 identical={} src_align={} dst_align={}",
                reference[..oute] == ys[..oute],
                gd.as_ptr() as usize % 16,
                ys.as_ptr() as usize % 16
            );
            let ou = oute as u64;
            measure("dscale_gray_scalar", "px_out", ou, || {
                let _ = pixel::downscale2x_gray8(gd, DW, DH, &mut reference[..oute]);
                Work { pixels: ou, ..Work::ZERO }
            });
            measure("dscale_gray_pie", "px_out", ou, || {
                let _ = rusty_esp_dsp_esp::pie_s3::downscale2x_gray8(gd, DW, DH, &mut ys[..oute]);
                Work { pixels: ou, ..Work::ZERO }
            });
        }

        // --- stereo_to_mono: gated against the StereoToMono element -------
        {
            use rusty_esp_audio_core::elements::StereoToMono;
            use rusty_esp_audio_core::pipeline::Element;
            use rusty_esp_dsp::esp_core::pcm::{PcmBlock, PcmFormat, SampleFormat};
            use rusty_esp_dsp::esp_core::time::Micros;
            let f_stereo = PcmFormat::new(16_000, 2, SampleFormat::I16).expect("fmt");
            let fr = NMIX / 2; // 256 frames in, 256 mono samples out
            let fru = fr as u64;
            let mut e = StereoToMono;
            // SAFETY: byte views of the two aligned scratch buffers.
            let (dref, dpie) = unsafe {
                (
                    core::slice::from_raw_parts_mut(msrc.as_mut_ptr().cast::<u8>(), NMIX * 2),
                    core::slice::from_raw_parts_mut(psrc.as_mut_ptr().cast::<u8>(), NMIX * 2),
                )
            };
            let blk = PcmBlock::new(f_stereo, Micros(0), &ibytes[..fr * 4]).expect("blk");
            let _ = e.process(blk, dref).expect("downmix scalar");
            rusty_esp_dsp_esp::pie_s3::stereo_to_mono_i16(&iv[..fr * 2], unsafe {
                // SAFETY: `dpie` is the byte view of an aligned `Vec<u128>`.
                core::slice::from_raw_parts_mut(dpie.as_mut_ptr().cast::<i16>(), NMIX)
            });
            println!(
                "PIEKERNEL stereo_to_mono identical={}",
                dref[..fr * 2] == dpie[..fr * 2]
            );
            measure("downmix_scalar", "sample", fru, || {
                let blk = PcmBlock::new(f_stereo, Micros(0), &ibytes[..fr * 4]).expect("blk");
                let _ = e.process(blk, dref);
                Work { samples: fru, ..Work::ZERO }
            });
            measure("downmix_pie", "sample", fru, || {
                // SAFETY: as above.
                let o = unsafe {
                    core::slice::from_raw_parts_mut(dpie.as_mut_ptr().cast::<i16>(), NMIX)
                };
                rusty_esp_dsp_esp::pie_s3::stereo_to_mono_i16(&iv[..fr * 2], o);
                Work { samples: fru, ..Work::ZERO }
            });
        }

        // --- the UNALIGNED arms: buffers that used to run fully scalar ---
        //
        // Every twin so far checks 16-byte alignment and hands anything else
        // to the oracle, so a slice starting one sample in got no SIMD at
        // all. `&iv[1..]` starts at 2 mod 16, which is exactly that case.
        // The alignment is printed beside each gate, because a buffer that
        // turned out to be aligned after all would make these arms measure
        // the aligned path and read as a win that is not there.
        {
            let ua = &iv[1..];
            let ub = &jv[1..];
            println!(
                "PIEUNALIGNED offset_a={} offset_b={}",
                ua.as_ptr() as usize % 16,
                ub.as_ptr() as usize % 16
            );
            let nu = (ua.len()) as u64;

            let sref = rusty_esp_dsp::sample::sum_sq_i16(ua);
            let spie = rusty_esp_dsp_esp::pie_s3::sum_sq_i16(ua);
            println!(
                "PIEKERNEL sum_sq_i16_unaligned identical={} scalar={sref} pie={spie}",
                sref == spie
            );
            measure("sumsq_un_scalar", "sample", nu, || {
                core::hint::black_box(rusty_esp_dsp::sample::sum_sq_i16(ua));
                Work { samples: nu, ..Work::ZERO }
            });
            measure("sumsq_un_pie", "sample", nu, || {
                core::hint::black_box(rusty_esp_dsp_esp::pie_s3::sum_sq_i16(ua));
                Work { samples: nu, ..Work::ZERO }
            });

            let pref = rusty_esp_dsp::sample::peak_abs_i16(ua);
            let ppie = rusty_esp_dsp_esp::pie_s3::peak_abs_i16(ua);
            println!(
                "PIEKERNEL peak_abs_i16_unaligned identical={} scalar={pref} pie={ppie}",
                pref == ppie
            );
            measure("peak_un_scalar", "sample", nu, || {
                core::hint::black_box(rusty_esp_dsp::sample::peak_abs_i16(ua));
                Work { samples: nu, ..Work::ZERO }
            });
            measure("peak_un_pie", "sample", nu, || {
                core::hint::black_box(rusty_esp_dsp_esp::pie_s3::peak_abs_i16(ua));
                Work { samples: nu, ..Work::ZERO }
            });

            // Opposing operands again, so the answer is negative and the
            // 40-bit sign reconstruction is under test on this path too.
            let dref = rusty_esp_dsp::sample::dot_i16(ua, ub);
            let dpie = rusty_esp_dsp_esp::pie_s3::dot_i16(ua, ub);
            println!(
                "PIEKERNEL dot_i16_unaligned identical={} negative={} scalar={dref} pie={dpie}",
                dref == dpie,
                dref < 0
            );
            measure("dot_un_scalar", "sample", nu, || {
                core::hint::black_box(rusty_esp_dsp::sample::dot_i16(ua, ub));
                Work { samples: nu, ..Work::ZERO }
            });
            measure("dot_un_pie", "sample", nu, || {
                core::hint::black_box(rusty_esp_dsp_esp::pie_s3::dot_i16(ua, ub));
                Work { samples: nu, ..Work::ZERO }
            });
        }

        // --- SAD at an ARBITRARY position, which is what a search does ---
        //
        // The aligned arm needs `stride % 16 == 0` and both bases aligned.
        // A reference block at a candidate motion vector satisfies neither,
        // so this arm is the one a real search would take. Stride 17 and
        // bases at 3 and 5 mod 16 are chosen to fail every condition the
        // aligned path tests, and all three are printed so that a buffer
        // which happened to qualify could not be mistaken for a win.
        {
            let mut ubuf: alloc::vec::Vec<u128> = alloc::vec![0; 48];
            // SAFETY: a `Vec<u128>` is 16-byte aligned and initialised;
            // viewing its 768 bytes as `u8` is a reinterpretation of POD.
            let ub = unsafe {
                core::slice::from_raw_parts_mut(ubuf.as_mut_ptr().cast::<u8>(), 768)
            };
            for (k, v) in ub.iter_mut().enumerate() {
                *v = (k.wrapping_mul(131) ^ (k >> 2)) as u8;
            }
            const US: usize = 17; // deliberately not a multiple of 16
            let (ua_blk, ub_blk) = (&ub[3..343], &ub[405..745]);
            println!(
                "PIEUNALIGNED sad stride={US} off_a={} off_b={} len={}",
                ua_blk.as_ptr() as usize % 16,
                ub_blk.as_ptr() as usize % 16,
                ua_blk.len()
            );

            let uref = rusty_esp_dsp::block::sad_16x16(ua_blk, US, ub_blk, US).unwrap_or(0);
            let upie =
                rusty_esp_dsp_esp::pie_s3::sad_16x16(ua_blk, US, ub_blk, US).unwrap_or(0);
            println!(
                "PIEKERNEL sad_16x16_unaligned identical={} scalar={uref} pie={upie}",
                uref == upie
            );
            measure("sad16_un_scalar", "block", 1, || {
                core::hint::black_box(
                    rusty_esp_dsp::block::sad_16x16(ua_blk, US, ub_blk, US).ok(),
                );
                Work { blocks: 1, ..Work::ZERO }
            });
            measure("sad16_un_pie", "block", 1, || {
                core::hint::black_box(
                    rusty_esp_dsp_esp::pie_s3::sad_16x16(ua_blk, US, ub_blk, US).ok(),
                );
                Work { blocks: 1, ..Work::ZERO }
            });

            let u8ref = rusty_esp_dsp::block::sad_8x8(ua_blk, US, ub_blk, US).unwrap_or(0);
            let u8pie = rusty_esp_dsp_esp::pie_s3::sad_8x8(ua_blk, US, ub_blk, US).unwrap_or(0);
            println!(
                "PIEKERNEL sad_8x8_unaligned identical={} scalar={u8ref} pie={u8pie}",
                u8ref == u8pie
            );
            measure("sad8_un_scalar", "block", 1, || {
                core::hint::black_box(rusty_esp_dsp::block::sad_8x8(ua_blk, US, ub_blk, US).ok());
                Work { blocks: 1, ..Work::ZERO }
            });
            measure("sad8_un_pie", "block", 1, || {
                core::hint::black_box(
                    rusty_esp_dsp_esp::pie_s3::sad_8x8(ua_blk, US, ub_blk, US).ok(),
                );
                Work { blocks: 1, ..Work::ZERO }
            });
        }

        // --- sum_sq_i16_le and rms_dbfs_i16 on an unaligned byte slice ----
        //
        // Both used to test 16-byte alignment themselves and hand anything
        // else to the oracle. They no longer do: the only question left is
        // whether the bytes ARE samples, and the reduction handles the rest.
        {
            let ubytes = &ibytes[2..];
            println!("PIEUNALIGNED le off={}", ubytes.as_ptr() as usize % 16);
            let nb = (ubytes.len() / 2) as u64;
            let lref = rusty_esp_dsp::sample::sum_sq_i16_le(ubytes);
            let lpie = rusty_esp_dsp_esp::pie_s3::sum_sq_i16_le(ubytes);
            println!(
                "PIEKERNEL sum_sq_i16_le_unaligned identical={} scalar={lref:?} pie={lpie:?}",
                lref == lpie
            );
            measure("sumsqle_un_scalar", "sample", nb, || {
                core::hint::black_box(rusty_esp_dsp::sample::sum_sq_i16_le(ubytes));
                Work { samples: nb, ..Work::ZERO }
            });
            measure("sumsqle_un_pie", "sample", nb, || {
                core::hint::black_box(rusty_esp_dsp_esp::pie_s3::sum_sq_i16_le(ubytes));
                Work { samples: nb, ..Work::ZERO }
            });

            let rref = rusty_esp_dsp::sample::rms_dbfs_i16(ubytes);
            let rpie = rusty_esp_dsp_esp::pie_s3::rms_dbfs_i16(ubytes);
            println!(
                "PIEKERNEL rms_dbfs_i16_unaligned identical={} scalar={rref} pie={rpie}",
                rref == rpie
            );
            measure("rms_un_scalar", "sample", nb, || {
                core::hint::black_box(rusty_esp_dsp::sample::rms_dbfs_i16(ubytes));
                Work { samples: nb, ..Work::ZERO }
            });
            measure("rms_un_pie", "sample", nb, || {
                core::hint::black_box(rusty_esp_dsp_esp::pie_s3::rms_dbfs_i16(ubytes));
                Work { samples: nb, ..Work::ZERO }
            });
        }

        // --- yuyv_to_gray8 from an UNALIGNED source, aligned destination -
        //
        // A cropped sub-region of a frame begins wherever its left edge
        // does; the gray buffer it is written into was allocated here and
        // so is aligned. `&ys[2..]` is that case exactly.
        {
            let usrc = &ys[2..];
            let upx = usrc.len() / 2;
            println!(
                "PIEUNALIGNED yuyv src_off={} dst_off={} px={upx}",
                usrc.as_ptr() as usize % 16,
                gd.as_ptr() as usize % 16
            );
            let _ = pixel::yuyv_to_gray8(usrc, &mut reference[..upx]);
            let _ = rusty_esp_dsp_esp::pie_s3::yuyv_to_gray8(usrc, &mut gd[..upx]);
            println!(
                "PIEKERNEL yuyv_to_gray8_unaligned identical={}",
                reference[..upx] == gd[..upx]
            );
            let u = upx as u64;
            measure("yuyv_un_scalar", "px", u, || {
                let _ = pixel::yuyv_to_gray8(usrc, &mut reference[..upx]);
                Work { pixels: u, ..Work::ZERO }
            });
            measure("yuyv_un_pie", "px", u, || {
                let _ = rusty_esp_dsp_esp::pie_s3::yuyv_to_gray8(usrc, &mut gd[..upx]);
                Work { pixels: u, ..Work::ZERO }
            });
        }

        report_memory("after_pie");
    }

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

    // ---- sample + block reductions (D3 candidates) -----------------------
    // Small buffers: the frame buffers already hold 172 800 of the 196 608 B
    // heap, so these are sized to fit the remainder with room to spare.
    const NS: usize = 2048;
    let pcm_a: alloc::vec::Vec<i16> = (0..NS)
        .map(|i| (((i as i32) * 37) % 65536 - 32768) as i16)
        .collect();
    let pcm_b: alloc::vec::Vec<i16> = (0..NS)
        .map(|i| (((i as i32) * 101) % 65536 - 32768) as i16)
        .collect();
    let ns = NS as u64;
    measure("dot_i16", "sample", ns, || {
        core::hint::black_box(rusty_esp_dsp::sample::dot_i16(&pcm_a, &pcm_b));
        Work { samples: ns, ..Work::ZERO }
    });
    measure("sum_sq_i16", "sample", ns, || {
        core::hint::black_box(rusty_esp_dsp::sample::sum_sq_i16(&pcm_a));
        Work { samples: ns, ..Work::ZERO }
    });
    measure("peak_abs_i16", "sample", ns, || {
        core::hint::black_box(rusty_esp_dsp::sample::peak_abs_i16(&pcm_a));
        Work { samples: ns, ..Work::ZERO }
    });
    let le_samples = (gray.len() / 2) as u64;
    measure("sum_sq_i16_le", "sample", le_samples, || {
        core::hint::black_box(rusty_esp_dsp::sample::sum_sq_i16_le(&gray));
        Work { samples: le_samples, ..Work::ZERO }
    });

    const NB: usize = 64;
    let res_blocks: alloc::vec::Vec<[i32; 16]> = (0..NB)
        .map(|b| {
            let mut x = [0i32; 16];
            let mut i = 0;
            while i < 16 {
                x[i] = (((b * 16 + i) as i32) % 511) - 255;
                i += 1;
            }
            x
        })
        .collect();
    let nb = NB as u64;
    measure("satd_4x4_sum", "block", nb, || {
        core::hint::black_box(rusty_esp_dsp::block::satd_4x4_sum(&res_blocks));
        Work { blocks: nb, ..Work::ZERO }
    });
    measure("residual_4x4", "block", blocks, || {
        let stride = W as usize;
        let mut acc = 0i32;
        for by in 0..blocks_y {
            for bx in 0..blocks_x {
                let at = by * 16 * stride + bx * 16;
                if let Ok(r) =
                    rusty_esp_dsp::block::residual_4x4(&gray[at..], stride, &gray[at + 1..], stride)
                {
                    acc = acc.wrapping_add(r[0]);
                }
            }
        }
        core::hint::black_box(acc);
        Work { blocks, ..Work::ZERO }
    });
    // hadamard_4x4 is public API and no longer reached through satd_4x4
    // (which fuses its own butterflies), so measure it directly.
    measure("hadamard_4x4", "block", nb, || {
        let mut acc = 0i32;
        for blk in &res_blocks {
            acc = acc.wrapping_add(rusty_esp_dsp::block::hadamard_4x4(blk)[0]);
        }
        core::hint::black_box(acc);
        Work { blocks: nb, ..Work::ZERO }
    });
    // isqrt: the CSI amplitude path's integer square root.
    measure("isqrt", "sample", ns, || {
        let mut acc = 0u32;
        for i in 0..NS {
            acc = acc.wrapping_add(rusty_esp_dsp::int::isqrt((i as u32) * 7919));
        }
        core::hint::black_box(acc);
        Work { samples: ns, ..Work::ZERO }
    });

    // pcm::convert -- the sample-format converter, both hot directions.
    // Small buffers: the frame buffers leave only ~23 KiB of heap.
    {
        use rusty_esp_dsp::esp_core::pcm::{PcmBlock, PcmFormat, SampleFormat};
        use rusty_esp_dsp::esp_core::time::Micros;
        use rusty_esp_dsp::sample::pcm;
        const NP: usize = 512;
        let np = NP as u64;
        let fmt_i16 = PcmFormat::new(16_000, 1, SampleFormat::I16).expect("fmt");
        let fmt_f32 = PcmFormat::new(16_000, 1, SampleFormat::F32).expect("fmt");
        let src_i16: alloc::vec::Vec<u8> = (0..NP * 2).map(|i| (i % 251) as u8).collect();
        let mut buf_f32 = vec![0u8; NP * 4];
        measure("pcm_i16_to_f32", "sample", np, || {
            let b = PcmBlock::new(fmt_i16, Micros(0), &src_i16).expect("blk");
            let _ = pcm::convert(b, SampleFormat::F32, &mut buf_f32);
            Work { samples: np, ..Work::ZERO }
        });
        let mut buf_i16 = vec![0u8; NP * 2];
        measure("pcm_f32_to_i16", "sample", np, || {
            let b = PcmBlock::new(fmt_f32, Micros(0), &buf_f32).expect("blk");
            let _ = pcm::convert(b, SampleFormat::I16, &mut buf_i16);
            Work { samples: np, ..Work::ZERO }
        });
    }

    // ---- audio element kernels (rusty_esp_audio-core) --------------------
    // Per-sample DSP loops with accumulator chains and per-frame channel
    // dispatch -- the shape that paid -43% on peak_abs_i16. Audited for
    // ALLOCATION earlier and clean; never for instruction count.
    {
        use rusty_esp_audio_core::elements::{
            Biquad, BiquadKind, DcBlock, Gain, LinearResampler, MonoToStereo, StereoToMono, mix_i16,
        };
        use rusty_esp_audio_core::pipeline::Element;
        use rusty_esp_dsp::esp_core::pcm::{PcmBlock, PcmFormat, SampleFormat};
        use rusty_esp_dsp::esp_core::time::Micros;

        // 256 frames: the heap has ~11 KiB left and these are five buffers.
        const NA: usize = 256;
        let na = NA as u64;
        let f_mono = PcmFormat::new(16_000, 1, SampleFormat::I16).expect("fmt");
        let f_stereo = PcmFormat::new(16_000, 2, SampleFormat::I16).expect("fmt");

        let mono: alloc::vec::Vec<u8> = (0..NA * 2).map(|i| (i % 251) as u8).collect();
        let stereo: alloc::vec::Vec<u8> = (0..NA * 4).map(|i| (i % 241) as u8).collect();
        let mut mono_out = vec![0u8; NA * 2];
        let mut stereo_out = vec![0u8; NA * 4];
        report_memory("audio_buffers");

        // Not unity: unity is a byte-exact copy shortcut and measures memcpy.
        let mut gain = Gain::linear(0.5);
        measure("gain_i16", "sample", na, || {
            let b = PcmBlock::new(f_mono, Micros(0), &mono).expect("blk");
            let _ = gain.process(b, &mut mono_out);
            Work { samples: na, ..Work::ZERO }
        });

        let mut m2s = MonoToStereo;
        measure("mono_to_stereo", "sample", na, || {
            let b = PcmBlock::new(f_mono, Micros(0), &mono).expect("blk");
            let _ = m2s.process(b, &mut stereo_out);
            Work { samples: na, ..Work::ZERO }
        });

        let mut s2m = StereoToMono;
        measure("stereo_to_mono", "frame", na, || {
            let b = PcmBlock::new(f_stereo, Micros(0), &stereo).expect("blk");
            let _ = s2m.process(b, &mut mono_out);
            Work { samples: na, ..Work::ZERO }
        });

        measure("mix_i16", "sample", na, || {
            let _ = mix_i16(&mono, &mono, &mut mono_out);
            Work { samples: na, ..Work::ZERO }
        });

        let mut dc = DcBlock::new();
        measure("dc_block_mono", "sample", na, || {
            let b = PcmBlock::new(f_mono, Micros(0), &mono).expect("blk");
            let _ = dc.process(b, &mut mono_out);
            Work { samples: na, ..Work::ZERO }
        });

        let mut bq = Biquad::new(BiquadKind::LowPass { f0: 3000.0, q: 0.7071 });
        measure("biquad_mono", "sample", na, || {
            let b = PcmBlock::new(f_mono, Micros(0), &mono).expect("blk");
            let _ = bq.process(b, &mut mono_out);
            Work { samples: na, ..Work::ZERO }
        });

        let mut agc = rusty_esp_audio_core::elements::Agc::default();
        measure("agc_i16", "sample", na, || {
            let b = PcmBlock::new(f_mono, Micros(20_000), &mono).expect("blk");
            let _ = agc.process(b, &mut mono_out);
            Work { samples: na, ..Work::ZERO }
        });

        let mut vad = rusty_esp_audio_core::elements::EnergyVad::default();
        measure("vad_i16", "sample", na, || {
            let b = PcmBlock::new(f_mono, Micros(20_000), &mono).expect("blk");
            let _ = vad.process(b, &mut mono_out);
            Work { samples: na, ..Work::ZERO }
        });

        // 16k -> 48k: in_r = 1, out_r = 3, so three output frames per input.
        let mut rs = LinearResampler::new(16_000, 48_000).expect("rs");
        let mut rs_out = vec![0u8; rs.max_output_frames(NA) * 2];
        measure("resample_16k_48k", "out_frame", na * 3, || {
            let b = PcmBlock::new(f_mono, Micros(0), &mono).expect("blk");
            let _ = rs.process(b, &mut rs_out);
            Work { samples: na * 3, ..Work::ZERO }
        });
        // ---- the PRODUCTION audio block, as the shipping firmware runs it --
        //
        // The reachability census (ledger R1) found that four of thirty-nine
        // kernels reach a shipping firmware, and that ONE firmware does all
        // the reaching: `xiao-s3-sense-idf-pdm-udp`. Its per-block loop is
        //
        //     let out = pipeline.process(block, scratch)?;   // 1 stage: DcBlock
        //     let level = rms_dbfs_i16(out.data);
        //     vad.judge(level);
        //
        // Every DcBlock number in this probe was taken by calling
        // `dc.process()` DIRECTLY. Production does not: it goes through
        // `Pipeline::process`, which per stage checks the output format,
        // asks `max_output_bytes`, builds a `PcmBlock`, and ping-pongs
        // between the two halves of the scratch buffer -- none of which this
        // probe has ever measured. `pipeline_dcblock` beside `dc_block_mono`
        // in the same binary IS that tax.
        {
            use rusty_esp_audio_core::pipeline::Pipeline;
            let mut pdc = rusty_esp_audio_core::elements::DcBlock::new();
            let mut pipe = Pipeline::new([&mut pdc as &mut dyn Element]);
            let scratch_len = pipe.scratch_bytes(f_mono, NA * 2).expect("scratch");
            let mut scratch = vec![0u8; scratch_len];
            println!("PRODPROBE scratch_bytes={scratch_len} for {} in", NA * 2);

            measure("pipeline_dcblock", "sample", na, || {
                let b = PcmBlock::new(f_mono, Micros(0), &mono).expect("blk");
                let out = pipe.process(b, &mut scratch).expect("pipe");
                core::hint::black_box(&out);
                Work { samples: na, ..Work::ZERO }
            });

            let mut pvad = rusty_esp_audio_core::elements::EnergyVad::default();
            measure("prod_audio_block", "sample", na, || {
                let b = PcmBlock::new(f_mono, Micros(0), &mono).expect("blk");
                if let Some(out) = pipe.process(b, &mut scratch).expect("pipe") {
                    let level = rusty_esp_audio_core::rms_dbfs_i16(out.data);
                    core::hint::black_box(pvad.judge(level));
                }
                Work { samples: na, ..Work::ZERO }
            });
        }

        report_memory("after_audio");
    }

    // ---- CSI presence kernels (rusty_esp_signal-core) --------------------
    // `features` runs once per radio frame at 20-50 Hz; `push` folds a
    // 50-frame window over up to 56 subcarriers on every one of them.
    {
        use rusty_esp_signal_core::radar::csi::{Config, CsiFrame, Layout, PresenceDetector};
        use rusty_esp_dsp::esp_core::time::Micros;

        // 64 interleaved (imaginary, real) i8 pairs, the shape the radio hands
        // over. Deterministic, and spread over the whole i8 range so the
        // amplitudes are not all one magnitude.
        let iq: alloc::vec::Vec<i8> = (0..128)
            .map(|i| (((i as i32) * 37) % 255 - 127) as i8)
            .collect();
        let frame = CsiFrame { timestamp: Micros(0), rssi: -50, channel: 6, iq: &iq };
        let layout = Layout::LLTF_20MHZ;
        fn feats_from(
            iq: &[i8],
            layout: &Layout,
        ) -> rusty_esp_signal_core::radar::csi::Features {
            CsiFrame { timestamp: Micros(0), rssi: -50, channel: 6, iq }
                .features(layout)
                .expect("features")
        }
        let sc = layout.count() as u64;
        measure("csi_features", "subcarrier", sc, || {
            core::hint::black_box(frame.features(&layout).ok());
            Work { samples: sc, ..Work::ZERO }
        });

        // FOUR distinct frames, cycled.
        //
        // Pushing one `Features` over and over makes every frame in the
        // window identical, so the population variance is exactly zero,
        // `var_w2` is zero, and `isqrt(0)` returns on its first line. That
        // measures the reduction with its most expensive step switched OFF,
        // and would price any change to the accumulate loop as a larger win
        // than real data could ever see. Four variants give the window real
        // spread; cycling an index costs one add per rep, where perturbing
        // the block in place would cost a loop as long as the kernel.
        let mut variants = [feats_from(&iq, &layout); 4];
        for (k, v) in variants.iter_mut().enumerate() {
            let mut shifted = iq.clone();
            for (i, b) in shifted.iter_mut().enumerate() {
                *b = b.wrapping_add(((i * 13 + k * 41) % 97) as i8);
            }
            *v = feats_from(&shifted, &layout);
        }
        let mut det = alloc::boxed::Box::new(PresenceDetector::<50>::new(Config::default()));
        let mut t = 0u64;
        let mut k = 0usize;
        measure("csi_wander", "subcarrier", sc, || {
            t += 20_000;
            k = (k + 1) & 3;
            core::hint::black_box(det.push(&variants[k], Micros(t)));
            Work { samples: sc, ..Work::ZERO }
        });
        // Say what the window actually holds, so a reader can see the
        // reduction was exercised rather than short-circuited.
        println!("CSIPROBE wander={} warm={}", det.wander(), det.warm());
        report_memory("after_csi");
    }

    // ---- the LD2410 UART parser: a state machine driven one byte at a time
    {
        use rusty_esp_signal_core::radar::ld2410::Parser;
        // The engineering-mode report: 45 bytes, of which 35 are a DATA
        // section the state machine re-dispatches on for every single one.
        const REPORT: [u8; 45] = [
            0xF4, 0xF3, 0xF2, 0xF1, 0x23, 0x00, 0x01, 0xAA, 0x03, 0x1E, 0x00, 0x3C, 0x00, 0x00,
            0x39, 0x00, 0x00, 0x08, 0x08, 0x3C, 0x22, 0x05, 0x03, 0x03, 0x04, 0x03, 0x06, 0x05,
            0x00, 0x00, 0x39, 0x10, 0x13, 0x06, 0x06, 0x08, 0x04, 0x03, 0x05, 0x55, 0x00, 0xF8,
            0xF7, 0xF6, 0xF5,
        ];
        let nb = REPORT.len() as u64;
        let mut parser = Parser::new();
        measure("ld2410_feed", "byte", nb, || {
            let mut at = 0usize;
            while at < REPORT.len() {
                let (used, frame) = parser.feed_slice(&REPORT[at..]);
                core::hint::black_box(&frame);
                if used == 0 {
                    break;
                }
                at += used;
            }
            Work { bytes: nb, ..Work::ZERO }
        });
    }

    // ---- the pixel kernels the DSP crate did not take --------------------
    {
        use rusty_esp_image_core::ops;
        measure("rotate90_gray8", "px", px, || {
            let _ = ops::rotate90_gray8(&gray, W, H, &mut small);
            Work::pixels(px)
        });
        // gray8, NOT rgb565: `small` is PX bytes, and an rgb565 source needs
        // 2*PX, so `expect_len` rejected it and the first run measured an
        // ERROR RETURN at 143 ps/px -- 0.03 cycles a pixel, which is what an
        // impossible number looks like when a kernel never ran.
        // The shape `rotate180` had before the cursor rewrite, kept HERE
        // rather than in the crate so the two can be measured in one build --
        // the optimised version shipped before it ever had a valid baseline
        // (its first probe passed a half-sized destination and measured an
        // error return), so this is how it earns one.
        fn rotate180_as_written(src: &[u8], bpp: usize, dst: &mut [u8]) {
            let n = src.len() / bpp;
            for (i, s) in src.chunks_exact(bpp).enumerate() {
                let o = (n - 1 - i) * bpp;
                dst[o..o + bpp].copy_from_slice(s);
            }
        }
        {
            use rusty_esp_dsp::esp_core::frame::{Geometry, PixelFormat};
            use rusty_esp_image_core::source::{ImageSource, TestPattern};
            let g = Geometry::new(W, H, PixelFormat::Rgb565).expect("geom");
            let mut tp = TestPattern::new(g, 30).expect("pattern");
            measure("testpattern_rgb565", "px", px, || {
                let _ = tp.grab(&mut rgb565);
                Work::pixels(px)
            });
        }

        measure("rotate180_as_written", "px", px, || {
            rotate180_as_written(&gray, 1, &mut small);
            Work::pixels(px)
        });
        measure("rotate180_gray8", "px", px, || {
            let _ = ops::rotate180(&gray, 1, &mut small);
            Work::pixels(px)
        });
        // No EOI in this buffer, so the scan runs its worst case -- which is
        // the case a DMA over-read actually hits.
        measure("jpeg_find_eoi", "byte", PX as u64, || {
            core::hint::black_box(rusty_esp_image_core::jpeg::find_eoi(&gray));
            Work { bytes: PX as u64, ..Work::ZERO }
        });
    }

    measure("sad_8x8", "block", blocks, || {
        let stride = W as usize;
        let mut sum = 0u32;
        for by in 0..blocks_y {
            for bx in 0..blocks_x {
                let at = by * 16 * stride + bx * 16;
                if let Ok(v) =
                    rusty_esp_dsp::block::sad_8x8(&gray[at..], stride, &gray[at + 1..], stride)
                {
                    sum = sum.wrapping_add(v);
                }
            }
        }
        core::hint::black_box(sum);
        Work { blocks, ..Work::ZERO }
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
