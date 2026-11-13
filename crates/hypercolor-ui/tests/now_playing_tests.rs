//! The phone and sidebar player's pure rules: stepping, shuffling, and
//! the strip caption.

use std::collections::HashMap;

use hypercolor_types::scene::ZoneRole;

use hypercolor_ui::components::now_playing::{
    CaptionInputs, NowPlayingCaption, now_playing_caption, shuffle_effect, step_effect,
};
use hypercolor_ui::zones::ZoneEffectState;
use hypercolor_ui::zones::surface::Surface;

fn ids(names: &[&str]) -> Vec<String> {
    names.iter().map(|name| (*name).to_owned()).collect()
}

fn zone(name: &str, effect: Option<&str>) -> ZoneEffectState {
    ZoneEffectState {
        zone: Surface {
            id: format!("{name}-id"),
            name: name.to_owned(),
            enabled: true,
            role: ZoneRole::Custom,
            color: None,
            layer_ids: effect
                .map(|_| vec![format!("{name}-layer")])
                .unwrap_or_default(),
            top_layer: effect.map(ToOwned::to_owned),
        },
        effect_id: effect.map(|effect| format!("{effect}-id")),
        effect_name: effect.map(ToOwned::to_owned),
        effect_category: None,
        control_values: HashMap::new(),
        preset_id: None,
        revision: 1,
        layer_labels: effect
            .map(|effect| vec![effect.to_owned()])
            .unwrap_or_default(),
        featured_layer: None,
    }
}

#[test]
fn stepping_wraps_at_both_ends() {
    let list = ids(&["a", "b", "c"]);
    assert_eq!(step_effect(&list, Some("b"), 1).as_deref(), Some("c"));
    assert_eq!(step_effect(&list, Some("c"), 1).as_deref(), Some("a"));
    assert_eq!(step_effect(&list, Some("a"), -1).as_deref(), Some("c"));
}

#[test]
fn stepping_from_an_unknown_effect_starts_at_the_first() {
    let list = ids(&["a", "b", "c"]);
    assert_eq!(step_effect(&list, Some("gone"), 1).as_deref(), Some("b"));
    assert_eq!(step_effect(&list, None, -1).as_deref(), Some("c"));
    assert_eq!(step_effect(&[], Some("a"), 1), None);
}

#[test]
fn shuffle_never_repeats_the_current_effect_when_it_can() {
    let list = ids(&["a", "b", "c"]);
    // Every roll that would land on the current effect moves on.
    for roll in [0.0, 0.2, 0.5, 0.7, 0.999] {
        let pick = shuffle_effect(&list, Some("b"), roll).expect("a pick");
        assert_ne!(pick, "b", "roll {roll}");
    }
    assert_eq!(
        shuffle_effect(&ids(&["only"]), Some("only"), 0.4).as_deref(),
        Some("only")
    );
    assert_eq!(shuffle_effect(&[], None, 0.4), None);
}

#[test]
fn shuffle_clamps_out_of_range_rolls() {
    let list = ids(&["a", "b"]);
    assert_eq!(shuffle_effect(&list, None, 1.0).as_deref(), Some("b"));
    assert_eq!(shuffle_effect(&list, None, -3.0).as_deref(), Some("a"));
}

#[test]
fn an_idle_rig_invites_a_pick() {
    let caption = now_playing_caption(CaptionInputs::default());
    assert_eq!(caption.title, "Nothing playing");
    assert_eq!(caption.detail, "Tap to pick a scene");
}

#[test]
fn a_single_zone_names_its_effect_and_category() {
    let caption = now_playing_caption(CaptionInputs {
        has_active: true,
        effect_name: Some("Color Wave"),
        category: "ambient",
        ..CaptionInputs::default()
    });
    assert_eq!(
        caption,
        NowPlayingCaption {
            title: "Color Wave".to_owned(),
            detail: "Ambient".to_owned(),
        }
    );
}

#[test]
fn a_multi_zone_scene_names_the_steered_zone() {
    let desk = zone("Desk", Some("Breathing"));
    let caption = now_playing_caption(CaptionInputs {
        has_active: true,
        multi_zone: true,
        zone_count: 3,
        shown: Some(&desk),
        effect_name: Some("Color Wave"),
        category: "ambient",
        ..CaptionInputs::default()
    });
    assert_eq!(caption.title, "Breathing");
    assert_eq!(caption.detail, "Desk \u{00b7} 3 zones");

    let empty = zone("Shelf", None);
    let caption = now_playing_caption(CaptionInputs {
        has_active: true,
        multi_zone: true,
        zone_count: 2,
        shown: Some(&empty),
        ..CaptionInputs::default()
    });
    assert_eq!(caption.title, "No effect");
}

#[test]
fn paused_output_says_so() {
    let caption = now_playing_caption(CaptionInputs {
        has_active: true,
        paused: true,
        effect_name: Some("Rainbow"),
        category: "ambient",
        ..CaptionInputs::default()
    });
    assert_eq!(caption.title, "Rainbow");
    assert_eq!(caption.detail, "Paused");
}

#[test]
fn scene_radios_step_with_arrows_and_jump_with_home_and_end() {
    use hypercolor_ui::components::phone_now_playing::radio_step;
    assert_eq!(radio_step(Some(0), 4, "ArrowDown"), Some(1));
    assert_eq!(radio_step(Some(0), 4, "ArrowRight"), Some(1));
    assert_eq!(radio_step(Some(3), 4, "ArrowDown"), Some(0));
    assert_eq!(radio_step(Some(0), 4, "ArrowUp"), Some(3));
    assert_eq!(radio_step(Some(2), 4, "ArrowLeft"), Some(1));
    assert_eq!(radio_step(Some(2), 4, "Home"), Some(0));
    assert_eq!(radio_step(Some(1), 4, "End"), Some(3));
    // Focus outside the group starts from an end.
    assert_eq!(radio_step(None, 4, "ArrowDown"), Some(0));
    // Other keys stay with the browser; an empty group has nowhere to go.
    assert_eq!(radio_step(Some(1), 4, "Tab"), None);
    assert_eq!(radio_step(Some(1), 4, " "), None);
    assert_eq!(radio_step(None, 0, "ArrowDown"), None);
}
