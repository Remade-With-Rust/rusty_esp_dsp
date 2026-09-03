//! The block kernels against the encoder's own transform: `rusty_h264-common`
//! is the reference, this crate's scalar versions must agree with it on every
//! block of a generated corpus. Block counts that are not a multiple of four
//! exercise both the reference's SIMD groups and its scalar tail.

use rusty_esp_dsp::block::{hadamard_4x4, residual_4x4, sad_4x4, sad_8x8, sad_16x16, satd_4x4_sum};
use rusty_h264_common::transform as reference;

struct Lcg(u64);

impl Lcg {
    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        self.0
    }

    /// A residual sample: −255..=255 (what `a − b` over bytes can be), and
    /// every so often the wider range a transform stage can produce.
    fn residual(&mut self, wide: bool) -> i32 {
        let span: u64 = if wide { 8_192 } else { 256 };
        (self.next() % (2 * span)) as i32 - span as i32
    }
}

#[test]
fn hadamard_matches_rusty_h264_common_on_every_block() {
    let mut rng = Lcg(0x4AD4_4AD4);
    for i in 0..20_000 {
        let wide = i % 5 == 0;
        let block: [i32; 16] = core::array::from_fn(|_| rng.residual(wide));
        assert_eq!(
            hadamard_4x4(&block),
            reference::hadamard_4x4(&block),
            "{block:?}"
        );
    }
}

#[test]
fn satd_sums_match_rusty_h264_common_for_every_block_count() {
    let mut rng = Lcg(0x5A7D_5A7D);
    let mut blocks: Vec<[i32; 16]> = Vec::new();
    for count in 0..=41usize {
        blocks.clear();
        for i in 0..count {
            let wide = i % 3 == 2;
            blocks.push(core::array::from_fn(|_| rng.residual(wide)));
        }
        assert_eq!(
            satd_4x4_sum(&blocks),
            reference::satd_4x4_sum(&blocks),
            "count {count}"
        );
    }
    // a QVGA frame's worth: 4 800 blocks in one call
    blocks.clear();
    for _ in 0..4_800 {
        blocks.push(core::array::from_fn(|_| rng.residual(false)));
    }
    assert_eq!(satd_4x4_sum(&blocks), reference::satd_4x4_sum(&blocks));
}

#[test]
fn sad_and_residual_agree_with_their_definitions_over_strided_planes() {
    let mut rng = Lcg(0x5AD0_5AD0);
    let (w, h) = (48usize, 40usize);
    let a: Vec<u8> = (0..w * h).map(|_| (rng.next() >> 56) as u8).collect();
    let b: Vec<u8> = (0..w * h).map(|_| (rng.next() >> 56) as u8).collect();
    let naive = |ax: usize, ay: usize, bx: usize, by: usize, n: usize| -> u32 {
        let mut s = 0u32;
        for r in 0..n {
            for c in 0..n {
                s += u32::from(a[(ay + r) * w + ax + c].abs_diff(b[(by + r) * w + bx + c]));
            }
        }
        s
    };
    for _ in 0..500 {
        let (ax, ay) = ((rng.next() % 32) as usize, (rng.next() % 24) as usize);
        let (bx, by) = ((rng.next() % 32) as usize, (rng.next() % 24) as usize);
        let pa = &a[ay * w + ax..];
        let pb = &b[by * w + bx..];
        assert_eq!(sad_16x16(pa, w, pb, w).unwrap(), naive(ax, ay, bx, by, 16));
        assert_eq!(sad_8x8(pa, w, pb, w).unwrap(), naive(ax, ay, bx, by, 8));
        assert_eq!(sad_4x4(pa, w, pb, w).unwrap(), naive(ax, ay, bx, by, 4));
        let res = residual_4x4(pa, w, pb, w).unwrap();
        for r in 0..4 {
            for c in 0..4 {
                assert_eq!(
                    res[r * 4 + c],
                    i32::from(a[(ay + r) * w + ax + c]) - i32::from(b[(by + r) * w + bx + c])
                );
            }
        }
        // SATD of the residual through the reference agrees with ours
        assert_eq!(satd_4x4_sum(&[res]), reference::satd_4x4_sum(&[res]));
    }
}
