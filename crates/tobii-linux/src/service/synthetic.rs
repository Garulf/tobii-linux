//! A synthetic tracker for runs without hardware: a slow head sweep and a gaze figure eight.

use std::time::Duration;

use tobii_headpose::pipeline::HeadPoseSample;
use tobii_protocol::gaze::{DeviceGaze, GazeFrame};
use tobii_protocol::numeric::round_f32;

use super::PrpService;
use super::clock::now_us;

/// A slow head sweep: yaw and pitch sinusoids with a small positional drift.
pub fn synthetic_sample(timestamp_us: i64, start_us: i64) -> HeadPoseSample {
    let seconds = (timestamp_us - start_us) as f64 / 1e6;
    let tau = std::f64::consts::TAU;
    let yaw = 0.35 * (tau * seconds / 6.0).sin();
    let pitch = 0.15 * (tau * seconds / 4.0).sin();
    let position = [20.0 * (tau * seconds / 8.0).sin(), 10.0, 600.0];
    HeadPoseSample {
        timestamp_us,
        valid: true,
        position_mm: position.map(|v| v as f32),
        rotation_rad: [pitch as f32, yaw as f32, 0.0],
    }
}

/// A gaze point tracing a slow figure eight over the display, both eyes valid.
pub fn synthetic_gaze_frame(timestamp_us: i64, start_us: i64) -> GazeFrame {
    let seconds = (timestamp_us - start_us) as f64 / 1e6;
    let pi = std::f64::consts::PI;
    let x = 0.5 + 0.35 * (2.0 * pi * seconds / 10.0).sin();
    let y = 0.5 + 0.3 * (4.0 * pi * seconds / 10.0).sin();
    let point = [round_f32(x), round_f32(y)];
    let triple = |values: [f64; 3]| values.map(round_f32);
    GazeFrame::new(DeviceGaze {
        timestamp: timestamp_us,
        combined_point: point,
        combined_validity: 1,
        responsive_point: point,
        responsive_validity: 1,
        left_gaze_point_2d: point,
        right_gaze_point_2d: point,
        left_validity: 0,
        right_validity: 0,
        left_origin_display: triple([-32.0, 20.0, 620.0]),
        right_origin_display: triple([32.0, 20.0, 620.0]),
        left_origin_display_validity: 1,
        right_origin_display_validity: 1,
        left_position_guide: triple([0.55, 0.5, 0.45]),
        right_position_guide: triple([0.45, 0.5, 0.45]),
        left_position_guide_validity: 1,
        right_position_guide_validity: 1,
        ..DeviceGaze::default()
    })
}

/// Publishes synthetic head pose (and gaze, with a gaze hub) at the real service's cadence.
pub async fn publish_synthetic(service: PrpService, period: Duration) {
    let clock = std::sync::Arc::clone(service.clock());
    let start = now_us(&clock);
    let first_tick = tokio::time::Instant::now();
    let mut tick = 0;
    loop {
        tick += 1;
        let now = now_us(&clock);
        service.hub().publish(&synthetic_sample(now, start));
        if let Some(gaze) = service.gaze_hub() {
            gaze.publish(&synthetic_gaze_frame(now, start));
        }
        tokio::time::sleep_until(first_tick + period * tick).await;
    }
}
