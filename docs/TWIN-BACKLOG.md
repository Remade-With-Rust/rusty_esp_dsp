# Kernel twin backlog — every scalar loop in the family, and its verdict

Built by census, not by memory: every `.rs` file in every janus repo outside
`target/`, `tests/`, `firmware/` and `examples/`, keeping functions that both
take or return a numeric slice AND contain a loop. That is the shape SIMD can
address; everything else is control flow, parsing or driver code.

Raw: **165 untwinned loop-over-slice functions, 28 already twinned.** Most of
the 165 are parsers, protocol framing, filesystem and host tooling. What
survives triage is below.

The script is `scratchpad/census.py` in the session that produced this; the
method is worth repeating rather than the output worth trusting, because a
glob over `src/*.rs` already missed a whole submodule once (`sample::pcm`,
ledger P4).

---

## Tier A — the twin EXISTS and nothing calls it

**This is the whole of the delivered-value gap.** No new kernels; each item
is a seam. The dsp seam itself was in this state for the entire campaign
(fifty-seven twins, no caller) until it was wired.

**ALL EIGHT DONE.** Each row is the CALL SITE measured before and after, on
an ESP32-S3, not the kernel in isolation — so these are delivered numbers.

| # | call site | crate | before | after | |
|---|---|---|---:|---:|---:|
| A1 | `rms_dbfs_i16` | `rusty_esp_audio-core` | 215,142 | 38,312 | **−82.2%** |
| A2 | `Gain::process` | `rusty_esp_audio-core` | 134,920 | 27,896 | **−79.3%** |
| A3 | `StereoToMono::process` | `rusty_esp_audio-core` | 96,008 | 42,897 | **−55.3%** |
| A4 | `MonoToStereo::process` | `rusty_esp_audio-core` | 55,259 | 27,165 | **−50.8%** |
| A5 | `mix_i16` | `rusty_esp_audio-core` | 108,364 | 16,783 | **−84.5%** |
| A6 | `peak_abs_i16` | `rusty_esp_audio-core` | 78,259 | 10,272 | **−86.9%** |
| A7 | `Convert::process` int pairs | `rusty_esp_audio-core` | 193,418 | 29,240 | **−84.9%** |
| A8 | `rotate90_gray8` | `rusty_esp_image-core` | 201,137 | 12,329 | **−93.9%** |

A1 is the one that matters most: the shipping PDM firmware calls it once per
captured block.

Each is behind an off-by-default `pie-s3` feature, dispatched by a helper
whose BOTH arms exist — off-chip or feature-off it returns `false`/`None`
from a const-foldable body, so the scalar stays visible as the oracle and is
never dead code. A7 needed `as_i32`/`as_i32_mut` added to `rusty_esp_core`.

**Wiring these broke five of the probe's gates and it took a deliberate look
to notice.** Once an element routes to the twin, a gate comparing the
element against the twin is comparing the twin with itself — and it keeps
printing `identical=true`. Each is now forced onto an arm the chip path
cannot take: an output at an ODD byte offset (so `as_i16_mut` refuses it and
the element runs its byte arm) or, for `rotate90_gray8`, an unaligned
destination its chip arm declines by precondition.

## Tier B — no twin, and worth building

| # | kernel | crate | why it is a candidate |
|---|---|---|---|
| ~~B1~~ | ~~`downscale2x_rgb565`~~ | `rusty_esp_dsp` | **DONE, −65.5%** (1,330,835 → 458,773 ps/px_out). 28.5 instructions per output pixel against the scalar's ~287 cycles. |
| ~~B2~~ | ~~`yuyv_to_rgb565`~~ | `rusty_esp_dsp` | **DONE, −63.2%** (399,135 → 146,995 ps/px). 10.1 instructions per pixel. |
| ~~B3~~ | ~~`find_start_code`~~ | `rusty_esp_dsp` (kernel) | **DONE, −93.4%** (119,815 → 7,911 ps/byte, 15.1×) as `pie_s3::find_start_code3`. **Not yet wired** into `rusty_esp_video-core`, which is the remaining delivery step. |
| ~~B4~~ | ~~`jpeg::find_eoi`~~ | `rusty_esp_image-core` | **DROPPED on inspection.** It scans BACKWARDS from the end and a well-formed JPEG has `FF D9` as its last two bytes, so it exits on the first iteration. Its recorded 175,274 ps/byte is a pathological-input number, not the production cost. Vectorising an O(1)-in-practice loop buys nothing. |
| ~~B5~~ | ~~`csi::amplitudes`~~ | `rusty_esp_signal-core` | **PRUNED on arithmetic, no kernel written.** Two arms: the full loop 1,319,368 ps/subcarrier, the same loop without the `isqrt` 217,933. **`isqrt` is 83.5%** — it is Newton's method over 32-bit DIVIDES, sequential, and PIE has no divide. Making the whole vectorisable remainder FREE caps a twin at 16.5%. |
| ~~B6~~ | ~~`sad_4x4`~~ | `rusty_esp_dsp` | **DONE, −15.8%** (4,053,875 → 3,414,401), after two revisions. See below. |

### ★ B6 pays where the other 4x4 kernels did not, and the difference is nameable

`residual_4x4` (+17.8%) and `satd_4x4` (+25.4%, then +7.6% rebuilt) were
refuted with "four bytes a row is a poor fit for a sixteen-byte register".
`sad_4x4` has exactly that geometry and came in at **−11.8%**.

The distinguishing feature is not the block size. It is that **`sad_4x4` has
no transpose.** The two that lost both had to recombine 64-bit halves — one
through memory, one in registers — and that was the cost the P6 entry named.
`sad_4x4` streams rows, widens against zero, subtracts and accumulates, with
nothing crossing lanes at all.

Read the old refutation as "a 4x4 TRANSPOSE does not pay here", not "4x4 does
not pay here". The `.xp` register-strided load does the row walk in one
instruction, and only the first four lanes of each widened row carry data —
the rest accumulate bytes that follow the row and are simply never read.

### ★ B6's three versions, and why instruction count chose the wrong one

| version | ps/block | |
|---|---:|---:|
| stream the four rows, `sad_8x8`'s body | 3,576,993 | −11.8% |
| gather the block with `ee.movi.32.q` | 3,539,482 | −12.7% |
| pair rows with `ee.vzip.32` | **3,414,401** | **−15.8%** |

The first left half the machine idle: a 4x4 row is FOUR bytes, so a
sixteen-byte load fetched four useful bytes and the widen/subtract/magnitude
that followed ran on four live lanes out of eight, four times over.

The obvious fix was to GATHER the block — `ee.movi.32.q` inserts a general
register into a chosen lane, so four `l32i`/insert pairs pack all sixteen
bytes into one register and the arithmetic runs once at full width. That cut
48 instructions to 34, a predicted −29%, **and bought one per cent.**
General-to-vector transfers are LATENCY, not issue slots, and four of them
in a dependency chain cost about what they saved.

The version that won never leaves the vector file. `ee.vzip.32` interleaves
32-bit lanes, so zipping row r with row r+1 puts their four-byte rows side
by side in the low half — one widening then covers both with every lane
live. Same idea as the gather, no transfers, and it is the only one of the
three whose measured gain tracked its instruction count.

### The constraint every byte-scan candidate (B3) has to work around

PIE has `ee.vcmp.eq.s8` but **no movemask** — nothing turns a lane mask into
a scalar bitfield. So "which lane matched" costs a store and a scalar walk,
which is what the scan already was.

What IS cheap is "did ANY lane match", entirely in registers:
`ee.zero.accx`, `ee.vmulas.s8.accx` of the mask against a broadcast one, then
`ee.srs.accx` into a general register and branch on it. The mask lanes are 0
or −1, so the accumulator is −(match count) and non-zero iff there was a hit.
That makes a scanner that skips 16 bytes per ~6 instructions and drops to
scalar only inside a block that hit — the right shape for B3, where the
common case is no match.

## Tier C — ruled out, with the reason

Kept so nobody re-derives them.

| kernel | why |
|---|---|
| `yuyv_to_rgb888`, `rgb565_to_rgb888`, `rgb888_to_rgb565` | 3 bytes a pixel needs a 3-way deinterleave; PIE has only 2-way `zip`/`unzip` and no general byte permute. **Instruction-set refutation — does not expire.** |
| `satd_4x4`, `hadamard_4x4`, `residual_4x4` | built and measured worse twice (+25.4% via memory transpose, +7.6% via register transpose). Ledger P4, P6. |
| `rotate180` | needs a byte reverse; `zip`/`unzip` rotate index bits, they do not complement them. `ee.bitrev` is FFT addressing. |
| `Biquad`, `DcBlock`, `Agc` filter, `adpcm_ima` | sequential feedback — `y[n]` depends on `y[n-1]`. |
| `LinearResampler` | gather at fractional positions; no gather instruction. |
| `crop` | already `copy_from_slice` per row, i.e. memcpy. |
| `eq_constant_time` | must stay constant-time. |
| `espino/*`, `rusty_esp_iroh/*`, video/signal protocol framing, `capability` | host-side tooling or parsing/crypto, not on-chip numeric work. |

---

## The rule this backlog exists to enforce

A kernel with a passing gate and a recorded win is **not delivered** until a
caller reaches it. Tier A is entirely kernels that were finished, measured
and unreachable. Wire the seam in the same commit as the kernel, and add the
reachability arm that proves it — the trait call must cost what the twin
costs, not what the oracle costs.
