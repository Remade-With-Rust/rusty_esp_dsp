# rusty_esp_dsp — ledger

Every number this package quotes lives here with the run that produced it.
There is no speed number yet: nothing has been measured on a chip, and the
host harness (D1) does not exist. When it does, every row it produces carries
its method line — pinned? CPU time or wall? how many pairs? null-arm floor?

## D0 correctness gates (host, 2026-09-02)

| Gate | Result |
|---|---|
| Every moved pixel kernel (five conversions) byte-identical to the `rusty_esp_image-core::ops` copy over an LCG corpus: 64 rounds, one of them a 76 800-pixel QVGA frame, over 100 000 pixels checked per kernel | **pass** |
| Both downscales byte-identical to their copies over 48 geometries including odd widths and heights (the dropped edge) and a 320×240 | pass |
| `rms_dbfs_i16` bit-identical (`f32::to_bits`) to the `rusty_esp_audio-core` copy over 256 blocks up to a second of 16 kHz stereo, quiet blocks included | pass |
| `isqrt` equal to the `rusty_esp_signal-core` copy and to `floor(sqrt(v))` in f64 over 200 000 random `u32` plus the edges (`0`, `1`, `u32::MAX`) | pass |
| `hadamard_4x4` equal to `rusty_h264_common::transform::hadamard_4x4` on 20 000 blocks, one in five with coefficients to ±8 192 | pass |
| `satd_4x4_sum` equal to `rusty_h264_common::transform::satd_4x4_sum` for every block count 0..=41 (the reference sums four blocks at a time in SIMD lanes; the counts cover every remainder) and for 4 800 blocks (a QVGA frame) | pass |
| SAD 4×4 / 8×8 / 16×16 and `residual_4x4` against their definitions over 500 random positions in two strided 48×40 planes, and the residual's SATD through the reference | pass |
| `sample::pcm` conversion: the three rule tests that moved with it (integer widths, ties-to-even float rounding and clamping, same-format copy and the too-small error) | pass |
| `probe`: the plan's own example (a 6 % stage, 4× → 45 ‰), a slowdown buys nothing, equal-to-floor is under it, saturating counters | pass |

Unit tests: **16 pass**. Oracle tests: **7 pass** (`tests/moved.rs` 4,
`tests/h264_oracle.rs` 3). `cargo clippy --workspace --all-targets -D
warnings` clean; `cargo fmt --check` clean; `cargo deny check` clean (license
allowances not yet encountered are warnings); `riscv32imac-unknown-none-elf`
and `riscv32imafc-unknown-none-elf` compile with `--no-default-features` and
with `--features alloc`.

Consumers after the switch (each repo's own gates, same day): image 14,
audio 43 + 3 + 7 oracle (the ffmpeg `swresample` byte identity held), signal
77 + 3 capture-oracle, video 34 + 12, iroh 31 — the point of the D0 gate is
that those suites did not change, and they did not.

## D1 share table (host, 2026-09-02)

`bench/share.ps1 -Reps 31`: the `share` example built in release and run on
one core (affinity mask 4) at High priority; each kernel timed in-process,
best of 31 single calls, one whole frame or one whole second per call; the
null arm is the same kernel timed as arm A and arm B alternating (ABBA),
and its floor is the spread of the two best-of-31 minima. Four runs; the
table is run 4 (the committed example), the spread column is over all four.
Machine: the Janus development laptop, busy (Wi-Fi, IDE, the user's own
builds) — the standing condition, not an excuse.

**This is a share table, not a speed claim.** It exists so the ceiling probe
has a share to multiply. A host CPU says nothing about an S3's PIE, and 47 to
63 ms of process CPU time per run is far below the 15 s the CPU-time A/B
harness (`bench/pinvs.ps1`) demands — that harness is for whole-process arm
comparisons and was not used for these numbers.

### QVGA raw frame path (320×240 YUYV in), run 4

| kernel | work | best of 31 | per unit | share | share over 4 runs |
|---|---:|---:|---:|---:|---:|
| `yuyv_to_gray8` | 76 800 px | 0.016 ms | 0.2 ns/px | 2.5 % | 2.4 – 2.6 % |
| `yuyv_to_rgb565` | 76 800 px | 0.081 ms | 1.0 ns/px | 13.0 % | 12.2 – 13.0 % |
| `yuyv_to_rgb888` | 76 800 px | 0.349 ms | 4.5 ns/px | **56.0 %** | 54.6 – 56.0 % |
| `rgb565_to_rgb888` | 76 800 px | 0.035 ms | 0.5 ns/px | 5.6 % | 5.6 – 6.1 % |
| `rgb888_to_rgb565` | 76 800 px | 0.028 ms | 0.4 ns/px | 4.5 % | 4.5 – 5.0 % |
| `downscale2x_gray8` | 19 200 px out | 0.009 ms | 0.5 ns/px | 1.4 % | 1.4 – 1.5 % |
| `downscale2x_rgb565` | 19 200 px out | 0.032 ms | 1.7 ns/px | 5.2 % | 5.2 – 5.6 % |
| `sad_16x16`, whole frame vs a shifted copy | 300 blocks | 0.004 ms | 13.0 ns/block | 0.6 % | 0.6 – 0.7 % |
| `residual_4x4` + `satd_4x4_sum`, whole frame | 4 800 blocks | 0.069 ms | 14.4 ns/block | 11.1 % | 11.1 – 11.9 % |
| **path** | 422 400 px + 5 100 blocks | **0.622 ms** | | 100 % | 0.615 – 0.674 ms |

### One second of 16 kHz mono i16, run 4

| kernel | work | best of 31 | per unit | share | share over 4 runs |
|---|---:|---:|---:|---:|---:|
| `rms_dbfs_i16` | 16 000 samples | 0.003 ms | 0.2 ns/sample | 3.1 % | 3.1 – 3.3 % |
| `sum_sq_i16` | 16 000 samples | 0.003 ms | 0.2 ns/sample | 3.1 % | 3.0 – 3.3 % |
| `dot_i16` | 16 000 samples | 0.003 ms | 0.2 ns/sample | 3.1 % | 3.0 – 3.4 % |
| `pcm convert` I16 → F32 | 16 000 samples | 0.017 ms | 1.1 ns/sample | 15.3 % | 15.1 – 16.3 % |
| `pcm convert` F32 → I16 | 16 000 samples | 0.084 ms | 5.2 ns/sample | **75.5 %** | 73.8 – 75.8 % |
| **path** | 80 000 samples | **0.111 ms** | | 100 % | 0.104 – 0.118 ms |

### The floor, and what the probe says with it

| run | null floor (spread of best-of-31 minima, `yuyv_to_gray8` / `yuyv_to_rgb888`) | process CPU time |
|---|---:|---:|
| 1 | 0 ‰ | 63 ms |
| 2 | 0 ‰ | 63 ms |
| 3 | 1 ‰ | 47 ms |
| 4 | 7 ‰ | 47 ms |

Within a run the minima of two identical arms agree to under 1 % (best-of-N
finds the floor; codec-measurement 1). **Between runs the path moved 9.6 %**
(0.615 to 0.674 ms), so a twin is judged inside one run, arm against arm,
never against a table from another day.

What `probe::ceiling` says, with a 10 ‰ floor for a within-run judgement:

| candidate twin | share | speedup assumed | `pipeline_gain_permille` | verdict |
|---|---:|---:|---:|---|
| `yuyv_to_rgb888` (S3 PIE, 8 px per op) | 560 ‰ | 4× | 420 ‰ | **Build** — the raw path's whole story |
| `yuyv_to_rgb565` | 130 ‰ | 4× | 98 ‰ | Build |
| `satd_4x4_sum` | 111 ‰ | 3× | 74 ‰ | Build, but through `rusty_h264`'s seam (D3) |
| `downscale2x_rgb565` | 52 ‰ | 4× | 39 ‰ | Build, marginal |
| `sad_16x16` | 6 ‰ | 8× | 5 ‰ | **BelowFloor** — do not write it for this path |
| F32 → I16 convert (one `rintf` per sample) | 755 ‰ of the PCM second | 4× | 566 ‰ | Build — but the fix is likely algorithmic (a fixed-point round), priced before any PIE |

The verdict column is the plan for D2; the numbers that decide it are the
board's, not this table's.

## The first brick the table bought: rounding without libm (host, 2026-09-02)

The F32 → I16 conversion was 75 % of the PCM second because every sample
paid a `libm::rintf` call. `(v + 1.5·2^23) − 1.5·2^23` rounds an `f32` under
`2^22` to the nearest integer, ties to even, in two additions; the same in
`f64` with `1.5·2^52` for F32 → I32. The integer → float divisions by a
power of two became multiplications by the exact reciprocal.

| gate | result |
|---|---|
| **Counter (primary):** libm calls per second of 16 kHz F32 → I16 | 16 000 → **0** |
| Bit identity vs the libm twins over **every `f32`** (2^32 patterns, both conversions), `cargo test --release -- --ignored exhaustive` | **pass**, 15.65 s |
| Bit identity on the edges (NaN, ±inf, ±0, subnormals, ±1, the ties at 1.5/32768 and 2.5/32768, 32766.5/32768, −32767.5/32768) and 10 M LCG bit patterns, in CI | pass |
| Reciprocal multiply == division, every `i16` and 10 M `i32` | pass |
| The three conversion rule tests | pass |
| `rusty_esp_audio-esp`'s ffmpeg `swresample` byte-identity oracle, rerun on the patched crate | **7 pass** |

Clock (confirmation), `bench/share.ps1 -Reps 31`, two runs each side, same
session:

| kernel | before (runs 3, 4) | after (runs 5, 6) |
|---|---:|---:|
| `pcm convert` F32 → I16 | 5.6 / 5.2 ns per sample, 0.089 / 0.084 ms | **1.9 / 1.8 ns per sample, 0.030 / 0.029 ms** |
| `pcm convert` I16 → F32 | 1.1 / 1.1 ns per sample | 1.1 / 1.0 ns per sample — inside the floor; the multiply is a chip argument (the S3 FPU has no divide instruction), not a host one |
| null floor | 1 ‰ / 7 ‰ | 0 ‰ / 0 ‰ |

Method line: `pinned=mask4 prio=High metric=wall-ns-in-process pairs=31-best-of order=ABBA-null null_floor=0-7‰ work=16000 samples per arm`.
A 2.8× on the kernel, byte-identical; on the host the PCM second is now
led by the same conversion at about 53 % instead of 75 %, and the number
that matters is still the board's.

## D2 software half: the seam and the chip crate (host, 2026-09-02)

No speed number — nothing here is faster yet. These are the gates the
software half had to pass so the first twin lands in a crate that already
builds for its chip and is already held to the oracle.

| gate | result |
|---|---|
| `cargo test --workspace --features rusty_esp_dsp-esp/std,rusty_esp_dsp-esp/pie-s3` | **28 pass** (19 unit incl. 2 seam, 3 h264 oracle, 4 moved, 2 `-esp`), 1 ignored (exhaustive) |
| `twin_matches_scalar(&Scalar, …, 64)` and `(&PieS3, …, 32)` | 832 and 416 comparisons, all identical (13 per round: 7 pixel, 3 sample, 3 block) |
| `cargo clippy --workspace --all-targets -- -D warnings`, with no features, with `pie-s3` + `std`, and with `pie-p4` | clean (the first fleet run caught the no-feature build's unused imports in `-esp`; fixed the same evening, and the fleet gate is what runs it) |
| `cargo check -p rusty_esp_dsp-esp --no-default-features [--features pie-s3 / pie-p4] --target riscv32imafc-unknown-none-elf` | 3 of 3 pass |
| `RUSTUP_TOOLCHAIN=esp cargo check -p rusty_esp_dsp-esp --no-default-features --features pie-s3 --target xtensa-esp32s3-none-elf -Z build-std=core` | **pass**, 14 s (esp toolchain, `core` built from source; the first Xtensa bare-metal check in the family) |
| `cargo deny check` | clean (no new dependencies) |
| `unsafe` in `-esp` | none; `#![deny(unsafe_code)]` |

The gate's corpus, per round: an even width 2–80 and height 2–60 (so every
downscale and every YUYV pair is exercised), fresh LCG bytes per input, i16
vectors of 1–500 samples, 32×32 planes for the SADs, 1–9 Hadamard blocks.
The twin's return value **and** its output buffer are compared, so a twin
that gets the pixels right but the `Geometry` or the byte count wrong fails
too.

## Method line for every future row

`pinned=<core> prio=High metric=<cpu|wall> pairs=<N> order=ABBA null_floor=<‰> work=<pixels|samples|blocks per arm>`

A row without it is not a number.
