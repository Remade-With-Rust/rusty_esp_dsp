//! The measurement discipline, as code: work counters both arms of a
//! comparison report, and the ceiling probe that prices a kernel before it
//! is written.
//!
//! On a chip the box is always busy (Wi-Fi, interrupts), so a deterministic
//! count of the work done is the primary evidence and the cycle counter is
//! confirmation. Two arms whose [`Work`] differs did not do the same job and
//! their times cannot be compared. A twin whose expected pipeline gain sits
//! under the harness's noise floor cannot be measured, however fast it is,
//! so it is not built: [`ceiling`] says so before any code exists.

/// What a kernel did, in units it cannot lie about.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Work {
    /// Pixels produced.
    pub pixels: u64,
    /// Samples (one channel each) consumed.
    pub samples: u64,
    /// Blocks (4×4, 8×8, 16×16 — the kernel says which) costed.
    pub blocks: u64,
    /// Bytes moved from input to output.
    pub bytes: u64,
}

impl Work {
    /// Nothing done yet.
    pub const ZERO: Work = Work {
        pixels: 0,
        samples: 0,
        blocks: 0,
        bytes: 0,
    };

    /// `n` pixels, nothing else.
    #[must_use]
    pub const fn pixels(n: u64) -> Work {
        Work {
            pixels: n,
            ..Work::ZERO
        }
    }

    /// `n` samples, nothing else.
    #[must_use]
    pub const fn samples(n: u64) -> Work {
        Work {
            samples: n,
            ..Work::ZERO
        }
    }

    /// `n` blocks, nothing else.
    #[must_use]
    pub const fn blocks(n: u64) -> Work {
        Work {
            blocks: n,
            ..Work::ZERO
        }
    }

    /// Fold another kernel's work into this one (saturating, so a runaway
    /// counter reads `u64::MAX` rather than wrapping to a small number).
    pub fn add(&mut self, other: Work) {
        self.pixels = self.pixels.saturating_add(other.pixels);
        self.samples = self.samples.saturating_add(other.samples);
        self.blocks = self.blocks.saturating_add(other.blocks);
        self.bytes = self.bytes.saturating_add(other.bytes);
    }

    /// `true` when the two arms of a comparison did identical work — the
    /// precondition for comparing their times at all.
    #[must_use]
    pub fn parity(&self, other: &Work) -> bool {
        self == other
    }
}

/// Expected whole-pipeline gain, in permille, when a stage holding
/// `share_permille` of the wall gets `speedup_x100` (100 = unchanged,
/// 400 = four times faster): `share × (1 − 1 / speedup)`.
///
/// A stage that is 6 % of the frame made 4× faster buys 4.5 % of the frame:
/// `pipeline_gain_permille(60, 400) == 45`.
#[must_use]
pub const fn pipeline_gain_permille(share_permille: u32, speedup_x100: u32) -> u32 {
    if speedup_x100 <= 100 {
        return 0;
    }
    let removed_x100 = speedup_x100 - 100;
    // share × removed / speedup, rounded to nearest
    let num = share_permille as u64 * removed_x100 as u64;
    ((num + speedup_x100 as u64 / 2) / speedup_x100 as u64) as u32
}

/// What the ceiling probe says about a kernel that does not exist yet.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// The expected gain clears the harness's floor: worth building, and
    /// this is the number the measurement has to meet.
    Build {
        /// Expected whole-pipeline gain, permille.
        gain_permille: u32,
    },
    /// The expected gain sits at or under the floor: the harness could not
    /// tell a success from noise, so the kernel is not built (and the reason
    /// is recorded, with these two numbers).
    BelowFloor {
        /// Expected whole-pipeline gain, permille.
        gain_permille: u32,
        /// The null-arm floor the harness measured, permille.
        floor_permille: u32,
    },
}

/// The ceiling probe: price a twin before writing it. `floor_permille` is the
/// null arm's spread on this machine (measured, never assumed).
#[must_use]
pub const fn ceiling(share_permille: u32, speedup_x100: u32, floor_permille: u32) -> Verdict {
    let gain_permille = pipeline_gain_permille(share_permille, speedup_x100);
    if gain_permille > floor_permille {
        Verdict::Build { gain_permille }
    } else {
        Verdict::BelowFloor {
            gain_permille,
            floor_permille,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_plan_example_six_percent_stage_four_times_faster_is_four_and_a_half() {
        assert_eq!(pipeline_gain_permille(60, 400), 45);
        assert_eq!(pipeline_gain_permille(1000, 200), 500);
        assert_eq!(pipeline_gain_permille(500, 100), 0);
        assert_eq!(
            pipeline_gain_permille(500, 50),
            0,
            "a slowdown buys nothing"
        );
        // the whole frame made a thousand times faster is all but a permille of it
        assert_eq!(pipeline_gain_permille(1000, 100_000), 999);
    }

    #[test]
    fn a_gain_under_the_floor_is_not_built() {
        assert_eq!(
            ceiling(60, 400, 50),
            Verdict::BelowFloor {
                gain_permille: 45,
                floor_permille: 50
            }
        );
        assert_eq!(ceiling(300, 400, 50), Verdict::Build { gain_permille: 225 });
        assert_eq!(
            ceiling(60, 400, 45),
            Verdict::BelowFloor {
                gain_permille: 45,
                floor_permille: 45
            },
            "equal to the floor is under it"
        );
    }

    #[test]
    fn work_adds_saturating_and_parity_is_equality() {
        let mut w = Work::pixels(76_800);
        w.add(Work::samples(16_000));
        w.add(Work {
            bytes: u64::MAX,
            ..Work::ZERO
        });
        w.add(Work {
            bytes: 1,
            ..Work::ZERO
        });
        assert_eq!(
            w,
            Work {
                pixels: 76_800,
                samples: 16_000,
                blocks: 0,
                bytes: u64::MAX
            }
        );
        assert!(w.parity(&w));
        assert!(!w.parity(&Work::pixels(76_800)));
        assert_eq!(Work::blocks(3).blocks, 3);
    }
}
