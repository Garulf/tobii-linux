use super::*;
use crate::test_util::{models, runtime};

#[test]
fn region_expands_around_centre_and_clamps() {
    assert_eq!(expanded_region([120, 80, 60, 60], 320, 240, EXPANSION).unwrap(), [75, 35, 150, 150]);
    assert_eq!(expanded_region([10, 5, 70, 70], 320, 240, EXPANSION).unwrap(), [0, 0, 132, 127]);
}

/// Region and working face box of the original 0x140079ce0 on a 320×240 frame
/// (tools/face_tracking_oracle.py), with the working image size.
#[test]
fn working_views_match_the_original_adjustments() {
    let frame = vec![0u8; 320 * 240];
    for (face, region, working_face, working_size) in [
        ([120, 80, 60, 60], [75, 35, 150, 150], [72, 72, 96, 96], [240, 240]),
        ([10, 5, 70, 70], [0, 0, 132, 127], [14, 7, 96, 96], [181, 174]),
        ([250, 170, 64, 64], [202, 122, 118, 118], [72, 72, 96, 96], [177, 177]),
        ([130, 90, 97, 97], [57, 17, 242, 223], [72, 72, 96, 96], [240, 221]),
        ([100, 60, 41, 41], [69, 29, 102, 102], [73, 73, 96, 96], [239, 239]),
    ] {
        assert_eq!(expanded_region(face, 320, 240, EXPANSION).unwrap(), region, "{face:?}");
        let view = working_view(&frame, 320, 240, face, EXPANSION, TARGET_FACE_WIDTH).unwrap();
        assert_eq!((view.region, view.face, [view.width, view.height]), (region, working_face, working_size));
    }
}

#[test]
fn working_view_scales_face_to_target_width_and_back() {
    let view = working_view(&vec![0u8; 320 * 240], 320, 240, [120, 80, 60, 60], EXPANSION, 96).unwrap();
    assert_eq!((view.width, view.height, view.face), (240, 240, [72, 72, 96, 96]));
    assert_eq!(working_face_in_frame(view.face, view.region, view.ratio).unwrap(), [120, 80, 60, 60]);
    assert!(working_view(&[0; 100], 10, 10, [50, 50, 4, 4], EXPANSION, 96).is_err());
}

#[test]
fn points_map_back_to_frame() {
    let mapped = to_frame_points(&[[0.0, 0.0], [1.0, 1.0]], [75, 35, 150, 150], 320, 240);
    assert_eq!(mapped, [[75.0 / 320.0, 35.0 / 240.0], [225.0 / 320.0, 185.0 / 240.0]]);
}

#[test]
fn landmark_box_is_square_around_points() {
    assert_eq!(landmark_box(&[[0.25, 0.25], [0.5, 0.75]], 200, 100).unwrap(), [50, 25, 50, 50]);
    assert_eq!(landmark_box(&[[0.25, 0.25], [f32::NAN, 0.5], [0.5, 0.75]], 200, 100).unwrap(), [50, 25, 50, 50]);
    assert!(landmark_box(&[[0.5, f32::INFINITY]], 200, 100).is_err());
}

#[test]
fn upright_face_has_zero_roll() {
    let points = [[0.4, 0.3], [0.6, 0.3], [0.45, 0.6], [0.55, 0.6]];
    assert!(temporal_roll(&points, &ROLL_POINTS, 100, 100).unwrap().abs() < 1e-4);
    assert!(temporal_roll(&points[..3], &ROLL_POINTS, 100, 100).is_err());
}

#[test]
fn redetection_needs_overlap_above_a_tenth() {
    let detection =
        |x1: f32, side: f32| Detection { x1, y1: x1, x2: x1 + side, y2: x1 + side, score: 0.9, angles: [0.0; 3] };
    assert_eq!(overlap([10.0, 10.0, 19.0, 19.0], [10, 10, 9, 9]), 1.0);
    let found = [detection(100.0, 20.0), detection(12.0, 30.0), detection(10.0, 31.0)];
    assert_eq!(best_redetection(&found, [10, 10, 30, 30]), Some([10, 10, 31, 31]));
    assert_eq!(best_redetection(&found[..1], [10, 10, 30, 30]), None);
}

#[test]
fn core_records_carry_the_four_aliases() {
    let ids = [(15, 17), (9, 3), (14, 23)];
    let records = frame_records(&[[0.1, 0.2], [0.3, 0.4], [0.5, 0.6]], &[0.9, 0.8, 0.7], &ids).unwrap();
    assert_eq!((records[&(2, 1)].x, records[&(13, 17)].y), (0.1, 0.2));
    assert_eq!(records[&(14, 21)].quality, 0.8);
    assert_eq!(records[&(9, 12)].x, 0.5);
    assert!(records.values().all(|record| record.present));
    assert!(frame_records(&[[0.1, 0.2]], &[0.9], &[(15, 17)]).is_err());
}

#[test]
fn blank_frame_stays_searching() {
    let Some(models) = models() else { return };
    let Some(runtime) = runtime() else { return };
    let mut tracker = FaceTracker::new(&runtime, models, None, None).unwrap();
    assert_eq!(tracker.recovery_timeout_ms, 800);
    let tracked = tracker.process(&vec![0; 640 * 480], 640, 480, 0).unwrap();
    assert_eq!((tracker.state, tracked.state, tracked.face), (CoreState::Searching, CoreState::Searching, None));
    assert!(tracked.fit.is_none() && tracked.quality_score.is_nan());
    assert!(tracker.process(&[0; 10], 640, 480, 0).is_err());
}
