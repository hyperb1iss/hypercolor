use std::collections::HashMap;

use hypercolor_types::effect::{ControlValue, EffectMetadata};
use hypercolor_types::viewport::ViewportRect;

const LEGACY_SCREEN_CAST_CONTROL_IDS: [&str; 4] =
    ["frame_x", "frame_y", "frame_width", "frame_height"];

pub fn migrate_effect_controls_for_load(
    metadata: &EffectMetadata,
    controls: &HashMap<String, ControlValue>,
) -> (HashMap<String, ControlValue>, bool) {
    if !is_screen_cast(metadata) || controls.contains_key("viewport") {
        return (controls.clone(), false);
    }
    if !LEGACY_SCREEN_CAST_CONTROL_IDS
        .iter()
        .any(|control_id| controls.contains_key(*control_id))
    {
        return (controls.clone(), false);
    }

    let viewport = ViewportRect::new(
        legacy_component(controls, "frame_x", 0.0),
        legacy_component(controls, "frame_y", 0.0),
        legacy_component(controls, "frame_width", 1.0),
        legacy_component(controls, "frame_height", 1.0),
    )
    .clamp();

    let mut migrated = controls.clone();
    for control_id in LEGACY_SCREEN_CAST_CONTROL_IDS {
        migrated.remove(control_id);
    }
    migrated.insert("viewport".to_owned(), ControlValue::Rect(viewport));
    (migrated, true)
}

fn is_screen_cast(metadata: &EffectMetadata) -> bool {
    metadata
        .source
        .source_stem()
        .is_some_and(|stem| stem.eq_ignore_ascii_case("screen_cast"))
}

fn legacy_component(
    controls: &HashMap<String, ControlValue>,
    control_id: &str,
    default: f32,
) -> f32 {
    controls
        .get(control_id)
        .and_then(ControlValue::as_f32)
        .unwrap_or(default)
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::path::PathBuf;

    use hypercolor_types::effect::{
        ControlValue, EffectCategory, EffectId, EffectMetadata, EffectSource,
    };
    use hypercolor_types::viewport::ViewportRect;
    use uuid::Uuid;

    use super::migrate_effect_controls_for_load;

    fn screen_cast_metadata() -> EffectMetadata {
        EffectMetadata {
            id: EffectId::new(Uuid::nil()),
            name: "Screen Cast".into(),
            author: "test".into(),
            version: "0.1.0".into(),
            description: "test".into(),
            category: EffectCategory::Utility,
            tags: Vec::new(),
            controls: Vec::new(),
            presets: Vec::new(),
            audio_reactive: false,
            screen_reactive: true,
            source: EffectSource::Native {
                path: PathBuf::from("builtin/screen_cast"),
            },
            license: None,
        }
    }

    #[test]
    fn migrates_legacy_screen_cast_slider_controls() {
        let metadata = screen_cast_metadata();
        let controls = HashMap::from([
            ("frame_x".to_owned(), ControlValue::Float(0.2)),
            ("frame_y".to_owned(), ControlValue::Float(0.1)),
            ("frame_width".to_owned(), ControlValue::Float(0.5)),
            ("frame_height".to_owned(), ControlValue::Float(0.4)),
        ]);

        let (migrated, changed) = migrate_effect_controls_for_load(&metadata, &controls);

        assert!(changed);
        assert_eq!(
            migrated.get("viewport"),
            Some(&ControlValue::Rect(ViewportRect::new(0.2, 0.1, 0.5, 0.4)))
        );
        assert!(!migrated.contains_key("frame_x"));
        assert!(!migrated.contains_key("frame_y"));
        assert!(!migrated.contains_key("frame_width"));
        assert!(!migrated.contains_key("frame_height"));
    }
}
