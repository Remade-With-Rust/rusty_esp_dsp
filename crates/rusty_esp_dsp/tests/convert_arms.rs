//! `pcm::convert` grew aligned fast arms for the two pairs a voice path runs.
//! Every ordinary buffer is aligned, so the existing tests only ever exercise
//! those; this drives the byte table as well and requires the two to agree.

use rusty_esp_core::pcm::{PcmBlock, PcmFormat, SampleFormat};
use rusty_esp_core::time::Micros;
use rusty_esp_dsp::sample::pcm::convert;

/// Offsetting by one byte makes the address wrong, which is why `as_i16` and
/// `as_f32` refuse in practice.
fn both_arms(from: SampleFormat, to: SampleFormat, src: &[u8], outlen: usize) {
    let f = PcmFormat::new(16_000, 1, from).unwrap();

    let mut fast = vec![0u8; outlen];
    let a = convert(PcmBlock::new(f, Micros(0), src).unwrap(), to, &mut fast).unwrap();

    let mut off_src = vec![0u8; src.len() + 1];
    off_src[1..].copy_from_slice(src);
    let mut off_dst = vec![0u8; outlen + 1];
    let b = convert(
        PcmBlock::new(f, Micros(0), &off_src[1..]).unwrap(),
        to,
        &mut off_dst[1..],
    )
    .unwrap();

    assert_eq!(a, b, "{from:?} -> {to:?}: byte counts differ");
    assert_eq!(
        &fast[..a],
        &off_dst[1..1 + b],
        "{from:?} -> {to:?}: the aligned and byte arms disagree"
    );
}

#[test]
fn convert_arms_agree() {
    for n in [1usize, 2, 3, 7, 16, 63, 256] {
        // i16 source: the extremes and the sign changes, not just a ramp
        let i16s: Vec<u8> = (0..n)
            .flat_map(|k| {
                let v: i16 = match k % 6 {
                    0 => i16::MIN,
                    1 => i16::MAX,
                    2 => 0,
                    3 => -1,
                    _ => ((k as i32 * 7919) % 65536 - 32768) as i16,
                };
                v.to_le_bytes()
            })
            .collect();
        both_arms(SampleFormat::I16, SampleFormat::F32, &i16s, n * 4);

        // f32 source: in range, out of range both ways, and the non-finites,
        // because `f32_to_i16` has to saturate all of them identically
        let f32s: Vec<u8> = (0..n)
            .flat_map(|k| {
                let v: f32 = match k % 7 {
                    0 => 0.0,
                    1 => 1.0,
                    2 => -1.0,
                    3 => 2.5,
                    4 => -2.5,
                    5 => f32::NAN,
                    _ => (k as f32 * 0.013).sin(),
                };
                v.to_le_bytes()
            })
            .collect();
        both_arms(SampleFormat::F32, SampleFormat::I16, &f32s, n * 2);
    }
}
