//! Every frame of a recorded session through the demo's processor and the head-pose command's
//! processor, printed as the browser events and JSON lines they produce, for comparison with
//! the Python package: `replay_events LOG [MODELS]`.

use std::path::PathBuf;

use tobii_linux::demo::events::{frame_event, pose_event};
use tobii_linux::demo::pipeline::{DemoProcessor, ModelDemoProcessor};
use tobii_linux::demo::sources::recorded_frames;
use tobii_linux::pyjson::{compact, dumps};
use tobii_linux::tracker::{FrameProcessor, ModelProcessor};
use tobii_protocol::frames::ImageFrame;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let log = PathBuf::from(args.next().ok_or("usage: replay_events LOG [MODELS]")?);
    let models = args.next().map_or_else(tobii_linux::paths::default_models, PathBuf::from);
    let corners = tobii_linux::cli::corners(&tobii_linux::demo::ALBEDO_DISPLAY);
    let area = tobii_linux::cli::display_area(&corners);
    let (calibration, rows) = recorded_frames(&log, None)?;
    let calibration = calibration.ok_or("the log has no calibration")?;
    let mut demo = ModelDemoProcessor::load(&models, area)?;
    let mut headpose = ModelProcessor::load(&models, area)?;
    demo.calibrate(&calibration);
    headpose.start(&calibration);
    println!("topology {}", compact(&demo.topology()));
    for (index, (timestamp, width, height, path)) in rows.into_iter().enumerate() {
        let frame = ImageFrame { timestamp, pixel_format: 8, width, height, pixels: std::fs::read(path)? };
        let processed = demo.process(&frame)?;
        let state = Some(demo.tracker_state());
        let pose = pose_event(&processed.sample, &processed.tracked, state, 0.0, processed.mesh_millimetres.as_deref());
        let mut overlay =
            frame_event(index as u64, &frame, Some(&processed.tracked), state, processed.mesh_pixels.as_deref())?;
        if let tobii_linux::pyjson::Json::Object(entries) = &mut overlay {
            entries.retain(|(key, _)| key != "png");
        }
        println!("pose {}", compact(&pose));
        println!("frame {}", compact(&overlay));
        println!("headpose {}", dumps(&tobii_linux::cli::sample_json(&headpose.process(&frame)?)));
    }
    Ok(())
}
