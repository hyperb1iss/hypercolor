//! Now Playing state shared by the desktop sidebar and the phone
//! now-playing strip and sheet.
//!
//! One model owns what the player steers (the zone the main preview
//! shows), the transport actions, the ambient `--np-*` palette, and the
//! global brightness value with its throttled write, so the two shells can
//! never drift apart or race each other's brightness writes. The shell
//! provides it once inside the router; [`use_now_playing`] reads it.
//!
//! The pure rules (stepping, shuffling, the strip caption) live in free
//! functions so they are testable without a DOM.

use leptos::prelude::*;
use leptos_router::hooks::use_location;
use leptos_use_fn_debounce_throttle::use_throttle_fn_with_arg;

use crate::api;
use crate::app::{EffectsContext, FrameAnalysisContext, WsContext};
use crate::async_helpers::spawn_api_call;
use crate::color::{self, CanvasPalette};
use crate::preview_zone::{PlayerTarget, PreviewZoneContext, player_target, shown_zone_state};
use crate::route_ui::NowPlayingCanvasMode;
use crate::style_utils::category_accent_rgb;
use crate::zones::ZoneEffectState;
use crate::zones::surface::apply_zone_id;
use hypercolor_leptos_ext::prelude::random_unit;

/// The effect `direction` steps away from `current` in `list`, wrapping at
/// both ends. A current effect the list does not hold steps from the
/// first entry. `None` only for an empty list.
#[must_use]
pub fn step_effect(list: &[String], current: Option<&str>, direction: i32) -> Option<String> {
    if list.is_empty() {
        return None;
    }
    let len = i32::try_from(list.len()).ok()?;
    let index = current
        .and_then(|id| list.iter().position(|entry| entry == id))
        .unwrap_or(0);
    let index = i32::try_from(index).ok()?;
    let next = usize::try_from((index + direction).rem_euclid(len)).ok()?;
    list.get(next).cloned()
}

/// A random effect from `list` for a unit draw `roll` in `[0, 1)`, never
/// the current one while the list offers another. `None` only for an
/// empty list.
#[must_use]
pub fn shuffle_effect(list: &[String], current: Option<&str>, roll: f64) -> Option<String> {
    if list.is_empty() {
        return None;
    }
    // Truncation is the intent: the roll picks a bucket.
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::cast_precision_loss
    )]
    let mut index = ((roll.clamp(0.0, 1.0) * list.len() as f64) as usize).min(list.len() - 1);
    if list.len() > 1 && current.is_some_and(|id| list[index] == id) {
        index = (index + 1) % list.len();
    }
    list.get(index).cloned()
}

/// `word` with its first letter in upper case ("ambient" reads "Ambient").
fn capitalize(word: &str) -> String {
    let mut chars = word.chars();
    chars.next().map_or_else(String::new, |first| {
        first.to_uppercase().chain(chars).collect()
    })
}

/// The two lines the phone now-playing strip shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NowPlayingCaption {
    /// What is playing, or why nothing is.
    pub title: String,
    /// One short line of context: the category, the zone, or the state.
    pub detail: String,
}

/// Everything [`now_playing_caption`] reads, as plain values.
#[derive(Debug, Clone, Copy, Default)]
pub struct CaptionInputs<'a> {
    /// Whether anything renders: an effect, or a multi-zone scene with a
    /// zone showing something.
    pub has_active: bool,
    /// Output is paused for every device.
    pub paused: bool,
    /// The active scene has more than one LED zone.
    pub multi_zone: bool,
    /// How many LED zones the scene has.
    pub zone_count: usize,
    /// The zone the player steers, in a multi-zone scene.
    pub shown: Option<&'a ZoneEffectState>,
    /// The primary zone's effect name.
    pub effect_name: Option<&'a str>,
    /// The primary zone's effect category.
    pub category: &'a str,
}

/// The strip's caption. A multi-zone scene names the steered zone's
/// effect and the zone; a single zone names its effect and category.
#[must_use]
pub fn now_playing_caption(inputs: CaptionInputs<'_>) -> NowPlayingCaption {
    if !inputs.has_active {
        return NowPlayingCaption {
            title: "Nothing playing".to_owned(),
            detail: "Tap to pick a scene".to_owned(),
        };
    }
    let (title, detail) = match inputs.shown.filter(|_| inputs.multi_zone) {
        Some(state) => (
            state
                .display_label()
                .unwrap_or_else(|| "No effect".to_owned()),
            format!("{} \u{00b7} {} zones", state.zone.name, inputs.zone_count),
        ),
        None => (
            inputs.effect_name.unwrap_or("Untitled effect").to_owned(),
            capitalize(inputs.category),
        ),
    };
    let detail = if inputs.paused {
        "Paused".to_owned()
    } else {
        detail
    };
    NowPlayingCaption { title, detail }
}

/// The shared Now Playing model.
#[derive(Clone, Copy)]
pub struct NowPlaying {
    /// Anything is rendering (see [`CaptionInputs::has_active`]).
    pub has_active: Memo<bool>,
    /// Whether this page shows its own preview, which turns the player's
    /// thumbnail into a palette swatch.
    pub canvas_mode: Signal<NowPlayingCanvasMode>,
    /// The state of the zone the main preview shows, which the player
    /// steers.
    pub shown_zone_effect: Memo<Option<ZoneEffectState>>,
    /// Ambient palette triplets (`"r, g, b"`), published as `--np-*`.
    pub primary_rgb: Memo<String>,
    pub secondary_rgb: Memo<String>,
    pub tertiary_rgb: Memo<String>,
    /// Output is paused for every device.
    pub output_paused: Signal<bool>,
    /// Live canvas frames and their rates, for player thumbnails.
    pub canvas_frame: Signal<Option<crate::ws::CanvasFrame>>,
    pub preview_fps: Signal<f32>,
    pub preview_target_fps: Signal<u32>,
    /// Global brightness percent, as last confirmed or dragged.
    pub global_brightness: ReadSignal<u8>,
    set_global_brightness: WriteSignal<u8>,
    push_global_brightness: StoredValue<Box<dyn Fn(u8)>, LocalStorage>,
    /// Captured at construction: the transport runs from click handlers,
    /// where no reactive owner is current to look a context up.
    fx: EffectsContext,
}

impl NowPlaying {
    fn runnable_effects(self) -> Vec<String> {
        self.fx.effects_index.with_untracked(|effects| {
            effects
                .iter()
                .filter(|entry| entry.effect.runnable)
                .map(|entry| entry.effect.id.clone())
                .collect()
        })
    }

    fn player(self) -> PlayerTarget {
        let fx = self.fx;
        let target_zone = apply_zone_id(fx.zones.target_zone_untracked().as_ref());
        self.shown_zone_effect.with_untracked(|shown| {
            player_target(
                shown.as_ref(),
                fx.active_effect_id.get_untracked().as_deref(),
                target_zone.as_deref(),
            )
        })
    }

    /// Step to the previous (`-1`) or next (`1`) runnable effect.
    pub fn step(self, direction: i32) {
        let fx = self.fx;
        let PlayerTarget {
            current_effect,
            apply_zone,
        } = self.player();
        if let Some(next) = step_effect(
            &self.runnable_effects(),
            current_effect.as_deref(),
            direction,
        ) {
            fx.apply_effect_to(next, apply_zone);
        }
    }

    /// Apply a random runnable effect other than the current one.
    pub fn shuffle(self) {
        let fx = self.fx;
        let PlayerTarget {
            current_effect,
            apply_zone,
        } = self.player();
        if let Some(next) = shuffle_effect(
            &self.runnable_effects(),
            current_effect.as_deref(),
            random_unit(),
        ) {
            fx.apply_effect_to(next, apply_zone);
        }
    }

    /// Pause output when it runs, resume it when it is paused.
    pub fn toggle_output(self) {
        if self.output_paused.get_untracked() {
            self.fx.resume_effect();
        } else {
            self.fx.stop_effect();
        }
    }

    /// Show `brightness` at once and send it, throttled, to the daemon.
    pub fn set_brightness(self, brightness: u8) {
        self.set_global_brightness.set(brightness);
        self.push_global_brightness
            .with_value(|push| push(brightness));
    }
}

/// Build the model and provide it as context. Call once from the shell,
/// inside the router.
pub fn provide_now_playing() -> NowPlaying {
    let location = use_location();
    let fx = expect_context::<EffectsContext>();
    let zones_ctx = expect_context::<crate::zones::ZonesContext>();

    // Multi-zone scenes keep the player alive whenever any zone is showing
    // something, even while the primary zone sits idle.
    let has_active = Memo::new(move |_| {
        fx.active_effect_id.get().is_some()
            || (zones_ctx.multi_zone.get()
                && fx.zone_effects.with(|zones| {
                    zones
                        .iter()
                        .any(|state| state.effect_id.is_some() || state.zone.top_layer.is_some())
                }))
    });
    let canvas_mode =
        Signal::derive(move || crate::route_ui::mounted_canvas_mode(&location.pathname.get()));

    let ws = use_context::<WsContext>();
    let (canvas_frame, preview_fps, preview_target_fps): (
        Signal<Option<crate::ws::CanvasFrame>>,
        Signal<f32>,
        Signal<u32>,
    ) = match ws {
        Some(ctx) => (
            ctx.canvas_frame.into(),
            ctx.preview_fps.into(),
            ctx.preview_target_fps.into(),
        ),
        None => (
            Signal::derive(|| None),
            Signal::derive(|| 0.0_f32),
            Signal::derive(|| 0_u32),
        ),
    };
    let output_paused = ws.map_or_else(
        || Signal::derive(|| false),
        |ctx| Signal::from(ctx.output_paused),
    );

    let global_brightness_resource = api::daemon_resource(api::fetch_global_brightness);
    let (global_brightness, set_global_brightness) = signal(100_u8);
    Effect::new(move |_| {
        if let Some(Ok(brightness)) = global_brightness_resource.get() {
            set_global_brightness.set(brightness);
        }
    });
    let throttled = use_throttle_fn_with_arg(
        move |brightness: u8| {
            spawn_api_call(
                "Global brightness update failed",
                api::set_global_brightness(brightness),
            );
        },
        50.0,
    );
    let push_global_brightness = StoredValue::new_local(Box::new(move |brightness: u8| {
        throttled(brightness);
    }) as Box<dyn Fn(u8)>);

    let frame_analysis = use_context::<FrameAnalysisContext>();
    let (live_palette, set_live_palette) = signal(None::<CanvasPalette>);
    if let Some(frame_analysis) = frame_analysis {
        Effect::new(move |_| {
            if canvas_mode.get() != NowPlayingCanvasMode::Palette {
                return;
            }
            let Some(analysis) = frame_analysis.live_canvas.get() else {
                return;
            };
            let smoothed = live_palette
                .get_untracked()
                .map_or(analysis.palette, |old| {
                    color::lerp_palette(old, analysis.palette, 0.3)
                });
            set_live_palette.set(Some(smoothed));
        });
    }

    // The player acts on the zone the preview shows: the thumbnail's
    // primary zone, or the zone a page's picker focused.
    let preview_zone = use_context::<PreviewZoneContext>();
    let shown_zone_effect = Memo::new(move |last_seen: Option<&Option<ZoneEffectState>>| {
        let zone_id = preview_zone.and_then(|preview| preview.zone.get());
        fx.zone_effects.with(|zones| {
            shown_zone_state(
                zone_id.as_deref(),
                zones,
                last_seen.and_then(Option::as_ref),
            )
        })
    });

    // The accent falls back to the shown zone's effect category until the
    // live palette arrives, so the player reads as the zone it steers.
    let accent_category = Memo::new(move |_| {
        shown_zone_effect.with(|shown| match shown {
            Some(state) => state.effect_category.clone().unwrap_or_default(),
            None => fx.active_effect_category.get(),
        })
    });
    // Every live-palette color passes through `harmonize_rgb` so the three
    // hues sit in a cohesive lightness and saturation band.
    let palette_rgb = move |pick: fn(&CanvasPalette) -> (f64, f64, f64)| {
        Memo::new(move |_| {
            let category = accent_category.get();
            if canvas_mode.get() != NowPlayingCanvasMode::Palette {
                return category_accent_rgb(&category).to_string();
            }
            live_palette.get().map_or_else(
                || category_accent_rgb(&category).to_string(),
                |palette| color::rgb_string(color::harmonize_rgb(pick(&palette))),
            )
        })
    };
    let primary_rgb = palette_rgb(|palette| palette.primary);
    let secondary_rgb = palette_rgb(|palette| palette.secondary);
    let tertiary_rgb = palette_rgb(|palette| palette.tertiary);

    let model = NowPlaying {
        has_active,
        canvas_mode,
        shown_zone_effect,
        primary_rgb,
        secondary_rgb,
        tertiary_rgb,
        output_paused,
        canvas_frame,
        preview_fps,
        preview_target_fps,
        global_brightness,
        set_global_brightness,
        push_global_brightness,
        fx,
    };
    provide_context(model);
    model
}

/// The shell's Now Playing model.
#[must_use]
pub fn use_now_playing() -> NowPlaying {
    expect_context::<NowPlaying>()
}
