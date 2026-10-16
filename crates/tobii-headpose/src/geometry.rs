//! Head-pose geometry stage recovered from the platform algorithm module.
//!
//! Matrices are float32 4x4 or 3x3 arrays in the original's memory order and are applied
//! to row vectors, as the stage itself does.

use crate::array::Vec3;
use crate::output::{Matrix3, PI, euler_matrix, matrix_euler};

pub type Matrix4 = [[f32; 4]; 4];

/// Divides by the length when the squared length (y, z first) is positive.
fn normalized(vector: Vec3) -> Vec3 {
    let [x, y, z] = vector;
    let squared = (y * y + z * z) + x * x;
    if squared > 0.0 {
        let length = squared.sqrt();
        return vector.map(|v| v / length);
    }
    vector
}

/// Three-term dot product summed as `(1 + 2) + 0`, the order used throughout the stage.
fn dot(row: Vec3, vector: Vec3) -> f32 {
    (row[1] * vector[1] + row[2] * vector[2]) + row[0] * vector[0]
}

fn sincos(angle: f32) -> (f32, f32) {
    (f64::from(angle).cos() as f32, f64::from(angle).sin() as f32)
}

/// 0x1402b10a0.
pub fn x_rotation(angle: f32) -> Matrix3 {
    let (c, s) = sincos(angle);
    [[1.0, 0.0, 0.0], [0.0, c, s], [0.0, -s, c]]
}

/// 0x1402b1120.
pub fn y_rotation(angle: f32) -> Matrix3 {
    let (c, s) = sincos(angle);
    [[c, 0.0, -s], [0.0, 1.0, 0.0], [s, 0.0, c]]
}

/// 0x1402b11a0.
pub fn z_rotation(angle: f32) -> Matrix3 {
    let (c, s) = sincos(angle);
    [[c, s, 0.0], [-s, c, 0.0], [0.0, 0.0, 1.0]]
}

fn homogeneous(rotation: Matrix3, translation: Vec3) -> Matrix4 {
    let mut matrix = [[0.0; 4]; 4];
    for (row, values) in matrix.iter_mut().zip(rotation) {
        row[..3].copy_from_slice(&values);
    }
    matrix[3] = [translation[0], translation[1], translation[2], 1.0];
    matrix
}

/// Constant settings of 0x14028fc30: the rotation 3x3 and the position 4x4.
pub fn geometry_config() -> (Matrix3, Matrix4) {
    (x_rotation(PI), homogeneous(z_rotation(PI), [0.0; 3]))
}

fn column3(matrix: &Matrix3, j: usize) -> Vec3 {
    [matrix[0][j], matrix[1][j], matrix[2][j]]
}

/// 3x3 product with each entry summed as `(1 + 2) + 0`, as 0x140294ba0.
fn product3(left: &Matrix3, right: &Matrix3) -> Matrix3 {
    std::array::from_fn(|i| std::array::from_fn(|j| dot(left[i], column3(right, j))))
}

/// 4x4 product with each entry summed as `((0 + 1) + 2) + 3`, as 0x140294820.
fn product4(left: &Matrix4, right: &Matrix4) -> Matrix4 {
    std::array::from_fn(|i| {
        std::array::from_fn(|j| {
            let (a, b) = (left[i], [right[0][j], right[1][j], right[2][j], right[3][j]]);
            ((a[1] * b[1] + a[0] * b[0]) + a[2] * b[2]) + a[3] * b[3]
        })
    })
}

fn subtract(a: Vec3, b: Vec3) -> Vec3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

/// 0x1402af610: unit normal `(top_right - top_left) x (top_left - bottom_left)`.
pub fn display_normal(top_left: Vec3, bottom_left: Vec3, top_right: Vec3) -> Vec3 {
    let up = subtract(top_left, bottom_left);
    let right = subtract(top_right, top_left);
    normalized([
        right[1] * up[2] - right[2] * up[1],
        right[2] * up[0] - up[2] * right[0],
        up[1] * right[0] - right[1] * up[0],
    ])
}

/// Display object slot +0x28 (0x1402afad0).
pub fn display_center(top_left: Vec3, bottom_left: Vec3, top_right: Vec3) -> Vec3 {
    let half_up = subtract(top_left, bottom_left).map(|v| v * 0.5);
    let half_right = subtract(top_right, top_left).map(|v| v * 0.5);
    std::array::from_fn(|i| (half_up[i] + bottom_left[i]) + half_right[i])
}

/// 0x140294230: maps a point in tracker millimetres to display-centred axes.
///
/// The axes are right (top_left to top_right), up (bottom_left to top_left) and the
/// display normal, so `[p, 1] @ matrix` gives `((p - c).right, (p - c).up, (p - c).normal)`
/// for display centre `c`.
pub fn display_matrix(top_left: Vec3, bottom_left: Vec3, top_right: Vec3) -> Matrix4 {
    let up = normalized(subtract(top_left, bottom_left));
    let right = normalized(subtract(top_right, top_left));
    let normal = display_normal(top_left, bottom_left, top_right);
    let center = display_center(top_left, bottom_left, top_right);
    let axes = [right, up, normal];
    let rotation = std::array::from_fn(|i| std::array::from_fn(|j| axes[j][i]));
    homogeneous(rotation, axes.map(|axis| -dot(axis, center)))
}

/// 0x1402b2100 as called by 0x1402b00f0 with `(a, b, c + pi)`: `Rx(a) Ry(b) Rz(c + pi)`
/// for row vectors.
pub fn camera_rotation(angles: Vec3) -> Matrix3 {
    let [a, b, c] = angles;
    let (x, y, z) = (x_rotation(a), y_rotation(b), z_rotation(c + PI));
    product3(&z, &product3(&y, &x))
}

/// Settings `+0x50` 4x4 built by 0x1402b00f0 from camera hardware info.
///
/// `translation` and `angles` are the camera record's six extrinsic values in the order
/// they are stored; the matrix maps camera coordinates to tracker coordinates for row
/// vectors.
pub fn camera_extrinsic(translation: Vec3, angles: Vec3) -> Matrix4 {
    homogeneous(camera_rotation(angles), translation)
}

/// 0x140294820: `[p, 1] @ (config4 @ camera @ display)`, divided by w.
pub fn transform_position(position: Vec3, camera: &Matrix4, display: &Matrix4, config4: &Matrix4) -> Vec3 {
    let combined = product4(config4, &product4(camera, display));
    let [x, y, z] = position;
    let row: [f32; 4] = std::array::from_fn(|j| {
        let total = (y * combined[1][j] + x * combined[0][j]) + z * combined[2][j];
        combined[3][j] * 1.0 + total
    });
    [row[0] / row[3], row[1] / row[3], row[2] / row[3]]
}

fn upper3(matrix: &Matrix4) -> Matrix3 {
    std::array::from_fn(|i| std::array::from_fn(|j| matrix[i][j]))
}

/// 0x140294ba0: order-3 angles in camera axes to angles in display axes.
///
/// The matrix `euler_matrix(angles) @ config3 @ camera3 @ display3 @ Ry(pi)` is converted
/// back with `matrix_euler`; the first and third angles are negated.
pub fn transform_rotation(angles: Vec3, camera: &Matrix4, display: &Matrix4, config3: &Matrix3) -> Vec3 {
    let product = product3(&upper3(display), &y_rotation(PI));
    let product = product3(&upper3(camera), &product);
    let product = product3(config3, &product);
    let product = product3(&euler_matrix(angles), &product);
    let [a, b, c] = matrix_euler(&product);
    [-a, b, -c]
}

/// Stage process 0x140294670: `(rotation, position)` in display axes.
pub fn geometry_stage(
    angles: Vec3,
    position: Vec3,
    camera: &Matrix4,
    display: &Matrix4,
    config: &(Matrix3, Matrix4),
) -> (Vec3, Vec3) {
    (transform_rotation(angles, camera, display, &config.0), transform_position(position, camera, display, &config.1))
}

#[cfg(test)]
mod tests {
    use super::*;

    const TOP_LEFT: Vec3 = [-296.6200866699219, 326.0040588378906, 111.8187484741211];
    const BOTTOM_LEFT: Vec3 = [-296.6200866699219, 10.267330169677734, -3.100020170211792];
    const TOP_RIGHT: Vec3 = [300.3799133300781, 326.0040588378906, 111.8187484741211];
    const CAMERA: (Vec3, Vec3) = ([10.0, -20.0, 5.0], [0.3, 0.1, -0.2]);

    #[test]
    fn display_matrix_matches_emulated_original() {
        let expected = [
            [1.0, 0.0, 0.0, 0.0],
            [0.0, 0.9396926164627075, -0.3420201241970062, 0.0],
            [0.0, 0.3420201539993286, 0.9396925568580627, 0.0],
            [-1.879913330078125, -176.58786010742188, 6.424701690673828, 1.0],
        ];
        assert_eq!(display_matrix(TOP_LEFT, BOTTOM_LEFT, TOP_RIGHT), expected);
    }

    #[test]
    fn normal_points_from_screen_towards_viewer() {
        assert_eq!(display_normal([-250.0, 300.0, 0.0], [-250.0, 10.0, 0.0], [250.0, 300.0, 0.0]), [0.0, 0.0, 1.0]);
    }

    #[test]
    fn stage_matches_emulated_original() {
        let cases = [
            (
                [0.0, 0.0, 0.0],
                [0.0, 0.0, 600.0],
                [-0.048820629715919495, 0.10011979937553406, -0.2049022614955902],
                [68.0201416015625, -164.39093017578125, 614.24755859375],
            ),
            (
                [0.15, 2.941592653589793, 0.05],
                [-40.0, 30.0, 600.0],
                [-0.05895845964550972, 3.016695261001587, 0.14097116887569427],
                [34.943626403808594, -126.92433166503906, 615.73046875],
            ),
            (
                [0.12, -3.0, 0.2],
                [-120.0, 80.0, 900.0],
                [-0.09773179888725281, -2.9278714656829834, 0.010633852332830429],
                [-3.23614501953125, -47.12995910644531, 917.5170288085938],
            ),
        ];
        let camera = camera_extrinsic(CAMERA.0, CAMERA.1);
        let display = display_matrix(TOP_LEFT, BOTTOM_LEFT, TOP_RIGHT);
        for (angles, position, rotation, translated) in cases {
            let (got_rotation, got_position) = geometry_stage(angles, position, &camera, &display, &geometry_config());
            assert!(got_rotation.iter().zip(rotation).all(|(a, b)| (a - b).abs() <= 1e-6));
            assert_eq!(got_position, translated);
        }
    }

    #[test]
    fn aligned_inputs_only_negate_first_and_third_angles() {
        let camera = camera_extrinsic([0.0; 3], [0.0; 3]);
        let display = display_matrix([-250.0, 300.0, 0.0], [-250.0, 10.0, 0.0], [250.0, 300.0, 0.0]);
        let (rotation, position) =
            geometry_stage([0.2, -0.1, 0.05], [10.0, 20.0, 600.0], &camera, &display, &geometry_config());
        assert!(rotation.iter().zip([-0.2, -0.1, -0.05]).all(|(a, b)| (a - b).abs() <= 1e-6));
        assert!(position.iter().zip([10.0, -135.0, 600.0]).all(|(a, b)| (a - b).abs() <= 1e-3));
    }
}
