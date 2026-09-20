# rusty_esp_dsp-esp

[![Remade With Rust](https://img.shields.io/badge/Remade%20With-Rust-000?logo=rust&logoColor=fff)](https://github.com/remade-with-rust) [![By Mata Network](https://img.shields.io/badge/by-Mata%20Network-5b2be0)](https://www.mata.network) [![crates.io](https://img.shields.io/crates/v/rusty_esp_dsp-esp.svg)](https://crates.io/crates/rusty_esp_dsp-esp) [![docs.rs](https://docs.rs/rusty_esp_dsp-esp/badge.svg)](https://docs.rs/rusty_esp_dsp-esp) [![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue)](https://github.com/Remade-With-Rust/rusty_esp_dsp/blob/main/LICENSE-MIT)

The chip half of the kernel package: hand-written vector twins of the scalar kernels for the ESP32-S3 and ESP32-P4 instruction extensions, each gated **byte-identical** against the scalar oracle that stays in the tree.

A twin that is not byte-identical to its oracle is a bug, not an optimisation. The scalar version is never deleted — it is the definition.

## What is twinned today

Enable `pie-s3` and hold `default_kernels()`; on an ESP32-S3 nine of the
seam's thirteen kernels run `ee.*` code, and everywhere else — a host build,
a host test, a RISC-V target — every one of them is the scalar oracle, so the
feature is safe to leave on.

```toml
rusty_esp_dsp-esp = { version = "0.1", features = ["pie-s3"] }
```

`yuyv_to_gray8`, `downscale2x_gray8`, `yuyv_to_rgb565`, `downscale2x_rgb565`,
`dot_i16`, `sum_sq_i16`, `peak_abs_i16`, `sad_8x8`, `sad_16x16`. Beyond the
seam the crate also carries twins for `rms_dbfs_i16`, `gain_i16`, `mix_i16`,
the channel converters, the integer `convert` pairs, `rotate90_gray8`,
`sad_4x4` and the Annex-B start-code scan, which the audio, image and video
packages reach through their own `pie-s3` features.

The four that are NOT twinned say why in the source rather than by omission:
`satd_4x4_sum` was built twice and measured worse, and the three RGB888
conversions are impossible on this unit — three bytes a pixel needs a 3-way
deinterleave and PIE has only 2-way `zip`/`unzip` with no general byte
permute.

Misaligned buffers are not a fallback to scalar: the unaligned-load idiom
(`ee.ld.128.usar.ip` + `ee.src.q`) reaches them too, and a slice starting one
sample in still runs vector code.

## Where the evidence is

This crate is part of [`rusty_esp_dsp`](https://crates.io/crates/rusty_esp_dsp). The
hardware results, the method lines and the open defects live in that package's
[README](https://github.com/Remade-With-Rust/rusty_esp_dsp#readme) and in
[`docs/LEDGER.md`](https://github.com/Remade-With-Rust/rusty_esp_dsp/blob/main/docs/LEDGER.md), where no number
appears without the run that produced it.

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
