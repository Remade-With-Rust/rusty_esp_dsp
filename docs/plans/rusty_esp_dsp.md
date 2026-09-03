# rusty_esp_dsp — the PIE kernel home (plan, 2026-09-02)

> Layer 0 foundation, next to `rusty_esp_core`. **D0 shipped on the host
> 2026-09-02** (JANUS.toml: `d0-host`): the repo exists, the scalar kernels
> the inventory below names live in it, and the image, audio and signal
> cores import them (the umbrella's `docs/plans/rusty_esp_dsp.md` is the
> roadmap this file was created from). What remains is D1 (the harness) on
> the host and D2 to D4 on boards.
>
> **Status by milestone:** D0 ✅ host 2026-09-02 (gates in `docs/LEDGER.md`:
> 16 unit + 7 oracle tests, byte identity against every replaced copy and
> against `rusty_h264-common`, clippy, deny, both riscv32 targets) · D1 ☐ ·
> D2 ☐ (board) · D3 ☐ (board, software half first) · D4 ☐ (board).

## 1. Why it exists, and the rule that keeps it small

Every function package carries the hot loops it needs, scalar, in its own
`-core`. That is the right first home: a kernel next to its one caller is
easy to gate and easy to delete. The moment **two** `-core` crates carry the
same loop — or one loop is the thing a chip-side speedup would be spent on —
it moves here, **scalar oracle first**, and the PIE twin is written against
that oracle, never against the caller.

Three rules, all inherited from the family plan (§11) and the codec skills:

1. **The scalar path is the oracle, forever.** Every PIE (S3 `ee.*`, P4
   `esp.*`) or hand-unrolled path is gated byte-identical (integer kernels)
   or tolerance-plus-end-to-end (float kernels) against it, on the host, in
   CI. The scalar version never leaves the tree.
2. **A ceiling probe before a kernel.** Expected pipeline gain = stage share
   × kernel speedup. If the stage is 6 % of the frame, a 4× kernel buys
   4.5 % — say so and do not build it. Count that the fixture reaches the
   kernel (a byte census) before pricing it.
3. **Counters before clocks.** On a chip the box is always busy (Wi-Fi,
   interrupts). Deterministic work counters (pixels, blocks, samples,
   bytes moved) are the primary evidence; cycle counts confirm. Work-count
   parity between the scalar and the PIE arm is checked before any ratio.

Non-goals: no FFT until a consumer needs one (audio's CELT/MDCT question is
A4's, and it starts with `rusty-opus` scalar cycles, not a kernel); no
neural-network ops (ESP-DL / ESP-NN are FFai's, not this crate's); no
product types, no allocator, no drivers.

## 2. What exists today, by owner

The inventory that decides D0's contents. "Consumers" is who calls it now;
"twin" is whether a PIE version is worth pricing on the S3.

| Kernel (today's name) | Owner | Consumers | Shape | Twin worth pricing? |
|---|---|---|---|---|
| `yuyv_to_rgb565`, `yuyv_to_rgb888`, `yuyv_to_gray8` | `rusty_esp_image-core::ops` | image (I2 Track B capture), video (raw → JPEG on the S3) | per-pixel packed conversion, integer | **yes** (I5): 320×240 at 15 fps is 1.2 Mpx/s, the whole raw path |
| `rgb565_to_rgb888` | image `ops` | image, video | per-pixel | yes, same loop family |
| `downscale2x_gray8`, `downscale2x_rgb565` | image `ops` | image (analytics), signal (presence ROI) | 2×2 box, integer | yes; also the first kernel two packages share |
| `Biquad` (RBJ, `elements::biquad`) | `rusty_esp_audio-core` | audio (DC block, EQ), signal (CSI band-pass on the wander signal, planned) | IIR, delay-1 recurrence, **serial** | **no** for the recurrence (delay-1 IIR does not vectorise); yes for the block layout around it |
| `Agc`, `rms_dbfs_i16`, `EnergyVad` | audio | audio | reductions over i16 | yes: `dot_i16` / sum-of-squares is one PIE op |
| PCM `convert` (`convert_sample`) | audio `codec::pcm` | audio, video (TS mux of audio) | per-sample format change | maybe; measure share first |
| CSI amplitude ×4 fixed point, wander (CV ‰) | `rusty_esp_signal-core::radar::csi` | signal | per-subcarrier magnitude + variance | not on its own (64 subcarriers × 10 Hz is nothing); shares `dot_i16` |
| SAD 8×8 / 16×16, SATD, Hadamard 4×4, quant, copy | `rusty_h264` (upstream, `rusty_h264-accel`) | video V3/V5 through the encoder | block kernels, integer | **yes** (V5), but through `rusty_h264`'s own accel seam — this crate supplies the S3 twins, upstream keeps the oracle |
| `Framer` / RTP scan copies | video | video | memcpy | no: the compiler already vectorises straight copies (codec-optimize law) |

Two loops are already shared (`downscale2x_*` by image and signal; `dot_i16`
in three shapes across audio and signal), which is the C3 trigger.

## 3. Crate shape

```
rusty_esp_dsp/                     one repo, deployable on its own (umbrella rule)
├── crates/rusty_esp_dsp/          no_std, forbid(unsafe): the scalar kernels + the seam
│   └── src/{lib.rs, pixel.rs, block.rs, sample.rs, probe.rs}
├── crates/rusty_esp_dsp-esp/      the WRAP crate: PIE intrinsics / inline asm behind
│   └── src/{s3.rs, p4.rs}         `pie-s3` / `pie-p4` features; the only fenced unsafe
├── bench/                         the pinvs-shaped host harness (ABBA, null arm, CPU time)
└── docs/{LEDGER.md, plans/rusty_esp_dsp.md}
```

- **Dependency direction:** `rusty_esp_dsp` depends on `rusty_esp_core`
  only. Function `-core` crates depend on `rusty_esp_dsp` (the kernel moves
  out, the caller imports it). Nothing depends on `-esp` except a firmware.
- **The seam:** one trait per kernel family with the scalar impl as the
  default and the PIE impl selected by feature *and* by runtime chip check
  where the instruction set differs within a target:
  ```rust
  pub trait PixelKernels { fn yuyv_to_rgb565(src: &[u8], dst: &mut [u8]) -> Result<usize>; ... }
  pub struct Scalar;                 // always present, always the oracle
  #[cfg(feature = "pie-s3")] pub struct PieS3;   // -esp
  ```
  Callers take `impl PixelKernels`, default `Scalar`. A firmware picks
  `PieS3` in one place. No `#[cfg]` inside a function package.
- **Every kernel ships with:** its scalar oracle, a byte-identical (or
  tolerance) test that runs the twin against it over a generated corpus, a
  work counter (`pixels`, `blocks`, `samples`) both arms report, and a
  ledger row before any claim.
- **Borrowed slices in, borrowed slices out** (the family's borrowed-frame
  rule): kernels never allocate; sizes are checked and `BufferTooSmall`
  names the need.

## 4. Milestones

| # | Deliverable | Gate |
|---|---|---|
| **D0** ✅ host 2026-09-02 | The repo and the scalar core: `pixel` (the four YUYV/RGB conversions, the two downscales), `sample` (`dot_i16`, `sum_sq_i16`, `rms_dbfs_i16`, PCM convert), `block` (`sad8x8`, `sad16x16`, `satd4x4`, `hadamard4x4` — scalar, matching `rusty_h264`'s), `probe` (the work counters + the ceiling-probe helper); image, audio and signal cores switched to import them, their own copies deleted | byte-identical outputs against the copies they replace over a generated corpus (the same LCG corpus style as the core's manifest test); all four consumer crates' suites unchanged; `riscv32imac` and `riscv32imafc` compile with `--no-default-features` |
| **D1** | The host harness: `bench/pinvs.ps1` ported (pinned, High priority, CPU time, ABBA, paired win-rate with z, null arm, method line printed) and a `probe` example that prints each kernel's share of a 320×240 raw frame path and a 16 kHz PCM second | a null-arm floor recorded per machine before any speed number; the share table in the ledger |
| **D2** | S3 PIE twins for the kernels the probe ranks first (expected: `yuyv_to_rgb565`, `downscale2x_rgb565`, `dot_i16`), in `-esp` behind `pie-s3`, gated byte-identical on the host against `Scalar` through a **cycle-accurate?** no — through the same oracle tests compiled for the S3 and run on the board; work counters equal both arms | on the S3: each twin's pixels/s or samples/s against the scalar arm, cycle counter, same work count; the ceiling probe's prediction beside the measurement (a twin that misses its prediction is a finding, not a failure) — **board row** |
| **D3** | The H.264 block twins: `sad`, `satd`, `hadamard`, `quant` for the S3, wired to `rusty_h264` through its accel seam (upstream PR, the way `rusty_h264-accel` carries x86 today), gated on the encoder's byte-identical bitstream vs scalar over the QVGA oracle | bitstream byte-identical; V3's S3 FPS row before and after, with the profile share that predicted it — **board row**, with the software half (the seam and the twins compiled for the S3) done first |
| **D4** | P4 (`esp.*`, 128-bit) twins of D2's kernels behind `pie-p4`; the ESP32/S2/C-series stay scalar by design | as D2, on the P4 — **board row** |

D0 and D1 are host work and are what "hammer rusty_esp_dsp" means before a
board arrives. D2 to D4 each have a software half (the twin, compiled and
oracle-tested on the host with the intrinsics stubbed to scalar) and a
board half (the number).

## 5. What the first session does, in order

1. `rusty_esp_dsp` repo from the family template: private, `JANUS.toml`
   entry to `d0-host` when D0 lands, the umbrella patch table taught the new
   sibling, `deny.toml` with the git allow-list.
2. Move `rusty_esp_image_core::ops::{yuyv_*, rgb565_to_rgb888, downscale2x_*}`
   to `rusty_esp_dsp::pixel` — **verbatim first, commit, then any change** —
   with the corpus test written against the moved copy before the original
   is deleted. Same for audio's reductions and PCM convert.
3. Write the H.264 block kernels scalar, matching `rusty_h264`'s scalar
   functions byte for byte (its `common` crate is the reference; the test
   pulls it as a dev-dependency and compares).
4. The `probe` module and the harness; the share table for the raw frame
   path and the PCM second, in the ledger, with the null-arm floor.
5. Update I5, V5, A5, C3 rows to point here; retire the "until
   `rusty_esp_dsp` exists" notes in the audio plan.

## 6. Decision log

| Date | Decision |
|---|---|
| 2026-09-02 | D0 kept `quantize` out: it drags the 52×8 MF/deadzone tables and the encoder's own dead-zone policy along; D3 is where the quantizer twin is priced, through `rusty_h264`'s seam, with the tables staying upstream. |
| 2026-09-02 | The block kernels take a stride and return `Result` (`BufferTooSmall` names the bytes needed): one bounds check per block is nothing next to 64 to 256 absolute differences, and it is the family's rule. |
| 2026-09-02 | Crate shape for D0 is one crate, `crates/rusty_esp_dsp`, not the function template's facade/core/esp trio: there is no backend to wrap yet, and the `-esp` crate arrives with D2's first twin. |
| 2026-09-02 | Written as a roadmap, not a crate: the trigger (two packages sharing a loop) is met by `downscale2x_*` and the `dot_i16` family, but the four milestones that name this crate all have a host oracle half that comes first. |
| 2026-09-02 | Delay-1 IIR (`Biquad`) is not a PIE candidate: a feedback loop vectorises byte-exact only when the delay is at least the vector width (codec-optimize law, 2026-07-10). Its block layout and the reductions around it are. |
| 2026-09-02 | The H.264 twins go upstream through `rusty_h264`'s accel seam, not into the encoder's callers; this crate is where the S3/P4 asm lives, `rusty_h264-common` is where the oracle lives. |
| 2026-09-02 | No claim without a ledger row, and no ledger row without the method line (pinned? CPU time? pairs? null-arm floor?). The harness enforces it; a document alone does not. |
