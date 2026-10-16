//! Dense float32 arrays in numpy's C order.

/// A point or vector of three float32 coordinates.
pub type Vec3 = [f32; 3];

/// A row-major float32 matrix.
#[derive(Debug, Clone, PartialEq)]
pub struct Matrix {
    pub rows: usize,
    pub cols: usize,
    pub values: Vec<f32>,
}

impl Matrix {
    pub fn zeros(rows: usize, cols: usize) -> Self {
        Self { rows, cols, values: vec![0.0; rows * cols] }
    }

    pub fn identity(order: usize) -> Self {
        let mut matrix = Self::zeros(order, order);
        for i in 0..order {
            matrix.values[i * order + i] = 1.0;
        }
        matrix
    }

    pub fn from_values(rows: usize, cols: usize, values: Vec<f32>) -> Option<Self> {
        (rows.checked_mul(cols) == Some(values.len())).then_some(Self { rows, cols, values })
    }

    pub fn get(&self, row: usize, col: usize) -> f32 {
        self.values[row * self.cols + col]
    }

    pub fn set(&mut self, row: usize, col: usize, value: f32) {
        self.values[row * self.cols + col] = value;
    }

    pub fn row(&self, row: usize) -> &[f32] {
        &self.values[row * self.cols..(row + 1) * self.cols]
    }

    /// The selected columns, in the given order.
    pub fn columns(&self, columns: &[usize]) -> Self {
        let values = (0..self.rows).flat_map(|r| columns.iter().map(move |&c| self.get(r, c))).collect();
        Self { rows: self.rows, cols: columns.len(), values }
    }
}

/// An `N x 3 x K` deformation basis (`values[(point * 3 + axis) * count + k]`) with the
/// per-element libm tanh, rounded to float32, that the fitting Jacobian uses.
#[derive(Debug, Clone, PartialEq)]
pub struct Basis {
    points: usize,
    count: usize,
    values: Vec<f32>,
    tanh: Vec<f32>,
}

impl Basis {
    pub fn new(points: usize, count: usize, values: Vec<f32>) -> Option<Self> {
        if points.checked_mul(3).and_then(|n| n.checked_mul(count)) != Some(values.len()) {
            return None;
        }
        let tanh = values.iter().map(|&v| f64::from(v).tanh() as f32).collect();
        Some(Self { points, count, values, tanh })
    }

    pub fn zeros(points: usize, count: usize) -> Self {
        Self::new(points, count, vec![0.0; points * 3 * count]).unwrap()
    }

    pub fn points(&self) -> usize {
        self.points
    }

    pub fn count(&self) -> usize {
        self.count
    }

    pub fn values(&self) -> &[f32] {
        &self.values
    }

    pub fn tanh(&self) -> &[f32] {
        &self.tanh
    }

    pub fn get(&self, point: usize, axis: usize, k: usize) -> f32 {
        self.values[(point * 3 + axis) * self.count + k]
    }

    /// The `count` components of one point axis.
    pub fn components(&self, point: usize, axis: usize) -> &[f32] {
        let start = (point * 3 + axis) * self.count;
        &self.values[start..start + self.count]
    }

    pub fn is_finite(&self) -> bool {
        self.values.iter().all(|v| v.is_finite())
    }

    /// Every element mapped through `f`; the tanh table is recomputed.
    pub fn map(&self, f: impl Fn(f32) -> f32) -> Self {
        Self::new(self.points, self.count, self.values.iter().map(|&v| f(v)).collect()).unwrap()
    }

    /// Columns whose flag is false set to zero, as the runtime loader does for disabled units.
    pub fn masked(&self, enabled: &[bool]) -> Self {
        let values = self.values.iter().enumerate().map(|(i, &v)| if enabled[i % self.count] { v } else { 0.0 });
        Self::new(self.points, self.count, values.collect()).unwrap()
    }

    /// The selected points, in the given order.
    pub fn select(&self, points: &[usize]) -> Self {
        let stride = 3 * self.count;
        let pick = |table: &[f32]| points.iter().flat_map(|&p| table[p * stride..(p + 1) * stride].to_vec()).collect();
        Self { points: points.len(), count: self.count, values: pick(&self.values), tanh: pick(&self.tanh) }
    }
}
