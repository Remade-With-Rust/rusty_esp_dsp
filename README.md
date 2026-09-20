### In The Wild with 44 Active Installs

FREE RAG Converter Online -- <a href="https://RAGconverter.com">RAGconverter.com</a>

# rusty_esp_dsp

[![Remade With Rust](https://img.shields.io/badge/Remade%20With-Rust-000?logo=rust&logoColor=fff)](https://github.com/remade-with-rust) [![By Mata Network](https://img.shields.io/badge/by-Mata%20Network-5b2be0)](https://www.mata.network) [![crates.io](https://img.shields.io/crates/v/rusty_esp_dsp.svg)](https://crates.io/crates/rusty_esp_dsp) [![docs.rs](https://docs.rs/rusty_esp_dsp/badge.svg)](https://docs.rs/rusty_esp_dsp) [![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue)](https://github.com/Remade-With-Rust/rusty_esp_dsp/blob/main/LICENSE-MIT)

The pixel and sample kernels the **Janus** ESP32 family shares: colour
conversion, scaling, block difference, and the arithmetic that image, video and
audio would otherwise each write for themselves. Pure Rust, no C, no FFI,
`no_std` by default. Every kernel has a scalar twin kept in the tree as its
oracle, and every accelerated path is gated byte-identical against it.

* **One implementation, many callers.** Eight kernels — YUYV to 24-bit and
  16-bit colour, 16-bit to 24-bit and back, half-size colour and grayscale,
  YUYV to grayscale, and 16×16 block difference — used by the camera, the
  encoder and the detector rather than duplicated in each.
* **Allocation-free by construction.** The chip reported its own free memory at
  four stages of a run and the figure never moved: **not one of the eight
  kernels allocates.** That is a property no code review can establish and a
  board can.
* **Measured on the chip, because the laptop lies.** The optimisation target
  for this package was originally chosen from a share table measured on a
  development machine, where one kernel was 63.1% of the work. On the part the
  code actually ships to, that kernel is **15.4%**, nothing exceeds 22.4%, and
  four kernels sit in a band together. See below — it is the most useful thing
  this package has produced.

## What has run on hardware

Measured on a Seeed XIAO ESP32-S3 Sense. The last column is the finding.

| kernel | share on the laptop | share on the chip | laptop faster by |
|---|---:|---:|---:|
| YUYV → 24-bit colour | 63.1% | **15.4%** | 85× |
| YUYV → 16-bit colour | 14.6% | **22.4%** | 534× |
| 16-bit → 24-bit colour | 6.3% | 19.4% | 1071× |
| half-size 16-bit colour | 5.8% | 17.7% | 1068× |
| 24-bit → 16-bit colour | 5.1% | 13.9% | 961× |
| YUYV → grayscale | 2.9% | 5.0% | 602× |
| half-size grayscale | 1.6% | 2.9% | 617× |
| block difference | 0.7% | 3.4% | 1617× |

**The last column explains the first two.** A laptop is between 85× and 1617×
faster at kernels doing comparable work per pixel. That nineteen-fold spread is
not a property of the chip — it is a map of which kernels the development
machine's compiler managed to vectorise, and the kernel where the gap is
smallest is the one it failed on. The host table had been ranking the host
compiler's blind spot.

A second hardware result, from adopting the house allocator: **four of eight
kernels moved by up to 8%, in both directions, with no allocator call inside
any of them.** It was where the buffers landed in memory, not the allocator's
speed — and a later release that deleted half the allocator's code left every
kernel within six parts per million. Placement, proven both ways.

Every number, with the run that produced it:
[`docs/LEDGER.md`](https://github.com/Remade-With-Rust/rusty_esp_dsp/blob/main/docs/LEDGER.md).

## The chip's vector unit

The ESP32-S3 has a 128-bit SIMD unit (`ee.*`, Espressif calls it PIE). It is
reachable from Rust through `core::arch::asm!` on the `esp` toolchain, and
`rusty_esp_dsp-esp` now carries hand-written twins for it — **every one gated
byte-identical against the scalar kernel that stays in the tree as the
oracle.** A twin that is not byte-identical is a bug, not an optimisation.

Measured on a Seeed XIAO ESP32-S3 Sense, same binary, both arms:

| kernel | scalar | vector | |
|---|---:|---:|---:|
| `rotate90_gray8` | 201,137 | 9,332 | **−95.4%** |
| `dot_i16` | 271,354 | 11,745 | **−93.9%** |
| `sum_sq_i16` | 160,527 | 10,005 | **−93.8%** |
| Annex-B start-code scan | 119,815 | 7,911 | **−93.4%** |
| `yuyv_to_gray8` | 52,875 | 6,791 | **−87.2%** |
| `peak_abs_i16` | 76,707 | 11,005 | **−85.7%** |
| `downscale2x_rgb565` | 1,330,835 | 458,773 | **−65.5%** |
| `yuyv_to_rgb565` | 399,135 | 146,995 | **−63.2%** |

(picoseconds per element; lower is better)

Three things are worth knowing before you reach for this.

**The instruction semantics were read off the silicon, not a manual.** Running
an instruction on known byte patterns and printing every register it could
have touched settles its behaviour more exactly than prose. Several readings
contradicted the obvious guess *and would still have returned plausible
numbers* — the accumulator is 40 bits rather than 64, and one multiply
instruction is still uncharacterised and therefore used nowhere.

**Not everything pays, and the failures are recorded with their numbers.** A
4x4 Hadamard twin was built twice and lost both times; the fused load-op
instruction family is slower than the two instructions it replaces; RGB565
to RGB888 is impossible on this unit at all, because three bytes a pixel
needs a 3-way deinterleave the ISA does not have.

**A kernel nobody calls is not an optimisation.** Every twin here is reached
through the seam, and the probe proves it by comparing the trait call against
the twin and against the oracle — wired and unwired differ by an order of
magnitude, so the check cannot be ambiguous.

Every number, with the run that produced it and the reverts:
[`docs/LEDGER.md`](https://github.com/Remade-With-Rust/rusty_esp_dsp/blob/main/docs/LEDGER.md).
What is twinned, what is not, and why:
[`docs/TWIN-BACKLOG.md`](https://github.com/Remade-With-Rust/rusty_esp_dsp/blob/main/docs/TWIN-BACKLOG.md).

## Using it

```rust
use rusty_esp_dsp::ops;

// Sensor gives YUYV; the encoder wants RGB565. No allocation, no copy back.
ops::yuyv_to_rgb565(&yuyv, &mut rgb565, width, height)?;

// Cheap motion signal: sum of absolute differences over a 16x16 block.
let score = ops::sad_16x16(&previous, &current, stride);
```

On an ESP32-S3, take the same kernels through the seam and the vector
twins run instead — same bytes out, and the scalar is still there for
every other target:

```toml
rusty_esp_dsp-esp = { version = "0.1", features = ["pie-s3"] }
```

```rust
use rusty_esp_dsp::seam::{PixelKernels, SampleKernels};

let k = rusty_esp_dsp_esp::default_kernels();
k.yuyv_to_gray8(&yuyv, &mut gray)?;
let energy = k.sum_sq_i16(&samples);
```

## Two tracks

| track | what it is | this crate |
|---|---|---|
| **A** | `std` on ESP-IDF | `--features std` |
| **B** | `no_std` on `esp-hal` | default |

Hand-written vector kernels for the chip's own extension are **not** here yet:
the extension has no Rust intrinsics, so that is a spike before it is a
feature, and the share table above is what will aim it.

```sh
cargo test --workspace
cargo check -p rusty_esp_dsp --no-default-features --target riscv32imac-unknown-none-elf
```

## Part of Janus

**Janus** rebuilds the Espressif ESP32 and Arduino application portfolio as
independent, memory-safe Rust packages — so a hardware maker can ship a device
that the [MATA](https://www.mata.network) home computer discovers, catalogs honestly, adopts
under its own identity, and pays for. Ten packages, three layers, and the
dependency direction never reverses.

| layer | packages |
|---|---|
| **0 — the vocabulary** | [`rusty_esp_core`](https://crates.io/crates/rusty_esp_core) · [`rusty_esp_dsp`](https://crates.io/crates/rusty_esp_dsp) |
| **1 — the functions** | [`rusty_esp_image`](https://crates.io/crates/rusty_esp_image) · [`rusty_esp_video`](https://crates.io/crates/rusty_esp_video) · [`rusty_esp_audio`](https://crates.io/crates/rusty_esp_audio) · [`rusty_esp_signal`](https://crates.io/crates/rusty_esp_signal) · [`rusty_esp_mid`](https://crates.io/crates/rusty_esp_mid) · [`rusty_esp_iroh`](https://crates.io/crates/rusty_esp_iroh) |
| **2 — the surfaces** | [`rusty_esp_arduino`](https://crates.io/crates/rusty_esp_arduino) — the sketch facade · `espino` — the maker's CLI (not published) |

Every package is host-verified against an external oracle and keeps a ledger
in which no number appears without the run that produced it. **Five of seven
device profiles have now run their kill tests on real silicon**, three of them
over a Wi-Fi network the board hosts itself.

Also check out the rest of [Remade With Rust](https://github.com/remade-with-rust) — including
[`rusty_alloc`](https://crates.io/crates/rusty_alloc), the pure-Rust rebuild of
mimalloc that these firmwares run on, and
[`rusty_jpeg`](https://crates.io/crates/rusty_jpeg), the JPEG engine behind the
camera path — and our sister project
[remade_ffmpeg_rs](https://github.com/Remade-With-Rust/remade_ffmpeg_rs), a ground-up Rust rebuild of FFmpeg.

## About Mata Network

[Mata Network](https://www.mata.network) builds sovereign, self-hostable infrastructure.
**Remade With Rust** is our open-source home for the permissively-licensed
building blocks that work depends on.

## License

MIT OR Apache-2.0, at your option. See [LICENSE-MIT](https://github.com/Remade-With-Rust/rusty_esp_dsp/blob/main/LICENSE-MIT)
and [LICENSE-APACHE](https://github.com/Remade-With-Rust/rusty_esp_dsp/blob/main/LICENSE-APACHE).
