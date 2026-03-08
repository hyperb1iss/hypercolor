//! Canvas preview — presents authoritative daemon frames in the browser via WebGL.

use std::cell::RefCell;
use std::rc::Rc;

use leptos::html::Canvas;
use leptos::prelude::*;
use wasm_bindgen::JsCast;
use wasm_bindgen::closure::Closure;
use web_sys::{HtmlCanvasElement, WebGlBuffer, WebGlProgram, WebGlRenderingContext as Gl, WebGlShader, WebGlTexture};

use crate::ws::CanvasFrame;

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

struct WebGlPreview {
    gl: Gl,
    program: WebGlProgram,
    vertex_buffer: WebGlBuffer,
    texture: WebGlTexture,
    width: u32,
    height: u32,
}

impl WebGlPreview {
    fn new(canvas: &HtmlCanvasElement) -> Option<Self> {
        let gl = canvas
            .get_context("webgl")
            .ok()
            .flatten()
            .or_else(|| canvas.get_context("experimental-webgl").ok().flatten())
            .and_then(|ctx| ctx.dyn_into::<Gl>().ok())?;

        let vertex_shader = compile_shader(&gl, Gl::VERTEX_SHADER, PREVIEW_VERTEX_SHADER)?;
        let fragment_shader = compile_shader(&gl, Gl::FRAGMENT_SHADER, PREVIEW_FRAGMENT_SHADER)?;
        let program = link_program(&gl, &vertex_shader, &fragment_shader)?;
        gl.use_program(Some(&program));

        let vertex_buffer = gl.create_buffer()?;
        gl.bind_buffer(Gl::ARRAY_BUFFER, Some(&vertex_buffer));

        let vertices: [f32; 16] = [
            -1.0, -1.0, 0.0, 1.0, 1.0, -1.0, 1.0, 1.0, -1.0, 1.0, 0.0, 0.0, 1.0, 1.0, 1.0, 0.0,
        ];
        let vertex_array = js_sys::Float32Array::from(vertices.as_slice());
        gl.buffer_data_with_array_buffer_view(Gl::ARRAY_BUFFER, &vertex_array, Gl::STATIC_DRAW);

        let position_attrib = u32::try_from(gl.get_attrib_location(&program, "a_position")).ok()?;
        let tex_coord_attrib =
            u32::try_from(gl.get_attrib_location(&program, "a_tex_coord")).ok()?;
        gl.enable_vertex_attrib_array(position_attrib);
        gl.vertex_attrib_pointer_with_i32(position_attrib, 2, Gl::FLOAT, false, 16, 0);
        gl.enable_vertex_attrib_array(tex_coord_attrib);
        gl.vertex_attrib_pointer_with_i32(tex_coord_attrib, 2, Gl::FLOAT, false, 16, 8);

        let texture = gl.create_texture()?;
        gl.active_texture(Gl::TEXTURE0);
        gl.bind_texture(Gl::TEXTURE_2D, Some(&texture));
        gl.pixel_storei(Gl::UNPACK_ALIGNMENT, 1);
        gl.tex_parameteri(Gl::TEXTURE_2D, Gl::TEXTURE_WRAP_S, Gl::CLAMP_TO_EDGE as i32);
        gl.tex_parameteri(Gl::TEXTURE_2D, Gl::TEXTURE_WRAP_T, Gl::CLAMP_TO_EDGE as i32);
        gl.tex_parameteri(Gl::TEXTURE_2D, Gl::TEXTURE_MIN_FILTER, Gl::NEAREST as i32);
        gl.tex_parameteri(Gl::TEXTURE_2D, Gl::TEXTURE_MAG_FILTER, Gl::NEAREST as i32);

        if let Some(location) = gl.get_uniform_location(&program, "u_texture") {
            gl.uniform1i(Some(&location), 0);
        }

        Some(Self {
            gl,
            program,
            vertex_buffer,
            texture,
            width: 0,
            height: 0,
        })
    }

    fn render(&mut self, canvas: &HtmlCanvasElement, frame: &CanvasFrame) {
        if self.width != frame.width || self.height != frame.height {
            canvas.set_width(frame.width);
            canvas.set_height(frame.height);
            self.width = frame.width;
            self.height = frame.height;
        }

        let Ok(width) = i32::try_from(frame.width) else {
            return;
        };
        let Ok(height) = i32::try_from(frame.height) else {
            return;
        };

        self.gl.viewport(0, 0, width, height);
        self.gl.use_program(Some(&self.program));
        self.gl
            .bind_buffer(Gl::ARRAY_BUFFER, Some(&self.vertex_buffer));
        self.gl.active_texture(Gl::TEXTURE0);
        self.gl.bind_texture(Gl::TEXTURE_2D, Some(&self.texture));

        let _ = self
            .gl
            .tex_image_2d_with_i32_and_i32_and_i32_and_format_and_type_and_opt_u8_array(
                Gl::TEXTURE_2D,
                0,
                Gl::RGBA as i32,
                width,
                height,
                0,
                Gl::RGBA,
                Gl::UNSIGNED_BYTE,
                Some(frame.pixels.as_ref()),
            );
        self.gl.draw_arrays(Gl::TRIANGLE_STRIP, 0, 4);
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

/// Live canvas preview that paints authoritative canvas pixels from WebSocket frames.
#[component]
pub fn CanvasPreview(
    #[prop(into)] frame: Signal<Option<CanvasFrame>>,
    #[prop(into)] fps: Signal<f32>,
    #[prop(default = false)] show_fps: bool,
    #[prop(default = "Preview".to_string())] fps_label: String,
    #[prop(optional)] fps_target: Option<u32>,
    #[prop(default = "100%".to_string())] max_width: String,
    #[prop(optional)] aspect_ratio: Option<String>,
) -> impl IntoView {
    let canvas_ref = NodeRef::<Canvas>::new();
    let latest_frame = Rc::new(RefCell::new(None::<CanvasFrame>));
    let presenter = Rc::new(RefCell::new(None::<WebGlPreview>));
    let animation = Rc::new(RefCell::new(None::<Closure<dyn FnMut(f64)>>));
    let last_presented_frame = Rc::new(RefCell::new(None::<u32>));

    // Stash the newest frame immediately and let requestAnimationFrame present it.
    Effect::new({
        let latest_frame = Rc::clone(&latest_frame);
        move |_| {
            *latest_frame.borrow_mut() = frame.get();
        }
    });

    // Start a single browser-paced presentation loop when the canvas mounts.
    Effect::new({
        let canvas_ref = canvas_ref.clone();
        let presenter = Rc::clone(&presenter);
        let animation = Rc::clone(&animation);
        let latest_frame = Rc::clone(&latest_frame);
        let last_presented_frame = Rc::clone(&last_presented_frame);

        move |_| {
            let Some(canvas) = canvas_ref.get() else {
                return;
            };
            if presenter.borrow().is_some() {
                return;
            }

            *presenter.borrow_mut() = WebGlPreview::new(&canvas);
            if presenter.borrow().is_none() {
                return;
            }

            let Some(window) = web_sys::window() else {
                return;
            };
            let loop_window = window.clone();
            let animation_handle = Rc::clone(&animation);
            let presenter_handle = Rc::clone(&presenter);
            let canvas_ref = canvas_ref.clone();
            let latest_frame = Rc::clone(&latest_frame);
            let last_presented_frame = Rc::clone(&last_presented_frame);

            let callback = Closure::<dyn FnMut(f64)>::new(move |_| {
                if let Some(canvas) = canvas_ref.get() {
                    if let Some(frame) = latest_frame.borrow().clone()
                        && Some(frame.frame_number) != *last_presented_frame.borrow()
                    {
                        if let Some(presenter) = presenter_handle.borrow_mut().as_mut() {
                            presenter.render(&canvas, &frame);
                            *last_presented_frame.borrow_mut() = Some(frame.frame_number);
                        }
                    }

                    if let Some(callback) = animation_handle.borrow().as_ref()
                        && loop_window
                            .request_animation_frame(callback.as_ref().unchecked_ref())
                            .is_ok()
                    {
                    }
                }
            });

            *animation.borrow_mut() = Some(callback);

            if let Some(callback) = animation.borrow().as_ref()
                && window
                    .request_animation_frame(callback.as_ref().unchecked_ref())
                    .is_ok()
            {
            }
        }
    });

    let canvas_style = format!("max-width: {max_width}; image-rendering: pixelated;");
    let wrapper_style = Signal::derive(move || {
        let ratio = aspect_ratio.clone().unwrap_or_else(|| {
            frame
                .get()
                .map(|frame| format!("{} / {}", frame.width.max(1), frame.height.max(1)))
                .unwrap_or_else(|| "320 / 200".to_string())
        });
        format!("max-width: {max_width}; width: 100%; height: 100%; aspect-ratio: {ratio};")
    });

    view! {
        <div class="relative bg-black" style=move || wrapper_style.get()>
            <canvas
                node_ref=canvas_ref
                class="w-full h-full block bg-black"
                style=canvas_style
            />
            {if show_fps {
                Some(view! {
                    <div class="absolute top-2 right-2 bg-black/70 backdrop-blur-sm px-2 py-0.5 rounded text-[10px] font-mono text-fg-tertiary
                                transition-all duration-300 animate-fade-in">
                        {move || {
                            if let Some(target) = fps_target {
                                format!("{fps_label} {:.0}/{target} fps", fps.get())
                            } else {
                                format!("{fps_label} {:.0} fps", fps.get())
                            }
                        }}
                    </div>
                })
            } else {
                None
            }}
        </div>
    }
}
