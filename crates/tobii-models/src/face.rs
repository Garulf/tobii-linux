//! Readers for the plaintext face and landmark assets under `support/`.
//!
//! These keep file coordinates and identifiers. They do not implement Tobii's pose solver,
//! assert coordinate units, or infer deformation coefficient scales. Binary values are
//! float32 and kept bit for bit. Text coordinates are parsed to the nearest `f64`, as
//! Python's `float()` does; the pose code narrows them to float32 afterwards.

use std::path::Path;

use crate::bytes::{f32_at, read_file, u32_at};
use crate::error::{Result, format};

const MAX_COUNT: usize = 1_000_000;
const MAX_LANDMARKS: u32 = 10000;

/// A float32 array file: its dimensions and values in file order.
#[derive(Debug, Clone, PartialEq)]
pub struct Shapes {
    pub dimensions: Vec<u32>,
    pub values: Vec<f32>,
}

impl Shapes {
    /// LE uint32 dimensions followed by packed LE float32 values; `init_shape.bin` has two
    /// dimensions and `NN/is.bin` three. The exact file extent is required, so an opaque
    /// or padded format is not silently accepted.
    pub fn parse(data: &[u8], dimensions: usize) -> Result<Self> {
        if !(2..=3).contains(&dimensions) || data.len() < dimensions * 4 {
            return format("Invalid shape header");
        }
        let shape: Vec<u32> = (0..dimensions).map(|i| u32_at(data, 4 * i).unwrap()).collect();
        let count = shape.iter().try_fold(1u64, |total, &d| total.checked_mul(u64::from(d)));
        let count = match count {
            Some(count) if !shape.contains(&0) && count <= MAX_COUNT as u64 => count as usize,
            _ => return format("Shape extent mismatch"),
        };
        if data.len() != (dimensions + count) * 4 {
            return format("Shape extent mismatch");
        }
        let values: Vec<f32> = (0..count).map(|i| f32_at(data, 4 * (dimensions + i)).unwrap()).collect();
        if !values.iter().all(|v| v.is_finite()) {
            return format("Non-finite shape coordinate");
        }
        Ok(Self { dimensions: shape, values })
    }

    pub fn read(path: &Path, dimensions: usize) -> Result<Self> {
        Self::parse(&read_file(path)?, dimensions)
    }
}

/// `landmarks.bin`: a LE uint32 count, then that many distinct `(group, index)` pairs.
pub fn parse_landmark_ids(data: &[u8]) -> Result<Vec<(u32, u32)>> {
    let Some(count) = u32_at(data, 0) else {
        return format("Missing landmark header");
    };
    if !(1..=MAX_LANDMARKS).contains(&count) || data.len() != 4 + count as usize * 8 {
        return format("Landmark extent mismatch");
    }
    let pairs: Vec<(u32, u32)> =
        (0..count as usize).map(|i| (u32_at(data, 4 + 8 * i).unwrap(), u32_at(data, 8 + 8 * i).unwrap())).collect();
    let mut sorted = pairs.clone();
    sorted.sort_unstable();
    sorted.dedup();
    if sorted.len() != pairs.len() {
        return format("Duplicate landmark identifier");
    }
    Ok(pairs)
}

pub fn read_landmark_ids(path: &Path) -> Result<Vec<(u32, u32)>> {
    parse_landmark_ids(&read_file(path)?)
}

/// One FDP row: a landmark's position and the mesh vertex it names (`0.191` is mesh 0,
/// vertex 191).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FdpPoint {
    pub position: [f64; 3],
    pub mesh: i64,
    pub vertex: i64,
}

/// FDP landmark ID to mesh-vertex mapping, in file order.
#[derive(Debug, Clone, PartialEq)]
pub struct Fdp {
    pub points: Vec<((i64, i64), FdpPoint)>,
}

impl Fdp {
    pub fn parse(text: &str) -> Result<Self> {
        let lines: Vec<&str> =
            python_lines(text).map(python_strip).filter(|line| !line.is_empty() && !line.starts_with('#')).collect();
        if !lines.first().is_some_and(|header| header.starts_with("1.0 ")) {
            return format("Unsupported FDP header");
        }
        let mut points: Vec<((i64, i64), FdpPoint)> = vec![];
        for line in &lines[1..] {
            let fields: Vec<&str> = python_split(line).collect();
            if fields.len() != 5 {
                return format("Unexpected FDP row");
            }
            let (identifier, reference) = (dotted_pair(fields[0])?, dotted_pair(fields[4])?);
            let (Some(identifier), Some((mesh, vertex))) = (identifier, reference) else {
                return format("Invalid/duplicate FDP mapping");
            };
            if points.iter().any(|(id, _)| *id == identifier) {
                return format("Invalid/duplicate FDP mapping");
            }
            let position = coordinates::<3>(&fields[1..4])?;
            points.push((identifier, FdpPoint { position, mesh, vertex }));
        }
        Ok(Self { points })
    }

    pub fn read(path: &Path) -> Result<Self> {
        Self::parse(&read_text(path)?)
    }

    pub fn get(&self, identifier: (i64, i64)) -> Option<&FdpPoint> {
        self.points.iter().find(|(id, _)| *id == identifier).map(|(_, point)| point)
    }
}

/// `Some(pair)` for exactly two dot-separated integers, `None` for another count.
fn dotted_pair(field: &str) -> Result<Option<(i64, i64)>> {
    let parts = field.split('.').map(python_int).collect::<Result<Vec<_>>>()?;
    Ok(match parts[..] {
        [a, b] => Some((a, b)),
        _ => None,
    })
}

/// A sparse deformation: its name and `(vertex, displacement)` entries.
#[derive(Debug, Clone, PartialEq)]
pub struct Deformation {
    pub name: String,
    pub entries: Vec<(usize, [f64; 3])>,
}

/// `jk_300.wfm`: vertices, texture coordinates, triangles and the sparse animation and
/// shape units.
#[derive(Debug, Clone, PartialEq)]
pub struct FaceMesh {
    pub vertices: Vec<[f64; 3]>,
    pub texcoords: Vec<[f64; 2]>,
    pub triangles: Vec<[usize; 3]>,
    pub animation_units: Vec<Deformation>,
    pub shape_units: Vec<Deformation>,
}

struct MeshLines<'a> {
    lines: std::vec::IntoIter<&'a str>,
}

impl<'a> MeshLines<'a> {
    fn take(&mut self) -> Result<&'a str> {
        match self.lines.next() {
            Some(line) => Ok(line),
            None => format("Truncated mesh"),
        }
    }

    fn header(&mut self, expected: &str) -> Result<usize> {
        if self.take()? != expected {
            return format(format!("Expected mesh section {expected}"));
        }
        let count = python_int(self.take()?)?;
        if !(0..=MAX_COUNT as i64).contains(&count) {
            return format("Invalid mesh count");
        }
        Ok(count as usize)
    }

    fn rows<T>(&mut self, count: usize, width: usize, convert: impl Fn(&[&str]) -> Result<T>) -> Result<Vec<T>> {
        (0..count)
            .map(|_| {
                let fields: Vec<&str> = python_split(self.take()?).collect();
                if fields.len() != width {
                    return format("Unexpected mesh row width");
                }
                convert(&fields)
            })
            .collect()
    }

    fn units(&mut self, label: &str, vertices: usize) -> Result<Vec<Deformation>> {
        let count = self.header(label)?;
        let mut result = Vec::with_capacity(count);
        for _ in 0..count {
            let Some(name) = self.take()?.strip_prefix("# ") else {
                return format("Missing deformation name");
            };
            let entries = python_int(self.take()?)?;
            if !(0..=vertices as i64).contains(&entries) {
                return format("Invalid deformation count");
            }
            let entries = self.rows(entries as usize, 4, |f| Ok((python_int(f[0])?, coordinates::<3>(&f[1..])?)))?;
            let mut indices: Vec<i64> = entries.iter().map(|(i, _)| *i).collect();
            indices.sort_unstable();
            indices.dedup();
            if indices.len() != entries.len() || indices.iter().any(|&i| i < 0 || i >= vertices as i64) {
                return format("Invalid deformation indexing");
            }
            let entries = entries.into_iter().map(|(i, xyz)| (i as usize, xyz)).collect();
            result.push(Deformation { name: name.to_string(), entries });
        }
        Ok(result)
    }
}

impl FaceMesh {
    pub fn parse(text: &str) -> Result<Self> {
        let lines: Vec<&str> = python_lines(text).map(python_strip).filter(|line| !line.is_empty()).collect();
        let mut lines = MeshLines { lines: lines.into_iter() };
        let count = lines.header("# VERTEX LIST:")?;
        let vertices = lines.rows(count, 3, coordinates::<3>)?;
        let count = lines.header("# TEXCOORD LIST:")?;
        let texcoords = lines.rows(count, 2, coordinates::<2>)?;
        let count = lines.header("# FACE LIST:")?;
        let triangles = lines.rows(count, 3, |f| Ok([python_int(f[0])?, python_int(f[1])?, python_int(f[2])?]))?;
        let in_mesh = |i: i64| (0..vertices.len() as i64).contains(&i);
        if vertices.len() != texcoords.len() || !triangles.iter().flatten().all(|&i| in_mesh(i)) {
            return format("Invalid mesh indexing");
        }
        let triangles = triangles.into_iter().map(|t| t.map(|i| i as usize)).collect();
        let animation_units = lines.units("# ANIMATION UNITS LIST:", vertices.len())?;
        let shape_units = lines.units("# SHAPE UNITS LIST:", vertices.len())?;
        if lines.lines.next().is_some() {
            return format("Unexpected trailing mesh data");
        }
        Ok(Self { vertices, texcoords, triangles, animation_units, shape_units })
    }

    pub fn read(path: &Path) -> Result<Self> {
        Self::parse(&read_text(path)?)
    }

    /// Mesh vertices of the identified landmarks: a join of file IDs, not a fitted face.
    pub fn landmark_vertices(&self, mapping: &Fdp, ids: &[(u32, u32)]) -> Result<Vec<[f64; 3]>> {
        ids.iter()
            .map(|&(group, index)| {
                let Some(point) = mapping.get((group.into(), index.into())) else {
                    return format(format!("Landmark ({group}, {index}) has no FDP mapping"));
                };
                match usize::try_from(point.vertex).ok().and_then(|v| self.vertices.get(v)) {
                    Some(vertex) if point.mesh == 0 => Ok(*vertex),
                    _ => format("Unsupported mesh reference"),
                }
            })
            .collect()
    }
}

fn read_text(path: &Path) -> Result<String> {
    String::from_utf8(read_file(path)?).or_else(|_| format(format!("{} is not UTF-8 text", path.display())))
}

fn coordinates<const N: usize>(fields: &[&str]) -> Result<[f64; N]> {
    let mut result = [0.0; N];
    for (value, field) in result.iter_mut().zip(fields) {
        *value = python_float(field)?;
        if !value.is_finite() {
            return format("Non-finite coordinate");
        }
    }
    Ok(result)
}

/// `int(text)` for a token without surrounding whitespace.
fn python_int(text: &str) -> Result<i64> {
    text.parse().or_else(|_| format(format!("invalid literal for int() with base 10: '{text}'")))
}

/// `float(text)`: Rust's parser rounds to nearest like Python's and accepts the same
/// `inf`/`nan` spellings.
fn python_float(text: &str) -> Result<f64> {
    text.parse().or_else(|_| format(format!("could not convert string to float: '{text}'")))
}

fn python_space(c: char) -> bool {
    c.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&c)
}

fn python_strip(line: &str) -> &str {
    line.trim_matches(python_space)
}

fn python_split(line: &str) -> impl Iterator<Item = &str> {
    line.split(python_space).filter(|field| !field.is_empty())
}

/// `str.splitlines()`: every Unicode line boundary, with `\r\n` as one.
fn python_lines(text: &str) -> impl Iterator<Item = &str> {
    let boundary = |c: char| {
        matches!(c, '\n' | '\r' | '\u{b}' | '\u{c}' | '\u{1c}'..='\u{1e}' | '\u{85}' | '\u{2028}' | '\u{2029}')
    };
    let mut rest = text;
    std::iter::from_fn(move || {
        if rest.is_empty() {
            return None;
        }
        let Some(end) = rest.find(boundary) else {
            return Some(std::mem::take(&mut rest));
        };
        let line = &rest[..end];
        let width = if rest[end..].starts_with("\r\n") { 2 } else { rest[end..].chars().next().unwrap().len_utf8() };
        rest = &rest[end + width..];
        Some(line)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::{SUPPORT, support_file};

    #[test]
    fn installed_landmarks_resolve_to_mesh() {
        if !SUPPORT.is_dir() {
            return;
        }
        let mesh = FaceMesh::read(&support_file("jk_300.wfm")).unwrap();
        let ids = read_landmark_ids(&support_file("bdtsdata/NN/vnn_nir/landmarks.bin")).unwrap();
        let mapping = Fdp::read(&support_file("jk_300_rigid.fdp")).unwrap();
        let mapped = mesh.landmark_vertices(&mapping, &ids).unwrap();
        assert_eq!((mesh.vertices.len(), mesh.triangles.len(), mapped.len()), (357, 640, 57));
        assert_eq!((mesh.animation_units.len(), mesh.shape_units.len()), (23, 40));
        assert_eq!(Shapes::read(&support_file("bdtsdata/NN/is.bin"), 3).unwrap().dimensions, [9, 35, 2]);
        assert_eq!(Shapes::read(&support_file("bdtsdata/NN/vnn_nir/init_shape.bin"), 2).unwrap().values.len(), 114);
    }

    #[test]
    fn shape_rejects_nonfinite_and_bad_extent() {
        let nan = [1u32.to_le_bytes(), 2u32.to_le_bytes(), f32::NAN.to_le_bytes(), 0f32.to_le_bytes()].concat();
        let short = [57u32.to_le_bytes(), 2u32.to_le_bytes()].concat();
        for data in [nan, short] {
            assert!(Shapes::parse(&data, 2).is_err());
        }
        let ok = [1u32.to_le_bytes(), 1u32.to_le_bytes(), 1.5f32.to_le_bytes()].concat();
        assert_eq!(Shapes::parse(&ok, 2).unwrap(), Shapes { dimensions: vec![1, 1], values: vec![1.5] });
    }

    #[test]
    fn landmarks_reject_duplicates() {
        let data: Vec<u8> = [2u32, 1, 2, 1, 2].iter().flat_map(|w| w.to_le_bytes()).collect();
        assert!(parse_landmark_ids(&data).is_err());
        let data: Vec<u8> = [2u32, 1, 2, 2, 1].iter().flat_map(|w| w.to_le_bytes()).collect();
        assert_eq!(parse_landmark_ids(&data).unwrap(), [(1, 2), (2, 1)]);
    }

    #[test]
    fn fdp_keeps_mesh_vertex_references() {
        let fdp = Fdp::parse("# comment\r\n1.0 header\r\n2.1 0.5 -1e-3 3 0.191\r\n\r\n2.2 1 2 3 0.7\n").unwrap();
        assert_eq!(fdp.points.len(), 2);
        assert_eq!(fdp.get((2, 1)), Some(&FdpPoint { position: [0.5, -0.001, 3.0], mesh: 0, vertex: 191 }));
        assert!(Fdp::parse("1.0 x\n2.1 0 0 0 0.1\n2.1 0 0 0 0.2\n").is_err());
        assert!(Fdp::parse("1.0 x\n2.1 0 0 inf 0.1\n").is_err());
        assert!(Fdp::parse("2.0 x\n").is_err());
    }

    #[test]
    fn mesh_sections_and_indexing() {
        let text = "# VERTEX LIST:\n2\n0 0 0\n1 2 3\n# TEXCOORD LIST:\n2\n0 0\n1 1\n# FACE LIST:\n1\n0 1 1\n\
                    # ANIMATION UNITS LIST:\n1\n# jaw\n1\n1 0.5 0 0\n# SHAPE UNITS LIST:\n0\n";
        let mesh = FaceMesh::parse(text).unwrap();
        assert_eq!(mesh.animation_units, [Deformation { name: "jaw".into(), entries: vec![(1, [0.5, 0.0, 0.0])] }]);
        assert!(FaceMesh::parse(&text.replace("0 1 1", "0 1 2")).is_err());
        assert!(FaceMesh::parse(&format!("{text}extra\n")).is_err());
        assert!(FaceMesh::parse(&text.replace("1 0.5 0 0", "2 0.5 0 0")).is_err());
    }

    #[test]
    fn python_line_splitting() {
        assert_eq!(python_lines("a\r\nb\rc\u{2028}d\n").collect::<Vec<_>>(), ["a", "b", "c", "d"]);
        assert_eq!(python_lines("a\n\nb").collect::<Vec<_>>(), ["a", "", "b"]);
    }
}
