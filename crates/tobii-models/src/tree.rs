//! `{relative path: bytes}` file trees in insertion order, as the Python modules' dicts.

use std::path::Path;

use crate::error::{Result, format};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FileTree {
    entries: Vec<(String, Vec<u8>)>,
}

impl FileTree {
    pub fn new() -> Self {
        Self::default()
    }

    /// Replaces the bytes of an existing path in place, keeping its position.
    pub fn insert(&mut self, path: impl Into<String>, data: Vec<u8>) {
        let path = path.into();
        match self.entries.iter_mut().find(|(p, _)| *p == path) {
            Some(entry) => entry.1 = data,
            None => self.entries.push((path, data)),
        }
    }

    pub fn get(&self, path: &str) -> Option<&[u8]> {
        self.entries.iter().find(|(p, _)| p == path).map(|(_, data)| data.as_slice())
    }

    pub fn contains(&self, path: &str) -> bool {
        self.get(path).is_some()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = (&str, &[u8])> {
        self.entries.iter().map(|(path, data)| (path.as_str(), data.as_slice()))
    }

    pub fn paths(&self) -> impl Iterator<Item = &str> {
        self.entries.iter().map(|(path, _)| path.as_str())
    }

    pub fn extend(&mut self, other: FileTree) {
        for (path, data) in other.entries {
            self.insert(path, data);
        }
    }

    /// Write every file under `root`, creating directories; with `refuse_changes` an
    /// existing file with different contents is an error instead of being replaced.
    pub fn write(&self, root: &Path, refuse_changes: bool) -> Result<()> {
        for (relative, data) in self.iter() {
            let target = root.join(relative);
            if refuse_changes && target.exists() && std::fs::read(&target)? != data {
                return format(format!("Refusing to replace different existing file: {}", target.display()));
            }
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(&target, data)?;
        }
        Ok(())
    }
}

impl FromIterator<(String, Vec<u8>)> for FileTree {
    fn from_iter<I: IntoIterator<Item = (String, Vec<u8>)>>(items: I) -> Self {
        let mut tree = Self::new();
        for (path, data) in items {
            tree.insert(path, data);
        }
        tree
    }
}

impl IntoIterator for FileTree {
    type Item = (String, Vec<u8>);
    type IntoIter = std::vec::IntoIter<(String, Vec<u8>)>;

    fn into_iter(self) -> Self::IntoIter {
        self.entries.into_iter()
    }
}
