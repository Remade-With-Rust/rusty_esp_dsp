//! The D0 gate: every kernel that moved here is byte-identical to the copy it
//! replaced. The `original` module below holds those copies verbatim (from
//! `rusty_esp_image-core::ops`, `rusty_esp_audio-core::rms_dbfs_i16` and
//! `rusty_esp_signal-core::radar::csi::isqrt` at the commit before the move);
//! the corpus is an LCG so the run is the same on every machine.

use rusty_esp_dsp::int::isqrt;
use rusty_esp_dsp::pixel::{
    downscale2x_gray8, downscale2x_rgb565, rgb565_to_rgb888, rgb888_to_rgb565, yuyv_to_gray8,
    yuyv_to_rgb565, yuyv_to_rgb888,
};
use rusty_esp_dsp::sample::rms_dbfs_i16;

/// The copies this crate replaced, untouched.
#[allow(clippy::all)]
mod original {
    pub const fn pack_rgb565(r: u8, g: u8, b: u8) -> u16 {
        ((r as u16 & 0xF8) << 8) | ((g as u16 & 0xFC) << 3) | (b as u16 >> 3)
    }

    pub const fn unpack_rgb565(p: u16) -> [u8; 3] {
        let r5 = ((p >> 11) & 0x1F) as u8;
        let g6 = ((p >> 5) & 0x3F) as u8;
        let b5 = (p & 0x1F) as u8;
        [
            (r5 << 3) | (r5 >> 2),
            (g6 << 2) | (g6 >> 4),
            (b5 << 3) | (b5 >> 2),
        ]
    }

    pub fn yuv_to_rgb(y: u8, u: u8, v: u8) -> [u8; 3] {
        let y = i32::from(y);
        let cb = i32::from(u) - 128;
        let cr = i32::from(v) - 128;
        let r = y + ((359 * cr) >> 8);
        let g = y - ((88 * cb + 183 * cr) >> 8);
        let b = y + ((454 * cb) >> 8);
        [clamp(r), clamp(g), clamp(b)]
    }

    fn clamp(v: i32) -> u8 {
        v.clamp(0, 255) as u8
    }

    pub fn rgb565_to_rgb888(src: &[u8], dst: &mut [u8]) -> usize {
        let pixels = src.len() / 2;
        for (s, d) in src.chunks_exact(2).zip(dst.chunks_exact_mut(3)) {
            d.copy_from_slice(&unpack_rgb565(u16::from_le_bytes([s[0], s[1]])));
        }
        pixels
    }

    pub fn rgb888_to_rgb565(src: &[u8], dst: &mut [u8]) -> usize {
        let pixels = src.len() / 3;
        for (s, d) in src.chunks_exact(3).zip(dst.chunks_exact_mut(2)) {
            d.copy_from_slice(&pack_rgb565(s[0], s[1], s[2]).to_le_bytes());
        }
        pixels
    }

    pub fn yuyv_to_rgb888(src: &[u8], dst: &mut [u8]) -> usize {
        let pixels = src.len() / 2;
        for (s, d) in src.chunks_exact(4).zip(dst.chunks_exact_mut(6)) {
            d[..3].copy_from_slice(&yuv_to_rgb(s[0], s[1], s[3]));
            d[3..].copy_from_slice(&yuv_to_rgb(s[2], s[1], s[3]));
        }
        pixels
    }

    pub fn yuyv_to_rgb565(src: &[u8], dst: &mut [u8]) -> usize {
        let pixels = src.len() / 2;
        for (s, d) in src.chunks_exact(4).zip(dst.chunks_exact_mut(4)) {
            let [r0, g0, b0] = yuv_to_rgb(s[0], s[1], s[3]);
            let [r1, g1, b1] = yuv_to_rgb(s[2], s[1], s[3]);
            d[..2].copy_from_slice(&pack_rgb565(r0, g0, b0).to_le_bytes());
            d[2..].copy_from_slice(&pack_rgb565(r1, g1, b1).to_le_bytes());
        }
        pixels
    }

    pub fn yuyv_to_gray8(src: &[u8], dst: &mut [u8]) -> usize {
        let pixels = src.len() / 2;
        for (s, d) in src.chunks_exact(2).zip(dst.iter_mut()) {
            *d = s[0];
        }
        pixels
    }

    pub fn downscale2x_gray8(src: &[u8], width: u32, height: u32, dst: &mut [u8]) {
        let (w, h) = (width as usize, height as usize);
        let (ow, oh) = (w / 2, h / 2);
        for oy in 0..oh {
            let r0 = &src[(2 * oy) * w..(2 * oy) * w + w];
            let r1 = &src[(2 * oy + 1) * w..(2 * oy + 1) * w + w];
            for ox in 0..ow {
                let sum = u32::from(r0[2 * ox])
                    + u32::from(r0[2 * ox + 1])
                    + u32::from(r1[2 * ox])
                    + u32::from(r1[2 * ox + 1]);
                dst[oy * ow + ox] = ((sum + 2) / 4) as u8;
            }
        }
    }

    pub fn downscale2x_rgb565(src: &[u8], width: u32, height: u32, dst: &mut [u8]) {
        let (w, h) = (width as usize, height as usize);
        let (ow, oh) = (w / 2, h / 2);
        let px = |x: usize, y: usize| -> [u8; 3] {
            let i = (y * w + x) * 2;
            unpack_rgb565(u16::from_le_bytes([src[i], src[i + 1]]))
        };
        for oy in 0..oh {
            for ox in 0..ow {
                let a = px(2 * ox, 2 * oy);
                let b = px(2 * ox + 1, 2 * oy);
                let c = px(2 * ox, 2 * oy + 1);
                let d = px(2 * ox + 1, 2 * oy + 1);
                let avg = |k: usize| {
                    ((u32::from(a[k]) + u32::from(b[k]) + u32::from(c[k]) + u32::from(d[k]) + 2)
                        / 4) as u8
                };
                let p = pack_rgb565(avg(0), avg(1), avg(2)).to_le_bytes();
                let o = (oy * ow + ox) * 2;
                dst[o..o + 2].copy_from_slice(&p);
            }
        }
    }

    pub fn rms_dbfs_i16(samples: &[u8]) -> f32 {
        let mut acc: i64 = 0;
        let mut n: i64 = 0;
        for s in samples.chunks_exact(2) {
            let v = i64::from(i16::from_le_bytes([s[0], s[1]]));
            acc += v * v;
            n += 1;
        }
        if n == 0 || acc == 0 {
            return -120.0;
        }
        let mean = acc as f64 / n as f64;
        let rms = libm::sqrt(mean) / 32768.0;
        (20.0 * libm::log10(rms)) as f32
    }

    pub const fn isqrt(v: u32) -> u32 {
        if v < 2 {
            return v;
        }
        let mut x = v;
        let mut y = x.div_ceil(2);
        while y < x {
            x = y;
            y = (x + v / x) / 2;
        }
        x
    }
}

/// A 64-bit LCG (Knuth's constants); the same corpus on every machine.
struct Lcg(u64);

impl Lcg {
    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        self.0
    }

    fn bytes(&mut self, n: usize) -> Vec<u8> {
        (0..n).map(|_| (self.next() >> 56) as u8).collect()
    }
}

#[test]
fn pixel_conversions_match_the_image_core_copies_over_a_corpus() {
    let mut rng = Lcg(0x5EED_D0D0);
    let mut pixels_checked = 0usize;
    for round in 0..64 {
        // even pixel counts so every layout is whole
        let pixels = 2 * (1 + (rng.next() % 640) as usize) + if round == 0 { 76_800 } else { 0 };
        let yuyv = rng.bytes(pixels * 2);
        let rgb = rng.bytes(pixels * 3);
        let p565 = rng.bytes(pixels * 2);

        let (mut a, mut b) = (vec![0u8; pixels * 3], vec![0u8; pixels * 3]);
        assert_eq!(
            yuyv_to_rgb888(&yuyv, &mut a).unwrap(),
            original::yuyv_to_rgb888(&yuyv, &mut b)
        );
        assert_eq!(a, b, "yuyv_to_rgb888 round {round}");

        let (mut a, mut b) = (vec![0u8; pixels * 2], vec![0u8; pixels * 2]);
        assert_eq!(
            yuyv_to_rgb565(&yuyv, &mut a).unwrap(),
            original::yuyv_to_rgb565(&yuyv, &mut b)
        );
        assert_eq!(a, b, "yuyv_to_rgb565 round {round}");

        let (mut a, mut b) = (vec![0u8; pixels], vec![0u8; pixels]);
        assert_eq!(
            yuyv_to_gray8(&yuyv, &mut a).unwrap(),
            original::yuyv_to_gray8(&yuyv, &mut b)
        );
        assert_eq!(a, b, "yuyv_to_gray8 round {round}");

        let (mut a, mut b) = (vec![0u8; pixels * 2], vec![0u8; pixels * 2]);
        assert_eq!(
            rgb888_to_rgb565(&rgb, &mut a).unwrap(),
            original::rgb888_to_rgb565(&rgb, &mut b)
        );
        assert_eq!(a, b, "rgb888_to_rgb565 round {round}");

        let (mut a, mut b) = (vec![0u8; pixels * 3], vec![0u8; pixels * 3]);
        assert_eq!(
            rgb565_to_rgb888(&p565, &mut a).unwrap(),
            original::rgb565_to_rgb888(&p565, &mut b)
        );
        assert_eq!(a, b, "rgb565_to_rgb888 round {round}");
        pixels_checked += pixels;
    }
    assert!(pixels_checked > 100_000, "{pixels_checked}");
}

#[test]
fn downscales_match_the_image_core_copies_including_odd_edges() {
    let mut rng = Lcg(0xD0DE_5CA1);
    for round in 0..48 {
        let (w, h) = if round == 0 {
            (320u32, 240u32)
        } else {
            (2 + (rng.next() % 96) as u32, 2 + (rng.next() % 60) as u32)
        };
        let (ow, oh) = ((w / 2) as usize, (h / 2) as usize);
        let gray = rng.bytes((w * h) as usize);
        let (mut a, mut b) = (vec![0u8; ow * oh], vec![0u8; ow * oh]);
        let geo = downscale2x_gray8(&gray, w, h, &mut a).unwrap();
        original::downscale2x_gray8(&gray, w, h, &mut b);
        assert_eq!(a, b, "gray8 {w}x{h}");
        assert_eq!((geo.width as usize, geo.height as usize), (ow, oh));

        let rgb = rng.bytes((w * h * 2) as usize);
        let (mut a, mut b) = (vec![0u8; ow * oh * 2], vec![0u8; ow * oh * 2]);
        downscale2x_rgb565(&rgb, w, h, &mut a).unwrap();
        original::downscale2x_rgb565(&rgb, w, h, &mut b);
        assert_eq!(a, b, "rgb565 {w}x{h}");
    }
}

#[test]
fn dbfs_matches_the_audio_core_copy_bit_for_bit() {
    let mut rng = Lcg(0xA0D1_0BEE);
    for round in 0..256 {
        let n = if round == 0 {
            16_000 * 2
        } else {
            (rng.next() % 4_097) as usize
        };
        let mut bytes = rng.bytes(n);
        if round % 7 == 3 {
            // quiet blocks exercise the small-value path of the log
            for b in bytes.iter_mut() {
                *b >>= 6;
            }
        }
        let ours = rms_dbfs_i16(&bytes);
        let theirs = original::rms_dbfs_i16(&bytes);
        // NOT bit-equality, and the change is deliberate (ledger R4).
        //
        // This gate's job is to prove a MOVE was faithful. For every other
        // kernel here that is bit-equality, because nothing about them
        // changed. `rms_dbfs_i16`'s float tail did change: it is single
        // precision now, because the ESP32-S3's FPU is, and the `f64` it
        // used to carry compiled to software routines costing ~48% of the
        // one audio block that ships.
        //
        // Pinning an implementation choice with `assert_eq!` turned a
        // refactor gate into a design gate and froze that cost in place. The
        // contract that matters is the ERROR, and it is bounded far harder
        // than this loop could: `tests/dbfs_tail_exhaustive.rs` walks all
        // 520,093,697 inputs the tail can ever receive and reports a maximum
        // of 1.526e-5 dB. The bound below is that figure with room, against
        // a reference this test computes in f64.
        //
        // A VAD threshold is whole dB; a log line prints one decimal.
        let err = f64::from(ours) - f64::from(theirs);
        assert!(
            err.abs() < 1.0e-4,
            "round {round}: {ours} vs {theirs} (err {err:e} dB)"
        );
    }
    assert_eq!(rms_dbfs_i16(&[]), original::rms_dbfs_i16(&[]));
}

#[test]
fn isqrt_matches_the_signal_core_copy_and_the_float_root() {
    let mut rng = Lcg(0x15C1_A5E7);
    for _ in 0..200_000 {
        let v = rng.next() as u32;
        let r = isqrt(v);
        assert_eq!(r, original::isqrt(v));
        assert_eq!(r, (v as f64).sqrt().floor() as u32, "{v}");
    }
    for v in [
        0u32,
        1,
        2,
        3,
        4,
        15,
        16,
        17,
        724 * 724,
        u32::MAX - 1,
        u32::MAX,
    ] {
        assert_eq!(isqrt(v), original::isqrt(v));
    }
}
