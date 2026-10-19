//! The OpenGL calls of the gaze preview, behind a small safe API: programs, textures, float
//! render targets, a uniform buffer, clears and draws.
//!
//! Entry points come from libepoxy, GTK's own GL dispatch library: every `glX` is the
//! function pointer variable `epoxy_glX`, valid for whichever context is current. Every
//! object keeps the context it was made in and makes that context current before each call,
//! so no call runs against another context or none; handles never leave this module and are
//! deleted on drop, which the overlay does on the GLArea's `unrealize`.
#![allow(unsafe_code, reason = "OpenGL calls through glow, confined to this module")]

use std::ffi::{CStr, c_void};
use std::fmt;
use std::rc::Rc;
use std::sync::OnceLock;

use glow::HasContext;
use gtk::gdk;
use gtk::prelude::*;
use gtk4 as gtk;
use libloading::Library;

const EPOXY: &str = "libepoxy.so.0";

/// A GL rectangle: x and y from the bottom-left corner, width and height.
pub type Rect = [i32; 4];

/// The accumulation format: `R32F` as Tobii's `R32_FLOAT`, or `R16F` where a GLES context
/// cannot render or filter 32-bit floats.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FloatFormat {
    R32F,
    R16F,
}

struct Shared {
    gl: glow::Context,
    context: gdk::GLContext,
    vertex_array: glow::VertexArray,
}

impl Shared {
    /// The GL functions, with this object's context current.
    fn current(&self) -> &glow::Context {
        if gdk::GLContext::current().as_ref() != Some(&self.context) {
            self.context.make_current();
        }
        &self.gl
    }
}

impl Drop for Shared {
    fn drop(&mut self) {
        let vertex_array = self.vertex_array;
        let gl = self.current();
        // SAFETY: the context is current and the vertex array was created in it; nothing else
        // holds the handle once the last owner is gone.
        unsafe { gl.delete_vertex_array(vertex_array) };
    }
}

/// A loaded GL context.
#[derive(Clone)]
pub struct Gl(Rc<Shared>);

impl fmt::Debug for Gl {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Gl").field("description", &self.description()).finish()
    }
}

fn epoxy() -> Result<&'static Library, String> {
    static LIBRARY: OnceLock<Result<Library, String>> = OnceLock::new();
    LIBRARY
        .get_or_init(|| {
            // SAFETY: libepoxy is the dispatch library libgtk-4 itself links, so it is already
            // loaded and initialised; opening it again only takes another reference.
            unsafe { Library::new(EPOXY) }.map_err(|error| format!("{EPOXY}: {error}"))
        })
        .as_ref()
        .map_err(Clone::clone)
}

fn entry_point(library: &Library, name: &CStr) -> *const c_void {
    let Ok(name) = name.to_str() else { return std::ptr::null() };
    // SAFETY: only the address of `epoxy_<name>` is taken, a global variable of libepoxy,
    // which stays loaded for the life of the process. libepoxy declares each one as a
    // function pointer it always initialises (to a resolver until the first call), so the
    // read is of an initialised, aligned, pointer-sized global.
    unsafe {
        match library.get::<*const c_void>(format!("epoxy_{name}")).ok().and_then(|symbol| symbol.try_as_raw_ptr()) {
            Some(variable) => *variable.cast::<*const c_void>(),
            None => std::ptr::null(),
        }
    }
}

impl Gl {
    /// Loads the functions for `context`, which is made current.
    pub fn load(context: &gdk::GLContext) -> Result<Gl, String> {
        let library = epoxy()?;
        context.make_current();
        // SAFETY: `context` is current (made so just above), which glow's loader requires to
        // read GL_VERSION; the loader returns libepoxy's dispatch pointers or null.
        let gl = unsafe { glow::Context::from_loader_function_cstr(|name| entry_point(library, name)) };
        // SAFETY: the context is current; an empty vertex array object is what core profiles
        // require to be bound for attribute-less draws.
        let vertex_array = unsafe { gl.create_vertex_array() }?;
        Ok(Gl(Rc::new(Shared { gl, context: context.clone(), vertex_array })))
    }

    fn gl(&self) -> &glow::Context {
        self.0.current()
    }

    pub fn is_es(&self) -> bool {
        self.0.gl.version().is_embedded
    }

    /// GL_VERSION and GL_RENDERER.
    pub fn description(&self) -> String {
        let gl = self.gl();
        // SAFETY: the context is current; both are plain string queries.
        unsafe { format!("{} ({})", gl.get_parameter_string(glow::VERSION), gl.get_parameter_string(glow::RENDERER)) }
    }

    /// The accumulation format this context can render to and filter, best first.
    pub fn float_formats(&self) -> Vec<FloatFormat> {
        let gl = &self.0.gl;
        if !self.is_es() {
            return vec![FloatFormat::R32F, FloatFormat::R16F];
        }
        let has = |name: &str| gl.supported_extensions().contains(name);
        let version = gl.version();
        let float_buffer = has("GL_EXT_color_buffer_float") || (version.major, version.minor) >= (3, 2);
        let mut formats = Vec::new();
        if float_buffer && has("GL_OES_texture_float_linear") {
            formats.push(FloatFormat::R32F);
        }
        if float_buffer || has("GL_EXT_color_buffer_half_float") {
            formats.push(FloatFormat::R16F);
        }
        formats
    }

    /// Compiles and links a program from shader bodies (without `#version`); the uniform
    /// block `Params` is bound to binding point 0.
    pub fn program(&self, vertex: &str, fragment: &str) -> Result<Program, String> {
        let header = if self.is_es() { "#version 300 es\n" } else { "#version 150\n" };
        let gl = self.gl();
        // SAFETY: the context is current; the shaders and program are created here and every
        // one is either deleted on failure or owned by the returned `Program`.
        unsafe {
            let program = gl.create_program()?;
            let mut shaders = Vec::new();
            for (kind, body) in [(glow::VERTEX_SHADER, vertex), (glow::FRAGMENT_SHADER, fragment)] {
                let shader = gl.create_shader(kind)?;
                gl.shader_source(shader, &format!("{header}{body}"));
                gl.compile_shader(shader);
                shaders.push(shader);
                if !gl.get_shader_compile_status(shader) {
                    let log = gl.get_shader_info_log(shader);
                    shaders.into_iter().for_each(|shader| gl.delete_shader(shader));
                    gl.delete_program(program);
                    return Err(format!("shader compilation failed: {log}"));
                }
                gl.attach_shader(program, shader);
            }
            if !self.is_es() {
                gl.bind_frag_data_location(program, 0, "frag");
            }
            gl.link_program(program);
            for shader in shaders {
                gl.detach_shader(program, shader);
                gl.delete_shader(shader);
            }
            if !gl.get_program_link_status(program) {
                let log = gl.get_program_info_log(program);
                gl.delete_program(program);
                return Err(format!("program link failed: {log}"));
            }
            if let Some(block) = gl.get_uniform_block_index(program, "Params") {
                gl.uniform_block_binding(program, block, 0);
            }
            Ok(Program { gl: self.clone(), program })
        }
    }

    /// A uniform buffer of `size` bytes, bound to binding point 0 for draws.
    pub fn uniform_buffer(&self, size: usize) -> Result<UniformBuffer, String> {
        let gl = self.gl();
        // SAFETY: the context is current; the buffer is created here and owned by the result.
        unsafe {
            let buffer = gl.create_buffer()?;
            gl.bind_buffer(glow::UNIFORM_BUFFER, Some(buffer));
            gl.buffer_data_size(
                glow::UNIFORM_BUFFER,
                i32::try_from(size).map_err(|e| e.to_string())?,
                glow::DYNAMIC_DRAW,
            );
            gl.bind_buffer(glow::UNIFORM_BUFFER, None);
            Ok(UniformBuffer { gl: self.clone(), buffer, size })
        }
    }

    /// An RGBA8 texture with linear filtering and clamped edges.
    pub fn rgba_texture(&self, width: i32, height: i32, pixels: &[u8]) -> Result<Texture, String> {
        if width < 1 || height < 1 || pixels.len() != width as usize * height as usize * 4 {
            return Err(format!("{} bytes for a {width}x{height} RGBA texture", pixels.len()));
        }
        let texture = self.texture()?;
        let gl = self.gl();
        // SAFETY: the context is current, the texture is ours and `pixels` holds exactly
        // width x height RGBA8 texels with rows of width * 4 bytes (alignment 1).
        unsafe {
            gl.bind_texture(glow::TEXTURE_2D, Some(texture.texture));
            gl.pixel_store_i32(glow::UNPACK_ALIGNMENT, 1);
            gl.tex_image_2d(
                glow::TEXTURE_2D,
                0,
                glow::RGBA8 as i32,
                width,
                height,
                0,
                glow::RGBA,
                glow::UNSIGNED_BYTE,
                glow::PixelUnpackData::Slice(Some(pixels)),
            );
            gl.bind_texture(glow::TEXTURE_2D, None);
        }
        Ok(texture)
    }

    fn texture(&self) -> Result<Texture, String> {
        let gl = self.gl();
        // SAFETY: the context is current; the texture is created here and owned by the result.
        unsafe {
            let texture = gl.create_texture()?;
            gl.bind_texture(glow::TEXTURE_2D, Some(texture));
            for (parameter, value) in [
                (glow::TEXTURE_MIN_FILTER, glow::LINEAR),
                (glow::TEXTURE_MAG_FILTER, glow::LINEAR),
                (glow::TEXTURE_WRAP_S, glow::CLAMP_TO_EDGE),
                (glow::TEXTURE_WRAP_T, glow::CLAMP_TO_EDGE),
            ] {
                gl.tex_parameter_i32(glow::TEXTURE_2D, parameter, value as i32);
            }
            gl.bind_texture(glow::TEXTURE_2D, None);
            Ok(Texture { gl: self.clone(), texture })
        }
    }

    /// A single-channel float texture with its framebuffer, cleared to 0; an error when the
    /// framebuffer is incomplete in this format.
    pub fn float_target(&self, width: i32, height: i32, format: FloatFormat) -> Result<Target, String> {
        let texture = self.texture()?;
        let (internal, kind) = match format {
            FloatFormat::R32F => (glow::R32F, glow::FLOAT),
            FloatFormat::R16F => (glow::R16F, glow::HALF_FLOAT),
        };
        let gl = self.gl();
        // SAFETY: the context is current, the texture is ours and allocated without data
        // (`None`), the framebuffer is created here and owned by the result.
        let (framebuffer, status) = unsafe {
            gl.bind_texture(glow::TEXTURE_2D, Some(texture.texture));
            gl.tex_image_2d(
                glow::TEXTURE_2D,
                0,
                internal as i32,
                width.max(1),
                height.max(1),
                0,
                glow::RED,
                kind,
                glow::PixelUnpackData::Slice(None),
            );
            gl.bind_texture(glow::TEXTURE_2D, None);
            let framebuffer = gl.create_framebuffer()?;
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(framebuffer));
            gl.framebuffer_texture_2d(
                glow::FRAMEBUFFER,
                glow::COLOR_ATTACHMENT0,
                glow::TEXTURE_2D,
                Some(texture.texture),
                0,
            );
            (framebuffer, gl.check_framebuffer_status(glow::FRAMEBUFFER))
        };
        let target = Target { texture, framebuffer };
        if status != glow::FRAMEBUFFER_COMPLETE {
            return Err(format!("{format:?} framebuffer incomplete (0x{status:x})"));
        }
        self.clear(Destination::Target(&target), None);
        Ok(target)
    }

    fn bind(&self, destination: Destination<'_>) {
        if let Destination::Target(target) = destination {
            let gl = self.gl();
            // SAFETY: the context is current and the framebuffer belongs to a live `Target`.
            unsafe { gl.bind_framebuffer(glow::FRAMEBUFFER, Some(target.framebuffer)) };
        }
    }

    fn set_scissor(&self, scissor: Option<Rect>) {
        let gl = self.gl();
        // SAFETY: the context is current; scissor sizes are clamped to be non-negative.
        unsafe {
            match scissor {
                Some([x, y, width, height]) => {
                    gl.enable(glow::SCISSOR_TEST);
                    gl.scissor(x, y, width.max(0), height.max(0));
                }
                None => gl.disable(glow::SCISSOR_TEST),
            }
        }
    }

    /// Clears `destination` to (0, 0, 0, 0), inside `scissor` when given.
    pub fn clear(&self, destination: Destination<'_>, scissor: Option<Rect>) {
        self.bind(destination);
        self.set_scissor(scissor);
        let gl = self.gl();
        // SAFETY: the context is current and the destination framebuffer is bound.
        unsafe {
            gl.clear_color(0.0, 0.0, 0.0, 0.0);
            gl.clear(glow::COLOR_BUFFER_BIT);
        }
    }

    /// One full-viewport quad (six vertices) with `program`, blending off, the uniform
    /// buffer at binding 0 and `textures` on units 0, 1, ... by sampler name.
    pub fn draw(&self, draw: Draw<'_>) {
        self.bind(draw.destination);
        self.set_scissor(Some(draw.scissor));
        let gl = self.gl();
        let [x, y, width, height] = draw.viewport;
        // SAFETY: the context is current; the program, buffer, textures and vertex array are
        // live objects of this context owned by their wrappers, the destination framebuffer
        // is bound, and the vertex shader generates the six vertices from gl_VertexID.
        unsafe {
            gl.viewport(x, y, width, height);
            gl.disable(glow::BLEND);
            gl.use_program(Some(draw.program.program));
            gl.bind_vertex_array(Some(self.0.vertex_array));
            gl.bind_buffer_base(glow::UNIFORM_BUFFER, 0, Some(draw.uniforms.buffer));
            for (unit, (name, texture)) in draw.textures.iter().enumerate() {
                gl.active_texture(glow::TEXTURE0 + unit as u32);
                gl.bind_texture(glow::TEXTURE_2D, Some(texture.texture));
                let location = gl.get_uniform_location(draw.program.program, name);
                gl.uniform_1_i32(location.as_ref(), unit as i32);
            }
            gl.draw_arrays(glow::TRIANGLES, 0, 6);
            for unit in 0..draw.textures.len() {
                gl.active_texture(glow::TEXTURE0 + unit as u32);
                gl.bind_texture(glow::TEXTURE_2D, None);
            }
            gl.active_texture(glow::TEXTURE0);
            gl.use_program(None);
        }
    }

    /// Leaves the scissor test off, as GTK finds the context.
    pub fn finish(&self) {
        self.set_scissor(None);
    }
}

/// Where a clear or draw goes.
#[derive(Debug, Clone, Copy)]
pub enum Destination<'a> {
    Target(&'a Target),
    /// The framebuffer bound now (the GLArea's, after `attach_buffers`).
    Bound,
}

/// The arguments of [`Gl::draw`].
#[derive(Debug)]
pub struct Draw<'a> {
    pub program: &'a Program,
    pub destination: Destination<'a>,
    pub viewport: Rect,
    pub scissor: Rect,
    pub uniforms: &'a UniformBuffer,
    pub textures: &'a [(&'a str, &'a Texture)],
}

/// A linked program, deleted on drop.
#[derive(Debug)]
pub struct Program {
    gl: Gl,
    program: glow::Program,
}

impl Drop for Program {
    fn drop(&mut self) {
        let gl = self.gl.gl();
        // SAFETY: the context is current and the program is owned by this wrapper only.
        unsafe { gl.delete_program(self.program) };
    }
}

/// A 2D texture, deleted on drop.
#[derive(Debug)]
pub struct Texture {
    gl: Gl,
    texture: glow::Texture,
}

impl Drop for Texture {
    fn drop(&mut self) {
        let gl = self.gl.gl();
        // SAFETY: the context is current and the texture is owned by this wrapper only.
        unsafe { gl.delete_texture(self.texture) };
    }
}

/// A float texture and the framebuffer that renders into it.
#[derive(Debug)]
pub struct Target {
    texture: Texture,
    framebuffer: glow::Framebuffer,
}

impl Target {
    pub fn texture(&self) -> &Texture {
        &self.texture
    }
}

impl Drop for Target {
    fn drop(&mut self) {
        let gl = self.texture.gl.gl();
        // SAFETY: the context is current and the framebuffer is owned by this wrapper only;
        // the texture is deleted after it by its own drop.
        unsafe { gl.delete_framebuffer(self.framebuffer) };
    }
}

/// A uniform buffer, deleted on drop.
#[derive(Debug)]
pub struct UniformBuffer {
    gl: Gl,
    buffer: glow::Buffer,
    size: usize,
}

impl UniformBuffer {
    /// Replaces the contents; `data` must be the buffer's size.
    pub fn write(&self, data: &[u8]) {
        if data.len() != self.size {
            log::warn!("gaze preview: {} bytes for a {}-byte uniform buffer", data.len(), self.size);
            return;
        }
        let gl = self.gl.gl();
        // SAFETY: the context is current, the buffer is ours and `data` fills it exactly.
        unsafe {
            gl.bind_buffer(glow::UNIFORM_BUFFER, Some(self.buffer));
            gl.buffer_sub_data_u8_slice(glow::UNIFORM_BUFFER, 0, data);
            gl.bind_buffer(glow::UNIFORM_BUFFER, None);
        }
    }
}

impl Drop for UniformBuffer {
    fn drop(&mut self) {
        let gl = self.gl.gl();
        // SAFETY: the context is current and the buffer is owned by this wrapper only.
        unsafe { gl.delete_buffer(self.buffer) };
    }
}
