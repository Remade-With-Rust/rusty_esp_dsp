
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
