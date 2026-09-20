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
| B1 | `downscale2x_rgb565` | `rusty_esp_dsp` | the highest arithmetic intensity left (~40 ops per output pixel on 4 loads) — and P6 showed the remaining headroom is in compute-bound kernels, not more unrolling |
| B2 | `yuyv_to_rgb565` | `rusty_esp_dsp` | colour conversion, 2 bytes out; QACC handles the coefficients |
| B3 | `find_start_code` / `nal_spans` / `access_units` | `rusty_esp_video-core` | scanning for `00 00 01` is a byte compare — `ee.vcmp.eq.s8` |
| B4 | `jpeg::find_eoi` | `rusty_esp_image-core` | scanning for `FF D9`, same shape as B3 |
| B5 | `csi::amplitudes` | `rusty_esp_signal-core` | i8 IQ pairs; `ee.vmulas.s8.accx` exists — but `isqrt` per subcarrier may dominate, so price that first |
| B6 | `sad_4x4` | `rusty_esp_dsp` | low expectation: the 4x4 geometry was refuted twice |

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
