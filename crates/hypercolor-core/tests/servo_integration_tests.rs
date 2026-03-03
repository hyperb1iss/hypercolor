#![cfg(feature = "servo")]

use std::path::PathBuf;

use hypercolor_core::effect::EffectEngine;
use hypercolor_types::audio::AudioData;
use hypercolor_types::effect::{EffectCategory, EffectId, EffectMetadata, EffectSource};
use tempfile::tempdir;
use uuid::Uuid;

fn html_metadata(path: PathBuf) -> EffectMetadata {
    EffectMetadata {
        id: EffectId::new(Uuid::now_v7()),
        name: "servo-smoke".to_owned(),
        author: "hypercolor-tests".to_owned(),
        version: "0.1.0".to_owned(),
        description: "servo smoke test".to_owned(),
        category: EffectCategory::Ambient,
        tags: vec!["servo".to_owned(), "smoke".to_owned()],
        source: EffectSource::Html { path },
        license: None,
    }
}

#[test]
#[ignore = "requires full Servo runtime and is expensive in CI/dev loops"]
fn servo_renderer_smoke_renders_temp_html_effect() {
    let tmp = tempdir().expect("tempdir should create");
    let html_path = tmp.path().join("smoke.html");
    let html = r#"<!doctype html>
<html>
<body style="margin:0;background:black;">
<canvas id="fx" width="320" height="200"></canvas>
<script>
const canvas = document.getElementById('fx');
const ctx = canvas.getContext('2d');
ctx.fillStyle = 'rgb(255,0,0)';
ctx.fillRect(0, 0, canvas.width, canvas.height);
</script>
</body>
</html>"#;
    std::fs::write(&html_path, html).expect("html write should work");

    let mut engine = EffectEngine::new();
    engine
        .activate_metadata(html_metadata(html_path))
        .expect("servo activation should succeed");

    let frame = engine
        .tick(0.016, &AudioData::silence())
        .expect("servo tick should produce a frame");

    assert_eq!(frame.width(), 320);
    assert_eq!(frame.height(), 200);
}
