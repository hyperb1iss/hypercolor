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
                PromptArgument {
                    name: "device_id".into(),
                    description: "Specific device ID if the issue is device-specific".into(),
                    required: false,
                },
            ],
        },
        PromptDefinition {
            name: "setup_automation".into(),
            title: "Set Up Lighting Automation".into(),
            description: "Guided workflow to create automated lighting schedules and scenes. Walks through trigger selection, profile assignment, and transition settings.".into(),
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

    let _audio_reactive = arguments
        .get("audio_reactive")
        .and_then(Value::as_str)
        .unwrap_or("auto");

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
                    "text": "Based on the available effects, connected devices, and current state, suggest an effect and control settings that match the requested mood. Consider the hardware setup and which effects work best with the device count and spatial layout. Provide your top 2-3 recommendations with explanations, then apply the best match after confirming."
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

    let _device_id = arguments.get("device_id").and_then(Value::as_str);

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
                    "text": "Use the diagnose tool to run a full diagnostic. Based on the results and the device/state information above, identify the root cause, explain it clearly, and provide step-by-step instructions to fix the issue. If the fix can be applied through Hypercolor tools (reconnecting a device, adjusting settings), offer to do it."
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
        "description": "Create automated lighting rules and schedules",
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
                        "uri": "hypercolor://profiles",
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
                "role": "user",
                "content": {
                    "type": "text",
                    "text": "Based on the available profiles and current state, help me create an automation rule. Ask about:\n1. When should it trigger? (time of day, solar event, device connection, etc.)\n2. What should happen? (apply a profile, set a specific effect, adjust brightness)\n3. Any conditions? (only on weekdays, only when a device is connected)\n4. Transition style? (instant, slow fade, etc.)\n\nThen use create_scene to create the automation."
                }
            }
        ]
    })
}
