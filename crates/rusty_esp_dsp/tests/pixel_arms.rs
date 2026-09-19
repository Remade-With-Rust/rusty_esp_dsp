//! The pixel and sample kernels that grew aligned `u16`/`i16` arms have to
//! agree with the byte arms they kept.
//!
//! Every ordinary buffer is aligned, so the in-crate tests only ever exercise
//! the FAST arm — a broken fallback would pass them forever. Sliding a buffer
//! one byte makes `as_u16`/`as_i16` refuse, which is the only way the byte
//! path runs.

use rusty_esp_dsp::pixel::{
    downscale2x_rgb565, rgb565_to_rgb888, rgb888_to_rgb565, yuyv_to_rgb565,
};
use rusty_esp_dsp::sample::sum_sq_i16_le;

/// Deterministic bytes whose every value is a function of position, so a
/// misplaced pixel is identifiable rather than merely wrong.
fn bytes(n: usize, salt: usize) -> Vec<u8> {
    (0..n)
        .map(|i| (i.wrapping_mul(97).wrapping_add(salt * 31) ^ (i >> 3)) as u8)
        .collect()
}

/// Run a kernel over aligned buffers and again over one-byte-offset buffers,
/// and require the same output bytes from both.
fn both<F>(name: &str, src: &[u8], outlen: usize, mut run: F)
where
    F: FnMut(&[u8], &mut [u8]) -> usize,
{
    let mut fast = vec![0u8; outlen];
    let a = run(src, &mut fast);

    let mut off_src = vec![0u8; src.len() + 1];
    off_src[1..].copy_from_slice(src);
    let mut off_dst = vec![0u8; outlen + 1];
    let b = run(&off_src[1..], &mut off_dst[1..]);

    assert_eq!(a, b, "{name}: the arms returned different counts");
    assert_eq!(
        &fast[..outlen],
        &off_dst[1..1 + outlen],
        "{name}: the aligned and byte arms disagree"
    );
}

#[test]
fn rgb565_converters_agree_on_both_arms() {
    // lengths either side of the 8-pixel unrolled body, and an odd remainder
    for px in [1usize, 2, 3, 7, 8, 9, 15, 16, 17, 31, 64, 255] {
        let src565 = bytes(px * 2, 1);
        both("rgb565_to_rgb888", &src565, px * 3, |s, d| {
            rgb565_to_rgb888(s, d).unwrap()
        });

        let src888 = bytes(px * 3, 2);
        both("rgb888_to_rgb565", &src888, px * 2, |s, d| {
            rgb888_to_rgb565(s, d).unwrap()
        });
    }

    // yuyv works in macropixels, so the pixel count is even
    for px in [2usize, 4, 8, 16, 18, 32, 64, 254] {
        let src = bytes(px * 2, 3);
        both("yuyv_to_rgb565", &src, px * 2, |s, d| {
            yuyv_to_rgb565(s, d).unwrap()
        });
    }
}

#[test]
fn downscale_agrees_on_both_arms() {
    // even dimensions, including non-square and a realistic frame
    for (w, h) in [(2u32, 2u32), (4, 2), (2, 4), (8, 8), (16, 10), (160, 120)] {
        let src = bytes(w as usize * h as usize * 2, 4);
        let outlen = (w as usize / 2) * (h as usize / 2) * 2;

        let mut fast = vec![0u8; outlen];
        let ga = downscale2x_rgb565(&src, w, h, &mut fast).unwrap();

        let mut off_src = vec![0u8; src.len() + 1];
        off_src[1..].copy_from_slice(&src);
        let mut off_dst = vec![0u8; outlen + 1];
        let gb = downscale2x_rgb565(&off_src[1..], w, h, &mut off_dst[1..]).unwrap();

        assert_eq!(ga, gb, "geometry differs at {w}x{h}");
        assert_eq!(
            fast,
            off_dst[1..1 + outlen],
            "downscale2x_rgb565: the arms disagree at {w}x{h}"
        );
    }
}

#[test]
fn sum_sq_agrees_on_both_arms() {
    for n in [0usize, 1, 2, 3, 7, 8, 9, 16, 17, 64, 255, 256] {
        // reach the extremes, not just a ramp: i16::MIN squared is the value
        // that would overflow a narrower accumulator
        let src: Vec<u8> = (0..n)
            .flat_map(|k| {
                let v: i16 = match k % 5 {
                    0 => i16::MIN,
                    1 => i16::MAX,
                    2 => 0,
                    _ => ((k as i32 * 7919) % 65536 - 32768) as i16,
                };
                v.to_le_bytes()
            })
            .collect();

        let aligned = sum_sq_i16_le(&src);

        let mut off = vec![0u8; src.len() + 1];
        off[1..].copy_from_slice(&src);
        let byte_arm = sum_sq_i16_le(&off[1..]);

        assert_eq!(aligned, byte_arm, "sum_sq_i16_le disagrees at {n} samples");

        // and an ODD trailing byte must still be ignored, down either path
        let mut odd = src.clone();
        odd.push(0xAB);
        assert_eq!(
            sum_sq_i16_le(&odd),
            aligned,
            "a trailing odd byte changed the result at {n} samples"
        );
    }
}
