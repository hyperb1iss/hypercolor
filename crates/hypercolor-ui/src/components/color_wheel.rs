//! HSV color wheel picker — hue ring + saturation/value square.
//!
//! Renders on an HTML canvas via `web-sys::ImageData` pixel manipulation.
//! Uses internal HSV state to avoid reactive round-trip flicker.
//! A transparent drag overlay captures mouse events outside the canvas bounds.

use leptos::prelude::*;
use wasm_bindgen::prelude::*;

use hypercolor_color::Hsv as KernelHsv;
use hypercolor_leptos_ext::canvas::{context_2d, image_data_rgba};
use hypercolor_types::canvas::{Rgb, Rgba};

// ── Canvas geometry ──────────────────────────────────────────────────────────

const CANVAS_SIZE: u32 = 220;
const RING_OUTER: f64 = 108.0;
const RING_INNER: f64 = 84.0;
const RING_MID: f64 = (RING_OUTER + RING_INNER) / 2.0;
const SQ_HALF: f64 = 57.0; // inner square half-side (fits inside ring)
const CENTER: f64 = (CANVAS_SIZE as f64) / 2.0;
const THUMB_RADIUS: f64 = 7.0;
const TAU: f64 = std::f64::consts::TAU;

// ── HSV math ─────────────────────────────────────────────────────────────────

/// The wheel's working color. `hypercolor_color::Hsv` is the conversion
/// kernel; this wrapper exists because the wheel geometry wants f64
/// angles and the canvas API hands back f64 pointer coordinates.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Hsv {
    h: f64, // 0..360
    s: f64, // 0..1
    v: f64, // 0..1
}

impl Hsv {
    #[allow(clippy::cast_possible_truncation)]
    fn to_rgb(self) -> Rgb {
        KernelHsv::new(self.h as f32, self.s as f32, self.v as f32).to_rgb()
    }

    fn to_hex(self) -> String {
        self.to_rgb().to_hex()
    }

    /// Parse the wheel's current color. A malformed value falls back to
    /// full-saturation white, which is what this widget has always shown
    /// for input it cannot read.
    fn from_hex(hex: &str) -> Self {
        Rgba::from_hex(hex.trim()).map_or(
            Self {
                h: 0.0,
                s: 1.0,
                v: 1.0,
            },
            |color| Self::from_rgb(color.to_rgb()),
        )
    }

    fn from_rgb(rgb: Rgb) -> Self {
        let hsv = KernelHsv::from_rgb(rgb);
        Self {
            h: f64::from(hsv.h),
            s: f64::from(hsv.s),
            v: f64::from(hsv.v),
        }
    }
}

// ── Hit-test regions ─────────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq)]
enum DragRegion {
    Ring,
    Square,
}

fn hit_test(x: f64, y: f64) -> Option<DragRegion> {
    let dx = x - CENTER;
    let dy = y - CENTER;
    let dist = (dx * dx + dy * dy).sqrt();

    if (RING_INNER..=RING_OUTER).contains(&dist) {
        return Some(DragRegion::Ring);
    }
    if dx.abs() <= SQ_HALF && dy.abs() <= SQ_HALF {
        return Some(DragRegion::Square);
    }
    None
}

// ── Canvas rendering ─────────────────────────────────────────────────────────

fn render_wheel(ctx: &web_sys::CanvasRenderingContext2d, hsv: Hsv) -> Result<(), JsValue> {
    let size = CANVAS_SIZE as usize;
    let mut pixels = vec![0u8; size * size * 4];

    ctx.clear_rect(0.0, 0.0, f64::from(CANVAS_SIZE), f64::from(CANVAS_SIZE));

    for py in 0..size {
        for px in 0..size {
            let x = px as f64 - CENTER;
            let y = py as f64 - CENTER;
            let dist = (x * x + y * y).sqrt();
            let idx = (py * size + px) * 4;

            if (RING_INNER..=RING_OUTER).contains(&dist) {
                let angle = y.atan2(x).to_degrees();
                let hue = (angle + 360.0) % 360.0;
                let rgb = (Hsv {
                    h: hue,
                    s: 1.0,
                    v: 1.0,
                })
                .to_rgb();
                pixels[idx] = rgb.r;
                pixels[idx + 1] = rgb.g;
                pixels[idx + 2] = rgb.b;
                pixels[idx + 3] = 255;
            } else if x.abs() <= SQ_HALF && y.abs() <= SQ_HALF {
                let s = (x + SQ_HALF) / (SQ_HALF * 2.0);
                let v = 1.0 - (y + SQ_HALF) / (SQ_HALF * 2.0);
                let rgb = (Hsv { h: hsv.h, s, v }).to_rgb();
                pixels[idx] = rgb.r;
                pixels[idx + 1] = rgb.g;
                pixels[idx + 2] = rgb.b;
                pixels[idx + 3] = 255;
            }
        }
    }

    let image_data = image_data_rgba(&pixels, CANVAS_SIZE, CANVAS_SIZE)?;
    ctx.put_image_data(&image_data, 0.0, 0.0)?;

    // Hue ring thumb
    let hue_rad = hsv.h.to_radians();
    let hue_x = CENTER + RING_MID * hue_rad.cos();
    let hue_y = CENTER + RING_MID * hue_rad.sin();
    draw_thumb(
        ctx,
        hue_x,
        hue_y,
        &(Hsv {
            h: hsv.h,
            s: 1.0,
            v: 1.0,
        })
        .to_hex(),
    );

    // SV square thumb
    let sq_x = CENTER - SQ_HALF + hsv.s * SQ_HALF * 2.0;
    let sq_y = CENTER - SQ_HALF + (1.0 - hsv.v) * SQ_HALF * 2.0;
    draw_thumb(ctx, sq_x, sq_y, &hsv.to_hex());

    Ok(())
}

fn draw_thumb(ctx: &web_sys::CanvasRenderingContext2d, x: f64, y: f64, fill_hex: &str) {
    ctx.begin_path();
    let _ = ctx.arc(x, y, THUMB_RADIUS + 2.0, 0.0, TAU);
    ctx.set_fill_style_str("rgba(0,0,0,0.3)");
    ctx.fill();

    ctx.begin_path();
    let _ = ctx.arc(x, y, THUMB_RADIUS, 0.0, TAU);
    ctx.set_stroke_style_str("white");
    ctx.set_line_width(2.5);
    ctx.stroke();

    ctx.begin_path();
    let _ = ctx.arc(x, y, THUMB_RADIUS - 1.5, 0.0, TAU);
    ctx.set_fill_style_str(fill_hex);
    ctx.fill();
}

// ── Leptos component ─────────────────────────────────────────────────────────

/// HSV color wheel with hue ring + saturation/value square.
/// Manages its own HSV state internally to avoid reactive round-trips.
/// A transparent overlay captures drag events even when the cursor leaves the canvas.
#[component]
pub fn ColorWheel(
    /// Current hex color (e.g. "#e135ff") — synced from parent when not dragging
    #[prop(into)]
    color: Signal<String>,
    /// Called with new hex color on every interaction
    on_change: Callback<String>,
) -> impl IntoView {
    let canvas_ref = NodeRef::<leptos::html::Canvas>::new();

    // Internal HSV state — source of truth during interaction
    let (hsv_state, set_hsv_state) = signal(Hsv::from_hex(&color.get_untracked()));
    let (dragging, set_dragging) = signal(Option::<DragRegion>::None);

    // Sync from parent color signal (e.g. swatch click, hex input) — guarded during drag
    Effect::new(move |_| {
        let hex = color.get();
        if dragging.get_untracked().is_none() {
            set_hsv_state.set(Hsv::from_hex(&hex));
        }
    });

    // Render whenever internal HSV changes
    Effect::new(move |_| {
        let current_hsv = hsv_state.get();
        if let Some(canvas) = canvas_ref.get() {
            let el: &web_sys::HtmlCanvasElement = &canvas;
            if let Some(ctx) = context_2d(el) {
                let _ = render_wheel(&ctx, current_hsv);
            }
        }
    });

    // Coordinate extraction — maps viewport coords to canvas space
    let get_canvas_coords = move |client_x: f64, client_y: f64| -> Option<(f64, f64)> {
        let canvas = canvas_ref.get()?;
        let el: &web_sys::HtmlCanvasElement = &canvas;
        let rect = el.get_bounding_client_rect();
        let scale_x = f64::from(CANVAS_SIZE) / rect.width();
        let scale_y = f64::from(CANVAS_SIZE) / rect.height();
        Some((
            (client_x - rect.left()) * scale_x,
            (client_y - rect.top()) * scale_y,
        ))
    };

    // Update internal HSV from canvas position, emit hex to parent
    let update_from_pos = move |x: f64, y: f64, region: DragRegion| {
        let current = hsv_state.get_untracked();
        let new_hsv = match region {
            DragRegion::Ring => {
                let angle = (y - CENTER).atan2(x - CENTER).to_degrees();
                Hsv {
                    h: (angle + 360.0) % 360.0,
                    s: current.s,
                    v: current.v,
                }
            }
            DragRegion::Square => {
                let s = ((x - (CENTER - SQ_HALF)) / (SQ_HALF * 2.0)).clamp(0.0, 1.0);
                let v = (1.0 - (y - (CENTER - SQ_HALF)) / (SQ_HALF * 2.0)).clamp(0.0, 1.0);
                Hsv { h: current.h, s, v }
            }
        };
        set_hsv_state.set(new_hsv);
        on_change.run(new_hsv.to_hex());
    };

    let on_pointer_down = move |client_x: f64, client_y: f64| {
        if let Some((x, y)) = get_canvas_coords(client_x, client_y)
            && let Some(region) = hit_test(x, y)
        {
            set_dragging.set(Some(region));
            update_from_pos(x, y, region);
        }
    };

    let on_pointer_move = move |client_x: f64, client_y: f64| {
        if let Some(region) = dragging.get_untracked()
            && let Some((x, y)) = get_canvas_coords(client_x, client_y)
        {
            update_from_pos(x, y, region);
        }
    };

    view! {
        <div class="relative">
            <canvas
                node_ref=canvas_ref
                width=CANVAS_SIZE
                height=CANVAS_SIZE
                class="cursor-crosshair select-none touch-none rounded-full"
                style=format!("width: {}px; height: {}px;", CANVAS_SIZE, CANVAS_SIZE)
                on:mousedown=move |ev| {
                    ev.prevent_default();
                    on_pointer_down(ev.client_x() as f64, ev.client_y() as f64);
                }
                on:touchstart=move |ev| {
                    ev.prevent_default();
                    if let Some(touch) = ev.touches().get(0) {
                        on_pointer_down(touch.client_x() as f64, touch.client_y() as f64);
                    }
                }
                on:touchmove=move |ev| {
                    ev.prevent_default();
                    if let Some(touch) = ev.touches().get(0) {
                        on_pointer_move(touch.client_x() as f64, touch.client_y() as f64);
                    }
                }
                on:touchend=move |_| set_dragging.set(None)
            />

            // Drag overlay — covers viewport during drag to capture events outside canvas
            <Show when=move || dragging.get().is_some()>
                <div
                    class="fixed inset-0 z-[100] cursor-crosshair"
                    on:mousemove=move |ev| {
                        ev.prevent_default();
                        on_pointer_move(ev.client_x() as f64, ev.client_y() as f64);
                    }
                    on:mouseup=move |_| set_dragging.set(None)
                />
            </Show>
        </div>
    }
}
