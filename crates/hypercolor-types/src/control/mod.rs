//! The canonical control-value algebra (Spec 76 §4.5).
//!
//! One value algebra unifies the two control systems that grew up
//! independently: the effect algebra (`crate::effect::ControlValue`,
//! f32/i32, externally tagged) and the driver-surface algebra
//! (`crate::controls::ControlValue`, f64/i64, `kind`/`value` tagged).
//! The canonical type is the **typed union of both** — variant identity
//! is preserved, so every value from either system round-trips through
//! canonical losslessly.
//!
//! The canonical type is deliberately **not serializable**. Wires and
//! persisted files speak the two existing projections; the write-side
//! flip to any canonical encoding is its own reviewed wave under the §0
//! lockstep doctrine. Keeping serde off the canonical type makes "which
//! encoding is this?" unrepresentable.
//!
//! # Per-variant contract
//!
//! | Variant | Effect wire | Driver wire | Widget | Invariant |
//! |---|---|---|---|---|
//! | `Null` | rejected | `null` | — | — |
//! | `Bool` | `boolean` | `bool` | Toggle | — |
//! | `Int` | `integer` (i32, range-checked) | `integer` (i64) | Slider/Stepper | — |
//! | `Float` | `float` (f32, overflow-checked) | `float` (f64) | Slider | finite (never NaN/±inf) |
//! | `Text` | `text` | `string` | TextInput/Asset | — |
//! | `SecretRef` | rejected | `secret_ref` | SecretInput | opaque store reference; the secret never transits |
//! | `Ip` | rejected | `ip_address` | IpInput | parses as `IpAddr`; original text preserved |
//! | `Mac` | rejected | `mac_address` | MacInput | six hex octets in any established encoding (colon/hyphen/bare/dotted); original text preserved |
//! | `Duration` | rejected | `duration_ms` (u64 ms, overflow-checked) | Duration | — |
//! | `ColorRgb` | rejected | `color_rgb` | ColorPicker | encoded sRGB bytes |
//! | `ColorRgba` | rejected | `color_rgba` | ColorPicker | encoded sRGB bytes |
//! | `ColorLinear` | `color` | rejected | ColorPicker | linear-light; components finite |
//! | `Gradient` | `gradient` | rejected | GradientEditor | stop positions and colors finite |
//! | `Rect` | `rect` | rejected | Rect | components finite |
//! | `Enum` | `enum` | `enum` | Dropdown | — |
//! | `Flags` | rejected | `flags` | CheckboxSet | ordered (a `Vec`, not a set) |
//! | `List` | rejected | `list` | — | elements validate recursively |
//! | `Map` | rejected | `object` | — | values validate recursively |
//! | `Unknown` | rejected | `<unrecognized>` | — | unit-level round-trip; payload already dropped by the legacy deserializer |
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

use std::collections::BTreeMap;
use std::net::IpAddr;
use std::time::Duration;

use hypercolor_color::{LinearRgba, Rgb, Rgba};

use crate::controls as driver;
use crate::effect::{self, GradientStop};
use crate::spatial::NormalizedRect;
use crate::viewport::ViewportRect;

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
/// preserved** — projecting back to the driver wire is byte-equal even
/// for non-canonical spellings (`::FFFF:1.2.3.4` stays uppercase).
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

/// MAC address text, validated on construction, original text
/// preserved. Accepts every encoding established in the wild —
/// colon (`aa:bb:cc:dd:ee:ff`), hyphen (`aa-bb-cc-dd-ee-ff`), bare
/// (`aabbccddeeff`, as Govee emits), and dotted (`aabb.ccdd.eeff`) —
/// because persisted driver settings carry all of them and §0 requires
/// old shapes stay readable. Spelling is preserved byte-for-byte.
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
    /// Integer at canonical width. The effect wire narrows to `i32`
    /// via a range-checked projection.
    Int(i64),
    /// Float at canonical width, finite by contract. The effect wire
    /// narrows to `f32` with an overflow check.
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
    /// unit `unknown` variant — the legacy driver deserializer already
    /// discards unrecognized payloads at the wire, so the payload loss
    /// is inherited, not introduced here. The §0 write-side flip must
    /// solve unknown-payload preservation before any canonical
    /// encoding becomes a storage format.
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

/// Why a canonical value cannot project onto the effect wire.
///
/// Narrowing `f64` → `f32` rounds to the nearest representable value
/// by contract — the effect wire IS f32, so rounding is definitional.
/// Only overflow past f32's range is an error.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum EffectProjectionError {
    /// The variant has no effect-wire representation.
    #[error("{0} values have no effect-wire representation")]
    DriverOnly(&'static str),
    /// The integer exceeds the effect wire's i32 range.
    #[error("integer {0} exceeds the effect wire's i32 range")]
    IntOutOfRange(i64),
    /// The float exceeds the effect wire's f32 range.
    #[error("float {0} overflows the effect wire's f32 range")]
    FloatOverflow(f64),
}

/// Why a canonical value cannot project onto the driver wire.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum DriverProjectionError {
    /// The variant has no driver-wire representation.
    #[error("{0} values have no driver-wire representation")]
    EffectOnly(&'static str),
    /// The duration exceeds the driver wire's u64 millisecond range.
    #[error("duration exceeds the driver wire's u64 millisecond range")]
    DurationOverflow,
    /// The duration carries sub-millisecond precision the driver wire
    /// (whole milliseconds) would silently truncate.
    #[error("duration carries sub-millisecond precision the driver wire cannot represent")]
    SubMillisecondDuration,
    /// A nested value failed; `path` locates it (`[3]`, `.host`).
    #[error("{path}: {source}")]
    Nested {
        /// Where in the list/map the failure sits.
        path: String,
        /// The underlying failure.
        source: Box<DriverProjectionError>,
    },
}

impl DriverProjectionError {
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

    /// Project onto the driver wire (`kind`/`value` tagged, f64/i64).
    ///
    /// Every driver-algebra variant projects to its existing `kind` tag
    /// byte-identically; effect-only variants error. Color conversions
    /// between the color variants never happen implicitly — variant
    /// identity is what round-trips.
    pub fn to_driver_wire(&self) -> Result<driver::ControlValue, DriverProjectionError> {
        match self {
            Self::Null => Ok(driver::ControlValue::Null),
            Self::Bool(value) => Ok(driver::ControlValue::Bool(*value)),
            Self::Int(value) => Ok(driver::ControlValue::Integer(*value)),
            Self::Float(value) => Ok(driver::ControlValue::Float(*value)),
            Self::Text(value) => Ok(driver::ControlValue::String(value.clone())),
            Self::SecretRef(reference) => Ok(driver::ControlValue::SecretRef(
                reference.as_str().to_owned(),
            )),
            Self::Ip(ip) => Ok(driver::ControlValue::IpAddress(ip.as_str().to_owned())),
            Self::Mac(mac) => Ok(driver::ControlValue::MacAddress(mac.as_str().to_owned())),
            Self::Duration(duration) => {
                if duration.subsec_nanos() % 1_000_000 != 0 {
                    return Err(DriverProjectionError::SubMillisecondDuration);
                }
                let millis = u64::try_from(duration.as_millis())
                    .map_err(|_| DriverProjectionError::DurationOverflow)?;
                Ok(driver::ControlValue::DurationMs(millis))
            }
            Self::ColorRgb(color) => {
                Ok(driver::ControlValue::ColorRgb([color.r, color.g, color.b]))
            }
            Self::ColorRgba(color) => Ok(driver::ControlValue::ColorRgba([
                color.r, color.g, color.b, color.a,
            ])),
            Self::Enum(value) => Ok(driver::ControlValue::Enum(value.clone())),
            Self::Flags(values) => Ok(driver::ControlValue::Flags(values.clone())),
            Self::List(items) => Ok(driver::ControlValue::List(
                items
                    .iter()
                    .enumerate()
                    .map(|(index, item)| {
                        item.to_driver_wire()
                            .map_err(|e| DriverProjectionError::nested(format!("[{index}]"), e))
                    })
                    .collect::<Result<_, _>>()?,
            )),
            Self::Map(entries) => Ok(driver::ControlValue::Object(
                entries
                    .iter()
                    .map(|(key, value)| {
                        value
                            .to_driver_wire()
                            .map(|projected| (key.clone(), projected))
                            .map_err(|e| DriverProjectionError::nested(format!(".{key}"), e))
                    })
                    .collect::<Result<_, DriverProjectionError>>()?,
            )),
            Self::Unknown => Ok(driver::ControlValue::Unknown),
            Self::ColorLinear(_) | Self::Gradient(_) | Self::Rect(_) => {
                Err(DriverProjectionError::EffectOnly(self.kind_name()))
            }
        }
    }

    /// Project onto the effect wire (externally tagged, f32/i32).
    ///
    /// Fails on width overflow and on every driver-only variant.
    pub fn to_effect_wire(&self) -> Result<effect::ControlValue, EffectProjectionError> {
        match self {
            Self::Bool(value) => Ok(effect::ControlValue::Boolean(*value)),
            Self::Int(value) => i32::try_from(*value)
                .map(effect::ControlValue::Integer)
                .map_err(|_| EffectProjectionError::IntOutOfRange(*value)),
            Self::Float(value) => {
                #[allow(clippy::cast_possible_truncation)]
                let narrowed = *value as f32;
                if narrowed.is_finite() {
                    Ok(effect::ControlValue::Float(narrowed))
                } else {
                    Err(EffectProjectionError::FloatOverflow(*value))
                }
            }
            Self::Text(value) => Ok(effect::ControlValue::Text(value.clone())),
            Self::Enum(value) => Ok(effect::ControlValue::Enum(value.clone())),
            Self::ColorLinear(color) => Ok(effect::ControlValue::Color([
                color.r, color.g, color.b, color.a,
            ])),
            Self::Gradient(stops) => Ok(effect::ControlValue::Gradient(stops.clone())),
            Self::Rect(rect) => Ok(effect::ControlValue::Rect(ViewportRect {
                x: rect.x,
                y: rect.y,
                width: rect.width,
                height: rect.height,
            })),
            Self::Null
            | Self::SecretRef(_)
            | Self::Ip(_)
            | Self::Mac(_)
            | Self::Duration(_)
            | Self::ColorRgb(_)
            | Self::ColorRgba(_)
            | Self::Flags(_)
            | Self::List(_)
            | Self::Map(_)
            | Self::Unknown => Err(EffectProjectionError::DriverOnly(self.kind_name())),
        }
    }
}

impl TryFrom<driver::ControlValue> for ControlValue {
    type Error = ControlValueInvalid;

    /// Canonicalize a driver-wire value, validating the canonical
    /// invariants (finite floats, well-formed ip/mac text).
    fn try_from(value: driver::ControlValue) -> Result<Self, Self::Error> {
        Ok(match value {
            driver::ControlValue::Null => Self::Null,
            driver::ControlValue::Bool(value) => Self::Bool(value),
            driver::ControlValue::Integer(value) => Self::Int(value),
            driver::ControlValue::Float(value) => {
                if !value.is_finite() {
                    return Err(ControlValueInvalid::NonFiniteFloat);
                }
                Self::Float(value)
            }
            driver::ControlValue::String(value) => Self::Text(value),
            driver::ControlValue::SecretRef(reference) => {
                Self::SecretRef(SecretRef::new(reference))
            }
            driver::ControlValue::ColorRgb([r, g, b]) => Self::ColorRgb(Rgb { r, g, b }),
            driver::ControlValue::ColorRgba([r, g, b, a]) => Self::ColorRgba(Rgba { r, g, b, a }),
            driver::ControlValue::IpAddress(text) => Self::Ip(IpText::new(text)?),
            driver::ControlValue::MacAddress(text) => Self::Mac(MacText::new(text)?),
            driver::ControlValue::DurationMs(millis) => {
                Self::Duration(Duration::from_millis(millis))
            }
            driver::ControlValue::Enum(value) => Self::Enum(value),
            driver::ControlValue::Flags(values) => Self::Flags(values),
            driver::ControlValue::List(items) => Self::List(
                items
                    .into_iter()
                    .enumerate()
                    .map(|(index, item)| {
                        Self::try_from(item)
                            .map_err(|e| ControlValueInvalid::nested(format!("[{index}]"), e))
                    })
                    .collect::<Result<_, _>>()?,
            ),
            driver::ControlValue::Object(entries) => Self::Map(
                entries
                    .into_iter()
                    .map(|(key, value)| {
                        Self::try_from(value)
                            .map(|canonical| (key.clone(), canonical))
                            .map_err(|e| ControlValueInvalid::nested(format!(".{key}"), e))
                    })
                    .collect::<Result<_, ControlValueInvalid>>()?,
            ),
            driver::ControlValue::Unknown => Self::Unknown,
        })
    }
}

impl TryFrom<effect::ControlValue> for ControlValue {
    type Error = ControlValueInvalid;

    /// Canonicalize an effect-wire value, validating finiteness.
    fn try_from(value: effect::ControlValue) -> Result<Self, Self::Error> {
        Ok(match value {
            effect::ControlValue::Float(value) => Self::Float(f64::from(finite(value)?)),
            effect::ControlValue::Integer(value) => Self::Int(i64::from(value)),
            effect::ControlValue::Boolean(value) => Self::Bool(value),
            effect::ControlValue::Color([r, g, b, a]) => Self::ColorLinear(LinearRgba::new(
                finite(r)?,
                finite(g)?,
                finite(b)?,
                finite(a)?,
            )),
            effect::ControlValue::Gradient(stops) => {
                for (index, stop) in stops.iter().enumerate() {
                    finite_stop(stop)
                        .map_err(|e| ControlValueInvalid::nested(format!("[{index}]"), e))?;
                }
                Self::Gradient(stops)
            }
            effect::ControlValue::Enum(value) => Self::Enum(value),
            effect::ControlValue::Text(value) => Self::Text(value),
            effect::ControlValue::Rect(rect) => Self::Rect(NormalizedRect {
                x: finite(rect.x)?,
                y: finite(rect.y)?,
                width: finite(rect.width)?,
                height: finite(rect.height)?,
            }),
        })
    }
}
