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

## A probe the table did not pay for: sharing the chroma in `yuyv_to_rgb888` (host, 2026-09-02, reverted)

The idea: a YUYV pair shares `(u, v)`, and `yuv_to_rgb` was called twice
per pair, so compute the three chroma terms once per pair (counter: chroma
computations per pair 2 → 1, same integer arithmetic, byte-identical — the
moved.rs oracle stayed green). The clock, `share` example built before and
after, run pinned (mask 4, High) in ABBA order, best-of-31 per kernel:

| run | `yuyv_to_rgb888` | `yuyv_to_rgb565` | null floor |
|---|---:|---:|---:|
| before, 1 | 0.357 ms · 4.6 ns/px | 0.080 ms | 17 ‰ |
| after, 1 | 0.376 ms · 4.9 ns/px | 0.085 ms | 1 ‰ |
| after, 2 | 0.362 ms · 4.7 ns/px | 0.084 ms | 4 ‰ |
| before, 2 | 0.346 ms · 4.5 ns/px | 0.079 ms | 5 ‰ |

Method line: `pinned=mask4 prio=High metric=wall-ns-in-process pairs=2×ABBA-runs
best-of-31 null_floor=1-17‰ work=76800 px per arm`.

**Reverted, measured worse-or-inside-the-drift**: both after runs sit above
both before runs (+4 % and +6 % on the two kernels), while the same binary
moved 3 % between its two runs. Either way there is nothing to keep: the
compiler was already sharing the chroma across the two inlined calls (the
multiplies were never the cost), and the restructure only changed codegen.
What the kernel spends its 4.5 ns/px on is the 4-in / 6-out byte layout
and three clamps per pixel — a layout question for the S3 twin (D2), not
an arithmetic one for the host. Two pairs are enough to refute a keep, not
to size an effect; no number above is a speed claim.

## Method line for every future row

`pinned=<core> prio=High metric=<cpu|wall> pairs=<N> order=ABBA null_floor=<‰> work=<pixels|samples|blocks per arm>`

A row without it is not a number.

## The transform oracle on rusty_h264-common 0.14 from crates.io (host, 2026-09-03)

`rusty_h264-common` moved from the `no-std` git branch (0.12) to the 0.14.0
release; the git allow-list row is gone. `tests/h264_oracle.rs` (the
Hadamard and SATD sums against the encoder's own transform) passes
unchanged: 3 of 3, and the workspace's 28.

## I5 on silicon: the same kernels on an ESP32-S3 (2026-09-06)

The D1 share table above says, in its own words, that "a host CPU says
nothing about an S3's PIE". This row is the S3 half. `firmware/xiao-s3-probe`
runs the same scalar kernels from this crate on a Seeed XIAO ESP32-S3 Sense
over Track B (esp-hal 1.2.0, no ESP-IDF), 160x120 frames out of a 220 KiB
heap, each kernel repeated until it has spent at least 100 ms so a fast one
is not measured against the timer.

Method line: `board=xiao-esp32s3-sense clock=80MHz opt-level=3 lto=fat
metric=in-process-us budget=100ms/kernel work=px-and-blocks-counted
pairs=1 null_floor=not-established`. One arm, no ABBA: this is a **cost
table for a chip that has no twin yet**, not an A/B, and nothing below is a
speed claim about anything but this part at this clock.

**At this clock** is load-bearing: the probe takes `Config::default()`, which
boots this part at 80 MHz, and a camera pipeline would run it at 240. Every
absolute figure below is therefore about 3x pessimistic. The **shares are
not** — every kernel scales with the same clock — and shares are what this
row exists to correct.

**The opt-level matters and was measured both ways.** The first run built at
`opt-level = "s"`; the host table it is compared against is speed-optimised,
and a size-optimised arm against a speed-optimised arm is not a comparison
(codec-measurement 4). Rebuilt at `3`, the two conversions that dominate the
host moved a long way and the rest barely moved:

| kernel | ps/unit at `"s"` | ps/unit at `3` |
|---|---:|---:|
| `yuyv_to_gray8` | 112 710 | 125 226 |
| `yuyv_to_rgb565` | 932 065 | **563 098** |
| `yuyv_to_rgb888` | 794 427 | **387 938** |
| `rgb565_to_rgb888` | 437 994 | 488 035 |
| `rgb888_to_rgb565` | 362 972 | 350 399 |
| `downscale2x_gray8` | 277 808 | 289 072 |
| `downscale2x_rgb565` | 1 716 875 | 1 779 253 |
| `sad_16x16` (per block) | 44 439 393 | 21 558 422 |

### The finding: the host's ranking does not survive the crossing

Holding the host's own work mix fixed (76 800 px per pixel kernel, 19 200 px
out per downscale, 300 blocks of SAD) and substituting the S3's per-unit
costs, both sides renormalised over the eight kernels the probe covers
(the host's `residual_4x4` + `satd_4x4_sum` has no S3 arm yet, so it is out
of both columns):

| kernel | host share | **S3 share** | S3 / host per unit |
|---|---:|---:|---:|
| `yuyv_to_gray8` | 2.9 % | 5.0 % | 602x |
| `yuyv_to_rgb565` | 14.6 % | **22.4 %** | 534x |
| `yuyv_to_rgb888` | **63.1 %** | 15.4 % | **85x** |
| `rgb565_to_rgb888` | 6.3 % | 19.4 % | 1071x |
| `rgb888_to_rgb565` | 5.1 % | 13.9 % | 961x |
| `downscale2x_gray8` | 1.6 % | 2.9 % | 617x |
| `downscale2x_rgb565` | 5.8 % | 17.7 % | 1068x |
| `sad_16x16` | 0.7 % | 3.4 % | 1617x |
| **path** | 0.553 ms | **193.2 ms** | |

**On the host one kernel is 63 % of the path; on the S3 the largest is 22 %
and four kernels sit between 14 % and 22 %.** There is no dominant kernel on
this silicon.

The last column says why, and it is the more useful half. The host is
between 85x and 1617x faster per unit — a **19x spread across kernels that
all do comparable per-pixel work**. `yuyv_to_rgb888` is the single kernel
where the host is under 500x, which is what a kernel the host's compiler
*failed* to vectorise looks like beside seven it vectorised. The D2 revert
above had already named the cause from the other side: that kernel's cost is
its 4-in / 6-out byte layout and three clamps per pixel. So the host's 56 %
was measuring the host's own vectorisation hole, and on a chip where nothing
is vectorised the ranking flattens.

### What this does to the ceiling probe

Re-running `probe::ceiling`'s arithmetic on the S3 shares, same 4x
assumption as the host table used:

| candidate twin | host `pipeline_gain_permille` | **S3** |
|---|---:|---:|
| `yuyv_to_rgb888` (PIE, 8 px per op) | 420 | **115** |
| `yuyv_to_rgb565` | 98 | **168** |
| `downscale2x_rgb565` | 39 | **133** |
| `rgb565_to_rgb888` | — | **146** |
| `sad_16x16` | 5 (**BelowFloor**) | 25 |

The twin D1 called "the raw path's whole story" is now the fourth-best of
five, and three kernels the host priced as marginal or did not price at all
are ahead of it. **Brick 12's target was chosen from the host number and
should be re-chosen from this one** — or, better, from a mix measured on a
real camera path rather than one-call-each, since every share above inherits
the host's synthetic mix.

### Memory, from the chip rather than the ELF

`esp_alloc::HEAP` at four stages, same run:

| stage | used | free | total |
|---|---:|---:|---:|
| boot | 0 | 225 280 | 225 280 |
| buffers | 172 800 | 52 480 | 225 280 |
| after kernels | 172 800 | 52 480 | 225 280 |
| end | 172 800 | 52 480 | 225 280 |

172 800 is exactly the five buffers asked for (19 200 px x 9 bytes), so the
allocator's accounting and the source agree, and **no kernel in this crate
allocates**: the figure does not move across eight kernels. That is a
property worth keeping — it is what lets these run under a fixed heap.

### What is NOT done here

No PIE twin exists, so **the "PIE ceiling probes" half of the I5 row is
still open**. Everything above is the scalar arm, which is the baseline a
PIE twin would be judged against; the ceiling table says which twin to write
first, and it is no longer the one the host chose.

## The allocator swap on silicon: rusty_alloc 2.0.1 against esp-alloc (2026-09-08)

`firmware/xiao-s3-probe` builds both allocators from **one source**, selected
by a cargo feature, so the dependency graph and every other line are
identical. Both arms are handed the same 220 KiB and run the same eight
kernels over the same five buffers.

Method line: `board=xiao-esp32s3-sense clock=80MHz opt-level=3 lto=fat
arms=2-one-source budget=225280-both metric=in-process-us budget_us=100000
work=px-and-blocks-counted pairs=1 null_floor=see-below`.

**Work parity.** Both arms allocate `19,200 x 2`, `38,400 x 2` and `57,600`
= **172,800 bytes live**, and neither frees before the end. Identical in both
arms, so the allocator is the only variable.

### The geometry, predicted then measured

The region is carved into whole segments and the remainder is stranded. From
the geometry alone, a 220 KiB region under the small profile should yield
3 x 64 KiB segments plus one 4 KiB backend page and strand the rest:

| | predicted | measured |
|---|---:|---:|
| region handed over | 225,280 | 225,280 |
| taken by the allocator | 200,704 | **200,704** |
| stranded, unusable | 24,576 | **24,576** |

**Exact, and it never moves** — `used` reads 200,704 at `buffers`,
`after_kernels` and `end` alike. So 10.9% of this budget is dead to a 64 KiB
granule, and the 172,800 live bytes occupy 87.9% of the 196,608 that the
segments do provide. Sizing the region as `k * 64 KiB + 4 KiB` is the fix and
is now upstream as `usable_bytes`.

### The kernels: four at six parts per million, four scattered

| kernel | esp-alloc | rusty_alloc | delta |
|---|---:|---:|---:|
| `yuyv_to_gray8` | 125,226 | 125,218 | **-0.006 %** |
| `downscale2x_gray8` | 289,072 | 289,086 | **+0.005 %** |
| `downscale2x_rgb565` | 1,779,253 | 1,779,322 | **+0.004 %** |
| `sad_16x16` (per block) | 21,558,422 | 21,558,635 | **+0.001 %** |
| `yuyv_to_rgb565` | 563,098 | 581,898 | +3.34 % |
| `yuyv_to_rgb888` | 387,938 | 406,726 | +4.84 % |
| `rgb888_to_rgb565` | 350,399 | 362,871 | +3.56 % |
| `rgb565_to_rgb888` | 488,035 | 450,468 | **-7.70 %** |

**The top four are the null arm, and they came for free.** Four kernels
reproduced to within six parts per million across a complete reflash, a
different allocator and two days. That is this instrument's floor, and it is
far below every difference in the bottom four — so those four are real
effects, not scatter.

**But they are not the allocator's speed.** The heap does not move once the
buffers exist: `used` is identical at all three stages, and I1 already
established that no kernel in this crate allocates. Zero allocator calls
happen during a measured kernel, so nothing the allocator does can be inside
these numbers.

**They are where the buffers landed.** The two allocators hand out different
addresses, and the four kernels that moved are exactly the four touching
`rgb888` (57,600 B) or `rgb565` (38,400 B) as source or destination; the four
that did not move touch only the two 19,200-byte buffers. The **mixed sign**
is the confirming detail: allocator overhead would push one way, and
`rgb565_to_rgb888` got 7.7 % *faster*. Alignment and bank placement do that.

So the honest summary is the one predicted before the run: **on a workload
that allocates five buffers once and never frees, the allocator swap is
invisible to compute.** What it does instead is shuffle buffer placement by up
to 8 % either way, which is a caution about comparing kernel numbers across
allocators rather than a result about either allocator.

### What it costs

| budget | delta | note |
|---|---:|---|
| flash (`.text` + `.rodata` + `.data`) | **+16,584 B** | +6.9 % of this firmware |
| static RAM (`.bss` + `.data`) | **+3,092 B** | comes straight out of `.stack` |
| region stranded by the granule | 24,576 B | recoverable by sizing to `k * 64 KiB + 4 KiB` |

`.stack` shrank by exactly 3,092 bytes, matching the static growth to the
byte, because the linker gives the stack whatever RAM is left. A firmware near
its stack limit does not get a bigger binary when it adopts this — it gets an
overflow. Decomposition, levers and the two floor functions are in
`rusty_alloc/docs/plans/firmware-code-size.md`.

**Why keep it, then.** Not for speed on this firmware, which the table above
says plainly. For the double-free abort: one block cannot be handed to two
owners. That is the trade being taken knowingly, and the cost is now measured
rather than assumed.

## rusty_alloc 2.0.2 verified, and the placement hypothesis settled (2026-09-09)

2.0.2 took the levers `rusty_alloc/docs/plans/firmware-code-size.md` proposed
and claims the firmware flash cost roughly halved. Re-measured here rather
than accepted, on this board, same firmware source, same 220 KiB budget, same
five buffers.

### The size claim holds

| | esp-alloc | 2.0.1 | **2.0.2** |
|---|---:|---:|---:|
| `.text` | 50,337 | 62,417 | **54,221** |
| `.rodata` | 7,724 | 10,124 | **9,828** |
| `.data` | 2,300 | 4,404 | **4,396** |
| **flash delta** | — | **+16,584** | **+8,084** |
| attributable symbols | 1,043 / 6 | 16,256 / 57 | **8,297 / 37** |
| static RAM delta | — | +3,092 | **+3,052** |

**-51.3 % of the flash cost.** Upstream quotes +7,860 against our +8,084; the
224-byte gap is this seam's own additions, not theirs — the three new `Error`
variants and their `Display` strings landed after their measurement. Their
attributable figure (8,262) and ours (8,297) agree to 35 bytes.

Every symbol the plan named is gone from the linked image: `adopt_segment`,
`drain_delayed`, `try_guarded`, `Random::refill`, and `init_thread_heap` —
the last being the one the plan explicitly refused to put a number on, now
replaced by a 966-byte `create_heap`. The `.stack` identity still holds to the
byte: static growth of 3,052 against `.stack` shrinking by exactly 3,052.

### The controlled experiment: it was placement, not code

The 2026-09-08 row found four of eight kernels moving 3.3 % to -7.7 % between
allocators, and argued from the mixed sign that this was where the buffers
landed rather than anything the allocator did. 2.0.2 tests that properly,
because it changes the allocator's **code** enormously while leaving its
**allocation behaviour** untouched:

| comparison | what changed | worst kernel delta |
|---|---|---:|
| esp-alloc to rusty 2.0.1 | code **and** addresses | **7.698 %** |
| rusty 2.0.1 to rusty 2.0.2 | code only, ~half of it removed | **0.006 %** |

**All eight kernels agree across 2.0.1 and 2.0.2 to within 0.006 %**, after
half the allocator's code was deleted. Two arms discriminate the two
explanations cleanly: if the allocator's code were inside those measurements,
removing 8 KB of it would move them, and it does not. The 7.7 % was buffer
placement.

That also makes this the third independent confirmation of the instrument's
floor at roughly six parts per million, now including a full reflash and a
different allocator build.

**The transferable caution stands and is now proven rather than argued:**
changing an allocator can move a compute benchmark by 8 % without executing a
single instruction inside the measured region. Kernel numbers are not
comparable across an allocator change.

### Geometry, unchanged

`used=200704 free=24576` at every stage, identical to 2.0.1 and to the
prediction: three 64 KiB segments plus the 4 KiB page, 24,576 bytes stranded
by the granule.
