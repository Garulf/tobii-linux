//! Little-endian reads that return `None` past the end, so each format reports its own error.

pub fn array<const N: usize>(data: &[u8], offset: usize) -> Option<[u8; N]> {
    data.get(offset..offset.checked_add(N)?)?.try_into().ok()
}

/// `data[start..end]` cut at the end of `data`, as a Python slice.
pub fn clamped(data: &[u8], start: usize, end: usize) -> &[u8] {
    &data[start.min(data.len())..end.min(data.len()).max(start.min(data.len()))]
}

pub fn u8_at(data: &[u8], offset: usize) -> Option<u8> {
    data.get(offset).copied()
}

pub fn u16_at(data: &[u8], offset: usize) -> Option<u16> {
    array(data, offset).map(u16::from_le_bytes)
}

pub fn u32_at(data: &[u8], offset: usize) -> Option<u32> {
    array(data, offset).map(u32::from_le_bytes)
}

pub fn i32_at(data: &[u8], offset: usize) -> Option<i32> {
    array(data, offset).map(i32::from_le_bytes)
}

pub fn u64_at(data: &[u8], offset: usize) -> Option<u64> {
    array(data, offset).map(u64::from_le_bytes)
}

pub fn f32_at(data: &[u8], offset: usize) -> Option<f32> {
    array(data, offset).map(f32::from_le_bytes)
}

pub fn f32_bytes(values: &[f32]) -> Vec<u8> {
    values.iter().flat_map(|value| value.to_le_bytes()).collect()
}

/// An I/O error naming the file, keeping its kind so Python still sees FileNotFoundError.
pub fn file_error(path: &std::path::Path, error: std::io::Error) -> std::io::Error {
    std::io::Error::new(error.kind(), format!("{}: {error}", path.display()))
}

pub fn read_file(path: &std::path::Path) -> std::io::Result<Vec<u8>> {
    std::fs::read(path).map_err(|e| file_error(path, e))
}

pub fn open_file(path: &std::path::Path) -> std::io::Result<std::fs::File> {
    std::fs::File::open(path).map_err(|e| file_error(path, e))
}

/// Lower-case hexadecimal SHA-256, as `hashlib.sha256(data).hexdigest()`.
pub fn sha256_hex(data: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    hex(&Sha256::digest(data))
}

pub fn hex(data: &[u8]) -> String {
    data.iter().map(|byte| format!("{byte:02x}")).collect()
}
