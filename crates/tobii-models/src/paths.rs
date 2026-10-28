//! Where the models are looked for when nothing else names a directory.

use std::path::PathBuf;

/// `$variable`, or `~/fallback` when it is unset or empty.
pub fn xdg_dir(variable: &str, fallback: &str) -> PathBuf {
    match std::env::var_os(variable) {
        Some(base) if !base.is_empty() => PathBuf::from(base),
        _ => std::env::home_dir().unwrap_or_default().join(fallback),
    }
}

/// `$XDG_DATA_HOME/tobii-linux/models`, by default `~/.local/share/tobii-linux/models`.
pub fn default_models_dir() -> PathBuf {
    xdg_dir("XDG_DATA_HOME", ".local/share").join("tobii-linux/models")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn models_dir_follows_xdg_data_home() {
        assert_eq!(xdg_dir("TOBII_MODELS_TEST_UNSET_VARIABLE", ".x"), std::env::home_dir().unwrap().join(".x"));
    }
}
