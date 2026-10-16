//! OpenVINO, loaded at run time through its C API.
//!
//! The library is found in this order: `TOBII_OPENVINO_LIBRARY` (the library file, or a
//! directory holding `libopenvino_c.so*`), then the directories the caller passes (the
//! Python bindings pass the `openvino/libs` directory of an installed OpenVINO wheel), then
//! `libopenvino_c.so` through the dynamic loader's search path. Networks compile on the CPU
//! with float32 inference precision and two inference threads, as the Python runner did.

mod ffi;

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

use crate::error::{Error, Result};
use crate::legacy_ir::{self, IrModel};

pub const LIBRARY_VARIABLE: &str = "TOBII_OPENVINO_LIBRARY";
const LIBRARY_NAME: &str = "libopenvino_c.so";
const DEVICE: &str = "CPU";
const PROPERTIES: [(&str, &str); 2] = [("INFERENCE_PRECISION_HINT", "f32"), ("INFERENCE_NUM_THREADS", "2")];

/// A loaded OpenVINO library and its core; clones share both.
#[derive(Debug, Clone)]
pub struct Runtime {
    core: Arc<ffi::Core>,
    library: PathBuf,
}

static SHARED: Mutex<Option<Runtime>> = Mutex::new(None);

/// `libopenvino_c.so`, or the highest versioned `libopenvino_c.so.*`, in `directory`.
fn library_in(directory: &Path) -> Option<PathBuf> {
    let mut found: Vec<PathBuf> = std::fs::read_dir(directory)
        .ok()?
        .filter_map(|entry| entry.ok().map(|e| e.path()))
        .filter(|path| {
            path.file_name().and_then(|n| n.to_str()).is_some_and(|name| {
                name == LIBRARY_NAME || name.strip_prefix(LIBRARY_NAME).is_some_and(|rest| rest.starts_with('.'))
            })
        })
        .collect();
    found.sort_by_key(|path| (path.file_name().is_some_and(|n| n != LIBRARY_NAME), path.clone()));
    found.into_iter().next()
}

impl Runtime {
    /// The library to load: see the module documentation for the search order.
    pub fn locate(directories: &[PathBuf]) -> Result<PathBuf> {
        if let Some(configured) = std::env::var_os(LIBRARY_VARIABLE).filter(|v| !v.is_empty()) {
            let configured = PathBuf::from(configured);
            if !configured.is_dir() {
                return Ok(configured);
            }
            return library_in(&configured).ok_or_else(|| {
                Error::Unavailable(format!("{LIBRARY_VARIABLE} names {} without {LIBRARY_NAME}", configured.display()))
            });
        }
        Ok(directories.iter().find_map(|d| library_in(d)).unwrap_or_else(|| PathBuf::from(LIBRARY_NAME)))
    }

    /// Loads the library at `library` and creates a core.
    pub fn open(library: &Path) -> Result<Self> {
        Ok(Self { core: Arc::new(ffi::Core::open(library)?), library: library.to_owned() })
    }

    /// The process-wide runtime, loaded on first success with `locate(directories)`.
    pub fn shared(directories: &[PathBuf]) -> Result<Self> {
        let mut shared = SHARED.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(runtime) = shared.as_ref() {
            return Ok(runtime.clone());
        }
        let runtime = Self::open(&Self::locate(directories)?)?;
        *shared = Some(runtime.clone());
        Ok(runtime)
    }

    pub fn library(&self) -> &Path {
        &self.library
    }

    /// OpenVINO's build number, as `openvino.get_version()` reports it.
    pub fn version(&self) -> Result<String> {
        self.core.version()
    }

    /// Compiles a translated network for the CPU with the runner's properties.
    pub fn compile(&self, model: &IrModel) -> Result<Network> {
        let [input] = model.inputs.as_slice() else {
            return Err(Error::Value("Expected a network with one input".into()));
        };
        let compiled = self.core.compile(&model.xml, &model.weights, DEVICE, &PROPERTIES)?;
        if compiled.outputs()? == 0 {
            return Err(Error::Value("Expected a network with an output".into()));
        }
        let request = compiled.request(input)?;
        Ok(Network { input_shape: input.clone(), request: Mutex::new(request), _compiled: compiled })
    }

    /// Translates and compiles the model directory (see `legacy_ir::load`).
    pub fn load(&self, directory: &Path) -> Result<Network> {
        self.compile(&legacy_ir::load(directory)?)
    }
}

/// A compiled network with one float32 input; `infer` returns its first output.
pub struct Network {
    input_shape: Vec<i64>,
    request: Mutex<ffi::Request>,
    _compiled: ffi::Compiled,
}

impl std::fmt::Debug for Network {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Network").field("input_shape", &self.input_shape).finish_non_exhaustive()
    }
}

impl Network {
    pub fn input_shape(&self) -> &[i64] {
        &self.input_shape
    }

    /// Runs the network on `input`, in row-major order of the input shape. Calls from
    /// several threads take turns on the one request.
    pub fn infer(&self, input: &[f32]) -> Result<Vec<f32>> {
        self.request.lock().unwrap_or_else(PoisonError::into_inner).infer(input)
    }
}

#[cfg(test)]
mod tests;
