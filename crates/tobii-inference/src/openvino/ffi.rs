//! The part of OpenVINO's C API the networks use, loaded at run time with libloading.
//!
//! This is the crate's only unsafe code. Every OpenVINO object is owned by one wrapper
//! that frees it on drop; pointers never leave this module. The declarations follow the
//! headers of OpenVINO 2026.4 (`openvino/c/*.h`); `ov_core_compile_model_props` is the
//! non-variadic compile entry point those headers provide for FFI callers.
#![allow(unsafe_code, reason = "foreign calls into the OpenVINO C library, confined to this module")]

use std::ffi::{CStr, CString, c_char, c_void};
use std::path::Path;
use std::ptr;
use std::sync::Arc;

use libloading::Library;

use crate::error::{Error, Result};

/// `ov_element_type_e` values used here.
pub const F32: u32 = 4;
pub const U8: u32 = 16;

#[repr(C)]
struct Shape {
    rank: i64,
    dims: *mut i64,
}

#[repr(C)]
struct Property {
    key: *const c_char,
    value: *const c_void,
}

#[repr(C)]
struct Version {
    build_number: *const c_char,
    description: *const c_char,
}

type Status = i32;
type Handle = *mut c_void;

/// Function pointers resolved from the library, which the struct keeps loaded.
struct Api {
    core_create: unsafe extern "C" fn(*mut Handle) -> Status,
    core_free: unsafe extern "C" fn(Handle),
    read_model: unsafe extern "C" fn(Handle, *const c_char, usize, Handle, *mut Handle) -> Status,
    model_free: unsafe extern "C" fn(Handle),
    compile: unsafe extern "C" fn(Handle, Handle, *const c_char, usize, *const Property, *mut Handle) -> Status,
    compiled_free: unsafe extern "C" fn(Handle),
    compiled_outputs: unsafe extern "C" fn(Handle, *mut usize) -> Status,
    request_create: unsafe extern "C" fn(Handle, *mut Handle) -> Status,
    request_free: unsafe extern "C" fn(Handle),
    set_input: unsafe extern "C" fn(Handle, usize, Handle) -> Status,
    infer: unsafe extern "C" fn(Handle) -> Status,
    get_output: unsafe extern "C" fn(Handle, usize, *mut Handle) -> Status,
    tensor_from_host: unsafe extern "C" fn(u32, Shape, *mut c_void, *mut Handle) -> Status,
    tensor_free: unsafe extern "C" fn(Handle),
    tensor_data: unsafe extern "C" fn(Handle, *mut *mut c_void) -> Status,
    tensor_size: unsafe extern "C" fn(Handle, *mut usize) -> Status,
    tensor_element_type: unsafe extern "C" fn(Handle, *mut u32) -> Status,
    shape_create: unsafe extern "C" fn(i64, *const i64, *mut Shape) -> Status,
    shape_free: unsafe extern "C" fn(*mut Shape) -> Status,
    error_info: unsafe extern "C" fn(Status) -> *const c_char,
    last_error: unsafe extern "C" fn() -> *const c_char,
    version: unsafe extern "C" fn(*mut Version) -> Status,
    version_free: unsafe extern "C" fn(*mut Version),
    _library: Library,
}

/// A loaded OpenVINO C library with one `ov_core_t`.
pub struct Core {
    api: Api,
    core: Handle,
}

// SAFETY: OpenVINO documents ov::Core as thread-safe, and the core handle is only read.
unsafe impl Send for Core {}
// SAFETY: as above; the function pointers are plain code addresses.
unsafe impl Sync for Core {}

impl std::fmt::Debug for Core {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Core").finish_non_exhaustive()
    }
}

fn unavailable(path: &Path, error: &libloading::Error) -> Error {
    let mut message = format!("Cannot load the OpenVINO C library {}: {error}", path.display());
    let mut source = std::error::Error::source(error);
    while let Some(cause) = source {
        message.push_str(&format!(": {cause}"));
        source = cause.source();
    }
    message.push_str(&format!(" (set {} to its location)", super::LIBRARY_VARIABLE));
    Error::Unavailable(message)
}

impl Core {
    /// Loads the library at `path` (a file name alone is searched by the dynamic loader)
    /// and creates a core.
    pub fn open(path: &Path) -> Result<Self> {
        // SAFETY: loading runs the library's initializers; OpenVINO's are ordinary C++
        // static constructors with no preconditions.
        let library = unsafe { Library::new(path) }.map_err(|error| unavailable(path, &error))?;
        macro_rules! symbol {
            ($name:literal) => {
                // SAFETY: the symbol has the signature of the field it is stored in, as
                // declared by the OpenVINO 2026.4 C headers.
                *unsafe { library.get(concat!($name, "\0").as_bytes()) }.map_err(|error| unavailable(path, &error))?
            };
        }
        let api = Api {
            core_create: symbol!("ov_core_create"),
            core_free: symbol!("ov_core_free"),
            read_model: symbol!("ov_core_read_model_from_memory_buffer"),
            model_free: symbol!("ov_model_free"),
            compile: symbol!("ov_core_compile_model_props"),
            compiled_free: symbol!("ov_compiled_model_free"),
            compiled_outputs: symbol!("ov_compiled_model_outputs_size"),
            request_create: symbol!("ov_compiled_model_create_infer_request"),
            request_free: symbol!("ov_infer_request_free"),
            set_input: symbol!("ov_infer_request_set_input_tensor_by_index"),
            infer: symbol!("ov_infer_request_infer"),
            get_output: symbol!("ov_infer_request_get_output_tensor_by_index"),
            tensor_from_host: symbol!("ov_tensor_create_from_host_ptr"),
            tensor_free: symbol!("ov_tensor_free"),
            tensor_data: symbol!("ov_tensor_data"),
            tensor_size: symbol!("ov_tensor_get_size"),
            tensor_element_type: symbol!("ov_tensor_get_element_type"),
            shape_create: symbol!("ov_shape_create"),
            shape_free: symbol!("ov_shape_free"),
            error_info: symbol!("ov_get_error_info"),
            last_error: symbol!("ov_get_last_err_msg"),
            version: symbol!("ov_get_openvino_version"),
            version_free: symbol!("ov_version_free"),
            _library: library,
        };
        let mut core = ptr::null_mut();
        // SAFETY: `core` is a valid out pointer.
        let status = unsafe { (api.core_create)(&mut core) };
        let mut created = Self { api, core };
        if let Err(error) = created.check(status, "ov_core_create") {
            created.core = ptr::null_mut();
            return Err(error);
        }
        Ok(created)
    }

    fn check(&self, status: Status, call: &str) -> Result<()> {
        if status == 0 {
            return Ok(());
        }
        // SAFETY: both return null or a NUL-terminated string. The last-error string is not
        // freed: the headers do not say who owns it, and a leak is the safe side.
        let text = |pointer: *const c_char| {
            (!pointer.is_null()).then(|| unsafe { CStr::from_ptr(pointer) }.to_string_lossy().trim().to_owned())
        };
        let info = text(unsafe { (self.api.error_info)(status) }).unwrap_or_default();
        let detail = text(unsafe { (self.api.last_error)() }).filter(|d| !d.is_empty());
        Err(Error::OpenVino(match detail {
            Some(detail) => format!("{call} failed ({info}): {detail}"),
            None => format!("{call} failed ({info}, status {status})"),
        }))
    }

    pub fn version(&self) -> Result<String> {
        let mut version = Version { build_number: ptr::null(), description: ptr::null() };
        // SAFETY: `version` is a valid out structure, freed by `ov_version_free`.
        self.check(unsafe { (self.api.version)(&mut version) }, "ov_get_openvino_version")?;
        let text = if version.build_number.is_null() {
            String::new()
        } else {
            // SAFETY: a NUL-terminated string owned by `version`.
            unsafe { CStr::from_ptr(version.build_number) }.to_string_lossy().into_owned()
        };
        // SAFETY: filled by `ov_get_openvino_version` above.
        unsafe { (self.api.version_free)(&mut version) };
        Ok(text)
    }

    /// A host tensor over `data`, which must outlive the tensor.
    fn tensor<T>(&self, element: u32, dims: &[i64], data: *mut T) -> Result<Tensor<'_>> {
        let mut shape = Shape { rank: 0, dims: ptr::null_mut() };
        // SAFETY: `dims` holds `rank` values and `shape` is a valid out structure.
        self.check(
            unsafe { (self.api.shape_create)(dims.len() as i64, dims.as_ptr(), &mut shape) },
            "ov_shape_create",
        )?;
        let mut tensor = ptr::null_mut();
        // SAFETY: the shape is valid; the caller keeps `data` alive and sized for the shape.
        let status = unsafe {
            (self.api.tensor_from_host)(element, Shape { rank: shape.rank, dims: shape.dims }, data.cast(), &mut tensor)
        };
        // SAFETY: created by `ov_shape_create`; the tensor keeps its own copy of the dims.
        unsafe { (self.api.shape_free)(&mut shape) };
        self.check(status, "ov_tensor_create_from_host_ptr")?;
        Ok(Tensor { core: self, handle: tensor })
    }

    /// Reads an IR model from memory and compiles it for `device` with string properties.
    pub fn compile(
        self: &Arc<Self>,
        xml: &str,
        weights: &[u8],
        device: &str,
        properties: &[(&str, &str)],
    ) -> Result<Compiled> {
        let mut weights: Box<[u8]> = weights.into();
        let weights_tensor = self.tensor(U8, &[weights.len() as i64], weights.as_mut_ptr())?;
        let mut model = ptr::null_mut();
        // SAFETY: the XML pointer and length describe a valid buffer, the tensor is live and
        // `model` is a valid out pointer. The reader copies what it keeps of both buffers.
        let status = unsafe {
            (self.api.read_model)(self.core, xml.as_ptr().cast(), xml.len(), weights_tensor.handle, &mut model)
        };
        self.check(status, "ov_core_read_model_from_memory_buffer")?;
        let model = Owned { handle: model, free: self.api.model_free };
        let device = CString::new(device).map_err(|_| Error::Value("Device name contains NUL".into()))?;
        let strings = properties
            .iter()
            .map(|(key, value)| Ok((CString::new(*key)?, CString::new(*value)?)))
            .collect::<Result<Vec<_>, std::ffi::NulError>>()
            .map_err(|_| Error::Value("Property contains NUL".into()))?;
        let pairs: Vec<Property> =
            strings.iter().map(|(key, value)| Property { key: key.as_ptr(), value: value.as_ptr().cast() }).collect();
        let mut compiled = ptr::null_mut();
        // SAFETY: model, device and properties are valid for the call; `compiled` is an out pointer.
        let status = unsafe {
            (self.api.compile)(self.core, model.handle, device.as_ptr(), pairs.len(), pairs.as_ptr(), &mut compiled)
        };
        self.check(status, "ov_core_compile_model_props")?;
        drop(weights_tensor);
        Ok(Compiled { core: Arc::clone(self), handle: compiled, _weights: weights })
    }
}

impl Drop for Core {
    fn drop(&mut self) {
        if !self.core.is_null() {
            // SAFETY: created by `ov_core_create` and freed once.
            unsafe { (self.api.core_free)(self.core) };
        }
    }
}

struct Owned {
    handle: Handle,
    free: unsafe extern "C" fn(Handle),
}

impl Drop for Owned {
    fn drop(&mut self) {
        // SAFETY: the handle came from the matching create call and is freed once.
        unsafe { (self.free)(self.handle) };
    }
}

struct Tensor<'a> {
    core: &'a Core,
    handle: Handle,
}

impl Drop for Tensor<'_> {
    fn drop(&mut self) {
        // SAFETY: created by `ov_tensor_create_from_host_ptr` or returned by the request.
        unsafe { (self.core.api.tensor_free)(self.handle) };
    }
}

/// A compiled model. The weights it was read from stay allocated with it: OpenVINO's
/// constants may alias the host buffer of the weights tensor.
pub struct Compiled {
    core: Arc<Core>,
    handle: Handle,
    _weights: Box<[u8]>,
}

// SAFETY: ov::CompiledModel is thread-safe.
unsafe impl Send for Compiled {}
// SAFETY: as above.
unsafe impl Sync for Compiled {}

impl Compiled {
    pub fn outputs(&self) -> Result<usize> {
        let mut count = 0;
        // SAFETY: valid handle and out pointer.
        let status = unsafe { (self.core.api.compiled_outputs)(self.handle, &mut count) };
        self.core.check(status, "ov_compiled_model_outputs_size")?;
        Ok(count)
    }

    /// A request whose first input is a float32 tensor of shape `dims`, bound once to a
    /// buffer the request owns.
    pub fn request(&self, dims: &[i64]) -> Result<Request> {
        let core = &self.core;
        let mut request = ptr::null_mut();
        // SAFETY: valid handle and out pointer.
        let status = unsafe { (core.api.request_create)(self.handle, &mut request) };
        core.check(status, "ov_compiled_model_create_infer_request")?;
        let owned = Owned { handle: request, free: core.api.request_free };
        let count = dims.iter().product::<i64>();
        let mut input = vec![0.0f32; usize::try_from(count).unwrap_or(0)].into_boxed_slice();
        let tensor = core.tensor(F32, dims, input.as_mut_ptr())?;
        // SAFETY: request and tensor are valid; `input` lives as long as the request.
        let status = unsafe { (core.api.set_input)(owned.handle, 0, tensor.handle) };
        core.check(status, "ov_infer_request_set_input_tensor_by_index")?;
        drop(tensor);
        Ok(Request { request: owned, input, core: Arc::clone(core) })
    }
}

impl Drop for Compiled {
    fn drop(&mut self) {
        // SAFETY: created by `ov_core_compile_model_props` and freed once.
        unsafe { (self.core.api.compiled_free)(self.handle) };
    }
}

/// An inference request with its input buffer; `&mut self` keeps it to one caller at a time.
/// Fields drop in order: the request before its input buffer and the library.
pub struct Request {
    request: Owned,
    input: Box<[f32]>,
    core: Arc<Core>,
}

// SAFETY: an infer request may move between threads; `infer` takes `&mut self`.
unsafe impl Send for Request {}

impl Request {
    /// Runs the model on `input` (exactly `input_len` values) and copies out its first output.
    pub fn infer(&mut self, input: &[f32]) -> Result<Vec<f32>> {
        if input.len() != self.input.len() {
            return Err(Error::Value("Input size does not match the network input".into()));
        }
        self.input.copy_from_slice(input);
        let core = &*self.core;
        let handle = self.request.handle;
        // SAFETY: valid request with its input bound to `self.input`.
        core.check(unsafe { (core.api.infer)(handle) }, "ov_infer_request_infer")?;
        let mut output = ptr::null_mut();
        // SAFETY: valid request and out pointer.
        let status = unsafe { (core.api.get_output)(handle, 0, &mut output) };
        core.check(status, "ov_infer_request_get_output_tensor_by_index")?;
        let output = Tensor { core, handle: output };
        let (mut element, mut size, mut data) = (0, 0, ptr::null_mut());
        // SAFETY: valid tensor and out pointers.
        core.check(
            unsafe { (core.api.tensor_element_type)(output.handle, &mut element) },
            "ov_tensor_get_element_type",
        )?;
        // SAFETY: as above.
        core.check(unsafe { (core.api.tensor_size)(output.handle, &mut size) }, "ov_tensor_get_size")?;
        // SAFETY: as above.
        core.check(unsafe { (core.api.tensor_data)(output.handle, &mut data) }, "ov_tensor_data")?;
        if element != F32 || data.is_null() {
            return Err(Error::OpenVino("Expected a float32 network output".into()));
        }
        // SAFETY: the tensor holds `size` float32 values at `data` while `output` lives.
        Ok(unsafe { std::slice::from_raw_parts(data.cast::<f32>(), size) }.to_vec())
    }
}
