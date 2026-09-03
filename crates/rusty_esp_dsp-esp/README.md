# rusty_esp_dsp-esp

The chip half of `rusty_esp_dsp`: the PIE twins of the scalar kernels —
`PieS3` (ESP32-S3 `ee.*`, feature `pie-s3`) and `PieP4` (ESP32-P4 `esp.*`,
feature `pie-p4`) — each implementing the seam's `PixelKernels`,
`SampleKernels` and `BlockKernels`, and each gated byte-identical against
`rusty_esp_dsp::seam::Scalar` over a generated corpus.

Today no twin exists: both types delegate to the scalar oracle, so a firmware
can already select its kernel set in one place (`default_kernels()`) and
nothing changes as twins arrive. The crate is `deny(unsafe_code)`; the first
twin brings the first fenced block, around an intrinsic and nothing else, and
its board row (D2) in the package ledger.

Part of Janus (Remade With Rust). Plan: `docs/plans/rusty_esp_dsp.md`.
