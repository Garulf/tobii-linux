//! Recovered yaw-dependent LBF refinement 0x14007d270 that follows a successful fit.
//!
//! VNNTrackerCore resets the tracker when this step returns false. The step fails only
//! when the working image carries an IplImage ROI that does not fit inside the image
//! (0x1400ac9c0); otherwise the four LBF stages always run. Refined coordinates are
//! written back only for points of FDP group 10.

use tobii_models::LbfModel;

use crate::error::{Result, invalid};
use crate::lbf::{Image, refine_landmarks};
use crate::model::FdpId;

pub const MIRROR_YAW_DEGREES: f32 = 15.0;
pub const REFINED_GROUP: u32 = 10;
pub const REFINED_DEPTH: f32 = 1000.0;
pub const REFINED_QUALITY: f32 = -1.0;
pub const NEUTRAL_YAW_DEGREES: f32 = 180.0;
pub const RADIANS_TO_DEGREES: f32 = 57.2957763671875;

const NONE: i32 = -1;

/// 0x140011900 for every FDP id (index 1..count per group); -1 means no mirror.
///
/// Tabulated by executing the original function over the runtime's FDP count table at
/// 0x14112f050; tools/eye_refine_oracle.py rechecks every entry.
pub const MIRRORED_INDEX: [(u32, &[i32]); 14] = [
    (2, &[1, 2, 3, 5, 4, 7, 6, 9, 8, NONE, NONE, NONE, NONE, NONE]),
    (3, &[2, 1, 4, 3, 6, 5, 12, 11, 10, 9, 8, 7, 14, 13]),
    (4, &[2, 1, 4, 3, 6, 5]),
    (5, &[NONE; 4]),
    (6, &[NONE; 4]),
    (7, &[NONE]),
    (8, &[1, 2, 4, 3, 6, 5, 8, 7, 10, 9]),
    (9, &[2, 1, 3, 5, 4, NONE, NONE, NONE, NONE, NONE, NONE, NONE, NONE, NONE, 15]),
    (10, &[2, 1, 4, 3, 6, 5, 8, 7, 10, 9, 12, 11, 14, 13, 16, 15, 18, 17, 20, 19, 22, 21, 24, 23]),
    (11, &[NONE; 6]),
    (
        12,
        &[
            NONE, NONE, NONE, NONE, 10, 9, 12, 11, 6, 5, 8, 7, NONE, NONE, NONE, NONE, NONE, NONE, NONE, NONE, NONE,
            NONE, NONE, NONE, NONE, NONE, NONE, NONE, NONE, NONE, NONE, NONE, NONE, NONE, NONE, NONE, NONE, NONE, NONE,
            NONE, NONE, NONE, NONE, NONE, NONE, NONE, NONE,
        ],
    ),
    (13, &[2, 1, 4, 3, 6, 5, 8, 7, 10, 9, 12, 11, 14, 13, 16, 15, 17]),
    (14, &[2, 1, 4, 3, 6, 5, 8, 7, 10, 9, 12, 11, 14, 13, 16, 15, 18, 17, 20, 19, 21, 22, 23, 24, 25, NONE]),
    (15, &[2, 1, 4, 3, 6, 5, 8, 7, 10, 9, 12, 11, 14, 13, 16, 15, 17]),
];

/// The mirrored index of an FDP id, or -1 when it has none.
pub fn mirrored_index(group: u32, index: u32) -> Result<i32> {
    let table = MIRRORED_INDEX.iter().find(|(g, _)| *g == group).map(|(_, table)| *table);
    match table.and_then(|table| table.get((index as usize).wrapping_sub(1))) {
        Some(&mirrored) => Ok(mirrored),
        None => invalid("FDP id outside the runtime count table"),
    }
}

/// Yaw passed by 0x140049450: `180 - (ry - atan2(tx, tz))` in degrees, float32.
pub fn tracker_yaw_degrees(rigid_state: &[f32; 6]) -> f32 {
    let bearing = f64::from(rigid_state[3]).atan2(f64::from(rigid_state[5])) as f32;
    NEUTRAL_YAW_DEGREES - (rigid_state[1] - bearing) * RADIANS_TO_DEGREES
}

/// The two installed LBF models.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LbfName {
    Fa,
    Fc,
}

impl LbfName {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Fa => "fa",
            Self::Fc => "fc",
        }
    }
}

/// `(model, mirrored)`. COMISS makes a NaN yaw select fa unmirrored.
pub fn select_lbf_model(yaw_degrees: f32) -> (LbfName, bool) {
    let mirrored = yaw_degrees > MIRROR_YAW_DEGREES;
    if yaw_degrees.abs() >= MIRROR_YAW_DEGREES { (LbfName::Fc, mirrored) } else { (LbfName::Fa, false) }
}

/// An IplImage region of interest `(x, y, width, height)`.
pub type Roi = [i64; 4];

/// The 0x1400ac9c0 guard; `None` is no ROI.
pub fn roi_inside(width: i64, height: i64, roi: Option<Roi>) -> bool {
    let Some([x, y, roi_width, roi_height]) = roi else {
        return true;
    };
    !(x < 0 || x + roi_width > width || y < 0 || y + roi_height > height)
}

/// The record fields the refinement writes: `(x, y, z, present, flag, quality)`.
pub type RefinedRecord = (f32, f32, f32, i32, i32, f32);

/// The LBF inputs of 0x14007d270: the model, whether to mirror, and the pixel coordinates
/// of `point_ids` (fa's ID order), read from the mirrored record when mirrored.
pub fn lbf_inputs(
    point_ids: &[FdpId],
    record: impl Fn(FdpId) -> Option<[f32; 2]>,
    width: i64,
    height: i64,
    yaw_degrees: f32,
) -> Result<(LbfName, bool, Vec<f32>, Vec<f32>)> {
    let (name, mirrored) = select_lbf_model(yaw_degrees);
    let (w, h) = (width as f64 as f32, height as f64 as f32);
    let (mut xs, mut ys) = (vec![], vec![]);
    for &(group, index) in point_ids {
        let source = if mirrored { mirrored_index(group, index)? } else { index as i32 };
        let Some([x, y]) = u32::try_from(source).ok().and_then(|source| record((group, source))) else {
            return invalid(format!("Missing FDP record ({group}, {source})"));
        };
        xs.push(w * x);
        ys.push(y * h);
    }
    Ok((name, mirrored, xs, ys))
}

/// The group-10 records written back from refined pixel coordinates, in point order.
pub fn refined_records(
    point_ids: &[FdpId],
    xs: &[f32],
    ys: &[f32],
    width: i64,
    height: i64,
    mirrored: bool,
) -> Result<Vec<(FdpId, RefinedRecord)>> {
    let (w, h) = (width as f64 as f32, height as f64 as f32);
    let mut written: Vec<(FdpId, RefinedRecord)> = vec![];
    for ((&(group, index), &x), &y) in point_ids.iter().zip(xs).zip(ys) {
        if group != REFINED_GROUP {
            continue;
        }
        let target = if mirrored { mirrored_index(group, index)? } else { index as i32 };
        let Ok(target) = u32::try_from(target) else {
            return invalid("Refined point has no mirrored record");
        };
        let record = (x / w, y / h, REFINED_DEPTH, 1, 1, REFINED_QUALITY);
        match written.iter_mut().find(|(id, _)| *id == (group, target)) {
            Some(entry) => entry.1 = record,
            None => written.push(((group, target), record)),
        }
    }
    Ok(written)
}

/// Runs the four LBF stages (0x1400acb80) on an 8-bit `height x width` image.
///
/// The ROI, when present, offsets the pixel origin and replaces the dimensions while the
/// landmark coordinates stay in full-image pixels, as in the original.
#[allow(clippy::too_many_arguments, reason = "mirrors the recovered routine's inputs")]
pub fn refine_region(
    model: &LbfModel,
    pixels: &[u8],
    width: i64,
    xs: &[f32],
    ys: &[f32],
    scale: f32,
    mirrored: bool,
    roi: Option<Roi>,
) -> Result<(Vec<f32>, Vec<f32>)> {
    let height = if width > 0 { pixels.len() as i64 / width } else { 0 };
    let [x, y, columns, rows] = roi.unwrap_or([0, 0, width, height]);
    let mut region = Vec::with_capacity(pixels.len());
    for row in y.max(0)..(y + rows).min(height) {
        let start = (row * width + x.clamp(0, width)) as usize;
        let end = (row * width + (x + columns).clamp(0, width)) as usize;
        region.extend_from_slice(&pixels[start..end.max(start)]);
    }
    let image = Image { pixels: &region, rows, columns, stride: columns };
    let centers: Vec<[f64; 2]> = ys.iter().zip(xs).map(|(&y, &x)| [f64::from(y), f64::from(x)]).collect();
    let refined = refine_landmarks(model, image, &centers, scale, mirrored)?;
    Ok((refined.iter().map(|p| p[1]).collect(), refined.iter().map(|p| p[0]).collect()))
}

/// The result of [`refine_eye_landmarks`]: false when the ROI guard failed, and the group-10
/// records to write.
pub type EyeRefinement = (bool, Vec<(FdpId, RefinedRecord)>);

/// 0x14007d270 for the LBF point table `point_ids` (fa's ID order).
///
/// `record` gives the normalized `(x, y)` of a present FDP record; every point must be
/// present because the original reads absent records' stale memory. `refine` runs the
/// selected model on pixel coordinates, as [`refine_region`] does.
#[allow(clippy::too_many_arguments, reason = "mirrors the recovered routine's inputs")]
pub fn refine_eye_landmarks(
    point_ids: &[FdpId],
    record: impl Fn(FdpId) -> Option<[f32; 2]>,
    width: i64,
    height: i64,
    scale: f32,
    yaw_degrees: f32,
    refine: impl FnOnce(LbfName, Vec<f32>, Vec<f32>, f32, bool, Option<Roi>) -> Result<(Vec<f32>, Vec<f32>)>,
    roi: Option<Roi>,
) -> Result<EyeRefinement> {
    let (name, mirrored, xs, ys) = lbf_inputs(point_ids, record, width, height, yaw_degrees)?;
    if !roi_inside(width, height, roi) {
        return Ok((false, vec![]));
    }
    let (xs, ys) = refine(name, xs, ys, scale, mirrored && name == LbfName::Fc, roi)?;
    Ok((true, refined_records(point_ids, &xs, &ys, width, height, mirrored)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_selection_thresholds() {
        assert_eq!(select_lbf_model(0.0), (LbfName::Fa, false));
        assert_eq!(select_lbf_model(15.0), (LbfName::Fc, false));
        assert_eq!(select_lbf_model(15.001), (LbfName::Fc, true));
        assert_eq!(select_lbf_model(-20.0), (LbfName::Fc, false));
        assert_eq!(select_lbf_model(f32::NAN), (LbfName::Fa, false));
    }

    #[test]
    fn roi_guard() {
        assert!(roi_inside(240, 200, None));
        assert!(roi_inside(240, 200, Some([40, 20, 200, 180])));
        assert!(!roi_inside(240, 200, Some([41, 20, 200, 180])));
        assert!(!roi_inside(240, 200, Some([0, -1, 10, 10])));
    }

    #[test]
    fn mirror_map_pairs_sides() {
        assert_eq!(mirrored_index(3, 7).unwrap(), 12);
        assert_eq!(mirrored_index(3, 8).unwrap(), 11);
        assert_eq!(mirrored_index(9, 15).unwrap(), 15);
        assert_eq!(mirrored_index(5, 1).unwrap(), -1);
        for (_, table) in MIRRORED_INDEX {
            for (index, &mirrored) in table.iter().enumerate() {
                if mirrored != -1 {
                    assert_eq!(table[mirrored as usize - 1], index as i32 + 1);
                }
            }
        }
        assert!(mirrored_index(3, 15).is_err());
        assert!(mirrored_index(3, 0).is_err());
    }

    #[test]
    fn group_10_writes_and_roi_failure() {
        let ids = [(3, 7), (10, 1), (10, 2)];
        let records = [((3, 7), [0.1, 0.2]), ((3, 12), [0.3, 0.4]), ((10, 1), [0.5, 0.5]), ((10, 2), [0.25, 0.75])];
        let record = |id| records.iter().find(|(known, _)| *known == id).map(|(_, p)| *p);
        let shift = |_, xs: Vec<f32>, ys: Vec<f32>, _, _, _| {
            Ok((xs.iter().map(|x| x + 4.0).collect(), ys.iter().map(|y| y + 2.0).collect()))
        };
        let (valid, written) = refine_eye_landmarks(&ids, record, 200, 100, 80.0, 0.0, shift, None).unwrap();
        assert!(valid);
        assert_eq!(written.iter().map(|w| w.0).collect::<Vec<_>>(), [(10, 1), (10, 2)]);
        assert_eq!(written[0].1, (0.52, 0.52, 1000.0, 1, 1, -1.0));
        let (valid, written) =
            refine_eye_landmarks(&ids, record, 200, 100, 80.0, 0.0, shift, Some([150, 0, 60, 50])).unwrap();
        assert!(!valid && written.is_empty());
    }

    #[test]
    fn tracker_yaw_is_zero_for_neutral_pose() {
        assert!(tracker_yaw_degrees(&[0.0, std::f32::consts::PI, 0.0, 0.0, 0.0, 0.65]).abs() < 1e-4);
    }
}
