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

Consumers after the switch (each repo's own gates, same day): recorded in
each consumer's ledger and in the umbrella's mission plan; the point of the
D0 gate is that those suites did not change.

## Method line for every future row

`pinned=<core> prio=High metric=<cpu|wall> pairs=<N> order=ABBA null_floor=<‰> work=<pixels|samples|blocks per arm>`

A row without it is not a number.
