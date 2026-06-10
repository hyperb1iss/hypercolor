use hypercolor_ui::api::EffectSummary;
use hypercolor_ui::effect_search::IndexedEffect;

fn effect(name: &str) -> EffectSummary {
    EffectSummary {
        id: "effect-1".to_owned(),
        name: name.to_owned(),
        description: "Cinematic ambient wash".to_owned(),
        author: "Nova".to_owned(),
        category: "ambient".to_owned(),
        source: "native".to_owned(),
        runnable: true,
        tags: vec!["cinematic".to_owned()],
        version: "1.0.0".to_owned(),
        audio_reactive: false,
        cover_image_url: None,
    }
}

#[test]
fn indexed_effect_matches_canonical_name_terms() {
    let indexed = IndexedEffect::new(effect("Blue Wave"));

    assert!(indexed.matches_search("blue wave"));
    assert!(indexed.matches_search("cinematic"));
}

#[test]
fn indexed_effect_does_not_match_legacy_name_alias_spellings() {
    let indexed = IndexedEffect::new(effect("Blue Wave"));

    assert!(!indexed.matches_search("blue_wave"));
    assert!(!indexed.matches_search("blue-wave"));
    assert!(!indexed.matches_search("bluewave"));
}
