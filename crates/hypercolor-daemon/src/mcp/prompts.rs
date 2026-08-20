//! MCP prompt templates — structured interaction patterns for common workflows.
//!
//! Prompt templates give AI assistants pre-built conversation flows for mood lighting,
//! troubleshooting, and automation setup. Clients surface these as slash commands.

use serde_json::{Value, json};

/// Definition of a single MCP prompt template.
#[derive(Debug, Clone)]
pub struct PromptDefinition {
    /// Prompt name (used as slash command, e.g., `/mood_lighting`).
    pub name: String,
    /// Human-readable title.
    pub title: String,
    /// What this prompt helps with.
    pub description: String,
    /// Arguments the user can provide.
    pub arguments: Vec<PromptArgument>,
}

/// A single argument for a prompt template.
#[derive(Debug, Clone)]
pub struct PromptArgument {
    /// Argument name.
    pub name: String,
    /// Description of what this argument controls.
    pub description: String,
    /// Whether the argument must be provided.
    pub required: bool,
}

/// Build all 3 MCP prompt template definitions.
pub fn build_prompt_definitions() -> Vec<PromptDefinition> {
    vec![
        PromptDefinition {
            name: "mood_lighting".into(),
            title: "Mood Lighting Setup".into(),
            description: "Interactive workflow to configure lighting based on a mood, vibe, or activity. Walks through effect selection, brightness, and color tuning.".into(),
            arguments: vec![
                PromptArgument {
                    name: "mood".into(),
                    description: "Desired mood or vibe (e.g., 'relaxing evening', 'energetic party', 'deep focus coding'). If omitted, the prompt will ask.".into(),
                    required: false,
                },
                PromptArgument {
                    name: "audio_reactive".into(),
                    description: "Whether to include audio-reactive effects in suggestions. Values: 'yes', 'no', 'auto'.".into(),
                    required: false,
                },
            ],
        },
        PromptDefinition {
            name: "troubleshoot".into(),
            title: "Troubleshoot Lighting Issues".into(),
            description: "Guided troubleshooting for device connectivity, rendering, or performance issues. Runs diagnostics and walks through fixes.".into(),
            arguments: vec![
                PromptArgument {
                    name: "issue".into(),
                    description: "Description of the problem (e.g., 'network strip not responding', 'colors look wrong', 'low frame rate')".into(),
                    required: true,
                },
            ],
        },
        PromptDefinition {
            name: "setup_automation".into(),
            title: "Plan Lighting Automation".into(),
            description: "Guided workflow to prepare reusable scenes for an external automation system. Hypercolor does not schedule or trigger scenes itself.".into(),
            arguments: vec![
                PromptArgument {
                    name: "description".into(),
                    description: "Natural language description of the desired automation (e.g., 'dim lights at 10pm', 'warm colors at sunset')".into(),
                    required: false,
                },
            ],
        },
    ]
}

/// Generate the message sequence for a prompt template, substituting arguments.
///
/// Returns `None` if the prompt name is not recognized.
pub fn get_prompt_messages(name: &str, arguments: &Value) -> Option<Value> {
    match name {
        "mood_lighting" => Some(build_mood_lighting_messages(arguments)),
        "troubleshoot" => Some(build_troubleshoot_messages(arguments)),
        "setup_automation" => Some(build_setup_automation_messages(arguments)),
        _ => None,
    }
}

/// Check whether a prompt name is recognized.
pub fn is_valid_prompt(name: &str) -> bool {
    matches!(name, "mood_lighting" | "troubleshoot" | "setup_automation")
}

// ── Prompt Builders ───────────────────────────────────────────────────────

fn build_mood_lighting_messages(arguments: &Value) -> Value {
    let mood = arguments
        .get("mood")
        .and_then(Value::as_str)
        .unwrap_or("a cozy vibe");

    let audio_reactive = arguments
        .get("audio_reactive")
        .and_then(Value::as_str)
        .unwrap_or("auto");
    let audio_guidance = match audio_reactive.to_ascii_lowercase().as_str() {
        "yes" => "Only consider catalog effects marked audio_reactive.",
        "no" => "Exclude catalog effects marked audio_reactive.",
        _ => {
            "Consider audio-reactive catalog effects only when they are the strongest match for the requested mood."
        }
    };

    json!({
        "description": "Configure Hypercolor RGB lighting to match a mood",
        "messages": [
            {
                "role": "user",
                "content": {
                    "type": "text",
                    "text": format!("I want to set up my RGB lighting for this mood: {mood}")
                }
            },
            {
                "role": "assistant",
                "content": {
                    "type": "text",
                    "text": "I'll help you set up the perfect lighting. Let me check what we're working with."
                }
            },
            {
                "role": "assistant",
                "content": {
                    "type": "resource",
                    "resource": {
                        "uri": "hypercolor://state",
                        "mimeType": "application/json"
                    }
                }
            },
            {
                "role": "assistant",
                "content": {
                    "type": "resource",
                    "resource": {
                        "uri": "hypercolor://effects",
                        "mimeType": "application/json"
                    }
                }
            },
            {
                "role": "assistant",
                "content": {
                    "type": "resource",
                    "resource": {
                        "uri": "hypercolor://devices",
                        "mimeType": "application/json"
                    }
                }
            },
            {
                "role": "user",
                "content": {
                    "type": "text",
                    "text": format!("{audio_guidance} Choose one deterministic best match from the catalog for the requested mood and hardware. Call set_effect exactly once. Use the returned zone and layer identities with adjust_controls to tune the applied layer. Explain the selection and final controls without applying alternate candidates.")
                }
            }
        ]
    })
}

fn build_troubleshoot_messages(arguments: &Value) -> Value {
    let issue = arguments
        .get("issue")
        .and_then(Value::as_str)
        .unwrap_or("general issues");

    json!({
        "description": "Troubleshoot Hypercolor device and rendering issues",
        "messages": [
            {
                "role": "user",
                "content": {
                    "type": "text",
                    "text": format!("I'm having an issue with my RGB lighting: {issue}")
                }
            },
            {
                "role": "assistant",
                "content": {
                    "type": "text",
                    "text": "Let me run diagnostics and check the system state."
                }
            },
            {
                "role": "assistant",
                "content": {
                    "type": "resource",
                    "resource": {
                        "uri": "hypercolor://state",
                        "mimeType": "application/json"
                    }
                }
            },
            {
                "role": "assistant",
                "content": {
                    "type": "resource",
                    "resource": {
                        "uri": "hypercolor://devices",
                        "mimeType": "application/json"
                    }
                }
            },
            {
                "role": "user",
                "content": {
                    "type": "text",
                    "text": "Use the diagnose tool to collect the canonical safe diagnostic report. Based on those results and the device/state information above, identify the root cause and provide concrete remediation steps. Use only registered Hypercolor tools for actions they actually support, and state plainly when remediation must happen outside Hypercolor."
                }
            }
        ]
    })
}

fn build_setup_automation_messages(arguments: &Value) -> Value {
    let description = arguments.get("description").and_then(Value::as_str);

    let user_text = match description {
        Some(desc) => format!("I want to set up automated lighting: {desc}."),
        None => "I want to set up automated lighting.".into(),
    };

    json!({
        "description": "Prepare reusable scenes for external automation",
        "messages": [
            {
                "role": "user",
                "content": {
                    "type": "text",
                    "text": user_text
                }
            },
            {
                "role": "assistant",
                "content": {
                    "type": "resource",
                    "resource": {
                        "uri": "hypercolor://scenes",
                        "mimeType": "application/json"
                    }
                }
            },
            {
                "role": "assistant",
                "content": {
                    "type": "resource",
                    "resource": {
                        "uri": "hypercolor://state",
                        "mimeType": "application/json"
                    }
                }
            },
            {
                "role": "assistant",
                "content": {
                    "type": "resource",
                    "resource": {
                        "uri": "hypercolor://effects",
                        "mimeType": "application/json"
                    }
                }
            },
            {
                "role": "user",
                "content": {
                    "type": "text",
                    "text": "Hypercolor does not schedule or trigger scenes. Define the desired reusable state, then create_scene to make an empty named scene when needed. Activate that scene, choose one catalog effect, and call set_effect once. Use the returned zone and layer identities with adjust_controls for final tuning. Creating a scene does not capture the current output. The external scheduler must call activate_scene when its own conditions match."
                }
            }
        ]
    })
}
