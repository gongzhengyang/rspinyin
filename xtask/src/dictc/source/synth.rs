//! The synthetic word generator: the probe corpus the timing and size runs measure.
//!
//! Responsibility: generate a fixed-seed corpus of two-character words drawn from the
//! CJK unified block, weighted so that roughly the same share falls inside the expansion
//! band as in a real source.
//!
//! Boundaries: this layer produces [`Word`]s and reads no file at all, so a probe run
//! needs only the L1 table. The generator is deterministic, which is what lets two probe
//! runs measure the same work.

use std::collections::BTreeMap;

use super::words::{baseline_key, compose_readings};

use super::Word;

/// Seed of the synthetic word generator; fixed so a probe run is reproducible.
const SYNTH_SEED: u64 = 0x5253_5044_0000_0001;

/// Width of the synthetic generator's weight range. Chosen so that about the same
/// share of synthetic words falls inside the band as in a real source.
const SYNTH_WEIGHT_SPAN: u64 = 116;

/// Generates `count` synthetic words for the timing and size probe.
///
/// The generator is a fixed-seed xorshift: the same count always produces the same
/// list, so two probe runs measure the same work. Words are two characters drawn
/// from the CJK unified block, weighted so that roughly the same share falls
/// inside the expansion band as in a real source.
pub(crate) fn synth_words(
    count: u32,
    l1: &BTreeMap<char, Vec<String>>,
    band_threshold: u32,
) -> Vec<Word> {
    let mut state = SYNTH_SEED;
    let mut words = Vec::with_capacity(count as usize);
    for _ in 0..count {
        let mut text = String::with_capacity(6);
        while text.chars().count() < 2 {
            let codepoint = 0x4E00 + (next_random(&mut state) % 3_000) as u32;
            if let Some(character) = char::from_u32(codepoint) {
                text.push(character);
            }
        }
        let weight = 1 + (next_random(&mut state) % SYNTH_WEIGHT_SPAN) as u32;
        let Some(readings) = compose_readings(&text, l1) else {
            continue;
        };
        let Some(key) = baseline_key(&readings) else {
            continue;
        };
        let polyphone = readings.iter().any(|readings| readings.len() > 1);
        words.push(Word {
            text,
            key,
            readings: if weight >= band_threshold {
                readings
            } else {
                Vec::new()
            },
            weight,
            flags: 0,
            polyphone,
        });
    }
    words
}

/// Advances the synthetic generator's xorshift state.
fn next_random(state: &mut u64) -> u64 {
    let mut value = *state;
    value ^= value >> 12;
    value ^= value << 25;
    value ^= value >> 27;
    *state = value;
    value.wrapping_mul(0x2545_F491_4F6C_DD1D)
}
