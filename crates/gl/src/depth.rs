//! A GL depth texture copied into a D3D11 `R32_FLOAT` texture ([`crate::Bridge::capture_depth`]):
//! depth cannot be blitted into a colour texture, so a small shader pass reads it (sample 0 of a
//! multisampled one) and writes each value as a colour, flipped to top-down like the colour copy.
//! The window depth (0..1) is copied as it is: with GL's default depth range it is what a D3D-style
//! projection of the same frustum writes.
//!
//! The pass reads through a sampler of its own (nearest texel, no depth comparison), so the game's
//! texture is read whatever its own filtering and compare mode, and none of its settings change.
//! The pass changes a good deal of GL state; everything it touches is saved first and put back.

use crate::{Error, Result, load};
use windows::Win32::Graphics::OpenGL::{
    GL_BLEND, GL_COLOR_WRITEMASK, GL_CULL_FACE, GL_DEPTH_TEST, GL_SCISSOR_TEST, GL_STENCIL_TEST, GL_TEXTURE_2D, GL_TEXTURE_BINDING_2D, GL_TRIANGLES,
    GL_VIEWPORT, glBindTexture, glColorMask, glDisable, glDrawArrays, glEnable, glGetBooleanv, glGetIntegerv, glIsEnabled, glViewport,
};

const GL_TEXTURE_2D_MULTISAMPLE: u32 = 0x9100;
const GL_TEXTURE_BINDING_2D_MULTISAMPLE: u32 = 0x9104;
const GL_TEXTURE0: u32 = 0x84C0;
const GL_ACTIVE_TEXTURE: u32 = 0x84E0;
const GL_CURRENT_PROGRAM: u32 = 0x8B8D;
const GL_VERTEX_ARRAY_BINDING: u32 = 0x85B5;
const GL_SAMPLER_BINDING: u32 = 0x8919;
const GL_TEXTURE_COMPARE_MODE: u32 = 0x884C;
const GL_TEXTURE_MIN_FILTER: u32 = 0x2801;
const GL_TEXTURE_MAG_FILTER: u32 = 0x2800;
const GL_NEAREST: i32 = 0x2600;
const GL_FRAMEBUFFER_SRGB: u32 = 0x8DB9;
const GL_RASTERIZER_DISCARD: u32 = 0x8C89;
const GL_DRAW_FRAMEBUFFER: u32 = 0x8CA9;
const GL_DRAW_FRAMEBUFFER_BINDING: u32 = 0x8CA6;
const GL_VERTEX_SHADER: u32 = 0x8B31;
const GL_FRAGMENT_SHADER: u32 = 0x8B30;
const GL_COMPILE_STATUS: u32 = 0x8B81;
const GL_LINK_STATUS: u32 = 0x8B82;

type CreateShader = unsafe extern "system" fn(u32) -> u32;
type ShaderSource = unsafe extern "system" fn(u32, i32, *const *const u8, *const i32);
type CompileShader = unsafe extern "system" fn(u32);
type GetShaderiv = unsafe extern "system" fn(u32, u32, *mut i32);
type DeleteShader = unsafe extern "system" fn(u32);
type CreateProgram = unsafe extern "system" fn() -> u32;
type AttachShader = unsafe extern "system" fn(u32, u32);
type LinkProgram = unsafe extern "system" fn(u32);
type DeleteProgram = unsafe extern "system" fn(u32);
type UseProgram = unsafe extern "system" fn(u32);
type GetUniformLocation = unsafe extern "system" fn(u32, *const u8) -> i32;
type Uniform1i = unsafe extern "system" fn(i32, i32);
type Uniform2i = unsafe extern "system" fn(i32, i32, i32);
type Uniform4i = unsafe extern "system" fn(i32, i32, i32, i32, i32);
type GenVertexArrays = unsafe extern "system" fn(i32, *mut u32);
type BindVertexArray = unsafe extern "system" fn(u32);
type DeleteVertexArrays = unsafe extern "system" fn(i32, *const u32);
type ActiveTexture = unsafe extern "system" fn(u32);
type BindSampler = unsafe extern "system" fn(u32, u32);
type GenSamplers = unsafe extern "system" fn(i32, *mut u32);
type DeleteSamplers = unsafe extern "system" fn(i32, *const u32);
type SamplerParameteri = unsafe extern "system" fn(u32, u32, i32);

/// The shader entry points the pass needs (GL 3.3), loaded at its first use.
#[derive(Clone, Copy)]
struct Functions {
    create_shader: CreateShader,
    shader_source: ShaderSource,
    compile_shader: CompileShader,
    get_shader: GetShaderiv,
    delete_shader: DeleteShader,
    create_program: CreateProgram,
    attach_shader: AttachShader,
    link_program: LinkProgram,
    get_program: GetShaderiv,
    delete_program: DeleteProgram,
    use_program: UseProgram,
    uniform_location: GetUniformLocation,
    uniform_1i: Uniform1i,
    uniform_2i: Uniform2i,
    uniform_4i: Uniform4i,
    gen_vertex_arrays: GenVertexArrays,
    bind_vertex_array: BindVertexArray,
    delete_vertex_arrays: DeleteVertexArrays,
    active_texture: ActiveTexture,
    bind_sampler: BindSampler,
    gen_samplers: GenSamplers,
    delete_samplers: DeleteSamplers,
    sampler_parameter: SamplerParameteri,
}

impl Functions {
    fn load() -> Result<Self> {
        Ok(Self {
            create_shader: load("glCreateShader")?,
            shader_source: load("glShaderSource")?,
            compile_shader: load("glCompileShader")?,
            get_shader: load("glGetShaderiv")?,
            delete_shader: load("glDeleteShader")?,
            create_program: load("glCreateProgram")?,
            attach_shader: load("glAttachShader")?,
            link_program: load("glLinkProgram")?,
            get_program: load("glGetProgramiv")?,
            delete_program: load("glDeleteProgram")?,
            use_program: load("glUseProgram")?,
            uniform_location: load("glGetUniformLocation")?,
            uniform_1i: load("glUniform1i")?,
            uniform_2i: load("glUniform2i")?,
            uniform_4i: load("glUniform4i")?,
            gen_vertex_arrays: load("glGenVertexArrays")?,
            bind_vertex_array: load("glBindVertexArray")?,
            delete_vertex_arrays: load("glDeleteVertexArrays")?,
            active_texture: load("glActiveTexture")?,
            bind_sampler: load("glBindSampler")?,
            gen_samplers: load("glGenSamplers")?,
            delete_samplers: load("glDeleteSamplers")?,
            sampler_parameter: load("glSamplerParameteri")?,
        })
    }
}

/// A full-screen triangle from the vertex number alone (no buffers).
const VERTEX: &str = "#version 150
void main() {
    vec2 corner = vec2(float((gl_VertexID << 1) & 2), float(gl_VertexID & 2));
    gl_Position = vec4(corner * 2.0 - 1.0, 0.0, 1.0);
}
";

/// Each output pixel's depth from the source rectangle `rect` (x, y of its bottom-left texel, width,
/// height, in texels) stretched over the output (`size`), the output's first row the rectangle's
/// top: GL rows run bottom-up, and the D3D texture under the output takes them top-down.
fn fragment(multisampled: bool) -> String {
    let (sampler, fetch) = if multisampled { ("sampler2DMS", "texelFetch(source, at, 0)") } else { ("sampler2D", "texelFetch(source, at, 0)") };
    format!(
        "#version 150
uniform {sampler} source;
uniform ivec4 rect;
uniform ivec2 size;
out vec4 depth;
void main() {{
    ivec2 p = ivec2(gl_FragCoord.xy);
    ivec2 at = ivec2(rect.x + p.x * rect.z / size.x, rect.y + rect.w - 1 - p.y * rect.w / size.y);
    depth = vec4({fetch}.r, 0.0, 0.0, 1.0);
}}
"
    )
}

/// One linked program and where its uniforms are.
#[derive(Clone, Copy)]
struct Program {
    program: u32,
    source: i32,
    rect: i32,
    size: i32,
}

/// The pass: its programs (plain and multisampled source), an empty vertex array and its sampler.
pub(crate) struct DepthPass {
    gl: Functions,
    programs: [Option<Program>; 2],
    vertex_array: u32,
    sampler: u32,
}

/// What the pass changes, as it was.
struct Saved {
    draw: i32,
    viewport: [i32; 4],
    program: i32,
    vertex_array: i32,
    active: i32,
    texture: i32,
    sampler: i32,
    colour_mask: [u8; 4],
    enabled: [(u32, bool); 7],
}

/// The switches the pass turns off.
const SWITCHES: [u32; 7] = [GL_DEPTH_TEST, GL_STENCIL_TEST, GL_BLEND, GL_CULL_FACE, GL_SCISSOR_TEST, GL_FRAMEBUFFER_SRGB, GL_RASTERIZER_DISCARD];

impl DepthPass {
    pub(crate) fn new() -> Result<Self> {
        let gl = Functions::load()?;
        let (mut vertex_array, mut sampler) = (0u32, 0u32);
        // SAFETY: creates an empty vertex array and a sampler with the context current.
        unsafe {
            (gl.gen_vertex_arrays)(1, &mut vertex_array);
            (gl.gen_samplers)(1, &mut sampler);
            (gl.sampler_parameter)(sampler, GL_TEXTURE_MIN_FILTER, GL_NEAREST);
            (gl.sampler_parameter)(sampler, GL_TEXTURE_MAG_FILTER, GL_NEAREST);
            (gl.sampler_parameter)(sampler, GL_TEXTURE_COMPARE_MODE, 0);
        }
        Ok(Self { gl, programs: [None, None], vertex_array, sampler })
    }

    fn compile(&self, kind: u32, text: &str) -> Result<u32> {
        let gl = self.gl;
        // SAFETY: GL shader calls with the context current; the source outlives the call.
        unsafe {
            let shader = (gl.create_shader)(kind);
            let pointer = text.as_ptr();
            let length = text.len() as i32;
            (gl.shader_source)(shader, 1, &pointer, &length);
            (gl.compile_shader)(shader);
            let mut ok = 0;
            (gl.get_shader)(shader, GL_COMPILE_STATUS, &mut ok);
            if ok == 0 {
                (gl.delete_shader)(shader);
                return Err(Error::Failed("the depth copy's shader did not compile"));
            }
            Ok(shader)
        }
    }

    fn program(&mut self, multisampled: bool) -> Result<Program> {
        let slot = usize::from(multisampled);
        if let Some(program) = self.programs[slot] {
            return Ok(program);
        }
        let gl = self.gl;
        let vertex = self.compile(GL_VERTEX_SHADER, VERTEX)?;
        let fragment = match self.compile(GL_FRAGMENT_SHADER, &fragment(multisampled)) {
            Ok(shader) => shader,
            Err(e) => {
                // SAFETY: deletes the shader just made, context current.
                unsafe { (gl.delete_shader)(vertex) };
                return Err(e);
            }
        };
        // SAFETY: links the two shaders just made into a program, context current; the shaders
        // go once linked (the program keeps them).
        unsafe {
            let program = (gl.create_program)();
            (gl.attach_shader)(program, vertex);
            (gl.attach_shader)(program, fragment);
            (gl.link_program)(program);
            (gl.delete_shader)(vertex);
            (gl.delete_shader)(fragment);
            let mut ok = 0;
            (gl.get_program)(program, GL_LINK_STATUS, &mut ok);
            if ok == 0 {
                (gl.delete_program)(program);
                return Err(Error::Failed("the depth copy's program did not link"));
            }
            let at = |name: &[u8]| (gl.uniform_location)(program, name.as_ptr());
            let made = Program { program, source: at(b"source\0"), rect: at(b"rect\0"), size: at(b"size\0") };
            self.programs[slot] = Some(made);
            Ok(made)
        }
    }

    fn save(&self, target: u32) -> Saved {
        let gl = self.gl;
        let mut saved = Saved {
            draw: 0,
            viewport: [0; 4],
            program: 0,
            vertex_array: 0,
            active: 0,
            texture: 0,
            sampler: 0,
            colour_mask: [0; 4],
            enabled: SWITCHES.map(|s| (s, false)),
        };
        // SAFETY: state queries into locals, context current; texture unit 0 is made active to read
        // its bindings (the active unit is put back after).
        unsafe {
            glGetIntegerv(GL_DRAW_FRAMEBUFFER_BINDING, &mut saved.draw);
            glGetIntegerv(GL_VIEWPORT, saved.viewport.as_mut_ptr());
            glGetIntegerv(GL_CURRENT_PROGRAM, &mut saved.program);
            glGetIntegerv(GL_VERTEX_ARRAY_BINDING, &mut saved.vertex_array);
            glGetIntegerv(GL_ACTIVE_TEXTURE, &mut saved.active);
            glGetBooleanv(GL_COLOR_WRITEMASK, saved.colour_mask.as_mut_ptr());
            for entry in saved.enabled.iter_mut() {
                entry.1 = glIsEnabled(entry.0) != 0;
            }
            (gl.active_texture)(GL_TEXTURE0);
            let binding = if target == GL_TEXTURE_2D { GL_TEXTURE_BINDING_2D } else { GL_TEXTURE_BINDING_2D_MULTISAMPLE };
            glGetIntegerv(binding, &mut saved.texture);
            glGetIntegerv(GL_SAMPLER_BINDING, &mut saved.sampler);
        }
        saved
    }

    fn restore(&self, target: u32, saved: &Saved) {
        let gl = self.gl;
        // SAFETY: puts back what `save` read, context current.
        unsafe {
            glBindTexture(target, saved.texture as u32);
            (gl.bind_sampler)(0, saved.sampler as u32);
            (gl.active_texture)(saved.active as u32);
            (gl.bind_vertex_array)(saved.vertex_array as u32);
            (gl.use_program)(saved.program as u32);
            glColorMask(saved.colour_mask[0], saved.colour_mask[1], saved.colour_mask[2], saved.colour_mask[3]);
            for (switch, on) in saved.enabled {
                if on {
                    glEnable(switch);
                }
            }
            glViewport(saved.viewport[0], saved.viewport[1], saved.viewport[2], saved.viewport[3]);
        }
    }

    /// Draws GL depth texture `source` (`multisampled`: sample 0), its rectangle `rect` (texels:
    /// x, y of the bottom-left, width, height), stretched into `framebuffer` (`width` x
    /// `height`, top row first); `bind_framebuffer` is the context's `glBindFramebuffer`.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn draw(&mut self, source: u32, multisampled: bool, rect: [i32; 4], framebuffer: u32, width: u32, height: u32, bind_framebuffer: crate::BindFramebuffer) -> Result<()> {
        let program = self.program(multisampled)?;
        let gl = self.gl;
        let target = if multisampled { GL_TEXTURE_2D_MULTISAMPLE } else { GL_TEXTURE_2D };
        let saved = self.save(target);
        // SAFETY: GL calls with the context current, on our own program, vertex array, sampler and
        // framebuffer and the game's depth texture (read only); everything changed is put back by
        // `restore`.
        unsafe {
            for (switch, on) in saved.enabled {
                if on {
                    glDisable(switch);
                }
            }
            glBindTexture(target, source);
            (gl.bind_sampler)(0, self.sampler);
            bind_framebuffer(GL_DRAW_FRAMEBUFFER, framebuffer);
            glViewport(0, 0, width as i32, height as i32);
            glColorMask(1, 1, 1, 1);
            (gl.use_program)(program.program);
            (gl.uniform_1i)(program.source, 0);
            (gl.uniform_4i)(program.rect, rect[0], rect[1], rect[2], rect[3]);
            (gl.uniform_2i)(program.size, width as i32, height as i32);
            (gl.bind_vertex_array)(self.vertex_array);
            glDrawArrays(GL_TRIANGLES, 0, 3);
            bind_framebuffer(GL_DRAW_FRAMEBUFFER, saved.draw as u32);
        }
        self.restore(target, &saved);
        Ok(())
    }

    /// Frees the programs, the vertex array and the sampler; call with the GL context current.
    pub(crate) fn release(self) {
        let gl = self.gl;
        // SAFETY: deletes the objects this pass created, context current.
        unsafe {
            (gl.delete_samplers)(1, &self.sampler);
            for program in self.programs.into_iter().flatten() {
                (gl.delete_program)(program.program);
            }
            (gl.delete_vertex_arrays)(1, &self.vertex_array);
        }
    }
}

/// The texel rectangle (x, y of the bottom-left, width, height) of a `width` x `height` texture
/// that `region` (x, y from the top-left, width, height: fractions of the texture) covers.
pub fn texel_rect(width: u32, height: u32, region: [f32; 4]) -> [i32; 4] {
    let (w, h) = (width as f32, height as f32);
    let x = (region[0] * w).round() as i32;
    let rw = ((region[2] * w).round() as i32).max(1);
    let rh = ((region[3] * h).round() as i32).max(1);
    // From the top in the region, from the bottom in GL.
    let y = height as i32 - (region[1] * h).round() as i32 - rh;
    [x, y.max(0), rw, rh]
}
