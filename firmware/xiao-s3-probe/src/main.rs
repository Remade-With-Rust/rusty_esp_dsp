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
