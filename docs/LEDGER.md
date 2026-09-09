
## rusty_alloc 2.0.4 validated, and what an aligned region really costs (2026-09-09)

2.0.4 fixes the alignment bug this firmware reported and ships
`prim::fixed::Region`, so this seam stops carrying a container of its own.
Both of its attempts were wrong, and neither was caught in review.

### First, a miss of ours worth recording

The `#[repr(align(65536))]` fix on 2026-09-09 was verified by reading
`free=0` and **not** by re-reading the section table. It cost **60,952 bytes
of stack**:

| | `.bss` | `.stack` |
|---|---:|---:|
| 2.0.3, 220 KiB, unaligned | 225,672 | 107,412 |
| 2.0.3, 200,704, `repr(align)` | 262,532 | **46,460** |

The old sizing rule was `k * SEGMENT_SIZE + FIXED_PAGE`, so 200,704 is not a
whole number of segments and aligning it rounded the *size* up to 262,144.
This crate established the `.stack` identity itself and then shipped a change
without re-checking it. A heap figure is not a footprint check.

### 2.0.4 validated

`Region` is aligned by construction, `N` must be whole segments, and
`size_of::<Region<N>>() == N` is asserted upstream. `HEAP_BYTES` is now
196,608, pinned by a `const` assert to `good_region_size(220 * 1024)` — the
assert caught the change, because 2.0.4 dropped the `+ FIXED_PAGE` and the old
literal stopped building.

On the board: `used=196608 free=0` at every stage. A whole-segment region with
nothing stranded and no descriptor page taken out of it.

| both arms at a 196,608 heap | esp-alloc | rusty 2.0.4 |
|---|---:|---:|
| flash (`.text`+`.rodata`+`.data`) | 60,361 | 63,693 (**+3,332**) |

### The finding: an aligned region's real cost is invisible in `.bss`

Upstream reports the new `Region` saving 63,780 bytes of `.bss` on its rig.
`.bss` is the wrong place to look, because **alignment padding belongs to no
section at all**. RAM on this part is fixed, so the sections must sum to the
same total, and they do not:

| build | `.data` | `.bss` | `.stack` | sum | unaccounted |
|---|---:|---:|---:|---:|---:|
| esp-alloc @196,608, unaligned | 2,300 | 196,700 | 136,368 | 335,368 | 0 |
| rusty 2.0.3 @225,280, unaligned | 2,284 | 225,672 | 107,412 | 335,368 | 0 |
| rusty 2.0.4 @196,608, **aligned** | 2,228 | 198,752 | 110,240 | 311,220 | **24,148** |

The aligned build's sections account for 24,148 fewer bytes of a fixed RAM
map. That is the gap the linker leaves reaching the next 64 KiB boundary, and
no section reports it.

So compare what the firmware actually gets:

| | usable heap | stack |
|---|---:|---:|
| rusty 2.0.3 @225,280 | 196,608 | 107,412 |
| rusty 2.0.4 @196,608 | 196,608 | **110,240** |

**The 24,576 stranded bytes were not recovered — about 24,148 of them
reappeared as the alignment gap.** Usable heap is identical and the real gain
is **2,828 bytes of stack**, not the ~26,920 that the `.bss` drop suggests.

This is not a regression and 2.0.4 is still the better build: it is smaller,
correct by construction, and it removes a startup panic. But on a fixed RAM
map, aligning a 64 KiB-granular region costs up to `SEGMENT_SIZE - 1` bytes
*somewhere*, and sizing cannot avoid it — only the linker's luck in where the
preceding data ended changes it. A `.bss` figure quoted without the section
sum will overstate the saving by roughly a segment.

### Kernels moved again, and it is placement again

| kernel | 2.0.3 @200,704 | 2.0.4 @196,608 | delta |
|---|---:|---:|---:|
| `yuyv_to_gray8` | 125,218 | 150,247 | **+20.0 %** |
| `yuyv_to_rgb565` | 581,875 | 606,915 | +4.3 % |
| `rgb565_to_rgb888` | 450,438 | 463,007 | +2.8 % |
| `rgb888_to_rgb565` | 362,892 | 350,371 | -3.4 % |
| `downscale2x_rgb565` | 1,779,184 | 1,779,340 | +0.01 % |
| `sad_16x16` | 21,557,782 | 21,559,275 | +0.01 % |

Dropping the 4 KiB descriptor page shifted every buffer within its segment,
and the kernels that care moved by up to 20 % while the ones that do not held
at 0.01 %. Mixed sign again, no allocator call inside any measured kernel
again. Fifth confirmation, and the largest placement swing yet — **20 % on a
compute benchmark from a 4 KiB shift in where a buffer starts.**
