//! The canonical control-value algebra (Spec 76 §4.5).
//!
//! One value algebra unifies effect controls and driver control surfaces.
//! The canonical type is the **typed union of both** and preserves variant
//! identity without transport-specific projections.
//!
//! The canonical JSON wire is internally tagged as
//! `{ "kind": "...", "value": ... }`. Every REST control mutation
//! speaks this encoding.
//!
//! # Per-variant contract
//!
//! | Variant | Widget | Invariant |
//! |---|---|---|
//! | `Null` | — | — |
//! | `Bool` | Toggle | — |
//! | `Int` | Slider/Stepper | — |
//! | `Float` | Slider | finite (never NaN/±inf) |
//! | `Text` | TextInput/Asset | — |
//! | `SecretRef` | SecretInput | opaque store reference; the secret never transits |
//! | `Ip` | IpInput | parses as `IpAddr`; original text preserved |
//! | `Mac` | MacInput | six hex octets in established encodings; original text preserved |
//! | `Duration` | Duration | whole milliseconds |
//! | `ColorRgb` | ColorPicker | encoded sRGB bytes |
//! | `ColorRgba` | ColorPicker | encoded sRGB bytes |
//! | `ColorLinear` | ColorPicker | linear-light; components finite |
//! | `Gradient` | GradientEditor | 2-8 ordered stops; positions and colors normalized and finite |
//! | `Rect` | Rect | components finite |
//! | `Enum` | Dropdown | — |
//! | `Flags` | CheckboxSet | ordered (a `Vec`, not a set) |
//! | `List` | — | elements validate recursively |
//! | `Map` | — | values validate recursively |
//! | `Unknown` | — | explicit unit sentinel; undeclared wire tags are rejected |
//!
//! **Finite-only floats are an invariant, not a loss**: serde_json
//! serializes NaN/Infinity as `null`, so a non-finite float silently
//! degrades on every JSON wire and can never round-trip. Every inbound
//! projection validates finiteness; sensor resolution sanitizes before
//! values enter a control set. Direct construction is possible (the
//! variants are public); anything that admits externally sourced values
//! calls [`ControlValue::validate`]. The enforced admission boundary is
//! wave 6.5b's `ControlSet`: its insertion API validates, so values
//! inside an authoritative set are valid by construction of the SET,
//! not of the enum.
//!
//! # Canonicalizing persisted values
//!
//! The driver wire accepts arbitrary strings where canonical requires
//! validity, so canonicalizing old persisted state CAN fail. A failed
//! canonicalization is a validation error surfaced to the caller,
//! naming the key — never a silent drop, never a crash. Nothing keeps
//! the raw value alive alongside the canonical one: migration is
//! one-time-forward per §0, so a shape that no longer canonicalizes is
//! hand-migrated or refused, not dual-read.

use std::borrow::Borrow;
use std::collections::BTreeMap;
use std::fmt;
use std::net::IpAddr;
use std::time::Duration;

use hypercolor_color::{LinearRgba, Rgb, Rgba};

use crate::effect::GradientStop;
use crate::spatial::NormalizedRect;

mod effect_projection;
mod wire;

pub use effect_projection::{EffectJsonValueError, narrow_effect_f32};
pub use wire::control_value_json_schema;

/// Minimum number of stops accepted by a canonical gradient control.
pub const MIN_GRADIENT_STOPS: usize = 2;
/// Maximum number of stops accepted by a canonical gradient control.
pub const MAX_GRADIENT_STOPS: usize = 8;

/// Stable identifier for a renderer control.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ControlId(String);

impl ControlId {
    /// Create an identifier from its authored name.
    #[must_use]
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// Borrow the authored identifier.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Borrow<str> for ControlId {
    fn borrow(&self) -> &str {
        self.as_str()
    }
}

impl fmt::Display for ControlId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl From<String> for ControlId {
    fn from(value: String) -> Self {
        Self::new(value)
    }
}

impl From<&str> for ControlId {
    fn from(value: &str) -> Self {
        Self::new(value)
    }
}

/// Revision of an authoritative control set.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SetRevision(u64);

impl SetRevision {
    /// Create a revision from the owning scene's monotonic counter.
    #[must_use]
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    /// Return the monotonic counter value.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

impl From<u64> for SetRevision {
    fn from(value: u64) -> Self {
        Self::new(value)
    }
}

impl fmt::Display for SetRevision {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

/// A validated, ordered control snapshot owned by one effect slot.
#[derive(Debug, Clone, PartialEq)]
pub struct ControlSet {
    set_revision: SetRevision,
    values: BTreeMap<ControlId, ControlValue>,
}

impl ControlSet {
    /// Create an empty control set at the supplied revision.
    #[must_use]
    pub const fn new(set_revision: SetRevision) -> Self {
        Self {
            set_revision,
            values: BTreeMap::new(),
        }
    }

    /// Build a set while validating every value at the admission boundary.
    pub fn try_from_entries(
        set_revision: SetRevision,
        entries: impl IntoIterator<Item = (ControlId, ControlValue)>,
    ) -> Result<Self, ControlSetError> {
        let mut controls = Self::new(set_revision);
        for (control_id, value) in entries {
            controls.insert(control_id, value)?;
        }
        Ok(controls)
    }

    /// Return the authoritative revision.
    #[must_use]
    pub const fn set_revision(&self) -> SetRevision {
        self.set_revision
    }

    /// Return a value by authored identifier.
    #[must_use]
    pub fn get(&self, control_id: &str) -> Option<&ControlValue> {
        self.values.get(control_id)
    }

    /// Iterate in stable identifier order.
    pub fn iter(&self) -> impl ExactSizeIterator<Item = (&ControlId, &ControlValue)> {
        self.values.iter()
    }

    /// Return the number of controls in the authoritative snapshot.
    #[must_use]
    pub fn len(&self) -> usize {
        self.values.len()
    }

    /// Return whether the authoritative snapshot contains no controls.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }

    /// Insert a value after validating the canonical invariants.
    pub fn insert(
        &mut self,
        control_id: ControlId,
        value: ControlValue,
    ) -> Result<Option<ControlValue>, ControlSetError> {
        value.validate().map_err(|source| ControlSetError {
            control_id: control_id.clone(),
            source,
        })?;
        Ok(self.values.insert(control_id, value))
    }
}

/// Ordered resolved control changes delivered atomically to one renderer.
#[derive(Debug, Clone, Copy)]
pub struct ControlDeltaBatch<'a> {
    /// Revision of the authoritative authored control set.
    pub set_revision: SetRevision,
    /// Sequence for resolved changes within the same authored revision.
    pub resolution_seq: u64,
    /// Stable, ordered control changes in this delivery.
    pub changes: &'a [(ControlId, ControlValue)],
}

impl<'a> ControlDeltaBatch<'a> {
    /// Create an atomic renderer delivery batch.
    #[must_use]
    pub const fn new(
        set_revision: SetRevision,
        resolution_seq: u64,
        changes: &'a [(ControlId, ControlValue)],
    ) -> Self {
        Self {
            set_revision,
            resolution_seq,
            changes,
        }
    }

    /// Return whether the batch carries no changed values.
    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.changes.is_empty()
    }
}

/// A value rejected while constructing an authoritative control set.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("control '{control_id}': {source}")]
pub struct ControlSetError {
    /// Identifier whose value failed validation.
    pub control_id: ControlId,
    /// Canonical invariant violation.
    #[source]
    pub source: ControlValueInvalid,
}

/// An opaque reference into the credential store. The secret itself
/// never transits — this is the name of a stored secret, distinct from
/// [`ControlValue::Text`] so redaction can key on the variant.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SecretRef(String);

impl SecretRef {
    /// Wrap a store reference. The reference is opaque — no grammar.
    #[must_use]
    pub fn new(reference: impl Into<String>) -> Self {
        Self(reference.into())
    }

    /// The store reference text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// IP address text, validated on construction, **original text
/// preserved** so the canonical wire is byte-equal even for non-canonical
/// spellings (`::FFFF:1.2.3.4` stays uppercase).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct IpText(String);

impl IpText {
    /// Validate and wrap. The text must parse as an [`IpAddr`].
    pub fn new(text: impl Into<String>) -> Result<Self, ControlValueInvalid> {
        let text = text.into();
        if text.parse::<IpAddr>().is_err() {
            return Err(ControlValueInvalid::InvalidIp);
        }
        Ok(Self(text))
    }

    /// The preserved original text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The parsed address.
    #[must_use]
    pub fn addr(&self) -> IpAddr {
        // Validated at construction; reparsing cannot fail.
        self.0
            .parse()
            .expect("IpText holds text validated as an IpAddr")
    }
}

impl From<IpAddr> for IpText {
    fn from(value: IpAddr) -> Self {
        Self(value.to_string())
    }
}

/// MAC address text, validated on construction, original text
/// preserved. Accepts every encoding established in the wild —
/// colon (`aa:bb:cc:dd:ee:ff`), hyphen (`aa-bb-cc-dd-ee-ff`), bare
/// (`aabbccddeeff`, as Govee emits), and dotted (`aabb.ccdd.eeff`) —
/// because drivers and their persisted settings legitimately speak all
/// four today. Spelling is preserved byte-for-byte.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct MacText(String);

impl MacText {
    /// Validate and wrap.
    pub fn new(text: impl Into<String>) -> Result<Self, ControlValueInvalid> {
        let text = text.into();
        if Self::parse(&text).is_none() {
            return Err(ControlValueInvalid::InvalidMac);
        }
        Ok(Self(text))
    }

    /// The preserved original text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The six octets.
    #[must_use]
    pub fn octets(&self) -> [u8; 6] {
        Self::parse(&self.0).expect("MacText holds text validated as a MAC address")
    }

    fn parse(text: &str) -> Option<[u8; 6]> {
        // Length gates below are byte counts; non-ASCII input would
        // slice mid-codepoint in from_hex_groups and can never be hex.
        if !text.is_ascii() {
            return None;
        }
        let bytes = text.as_bytes();
        match bytes.len() {
            12 => Self::from_hex_groups(text, None),
            17 if bytes[2] == b':' => Self::from_hex_groups(text, Some((':', 2))),
            17 if bytes[2] == b'-' => Self::from_hex_groups(text, Some(('-', 2))),
            14 if bytes[4] == b'.' => Self::from_hex_groups(text, Some(('.', 4))),
            _ => None,
        }
    }

    fn from_hex_groups(text: &str, separator: Option<(char, usize)>) -> Option<[u8; 6]> {
        let mut hex = String::with_capacity(12);
        match separator {
            None => hex.push_str(text),
            Some((separator, group_len)) => {
                for part in text.split(separator) {
                    if part.len() != group_len {
                        return None;
                    }
                    hex.push_str(part);
                }
            }
        }
        if hex.len() != 12 {
            return None;
        }
        let mut octets = [0_u8; 6];
        for (index, octet) in octets.iter_mut().enumerate() {
            *octet = u8::from_str_radix(&hex[index * 2..index * 2 + 2], 16).ok()?;
        }
        Some(octets)
    }
}

/// The canonical control value — the typed union of the effect and
/// driver algebras. See the module docs for the per-variant contract.
#[derive(Debug, Clone, PartialEq)]
pub enum ControlValue {
    /// Empty value (driver algebra).
    Null,
    /// Boolean.
    Bool(bool),
    /// Integer at canonical width.
    Int(i64),
    /// Float at canonical width, finite by contract.
    Float(f64),
    /// Free-form text.
    Text(String),
    /// Credential-store reference (driver-only).
    SecretRef(SecretRef),
    /// IP address text (driver-only).
    Ip(IpText),
    /// MAC address text (driver-only).
    Mac(MacText),
    /// Duration (driver wire: whole milliseconds).
    Duration(Duration),
    /// Encoded sRGB color, no alpha (driver-only).
    ColorRgb(Rgb),
    /// Encoded sRGB color with alpha (driver-only).
    ColorRgba(Rgba),
    /// Linear-light color (effect-only).
    ColorLinear(LinearRgba),
    /// Multi-stop gradient (effect-only).
    Gradient(Vec<GradientStop>),
    /// Normalized rectangle (effect-only).
    Rect(NormalizedRect),
    /// Named enum option.
    Enum(String),
    /// Ordered flag set (driver-only).
    Flags(Vec<String>),
    /// Homogeneous list (driver-only).
    List(Vec<ControlValue>),
    /// Structured object (driver-only).
    Map(BTreeMap<String, ControlValue>),
    /// Explicit unknown-value sentinel.
    Unknown,
}

/// Why a value violates the canonical invariants.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ControlValueInvalid {
    /// A float (or a float component of a color, gradient, or rect)
    /// is NaN or infinite.
    #[error("non-finite float values cannot round-trip a JSON wire")]
    NonFiniteFloat,
    /// IP text does not parse as an address.
    #[error("text does not parse as an IP address")]
    InvalidIp,
    /// MAC text does not parse in any established encoding.
    #[error("text does not parse as a MAC address")]
    InvalidMac,
    /// A duration cannot fit the canonical whole-millisecond wire.
    #[error("duration exceeds the control wire's u64 millisecond range")]
    DurationOverflow,
    /// A duration carries precision the canonical wire cannot represent.
    #[error("duration carries sub-millisecond precision the control wire cannot represent")]
    SubMillisecondDuration,
    /// A gradient does not contain the supported number of stops.
    #[error("gradient must contain 2 to 8 stops, got {actual}")]
    GradientStopCount {
        /// Number of supplied stops.
        actual: usize,
    },
    /// A gradient stop position is outside the normalized interval.
    #[error("gradient stop position must be within 0.0..=1.0")]
    GradientPositionOutOfRange,
    /// A gradient color channel is outside the normalized interval.
    #[error("gradient color channel must be within 0.0..=1.0")]
    GradientColorOutOfRange,
    /// Gradient stop positions descend instead of advancing monotonically.
    #[error("gradient stop positions must be in nondecreasing order")]
    GradientPositionsOutOfOrder,
    /// A nested value failed; `path` locates it (`[3]`, `.host`).
    #[error("{path}: {source}")]
    Nested {
        /// Where in the list/map/gradient the failure sits.
        path: String,
        /// The underlying failure.
        source: Box<ControlValueInvalid>,
    },
}

impl ControlValueInvalid {
    fn nested(path: impl Into<String>, source: Self) -> Self {
        Self::Nested {
            path: path.into(),
            source: Box::new(source),
        }
    }
}

fn finite(value: f32) -> Result<f32, ControlValueInvalid> {
    if value.is_finite() {
        Ok(value)
    } else {
        Err(ControlValueInvalid::NonFiniteFloat)
    }
}

fn validate_gradient(stops: &[GradientStop]) -> Result<(), ControlValueInvalid> {
    if !(MIN_GRADIENT_STOPS..=MAX_GRADIENT_STOPS).contains(&stops.len()) {
        return Err(ControlValueInvalid::GradientStopCount {
            actual: stops.len(),
        });
    }

    let mut previous_position = None;
    for (index, stop) in stops.iter().enumerate() {
        finite(stop.position)
            .map_err(|error| ControlValueInvalid::nested(format!("[{index}].position"), error))?;
        if !(0.0..=1.0).contains(&stop.position) {
            return Err(ControlValueInvalid::nested(
                format!("[{index}].position"),
                ControlValueInvalid::GradientPositionOutOfRange,
            ));
        }
        if previous_position.is_some_and(|previous| previous > stop.position) {
            return Err(ControlValueInvalid::nested(
                format!("[{index}].position"),
                ControlValueInvalid::GradientPositionsOutOfOrder,
            ));
        }
        previous_position = Some(stop.position);

        for (channel, value) in stop.color.iter().copied().enumerate() {
            finite(value).map_err(|error| {
                ControlValueInvalid::nested(format!("[{index}].color[{channel}]"), error)
            })?;
            if !(0.0..=1.0).contains(&value) {
                return Err(ControlValueInvalid::nested(
                    format!("[{index}].color[{channel}]"),
                    ControlValueInvalid::GradientColorOutOfRange,
                ));
            }
        }
    }
    Ok(())
}

impl ControlValue {
    /// Validate and build an IP-address value while preserving its spelling.
    pub fn ip(text: impl Into<String>) -> Result<Self, ControlValueInvalid> {
        IpText::new(text).map(Self::Ip)
    }

    /// Validate and build a MAC-address value while preserving its spelling.
    pub fn mac(text: impl Into<String>) -> Result<Self, ControlValueInvalid> {
        MacText::new(text).map(Self::Mac)
    }

    /// Build a canonical linear-light color from RGBA channels.
    #[must_use]
    pub const fn linear_color([r, g, b, a]: [f32; 4]) -> Self {
        Self::ColorLinear(LinearRgba::new(r, g, b, a))
    }

    /// Build a canonical rectangle from an established viewport rectangle.
    #[must_use]
    pub fn rect(value: impl Into<NormalizedRect>) -> Self {
        Self::Rect(value.into())
    }

    /// The variant's name, for error messages and dispatch.
    #[must_use]
    pub const fn kind_name(&self) -> &'static str {
        match self {
            Self::Null => "null",
            Self::Bool(_) => "bool",
            Self::Int(_) => "int",
            Self::Float(_) => "float",
            Self::Text(_) => "text",
            Self::SecretRef(_) => "secret_ref",
            Self::Ip(_) => "ip",
            Self::Mac(_) => "mac",
            Self::Duration(_) => "duration",
            Self::ColorRgb(_) => "color_rgb",
            Self::ColorRgba(_) => "color_rgba",
            Self::ColorLinear(_) => "color_linear",
            Self::Gradient(_) => "gradient",
            Self::Rect(_) => "rect",
            Self::Enum(_) => "enum",
            Self::Flags(_) => "flags",
            Self::List(_) => "list",
            Self::Map(_) => "map",
            Self::Unknown => "unknown",
        }
    }

    /// Return an effect-renderer-compatible scalar when this value is numeric.
    ///
    /// Values outside the effect renderer's `f32` range, and integers that
    /// cannot be represented exactly as `f32`, are refused.
    #[must_use]
    pub fn as_effect_f32(&self) -> Option<f32> {
        match self {
            Self::Float(value) => {
                #[expect(clippy::cast_possible_truncation, clippy::as_conversions)]
                let narrowed = *value as f32;
                narrowed.is_finite().then_some(narrowed)
            }
            Self::Int(value) => i32::try_from(*value).ok().and_then(|value| {
                #[expect(clippy::cast_precision_loss, clippy::as_conversions)]
                let narrowed = value as f32;
                (f64::from(narrowed) == f64::from(value)).then_some(narrowed)
            }),
            _ => None,
        }
    }

    /// Check the canonical invariants (finite floats, recursively).
    ///
    /// The projections validate on the way in; this is the check for
    /// values constructed directly.
    pub fn validate(&self) -> Result<(), ControlValueInvalid> {
        match self {
            Self::Float(value) => {
                if value.is_finite() {
                    Ok(())
                } else {
                    Err(ControlValueInvalid::NonFiniteFloat)
                }
            }
            Self::ColorLinear(color) => {
                for channel in [color.r, color.g, color.b, color.a] {
                    finite(channel)?;
                }
                Ok(())
            }
            Self::Gradient(stops) => validate_gradient(stops),
            Self::Rect(rect) => {
                for component in [rect.x, rect.y, rect.width, rect.height] {
                    finite(component)?;
                }
                Ok(())
            }
            Self::Duration(value) => {
                if value.subsec_nanos() % 1_000_000 != 0 {
                    return Err(ControlValueInvalid::SubMillisecondDuration);
                }
                u64::try_from(value.as_millis())
                    .map(|_| ())
                    .map_err(|_| ControlValueInvalid::DurationOverflow)
            }
            Self::List(items) => {
                for (index, item) in items.iter().enumerate() {
                    item.validate()
                        .map_err(|e| ControlValueInvalid::nested(format!("[{index}]"), e))?;
                }
                Ok(())
            }
            Self::Map(entries) => {
                for (key, value) in entries {
                    value
                        .validate()
                        .map_err(|e| ControlValueInvalid::nested(format!(".{key}"), e))?;
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }
}
