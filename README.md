# rusty_esp_dsp

[![crates.io](https://img.shields.io/crates/v/rusty_esp_dsp.svg)](https://crates.io/crates/rusty_esp_dsp)
[![docs.rs](https://docs.rs/rusty_esp_dsp/badge.svg)](https://docs.rs/rusty_esp_dsp)
[![license](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](#license)

ESP-DSP remade for the Janus family: the scalar kernel home. The pixel
conversions a camera pipeline needs, the PCM reductions and sample-format
conversion an audio pipeline needs, the H.264 block costs, the integer
helpers the fixed-point paths share — each one scalar, bounds-checked,
`no_std`, `forbid(unsafe)`, and the oracle every faster twin (ESP32-S3
`ee.*`, ESP32-P4 `esp.*`) will be gated against, byte for byte.

Part of **Janus**, the Remade-With-Rust programme that rebuilds the Espressif
ESP32 and Arduino application portfolio in memory-safe Rust so hardware makers
can ship products that plug straight into the MATA home computer.

- This package's plan: [docs/plans/rusty_esp_dsp.md](docs/plans/rusty_esp_dsp.md)
- Every number: [docs/LEDGER.md](docs/LEDGER.md)
- The family plan: Janus `docs/plans/janus-mission.md` (umbrella repo)

**Claims discipline:** this README makes no performance or capability claim that
is not backed by a test, a benchmark ledger entry, or a kill test recorded in the
plan. There is no speed number here yet, because nothing has been measured on a
chip; D1 builds the harness and D2 produces the first board row.

## Status

**D0 shipped on the host (2026-09-02).** The kernels that two function
packages carried, or that a chip-side speedup would be spent on, live here
now, and the packages import them:

| module | what | came from | consumers today |
|---|---|---|---|
| `pixel` | RGB565 pack/unpack, BT.601 YUV→RGB, YUYV→RGB888/RGB565/Gray8, RGB565↔RGB888, 2× box downscale (Gray8, RGB565) | `rusty_esp_image-core::ops`, verbatim | image (re-exports at the old paths), video through image |
| `sample` | `dot_i16`, `sum_sq_i16`, `sum_sq_i16_le`, `peak_abs_i16`, `rms_dbfs_i16` | `rusty_esp_audio-core`, verbatim | audio (AGC, VAD, the level meter) |
| `sample::pcm` | sample-format conversion with `swresample`'s rules | `rusty_esp_audio-core::codec::pcm`, verbatim | audio |
| `block` | `sad_4x4` / `sad_8x8` / `sad_16x16`, `residual_4x4`, `hadamard_4x4`, `satd_4x4`, `satd_4x4_sum` | written scalar to match `rusty_h264-common` | none yet (D3 wires the S3 twins through `rusty_h264`'s accel seam) |
| `int` | `isqrt` | `rusty_esp_signal-core::radar::csi`, verbatim | signal (CSI amplitude) |
| `probe` | `Work` counters, `pipeline_gain_permille`, `ceiling` → `Verdict` | new | the D1 harness and every ledger row after it |

The gate D0 had to pass, and did: every moved kernel is byte-identical to
the copy it replaced over a generated corpus (the copies live in
`tests/moved.rs`, untouched), the block kernels agree with `rusty_h264-common`
on every block of a generated corpus including block counts that are not a
multiple of four, and the three consumers' own test suites are unchanged.
Counts and commands are in the ledger.

## The rule that keeps this crate small

A kernel lives next to its one caller until a second package needs it, or
until it is the thing a PIE twin would be spent on. Then it moves here,
**scalar first**, with the copy it replaces held in a test until the move is
proven byte-identical. The scalar version never leaves the tree.

Before any twin is written, `probe::ceiling` prices it: expected pipeline
gain = stage share × (1 − 1/speedup). A 6 % stage made 4× faster buys 4.5 %;
if the harness's null-arm floor is wider than that, the twin is not built and
the reason is recorded.

Non-goals: no FFT until a consumer needs one (audio's Opus question, A4,
starts with `rusty-opus` scalar cycles, not a kernel); no neural-network ops
(ESP-DL / ESP-NN are FFai's); no product types, no allocator, no drivers.

## Layout

```text
crates/rusty_esp_dsp     no_std + forbid(unsafe): the scalar kernels and the probe
  src/pixel.rs           packed-pixel conversions and downscales
  src/sample.rs          i16 reductions; sample/pcm.rs the format conversion
  src/block.rs           H.264 block costs
  src/int.rs             integer helpers
  src/probe.rs           work counters and the ceiling probe
  tests/moved.rs         byte identity against the copies D0 replaced
  tests/h264_oracle.rs   byte identity against rusty_h264-common's transform
docs/plans/              the roadmap: D0 to D4, the kernel inventory, the decision log
docs/LEDGER.md           every number, with the run that produced it
```

`crates/rusty_esp_dsp-esp` (the PIE twins behind `pie-s3` / `pie-p4`, the
only fenced `unsafe`) arrives with D2; `bench/` (the pinned, interleaved,
null-armed host harness) with D1.

## Build

```sh
cargo test --workspace                                   # host: unit + the two oracle suites
cargo clippy --workspace --all-targets -- -D warnings
cargo check -p rusty_esp_dsp --no-default-features --target riscv32imac-unknown-none-elf
cargo check -p rusty_esp_dsp --no-default-features --features alloc --target riscv32imafc-unknown-none-elf
cargo deny check
```

Features: `std` (default) implies `alloc`; with neither the crate is pure
`core`. Depends on `rusty_esp_core` only. The H.264 oracle is a
dev-dependency on `rusty_h264-common` with its default features off (no
allocator hijack, no x86 asm).

## License

MIT OR Apache-2.0, at your option.
