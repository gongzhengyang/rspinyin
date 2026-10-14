//! Fixed-point exponential decay for logical user-dictionary clocks.
//!
//! Responsibility: age a stored accumulator from one tick to another and turn that
//! accumulator into the integer frequency the frozen [`UserFreqSource`] contract
//! consumes. The arithmetic is pure and saturating: the same `(accumulator, from, now)`
//! always produces the same result, with no clock, filesystem, allocation or floating
//! point in the ranking path.

use ime_types::ImeError;

/// Decay parameters in the integer domain the store can evaluate cheaply.
///
/// The default half-life is 139 commits: it is the fixed-point equivalent of librime's
/// `exp((ta - t) / 200)` e-folding constant, whose half-life is `200 * ln(2)`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DecayParams {
    /// Commits over which a contribution loses half its weight.
    pub half_life_ticks: u32,
    /// Accumulator floor in Q16.16; folding never drives a record below it.
    pub floor_q16: i64,
}

impl Default for DecayParams {
    fn default() -> Self {
        Self::new(139, 1 << 16).unwrap_or(Self {
            half_life_ticks: 139,
            floor_q16: 1 << 16,
        })
    }
}

impl DecayParams {
    /// Validates externally supplied parameters.
    ///
    /// # Errors
    ///
    /// Returns [`ImeError::ConfigInvalid`] for a zero half-life, which would otherwise
    /// divide by zero, or a negative floor.
    pub fn new(half_life_ticks: u32, floor_q16: i64) -> Result<Self, ImeError> {
        if half_life_ticks == 0 {
            return Err(ImeError::ConfigInvalid {
                key: "decay_half_life_ticks".to_owned(),
                reason: "must be greater than zero".to_owned(),
            });
        }
        if floor_q16 < 0 {
            return Err(ImeError::ConfigInvalid {
                key: "decay_floor".to_owned(),
                reason: "must not be negative".to_owned(),
            });
        }
        Ok(Self {
            half_life_ticks,
            floor_q16,
        })
    }

    /// Folds an accumulator forward from `tick` to `now`.
    ///
    /// The factor is `2^(-elapsed / half_life)`, split into whole halvings and a
    /// fractional step selected from [`DECAY_TABLE`]. The table covers `2^(-k / 256)`
    /// at 1/65,536 resolution, keeping the factor error below one part in 256 while a
    /// decode costs one table load and shift.
    pub fn fold(&self, dee_q16: i64, tick: u64, now: u64) -> i64 {
        if dee_q16 <= 0 || now <= tick {
            return dee_q16.max(0);
        }
        let elapsed = now - tick;
        let half_life = u64::from(self.half_life_ticks);
        let whole_steps = elapsed / half_life;
        let remainder = elapsed % half_life;
        // Splitting the division first avoids overflow in `elapsed * 256` across the
        // full u64 tick range while producing the same fixed-point age.
        // Saturate the fixed-point age itself: a pathological tick gap must age the
        // record to the floor, not overflow the exponent and panic the decode path.
        let units = whole_steps
            .saturating_mul(256)
            .saturating_add((remainder.saturating_mul(256)) / half_life);
        let fractional = DECAY_TABLE[usize::try_from(units & 0xff).unwrap_or(0)];
        let shift = (units >> 8).min(i64::BITS as u64 - 1);
        let factor = fractional >> u32::try_from(shift).unwrap_or(0);
        let decayed = dee_q16
            .checked_mul(factor)
            .map_or(i64::MAX, |scaled| scaled >> 16);
        decayed.max(self.floor_q16)
    }

    /// Maps a decayed accumulator to the frequency the existing scorer consumes.
    ///
    /// The accumulator is a weighted commit count, so its integer part is already in
    /// the `u32` domain the scorer expects. `commits` is a ceiling against a corrupt
    /// or imported accumulator claiming more observations than the record stored.
    pub fn effective_freq(&self, commits: u32, dee_q16: i64) -> u32 {
        // Round rather than truncate: a record folded from two fresh commits to 1.95
        // still means two uses at this resolution, while a true half-life result of
        // 1.0 stays at one. The conversion is also the only place the accumulator
        // becomes the scorer's coarse integer frequency.
        let weighted = (dee_q16.max(0).saturating_add(1 << 15)) >> 16;
        let weighted = u32::try_from(weighted).unwrap_or(u32::MAX).min(commits);
        if dee_q16 < self.floor_q16 {
            0
        } else {
            weighted
        }
    }
}

/// `round(65536 * 2^(-k / 256))` for every fractional step of one halving.
const DECAY_TABLE: [i64; 257] = [
    65536, 65359, 65182, 65006, 64830, 64655, 64480, 64306, 64132, 63958, 63785, 63613, 63441,
    63269, 63098, 62928, 62757, 62588, 62419, 62250, 62081, 61914, 61746, 61579, 61413, 61247,
    61081, 60916, 60751, 60587, 60423, 60260, 60097, 59934, 59772, 59611, 59449, 59289, 59128,
    58968, 58809, 58650, 58491, 58333, 58176, 58018, 57861, 57705, 57549, 57393, 57238, 57083,
    56929, 56775, 56622, 56468, 56316, 56163, 56012, 55860, 55709, 55558, 55408, 55258, 55109,
    54960, 54811, 54663, 54515, 54368, 54221, 54074, 53928, 53782, 53637, 53492, 53347, 53203,
    53059, 52916, 52773, 52630, 52488, 52346, 52204, 52063, 51922, 51782, 51642, 51502, 51363,
    51224, 51085, 50947, 50810, 50672, 50535, 50399, 50262, 50126, 49991, 49856, 49721, 49586,
    49452, 49319, 49185, 49052, 48920, 48787, 48655, 48524, 48393, 48262, 48131, 48001, 47871,
    47742, 47613, 47484, 47356, 47228, 47100, 46973, 46846, 46719, 46593, 46467, 46341, 46216,
    46091, 45966, 45842, 45718, 45594, 45471, 45348, 45225, 45103, 44981, 44859, 44738, 44617,
    44497, 44376, 44256, 44137, 44017, 43898, 43780, 43661, 43543, 43425, 43308, 43191, 43074,
    42958, 42841, 42726, 42610, 42495, 42380, 42265, 42151, 42037, 41923, 41810, 41697, 41584,
    41472, 41360, 41248, 41136, 41025, 40914, 40804, 40693, 40583, 40473, 40364, 40255, 40146,
    40037, 39929, 39821, 39714, 39606, 39499, 39392, 39286, 39180, 39074, 38968, 38863, 38757,
    38653, 38548, 38444, 38340, 38236, 38133, 38030, 37927, 37824, 37722, 37620, 37518, 37417,
    37316, 37215, 37114, 37014, 36914, 36814, 36715, 36615, 36516, 36417, 36319, 36221, 36123,
    36025, 35928, 35831, 35734, 35637, 35541, 35445, 35349, 35253, 35158, 35063, 34968, 34874,
    34779, 34685, 34591, 34498, 34405, 34312, 34219, 34126, 34034, 33942, 33850, 33759, 33667,
    33576, 33486, 33395, 33305, 33215, 33125, 33035, 32946, 32857, 32768,
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fold_half_life_is_exact_to_one_half() {
        let params = DecayParams::default();
        assert_eq!(params.fold(1 << 20, 100, 239), 1 << 19);
    }

    #[test]
    fn test_fold_is_monotonic_and_bounded_by_the_floor() {
        let params = DecayParams::default();
        let mut previous = params.fold(1 << 24, 0, 0);
        for age in 1..=1_000u64 {
            let folded = params.fold(1 << 24, 0, age);
            assert!(folded <= previous);
            assert!(folded >= params.floor_q16);
            previous = folded;
        }
    }

    #[test]
    fn test_fold_treats_a_rewound_tick_as_unaged() {
        let params = DecayParams::default();
        assert_eq!(params.fold(9 << 16, 100, 40), 9 << 16);
    }

    #[test]
    fn test_fold_saturates_an_extreme_tick_gap_at_the_floor() {
        let params = DecayParams::default();
        assert_eq!(params.fold(9 << 16, 0, u64::MAX), params.floor_q16);
    }

    #[test]
    fn test_effective_freq_saturates_and_floors() {
        let params = DecayParams::default();
        assert_eq!(params.effective_freq(7, 4 << 16), 4);
        assert_eq!(params.effective_freq(3, 100 << 16), 3);
        assert_eq!(params.effective_freq(8, (1 << 16) - 1), 0);
    }

    #[test]
    fn test_new_refuses_invalid_parameters() {
        assert!(DecayParams::new(0, 1 << 16).is_err());
        assert!(DecayParams::new(139, -1).is_err());
        assert_eq!(
            DecayParams::new(139, 1 << 16).expect("the defaults are legal"),
            DecayParams::default()
        );
    }
}
