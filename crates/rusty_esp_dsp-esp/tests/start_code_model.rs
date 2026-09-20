//! The Annex-B start-code scan (backlog B3), gated as an ALGORITHM on the host.
//!
//! `pie_s3::find_start_code3` is `cfg(target_arch = "xtensa")`, so this file
//! cannot call it. What it can do is carry the same control flow with the one
//! vector step replaced by a scalar equivalent, and fuzz that against a naive
//! scan. That gates everything in the kernel which is NOT the assembly: the
//! prefix walk to the first 16-byte boundary, the two-bytes-past-the-end read
//! that catches a code straddling a block edge, the skip of zero-free blocks,
//! and the tail past the last whole block.
//!
//! The assembly itself is gated on hardware, byte-identical against
//! `annexb::scan3`, and that run is the real gate. This one is the cheap half
//! that catches a boundary error without a board, and it is the half that
//! would have caught the straddle case had it been wrong.
//!
//! Keep the two in step: if `find_start_code3` changes shape, change `model`.

/// The naive definition, and the oracle for this file.
fn naive(bytes: &[u8]) -> Option<usize> {
    if bytes.len() < 3 {
        return None;
    }
    (0..bytes.len() - 2).find(|&i| bytes[i] == 0 && bytes[i + 1] == 0 && bytes[i + 2] == 1)
}

/// What `first_zero_block` computes, without the vector unit: the index of
/// the first 16-byte block at or after `from` that holds a zero byte.
fn first_zero_block(bytes: &[u8], from: usize, nblocks: usize) -> usize {
    (from..nblocks)
        .find(|&k| bytes[k * 16..k * 16 + 16].contains(&0))
        .unwrap_or(nblocks)
}

/// Mirror of `pie_s3::aligned_scan`.
fn aligned_scan(bytes: &[u8]) -> Option<usize> {
    let n = bytes.len();
    if n < 3 {
        return None;
    }
    let hit = |i: usize| i + 2 < n && bytes[i] == 0 && bytes[i + 1] == 0 && bytes[i + 2] == 1;
    let nblocks = n / 16;
    if nblocks == 0 {
        return (0..n - 2).find(|&i| hit(i));
    }
    let mut b = 0usize;
    while b < nblocks {
        let k = first_zero_block(bytes, b, nblocks);
        if k >= nblocks {
            break;
        }
        let start = k * 16;
        let end = (start + 16).min(n - 2);
        for i in start..end {
            if hit(i) {
                return Some(i);
            }
        }
        b = k + 1;
    }
    ((nblocks * 16)..n - 2).find(|&i| hit(i))
}

/// Mirror of `pie_s3::find_start_code3`. `align` stands in for the real
/// `align_offset(16)`, which a host test cannot control directly.
fn model(bytes: &[u8], align: usize) -> Option<usize> {
    let n = bytes.len();
    if n < 3 {
        return None;
    }
    let hit = |i: usize| i + 2 < n && bytes[i] == 0 && bytes[i + 1] == 0 && bytes[i + 2] == 1;
    let off = align;
    if off + 16 > n {
        return (0..n - 2).find(|&i| hit(i));
    }
    for i in 0..off {
        if hit(i) {
            return Some(i);
        }
    }
    aligned_scan(&bytes[off..]).map(|i| i + off)
}

/// xorshift64*, so a failure is reproducible from its seed.
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }
    /// Bytes drawn from a tiny alphabet, so start codes and near-misses
    /// (`00 00 00 01`, `00 00 02`, long zero runs) occur by chance rather
    /// than having to be planted.
    fn byte(&mut self) -> u8 {
        match self.next() % 10 {
            0..=5 => 0,
            6 | 7 => 1,
            8 => 2,
            _ => (self.next() >> 33) as u8,
        }
    }
}

#[test]
fn model_matches_the_naive_scan_at_every_alignment() {
    let mut rng = Rng(0x0B3_5CA4_2026);
    let mut checked = 0usize;
    for len in 0..200usize {
        for _ in 0..40 {
            let buf: Vec<u8> = (0..len).map(|_| rng.byte()).collect();
            // Every offset the real `align_offset(16)` could return.
            for align in 0..16usize {
                assert_eq!(
                    model(&buf, align),
                    naive(&buf),
                    "len={len} align={align} buf={buf:?}"
                );
                checked += 1;
            }
        }
    }
    assert!(checked > 100_000, "the sweep did not run: {checked}");
}

#[test]
fn a_code_straddling_a_block_boundary_is_found() {
    // The case the block skip could lose: the `00 00 01` begins in one
    // sixteen-byte block and ends in the next, and the block it begins in
    // is the only one holding a zero.
    for align in 0..16usize {
        for start in 0..96usize {
            let mut buf = vec![0xFFu8; 160];
            buf[start] = 0;
            buf[start + 1] = 0;
            buf[start + 2] = 1;
            assert_eq!(
                model(&buf, align),
                Some(start),
                "align={align} start={start}"
            );
        }
    }
}

#[test]
fn a_zero_free_stream_is_never_a_false_positive() {
    let buf = vec![0x5Au8; 300];
    for align in 0..16usize {
        assert_eq!(model(&buf, align), None);
    }
}

#[test]
fn the_short_and_empty_cases_agree() {
    for len in 0..20usize {
        for pattern in [0u8, 1, 0xFF] {
            let buf = vec![pattern; len];
            for align in 0..16usize {
                assert_eq!(model(&buf, align), naive(&buf), "len={len} align={align}");
            }
        }
    }
    // The all-zeros-then-one shape, which is what a real Annex-B AUD is.
    let buf = [0u8, 0, 0, 1, 9, 0xF0];
    for align in 0..16usize {
        assert_eq!(model(&buf, align), naive(&buf));
    }
    // ONE, not zero: this scan finds `00 00 01`, and in a four-byte start
    // code that sits at index 1. Backing up over the leading zeros to reach
    // the start of the code is `annexb::find_start_code`'s job, and keeping
    // the two responsibilities apart is what lets this half be a kernel.
    assert_eq!(naive(&buf), Some(1));
}
