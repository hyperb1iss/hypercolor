use std::fmt::Write as _;

use super::ServoFramePayload;
use super::console::truncate_for_log;

fn script_preview(script: &str) -> String {
    let single_line = script.split_whitespace().collect::<Vec<_>>().join(" ");
    truncate_for_log(&single_line, 120)
}

pub(super) fn render_update_preview(
    scripts: &[String],
    frame_payloads: &[ServoFramePayload],
) -> String {
    let update_count = scripts.len() + frame_payloads.len();
    if scripts.len() == 1 && frame_payloads.is_empty() {
        return script_preview(&scripts[0]);
    }
    if scripts.is_empty() && frame_payloads.len() == 1 {
        return format!(
            "frame payload: {}",
            script_preview(frame_payloads[0].as_json())
        );
    }

    let mut previews = scripts
        .iter()
        .take(3)
        .map(|script| script_preview(script))
        .collect::<Vec<_>>();
    if previews.len() < 3 {
        previews.extend(
            frame_payloads
                .iter()
                .take(3 - previews.len())
                .map(|payload| format!("frame payload: {}", script_preview(payload.as_json()))),
        );
    }
    format!("{} updates: {}", update_count, previews.join(" | "))
}

pub(super) fn combined_script(
    buffer: &mut String,
    scripts: &[String],
    frame_payloads: &[ServoFramePayload],
) {
    let script_bytes = scripts.iter().map(String::len).sum::<usize>();
    let payload_bytes = frame_payloads
        .iter()
        .map(ServoFramePayload::len)
        .sum::<usize>();
    let capacity = script_bytes
        + payload_bytes
        + scripts.len()
        + frame_payloads.len() * "window.__hypercolorApplyFramePayload();\n".len();
    buffer.clear();
    if buffer.capacity() < capacity {
        buffer.reserve(capacity - buffer.capacity());
    }
    for script in scripts {
        buffer.push_str(script);
        buffer.push('\n');
    }
    for payload in frame_payloads {
        let _ = writeln!(
            buffer,
            "window.__hypercolorApplyFramePayload({});",
            payload.as_json()
        );
    }
}
