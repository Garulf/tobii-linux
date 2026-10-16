//! Vectors the emulated original routines produced (tools/*_oracle.py), and the installed
//! model's fitting behaviour from the Python suite.

use std::path::{Path, PathBuf};

use tobii_headpose::fit::{FitMode, mode_step, prepare_observations, rigid_step};
use tobii_headpose::model::{load_pose_model, select_pose_landmarks};
use tobii_headpose::policy::fit_staged_landmarks;
use tobii_headpose::projection::{Deformation, model_jacobian, project_points};
use tobii_headpose::temporal::TemporalPoseFitter;
use tobii_headpose::{PoseModel, Vec3};

fn bits(line: &str) -> Vec<f32> {
    line.split_whitespace().map(|word| f32::from_bits(u32::from_str_radix(word, 16).unwrap())).collect()
}

fn rows<const N: usize>(values: &[f32]) -> Vec<[f32; N]> {
    values.as_chunks::<N>().0.to_vec()
}

#[test]
fn rigid_step_reproduces_the_original_update() {
    let data = include_str!("data/rigid_fit_original.txt");
    let mut lines = data.lines().filter(|l| !l.starts_with('#'));
    let points: Vec<Vec3> = rows(&bits(lines.next().unwrap().strip_prefix("points ").unwrap()));
    let mut exact = 0;
    while let Some(case) = lines.next() {
        assert!(case.starts_with("case "));
        let mut field = |name: &str| bits(lines.next().unwrap().strip_prefix(name).unwrap());
        let observations = rows::<2>(&field("observations "));
        let state_sensitivity = field("state_sensitivity ");
        let observation_sensitivity = field("observation_sensitivity ");
        let mut state = [0.0, 0.0, 0.0, 0.0, 0.0, 6.0];
        for _ in 0..3 {
            let line = lines.next().unwrap();
            let (kind, values) = line.strip_prefix("original ").unwrap().split_once(' ').unwrap();
            let original: [f32; 6] = bits(values).try_into().unwrap();
            let updated = rigid_step(
                &points,
                &observations,
                &state,
                1.7,
                0.8,
                Some(&state_sensitivity),
                Some(&observation_sensitivity),
            )
            .unwrap();
            if kind == "exact" {
                assert_eq!(updated, original, "{case}");
                exact += 1;
            } else {
                assert!(updated.iter().zip(original).all(|(a, b)| (a - b).abs() <= 1e-5), "{case}");
            }
            state = original;
        }
    }
    assert_eq!(exact, 11);
}

fn support() -> Option<PathBuf> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../models/tobii-et5/support");
    path.is_dir().then_some(path)
}

/// Normalized D1 points of the model posed at `angles` and `translation`, at 640x480.
fn synthetic_frame(model: &PoseModel, angles: Vec3, translation: Vec3) -> Vec<[f32; 2]> {
    let target = model
        .initial_state(&[angles[0], angles[1], angles[2], translation[0], translation[1], translation[2]])
        .unwrap();
    let block = |basis, latent, ranges| Deformation { basis, latent, ranges, mask: None, fixed: None };
    let (_, projected) = model_jacobian(
        &model.vertices,
        angles,
        translation,
        model.focal_factor,
        1.0,
        block(&model.shape_basis, &target[6..46], &model.shape_ranges),
        block(&model.animation_basis, &target[46..], &model.animation_ranges),
    )
    .unwrap();
    let aspect = 640.0f32 / 480.0;
    projected.iter().map(|&[x, y]| [(1.0 - x / aspect) * 0.5, (1.0 - y) * 0.5]).collect()
}

#[test]
fn installed_model_loads_and_fits_a_neutral_face() {
    let Some(support) = support() else { return };
    let model = load_pose_model(&support).unwrap();
    assert_eq!((model.vertices.len(), model.state_count()), (57, 69));
    for (k, enabled) in model.shape_mask.iter().enumerate() {
        if !enabled {
            assert!((0..57).all(|p| (0..3).all(|a| model.shape_basis.get(p, a, k) == 0.0)));
        }
    }
    let pi = std::f32::consts::PI;
    let (projected, _) =
        project_points(&model.vertices, [0.0, pi, 0.0], [0.0, 0.0, 0.65], model.focal_factor, 1.0).unwrap();
    let points: Vec<[f32; 2]> =
        projected.iter().map(|&[x, y]| [(1.0 - x / (640.0 / 480.0)) * 0.5, (1.0 - y) * 0.5]).collect();
    let fit = fit_staged_landmarks(&model, &points, 640, 480).unwrap();
    assert!(fit.valid);
    assert_eq!(
        fit.stages.iter().map(|s| (s.mode, s.updates)).collect::<Vec<_>>(),
        [(FitMode::Shape, 2), (FitMode::Animation, 1)]
    );
    assert!((fit.rigid_state[1] - pi).abs() < 0.02);
    let animation: Vec<f32> = (0..23).map(|i| -0.02 + 0.04 * i as f32 / 22.0).collect();
    let (observations, _) =
        project_points(&model.vertices, [0.0, pi, 0.0], [0.0, 0.0, 0.65], model.focal_factor, 1.0).unwrap();
    let step = mode_step(
        &model,
        &observations,
        &[0.0, pi, 0.0, 0.0, 0.0, 0.65],
        &[0.0; 40],
        &animation,
        FitMode::Shape,
        None,
        None,
        Default::default(),
    )
    .unwrap();
    assert_eq!(step.active_state.len(), 46);
    assert_eq!(step.animation_coefficients, animation);
    let (selected, order) =
        select_pose_landmarks(&model, &[model.landmark_ids.clone(), vec![(13, 17), (2, 1)]].concat()).unwrap();
    let mut sorted = model.landmark_ids.clone();
    sorted.sort_unstable();
    assert_eq!(selected.landmark_ids, sorted);
    assert_eq!(selected.vertices, order.iter().map(|&i| model.vertices[i]).collect::<Vec<_>>());
    let (_, weights) = prepare_observations(&vec![[0.5; 2]; 57], &model.landmark_ids, 640, 480).unwrap();
    assert_eq!(weights.iter().filter(|&&w| w == 100001.0).count(), 4);
}

#[test]
fn temporal_fitter_tracks_and_refits_when_frontal() {
    let Some(support) = support() else { return };
    let model = load_pose_model(&support).unwrap();
    let pi = std::f32::consts::PI;
    let mut fitter = TemporalPoseFitter::new(model.clone()).unwrap();
    let modes = |fit: &tobii_headpose::TemporalFit| fit.trace.iter().map(|t| (t.mode, t.valid)).collect::<Vec<_>>();
    let first = fitter.process(&synthetic_frame(&model, [0.02, pi + 0.3, 0.01], [0.01, 0.0, 0.65]), 640, 480).unwrap();
    assert_eq!(modes(&first), [(FitMode::Shape, true), (FitMode::Animation, true)]);
    assert!(first.valid && first.initialised && !first.frontal);
    assert!(first.rotation.iter().zip([0.02, 0.3, 0.01]).all(|(a, b)| (a - b).abs() < 0.01));
    let steady =
        fitter.process(&synthetic_frame(&model, [0.02, pi + 0.32, 0.01], [0.012, 0.0, 0.65]), 640, 480).unwrap();
    assert_eq!(modes(&steady), [(FitMode::Animation, true)]);
    fitter.process(&synthetic_frame(&model, [0.02, pi + 0.18, 0.01], [0.01, 0.0, 0.65]), 640, 480).unwrap();
    let frontal = fitter.process(&synthetic_frame(&model, [0.0, pi + 0.04, 0.0], [0.01, 0.0, 0.65]), 640, 480).unwrap();
    assert_eq!(modes(&frontal), [(FitMode::Animation, true), (FitMode::Shape, true), (FitMode::Animation, true)]);
    assert!(frontal.frontal);
    fitter.reset();
    assert!(fitter.fit_mode.is_none() && !fitter.initialised && fitter.shape.iter().all(|&c| c == 0.0));
}
