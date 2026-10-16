//! Confidence of complete D1 records (0x14007a880 after the alias filling of 0x140055a20)
//! and the observed caller gate (0x1400495d4).

use std::collections::BTreeMap;

use crate::error::{Result, value};

/// The 57 FDP record identifiers the D1 landmarks fill, in sorted order.
#[rustfmt::skip]
pub const D1_IDS: [(i64, i64); 57] = [
    (2, 2), (2, 3),
    (3, 1), (3, 2), (3, 3), (3, 4), (3, 5), (3, 6), (3, 7), (3, 8), (3, 9), (3, 10), (3, 11), (3, 12), (3, 13), (3, 14),
    (4, 1), (4, 2), (4, 3), (4, 4), (4, 5), (4, 6),
    (8, 1), (8, 2), (8, 3), (8, 4), (8, 5), (8, 6), (8, 7), (8, 8), (8, 9), (8, 10),
    (9, 3), (9, 4), (9, 5), (9, 15),
    (14, 22), (14, 23), (14, 24), (14, 25),
    (15, 1), (15, 2), (15, 3), (15, 4), (15, 5), (15, 6), (15, 7), (15, 8), (15, 9), (15, 10), (15, 11), (15, 12),
    (15, 13), (15, 14), (15, 15), (15, 16), (15, 17),
];

/// Records the FDP wrapper fills from others before averaging: `(alias, source)`.
const ALIASES: [((i64, i64), (i64, i64)); 4] =
    [((2, 1), (15, 17)), ((13, 17), (15, 17)), ((14, 21), (9, 3)), ((9, 12), (14, 23))];

pub const QUALITY_MESSAGE: &str = "Expected 57 finite complete D1 quality records";

/// The mean of the nonnegative qualities in FDP order, after the four aliases are filled,
/// so not the plain mean of the 57 network scores. NaN when none is nonnegative. Complete
/// D1 records only: every identifier exactly once, finite values.
pub fn d1_quality_score(quality: &[f32], landmark_ids: &[(i64, i64)]) -> Result<f32> {
    let mut records: BTreeMap<(i64, i64), f32> = BTreeMap::new();
    if quality.len() == D1_IDS.len() && landmark_ids.len() == D1_IDS.len() {
        records.extend(landmark_ids.iter().copied().zip(quality.iter().copied()));
    }
    let complete = records.len() == D1_IDS.len() && D1_IDS.iter().all(|id| records.contains_key(id));
    if !complete || !quality.iter().all(|q| q.is_finite()) {
        return value(QUALITY_MESSAGE);
    }
    for (alias, source) in ALIASES {
        let copied = records[&source];
        records.insert(alias, copied);
    }
    let mut total = 0.0f32;
    let mut count = 0u32;
    for &quality in records.values().filter(|&&q| q >= 0.0) {
        total += quality;
        count += 1;
    }
    Ok(if count == 0 { f32::NAN } else { total / count as f32 })
}

/// The strict gate: modes 0 and 1 need a score above 0.5, mode 2 above float32(0.35).
/// Other modes fail. Passing it does not establish tracking validity or a fitted pose.
pub fn passes_d1_quality(score: f32, caller_mode: i64) -> bool {
    match caller_mode {
        0 | 1 => score > 0.5,
        2 => score > 0.35,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alias_scores_are_included_in_fdp_order() {
        let mut values = [0.0f32; 57];
        values[D1_IDS.iter().position(|&id| id == (15, 17)).unwrap()] = 1.0;
        let score = d1_quality_score(&values, &D1_IDS).unwrap();
        assert!((f64::from(score) - 3.0 / 61.0).abs() < 1e-7);
    }

    #[test]
    fn negative_records_are_skipped_and_all_negative_is_nan() {
        assert!(d1_quality_score(&[-1.0; 57], &D1_IDS).unwrap().is_nan());
        let mut values = [-1.0f32; 57];
        values[0] = 0.5;
        assert_eq!(d1_quality_score(&values, &D1_IDS).unwrap(), 0.5);
    }

    #[test]
    fn incomplete_or_nonfinite_records_are_rejected() {
        let mut ids = D1_IDS;
        ids[0] = ids[1];
        assert!(d1_quality_score(&[0.0; 57], &ids).is_err());
        assert!(d1_quality_score(&[0.0; 56], &D1_IDS[..56]).is_err());
        let mut values = [0.0f32; 57];
        values[3] = f32::NAN;
        assert!(d1_quality_score(&values, &D1_IDS).is_err());
    }

    /// tools/d1_quality_oracle.py's inputs in landmarks.bin order and the scores of the
    /// original alias filling 0x140055a20 and aggregation 0x14007a880.
    #[test]
    fn scores_match_the_original_aggregation() {
        let path =
            concat!(env!("CARGO_MANIFEST_DIR"), "/../../models/tobii-et5/support/bdtsdata/NN/vnn_nir/landmarks.bin");
        let ids: Vec<(i64, i64)> = tobii_models::face::read_landmark_ids(std::path::Path::new(path))
            .unwrap()
            .into_iter()
            .map(|(g, i)| (i64::from(g), i64::from(i)))
            .collect();
        let ramp = |start: f64, stop: f64| -> Vec<f32> {
            (0..57)
                .map(|i| if i == 56 { stop } else { f64::from(i) * ((stop - start) / 56.0) + start } as f32)
                .collect()
        };
        for (quality, expected) in [
            (vec![0.0; 57], 0.0),
            (vec![0.5; 57], 0.5),
            (ramp(0.0, 1.0), 0.492_974_221_706_390_4),
            (ramp(-1.0, 2.0), 1.031_593_561_172_485_4),
        ] {
            assert_eq!(f64::from(d1_quality_score(&quality, &ids).unwrap()), expected);
        }
        assert!(d1_quality_score(&[-1.0; 57], &ids).unwrap().is_nan());
    }

    #[test]
    fn quality_thresholds_are_strict() {
        for (mode, threshold) in [(0, 0.5f32), (1, 0.5), (2, 0.35)] {
            assert!(!passes_d1_quality(threshold, mode));
            assert!(passes_d1_quality(f32::from_bits(threshold.to_bits() + 1), mode));
        }
        assert!(!passes_d1_quality(f32::NAN, 2));
        assert!(!passes_d1_quality(1.0, 3));
    }
}
