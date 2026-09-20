
## rusty_alloc 2.0.5: the alignment gap is gone, and the RAM finally reconciles (2026-09-09)

2.0.5 takes the proposal in `region-alignment-dissolve.md`: on a fixed region
segments are carved at `SEGMENT_SIZE` strides from the region's **base**, and
`segment_of` masks the offset from that base instead of the address. `Region`
drops to 16-byte alignment, so the linker has nothing to pad.

### The gap, measured before and after

RAM is fixed, so the sections must sum to a constant. On 2.0.4 they did not,
and that 24,148-byte shortfall was the whole finding:

| build | `.data` | `.bss` | `.stack` | sum | unaccounted |
|---|---:|---:|---:|---:|---:|
| esp-alloc @196,608 | 2,300 | 196,700 | 136,368 | 335,368 | 0 |
| rusty 2.0.4, segment-aligned | 2,228 | 198,752 | 110,240 | 311,220 | **24,148** |
| **rusty 2.0.5** | 2,228 | 198,752 | **134,384** | **335,364** | **4** |

**The gap closed from 24,148 bytes to 4**, and `.stack` recovered **+24,144**
with `.bss` byte-identical. Both halves of the earlier finding are confirmed
by the fix: the memory was real, and `.bss` could never see it. A saving that
shows up only in the section *sum* is exactly the shape the last row said to
look for.

### Everything else held

| | |
|---|---|
| heap accounting | `used=196608 free=0` at every stage, whole segments |
| `.text` | 51,201 to 51,273, **+72 bytes** for the stride arithmetic |
| `good_region_size(220 KiB)` | still 196,608; the `const` assert still builds |
| host tests | 31 pass |

### What this consumer cannot measure, and will not quote

Upstream prices the change at **three instructions on every `free`** (39
against 36) and 9-17 ns per alloc/free pair. **That number is theirs and is
not reproduced here.** This firmware allocates five buffers once and never
frees, so it cannot see a per-free cost at all — the same reason it could
never price the allocator's hot path. It is the right trade on this part
regardless: RAM binds here and cycles do not, and `--cfg ra_aligned_region`
restores the old layout for a firmware that would rather have the mask.

### Kernels, and one more placement move

| kernel | 2.0.4 | 2.0.5 | delta |
|---|---:|---:|---:|
| `rgb888_to_rgb565` | 350,371 | 337,848 | **-3.6 %** |
| `sad_16x16` | 21,559,275 | 21,575,266 | +0.07 % |
| `downscale2x_gray8` | 289,086 | 289,086 | **0.000 %** |
| the other five | | | within 0.01 % |

The region's base moved, so the buffers did. One kernel gained 3.6 % and ran
16 repetitions instead of 15; one held bit-identical. Sixth confirmation, and
nothing here is a speed claim about either allocator.

### Where this leaves the swap

| | esp-alloc | rusty_alloc 2.0.5 |
|---|---:|---:|
| allocator code | 1,043 B | ~6,200 B |
| firmware flash | 60,361 B | 63,765 B (+5.6 %) |
| usable heap from its region | 196,608 | 196,608 |
| RAM lost to the allocator's shape | 0 | **4 B** |
| double free | free list corruption | abort |

From +16,584 B of flash, +3,092 B of stack and 24,576 stranded bytes at
2.0.1, to +3,404 B of flash and 4 bytes of RAM at 2.0.5. The footprint
argument against adopting it on this firmware has essentially gone; what
remains is the flash, and the reason to pay it is still the double-free abort
rather than speed on a workload of this shape.

## I5: the scalar kernels priced on the S3 (2026-09-18)

The `xiao-s3-probe` firmware (Track B, esp-hal, esp-alloc arm) ran the scalar
pixel and block kernels on a XIAO ESP32-S3 and reported per-kernel throughput.
Serial only. `PieS3` still `delegate_to_scalar!`, so these ARE the kernels the
chip runs today; the numbers are the ceiling a hand-written `ee.*` SIMD kernel
would have to beat.

| kernel | unit | ps/unit | throughput |
|---|---|---:|---:|
| yuyv_to_gray8 | px | 150,245 | 6.65 Mpx/s |
| rgb888_to_rgb565 | px | 350,399 | 2.85 Mpx/s |
| downscale2x_gray8 | px_out | 289,081 | 3.46 Mpx/s |
| yuyv_to_rgb888 | px | 375,442 | 2.66 Mpx/s |
| rgb565_to_rgb888 | px | 463,003 | 2.16 Mpx/s |
| yuyv_to_rgb565 | px | 563,093 | 1.78 Mpx/s |
| downscale2x_rgb565 | px_out | 1,779,218 | 0.56 Mpx/s |
| sad_16x16 | block | 21,577,185 | 46.4 k block/s |

Method: `esp-hal::time::Instant`, each kernel run in a loop until ≥ 100,000 µs
elapsed (so a fast kernel is not measured against timer resolution), one untimed
warm pass first; `ps_per_unit = us·1e6 / units`, integer arithmetic. esp-alloc
196,608 B heap, 160×120 frame. The rusty_alloc arm panicked on 2026-09-09 and is
excluded; esp-alloc is the baseline. The kernels' correctness is the scalar
oracle by construction (host `assert_eq!` tests); this row is their speed on the
silicon they are meant to price. A SIMD `ee.*` kernel is worth writing where the
row is slow and the pipeline share is real — `downscale2x_rgb565` and the SAD
are the fat ones.

## I6: the pixel kernels cracked open on the S3 (2026-09-19)

A `codec-vectorize-kernel` pass over the pixel and block kernels, measured
three ways: emitted asm from the crate build, disassembly of the FLASHED
firmware, and throughput on a XIAO ESP32-S3 read over serial (esp-alloc arm,
the I5 baseline). The three disagree, and the disagreements are the finding.

### What shipped

| change | flashed asm | on the S3 |
|---|---|---:|
| row slices + `chunks_exact` in both downscales | plain build −39 loop instrs, 9→6 guards; **LTO build byte-identical** | **0.00%** |
| keep RGB565 pixels PACKED, widen one channel at a time | loop 122 → 119, **bit-ops 71 → 56 (−21%)**, loads 20 → 18, spills 10 → 9 | **−7.7%** |

`downscale2x_rgb565`: **1,779,218 → 1,641,602 ps/px_out, 0.562 → 0.609
Mpx/s.** Byte-identity gated by `tests/moved.rs`, which holds the pre-move
implementations and demands identity over a generated corpus (26 tests green).

**Null arm, free and unusually clean:** the seven kernels neither change
touches reproduced across the two runs to 0.001% or exactly —
`yuyv_to_rgb565` 569,369 both times, `rgb888_to_rgb565` 337,861 both times,
`downscale2x_gray8` 289,075 both times, `sad_16x16` 21,573,560 → 21,573,347.
So the −7.7% is the change, not drift.

### Three instrument corrections, all of which changed a conclusion

1. **Censusing the wrong build.** `cargo build --release` on the crate and the
   firmware profile (`opt-level=3`, fat LTO, `codegen-units=1`) are different
   programs — **257 vs 164 instructions** for this kernel. The row-slicing
   restructure was worth −39 loop instructions in the first and produced a
   byte-identical function in the second. Only the flashed ELF predicts the chip.
2. **`s*` is not "store" on Xtensa.** Counting any `s`-prefixed mnemonic as a
   store swept in `srli`/`slli` and reported **30 stores in a loop that writes
   2 bytes**. That read as catastrophic spilling and aimed a whole change at a
   problem that did not exist. The real loop is 8 pixel loads, 2 stores, 9
   spill accesses and 56 bit-ops.
3. **A back-edge is not a loop.** The smallest span picked a 14-instruction
   slow-path loop over the 116-instruction pixel loop.

### What is left, and where it goes

The kernel is now 56 bit-ops and 427 cycles per output pixel (3.6 CPI) doing
twelve 5/6-bit channel widenings — a SIMD shape. **ESP32-S3 PIE (128-bit) is
reachable from Rust**, verified: under
`#![cfg_attr(target_arch = "xtensa", feature(asm_experimental_arch))]` an
`asm!` block emits `ee.zero.q` / `ee.vld.128.ip` / `ee.vadds.s8` /
`ee.vst.128.ip` for `xtensa-esp32s3-none-elf` on the `esp-198` toolchain.
Before a real twin: `ee.vld.128.ip` wants 16-byte alignment (unaligned is
`ee.ld.128.usar.ip` + `ee.src.q`) and `ee.vunzip.8` lane semantics need the
S3 TRM rather than a guess. `PieS3` still `delegate_to_scalar!`; a twin
overrides one method there and is gated by `seam`'s differential harness.

`sad_16x16` prices as 129 loop instructions, **33 loads, 0 bit-ops, 0
spills** — purely load-bound, matching the standing finding that SAD does not
reward wider SIMD. It is not the place to start.

Instrument and the full laws: `rusty-esp-embedded` §21 + its `xtensa_census.py`.

## I7: ten deterministic wins on the reduction kernels (2026-09-19)

Follow-on to I6, after extending `xiao-s3-probe` to measure the sample and
block reductions. Every change byte-identical, gated by `tests/moved.rs`
(pre-move implementations over a generated corpus) and `tests/h264_oracle.rs`
(rusty_h264-common's transform); 26 tests green throughout.

### Measured on the S3, over serial

| kernel | baseline | final | |
|---|---:|---:|---:|
| `yuyv_to_gray8` | 137,735 | **62,686** ps/px | **−54.5%** |
| `peak_abs_i16` | 151,379 | **88,988** ps/sample | **−41.2%** |
| `satd_4x4_sum` | 4,636,823 | **2,761,318** ps/block | **−40.4%** |
| `sum_sq_i16` | 240,203 | **168,840** ps/sample | **−29.7%** |
| `sum_sq_i16_le` | 289,020 | **220,590** ps/sample | **−23.7%** |
| `dot_i16` | 294,660 | **257,532** ps/sample | **−12.6%** |

Ten wins across three techniques, in descending yield:

1. **Break the loop-carried dependency** (4 wins). `acc += ...` over a slice
   makes every add wait on the previous one. Four independent accumulators —
   or four running maxima — are byte-identical for integer work and let the
   adds issue back to back: `peak_abs_i16` −43.3%, `sum_sq_i16` −25.0%,
   `sum_sq_i16_le` −20.2%, `dot_i16` −7.0%. **The instruction count barely
   moves; this is pure instruction-level parallelism.**
2. **Stop using 64-bit arithmetic for range that cannot be reached** (4 wins).
   Two i16s multiply to at most 2^30, exact in i32, so `i64 * i64` bought
   nothing and cost a multi-instruction 64×64 sequence: −4.2 to −8.7%.
   `satd_4x4`'s i64 accumulator had the same problem against a maximum of
   65,280 — **−29.4%, loop 313 → 217 instructions, spills 113 → 80.**
3. **Remove a round trip through memory** (1 win) and **amortise loop
   overhead** (1 win). Fusing the Hadamard into `satd_4x4` so the sixteen
   coefficients never reach an array: −15.7%, spills 80 → 51, stores 39 → 22.
   Unrolling `yuyv_to_gray8` 4× (a strided byte copy costing 33 cycles per
   output byte): −54.5%.

### The null arm was free, and it is what makes these verdicts

Every run measures sixteen kernels; the ones a change does not touch are its
null arm. At best they reproduced **exactly** — five kernels bit-for-bit
identical across the satd-fusion pair of runs — and the widest layout-driven
mover on any run was 4.9%. Each claimed win clears its run's band by 1.5× to
11×. Two changes were REFUTED on this evidence and reverted: rewriting
`u8::abs_diff` as `(i32 − i32).unsigned_abs()` produced a byte-identical
function (LLVM already lowers it that way), and row-slicing `residual_4x4`
could not be separated from a ±8% layout swing in its own run.

## I8: fifteen more kernel wins, and where each technique STOPS (2026-09-19)

Follow-on to I7, after extending the probe to 18 kernels (adding hadamard_4x4,
isqrt and both pcm::convert directions). Every change byte-identical; gated by
`tests/moved.rs`, `tests/h264_oracle.rs` and a widened `isqrt` sweep.

### Measured on the S3, over serial

| kernel | before | after | |
|---|---:|---:|---:|
| `pcm_i16_to_f32` | 659,301 | **348,279** | **−47.2%** |
| `isqrt` | 2,377,487 | **1,239,440** | **−47.9%** |
| `pcm_f32_to_i16` | 1,322,358 | **786,787** | **−40.5%** |
| `rgb565_to_rgb888` | 462,951 | **309,727** | **−33.1%** |
| `rgb888_to_rgb565` | 337,880 | **233,113** | **−31.0%** |
| `yuyv_to_rgb888` | 450,494 | **334,752** | **−25.7%** |
| `yuyv_to_rgb565` | 606,886 | **477,017** | **−21.4%** |
| `downscale2x_gray8` | 289,063 | **234,062** | **−19.0%** |
| `yuyv_to_gray8` | 62,686 | **50,964** | **−18.7%** |
| `peak_abs_i16` | 85,851 | **79,676** | **−7.2%** |

Four techniques: hoist a per-call dispatch out of a per-sample loop (pcm,
−47%/−40%); seed an iterative algorithm near its answer (isqrt's Newton
started at `v` and spent ~log2(v) DIVIDES halving down, −47.9%); process more
units per trip; break a loop-carried dependency.

### ★ Every technique has a measured STOPPING POINT, and it is per kernel

The optimum unroll width is not a constant and cannot be reasoned to:

| kernel | best width | what the next step up measured |
|---|---|---|
| `yuyv_to_gray8` | **16** | 8→16 still paid −7.1% |
| `rgb565_to_rgb888` | **8** | — |
| `rgb888_to_rgb565` | **8** | 4→8 only −0.7% |
| `yuyv_to_rgb888` | **4** | 8 measured **+1.9%** |
| `downscale2x_gray8` | **2** | 4 measured **+2.8%** |
| `downscale2x_rgb565` | **1** | 2 measured **+2.7%** |

The ordering tracks body size: widening pays while the trip is dominated by
loop overhead and costs registers once it is not. `downscale2x_rgb565` already
carries 56 bit-ops per output pixel, so it refuses at width two.

The same applies to dependency-breaking, which is NOT universal: four lanes
won −43% on `peak_abs_i16` and −25% on `sum_sq_i16`, and **lost** on `sad`
(+47.5% on 16×16, +12% on 8×8) because that loop is load-bound and already
unrolled. And lane count is bounded by the register file: eight `u16` maxima
pay (−7.2%), eight `i64` accumulators do not (`dot_i16` +6.6%, `sum_sq_i16`
+13.3%) because sixteen registers exceed the window.

### What LLVM had already done (five refutations, all 0.00% or worse)

Strength-reducing the downscale row bases; dropping `sum_sq_i16_le`'s running
count (it is `len / 2`); writing an unroll out explicitly instead of
`for k in 0..N`; lowering `u8::abs_diff` to a single `abs`; and folding the
565 bit-replication algebraically (`sum(r8) = 8·sum(v) + sum(v>>2)`, exact)
— which read **+4.6%** with the census agreeing, bit-ops 56 → **57**. Paper
op-count is not emitted op-count.

`#[cold]` on the `expect_len` error path changed no census anywhere and moved
`sad_16x16` **44%** on layout alone, its loop untouched at 129 instructions:
that kernel's ~400-byte loop is acutely cache-line sensitive, which is also
why it must be judged against same-run neighbours and never across builds.

## I9 — the audio element kernels (2026-09-19)

The elements in `rusty_esp_audio-core` were audited for ALLOCATION in the
codec-memory-copies pass and found clean. Nobody had asked the other question.
The probe grew from 19 kernels to 26 so they would be measured beside the DSP
ones, which makes every run carry its own null arm; the floors were **2.0%**
on the first flash and **0.01%** on each of the next three.

**Sixteen wins, byte-identical, against the pass baseline:**

| kernel | before | after | |
|---|---:|---:|---:|
| `gain_i16` | 560 807 | 203 450 | **−63.7%** |
| `mono_to_stereo` | 247 962 | 100 898 | **−59.3%** |
| `dc_block_mono` | 1 557 768 | 822 003 | **−47.2%** |
| `resample_16k_48k` | 4 126 993 | 2 207 318 | **−46.5%** |
| `mix_i16` | 385 505 | 222 825 | **−42.2%** |
| `biquad_mono` | 1 781 711 | 1 124 517 | **−36.9%** |
| `stereo_to_mono` | 385 524 | 261 556 | **−32.2%** |

ps per sample (per output frame for the resampler), ESP32-S3.

### The five techniques, and that four of them were already in this ledger

**A quantity is a property of the CALL, not of the sample.** `Gain` formed an
`i64` product for a range `|x| ≤ 32768, |q15| ≤ 65535` cannot reach (the
product is at most 2^31 − 2^15, so the rounding term still fits `i32`), and
hoisting that test out of the loop is −26.8%. `DcBlock` and `Biquad` re-read
and wrote back two and four state words **through the struct** every frame,
because the channel loop hid that `ch` is fixed for the block; resolving
`ch == 1` / `ch == 2` once holds the state in registers, −28.0% and −20.4%.
The resampler's `frame()` multiplied by a runtime frame size for a channel
loop that runs once: a mono arm is −30.8%.

**A clamp that cannot fire.** `Gain::apply` clamped to the `i32` range and
then saturated to `i16`. `i16` is a subset of `i32`, so the first clamp could
never change an outcome the second did not.

**A value computed twice in two units.** `libm::roundf(x)` IS
`truncf(x + copysignf(0.5 − 0.25·EPSILON, x))`, and the `as i16` after it
truncates toward zero as well — the float unit computed what the cast then
recomputed. Fusing them is −26.7% on `dc_block` and −20.7% on `biquad`. This
is an ALGEBRAIC claim about someone else's implementation, which a corpus can
only fail to refute, so it is gated by a sweep of all 2^32 `f32` bit patterns
(`tests/round_sat16_exhaustive.rs`, 13 s in release) rather than by samples.

**A 64-bit division is a libcall on this core.** `(rem << 32) / out_r` — and
`out_r` is a *reduced* rate ratio, so it fits 16 bits and long division in
base 2^16 gives the identical quotient from two 32-bit divides, which are
instructions: −12.7%. Carrying `idx`/`rem` instead of recomputing them
retires two MORE divisions per output frame and measured **−1.3% inside a
2.0% floor** — real work removed, unresolvable, not counted.

**Unroll width is per kernel and only measurement knows it.** `gain` won at 4
(−40.5%), again at 8 (−9.0%) and again at 16 (−8.5%). `mono_to_stereo` won at
4 (−54.3%) and at 8 (−10.9%). `mix_i16` won at 4 (−43.8%) and **lost at 8
(+23.4%)** — its body is the largest of the three movers, so it stops one step
earlier. Same law as I8, three more data points.

### Refuted, measured worse, reverted

`t.clamp(-32768.0, 32767.0) as i16` in `round_sat16`: identical for all 2^32
patterns, reads better, and cost `dc_block` **+12.1%** and `biquad` **+6.5%**.
Branching to a constant wins because the saturating cast does not fold away
after `clamp` the way it does after a range-proving branch.

Both reverts were **confirmed on the chip, not assumed**: `dc_block` returned
to 822 003 against the 822 060 it left, `biquad` to 1 124 517 against
1 124 618.

### ★ The largest lever here is still unspent, and it is an architecture question

**Every i16 access in every element is BYTE-WISE.** A census of the flashed
ELF reads `halfword ld/st = 0` in all five element loops, while the same
firmware emits 88 halfword ops — every one of them from the DSP kernels that
take `&[i16]`. `StereoToMono` spends **56 of its 72 loop instructions**
(24 byte ld/st, 8 `slli`, 8 `or`, 8 `sext`, 8 `srli`) marshalling bytes into
i16s and back, to do four adds and four shifts.

The cause is not the loops: `PcmBlock` carries `&[u8]`, `l16ui` needs 2-byte
alignment, and LLVM cannot prove a `&[u8]` has it. Closing it needs a safe
alignment-checked cast — a new dependency on a `forbid(unsafe_code)` crate —
or a change to the `Element` trait's buffer type. Both are decisions about
the architecture rather than optimisations of it, so this is recorded and
left alone.

## I10 — the seams next door (2026-09-19)

I9 said the audio elements were an untouched seam and found sixteen wins in
them. The same question asked of `rusty_esp_signal-core` and
`rusty_esp_image-core` found two more. The probe now runs **36 kernels** in
one binary, which is what makes small wins claimable: the null-arm floor read
4.6% on the flash that grew the binary by two crates and **0.01% or 0.00%** on
every flash after it.

**Eleven wins, byte-identical**, against this pass's baseline:

| kernel | before | after | |
|---|---:|---:|---:|
| `testpattern_rgb565` | 1 458 528 | 308 538 | **−78.8%** |
| `rotate90_gray8` | 138 358 | 59 806 | **−56.8%** |
| `ld2410_feed` | 718 372 | 433 673 | **−39.6%** |
| `csi_wander` | 13 029 625 | 10 029 346 | **−23.0%** |
| `csi_features` | 2 253 130 | 1 895 793 | **−15.9%** |
| `biquad_mono` | 1 125 673 | 1 032 878 | **−8.2%** |
| `agc_i16` | 1 296 280 | 1 194 643 | **−7.8%** |
| `mono_to_stereo` | 102 219 | 94 285 | **−7.0%** |
| `dc_block_mono` | 823 889 | 808 593 | **−1.9%** |

### ★★ The census found a spill that the SOURCE said was an optimisation

`compute_wander`'s inner loop was 34 instructions for two elements, and TEN
were `l32i.n a?, a1, N` — reloads off the stack pointer — plus a spill store.
The cause was a change made EARLIER IN THIS SAME PASS: two accumulators each
for the sum and the squares, to break the dependency chain. Four `u64`
accumulators is eight 32-bit registers, past the Xtensa window. Ledger I8
already contains this exact law — *eight `u16` maxima fit the window, eight
`i64` accumulators do not* — and it was broken anyway, because the split was
bundled with three other changes and never measured on its own. One
accumulator each: **−15.3%**.

**A technique that is a law in one kernel is a hypothesis in the next, and
bundling it with other changes is how it stops being tested.**

### ★ Same-build A/B — and the one probe that lied

Two leaf functions cannot be compared across builds here, so put BOTH arms in
one binary. That settled `isqrt` in a single flash: the restoring
square root (sixteen shift-compare-subtract steps, no division) is **+54.4%
worse** than Newton's four steps with a `quou` in each. Divide-free is not
free when the divide is an instruction. It is *correct* — an exhaustive sweep
of all 2^32 inputs agrees with Newton everywhere, in 99 s — so this is a
speed refutation, not a correctness one.

**But a same-build A/B is still one probe.** `rotate180`'s reversed-chunk
rewrite read **−9.1%** that way, and **identical to within 8 ps in the five
builds after it**. One probe said win, five said wash: the −9.1% was codegen
that did not survive two more crates entering the binary. The win is
**retracted**; the code stays for being byte-identical and clearer.

### The other findings

- **`TestPattern::grab` was an algebra problem wearing a loop.**
  `colour_at(x, y)` depends on `y` only through `y == marker_y`, so a frame is
  TWO distinct rows. It was also running two 64-bit divisions — libcalls —
  *per pixel*, and re-matching the pixel format per pixel including an
  `Unsupported` arm the constructor had already rejected. −78.8%.
- **A transpose cannot make both sides sequential, so make both LOCAL.**
  `rotate90_gray8` tiled 8×8: −52.5%. **T = 16 measured +422%** — a cliff, not
  a slope.
- **A no-op on the value is not a no-op on the code.**
  `n.min(MAX_SUBCARRIERS)` cannot change `n`, and it is what lets the compiler
  see `frame[sc]` is in bounds, retiring a compare and a panic branch that ran
  `W · n` times a frame. −3.0%.
- **The INPUT TYPE can be the proof.** `features` accumulates its variance in
  `u32` because `iq: &[i8]` bounds every amplitude at `isqrt(16·2·128²) = 724`.
  A `u64` accumulator here is `add.n` + `bltu` + a carry `mov` per element.
- **A state machine's phase is invariant for the run it is in.** `feed_slice`
  re-entered `step` for each of the 35 DATA bytes of a 45-byte report to move
  one byte each. −39.6%.
- Unroll widths again, per kernel: `mono_to_stereo` won at 16, `gain` **fell
  off a cliff at 32 (+115.3%)** having won at 16, `agc` was flat at 8.

### I10 addendum — `csi_wander` again, and an instrument that was lying

Asked to look once more at the pass's most expensive kernel. Two things were
hiding, and the second one invalidates numbers already recorded above.

**The ring is a SLIDING WINDOW and the reduction was rebuilding it.** Exactly
one frame leaves and one arrives per push, yet `compute_wander` recomputed
both totals across all `W` frames every time: `W · n` multiply-accumulates,
**2600** of them at W = 50 and 52 subcarriers, to fold in ONE new frame.
Carrying `sum` and `sumsq` in the detector and updating them from the frame
that changes is `n` updates instead of `W · n`.

| | before | after | |
|---|---:|---:|---:|
| `csi_wander` | 10 933 894 | 2 818 616 | **−74.2%** |
| reps in the same 100 ms | 176 | 683 | **3.9×** |

**And the arithmetic predicts it, which is what makes it a finding rather than
a number.** 2600 accumulate trips at 12 instructions plus 52 reductions at
~150 is ~39 000 per push; 52 updates at ~15 plus the same 52 reductions is
~8 600 — predicted −77.8%, measured −74.2%. What remains is `isqrt` and two
divides per subcarrier, i.e. the inherent work, and the census confirms **no
64-bit divide libcall survives**: dividing by the const generic `W` was
already strength-reduced.

**★★ The probe was measuring this kernel with its most expensive step switched
OFF.** It pushed ONE `Features` over and over, so every frame in the window
was identical, the population variance was exactly zero, `var_w2` was zero,
and `isqrt(0)` returns on its first line. **Every earlier `csi_wander` number
in I10 was taken that way, and the honest baseline is 9.0% higher.** The probe
now cycles four distinct frames and prints `CSIPROBE wander=192 warm=true`, so
a reader can see the reduction ran instead of short-circuiting — and that same
line is what confirmed this change byte-identical on the chip, 192 either
side.

The general form, which is not specific to CSI: **a degenerate input can make
a kernel skip its own hot path, and the probe will still report a number.**
Feed a reduction constant data and its variance term vanishes; feed a codec
a flat frame and its entropy coder idles; feed a filter silence and its
saturation never fires. **Print something the kernel COMPUTED, not just how
long it took** — a checksum, a verdict, a count — or the null arm and the
byte-identity gate will both pass over an arm that never ran.

One law confirmed twice in one pass: the accumulate loop had already been
made 12 instructions and spill-free, and it was still the wrong loop. **A
kernel you have optimised is not a kernel you have questioned.**

### I10 addendum 2 — the byte-wise i16 access, PRICED (2026-09-19)

I9 recorded this as "the largest lever left, and an architecture decision
rather than an optimisation", and then asked for the decision **without a
number attached**, which is the wrong order. This is the number.

A throwaway same-build A/B, written entirely inside the probe so no library
was touched and nothing had to be reverted: arm **A** is the shape the
elements ship today (`i16::from_le_bytes([b[0], b[1]])` over a `&[u8]`, which
is two `l8ui` plus a shift and an or), arm **B** is the identical loop, unroll
and arithmetic over an aligned `&[i16]` — which is exactly what a safe
alignment-checked cast hands you, with the check paid once per call.

| kernel | bytes (A) | aligned (B) | ceiling |
|---|---:|---:|---:|
| `stereo_to_mono` | 271 125 | 112 301 | **−58.6%** |
| `gain` | 188 550 | 98 704 | **−47.7%** |
| `mono_to_stereo` | 101 922 | 58 880 | **−42.2%** |

`ALIGNPROBE identical s2m=true gain=true m2s=true` — the arms were compared
byte for byte before either number was read, because a measurement whose arms
disagree is pricing two different kernels. Arm A tracks the shipping elements
in the same build (271 125 vs 255 407; 101 922 vs 94 645), so the twins are
representative; the shipping figures carry per-call overhead that will not
shrink, so the realistic whole-kernel win is somewhat under the loop ceiling.

**42–59%, on top of everything I9 and I10 already won.** That is larger than
this entire campaign's per-kernel wins, and it is one decision rather than
forty edits.

**The seam that costs least.** `rusty_esp_core`, `rusty_esp_dsp` and
`rusty_esp_audio-core` all carry `#![forbid(unsafe_code)]`, and audio-core's
own module docs list it as principle 5. So the cheapest shape is the one this
family already uses for the allocator: **put the decision in ONE crate.** A
checked cast in `rusty_esp_core::pcm` —

```rust
pub fn as_i16(b: &[u8]) -> Option<&[i16]>;       // None when misaligned
pub fn as_i16_mut(b: &mut [u8]) -> Option<&mut [i16]>;
```

— lets every element keep `forbid(unsafe_code)` *and* keep bytemuck out of its
own manifest, and puts the `#[cfg(target_endian = "little")]` gate in exactly
one place instead of in every kernel. Implemented with `bytemuck` it needs no
`unsafe` anywhere (bytemuck is pure Rust, `no_std`, and has no dependencies of
its own); implemented by hand it is one audited `align_to` behind a safe API.
Either way the elements gain a fast arm and keep the byte loop as the oracle
and the misaligned fallback — which is the same scalar-oracle shape every twin
in this family already has.

**Why it is still not taken here.** It is a dependency (or a lint relaxation)
in the foundation crate of the family, and that is the user's call, not an
optimiser's. What has changed is that it is now a call with a number on it.

### I11 — `StereoToMono` taken to the end (2026-09-19)

The first kernel to spend the alignment finding. **385 524 -> 97 139 ps/frame,
-74.8%**, null arm 0.00%, in four measured steps and byte-identical at every
one:

| | ps/frame | step |
|---|---:|---|
| as found | 385 524 | |
| byte-path unrolling (4, then 16) | 255 159 | **-33.8%** |
| the aligned arm at 8 a trip | 104 224 | **-59.2%** |
| the aligned arm at 16 | 97 139 | **-6.8%** |

**The ceiling probe predicted the outcome to 0.6 points.** It said -58.6% for
this kernel before a line of the real change was written; the shipped result
was -59.2%. A prediction that survives implementation is what separates a
finding from a number, and it is the second time in this campaign the
arithmetic has done that (the other was the sliding window, -77.8% predicted
against -74.2% measured).

**The seam.** `rusty_esp_core::pcm::as_i16` / `as_i16_mut` return
`Option<&[i16]>`: the alignment is established ONCE per call, and `None` means
"take the byte path". Every element keeps its byte loop as the oracle and as
what a misaligned or big-endian buffer gets -- the same shape a scalar kernel
keeps for its vector twin. One crate holds the decision, so no other manifest
changes and no other crate loses `forbid(unsafe_code)`.

The cost was honest and is recorded where it happened: `rusty_esp_core` went
from `forbid(unsafe_code)` to `deny` with `#[allow]` on exactly two functions,
six lines of `align_to` on a POD type. `bytemuck` would have needed no unsafe
at all and was declined only because that crate is the base of nine repos and
has zero dependencies; `lib.rs` records the trade and that reversing it is
three lines.

**The width is measured, not assumed** -- all five beside each other in ONE
binary, byte-identity checked before any was timed:

| frames/trip | 1 | 4 | 8 | **16** | 32 |
|---|---:|---:|---:|---:|---:|
| ps/frame | 249 207 | 112 723 | 104 224 | **97 332** | 178 832 |

Thirty-two falls off the register-window cliff exactly as `Gain` does at the
same width. **And the first column is the finding:** ONE frame a trip over
aligned samples (249 207) is no better than SIXTEEN frames a trip over bytes
(255 159). The access method and the unroll are worth about the same on this
kernel, and they COMPOSE -- which is why the byte-path work done earlier in
the campaign was not wasted when the aligned arm landed on top of it.

**What the remaining elements are worth.** The ceiling probe priced `gain` at
-47.7% and `mono_to_stereo` at -42.2% on the same basis, and `mix_i16`,
`DcBlock`, `Biquad`, `Agc`, `Convert` and `LinearResampler` all read i16
through the same byte path. None of them has been converted yet.

### I12 — the alignment seam spent across all seven remaining i16 kernels (2026-09-19)

`StereoToMono` (I11) proved the seam. These are the rest. Every one keeps its
byte loop as the oracle and as what a misaligned or big-endian buffer gets.
ESP32-S3, ~30 untouched kernels in the same build, **null arm 0.00% on every
flash**:

| kernel | before | after | |
|---|---:|---:|---:|
| `mix_i16` | 216 383 | 124 881 | **−42.3%** |
| `pcm_i16_to_f32` | 348 446 | 209 293 | **−39.9%** |
| `gain_i16` | 201 691 | 127 920 | **−36.6%** |
| `pcm_f32_to_i16` | 786 858 | 534 990 | **−32.0%** |
| `resample_16k_48k` | 2 207 759 | 1 691 237 | **−23.4%** |
| `dc_block_mono` | 807 576 | 721 604 | **−10.6%** |
| `biquad_mono` | 1 032 476 | 984 158 | **−4.7%** |
| `agc_i16` | 1 193 621 | 1 139 770 | **−4.5%** |

### ★★ The SPREAD is the finding: a lever is worth what the rest of the body is NOT

Same change, same seam, same target, **42.3% down to 4.5%**. A kernel whose
body is a load, an add and a store gets nearly everything from the access
method. One carrying an int-to-float convert, a multiply and `round_sat16`
gets almost nothing, because that float path is ~9× the cost and the two byte
loads are noise beside it.

That ratio is predictable *before* the edit, from the kernel's ps/unit next to
a kernel of known shape: `agc_i16` at 1 193 621 against `gain_i16` at 201 691
said the float path was ~9× everything else, and therefore that the alignment
lever could be worth at most ~10% there. **Rank conversions by that ratio
rather than converting in source order.**

### The f32 side is worse than the i16 side

`as_f32` / `as_f32_mut` joined `as_i16`, because a conversion pays the
marshalling twice: an `i16` out of a `&[u8]` is two byte loads, a shift and an
or; an `f32` is **four byte loads, three shifts and three ors**. That is why
`pcm_i16_to_f32` (−39.9%) beats `gain_i16` (−36.6%) despite doing strictly
more arithmetic.

### The surprise: `LinearResampler`, −23.4%

Its mono arm reads endpoints only when `idx` moves — a third of output frames
at 16k → 48k — so the *loads* looked like a small target. But it **STORES on
every output frame**, and that store was two byte writes. The store side is
what paid. Its loop is now a free function so the byte and aligned paths
cannot drift, and the Q32 phase division they share is one `frac_q32`.

### The gate that matters here

Every ordinary buffer is aligned, so a digest exercises only the FAST arm and
would pass over a broken fallback forever. `arms_agree` drives BOTH for every
converted element by sliding input and output one byte — twelve frame counts,
both channel counts, and six gains spanning the Q15 range either side of where
the product stops fitting `i32`. `convert` and `LinearResampler` get their own
because their output length is not their input's; the `f32 → i16` corpus
carries out-of-range values both ways and NaN, since `f32_to_i16` must
saturate all of them identically down either path.

### I13 — fifteen more, and where they came from (2026-09-19)

| # | kernel | change | |
|---|---|---|---:|
| 1 | `mono_to_stereo` | the element the conversion pass missed | **−35.9%** |
| 2 | `sum_sq_i16_le` | aligned i16 view | **−21.4%** |
| 3 | `dc_block_mono` | `to_int_unchecked` | **−18.0%** |
| 4 | `biquad_mono` | `to_int_unchecked` | **−16.9%** |
| 5 | `downscale2x_rgb565` | width 1 → 4 | **−14.9%** |
| 6 | `downscale2x_rgb565` | aligned u16 view | **−14.3%** |
| 7 | `rgb565_to_rgb888` | aligned u16 read | **−12.1%** |
| 8 | `mix_i16` | width 8 → 16 | **−10.0%** |
| 9 | `yuyv_to_rgb565` | aligned u16 write | **−9.5%** |
| 10 | `vad_i16` | via `sum_sq_i16_le` | **−9.3%** |
| 11 | `agc_i16` | `to_int_unchecked` | **−8.1%** |
| 12 | `rgb888_to_rgb565` | aligned u16 write | **−7.4%** |
| 13 | `mono_to_stereo` | width 16 → 32 | **−6.3%** |
| 14 | `agc_i16` | via `rms_dbfs_i16` | **−6.1%** |
| 15 | `rgb888_to_rgb565` | width 8 → 16 | **−4.0%** |

### ★★ Read the null arm's DISTRIBUTION, not only its widest mover

One flash read a 6.66% widest mover and would have voided four readings. The
distribution said otherwise: **median 0.00%, p90 0.40%, and 2 of 27 kernels
over 2%** — and both movers were `isqrt` and `jpeg_find_eoi`, tiny loops
already known to be layout-sensitive (`sad_16x16` once swung 44% from a
+1-instruction change elsewhere). Against a p90 of 0.40% a −9.5% is 24× the
resolving power.

**Quote the max AND the p90.** The max is the worst case and is what a single
straddling cache line does to one small loop; the p90 is what the instrument
can actually resolve. Reporting only the max discards real wins; reporting
only the p90 hides a build that genuinely moved.

### ★ Three kernels moving together from ONE change is the attribution

`round_sat_i16` is called once per sample by `DcBlock`, `Biquad` and `Agc`.
Changing it moved all three, same direction, against a 0.00% null arm — which
is a stronger statement than any one of them alone, because layout cannot
move three independent kernels the same way at once.

### ★ A saturating cast does not fold away just because you proved the range

`x as i16` on an `f32` is a SATURATING cast: it emits its own range and NaN
tests. The census found those still being emitted **after** two explicit
comparisons had already established the range — four float compares per
sample where two suffice. `to_int_unchecked` behind those same tests is
−18.0% / −16.9% / −8.1%.

The soundness is not an argument, it is a sweep: the exhaustive test calls the
SHIPPED function over all 2^32 `f32` patterns, so it proves the cast is never
handed a value it cannot represent. **A copy of the function in the test would
have proved nothing about the code that runs.**

### The widths all had to be RE-MEASURED after the access method changed

Every aligned body is smaller than the byte body whose stopping point it
inherited, so none of those stops transferred. `downscale2x_rgb565` moved from
1 to 4 (and 8 is +10.7%); `mix_i16` from 8 to 16; `mono_to_stereo` from 16 to
32; `rgb888_to_rgb565` from 8 to 16. Where a stop did NOT move, the number is
recorded next to the constant: `rgb565_to_rgb888` at 16 is **+39.6%** (its
three-byte destination store is the limit), `yuyv_to_rgb565` at 8 macropixels
is **+8.3%**, and `Gain` at 32 is **+100.5%** — the *same* cliff its byte arm
hits at the same width, so the register window sets that one, not the access.

### Refuted, measured, reverted

The `copysignf` bias replaced by a float select: exact for all 2^32 patterns,
and **+2.0%** in a same-build A/B. The sign mask is cheaper than a float
compare and select. It was priced ALONE only after being bundled once — where
it read +6.5% / +0.2% / +2.2% against a 9.1% null arm and said nothing.

## R1 — the reachability census, and what it says about I6-I13 (2026-09-19)

Deferred three times, finally run. Every optimised kernel traced from the 11
shipping firmware entry points. **Four of thirty-nine are production-
reachable, and ONE firmware does all the reaching.**

| | kernel | how |
|---|---|---|
| 1 | `sample::rms_dbfs_i16` | called directly by `xiao-s3-sense-idf-pdm-udp` |
| 2 | `sample::sum_sq_i16_le` | through `rms_dbfs_i16` |
| 3 | `DcBlock::process` | `Pipeline::new([&mut dc as &mut dyn Element])`, then `.process` |
| 4 | `EnergyVad` | **partial**: `judge()` runs; the optimised `Element::process` body does not |

The rest: **32 probe-only**, 2 test-only (`Convert`, `jpeg::probe`), and
`ops::crop` with **no caller at all** outside its own unit test.

Verified independently of the audit, because a claim this large should not
rest on one pass: **no production firmware's `Cargo.toml` names
`rusty_esp_dsp`**; `pdm-udp/main.rs` really does build the pipeline and call
`rms_dbfs_i16` at lines 114-116, 162 and 218; `c6-mesh-node` really does reach
its CSI helpers only as `let _ = install_csi;`; and `crop`'s only call sites
are lines 167 and 171 of its own `#[cfg(test)]`.

### ★★ A dependency edge is not a call edge, and `pub use` makes them identical in a manifest

`rusty_esp_image-core` and `rusty_esp_signal-core` both depend on
`rusty_esp_dsp`, and `cargo tree` shows the edge. Neither CALLS into it on a
production path: `ops.rs:14` is `pub use rusty_esp_dsp::pixel::{...}` and
`csi.rs:507` is `pub use rusty_esp_dsp::int::isqrt;`. A re-export consumes the
dependency and creates no call. **A manifest edge, a `cargo tree` row and a
successful build all look the same whether the callee runs or not** — which is
why this needed a census and not an argument.

### ★ `let _ = f;` compiles a function without calling it

`c6-mesh-node` names `install_csi`, `drive_station` and `read_ld2410` this
way, deliberately, to prove they compile for the chip. It forces
monomorphisation and creates no call edge. The entire CSI presence path —
`CsiFrame::features`, `PresenceDetector::push`, and `isqrt` beneath them —
hangs off that, so the most expensive kernel in the probe reaches production
through a function reference that is never invoked. **Grep for `let _ = ` on a
function item before concluding a path is live.**

### What this does and does not mean for I6-I13

It does NOT retire the work. `rusty_esp_dsp`'s own `lib.rs` says what it is:
the scalar kernel home, where a loop moves once two packages carry it or once
a chip-side speedup would be spent on it, and where the twin is written
against the scalar oracle. Kernels living there before a caller exists is the
stated design. And some of the probe-only set is **pre-positioned for known
planned work** rather than speculative: the CSI presence path is waiting on a
deployment trip, and the LD2410 is a sensor that exists.

It DOES resize the claims. Of everything I6-I13 measured, the wins that touch
a shipping path today are `sum_sq_i16_le` (−21.4%), `rms_dbfs_i16` through it,
and `DcBlock` (−47.2% across I9, then −18.0% more from `round_sat_i16`) —
which makes `DcBlock` the most valuable kernel in this tree by a wide margin,
and it was never treated as such. The −78.8% on a test-pattern generator, the
−74.2% on `csi_wander` and the −74.8% on `StereoToMono` are groundwork.

**Two structural causes, both fixable and neither an optimisation:**
(a) the video path ships `Passthrough` over a sensor that already emits JPEG,
so not one pixel byte is read end to end — every pixel and block kernel is
downstream of a decision made in the camera driver;
(b) the two crates above consume `rusty_esp_dsp` as re-export surface only.

## R2 — measuring the path that actually ships (2026-09-19)

R1 found that ONE firmware reaches any kernel, and that its per-block loop is

```rust
let out = pipeline.process(block, scratch)?;   // 1 stage: DcBlock
let level = rms_dbfs_i16(out.data);
vad.judge(level);
```

**`Pipeline` appeared ZERO times in this probe.** Every DcBlock number in
I9-I13 was taken by calling `dc.process()` directly; production does not.
The probe now carries `pipeline_dcblock` and `prod_audio_block`, which are the
shape that ships.

### The Pipeline wrapper is cheap: +2.8%, a null result worth having

`dc_block_mono` 593 317 vs `pipeline_dcblock` 609 765, same binary. The
per-stage format check, `max_output_bytes` call, `PcmBlock::new` and
scratch ping-pong cost **2.8%** over a direct call. That was worth measuring
and is not worth optimising.

### ★ Where the production block's time actually goes

Per block (256 samples), from three same-build arms:

| | ps/sample | share of the block |
|---|---:|---:|
| `prod_audio_block` (all of it) | 1 034 112 | 100% |
| `pipeline_dcblock` | 609 765 | 59% |
| `rms_dbfs_i16` + `judge` | ~424 347 | 41% |
| of which `sum_sq_i16_le` | 185 707 | 18% |
| **of which the f64 TAIL** | **~240 000** | **23%** |

`rms_dbfs_i16`'s tail is `acc as f64 / n as f64`, `libm::sqrt`,
`libm::log10`. **The ESP32-S3's FPU is single precision**, so every one of
those is a software routine — and they are 23% of the only audio block that
ships.

### The menu, priced — and the decision is NOT an optimiser's

| form | ps/sample | vs shipped | exactness |
|---|---:|---:|---|
| f64 (shipped) | 430 026 | — | bit-exact |
| f64 without the sqrt | 416 096 | **−3.2%** | 0.97% of points differ, max **5.8e-11 dB** |
| f32 throughout | 222 945 | **−48.2%** | 66% differ, max **1.53e-5 dB** |

**Dropping the software sqrt buys almost nothing.** The cost is f64
arithmetic generally — the `log10` and the divide — not the square root, so
the middle row is a bad trade: it breaks exactness for 3.2%.

Neither cheaper form is bit-identical, and `tests/moved.rs` pins this function
with `assert_eq!` against the version that moved out of `rusty_esp_audio-core`
— a deliberate exactness contract. **Taking the 48% means retiring that
contract**, which is a decision about what the D0 move test is for, not an
optimisation. Numbers are here so it can be made with one.

### ★★ A 190-point corpus said "bit-identical". 3.5 million points said one in 110.

The no-sqrt form reported max error **exactly 0** over the structured corpus —
which would have made it shippable under `assert_eq!`. Sweeping 3.46 M points
found **32 037 disagreements (0.9%)**, the first at `acc=1073741823, n=1`.

A corpus can only ever FAIL TO REFUTE a claim about rounding. Where the claim
is "these two float expressions produce identical bits", the corpus has to be
millions of points or it is not evidence. `tests/rms_tail_tradeoff.rs` keeps
both the sweep and the refutation.

## R3 — 4 of 39 to 20 of 39 (2026-09-19)

R1 found four production-reachable kernels and one firmware doing all the
reaching. This is what could honestly be wired, and what could not.

### Now reachable (20)

**Audio, through `xiao-s3-sense-idf-pdm-udp` (8):** `rms_dbfs_i16`,
`sum_sq_i16_le`, `sum_sq_i16`, `DcBlock`, `EnergyVad::judge`, **`Biquad`**,
**`Agc`**, **`peak_abs_i16`**. The pipeline was one stage; it is now the three
a voice front end actually has — offset out, rumble out, level up — and each
block reports its PEAK as well as its RMS, because RMS hides clipping and
make-up gain is where clipping appears.

**Image, through `xiao-s3-sense-idf-capture` (12):** all seven pixel kernels,
`rotate90_gray8`, `rotate180`, `crop`, `jpeg::probe`, `jpeg::find_eoi`.

### The single decision that unlocked twelve of them

`IdfCamera::init` rejected everything but JPEG, and `Mode` had only a
`jpeg()` constructor. **`grab` was already format-agnostic.** One format gate
and one assignment later the sensor can be asked for RGB565, YUYV422 or
GRAYSCALE, and the I1 bench can say what a preview path costs.

**When a whole category of kernels is unreachable, look for the ONE upstream
decision** — it was never forty missing call sites, it was a camera that could
only be asked for JPEG.

### `crop` had no caller anywhere, and `sum_sq_i16` had a twin

`crop` was called by nothing in the tree — not even the probe. And the aligned
arm added to `sum_sq_i16_le` in I13 was a hand-rolled copy of `sum_sq_i16`
three lines above it: same four accumulators, same `u32` square, same tail.
Calling it instead is **−0.1%**, i.e. free, and turns two loops into one.
**Check for the twin every time you add a fast arm.**

### Still unreachable (19), with the reason for each

| kernels | why |
|---|---|
| `sad_16x16`, `sad_8x8`, `satd_4x4(_sum)`, `residual_4x4`, `hadamard_4x4` | they are H.264 motion-estimation kernels and **`rusty_h264` is not in this tree** — they were written to match a codec that lives in another project |
| `isqrt`, `CsiFrame::features`, `PresenceDetector::push`, `ld2410::feed` | driven by the on-radio kill tests during a trip, by `c6-mesh-node`'s own design — not a defect |
| `Gain`, `MonoToStereo`, `StereoToMono`, `mix_i16`, `LinearResampler`, `Convert`, `pcm::convert` | no honest use in a mono 16 kHz PDM to UDP path |
| `EnergyVad::process` | would recompute the `rms_dbfs_i16` the firmware already has; calling `judge` directly is the correct choice |
| `dot_i16` | a correlation with no consumer |
| `TestPattern::grab` | its only non-test constructor is behind a cargo feature the sketch firmware disables |

**Neither remaining group is a wiring job.** Adding call sites to move this
number would be the same defect the census exists to find, pointed the other
way.

### Both firmwares were BUILT, neither was flashed

`cargo build --release` on the ESP-IDF target, exit 0 for both (964 KB and
837 KB), compiling the local sibling crates. Three environment blockers had
to go first, and the third is worth writing down: **a `python` on PATH that is
itself a virtualenv** makes `sys.prefix != sys.base_prefix`, and
`idf_tools.py` refuses whatever `VIRTUAL_ENV` says. Pointing PATH at the base
interpreter is the fix. (The others: an output path over 10 characters, fixed
with `CARGO_TARGET_DIR=C:/janus-*`; and a nested venv.)

Not flashed: one joins Wi-Fi and the operator is remote, and there is no
camera on this desk. The pixel arm is therefore 96x96, runs last with the
JPEG slot already dropped, and prints a reason and returns on every failure
path rather than disturbing the I1 numbers.

### R3 addendum — the six block kernels are ORACLES, and the census mislabelled them

R1 and R3 both filed `sad_16x16`, `sad_8x8`, `satd_4x4(_sum)`, `residual_4x4`
and `hadamard_4x4` under "no shipping caller", with the implication that this
is a gap. It is not. `block.rs` says what they are, and it says it in the
first paragraph:

> The S3 / P4 twins of these (D3) go upstream through `rusty_h264`'s accel
> seam; **this module is where their oracle lives.**

`Cargo.toml` carries `rusty_h264-common = "0.14"` as a dev-dependency and
`tests/h264_oracle.rs` demands byte-identity against its transform over a
generated corpus. These kernels exist to be the scalar reference that a
vector twin is gated against. **An oracle is correctly not on a hot path**;
wiring one into a firmware to move a reachability number would be a category
error, and would also be the exact defect the census exists to find, pointed
backwards.

**The denominator was wrong, not the numerator.** Of the 39:

| | |
|---|---|
| **20** | reachable and shipping |
| **6** | oracles by design — correctly unreachable |
| **4** | pre-positioned for a planned deployment (the CSI/LD2410 kill tests) |
| **9** | no honest consumer in any firmware that exists |

So the shippable set is **20 of 33**, and the nine are a question about the
API surface rather than about wiring: `Gain` (AGC covers it), `MonoToStereo`,
`StereoToMono`, `mix_i16` (a mono path), `LinearResampler`, `Convert`,
`pcm::convert` (the mic and the wire are both 16 kHz i16),
`EnergyVad::process` (would recompute the RMS the firmware already has), and
`dot_i16` (a correlation nothing correlates).

**The lesson for the census itself: classify a kernel's INTENT before
counting it.** "Production-reachable" is the right question for a kernel
meant to ship, and the wrong question for one deliberately kept as an oracle
or a fixture. A census that does not separate them reports a gap where there
is a design.

## P1 — the ESP32-S3 PIE twins, with semantics read off the silicon (2026-09-19)

D2 waited three sessions for "an S3 on the bench and the TRM". The bench was
here all along and **the TRM turned out not to be needed**: an instruction run
on known byte patterns reports its own semantics more exactly than prose can.

The probe loads two 16-byte patterns into q0/q1, runs one `ee.*`, and prints
q0, q1 AND q2 — because several PIE instructions write their operands in
place, and which ones do is exactly what is being measured.

### What the silicon said

| instruction | behaviour |
|---|---|
| `ee.vunzip.8 qa, qb` | `qa` <- the **even** bytes of `[qa‖qb]`, `qb` <- the **odd** ones, both in place |
| `ee.vzip.8 qa, qb` | the interleave — so **zipping with zero IS a zero-extending widen**, 16 bytes to two vectors of eight `u16` |
| `ee.vadds/vsubs.s8/.s16` | signed saturating, little-endian lanes |
| `ee.vmax/vmin.s8/.s16` | lane-wise, signed |
| `QACC` | **FOUR** lanes of 40 bits, **BIT-packed**: lane 1 starts at byte 5, not byte 4 |
| `ee.vmulas.s16.qacc` | accumulates the first **four** lanes only |

And by assembling candidates, what is NOT there: **q0-q7 only; no
`ee.vabs.*`; no unsigned max, min or subtract; no SAD instruction; no 16-bit
shift.** Every one of those absences changed a kernel's design.

### Four twins, every one byte-identical, same-binary A/B

| kernel | scalar | PIE | |
|---|---:|---:|---:|
| `yuyv_to_gray8` | 52 875 | 9 131 | **−82.7%** (5.8×) |
| `peak_abs_i16` | 82 915 | 16 288 | **−80.4%** (5.1×) |
| `sad_16x16` | 34 134 129 | 7 817 854 | **−77.1%** (4.4×) |
| `sad_8x8` | 11 732 871 | 4 916 568 | **−58.1%** |

**`yuyv_to_gray8` is the shape PIE was made for.** YUYV is `Y0 U Y1 V`, so the
luma bytes ARE the even indices, and one `ee.vunzip.8` turns 32 source bytes
into 16 luma bytes where the scalar kernel needs sixteen byte loads and
sixteen byte stores.

**`peak_abs_i16` needed care at exactly one input.** With no `ee.vabs.*` the
magnitude must be built, and negating via `ee.vsubs.s16` from zero SATURATES:
`-32768` negates to `32767` where the oracle reports `32768`. Carrying the
lane-wise MINIMUM beside the maximum settles it for one instruction a trip.

**The SAD twins are built entirely from signed ops**, because the unsigned
ones do not exist: widen with `ee.vzip.8` against zero, subtract in `s16`
where `0..=255` minus `0..=255` cannot saturate, and take the magnitude with
`ee.vmax.s16(d, 0 - d)` where `-255..=255` cannot saturate either. Each step
is exact rather than approximately right, which is why the gate passes.

### ★★ Read the semantics off the chip, not the manual

Two of the facts above would have been guessed wrong, and both would have
produced silently plausible results: that the accumulator holds four lanes
rather than eight, and that its lanes are **bit**-packed rather than
byte-aligned. A kernel written from either guess would have summed the wrong
products into the wrong places and still returned a number.

**The prober costs one firmware arm and settles a whole instruction set.**
Print every register the instruction could have touched, not just the one you
expect it to write.

### The alignment work was the enabling step

`ee.vld/vst.128` require 16-byte alignment, which was half of why this was
parked. Every twin checks it once and hands anything else to the oracle — the
same two-arm shape I13 put through the audio elements. And the probe's own
buffers come from a `Vec<u128>`, because a `Vec<u8>` does not promise it and
the twin would otherwise take its fallback silently and read FLAT.

## P2 — the PIE campaign closes at 10/10

Five more twins on top of P1's five, every one `identical=true` against its
scalar oracle in a same-build A/B on the S3.

| kernel | scalar ps/sample | PIE | |
|---|---:|---:|---:|
| `dot_i16` | 271,354 | 16,580 | **−93.9%** (16.4×) |
| `sum_sq_i16_le` | 187,340 | 16,019 | **−91.5%** (11.7×) |
| `mix_i16` | 108,364 | 19,105 | **−82.4%** (5.7×) |
| `rms_dbfs_i16` | 215,092 | 43,799 | **−79.6%** (4.9×) |
| `mono_to_stereo` | 55,259 | 26,626 | **−51.8%** (2.1×) |

Two of these are the SHIPPING path rather than a pre-positioned kernel:
`rms_dbfs_i16` is what the per-block loop of `xiao-s3-sense-idf-pdm-udp`
calls after `Pipeline::process`, and it is a wrapper — a `sum_sq_i16_le` and
an f64 tail — so the twin reuses the tail character for character and is
bit-identical by construction.

### ★★ ACCX is FORTY bits, and `sum_sq_i16` was right by accident

P1 composed `rur.accx_1` and `rur.accx_0` into a `u64` and gated
`identical=true`, which reads like proof that the accumulator is 64 bits
wide. It is not. `dot_i16` — the first kernel here whose result can be
negative — failed its gate with the LOW word exactly correct (1174948039,
which is `-3120019257 mod 2^32`) and the high word reading 2559.

A direct probe on accumulators of known value settled it in one flash:

```
accumulator  -1  ->  accx_0=0xffffffff  accx_1=0x000000ff
accumulator  +1  ->  accx_0=0x00000001  accx_1=0x00000000
```

ACCX is 40 bits and `rur.accx_1` delivers bits 32..=39 **zero**-extended.
Composing the halves as a `u64` is exact only while the accumulator is
non-negative, which is why a sum of squares got away with it and a dot
product did not. The general lesson is the sharper half: **a gate that
passes tells you the kernel is right on the DOMAIN YOU FED IT, and a
non-negative corpus cannot ask a signed question.** The A/B for `dot_i16`
therefore builds its second operand to OPPOSE the first, so the running
total crosses zero and the answer is negative — `negative=true` is printed
beside `identical=` precisely so a future corpus change cannot quietly
retire the test.

40 bits is also what bounds the flush batch: 16 instructions × 8 lanes = 128
products of at most 2^30 peaks at 2^37, leaving 4× headroom.

### The two cheap instructions

**`ee.vadds.s16` IS `mix_i16`.** Eight lanes of saturating add, clamped to
the i16 range, one instruction, against the scalar arm's widen-add-compare-
compare-narrow per sample.

**`ee.vzip.16` against a second load of the SAME address is mono→stereo.**
Zip interleaves two registers; zipping a register with itself duplicates
every lane. Eight samples become sixteen in five instructions. It is the
smallest win of the ten (−51.8%) because the scalar arm was already only a
load and two stores — there was less to remove.

### Two process notes worth more than either kernel

**`str.replace(old, new, 1)` patched the wrong function.** The sign-extension
fix was written against a line that `sum_sq_i16` and `dot_i16` shared
verbatim, and the first occurrence is `sum_sq_i16`. The tell was §10's: the
re-flashed board returned a bit-for-bit identical wrong answer, which is the
question "is my binary even rebuilding?" It also produced a free reading —
`sum_sq_i16` gated `identical=true` WITH the sign-extending composition
applied, which prices the change at zero where it does not matter.

**A dependency appended to the end of a `Cargo.toml` lands under `[lints]`.**
`libm` for the dBFS tail went in with a blind `>>` and became a lint key. It
did not fail the build; it would have failed to be a dependency.

## P3 — ten more, and the alignment check was hiding most of them

| kernel | scalar | PIE | |
|---|---:|---:|---:|
| `sum_sq_i16_le` unaligned | 187,453 | 15,392 | **−91.8%** |
| `sum_sq_i16` unaligned | 166,830 | 14,279 | **−91.4%** |
| `dot_i16` unaligned | 262,536 | 26,389 | **−89.9%** |
| `peak_abs_i16` unaligned | 76,672 | 13,735 | **−82.1%** |
| `downscale2x_gray8` | 235,696 | 46,192 | **−80.4%** |
| `rms_dbfs_i16` unaligned | 215,282 | 43,272 | **−79.9%** |
| `yuyv_to_gray8` unaligned src | 45,500 | 12,276 | **−73.0%** |
| `sad_16x16` unaligned | 34,045,268 | 9,468,093 | **−72.2%** |
| `stereo_to_mono` | 96,008 | 37,962 | **−60.5%** |
| `sad_8x8` unaligned | 11,569,412 | 5,279,206 | **−54.4%** |

### ★★ Eight of the ten were work the instrument could not see

P1 and P2 each opened by asking "which kernel next?". The better question
turned out to be "which PATH is not being measured?" Every twin ended its
preamble with `!aligned16(p) => return the_oracle(...)`, so a slice one
sample in got no SIMD at all — and nothing anywhere reported that, because
the gate still read `identical=true` and the A/B still read a number. It
read the SCALAR number twice.

That is the reachability defect from `codec-vectorize-kernel/REACHABILITY.md`
in its purest form, committed by me, in code I had written that week. The
kernel was not slow. It was **invisible**: work that never runs cannot
appear in a profile of the work that does.

**`sad_16x16` is where it cost the most.** Its aligned arm requires
`stride % 16 == 0` AND both bases 16-byte aligned. A current block at a
macroblock boundary usually qualifies; a reference block at a candidate
motion vector essentially never does. So the half of block matching that
a search actually spends its time in was taking the oracle every time, and
the −77.1% recorded in P1 was measured on the one case that is not the hot
one.

**The guard that makes this reading honest is one `println!`.** Each new
A/B prints the offset and stride beside its verdict — `off_a=3 off_b=5
stride=17`. A buffer that turned out to be aligned after all would have
measured the aligned path and read as a win that was not there. Do not
gate an alignment-sensitive kernel without printing the alignment.

### The unaligned idiom

    ee.ld.128.usar.ip   loads the 16-byte-ALIGNED BLOCK containing the
                        address, and sets SAR_BYTE from its low four bits
    ee.src.q            funnels the register pair by SAR_BYTE

Read off the part: offset 3 yields bytes 3..=18, offset 7 yields 7..=22.
The loop unrolls by two so the block loaded for one window is the low half
of the next and the two block registers swap roles — no register move,
which this unit does not appear to have. Two streams at DIFFERENT offsets
work with no extra cost, because every load sets SAR_BYTE and each
`ee.src.q` sits immediately after its own stream's load.

**It costs a tail, and the tail is a safety property, not a tuning knob.**
Producing the last window reads the aligned block containing its last byte,
which can end 15 bytes past it. Reading beyond a slice is undefined
behaviour whatever the silicon does, so every unaligned body stops short —
8 samples for the reductions, 16 pixels for the luma gather, 32 bytes of
slack for the block kernels — and the scalar loop finishes.

**The asymmetry that decides which kernels can take it:** there is no
unaligned STORE. `ee.vst.128` requires alignment and the alternatives cost
a lane extract and a byte store each. So a kernel whose SOURCE alone is
misaligned is reachable — `yuyv_to_gray8` gains −73.0% with a misaligned
frame and its own aligned output buffer — and a kernel whose destination is
misaligned still belongs to the oracle.

### The two that were ordinary kernel work

`stereo_to_mono` would have been silently wrong the obvious way. `(l + r) >> 1`
through `ee.vadds.s16` clamps two near-full-scale samples to 32767 and
reports half the right answer — **only on loud input**, which a quiet test
corpus never reaches. The sum has to happen in 32-bit lanes.

`downscale2x_gray8` has no 16-bit shift available, so its sums widen to
32-bit lanes to divide by four, against ZERO rather than a sign mask since
a sum of four bytes is never negative. Sixteen output pixels a trip so the
tail is a full 16-byte store. The round term 2 is BUILT in registers rather
than loaded (`ee.vcmp.eq.s16 q,q,q` is all-ones, `0 - (-1)` is one, one
left shift makes two), which avoids depending on `ee.vldbc.32` — an
instruction this module has still not measured and therefore does not use.

### Also measured, and recorded before anything needs them

`ee.vmul.s16` **wraps** rather than saturates (32767 x 3 reads 32765), so
it cannot stand in anywhere the scalar clamps — which is why `Gain` has no
twin yet. `ee.srcmb.s16.qacc` **truncates** rather than rounds (6, 10, 14,
18 shifted right by two come back 1, 2, 3, 4). `ee.vld.l.64.ip` and
`ee.vld.h.64.ip` load one half and PRESERVE the other. `ee.movi.32.q`
inserts a general register into a chosen lane.

### Ruled OUT, with the reason

**RGB565 <-> RGB888 cannot be done on this unit.** Three bytes a pixel needs
a 3-way deinterleave and PIE has only 2-way `zip`/`unzip` with no general
byte permute. This is a refutation about the INSTRUCTION SET, not about the
kernel, and it does not expire when the surrounding code changes.

## P4 — twelve more, and the 4x4 block family REFUTED

| kernel | scalar | PIE | |
|---|---:|---:|---:|
| `convert_i32_to_i24in32` | 477,487 | 39,042 | **−91.8%** |
| `convert_i32_to_i24in32` unaligned | 479,685 | 48,889 | **−89.8%** |
| `convert_i16_to_i32` | 178,978 | 26,238 | **−85.3%** |
| `convert_i16_to_i32` unaligned | 179,335 | 30,334 | **−83.1%** |
| `convert_i32_to_i16` | 162,852 | 24,577 | **−84.9%** |
| `convert_i32_to_i16` unaligned | 164,975 | 33,291 | **−79.8%** |
| `gain_i16` | 134,920 | 24,297 | **−82.0%** |
| `gain_i16` unaligned | 134,939 | 32,912 | **−75.6%** |
| `mix_i16` unaligned | 111,154 | 33,903 | **−69.5%** |
| `mono_to_stereo` unaligned | 65,415 | 31,325 | **−52.1%** |
| `stereo_to_mono` unaligned | 99,812 | 49,235 | **−50.7%** |
| `downscale2x_gray8` unaligned | 225,563 | 135,181 | **−40.1%** |

### ★★ REFUTED: the 4x4 block family does not pay on this unit

Three kernels built, gated `identical=true`, and every one measured WORSE.
Recording per §12 which kind of revert this is: **measured worse, not inside
the noise.**

| kernel | scalar | PIE | |
|---|---:|---:|---:|
| `residual_4x4` | 4,417,156 | 5,203,715 | **+17.8%** |
| `satd_4x4` | 4,827,934 | 6,056,077 | **+25.4%** |
| `satd_4x4_sum` | 2,611,044 | 4,000,359 | **+53.2%** |

**Four bytes a row is a poor fit for a sixteen-byte register**, and the
unaligned funnel spends three instructions to deliver four useful bytes.
`residual_4x4` said so first. The obvious rescue was arithmetic density —
`satd_4x4` does about six times the work per loaded byte, two butterfly
passes and sixteen magnitudes and a reduction — and it did NOT rescue it.
`satd_4x4_sum`, which amortises the per-call setup over 64 blocks, was worse
still, which says the cost is not per-call overhead either.

That is three probes varied along the axis that could have flipped the answer
(low density, high density, amortised), so §11 is satisfied and this is a
real refutation rather than one measurement.

**The mechanism that is left is the transpose.** A 4x4 i32 transpose has no
shuffle instruction on this unit: two `ee.vzip.32`s pair the lanes and the
64-bit halves must be recombined THROUGH MEMORY with `ee.vld.l.64` /
`ee.vld.h.64`. Four stores immediately followed by eight loads of the same
addresses is a store-to-load hazard on an in-order core, and it sits in the
middle of the dependency chain where nothing can hide it.

**This refutation is provisional, not permanent** (§12): it would expire if a
register-level 64-bit half-swap were found. `ee.src.q` with SAR_BYTE = 8
funnels a pair by eight bytes and might serve — SAR_BYTE is settable only by
`ee.ld.128.usar.ip` from an address, so it would take a dummy load from an
address ≡ 8 (mod 16). Untried.

### The seam that IS exhausted

Every kernel in this module now has an unaligned arm where one is possible:
the twelve reductions and element-wise kernels, both SADs, the luma gather
and the gray downscale. The only kernels without one are those whose
DESTINATION can be misaligned, and that is not a gap — `ee.vst.128` requires
alignment and the alternatives cost a lane extract and a byte store each.

### rotate90_gray8, and the two that the numbers themselves asked for

| kernel | scalar | PIE | |
|---|---:|---:|---:|
| `rotate90_gray8` | 201,137 | 9,332 | **−95.4%** (21.6×) |
| `sum_sq_i16` widened | 14,960 | 10,005 | **−33.1%** |
| `peak_abs_i16` widened | 16,288 | 11,005 | **−32.4%** |

`rotate90_gray8` is the campaign's largest margin. Two things did it. The
destination column is REVERSED (`h - 1 - y`) and this unit has no byte
reverse — but loading a tile's eight source rows BOTTOM-TO-TOP emits the
transposed bytes in exactly the order the destination run wants, so the term
never appears in the kernel. And the 8×8 byte transpose is eight
instructions: `ee.vzip.8/16/32` are a perfect shuffle across the register
PAIR, which is precisely a transpose's three stages at 1-, 2- and 4-byte
granularity.

Note it against the 4×4 refutation above. That family also needed a
transpose and lost — because a 4×4 **i32** transpose must recombine 64-bit
halves through memory. The 8×8 **byte** transpose never touches memory
beyond the loads and stores it would need anyway. **The refutation was about
the memory round trip, not about transposes.**

### ★★ The last two wins were sitting in the results table, unread

| | aligned | unaligned |
|---|---:|---:|
| `sum_sq_i16` | 14,960 | **14,279** |
| `peak_abs_i16` | 16,288 | **13,735** |

The unaligned arm does THREE MORE instructions per window — two loads and a
funnel where the aligned arm does one load — and it was **faster, in both
kernels.** That cannot be true of the loads, so it was never about the
loads: the unaligned arms unroll to sixteen samples a trip and the aligned
arms only did eight, so the aligned loops paid `addi`+`bnez` twice as often.
At eight samples `sum_sq`'s body was one load, one MAC and two instructions
of loop — **fifty per cent overhead**.

Widening both aligned arms to sixteen samples a trip: −33.1% and −32.4%,
and the ordering inverts back to aligned-faster-than-unaligned, which is the
check that the anomaly is actually resolved rather than merely moved.

**Two numbers in a table I had already published, in a comparison I had
already made.** §7 says an impossible number is the instrument asking for
help; this is the softer version — a number that is merely *backwards*, sat
in the ledger for two passes before anyone read the two columns against each
other. Worth a standing habit: when the same kernel has two arms, compare
them to EACH OTHER, not only each to its own scalar.

The remaining aligned arms (`mix_i16`, `gain_i16`, the three converts) are
still at eight samples a trip and are candidates for the same treatment —
not claimed here, because none has been measured.

## P5 — ten wins from unroll width alone, and one kernel that refused it

Not one new kernel. Every win here is the same edit — the trip was too
narrow and the body was mostly LOOP — applied to arms that P4's §2b finding
said to look at.

| kernel | before | after | | instr/elem |
|---|---:|---:|---:|---|
| `convert_i32_to_i24in32` | 39,042 | 24,873 | **−36.3%** | 1.25 → 0.875 |
| `dot_i16` | 16,580 | 11,745 | **−29.2%** | 0.625 → 0.5 |
| `yuyv_to_gray8` | 9,131 | 6,791 | **−25.6%** | ~0.44 → 0.31 |
| `mix_i16` | 19,105 | 14,708 | **−23.0%** | 0.75 → 0.625 |
| `convert_i32_to_i16` | 24,577 | 20,278 | **−17.5%** | 0.75 → 0.625 |
| `convert_i16_to_i32` | 26,238 | 22,088 | **−15.8%** | 0.875 → 0.75 |
| `mono_to_stereo` | 26,626 | 22,913 | **−13.9%** | 0.875 → 0.75 |
| `convert_i32_to_i24in32` unal. | 48,889 | 43,286 | **−11.5%** | 1.75 → 1.375 |
| `mix_i16` unaligned | 33,903 | 30,060 | **−11.3%** | 1.25 → 1.125 |
| `dot_i16` unaligned | 26,389 | 23,830 | **−9.7%** | 1.125 → 1.0 |

**These before/after pairs are CROSS-BUILD, which §12 says not to headline
on its own.** So the instr/elem column is there as the second instrument:
it is counted from the source, is exact, and needs no board. The two agree
everywhere — predicted −10% measured −11.3%, predicted −14% measured −13.9%,
predicted −17% measured −17.5%. Where they differ they differ in our favour
(−20% predicted, −29.2% measured for `dot_i16`), which is what you expect
when removing a loop-carried branch also helps the pipeline.

`yuyv_to_gray8` was the worst shape of all and not because of its width:
its loop was in **Rust**, around a four-instruction `asm!` block, so the
block was re-entered every sixteen pixels and paid the Rust counter and
branch on TOP of its own four instructions. Moving the loop inside the asm
and going to 32 pixels a trip is the whole −25.6%.

### ★★ REFUTED: `gain_i16` gets WORSE when you widen it

Same edit, eleventh kernel, and the only one that refused. Widened to
sixteen it read 27,329 against the eight-wide arm's 24,297 — reverted.

**The instruction count went DOWN and the time went UP.** Predicted −12.5%
on instructions per sample (1.0 → 0.875); measured worse. That can only be a
stall, and the mechanism is named: **QACC is a single resource.** The body is

    ee.zero.qacc -> vmulas -> vmulas -> ee.srcmb

a chain, and the second half's `ee.zero.qacc` cannot start until the first
half's `ee.srcmb` has read the accumulator. Widening doubles the serial
chain to save two instructions of loop.

The transferable form: **unrolling pays when the bodies are INDEPENDENT.**
Every other kernel here works in q-registers, which are plentiful and
renamable by hand — two loads and two adds are two independent chains.
`gain_i16` works through the one accumulator, so its two halves are one
chain, and instruction count stops predicting time. Count instructions to
find the candidates; measure to keep them.

## P6 — eight smaller wins, three refutations, and the well running dry

| kernel | before | after | |
|---|---:|---:|---:|
| `yuyv_to_gray8` unaligned | 12,276 | 10,784 | **−12.2%** |
| `convert_i32_to_i16` unaligned | 33,291 | 29,524 | **−11.3%** |
| `mono_to_stereo` unaligned | 31,325 | 29,256 | **−6.6%** |
| `convert_i16_to_i32` unaligned | 30,334 | 28,459 | **−6.2%** |
| `sad_8x8` aligned | 4,916,568 | 4,616,638 | **−6.1%** |
| `sum_sq_i16` unaligned | 14,310 | 13,901 | −2.9% |
| `peak_abs_i16` unaligned | 13,796 | 13,436 | −2.6% |
| `sad_16x16` aligned | 7,817,854 | 7,655,006 | −2.1% |

`ee.vld.128.xp` loads AND advances by a register-valued stride, which is
what a row walk wants — it replaces the load plus the `add` after it, on
both streams, in both SADs.

### ★★ The kernels have crossed from INSTRUCTION-bound to LOAD-bound

Every widening in P5 landed within a point or two of what its
instructions-per-element predicted. In P6 the same edit under-delivers by
three to five times:

| | predicted | measured |
|---|---:|---:|
| `sum_sq_i16` unaligned 16→32 | −12.5% | −2.9% |
| `peak_abs_i16` unaligned 16→32 | −10% | −2.6% |
| `sad_16x16` `.xp` | −11% | −2.1% |

The three that fell shortest are the three that do the MOST loads per byte
of arithmetic — the unaligned reductions run two loads and a funnel per
sixteen bytes, and SAD reads two streams. **Instructions are no longer the
binding constraint on these loops; the load ports are.** That is the whole
reason this entry is smaller than the last one, and it predicts where the
remaining headroom is: kernels with HIGH ARITHMETIC INTENSITY, not more
unrolling.

### ★★ REFUTED: the FUSED load-op forms are slower

The assembler accepts a family nothing here had used, and the part confirmed
their semantics exactly:

    ee.vmulas.s16.accx.ld.ip q0, a, 16, q1, q2   ACCX += q1·q2 AND q0 <- [a], a += 16
    ee.vadds.s16.ld.incp     q0, a, q3, q1, q2   q3 = q1+q2  AND q0 <- [a], a += 16

One instruction doing a full 128-bit load and an eight-lane operation is
strictly fewer instructions. All three kernels rewritten with them got
**slower**:

    dot_i16    11,745 -> 13,833   +17.8%
    mix_i16    14,708 -> 24,212   +64.6%
    sum_sq_i16 10,005 -> 11,027   +10%   (and doing LESS work -- see below)

Reverted. This is the `gain_i16` law at ISA scale: **instruction count is
not cycles.** These forms do not reduce the number of LOADS, which is what
these loops are actually limited by, and they appear to cost more than the
two instructions they replace.

One process note, because it nearly became a wrong number. `sum_sq` and
`dot` run their asm once per accumulator-flush batch with the pointer as
`inout`, and the fused prologue loads one block AHEAD — so the asm left the
pointer sixteen bytes past the last block processed, and every batch after
the first silently SKIPPED a block. The gate caught it (`identical=false`,
and the PIE sum was SMALLER, which is what skipping looks like). Had the
kernel been one that happened to still agree, the timing would have been
recorded for a loop doing less work than it claimed.

### ★★ REFUTED AGAIN, by a second mechanism: `satd_4x4`

P4 refuted this kernel at +25.4% and named its transpose's memory round trip
as the cause, recording that the refutation **would expire** if a
register-level 64-bit half-swap were found. One was:
`ee.src.q q, x, x` at SAR_BYTE = 8 is `x.hi64 ++ x.lo64`, measured
`[08..0f, 00..07]`, and SAR_BYTE is settable by any `ee.ld.128.usar.ip`
from an address congruent to 8 (mod 16).

Rebuilt with the whole transpose in eight register instructions and no
memory traffic at all:

    satd_4x4       4,640,155 -> 4,992,012   +7.6%   (was +25.4%)
    satd_4x4_sum   2,558,459 -> 3,108,225  +21.5%   (was +53.2%)

**The memory round trip was most of the cost and removing it was not
enough.** The refutation now rests on two independently-tested mechanisms
rather than one, which is what §11 asks of a refutation worth keeping.

### REFUTED: removing a host loop is not always a win

Moving `rotate90_gray8`'s tile loop into the asm — the edit that made
`yuyv_to_gray8` 25.6% faster — made this kernel **9.3% slower** (10,199
against 9,332). The reason is the opposite of a saving: Rust recomputed both
cursors from `x0` and `y0` each tile, so a tile's first load depended on
nothing the previous tile did. An in-asm loop must walk them incrementally,
so the next tile's first load waits on the previous tile's last store.

**Removing a host loop wins when the address is already a running cursor,
and loses when the host was computing an INDEPENDENT address each trip.**
Redundant-looking arithmetic can be breaking a dependency chain.

## P7 — the FULL fused-op census, and which families actually pay

P6 refuted "fused load-op instructions" on three kernels. That was one
FAMILY, not the feature. The complete list settles it.

### Reading the ISA table instead of guessing mnemonics

`xtensa-esp32s3-elf-as` is a 900 KB wrapper; the instruction set lives in
`<toolchain>/lib/xtensa_esp32s3.so`, loaded through `xtensa-dynconfig`.
Extracting printable strings from THAT gives the authoritative table:

    217 ee.* mnemonics, 108 of them carrying a fused load or store.

(`strings` is not installed here, and it exits 0 printing nothing — the
first two sweeps read "0 matches" and meant "no such command". A count of
zero deserves the same suspicion as an impossible one.)

The 108 are about a dozen FAMILIES; the rest are s8/u8/s16/u16/s32 variants.

| family | verdict |
|---|---|
| `v{adds,subs,max,min,mul}.*.ld.incp` (ALU+load) | **REFUTED**, P6: +10% to +64.6% |
| `v{adds,subs,max,min,mul}.*.st.incp` (ALU+store) | **REFUTED**: `mix_i16` +57% |
| `vmulas.*.{accx,qacc}.ld.{ip,xp}` (MAC+load) | **REFUTED**, P6 |
| **`src.q.ld.{ip,xp}` (funnel+load)** | **PAYS — four wins below** |
| `ldqa.*` / `mov.*.qacc` (load/move to QACC) | ruled out: not widening loads |
| `srs.accx` (ACCX → general register) | works, but 32-bit; our accumulators reach 2^37 |
| `ldf/stf.{64,128}` (float load/store) | no float-SIMD arithmetic exists to pair with |
| `cmul.s16.*`, `fft.*` | no kernel of that shape here yet |

### `ee.src.q.ld.ip` is the one that pays

| kernel | before | after | |
|---|---:|---:|---:|
| `yuyv_to_gray8` unaligned | 10,784 | 8,291 | **−23.1%** |
| `convert_i32_to_i24in32` unaligned | 43,286 | 34,548 | **−20.2%** |
| `peak_abs_i16` unaligned | 13,436 | 11,047 | **−17.8%** |
| `sum_sq_i16` unaligned | 13,901 | 11,993 | **−13.7%** |

Measured semantics: `ee.src.q.ld.ip qu, as, imm, qx, qy` puts
`funnel(qx, qy)` in **qx** and `[as]` in **qu**, `as += imm`. It is not an
ALU fusion at all — it is the funnel unit plus a PLAIN load replacing the
slower `ee.ld.128.usar.ip`, since SAR_BYTE only has to be set once per
stream. Different hardware path, opposite verdict from the ALU family.

Three of them close a register rotation: from `(q0,q1) = (block k, k+1)`
they leave `(block k+3, k+4)`, with the windows landing in q0, q1, q2 in
turn — so a trip is three windows, and the window register is free to be
clobbered because the next rotation step overwrites it anyway.

**It cannot serve two streams at different offsets.** SAR_BYTE is global and
the fused load does not update it, so `dot_i16`, `mix_i16` and both SADs are
structurally excluded from this form.

### ★★ A two-variable gate separates where it pays

Counting the per-window body — `x` non-fused instructions, `s` of them
stores:

| kernel | x | s | x+s | result |
|---|---:|---:|---:|---:|
| `sum_sq` | 1 | 0 | 1 | −13.7% |
| `peak_abs` | 2 | 0 | 2 | −17.8% |
| `convert_i32_to_i24in32` | 2 | 1 | 3 | −20.2% |
| `gain` | 5 | 1 | 6 | +5.1% |
| `convert_i16_to_i32` | 4 | 2 | 6 | +37.5% |
| `mono_to_stereo` | 4 | 2 | 6 | +52.8% |

**`x + s ≤ 3` pays; `x + s = 6` costs**, with nothing observed between.
Stores carry double weight, which is why a 2-instruction body with a store
wins and a 5-instruction body with a store does not. The gate made one
out-of-sample prediction, `yuyv_to_gray8` at x+s ≈ 2, and it came in at
−23.1%.

### The second prediction's test was INVALID, and is recorded as open

`convert_i32_to_i16` unaligned, also x+s ≈ 2, read **+31.4%** — and that
number is not admissible. Moving the trip from 16 samples to 24 changed the
scalar TAIL on its 239-sample test buffer from 15 samples to 23. At the
scalar arm's ~165,000 ps/sample those eight extra samples add ~5,500
ps/sample by themselves, which is more than the whole apparent regression.
The two arms did not do the same work (§4).

`yuyv_to_gray8` escaped the same trap only by luck: its body length made the
reservation land on 2016 pixels at BOTH trip widths, so its tail was
identical and its number is clean. **When a change alters the vector/scalar
split, the per-element average is measuring the split, not the change.**
Re-test with a buffer length that is a multiple of both trip widths.


## P8 — B3 delivered: the start-code scan, and the guard that would have made it unreachable (2026-09-20)

Backlog B3 had a measured twin and no caller. Wiring it took one seam in
`rusty_esp_video-core` — and found, on the way, that the kernel as written
could not have served that caller at all.

### The defect: an alignment guard against a caller that cannot align

`find_start_code3` began `if !aligned16(bytes.as_ptr()) { return the_oracle }`.
Its one intended caller is `annexb::NalSpans`, which scans
`&stream[pos..]` where `pos` is wherever the previous NAL ended — an
arbitrary offset, essentially never a multiple of sixteen.

So the twin would have returned the correct answer from the scalar arm on
nearly every real call, while `identical=true` and every host test passed.
**This is the third time in this campaign that an alignment precondition has
been the reachability defect** (P2 found eight wins behind the same shape),
and the first time it was caught before the number was banked rather than
after.

The fix is the prefix walk: `align_offset(16)` gives the distance to the
first boundary, at most fifteen bytes are tested scalar-ly — reading two past
each index, so a code straddling the boundary is found there and not lost
between the arms — and the vector scan takes the rest.

### Measured on the S3, over serial, null arm max 0.02% across 91 kernels

| arm | ps/byte | vs its oracle |
|---|---:|---:|
| `nalscan_scalar` | 119,820 | — |
| `nalscan_pie` (aligned base) | 8,089 | **−93.2%**, 14.8× |
| `nalscan_pie_unaligned` (base+1, the production shape) | 8,540 | **−92.9%**, 14.0× |
| `b3_oracle_walk` (`NalSpans`-shaped walk, scalar scan) | 138,622 | — |
| `b3_twin_direct` (same walk, twin) | 16,032 | −88.4% |
| `b3_seam_nal_spans` (**the production call site**) | 18,861 | **−86.4%**, 7.35× |

The unaligned arm is the one that matters: at 8,540 against 8,089 the prefix
costs 5.6%, and against the oracle's 119,820 it is still 14.0×. Had the
guard survived, that row would have read ~119,820 and nothing else in the
table would have changed.

**The seam sits 17.6% above the bare twin, not 7× above it.** That gap is
`NalSpans`' own bookkeeping — the backwards walk over leading zeros, the
`NalSpan` construction, the access-unit bookkeeping — and it is what a wired
seam is supposed to look like. An unwired one reads like `b3_oracle_walk`.

Work-count parity checked, not assumed: `PIEB3SEAM nals_seam=8
nals_oracle=8 planted=8 agree=true` (§4).

### Gated twice, because the chip gate cannot run on a laptop

`pie_s3` is `cfg(target_arch = "xtensa")`, so a host test cannot call the
twin. Two gates instead:

- **On chip**, `all_offsets_identical=true` — the twin against the scalar
  scan at offsets 1, 2, 3, 7, 8, 15, 16, 17, which is every distinct
  `align_offset` case. This is the real gate.
- **On host**, `rusty_esp_dsp-esp/tests/start_code_model.rs` carries the same
  control flow with the one vector step replaced by a scalar equivalent and
  fuzzes it against a naive scan over 128,000 cases at every alignment, plus
  a planted straddle at all 96 positions. That gates the prefix walk, the
  block skip, the two-past-the-end read and the tail — everything that is not
  the assembly.
- And in `rusty_esp_video-core`, `the_split_scan_matches_the_scan_it_replaced`
  fuzzes `find_start_code` against a verbatim copy of its pre-split body, at
  every offset into every buffer, so the split itself is gated by the thing
  it replaced rather than by a restatement of itself.

### One more build-graph reachability bug, found by looking

`rusty_esp_image` and `rusty_esp_audio` both consume `rusty_esp_dsp-esp`
behind `pie-s3`, and neither repo's generated `.cargo/config.toml` patched
it — only the scalar half. `cargo check -p rusty_esp_image-core --features
pie-s3` was resolving the chip half to **the published crate at origin/main**,
which has none of this campaign in it. It compiled only because the twin
calls sit behind `cfg(target_arch = "xtensa")` and a host build never
references them.

`tools/gen-sibling-patches.py` now lists both halves for image, video and
audio. **Patch every crate of a sibling, not the one you happened to need
first** — the same law the probe manifest already carried in a comment, in a
place the generator could not see.


## R4 — the f64 tail retired, and the gate replaced rather than deleted (2026-09-20)

`rms_dbfs_i16`'s float tail was `f64`. The ESP32-S3's FPU is **single
precision**, so its divide, `sqrt` and `log10` were all software routines,
and R2 priced them at ~48% of the one audio block that ships.

That was never a precision decision. It was a desktop habit that nobody
questioned, paid for in the hottest audio code on the chip, for digits no
consumer reads — a VAD threshold is whole dB and a log line prints one
decimal.

### What blocked it was a gate pinning the wrong property

`tests/moved.rs` is the **D0** gate: "every kernel that moved here is
byte-identical to the copy it replaced." That is a *refactor* gate — it
proves a move was faithful. Applied to a function whose implementation
should still be free to change, it had quietly become a *design* gate,
freezing an arbitrary arithmetic choice forever.

So the answer was not to delete an assertion. It was to state the contract
that actually matters and prove it **harder** than the one it replaces.

### The tail takes one number, so its domain is enumerable

`20·log10(√m / 32768)` is `10·log10(m) − 20·log10(32768)`, which drops the
square root outright. The whole tail is then one `f32` in, one out — and
`tests/dbfs_tail_exhaustive.rs` walks **every input it can ever receive**:

```
EXHAUSTIVE: 520,093,697 points over [2⁻³², 2³⁰]
  max |err| = 1.526e-5 dB, at the domain floor
```

17.9 seconds, calling the SHIPPED function. Not a corpus — R2 itself records
a 190-point corpus reporting a max error of exactly zero for a form that
3.46 M points later showed disagreeing on 0.9% of cases. **A sample can only
fail to refute a rounding claim. An enumeration settles it.**

The domain floor is `2⁻³²`, which allows `n` up to 4.29e9 samples in one
block. Below it lie the subnormals, where both candidate forms read 3.05e-5
dB; excluding them is not cherry-picking, because `acc as f32 / n as f32`
with integer `acc ≥ 1` would need `n` near 1e38 to reach one.

The error is proportional to the magnitude of the result, so 1.526e-5 dB is
the figure at about −186 dBFS and it is nearer 4e-6 dB across normal levels.

### Measured on the S3, null arm median 0.00% / p90 0.02% over 113 kernels

**Eight kernels moved together, and every one of them computes a level.**
That is attribution layout cannot fake:

| kernel | before | after | |
|---|---:|---:|---:|
| `vad_i16` | 282,765 | 71,875 | **−74.6%** |
| `rms_pie` | 38,280 | 12,678 | **−66.9%** |
| `audio_rms` (the seam) | 38,312 | 12,709 | **−66.8%** |
| `rms_un_pie` | 41,054 | 15,361 | **−62.6%** |
| **`prod_audio_block`** | **869,244** | **641,606** | **−26.2%** |
| `agc_i16` | 841,028 | 630,090 | −25.1% |
| `rms_scalar` | 215,166 | 189,621 | −11.9% |
| `rms_un_scalar` | 215,299 | 189,664 | −11.9% |

`prod_audio_block` is the shipping number: **−26.2% on the only audio block
that ships.**

★ The spread says the I12 law again. The same change is −66.9% on the PIE
path and −11.9% on the scalar one, because on the PIE path the sum of
squares is already vector and the tail WAS the function, while on the scalar
path the sum still dominates. **A lever is worth what the rest of the body
is not.**

### And it removed a duplicate

`pie_s3.rs` carried a byte-for-byte copy of the tail. Both now call
`rusty_esp_dsp::sample::dbfs_from_mean_square`, so the twin and its oracle
cannot drift apart — two places for one contract to rot in was the real
defect, and the pin had been protecting it.

---

## R5 — the fused funnel was never the problem, and a do-while that could run 4 billion times (2026-09-20)

### The open question, closed

Ledger I8 recorded `convert_i32_to_i16`'s fused-funnel arm at **+31.4%** and
refused to call it a refutation: the trip had gone 16 → 24 on a 239-sample
buffer, which moved the scalar TAIL from 15 samples to 23, and at ~165,000
ps/sample those eight samples were worth more than the whole apparent
regression (§4).

Re-measured at **n = 200**, where `n − UNALIGNED_TAIL = 192` divides by both
16 and 24 so both arms leave an identical 8-sample tail:

| arm | ps/sample | |
|---|---:|---:|
| 16-wide, `ee.ld.128.usar.ip` + `ee.src.q` | 29,148 | — |
| 24-wide, `ee.src.q.ld.ip` | **24,642** | **−15.5%** |

`identical=true`, `fused_body=192`. Reproduced at −17.7% and −21.3% in two
other builds. **The instruction was never the problem; the buffer length
was.** I8's refusal to bank the number was correct.

### But the width is not free, and shipping it needed a work count

A wider trip strands a wider remainder. At n = 239 the 24-wide vectorises
216 samples and the 16-wide vectorises 224 — and eight extra samples at the
scalar rate cost more than the faster kernel saves over the other 216.
Chaining the arms does not rescue it either: the 23 samples the 24-wide
leaves are one short of the 24 the 16-wide needs.

So `convert_i32_to_i16` now runs **whichever arm vectorises more**, via a
shared `unaligned_body(n, w)` so the caller computes exactly what the arms
will do rather than a paraphrase that can drift. No cost model is needed:
the vector rate is strictly below the scalar rate, so more vectorised
samples is never worse, and at equal counts the faster kernel wins.

### ★★ The fault that found a latent bug in five kernels

Chaining the arms crashed the board with *"Detected a write to the stack
guard value on ProCpu"*. The cause is general and was sitting in the tree:

> Every unaligned arm's loop is a **do-while** — `bnez` tests after the
> body. Its early guard bounds `n`, not `body`, and the two are not the same
> condition. `convert_i32_to_i16_unaligned_src` passes `n >= 16` and still
> computes `body == 0` for every n in 16..=23. `left = 0`, `addi` makes it
> −1, `bnez` is true, and the loop runs about four billion times writing
> through memory.

`downscale2x_gray8_unaligned_row` already had `if body == 0 { return 0; }`.
**Five others did not** — `sum_sq_i16_unaligned`, `stereo_to_mono_i16_-
unaligned_src`, `convert_i16_to_i32_unaligned_src`,
`convert_i32_to_i16_unaligned_src` and `convert_i32_to_i24in32_unaligned_src`
— and each has a reachable window of input lengths that triggers it. All six
are guarded now.

It is reachable from a short buffer and no gate in this campaign could see
it: byte-identity never ran, because the board faulted first.

### ★ `#[inline(never)]` is not a free way out of a link error

The probe's `main` went past the `l32r` literal range again. Marking the new
kernel `#[inline(never)]` fixes that — and costs the kernel **+23.6%**
(24,081 → 29,769). The firmware shrank its own `main` instead. **A test
binary's layout problem does not get solved in the library.**

### A note on what is admissible here

The cross-build readings in this entry are not. Extracting 103 lines out of
`main` moved the null arm to p90 2.27% / max 24.87%, so `cvt3216_un_pie`'s
apparent +16.7% is layout: at n = 239 the selection is deterministic
(`unaligned_body(239,24) = 216 < 224`), so the shipped path takes the same
16-wide arm it took before, and the code cannot have changed cost. Every
verdict above rests on the **same-build** A/B instead (§12).
