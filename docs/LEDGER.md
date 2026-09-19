
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
