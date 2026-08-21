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
//! | `Gradient` | GradientEditor | stop positions and colors finite |
//! | `Rect` | Rect | components finite |
//! | `Enum` | Dropdown | — |
//! | `Flags` | CheckboxSet | ordered (a `Vec`, not a set) |
//! | `List` | — | elements validate recursively |
//! | `Map` | — | values validate recursively |
//! | `Unknown` | — | unit-level forward-compatibility marker |
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
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use utoipa::{PartialSchema, ToSchema};

use crate::effect::GradientStop;
use crate::spatial::NormalizedRect;

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
    /// Value whose `kind` a newer schema minted. Round-trips as the
    /// unit `unknown` variant, matching the established driver value
    /// semantics.
    Unknown,
}

#[derive(Serialize)]
#[serde(rename_all = "snake_case", tag = "kind", content = "value")]
enum ControlValueRef<'a> {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    Text(&'a str),
    SecretRef(&'a str),
    Ip(&'a str),
    Mac(&'a str),
    Duration(u64),
    ColorRgb(Rgb),
    ColorRgba(Rgba),
    ColorLinear(LinearRgba),
    Gradient(&'a [GradientStop]),
    Rect(NormalizedRect),
    Enum(&'a str),
    Flags(&'a [String]),
    List(&'a [ControlValue]),
    Map(&'a BTreeMap<String, ControlValue>),
    Unknown,
}

#[derive(Deserialize, ToSchema)]
#[allow(dead_code)]
#[schema(no_recursion)]
#[serde(rename_all = "snake_case", tag = "kind", content = "value")]
enum ControlValueWire {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    Text(String),
    SecretRef(String),
    Ip(String),
    Mac(String),
    Duration(u64),
    ColorRgb(Rgb),
    ColorRgba(Rgba),
    ColorLinear(LinearRgba),
    Gradient(Vec<GradientStop>),
    Rect(NormalizedRect),
    Enum(String),
    Flags(Vec<String>),
    List(Vec<ControlValue>),
    Map(BTreeMap<String, ControlValue>),
    Unknown,
}

impl utoipa::__dev::ComposeSchema for ControlValue {
    fn compose(
        _new_generics: Vec<utoipa::openapi::RefOr<utoipa::openapi::schema::Schema>>,
    ) -> utoipa::openapi::RefOr<utoipa::openapi::schema::Schema> {
        ControlValueWire::schema()
    }
}

impl ToSchema for ControlValue {
    fn name() -> std::borrow::Cow<'static, str> {
        std::borrow::Cow::Borrowed("ControlValue")
    }

    fn schemas(
        schemas: &mut Vec<(
            String,
            utoipa::openapi::RefOr<utoipa::openapi::schema::Schema>,
        )>,
    ) {
        ControlValueWire::schemas(schemas);
        for (name, schema) in [
            (Rgb::name().into_owned(), Rgb::schema()),
            (Rgba::name().into_owned(), Rgba::schema()),
            (LinearRgba::name().into_owned(), LinearRgba::schema()),
            (GradientStop::name().into_owned(), GradientStop::schema()),
            (
                NormalizedRect::name().into_owned(),
                NormalizedRect::schema(),
            ),
        ] {
            schemas.push((name, schema));
        }
    }
}

impl Serialize for ControlValue {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.validate().map_err(serde::ser::Error::custom)?;
        let wire = match self {
            Self::Null => ControlValueRef::Null,
            Self::Bool(value) => ControlValueRef::Bool(*value),
            Self::Int(value) => ControlValueRef::Int(*value),
            Self::Float(value) => ControlValueRef::Float(*value),
            Self::Text(value) => ControlValueRef::Text(value),
            Self::SecretRef(value) => ControlValueRef::SecretRef(value.as_str()),
            Self::Ip(value) => ControlValueRef::Ip(value.as_str()),
            Self::Mac(value) => ControlValueRef::Mac(value.as_str()),
            Self::Duration(value) => {
                if value.subsec_nanos() % 1_000_000 != 0 {
                    return Err(serde::ser::Error::custom(
                        ControlValueInvalid::SubMillisecondDuration,
                    ));
                }
                let millis = u64::try_from(value.as_millis()).map_err(|_| {
                    serde::ser::Error::custom(ControlValueInvalid::DurationOverflow)
                })?;
                ControlValueRef::Duration(millis)
            }
            Self::ColorRgb(value) => ControlValueRef::ColorRgb(*value),
            Self::ColorRgba(value) => ControlValueRef::ColorRgba(*value),
            Self::ColorLinear(value) => ControlValueRef::ColorLinear(*value),
            Self::Gradient(value) => ControlValueRef::Gradient(value),
            Self::Rect(value) => ControlValueRef::Rect(*value),
            Self::Enum(value) => ControlValueRef::Enum(value),
            Self::Flags(value) => ControlValueRef::Flags(value),
            Self::List(value) => ControlValueRef::List(value),
            Self::Map(value) => ControlValueRef::Map(value),
            Self::Unknown => ControlValueRef::Unknown,
        };
        wire.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for ControlValue {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct RawControlValue {
            kind: String,
            #[serde(default)]
            value: Option<serde_json::Value>,
        }

        fn parse_value<T, E>(kind: &str, value: Option<serde_json::Value>) -> Result<T, E>
        where
            T: serde::de::DeserializeOwned,
            E: serde::de::Error,
        {
            let value = value.ok_or_else(|| E::custom(format!("missing value for {kind}")))?;
            serde_json::from_value(value)
                .map_err(|error| E::custom(format!("invalid {kind} value: {error}")))
        }

        let raw = RawControlValue::deserialize(deserializer)?;
        let value = match raw.kind.as_str() {
            "null" => Self::Null,
            "bool" => Self::Bool(parse_value("bool", raw.value)?),
            "int" => Self::Int(parse_value("int", raw.value)?),
            "float" => Self::Float(parse_value("float", raw.value)?),
            "text" => Self::Text(parse_value("text", raw.value)?),
            "secret_ref" => Self::SecretRef(SecretRef::new(parse_value::<String, D::Error>(
                "secret_ref",
                raw.value,
            )?)),
            "ip" => Self::Ip(
                IpText::new(parse_value::<String, D::Error>("ip", raw.value)?)
                    .map_err(serde::de::Error::custom)?,
            ),
            "mac" => Self::Mac(
                MacText::new(parse_value::<String, D::Error>("mac", raw.value)?)
                    .map_err(serde::de::Error::custom)?,
            ),
            "duration" => {
                Self::Duration(Duration::from_millis(parse_value("duration", raw.value)?))
            }
            "color_rgb" => Self::ColorRgb(parse_value("color_rgb", raw.value)?),
            "color_rgba" => Self::ColorRgba(parse_value("color_rgba", raw.value)?),
            "color_linear" => Self::ColorLinear(parse_value("color_linear", raw.value)?),
            "gradient" => Self::Gradient(parse_value("gradient", raw.value)?),
            "rect" => Self::Rect(parse_value("rect", raw.value)?),
            "enum" => Self::Enum(parse_value("enum", raw.value)?),
            "flags" => Self::Flags(parse_value("flags", raw.value)?),
            "list" => Self::List(parse_value("list", raw.value)?),
            "map" => Self::Map(parse_value("map", raw.value)?),
            _ => Self::Unknown,
        };
        value.validate().map_err(serde::de::Error::custom)?;
        Ok(value)
    }
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

fn finite_stop(stop: &GradientStop) -> Result<(), ControlValueInvalid> {
    finite(stop.position)?;
    for channel in stop.color {
        finite(channel)?;
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
    /// Values outside the effect renderer's `f32`/`i32` range are refused
    /// instead of narrowing to infinity or silently losing integer width.
    #[must_use]
    pub fn as_effect_f32(&self) -> Option<f32> {
        match self {
            Self::Float(value) => {
                #[expect(clippy::cast_possible_truncation, clippy::as_conversions)]
                let narrowed = *value as f32;
                narrowed.is_finite().then_some(narrowed)
            }
            Self::Int(value) => i32::try_from(*value).ok().map(|value| {
                #[expect(clippy::cast_precision_loss, clippy::as_conversions)]
                let narrowed = value as f32;
                narrowed
            }),
            Self::Bool(value) => Some(if *value { 1.0 } else { 0.0 }),
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
            Self::Gradient(stops) => {
                for (index, stop) in stops.iter().enumerate() {
                    finite_stop(stop)
                        .map_err(|e| ControlValueInvalid::nested(format!("[{index}]"), e))?;
                }
                Ok(())
            }
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
