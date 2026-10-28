"""Where the models are looked for when nothing else names a directory.

The implementation is crates/tobii-models/src/paths.rs, shared with the Rust binary.
"""
from pathlib import Path

from ._native import model_paths as _paths


def default_models_dir():
    """$XDG_DATA_HOME/tobii-linux/models, by default ~/.local/share/tobii-linux/models."""
    return Path(_paths.default_models_dir())
