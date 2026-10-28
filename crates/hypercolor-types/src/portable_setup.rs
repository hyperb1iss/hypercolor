//! Portable authored lighting state, independent of an installation's paths,
//! credentials, hardware endpoints, and mutation counters.
//!
//! A setup is one physical rig. Its device references are stable setup-local
//! UUIDs; account adapters associate those references with account device slots.
//! Blob bytes travel separately from this manifest and are verified by SHA-256.

mod backup;
mod projection;
mod remap;
mod scene;
mod validation;
pub use backup::{
    PortableAudioPreferences, PortableBackupV1, PortableCapturePreferences, PortablePreferences,
    PortableShutdownPreferences,
};
pub use projection::{SetupBindings, UnmappedSetupReference};
pub use scene::{
    PortableDisplayTarget, PortableLayout, PortableOutput, PortableScene, PortableZone,
};
pub use validation::{PortableReferenceCatalog, PortableSetupError, is_portable_bundle_path};

use crate::attachment::{
    ComponentBinding, ComponentSlot, ComponentTemplate, DeviceComponentProfile,
};
use crate::control::ControlValue;
use crate::effect::EffectId;
use crate::layer::BlendMode;
use crate::library::{EffectPlaylist, EffectPreset};
use crate::portable::PortableDeviceKey;
use crate::scene::DisplayRotation;
use crate::scene::SceneId;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use uuid::Uuid;

/// Versioned manifest shared by manual archives and setup synchronization.
///
/// Collections are keyed by stable identity. Manual merge does not interpret an
/// absent member as deletion; synchronized replacement uses an explicit revision.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PortableSetupV1 {
    pub schema_version: u32,
    pub setup_id: Uuid,
    pub name: String,
    pub devices: BTreeMap<Uuid, PortableDevice>,
    pub logical_devices: BTreeMap<Uuid, PortableLogicalDevice>,
    pub scenes: BTreeMap<Uuid, PortableScene>,
    pub layouts: BTreeMap<String, PortableLayout>,
    pub presets: BTreeMap<Uuid, EffectPreset>,
    pub playlists: BTreeMap<Uuid, EffectPlaylist>,
    pub attachments: BTreeMap<Uuid, PortableAttachments>,
    pub attachment_templates: BTreeMap<String, ComponentTemplate>,
    pub effects: BTreeMap<Uuid, PortableEffect>,
    pub assets: BTreeMap<Uuid, PortableAsset>,
    pub display_faces: BTreeMap<Uuid, PortableDisplayFace>,
    pub device_output: BTreeMap<Uuid, PortableDeviceOutput>,
    pub selection: PortableSelection,
    pub output: PortableOutputState,
}

impl PortableSetupV1 {
    /// Start a new manifest; callers populate it from owning domain snapshots.
    #[must_use]
    pub fn new(setup_id: Uuid, name: String, output: PortableOutputState) -> Self {
        Self {
            schema_version: 1,
            setup_id,
            name,
            devices: BTreeMap::new(),
            logical_devices: BTreeMap::new(),
            scenes: BTreeMap::new(),
            layouts: BTreeMap::new(),
            presets: BTreeMap::new(),
            playlists: BTreeMap::new(),
            attachments: BTreeMap::new(),
            attachment_templates: BTreeMap::new(),
            effects: BTreeMap::new(),
            assets: BTreeMap::new(),
            display_faces: BTreeMap::new(),
            device_output: BTreeMap::new(),
            selection: PortableSelection {
                active_scene_id: None,
                default_scene_id: None,
            },
            output,
        }
    }
}

/// Physical hardware hint without an attachment path or discovery endpoint.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PortableDevice {
    pub name: String,
    pub portable_key: Option<PortableDeviceKey>,
    pub driver_id: String,
    pub vendor_id: Option<u16>,
    pub product_id: Option<u16>,
    pub model: Option<String>,
    pub led_count: u32,
    #[serde(default)]
    pub binding_hint: Option<PortableBindingHint>,
}

/// Conservative device-class evidence shared with complete-sweep reconciliation.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum PortableBindingClass {
    ClaimlessUsb {
        owner: String,
        vendor_id: u16,
        product_id: u16,
    },
    SmBus {
        owner: String,
        protocol_or_model: String,
        address: u16,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PortableSegmentShape {
    pub name: String,
    pub led_count: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PortableBindingHint {
    pub class: PortableBindingClass,
    pub segments: Vec<PortableSegmentShape>,
}

/// An authored segment of a setup device, independent of host discovery IDs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PortableLogicalDevice {
    pub device: Uuid,
    pub name: String,
    pub led_start: u32,
    pub led_count: u32,
    pub enabled: bool,
}

/// Authored attachment assignments; zone suggestions are regenerated locally.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PortableAttachments {
    pub slots: Vec<ComponentSlot>,
    pub bindings: Vec<ComponentBinding>,
}

impl From<&DeviceComponentProfile> for PortableAttachments {
    fn from(profile: &DeviceComponentProfile) -> Self {
        Self {
            slots: profile.slots.clone(),
            bindings: profile.bindings.clone(),
        }
    }
}

impl PortableAttachments {
    /// Reconstruct authored input for the attachment domain's template resolver.
    #[must_use]
    pub fn materialize(&self) -> DeviceComponentProfile {
        DeviceComponentProfile {
            slots: self.slots.clone(),
            bindings: self.bindings.clone(),
            ..Default::default()
        }
    }
}

/// Content-addressed bytes. Paths and URLs are deliberately not part of a blob.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PortableBlob {
    pub sha256: String,
    pub byte_len: u64,
}

/// User media metadata; target-local scanning recreates derived metadata.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PortableAsset {
    pub name: String,
    pub mime_type: String,
    pub tags: Vec<String>,
    pub content: PortableBlob,
}

/// HTML entry point and its local resources, all relative to one effect bundle.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PortableEffect {
    pub name: String,
    pub entrypoint: String,
    pub files: BTreeMap<String, PortableBlob>,
}

/// Selection is applied only after the caller establishes the same physical rig.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PortableSelection {
    pub active_scene_id: Option<SceneId>,
    pub default_scene_id: Option<SceneId>,
}

/// Rig-level output behavior. Capture/audio endpoints and driver settings remain
/// installation-local. Layouts never carry their own canvas dimensions.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PortableOutputState {
    pub canvas_width: u32,
    pub canvas_height: u32,
    pub brightness: f32,
    pub enabled: bool,
    pub paused: bool,
}

/// Default display face used when the authored scene does not target a panel.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PortableDisplayFace {
    pub effect_id: EffectId,
    pub controls: std::collections::HashMap<String, ControlValue>,
    pub blend_mode: BlendMode,
    pub opacity: f32,
}

/// Authored device output behavior, excluding driver controls and account names.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PortableDeviceOutput {
    pub disabled: bool,
    pub brightness: f32,
    pub rotation: DisplayRotation,
}
