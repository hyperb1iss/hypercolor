use std::collections::{HashMap, HashSet};
use std::path::Path;

use hypercolor_leptos_ext::events::Change;
use leptos::html;
use leptos::prelude::*;
use leptos_icons::Icon;
use leptos_router::hooks::use_navigate;
use wasm_bindgen_futures::JsFuture;

use crate::api;
use crate::app::EffectsContext;
use crate::icons::*;
use crate::toasts;

#[derive(Clone)]
struct InstallPreview {
    author: Option<String>,
    controls: usize,
    description: Option<String>,
    errors: Vec<String>,
    file_name: String,
    presets: usize,
    title: String,
    warnings: Vec<String>,
}

#[component]
pub fn InstallEffectPanel() -> impl IntoView {
    let fx = expect_context::<EffectsContext>();
    let navigate = use_navigate();
    let input_ref = NodeRef::<html::Input>::new();
    let (panel_open, set_panel_open) = signal(false);
    let (is_parsing, set_is_parsing) = signal(false);
    let (is_uploading, set_is_uploading) = signal(false);
    let (pending_file, set_pending_file) = signal(None::<web_sys::File>);
    let (preview, set_preview) = signal(None::<InstallPreview>);

    let open_picker = {
        Callback::new(move |_| {
            if let Some(input) = input_ref.get() {
                input.click();
            }
        })
    };

    let close_panel = Callback::new(move |_| {
        set_panel_open.set(false);
        set_is_parsing.set(false);
        set_is_uploading.set(false);
    });

    let choose_another = open_picker;

    let on_change = move |ev: web_sys::Event| {
        let event = Change::from_event(ev);
        let Some(file) = event.files().and_then(|files| files.get(0)) else {
            return;
        };

        set_panel_open.set(true);
        set_is_parsing.set(true);
        set_preview.set(None);
        set_pending_file.set(None);

        let file_for_task = file.clone();
        leptos::task::spawn_local(async move {
            match read_file_text(&file_for_task).await {
                Ok(html) => {
                    set_pending_file.set(Some(file_for_task.clone()));
                    set_preview.set(Some(parse_install_preview(&file_for_task.name(), &html)));
                }
                Err(error) => {
                    log::warn!("failed to read selected effect file: {error}");
                    toasts::toast_error("Couldn't read the selected effect file.");
                    set_panel_open.set(false);
                }
            }

            set_is_parsing.set(false);
        });
    };

    let install_effect = {
        let navigate = navigate.clone();
        Callback::new(move |_| {
            if is_uploading.get_untracked() {
                return;
            }

            let Some(file) = pending_file.get_untracked() else {
                return;
            };

            set_is_uploading.set(true);
            let fx = fx;
            let navigate = navigate.clone();
            leptos::task::spawn_local(async move {
                match api::upload_effect(file).await {
                    Ok(installed) => {
                        fx.refresh_effects.run(());
                        toasts::toast_success(&format!(
                            "Installed {} with {} controls.",
                            installed.name, installed.controls
                        ));
                        set_panel_open.set(false);
                        set_preview.set(None);
                        set_pending_file.set(None);
                        if let Some(input) = input_ref.get() {
                            input.set_value("");
                        }
                        navigate("/effects", Default::default());
                    }
                    Err(error) => {
                        toasts::toast_error(&format!("Couldn't install effect: {error}"));
                    }
                }

                set_is_uploading.set(false);
            });
        })
    };

    view! {
        <div class="relative shrink-0">
            <input
                type="file"
                accept=".html,text/html"
                class="hidden"
                node_ref=input_ref
                on:change=on_change
            />

            <button
                type="button"
                class="inline-flex items-center gap-2 rounded-xl border px-3 py-2 text-xs font-medium transition-all duration-200
                       text-fg-primary bg-surface-overlay/70 border-edge-subtle hover:border-accent-muted
                       hover:bg-surface-overlay glow-ring"
                on:click=move |_| open_picker.run(())
            >
                <Icon icon=LuFolder width="14px" height="14px" />
                <span>{move || if is_uploading.get() { "Installing..." } else if is_parsing.get() { "Parsing..." } else { "Install Effect" }}</span>
            </button>

            {move || panel_open.get().then(|| view! {
                <div class="fixed inset-0 z-20" on:click=move |_| close_panel.run(()) />
                <div
                    class="absolute right-0 top-full z-30 mt-2 w-[380px] max-w-[calc(100vw-2rem)]
                           rounded-2xl border border-edge-subtle bg-surface-overlay/97 shadow-2xl
                           shadow-black/45 backdrop-blur-xl"
                >
                    <div class="flex items-center justify-between gap-3 border-b border-edge-subtle/70 px-4 py-3">
                        <div>
                            <div class="text-sm font-semibold text-fg-primary">"Install Custom Effect"</div>
                            <div class="text-[11px] text-fg-tertiary">"Preview the artifact metadata before uploading it to the daemon."</div>
                        </div>
                        <button
                            type="button"
                            class="rounded-lg border border-edge-subtle p-1.5 text-fg-tertiary transition-colors hover:text-fg-primary"
                            on:click=move |_| close_panel.run(())
                        >
                            <Icon icon=LuX width="14px" height="14px" />
                        </button>
                    </div>

                    <div class="space-y-4 px-4 py-4">
                        {move || if is_parsing.get() {
                            view! {
                                <div class="flex items-center gap-3 rounded-xl border border-edge-subtle bg-surface-sunken/40 px-4 py-4 text-sm text-fg-secondary">
                                    <span class="inline-flex animate-spin">
                                        <Icon icon=LuLoader width="16px" height="16px" />
                                    </span>
                                    <span>"Reading metadata from the selected effect…"</span>
                                </div>
                            }.into_any()
                        } else if let Some(preview) = preview.get() {
                            let has_errors = !preview.errors.is_empty();
                            let preview_for_title = preview.clone();
                            let preview_for_desc = preview.clone();
                            let preview_for_meta = preview.clone();
                            let preview_for_errors = preview.clone();
                            let preview_for_warnings = preview.clone();
                            view! {
                                <div class="space-y-4">
                                    <div class="rounded-2xl border border-edge-subtle bg-surface-sunken/45 px-4 py-4">
                                        <div class="flex items-start justify-between gap-3">
                                            <div class="min-w-0">
                                                <div class="text-sm font-semibold text-fg-primary">{preview_for_title.title}</div>
                                                <div class="mt-1 truncate text-[11px] text-fg-tertiary">{preview_for_title.file_name}</div>
                                            </div>
                                            <div
                                                class="inline-flex items-center gap-1 rounded-full px-2 py-1 text-[10px] font-medium"
                                                class=("bg-error-red/12 text-error-red border border-error-red/25", has_errors)
                                                class=("bg-success-green/12 text-success-green border border-success-green/25", !has_errors)
                                            >
                                                <Icon icon=if has_errors { LuTriangleAlert } else { LuCircleCheck } width="12px" height="12px" />
                                                {if has_errors { "Needs fixes" } else { "Ready" }}
                                            </div>
                                        </div>

                                        <div class="mt-3 text-xs leading-relaxed text-fg-secondary">
                                            {preview_for_desc.description.unwrap_or_else(|| "No description metadata found.".to_owned())}
                                        </div>

                                        <div class="mt-4 grid grid-cols-3 gap-2 text-[11px]">
                                            <div class="rounded-xl border border-edge-subtle/70 bg-surface-base/40 px-3 py-2">
                                                <div class="text-fg-tertiary">"Author"</div>
                                                <div class="mt-1 truncate text-fg-primary">{preview_for_meta.author.unwrap_or_else(|| "Unknown".to_owned())}</div>
                                            </div>
                                            <div class="rounded-xl border border-edge-subtle/70 bg-surface-base/40 px-3 py-2">
                                                <div class="text-fg-tertiary">"Controls"</div>
                                                <div class="mt-1 text-fg-primary">{preview_for_meta.controls}</div>
                                            </div>
                                            <div class="rounded-xl border border-edge-subtle/70 bg-surface-base/40 px-3 py-2">
                                                <div class="text-fg-tertiary">"Presets"</div>
                                                <div class="mt-1 text-fg-primary">{preview_for_meta.presets}</div>
                                            </div>
                                        </div>
                                    </div>

                                    {(!preview_for_errors.errors.is_empty()).then(|| view! {
                                        <div class="rounded-2xl border border-error-red/30 bg-error-red/8 px-4 py-3">
                                            <div class="mb-2 flex items-center gap-2 text-xs font-medium text-error-red">
                                                <Icon icon=LuTriangleAlert width="14px" height="14px" />
                                                "Validation errors"
                                            </div>
                                            <ul class="space-y-1 text-[11px] text-fg-secondary">
                                                {preview_for_errors.errors.into_iter().map(|error| view! {
                                                    <li>{error}</li>
                                                }).collect_view()}
                                            </ul>
                                        </div>
                                    })}

                                    {(!preview_for_warnings.warnings.is_empty()).then(|| view! {
                                        <div class="rounded-2xl border border-accent-muted/25 bg-accent-muted/6 px-4 py-3">
                                            <div class="mb-2 flex items-center gap-2 text-xs font-medium text-fg-primary">
                                                <Icon icon=LuInfo width="14px" height="14px" />
                                                "Metadata notes"
                                            </div>
                                            <ul class="space-y-1 text-[11px] text-fg-secondary">
                                                {preview_for_warnings.warnings.into_iter().map(|warning| view! {
                                                    <li>{warning}</li>
                                                }).collect_view()}
                                            </ul>
                                        </div>
                                    })}
                                </div>
                            }.into_any()
                        } else {
                            view! {
                                <div class="rounded-xl border border-dashed border-edge-subtle px-4 py-5 text-center text-xs text-fg-tertiary">
                                    "Choose a built HTML effect to preview its metadata."
                                </div>
                            }.into_any()
                        }}
                    </div>

                    <div class="flex items-center justify-between gap-3 border-t border-edge-subtle/70 px-4 py-3">
                        <button
                            type="button"
                            class="rounded-xl border border-edge-subtle px-3 py-2 text-xs text-fg-secondary transition-colors hover:text-fg-primary"
                            on:click=move |_| choose_another.run(())
                        >
                            "Choose Another"
                        </button>
                        <button
                            type="button"
                            class="inline-flex items-center gap-2 rounded-xl px-3 py-2 text-xs font-medium transition-all duration-200"
                            class=("bg-electric-purple/85 text-white hover:bg-electric-purple", move || preview.get().is_some_and(|preview| preview.errors.is_empty()) && !is_uploading.get())
                            class=("cursor-not-allowed bg-surface-sunken text-fg-tertiary", move || preview.get().is_none_or(|preview| !preview.errors.is_empty()) || is_uploading.get())
                            disabled=move || preview.get().is_none_or(|preview| !preview.errors.is_empty()) || is_uploading.get()
                            on:click=move |_| install_effect.run(())
                        >
                            <span class=("inline-flex", true) class=("animate-spin", move || is_uploading.get())>
                                <Icon icon=if is_uploading.get() { LuLoader } else { LuCheck } width="14px" height="14px" />
                            </span>
                            <span>{move || if is_uploading.get() { "Uploading…" } else { "Install to Daemon" }}</span>
                        </button>
                    </div>
                </div>
            })}
        </div>
    }
}

async fn read_file_text(file: &web_sys::File) -> Result<String, String> {
    let text = JsFuture::from(file.text())
        .await
        .map_err(|error| format!("{error:?}"))?;
    text.as_string()
        .ok_or_else(|| "selected file did not decode to text".to_owned())
}

fn parse_install_preview(file_name: &str, html: &str) -> InstallPreview {
    let sanitized = strip_html_comments(html);
    let title = extract_html_title(&sanitized).unwrap_or_else(|| file_stem(file_name));
    let mut author = None;
    let mut description = None;
    let mut version = None;
    let mut controls = 0usize;
    let mut presets = 0usize;
    let mut errors = Vec::new();
    let mut warnings = Vec::new();
    let mut seen_controls = HashSet::new();

    for tag in extract_start_tags(&sanitized, "meta") {
        let attrs = parse_tag_attributes(&tag);

        if let Some(preset_name) = attr_value(&attrs, "preset") {
            presets += 1;
            let Some(raw_controls) = attr_value(&attrs, "preset-controls") else {
                errors.push(format!(
                    "Preset \"{}\" is missing preset-controls JSON.",
                    normalize_whitespace(preset_name)
                ));
                continue;
            };

            if serde_json::from_str::<serde_json::Value>(raw_controls)
                .ok()
                .and_then(|value| value.as_object().cloned())
                .is_none()
            {
                errors.push(format!(
                    "Preset \"{}\" has invalid preset-controls JSON.",
                    normalize_whitespace(preset_name)
                ));
            }
            continue;
        }

        if let Some(property) = attr_value(&attrs, "property") {
            controls += 1;
            if !seen_controls.insert(property.to_owned()) {
                errors.push(format!("Duplicate control property \"{property}\"."));
            }

            let control_type = attr_value(&attrs, "type")
                .unwrap_or("number")
                .trim()
                .to_ascii_lowercase();
            if !matches!(
                control_type.as_str(),
                "number"
                    | "boolean"
                    | "color"
                    | "combobox"
                    | "dropdown"
                    | "hue"
                    | "text"
                    | "textfield"
                    | "input"
                    | "sensor"
                    | "area"
                    | "rect"
            ) {
                errors.push(format!(
                    "Control \"{property}\" uses unknown type \"{control_type}\"."
                ));
            }

            if matches!(control_type.as_str(), "combobox" | "dropdown")
                && attr_value(&attrs, "values").is_none()
            {
                errors.push(format!(
                    "Control \"{property}\" is missing its combobox values."
                ));
            }

            let min = attr_value(&attrs, "min").and_then(|value| value.parse::<f32>().ok());
            let max = attr_value(&attrs, "max").and_then(|value| value.parse::<f32>().ok());
            if let (Some(min), Some(max)) = (min, max)
                && min >= max
            {
                errors.push(format!("Control \"{property}\" has min >= max."));
            }

            continue;
        }

        description =
            description.or_else(|| attr_value(&attrs, "description").map(ToOwned::to_owned));
        author = author.or_else(|| {
            attr_value(&attrs, "publisher")
                .map(ToOwned::to_owned)
                .or_else(|| {
                    (attr_value(&attrs, "name") == Some("author"))
                        .then(|| attr_value(&attrs, "content").map(ToOwned::to_owned))
                        .flatten()
                })
        });
        version = version.or_else(|| {
            (attr_value(&attrs, "name") == Some("hypercolor-version"))
                .then(|| attr_value(&attrs, "content").map(ToOwned::to_owned))
                .flatten()
                .or_else(|| attr_value(&attrs, "hypercolor-version").map(ToOwned::to_owned))
        });
    }

    if extract_html_title(&sanitized).is_none() {
        errors.push("Missing <title> tag.".to_owned());
    }
    if !has_render_surface(&sanitized) {
        errors.push("Missing the required render surface.".to_owned());
    }
    if extract_start_tags(&sanitized, "script").is_empty() {
        errors.push("Missing <script> tag.".to_owned());
    }
    if version.is_none() {
        warnings.push("Missing hypercolor-version metadata.".to_owned());
    }
    if description.is_none() {
        warnings.push("Missing description metadata.".to_owned());
    }
    if author.is_none() {
        warnings.push("Missing publisher metadata.".to_owned());
    }

    InstallPreview {
        author,
        controls,
        description,
        errors,
        file_name: file_name.to_owned(),
        presets,
        title,
        warnings,
    }
}

fn file_stem(file_name: &str) -> String {
    Path::new(file_name)
        .file_stem()
        .and_then(|stem| stem.to_str())
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| file_name.to_owned())
}

fn strip_html_comments(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut cursor = 0usize;

    while let Some(start_rel) = input[cursor..].find("<!--") {
        let start = cursor + start_rel;
        out.push_str(&input[cursor..start]);

        let body_start = start + 4;
        if let Some(end_rel) = input[body_start..].find("-->") {
            cursor = body_start + end_rel + 3;
        } else {
            cursor = input.len();
            break;
        }
    }

    out.push_str(&input[cursor..]);
    out
}

fn extract_start_tags(input: &str, tag_name: &str) -> Vec<String> {
    let mut tags = Vec::new();
    let needle = format!("<{}", tag_name.to_ascii_lowercase());
    let haystack = input.to_ascii_lowercase();
    let mut cursor = 0usize;

    while let Some(start_rel) = haystack[cursor..].find(&needle) {
        let start = cursor + start_rel;
        let tail = &input[start..];
        let Some(end_rel) = tail.find('>') else {
            break;
        };
        tags.push(input[start..start + end_rel + 1].to_owned());
        cursor = start + end_rel + 1;
    }

    tags
}

fn parse_tag_attributes(tag: &str) -> HashMap<String, String> {
    let mut attrs = HashMap::new();
    let trimmed = tag
        .trim()
        .trim_start_matches('<')
        .trim_end_matches('>')
        .trim_end_matches('/')
        .trim();
    let body = trimmed
        .find(char::is_whitespace)
        .map_or("", |index| &trimmed[index..])
        .trim();
    let bytes = body.as_bytes();
    let mut idx = 0usize;

    while idx < bytes.len() {
        while idx < bytes.len() && bytes[idx].is_ascii_whitespace() {
            idx += 1;
        }
        if idx >= bytes.len() {
            break;
        }

        let key_start = idx;
        while idx < bytes.len() {
            let byte = bytes[idx];
            if byte.is_ascii_whitespace() || byte == b'=' || byte == b'/' {
                break;
            }
            idx += 1;
        }
        if idx == key_start {
            idx += 1;
            continue;
        }

        let key = body[key_start..idx].to_ascii_lowercase();
        while idx < bytes.len() && bytes[idx].is_ascii_whitespace() {
            idx += 1;
        }

        let mut value = String::new();
        if idx < bytes.len() && bytes[idx] == b'=' {
            idx += 1;
            while idx < bytes.len() && bytes[idx].is_ascii_whitespace() {
                idx += 1;
            }

            if idx < bytes.len() {
                if matches!(bytes[idx], b'"' | b'\'') {
                    let quote = bytes[idx];
                    idx += 1;
                    let value_start = idx;
                    while idx < bytes.len() && bytes[idx] != quote {
                        idx += 1;
                    }
                    value.push_str(&body[value_start..idx]);
                    if idx < bytes.len() {
                        idx += 1;
                    }
                } else {
                    let value_start = idx;
                    while idx < bytes.len() {
                        let byte = bytes[idx];
                        if byte.is_ascii_whitespace() || byte == b'/' {
                            break;
                        }
                        idx += 1;
                    }
                    value.push_str(&body[value_start..idx]);
                }
            }
        }

        attrs.insert(key, value);
    }

    attrs
}

fn attr_value<'a>(attrs: &'a HashMap<String, String>, key: &str) -> Option<&'a str> {
    attrs
        .get(key)
        .map(String::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

fn has_render_surface(html: &str) -> bool {
    has_tag_with_id(html, "canvas", "excanvas") || has_tag_with_id(html, "div", "facecontainer")
}

fn has_tag_with_id(html: &str, tag_name: &str, expected_id: &str) -> bool {
    extract_start_tags(html, tag_name).into_iter().any(|tag| {
        parse_tag_attributes(&tag)
            .get("id")
            .is_some_and(|value| value.eq_ignore_ascii_case(expected_id))
    })
}

fn extract_html_title(input: &str) -> Option<String> {
    let lower = input.to_ascii_lowercase();
    let start = lower.find("<title")?;
    let open_end = input[start..].find('>')? + start + 1;
    let close_start = lower[open_end..].find("</title>")? + open_end;
    let normalized = normalize_whitespace(&input[open_end..close_start]);
    if normalized.is_empty() {
        None
    } else {
        Some(normalized)
    }
}

fn normalize_whitespace(input: &str) -> String {
    input.split_whitespace().collect::<Vec<_>>().join(" ")
}
