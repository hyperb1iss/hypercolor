use hypercolor_leptos_ext::canvas::{
    allocate_texture_u8, buffer_data_f32, set_canvas_size, update_texture_u8_or_reallocate,
    webgl_context,
};
use web_sys::{
    HtmlCanvasElement, WebGlBuffer, WebGlProgram, WebGlRenderingContext as Gl, WebGlShader,
    WebGlTexture,
};

use crate::ws::{CanvasFrame, CanvasPixelFormat};

use super::{PreviewRenderOutcome, TextureShape};

const PREVIEW_VERTEX_SHADER: &str = r#"
attribute vec2 a_position;
attribute vec2 a_tex_coord;
varying vec2 v_tex_coord;

void main() {
    gl_Position = vec4(a_position, 0.0, 1.0);
    v_tex_coord = a_tex_coord;
}
"#;

const PREVIEW_FRAGMENT_SHADER: &str = r#"
precision mediump float;
varying vec2 v_tex_coord;
uniform sampler2D u_texture;

void main() {
    gl_FragColor = texture2D(u_texture, v_tex_coord);
}
"#;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TextureUploadStrategy {
    Allocate,
    Update,
}

fn texture_upload_strategy(
    current_shape: Option<TextureShape>,
    next_shape: TextureShape,
) -> TextureUploadStrategy {
    if current_shape == Some(next_shape) {
        TextureUploadStrategy::Update
    } else {
        TextureUploadStrategy::Allocate
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum WebGlInitError {
    ContextUnavailable,
    InitializationFailed,
}

pub(super) struct WebGlPreviewRuntime {
    gl: Gl,
    program: WebGlProgram,
    vertex_buffer: WebGlBuffer,
    texture: WebGlTexture,
    texture_shape: Option<TextureShape>,
    smooth_scaling: bool,
}

impl WebGlPreviewRuntime {
    pub(super) fn new(
        canvas: &HtmlCanvasElement,
        smooth_scaling: bool,
    ) -> Result<Self, WebGlInitError> {
        let gl = webgl_context(canvas).ok_or(WebGlInitError::ContextUnavailable)?;

        let vertex_shader = compile_shader(&gl, Gl::VERTEX_SHADER, PREVIEW_VERTEX_SHADER)
            .ok_or(WebGlInitError::InitializationFailed)?;
        let fragment_shader = compile_shader(&gl, Gl::FRAGMENT_SHADER, PREVIEW_FRAGMENT_SHADER)
            .ok_or(WebGlInitError::InitializationFailed)?;
        let program = link_program(&gl, &vertex_shader, &fragment_shader)
            .ok_or(WebGlInitError::InitializationFailed)?;
        gl.use_program(Some(&program));

        let vertex_buffer = gl
            .create_buffer()
            .ok_or(WebGlInitError::InitializationFailed)?;
        gl.bind_buffer(Gl::ARRAY_BUFFER, Some(&vertex_buffer));

        let vertices: [f32; 16] = [
            -1.0, -1.0, 0.0, 1.0, 1.0, -1.0, 1.0, 1.0, -1.0, 1.0, 0.0, 0.0, 1.0, 1.0, 1.0, 0.0,
        ];
        buffer_data_f32(&gl, Gl::ARRAY_BUFFER, &vertices, Gl::STATIC_DRAW);

        let position_attrib = u32::try_from(gl.get_attrib_location(&program, "a_position"))
            .ok()
            .ok_or(WebGlInitError::InitializationFailed)?;
        let tex_coord_attrib = u32::try_from(gl.get_attrib_location(&program, "a_tex_coord"))
            .ok()
            .ok_or(WebGlInitError::InitializationFailed)?;
        gl.enable_vertex_attrib_array(position_attrib);
        gl.vertex_attrib_pointer_with_i32(position_attrib, 2, Gl::FLOAT, false, 16, 0);
        gl.enable_vertex_attrib_array(tex_coord_attrib);
        gl.vertex_attrib_pointer_with_i32(tex_coord_attrib, 2, Gl::FLOAT, false, 16, 8);

        let texture = gl
            .create_texture()
            .ok_or(WebGlInitError::InitializationFailed)?;
        gl.active_texture(Gl::TEXTURE0);
        gl.bind_texture(Gl::TEXTURE_2D, Some(&texture));
        gl.pixel_storei(Gl::UNPACK_ALIGNMENT, 1);
        gl.tex_parameteri(Gl::TEXTURE_2D, Gl::TEXTURE_WRAP_S, Gl::CLAMP_TO_EDGE as i32);
        gl.tex_parameteri(Gl::TEXTURE_2D, Gl::TEXTURE_WRAP_T, Gl::CLAMP_TO_EDGE as i32);
        let texture_filter = if smooth_scaling {
            Gl::LINEAR
        } else {
            Gl::NEAREST
        };
        gl.tex_parameteri(
            Gl::TEXTURE_2D,
            Gl::TEXTURE_MIN_FILTER,
            texture_filter as i32,
        );
        gl.tex_parameteri(
            Gl::TEXTURE_2D,
            Gl::TEXTURE_MAG_FILTER,
            texture_filter as i32,
        );

        if let Some(location) = gl.get_uniform_location(&program, "u_texture") {
            gl.uniform1i(Some(&location), 0);
        }

        Ok(Self {
            gl,
            program,
            vertex_buffer,
            texture,
            texture_shape: None,
            smooth_scaling,
        })
    }

    fn reinitialize_for_canvas_size(
        &mut self,
        canvas: &HtmlCanvasElement,
        width: u32,
        height: u32,
    ) -> bool {
        set_canvas_size(canvas, width, height);
        let Ok(replacement) = Self::new(canvas, self.smooth_scaling) else {
            return false;
        };
        *self = replacement;
        true
    }

    pub(super) fn render(
        &mut self,
        canvas: &HtmlCanvasElement,
        frame: &CanvasFrame,
    ) -> PreviewRenderOutcome {
        let canvas_resized = canvas.width() != frame.width || canvas.height() != frame.height;
        if canvas_resized && !self.reinitialize_for_canvas_size(canvas, frame.width, frame.height) {
            return PreviewRenderOutcome::Reinitialize;
        }

        let Ok(width) = i32::try_from(frame.width) else {
            return PreviewRenderOutcome::Reinitialize;
        };
        let Ok(height) = i32::try_from(frame.height) else {
            return PreviewRenderOutcome::Reinitialize;
        };

        self.gl.viewport(0, 0, width, height);
        self.gl.use_program(Some(&self.program));
        self.gl
            .bind_buffer(Gl::ARRAY_BUFFER, Some(&self.vertex_buffer));
        self.gl.active_texture(Gl::TEXTURE0);
        self.gl.bind_texture(Gl::TEXTURE_2D, Some(&self.texture));

        let frame_format = frame.pixel_format();
        let gl_format = match frame_format {
            CanvasPixelFormat::Rgb => Gl::RGB,
            CanvasPixelFormat::Rgba => Gl::RGBA,
            CanvasPixelFormat::Jpeg => return PreviewRenderOutcome::Reinitialize,
        };

        let shape = TextureShape {
            width: frame.width,
            height: frame.height,
            format: frame_format,
        };

        let upload_result = match texture_upload_strategy(self.texture_shape, shape) {
            TextureUploadStrategy::Allocate => {
                allocate_texture_u8(&self.gl, width, height, gl_format, frame.pixels_js())
            }
            TextureUploadStrategy::Update => update_texture_u8_or_reallocate(
                &self.gl,
                width,
                height,
                gl_format,
                frame.pixels_js(),
            ),
        };

        self.gl.draw_arrays(Gl::TRIANGLE_STRIP, 0, 4);

        if upload_result.is_err() {
            return PreviewRenderOutcome::Reinitialize;
        }

        self.texture_shape = Some(shape);
        PreviewRenderOutcome::Presented
    }
}

fn compile_shader(gl: &Gl, shader_type: u32, source: &str) -> Option<WebGlShader> {
    let shader = gl.create_shader(shader_type)?;
    gl.shader_source(&shader, source);
    gl.compile_shader(&shader);

    gl.get_shader_parameter(&shader, Gl::COMPILE_STATUS)
        .as_bool()
        .filter(|success| *success)
        .map(|_| shader)
}

fn link_program(gl: &Gl, vertex: &WebGlShader, fragment: &WebGlShader) -> Option<WebGlProgram> {
    let program = gl.create_program()?;
    gl.attach_shader(&program, vertex);
    gl.attach_shader(&program, fragment);
    gl.link_program(&program);

    gl.get_program_parameter(&program, Gl::LINK_STATUS)
        .as_bool()
        .filter(|success| *success)
        .map(|_| program)
}
