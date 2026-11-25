//! Drawing the overlay inside an OpenGL game (`wglSwapBuffers` / `SwapBuffers`
//! on Windows and Proton/Wine).
//!
//! Hooks `opengl32.dll!wglSwapBuffers` and `gdi32.dll!SwapBuffers` via W^X-safe
//! IAT patching across all loaded game modules (no inline detours, no RWX
//! pages). When a call arrives with an active WGL context, we save the full GL
//! state, render egui's premultiplied-alpha triangles into the default
//! framebuffer (works in both Core and Compatibility profiles), restore every
//! GL state, and invoke the real swap function.

use super::util::{self, Backend};
use super::{Frame, TexOp, shared};
use crate::overlay::wire::VERTEX_SIZE;
use std::cell::Cell;
use std::collections::HashMap;
use std::ffi::{CString, c_char, c_int, c_uint, c_void};
use std::sync::{LazyLock, Mutex};
use std::sync::atomic::{AtomicUsize, Ordering};
use windows::Win32::Foundation::{HMODULE, HWND, RECT};
use windows::Win32::Graphics::Gdi::{HDC, WindowFromDC};
use windows::Win32::UI::WindowsAndMessaging::GetClientRect;
use windows::core::{BOOL, PCSTR, s, w};

type SwapBuffersFn = unsafe extern "system" fn(*mut c_void) -> BOOL;
type WglGetCurrentContextFn = unsafe extern "system" fn() -> *mut c_void;
type WglGetProcAddressFn = unsafe extern "system" fn(*const c_char) -> *mut c_void;

static REAL_WGL_SWAP: AtomicUsize = AtomicUsize::new(0);
static REAL_GDI_SWAP: AtomicUsize = AtomicUsize::new(0);
static WGL_GET_CTX: AtomicUsize = AtomicUsize::new(0);
static WGL_GET_PROC: AtomicUsize = AtomicUsize::new(0);
static OPENGL_MODULE: AtomicUsize = AtomicUsize::new(0);

thread_local! {
    static IN_SWAP: Cell<bool> = const { Cell::new(false) };
}

#[derive(Default)]
struct Renderers {
    contexts: HashMap<usize, GlRenderer>,
    textures: HashMap<u64, super::wire::TexSet>,
}

static RENDERERS: LazyLock<Mutex<Renderers>> = LazyLock::new(Mutex::default);
static REAL_DELETE_CTX: AtomicUsize = AtomicUsize::new(0);

/// Patch `wglSwapBuffers` / `SwapBuffers` IAT slots across all loaded game
/// modules once `opengl32.dll` is loaded. Safe to call repeatedly as new
/// modules load.
pub fn install_if_loaded() -> bool {
    unsafe {
        use windows::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
        let Ok(opengl32) = GetModuleHandleW(w!("opengl32.dll")) else {
            return false;
        };
        OPENGL_MODULE.store(opengl32.0 as usize, Ordering::Release);
        if let Some(p) = GetProcAddress(opengl32, s!("wglGetCurrentContext")) {
            WGL_GET_CTX.store(p as usize, Ordering::Release);
        }
        if let Some(p) = GetProcAddress(opengl32, s!("wglGetProcAddress")) {
            WGL_GET_PROC.store(p as usize, Ordering::Release);
        }

        let mut table = Vec::new();
        let mut skip = vec![opengl32];
        if let Some(p) = GetProcAddress(opengl32, s!("wglSwapBuffers")) {
            REAL_WGL_SWAP.compare_exchange(0, p as usize, Ordering::AcqRel, Ordering::Acquire).ok();
            table.push((REAL_WGL_SWAP.load(Ordering::Acquire), hook_wgl_swap_buffers as *const () as usize));
        }
        if let Ok(gdi32) = GetModuleHandleW(w!("gdi32.dll")) {
            skip.push(gdi32);
            if let Some(p) = GetProcAddress(gdi32, s!("SwapBuffers")) {
                REAL_GDI_SWAP.compare_exchange(0, p as usize, Ordering::AcqRel, Ordering::Acquire).ok();
                table.push((REAL_GDI_SWAP.load(Ordering::Acquire), hook_gdi_swap_buffers as *const () as usize));
            }
        }
        if let Some(p) = GetProcAddress(opengl32, s!("wglDeleteContext")) {
            REAL_DELETE_CTX.store(p as usize, Ordering::Release);
            table.push((p as usize, hook_delete_context as *const () as usize));
        }
        let patched = util::patch_all_modules(&table, &skip);
        if patched > 0 {
            crate::log!("overlay: OpenGL hooks in ({patched} IAT slots)");
        }
        patched > 0 || REAL_WGL_SWAP.load(Ordering::Acquire) != 0
    }
}

unsafe extern "system" fn hook_wgl_swap_buffers(hdc: *mut c_void) -> BOOL {
    on_swap(hdc);
    let real: SwapBuffersFn = unsafe { std::mem::transmute(REAL_WGL_SWAP.load(Ordering::Acquire)) };
    unsafe { real(hdc) }
}

unsafe extern "system" fn hook_gdi_swap_buffers(hdc: *mut c_void) -> BOOL {
    on_swap(hdc);
    let real: SwapBuffersFn = unsafe { std::mem::transmute(REAL_GDI_SWAP.load(Ordering::Acquire)) };
    unsafe { real(hdc) }
}

unsafe extern "system" fn hook_delete_context(ctx: *mut c_void) -> BOOL {
    let real: unsafe extern "system" fn(*mut c_void) -> BOOL = unsafe { std::mem::transmute(REAL_DELETE_CTX.load(Ordering::Acquire)) };
    let mut cache = RENDERERS.lock().unwrap_or_else(|e| e.into_inner());
    if ctx == unsafe { current_gl_context() } {
        cache.contexts.remove(&(ctx as usize));
    }
    let deleted = unsafe { real(ctx) };
    if deleted.as_bool() { cache.contexts.remove(&(ctx as usize)); }
    deleted
}

fn on_swap(hdc: *mut c_void) {
    if IN_SWAP.with(|f| f.replace(true)) {
        return;
    }
    let _ = std::panic::catch_unwind(|| unsafe { swap_inner(hdc) });
    IN_SWAP.with(|f| f.set(false));
}

unsafe fn current_gl_context() -> *mut c_void {
    let f = WGL_GET_CTX.load(Ordering::Acquire);
    if f == 0 {
        return std::ptr::null_mut();
    }
    let get_ctx: WglGetCurrentContextFn = unsafe { std::mem::transmute(f) };
    unsafe { get_ctx() }
}

unsafe fn swap_inner(hdc: *mut c_void) {
    let active = util::active_backend();
    if active != Backend::None && active != Backend::OpenGl {
        return;
    }
    let ctx = unsafe { current_gl_context() };
    if ctx.is_null() {
        return;
    }
    let hwnd = unsafe { WindowFromDC(HDC(hdc)) };
    if hwnd.is_invalid() {
        return;
    }
    let mut rect = RECT::default();
    if unsafe { GetClientRect(hwnd, &mut rect) }.is_err() {
        return;
    }
    let size = ((rect.right - rect.left).max(0) as u32, (rect.bottom - rect.top).max(0) as u32);
    if size.0 == 0 || size.1 == 0 {
        return;
    }

    let Ok(mut cache) = RENDERERS.lock() else { return };
    if !util::claim_backend(Backend::OpenGl) { return; }
    super::input::attach(hwnd);
    if !shared::with(|s| s.hooked) { super::hooked(size); }
    if shared::with(|s| s.size != size) { super::resized(size); }
    let (ops, frame) = shared::take_for_present();
    let key = ctx as usize;
    let Renderers { contexts, textures } = &mut *cache;
    for op in ops {
        let id = match &op { TexOp::Set(t) => t.id, TexOp::Free(id) => *id };
        for (&context, r) in contexts.iter_mut() {
            if context != key { r.dirty.insert(id); }
        }
        match &op {
            TexOp::Free(id) => { textures.remove(id); }
            TexOp::Set(t) if util::validate_tex_rgba(t.size, t.rgba.len()).is_some() => {
                if let Some([x,y]) = t.pos {
                    if let Some(full) = textures.get_mut(&id)
                        && util::validate_tex_subrect([x,y], t.size, full.size) {
                        full.linear = t.linear;
                        for (row, bytes) in t.rgba.chunks_exact(t.size[0] as usize * 4).enumerate() {
                            let offset = ((y as usize + row) * full.size[0] as usize + x as usize) * 4;
                            full.rgba[offset..offset+bytes.len()].copy_from_slice(bytes);
                        }
                    }
                } else { textures.insert(id, t.clone()); }
            }
            _ => {}
        }
        if let Some(r) = contexts.get_mut(&key) {
            if !r.dirty.contains(&id) { unsafe { r.apply(op) }; }
        }
    }
    if frame.as_ref().is_none_or(|f| f.meshes.is_empty()) { return; }
    if !contexts.contains_key(&key) {
        if contexts.len() >= 8 { return; }
        let Ok(mut renderer) = (unsafe { GlRenderer::new(key, hwnd, size) }) else { return };
        renderer.dirty.extend(textures.keys().copied());
        contexts.insert(key, renderer);
    }
    let r = contexts.get_mut(&key).unwrap();
    r.size = size;
    for id in std::mem::take(&mut r.dirty) {
        let op = textures.get(&id).cloned().map_or(TexOp::Free(id), TexOp::Set);
        unsafe { r.apply(op) };
    }
    if let Some(frame) = frame.filter(|f| !f.meshes.is_empty()) {
        unsafe { r.draw(&frame) };
    }
}

// ---- GL constants -------------------------------------------------------

const GL_FALSE: u8 = 0;
const GL_TRIANGLES: c_uint = 0x0004;
const GL_UNSIGNED_BYTE: c_uint = 0x1401;
const GL_UNSIGNED_INT: c_uint = 0x1405;
const GL_FLOAT: c_uint = 0x1406;
const GL_RGBA: c_uint = 0x1908;
const GL_NEAREST: c_int = 0x2600;
const GL_LINEAR: c_int = 0x2601;
const GL_TEXTURE_MAG_FILTER: c_uint = 0x2800;
const GL_TEXTURE_MIN_FILTER: c_uint = 0x2801;
const GL_TEXTURE_WRAP_S: c_uint = 0x2802;
const GL_TEXTURE_WRAP_T: c_uint = 0x2803;
const GL_CLAMP_TO_EDGE: c_int = 0x812F;
const GL_TEXTURE_2D: c_uint = 0x0DE1;
const GL_TEXTURE0: c_uint = 0x84C0;
const GL_ACTIVE_TEXTURE: c_uint = 0x84E0;
const GL_TEXTURE_BINDING_2D: c_uint = 0x8069;
const GL_SAMPLER_BINDING: c_uint = 0x8919;
const GL_BLEND: c_uint = 0x0BE2;
const GL_ONE: c_uint = 1;
const GL_ONE_MINUS_SRC_ALPHA: c_uint = 0x0303;
const GL_ONE_MINUS_DST_ALPHA: c_uint = 0x0305;
const GL_FUNC_ADD: c_uint = 0x8006;
const GL_BLEND_SRC_RGB: c_uint = 0x80C9;
const GL_BLEND_DST_RGB: c_uint = 0x80C8;
const GL_BLEND_SRC_ALPHA: c_uint = 0x80CB;
const GL_BLEND_DST_ALPHA: c_uint = 0x80CA;
const GL_BLEND_EQUATION_RGB: c_uint = 0x8009;
const GL_BLEND_EQUATION_ALPHA: c_uint = 0x883D;
const GL_SCISSOR_TEST: c_uint = 0x0C11;
const GL_SCISSOR_BOX: c_uint = 0x0C10;
const GL_VIEWPORT: c_uint = 0x0BA2;
const GL_DEPTH_TEST: c_uint = 0x0B71;
const GL_STENCIL_TEST: c_uint = 0x0B90;
const GL_CULL_FACE: c_uint = 0x0B44;
const GL_FRAMEBUFFER_SRGB: c_uint = 0x8DB9;
const GL_FRONT_AND_BACK: c_uint = 0x0408;
const GL_FILL: c_uint = 0x1B02;
const GL_POLYGON_MODE: c_uint = 0x0B40;
const GL_UNPACK_ROW_LENGTH: c_uint = 0x0CF2;
const GL_UNPACK_SKIP_ROWS: c_uint = 0x0CF3;
const GL_UNPACK_SKIP_PIXELS: c_uint = 0x0CF4;
const GL_UNPACK_ALIGNMENT: c_uint = 0x0CF5;
const GL_ARRAY_BUFFER: c_uint = 0x8892;
const GL_ELEMENT_ARRAY_BUFFER: c_uint = 0x8893;
const GL_ARRAY_BUFFER_BINDING: c_uint = 0x8894;
const GL_ELEMENT_ARRAY_BUFFER_BINDING: c_uint = 0x8895;
const GL_VERTEX_ARRAY_BINDING: c_uint = 0x85B5;
const GL_CURRENT_PROGRAM: c_uint = 0x8B8D;
const GL_DRAW_FRAMEBUFFER: c_uint = 0x8CA9;
const GL_READ_FRAMEBUFFER: c_uint = 0x8CA8;
const GL_DRAW_FRAMEBUFFER_BINDING: c_uint = 0x8CA6;
const GL_READ_FRAMEBUFFER_BINDING: c_uint = 0x8CAA;
const GL_STREAM_DRAW: c_uint = 0x88E0;
const GL_FRAGMENT_SHADER: c_uint = 0x8B30;
const GL_VERTEX_SHADER: c_uint = 0x8B31;
const GL_COMPILE_STATUS: c_uint = 0x8B81;
const GL_LINK_STATUS: c_uint = 0x8B82;

// ---- Dynamic GL loader --------------------------------------------------

struct GlFns {
    enable: unsafe extern "system" fn(c_uint),
    disable: unsafe extern "system" fn(c_uint),
    is_enabled: unsafe extern "system" fn(c_uint) -> u8,
    get_integerv: unsafe extern "system" fn(c_uint, *mut c_int),
    viewport: unsafe extern "system" fn(c_int, c_int, c_int, c_int),
    scissor: unsafe extern "system" fn(c_int, c_int, c_int, c_int),
    polygon_mode: unsafe extern "system" fn(c_uint, c_uint),
    gen_textures: unsafe extern "system" fn(c_int, *mut c_uint),
    delete_textures: unsafe extern "system" fn(c_int, *const c_uint),
    bind_texture: unsafe extern "system" fn(c_uint, c_uint),
    tex_parameteri: unsafe extern "system" fn(c_uint, c_uint, c_int),
    tex_image_2d: unsafe extern "system" fn(c_uint, c_int, c_int, c_int, c_int, c_int, c_uint, c_uint, *const c_void),
    tex_sub_image_2d: unsafe extern "system" fn(c_uint, c_int, c_int, c_int, c_int, c_int, c_uint, c_uint, *const c_void),
    pixel_storei: unsafe extern "system" fn(c_uint, c_int),
    draw_elements: unsafe extern "system" fn(c_uint, c_int, c_uint, *const c_void),
    active_texture: unsafe extern "system" fn(c_uint),
    blend_func_separate: unsafe extern "system" fn(c_uint, c_uint, c_uint, c_uint),
    blend_equation_separate: unsafe extern "system" fn(c_uint, c_uint),
    create_shader: unsafe extern "system" fn(c_uint) -> c_uint,
    shader_source: unsafe extern "system" fn(c_uint, c_int, *const *const c_char, *const c_int),
    compile_shader: unsafe extern "system" fn(c_uint),
    get_shaderiv: unsafe extern "system" fn(c_uint, c_uint, *mut c_int),
    delete_shader: unsafe extern "system" fn(c_uint),
    create_program: unsafe extern "system" fn() -> c_uint,
    attach_shader: unsafe extern "system" fn(c_uint, c_uint),
    link_program: unsafe extern "system" fn(c_uint),
    get_programiv: unsafe extern "system" fn(c_uint, c_uint, *mut c_int),
    use_program: unsafe extern "system" fn(c_uint),
    delete_program: unsafe extern "system" fn(c_uint),
    get_uniform_location: unsafe extern "system" fn(c_uint, *const c_char) -> c_int,
    get_attrib_location: unsafe extern "system" fn(c_uint, *const c_char) -> c_int,
    uniform1i: unsafe extern "system" fn(c_int, c_int),
    uniform2f: unsafe extern "system" fn(c_int, f32, f32),
    gen_buffers: unsafe extern "system" fn(c_int, *mut c_uint),
    delete_buffers: unsafe extern "system" fn(c_int, *const c_uint),
    bind_buffer: unsafe extern "system" fn(c_uint, c_uint),
    buffer_data: unsafe extern "system" fn(c_uint, isize, *const c_void, c_uint),
    enable_vertex_attrib_array: unsafe extern "system" fn(c_uint),
    vertex_attrib_pointer: unsafe extern "system" fn(c_uint, c_int, c_uint, u8, c_int, *const c_void),
    // Optional (GL 3.0+ / 3.3+)
    gen_vertex_arrays: Option<unsafe extern "system" fn(c_int, *mut c_uint)>,
    delete_vertex_arrays: Option<unsafe extern "system" fn(c_int, *const c_uint)>,
    bind_vertex_array: Option<unsafe extern "system" fn(c_uint)>,
    bind_framebuffer: Option<unsafe extern "system" fn(c_uint, c_uint)>,
    bind_sampler: Option<unsafe extern "system" fn(c_uint, c_uint)>,
}

unsafe fn gl_proc(name: &str) -> Option<*mut c_void> {
    use windows::Win32::System::LibraryLoader::GetProcAddress;
    let cstr = CString::new(name).ok()?;
    let gp = WGL_GET_PROC.load(Ordering::Acquire);
    if gp != 0 {
        let get_proc: WglGetProcAddressFn = unsafe { std::mem::transmute(gp) };
        let p = unsafe { get_proc(cstr.as_ptr()) };
        // wglGetProcAddress returns 0, 1, 2, 3, or -1 on failure.
        let addr = p as usize;
        if addr > 3 && addr != usize::MAX {
            return Some(p);
        }
    }
    let m = OPENGL_MODULE.load(Ordering::Acquire);
    if m != 0 {
        if let Some(p) = unsafe { GetProcAddress(HMODULE(m as *mut c_void), PCSTR(cstr.as_ptr() as *const u8)) } {
            return Some(p as *mut c_void);
        }
    }
    None
}

macro_rules! load_gl {
    ($name:literal) => {
        std::mem::transmute(gl_proc($name).ok_or_else(|| format!("missing GL proc {}", $name))?)
    };
}

macro_rules! load_gl_opt {
    ($name:literal) => {
        gl_proc($name).map(|p| std::mem::transmute(p))
    };
}

impl GlFns {
    unsafe fn load() -> Result<Self, String> {
        unsafe {
            Ok(Self {
                enable: load_gl!("glEnable"),
                disable: load_gl!("glDisable"),
                is_enabled: load_gl!("glIsEnabled"),
                get_integerv: load_gl!("glGetIntegerv"),
                viewport: load_gl!("glViewport"),
                scissor: load_gl!("glScissor"),
                polygon_mode: load_gl!("glPolygonMode"),
                gen_textures: load_gl!("glGenTextures"),
                delete_textures: load_gl!("glDeleteTextures"),
                bind_texture: load_gl!("glBindTexture"),
                tex_parameteri: load_gl!("glTexParameteri"),
                tex_image_2d: load_gl!("glTexImage2D"),
                tex_sub_image_2d: load_gl!("glTexSubImage2D"),
                pixel_storei: load_gl!("glPixelStorei"),
                draw_elements: load_gl!("glDrawElements"),
                active_texture: load_gl!("glActiveTexture"),
                blend_func_separate: load_gl!("glBlendFuncSeparate"),
                blend_equation_separate: load_gl!("glBlendEquationSeparate"),
                create_shader: load_gl!("glCreateShader"),
                shader_source: load_gl!("glShaderSource"),
                compile_shader: load_gl!("glCompileShader"),
                get_shaderiv: load_gl!("glGetShaderiv"),
                delete_shader: load_gl!("glDeleteShader"),
                create_program: load_gl!("glCreateProgram"),
                attach_shader: load_gl!("glAttachShader"),
                link_program: load_gl!("glLinkProgram"),
                get_programiv: load_gl!("glGetProgramiv"),
                use_program: load_gl!("glUseProgram"),
                delete_program: load_gl!("glDeleteProgram"),
                get_uniform_location: load_gl!("glGetUniformLocation"),
                get_attrib_location: load_gl!("glGetAttribLocation"),
                uniform1i: load_gl!("glUniform1i"),
                uniform2f: load_gl!("glUniform2f"),
                gen_buffers: load_gl!("glGenBuffers"),
                delete_buffers: load_gl!("glDeleteBuffers"),
                bind_buffer: load_gl!("glBindBuffer"),
                buffer_data: load_gl!("glBufferData"),
                enable_vertex_attrib_array: load_gl!("glEnableVertexAttribArray"),
                vertex_attrib_pointer: load_gl!("glVertexAttribPointer"),
                gen_vertex_arrays: load_gl_opt!("glGenVertexArrays"),
                delete_vertex_arrays: load_gl_opt!("glDeleteVertexArrays"),
                bind_vertex_array: load_gl_opt!("glBindVertexArray"),
                bind_framebuffer: load_gl_opt!("glBindFramebuffer"),
                bind_sampler: load_gl_opt!("glBindSampler"),
            })
        }
    }
}

const VS_330: &str = "#version 330 core\n\
uniform vec2 u_screen;\n\
in vec2 a_pos;\n\
in vec2 a_uv;\n\
in vec4 a_col;\n\
out vec2 v_uv;\n\
out vec4 v_col;\n\
void main() {\n\
    gl_Position = vec4(2.0 * a_pos.x / u_screen.x - 1.0, 1.0 - 2.0 * a_pos.y / u_screen.y, 0.0, 1.0);\n\
    v_uv = a_uv;\n\
    v_col = a_col;\n\
}\n";

const FS_330: &str = "#version 330 core\n\
uniform sampler2D u_tex;\n\
in vec2 v_uv;\n\
in vec4 v_col;\n\
out vec4 o_col;\n\
void main() {\n\
    o_col = v_col * texture(u_tex, v_uv);\n\
}\n";

const VS_120: &str = "#version 120\n\
uniform vec2 u_screen;\n\
attribute vec2 a_pos;\n\
attribute vec2 a_uv;\n\
attribute vec4 a_col;\n\
varying vec2 v_uv;\n\
varying vec4 v_col;\n\
void main() {\n\
    gl_Position = vec4(2.0 * a_pos.x / u_screen.x - 1.0, 1.0 - 2.0 * a_pos.y / u_screen.y, 0.0, 1.0);\n\
    v_uv = a_uv;\n\
    v_col = a_col;\n\
}\n";

const FS_120: &str = "#version 120\n\
uniform sampler2D u_tex;\n\
varying vec2 v_uv;\n\
varying vec4 v_col;\n\
void main() {\n\
    gl_FragColor = v_col * texture2D(u_tex, v_uv);\n\
}\n";

struct GlTex {
    id: c_uint,
    size: [u32; 2],
    linear: bool,
}

struct GlRenderer {
    dirty: std::collections::HashSet<u64>,
    hglrc: usize,
    hwnd: HWND,
    size: (u32, u32),
    gl: GlFns,
    program: c_uint,
    u_screen: c_int,
    u_tex: c_int,
    a_pos: c_uint,
    a_uv: c_uint,
    a_col: c_uint,
    vao: c_uint,
    vbo: c_uint,
    ebo: c_uint,
    textures: HashMap<u64, GlTex>,
}

unsafe impl Send for GlRenderer {}

impl Drop for GlRenderer {
    fn drop(&mut self) {
        if unsafe { current_gl_context() } as usize != self.hglrc { return; }
        unsafe {
            for tex in self.textures.values() { (self.gl.delete_textures)(1, &tex.id); }
            (self.gl.delete_buffers)(2, [self.vbo, self.ebo].as_ptr());
            if let Some(delete) = self.gl.delete_vertex_arrays { delete(1, &self.vao); }
            (self.gl.delete_program)(self.program);
        }
    }
}

impl GlRenderer {
    unsafe fn compile_shader(gl: &GlFns, kind: c_uint, src: &str) -> Option<c_uint> {
        unsafe {
            let s = (gl.create_shader)(kind);
            if s == 0 {
                return None;
            }
            let ptr = src.as_ptr() as *const c_char;
            let len = src.len() as c_int;
            (gl.shader_source)(s, 1, &ptr, &len);
            (gl.compile_shader)(s);
            let mut ok = 0;
            (gl.get_shaderiv)(s, GL_COMPILE_STATUS, &mut ok);
            if ok == 0 {
                (gl.delete_shader)(s);
                return None;
            }
            Some(s)
        }
    }

    unsafe fn build_program(gl: &GlFns, vs_src: &str, fs_src: &str) -> Option<c_uint> {
        unsafe {
            let vs = Self::compile_shader(gl, GL_VERTEX_SHADER, vs_src)?;
            let Some(fs) = Self::compile_shader(gl, GL_FRAGMENT_SHADER, fs_src) else {
                (gl.delete_shader)(vs);
                return None;
            };
            let prog = (gl.create_program)();
            (gl.attach_shader)(prog, vs);
            (gl.attach_shader)(prog, fs);
            (gl.link_program)(prog);
            (gl.delete_shader)(vs);
            (gl.delete_shader)(fs);
            let mut ok = 0;
            (gl.get_programiv)(prog, GL_LINK_STATUS, &mut ok);
            if ok == 0 {
                (gl.delete_program)(prog);
                return None;
            }
            Some(prog)
        }
    }

    unsafe fn new(hglrc: usize, hwnd: HWND, size: (u32, u32)) -> Result<Self, String> {
        unsafe {
            let gl = GlFns::load()?;
            let program = Self::build_program(&gl, VS_330, FS_330)
                .or_else(|| Self::build_program(&gl, VS_120, FS_120))
                .ok_or_else(|| "failed to compile GL overlay shaders".to_string())?;
            let u_screen = (gl.get_uniform_location)(program, c"u_screen".as_ptr());
            let u_tex = (gl.get_uniform_location)(program, c"u_tex".as_ptr());
            let a_pos = (gl.get_attrib_location)(program, c"a_pos".as_ptr());
            let a_uv = (gl.get_attrib_location)(program, c"a_uv".as_ptr());
            let a_col = (gl.get_attrib_location)(program, c"a_col".as_ptr());
            if a_pos < 0 || a_uv < 0 || a_col < 0 {
                (gl.delete_program)(program);
                return Err("missing shader attributes".into());
            }
            let (mut vao, mut vbo, mut ebo) = (0, 0, 0);
            if let Some(gen_vao) = gl.gen_vertex_arrays {
                gen_vao(1, &mut vao);
            }
            (gl.gen_buffers)(1, &mut vbo);
            (gl.gen_buffers)(1, &mut ebo);
            Ok(Self {
                dirty: Default::default(),
                hglrc,
                hwnd,
                size,
                gl,
                program,
                u_screen,
                u_tex,
                a_pos: a_pos as c_uint,
                a_uv: a_uv as c_uint,
                a_col: a_col as c_uint,
                vao,
                vbo,
                ebo,
                textures: HashMap::new(),
            })
        }
    }

    unsafe fn apply(&mut self, op: TexOp) {
        let gl = &self.gl;
        match op {
            TexOp::Free(id) => {
                if let Some(t) = self.textures.remove(&id) {
                    unsafe { (gl.delete_textures)(1, &t.id) };
                }
            }
            TexOp::Set(t) => unsafe {
                let [w, h] = t.size;
                if util::validate_tex_rgba(t.size, t.rgba.len()).is_none() {
                    return;
                }
                const GL_PIXEL_UNPACK_BUFFER: c_uint = 0x88EC;
                const GL_PIXEL_UNPACK_BUFFER_BINDING: c_uint = 0x88EF;
                let mut prev_tex = 0;
                let mut prev_align = 4;
                let mut prev_row_len = 0;
                let mut prev_skip_px = 0;
                let mut prev_skip_rows = 0;
                let mut prev_pbo = 0;
                (gl.get_integerv)(GL_TEXTURE_BINDING_2D, &mut prev_tex);
                (gl.get_integerv)(GL_UNPACK_ALIGNMENT, &mut prev_align);
                (gl.get_integerv)(GL_UNPACK_ROW_LENGTH, &mut prev_row_len);
                (gl.get_integerv)(GL_UNPACK_SKIP_PIXELS, &mut prev_skip_px);
                (gl.get_integerv)(GL_UNPACK_SKIP_ROWS, &mut prev_skip_rows);
                (gl.get_integerv)(GL_PIXEL_UNPACK_BUFFER_BINDING, &mut prev_pbo);

                if prev_pbo != 0 {
                    (gl.bind_buffer)(GL_PIXEL_UNPACK_BUFFER, 0);
                }
                (gl.pixel_storei)(GL_UNPACK_ALIGNMENT, 1);
                (gl.pixel_storei)(GL_UNPACK_ROW_LENGTH, 0);
                (gl.pixel_storei)(GL_UNPACK_SKIP_PIXELS, 0);
                (gl.pixel_storei)(GL_UNPACK_SKIP_ROWS, 0);

                let filter = if t.linear { GL_LINEAR } else { GL_NEAREST };
                match t.pos {
                    Some([x, y]) => {
                        if let Some(tex) = self.textures.get_mut(&t.id) {
                            if util::validate_tex_subrect([x, y], t.size, tex.size) {
                                tex.linear = t.linear;
                                (gl.bind_texture)(GL_TEXTURE_2D, tex.id);
                                (gl.tex_parameteri)(GL_TEXTURE_2D, GL_TEXTURE_MIN_FILTER, filter);
                                (gl.tex_parameteri)(GL_TEXTURE_2D, GL_TEXTURE_MAG_FILTER, filter);
                                (gl.tex_sub_image_2d)(GL_TEXTURE_2D, 0, x as c_int, y as c_int, w as c_int, h as c_int, GL_RGBA, GL_UNSIGNED_BYTE, t.rgba.as_ptr().cast());
                            }
                        }
                    }
                    None => {
                        let gl_id = if let Some(old) = self.textures.get(&t.id) {
                            old.id
                        } else {
                            let mut new_id = 0;
                            (gl.gen_textures)(1, &mut new_id);
                            new_id
                        };
                        (gl.bind_texture)(GL_TEXTURE_2D, gl_id);
                        (gl.tex_parameteri)(GL_TEXTURE_2D, GL_TEXTURE_MIN_FILTER, filter);
                        (gl.tex_parameteri)(GL_TEXTURE_2D, GL_TEXTURE_MAG_FILTER, filter);
                        (gl.tex_parameteri)(GL_TEXTURE_2D, GL_TEXTURE_WRAP_S, GL_CLAMP_TO_EDGE);
                        (gl.tex_parameteri)(GL_TEXTURE_2D, GL_TEXTURE_WRAP_T, GL_CLAMP_TO_EDGE);
                        (gl.tex_image_2d)(GL_TEXTURE_2D, 0, GL_RGBA as c_int, w as c_int, h as c_int, 0, GL_RGBA, GL_UNSIGNED_BYTE, t.rgba.as_ptr().cast());
                        self.textures.insert(t.id, GlTex { id: gl_id, size: t.size, linear: t.linear });
                    }
                }

                if prev_pbo != 0 {
                    (gl.bind_buffer)(GL_PIXEL_UNPACK_BUFFER, prev_pbo as c_uint);
                }

                (gl.pixel_storei)(GL_UNPACK_ALIGNMENT, prev_align);
                (gl.pixel_storei)(GL_UNPACK_ROW_LENGTH, prev_row_len);
                (gl.pixel_storei)(GL_UNPACK_SKIP_PIXELS, prev_skip_px);
                (gl.pixel_storei)(GL_UNPACK_SKIP_ROWS, prev_skip_rows);
                (gl.bind_texture)(GL_TEXTURE_2D, prev_tex as c_uint);
            },
        }
    }

    unsafe fn draw(&mut self, frame: &Frame) {
        let gl = &self.gl;
        unsafe {
            // Save GL state.
            let mut prev_prog = 0;
            let mut prev_active_tex = 0;
            let mut prev_tex = 0;
            let mut prev_sampler = 0;
            let mut prev_vao = 0;
            let mut prev_vbo = 0;
            let mut prev_ebo = 0;
            let mut prev_draw_fbo = 0;
            let mut prev_read_fbo = 0;
            let mut prev_vp = [0i32; 4];
            let mut prev_scissor = [0i32; 4];
            let mut prev_poly = [GL_FILL as c_int; 2];
            let mut prev_src_rgb = 0;
            let mut prev_dst_rgb = 0;
            let mut prev_src_a = 0;
            let mut prev_dst_a = 0;
            let mut prev_eq_rgb = 0;
            let mut prev_eq_a = 0;

            (gl.get_integerv)(GL_CURRENT_PROGRAM, &mut prev_prog);
            (gl.get_integerv)(GL_ACTIVE_TEXTURE, &mut prev_active_tex);
            (gl.active_texture)(GL_TEXTURE0);
            (gl.get_integerv)(GL_TEXTURE_BINDING_2D, &mut prev_tex);
            if gl.bind_sampler.is_some() {
                (gl.get_integerv)(GL_SAMPLER_BINDING, &mut prev_sampler);
            }
            if gl.bind_vertex_array.is_some() {
                (gl.get_integerv)(GL_VERTEX_ARRAY_BINDING, &mut prev_vao);
            }
            (gl.get_integerv)(GL_ARRAY_BUFFER_BINDING, &mut prev_vbo);
            (gl.get_integerv)(GL_ELEMENT_ARRAY_BUFFER_BINDING, &mut prev_ebo);
            if gl.bind_framebuffer.is_some() {
                (gl.get_integerv)(GL_DRAW_FRAMEBUFFER_BINDING, &mut prev_draw_fbo);
                (gl.get_integerv)(GL_READ_FRAMEBUFFER_BINDING, &mut prev_read_fbo);
            }
            (gl.get_integerv)(GL_VIEWPORT, prev_vp.as_mut_ptr());
            (gl.get_integerv)(GL_SCISSOR_BOX, prev_scissor.as_mut_ptr());
            (gl.get_integerv)(GL_POLYGON_MODE, prev_poly.as_mut_ptr());
            (gl.get_integerv)(GL_BLEND_SRC_RGB, &mut prev_src_rgb);
            (gl.get_integerv)(GL_BLEND_DST_RGB, &mut prev_dst_rgb);
            (gl.get_integerv)(GL_BLEND_SRC_ALPHA, &mut prev_src_a);
            (gl.get_integerv)(GL_BLEND_DST_ALPHA, &mut prev_dst_a);
            (gl.get_integerv)(GL_BLEND_EQUATION_RGB, &mut prev_eq_rgb);
            (gl.get_integerv)(GL_BLEND_EQUATION_ALPHA, &mut prev_eq_a);

            let was_blend = (gl.is_enabled)(GL_BLEND) != 0;
            let was_scissor = (gl.is_enabled)(GL_SCISSOR_TEST) != 0;
            let was_depth = (gl.is_enabled)(GL_DEPTH_TEST) != 0;
            let was_stencil = (gl.is_enabled)(GL_STENCIL_TEST) != 0;
            let was_cull = (gl.is_enabled)(GL_CULL_FACE) != 0;
            let was_srgb = (gl.is_enabled)(GL_FRAMEBUFFER_SRGB) != 0;

            // Set overlay state.
            if let Some(bind_fbo) = gl.bind_framebuffer {
                bind_fbo(GL_DRAW_FRAMEBUFFER, 0);
            }
            if let Some(bind_sampler) = gl.bind_sampler {
                bind_sampler(0, 0);
            }
            (gl.enable)(GL_BLEND);
            (gl.blend_equation_separate)(GL_FUNC_ADD, GL_FUNC_ADD);
            (gl.blend_func_separate)(GL_ONE, GL_ONE_MINUS_SRC_ALPHA, GL_ONE_MINUS_DST_ALPHA, GL_ONE);
            (gl.enable)(GL_SCISSOR_TEST);
            (gl.disable)(GL_DEPTH_TEST);
            (gl.disable)(GL_STENCIL_TEST);
            (gl.disable)(GL_CULL_FACE);
            (gl.disable)(GL_FRAMEBUFFER_SRGB);
            (gl.polygon_mode)(GL_FRONT_AND_BACK, GL_FILL);

            let (w, h) = (self.size.0 as c_int, self.size.1 as c_int);
            let ppp = frame.pixels_per_point.max(0.1);
            (gl.viewport)(0, 0, w, h);
            (gl.use_program)(self.program);
            (gl.uniform2f)(self.u_screen, self.size.0 as f32 / ppp, self.size.1 as f32 / ppp);
            (gl.uniform1i)(self.u_tex, 0);

            if let Some(bind_vao) = gl.bind_vertex_array {
                bind_vao(self.vao);
            }
            (gl.bind_buffer)(GL_ARRAY_BUFFER, self.vbo);
            (gl.bind_buffer)(GL_ELEMENT_ARRAY_BUFFER, self.ebo);
            (gl.enable_vertex_attrib_array)(self.a_pos);
            (gl.vertex_attrib_pointer)(self.a_pos, 2, GL_FLOAT, GL_FALSE, VERTEX_SIZE as c_int, std::ptr::null());
            (gl.enable_vertex_attrib_array)(self.a_uv);
            (gl.vertex_attrib_pointer)(self.a_uv, 2, GL_FLOAT, GL_FALSE, VERTEX_SIZE as c_int, 8usize as *const c_void);
            (gl.enable_vertex_attrib_array)(self.a_col);
            (gl.vertex_attrib_pointer)(self.a_col, 4, GL_UNSIGNED_BYTE, 1, VERTEX_SIZE as c_int, 16usize as *const c_void);

            for m in &frame.meshes {
                if m.indices.is_empty() || m.vertices.is_empty() {
                    continue;
                }
                let Some([sx, sy, sw, sh]) = util::gl_scissor_rect(m.clip, ppp, self.size.0, self.size.1) else {
                    continue;
                };
                let Some(tex) = self.textures.get(&m.texture) else { continue };
                (gl.scissor)(sx, sy, sw, sh);
                (gl.bind_texture)(GL_TEXTURE_2D, tex.id);
                (gl.buffer_data)(GL_ARRAY_BUFFER, m.vertices.len() as isize, m.vertices.as_ptr().cast(), GL_STREAM_DRAW);
                (gl.buffer_data)(GL_ELEMENT_ARRAY_BUFFER, (m.indices.len() * 4) as isize, m.indices.as_ptr().cast(), GL_STREAM_DRAW);
                (gl.draw_elements)(GL_TRIANGLES, m.indices.len() as c_int, GL_UNSIGNED_INT, std::ptr::null());
            }

            // Restore GL state.
            (gl.use_program)(prev_prog as c_uint);
            if let Some(bind_vao) = gl.bind_vertex_array {
                bind_vao(prev_vao as c_uint);
            }
            (gl.bind_buffer)(GL_ARRAY_BUFFER, prev_vbo as c_uint);
            (gl.bind_buffer)(GL_ELEMENT_ARRAY_BUFFER, prev_ebo as c_uint);
            if let Some(bind_sampler) = gl.bind_sampler {
                bind_sampler(0, prev_sampler as c_uint);
            }
            (gl.bind_texture)(GL_TEXTURE_2D, prev_tex as c_uint);
            (gl.active_texture)(prev_active_tex as c_uint);
            if let Some(bind_fbo) = gl.bind_framebuffer {
                bind_fbo(GL_DRAW_FRAMEBUFFER, prev_draw_fbo as c_uint);
                bind_fbo(GL_READ_FRAMEBUFFER, prev_read_fbo as c_uint);
            }
            (gl.viewport)(prev_vp[0], prev_vp[1], prev_vp[2], prev_vp[3]);
            (gl.scissor)(prev_scissor[0], prev_scissor[1], prev_scissor[2], prev_scissor[3]);
            (gl.polygon_mode)(GL_FRONT_AND_BACK, prev_poly[0] as c_uint);
            (gl.blend_equation_separate)(prev_eq_rgb as c_uint, prev_eq_a as c_uint);
            (gl.blend_func_separate)(prev_src_rgb as c_uint, prev_dst_rgb as c_uint, prev_src_a as c_uint, prev_dst_a as c_uint);
            let restore_cap = |cap, on| {
                if on {
                    (gl.enable)(cap);
                } else {
                    (gl.disable)(cap);
                }
            };
            restore_cap(GL_BLEND, was_blend);
            restore_cap(GL_SCISSOR_TEST, was_scissor);
            restore_cap(GL_DEPTH_TEST, was_depth);
            restore_cap(GL_STENCIL_TEST, was_stencil);
            restore_cap(GL_CULL_FACE, was_cull);
            restore_cap(GL_FRAMEBUFFER_SRGB, was_srgb);
        }
    }
}
