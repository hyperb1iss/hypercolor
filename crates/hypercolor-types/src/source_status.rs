//! Bounded platform diagnostics carried through neutral source status.

use serde::de::{self, DeserializeSeed, IgnoredAny, MapAccess, SeqAccess, Unexpected, Visitor};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{Map, Number, Value};
use std::fmt;
use thiserror::Error;
use utoipa::ToSchema;

pub const SOURCE_DIAGNOSTICS_SCHEMA_MAX_BYTES: usize = 64;
pub const SOURCE_DIAGNOSTICS_VERSION_MAX: u16 = 255;
pub const SOURCE_DIAGNOSTICS_PAYLOAD_MAX_BYTES: usize = 16 * 1024;
/// Maximum raw JSON size accepted before decoding an optional envelope.
pub const SOURCE_DIAGNOSTICS_ENVELOPE_MAX_BYTES: usize = 192 * 1024;
pub const SOURCE_DIAGNOSTICS_DISPLAY_FIELD_MAX_COUNT: usize = 24;
pub const SOURCE_DIAGNOSTICS_DISPLAY_KEY_MAX_BYTES: usize = 64;
pub const SOURCE_DIAGNOSTICS_DISPLAY_LABEL_MAX_BYTES: usize = 64;
pub const SOURCE_DIAGNOSTICS_DISPLAY_VALUE_MAX_BYTES: usize = 256;
const PAYLOAD_PREALLOC_MAX_ITEMS: usize = 32;

/// One platform-authored, presentation-safe diagnostic value.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct SourceDiagnosticsDisplayField {
    #[schema(min_length = 1, max_length = 64)]
    pub key: String,
    #[schema(min_length = 1, max_length = 64)]
    pub label: String,
    #[schema(max_length = 256)]
    pub value: String,
}

impl SourceDiagnosticsDisplayField {
    #[must_use]
    pub fn new(key: impl Into<String>, label: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            label: label.into(),
            value: value.into(),
        }
    }
}

#[derive(Clone, Copy)]
struct BoundedStringSeed {
    max_bytes: usize,
    allow_empty: bool,
    error: SourceDiagnosticsEnvelopeError,
}

impl<'de> DeserializeSeed<'de> for BoundedStringSeed {
    type Value = String;

    fn deserialize<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_string(BoundedStringVisitor(self))
    }
}

struct BoundedStringVisitor(BoundedStringSeed);

impl Visitor<'_> for BoundedStringVisitor {
    type Value = String;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "a UTF-8 string no longer than {} bytes",
            self.0.max_bytes
        )
    }

    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        if (!self.0.allow_empty && value.is_empty()) || value.len() > self.0.max_bytes {
            return Err(E::custom(self.0.error));
        }
        Ok(value.to_owned())
    }

    fn visit_string<E>(self, value: String) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        if (!self.0.allow_empty && value.is_empty()) || value.len() > self.0.max_bytes {
            return Err(E::custom(self.0.error));
        }
        Ok(value)
    }
}

#[derive(Clone, Copy)]
enum DisplayFieldKey {
    Key,
    Label,
    Value,
    Unknown,
}

impl<'de> Deserialize<'de> for DisplayFieldKey {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct FieldVisitor;

        impl Visitor<'_> for FieldVisitor {
            type Value = DisplayFieldKey;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a diagnostic display field name")
            }

            fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                Ok(match value {
                    "key" => DisplayFieldKey::Key,
                    "label" => DisplayFieldKey::Label,
                    "value" => DisplayFieldKey::Value,
                    _ => DisplayFieldKey::Unknown,
                })
            }
        }

        deserializer.deserialize_identifier(FieldVisitor)
    }
}

impl<'de> Deserialize<'de> for SourceDiagnosticsDisplayField {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct FieldVisitor;

        impl<'de> Visitor<'de> for FieldVisitor {
            type Value = SourceDiagnosticsDisplayField;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a bounded diagnostic display field")
            }

            fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
            where
                A: MapAccess<'de>,
            {
                let mut key = None;
                let mut label = None;
                let mut value = None;
                while let Some(field) = map.next_key::<DisplayFieldKey>()? {
                    match field {
                        DisplayFieldKey::Key => {
                            if key.is_some() {
                                return Err(de::Error::duplicate_field("key"));
                            }
                            key = Some(map.next_value_seed(BoundedStringSeed {
                                max_bytes: SOURCE_DIAGNOSTICS_DISPLAY_KEY_MAX_BYTES,
                                allow_empty: false,
                                error: SourceDiagnosticsEnvelopeError::InvalidDisplayKey,
                            })?);
                        }
                        DisplayFieldKey::Label => {
                            if label.is_some() {
                                return Err(de::Error::duplicate_field("label"));
                            }
                            label = Some(map.next_value_seed(BoundedStringSeed {
                                max_bytes: SOURCE_DIAGNOSTICS_DISPLAY_LABEL_MAX_BYTES,
                                allow_empty: false,
                                error: SourceDiagnosticsEnvelopeError::InvalidDisplayLabel,
                            })?);
                        }
                        DisplayFieldKey::Value => {
                            if value.is_some() {
                                return Err(de::Error::duplicate_field("value"));
                            }
                            value = Some(map.next_value_seed(BoundedStringSeed {
                                max_bytes: SOURCE_DIAGNOSTICS_DISPLAY_VALUE_MAX_BYTES,
                                allow_empty: true,
                                error: SourceDiagnosticsEnvelopeError::DisplayValueTooLarge,
                            })?);
                        }
                        DisplayFieldKey::Unknown => {
                            map.next_value::<IgnoredAny>()?;
                        }
                    }
                }
                Ok(SourceDiagnosticsDisplayField {
                    key: key.ok_or_else(|| de::Error::missing_field("key"))?,
                    label: label.ok_or_else(|| de::Error::missing_field("label"))?,
                    value: value.ok_or_else(|| de::Error::missing_field("value"))?,
                })
            }
        }

        deserializer.deserialize_struct(
            "SourceDiagnosticsDisplayField",
            &["key", "label", "value"],
            FieldVisitor,
        )
    }
}

struct DisplayFieldsSeed;

impl<'de> DeserializeSeed<'de> for DisplayFieldsSeed {
    type Value = Vec<SourceDiagnosticsDisplayField>;

    fn deserialize<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct FieldsVisitor;

        impl<'de> Visitor<'de> for FieldsVisitor {
            type Value = Vec<SourceDiagnosticsDisplayField>;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(
                    formatter,
                    "at most {SOURCE_DIAGNOSTICS_DISPLAY_FIELD_MAX_COUNT} display fields"
                )
            }

            fn visit_seq<A>(self, mut sequence: A) -> Result<Self::Value, A::Error>
            where
                A: SeqAccess<'de>,
            {
                let capacity = sequence
                    .size_hint()
                    .unwrap_or(0)
                    .min(SOURCE_DIAGNOSTICS_DISPLAY_FIELD_MAX_COUNT);
                let mut fields = Vec::with_capacity(capacity);
                while fields.len() < SOURCE_DIAGNOSTICS_DISPLAY_FIELD_MAX_COUNT {
                    let Some(field) = sequence.next_element()? else {
                        return Ok(fields);
                    };
                    fields.push(field);
                }
                if sequence.next_element::<IgnoredAny>()?.is_some() {
                    return Err(de::Error::custom(
                        SourceDiagnosticsEnvelopeError::TooManyDisplayFields,
                    ));
                }
                Ok(fields)
            }
        }

        deserializer.deserialize_seq(FieldsVisitor)
    }
}

/// A versioned payload whose semantics remain owned by its platform crate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct SourceDiagnosticsEnvelope {
    #[schema(min_length = 1, max_length = 64)]
    schema: String,
    #[schema(minimum = 1, maximum = 255)]
    version: u16,
    #[schema(max_items = 24)]
    display: Vec<SourceDiagnosticsDisplayField>,
    /// Opaque platform JSON bounded to 16384 serialized UTF-8 bytes.
    payload: Value,
}

impl SourceDiagnosticsEnvelope {
    /// Build an envelope after enforcing every transport and display bound.
    pub fn try_new(
        schema: impl Into<String>,
        version: u16,
        display: Vec<SourceDiagnosticsDisplayField>,
        payload: Value,
    ) -> Result<Self, SourceDiagnosticsEnvelopeError> {
        let envelope = Self {
            schema: schema.into(),
            version,
            display,
            payload,
        };
        envelope.validate()?;
        Ok(envelope)
    }

    /// Parse an opaque JSON payload without letting malformed data enter status.
    pub fn try_from_json(
        schema: impl Into<String>,
        version: u16,
        display: Vec<SourceDiagnosticsDisplayField>,
        payload: &str,
    ) -> Result<Self, SourceDiagnosticsEnvelopeError> {
        if payload.len() > SOURCE_DIAGNOSTICS_PAYLOAD_MAX_BYTES {
            return Err(SourceDiagnosticsEnvelopeError::PayloadTooLarge);
        }
        let payload = serde_json::from_str(payload)
            .map_err(|_| SourceDiagnosticsEnvelopeError::MalformedPayload)?;
        Self::try_new(schema, version, display, payload)
    }

    #[must_use]
    pub fn schema(&self) -> &str {
        &self.schema
    }

    #[must_use]
    pub const fn version(&self) -> u16 {
        self.version
    }

    #[must_use]
    pub fn display(&self) -> &[SourceDiagnosticsDisplayField] {
        &self.display
    }

    #[must_use]
    pub const fn payload(&self) -> &Value {
        &self.payload
    }

    fn validate(&self) -> Result<(), SourceDiagnosticsEnvelopeError> {
        if self.schema.is_empty() || self.schema.len() > SOURCE_DIAGNOSTICS_SCHEMA_MAX_BYTES {
            return Err(SourceDiagnosticsEnvelopeError::InvalidSchema);
        }
        if self.version == 0 || self.version > SOURCE_DIAGNOSTICS_VERSION_MAX {
            return Err(SourceDiagnosticsEnvelopeError::InvalidVersion);
        }
        if self.display.len() > SOURCE_DIAGNOSTICS_DISPLAY_FIELD_MAX_COUNT {
            return Err(SourceDiagnosticsEnvelopeError::TooManyDisplayFields);
        }
        for field in &self.display {
            if field.key.is_empty() || field.key.len() > SOURCE_DIAGNOSTICS_DISPLAY_KEY_MAX_BYTES {
                return Err(SourceDiagnosticsEnvelopeError::InvalidDisplayKey);
            }
            if field.label.is_empty()
                || field.label.len() > SOURCE_DIAGNOSTICS_DISPLAY_LABEL_MAX_BYTES
            {
                return Err(SourceDiagnosticsEnvelopeError::InvalidDisplayLabel);
            }
            if field.value.len() > SOURCE_DIAGNOSTICS_DISPLAY_VALUE_MAX_BYTES {
                return Err(SourceDiagnosticsEnvelopeError::DisplayValueTooLarge);
            }
        }
        let payload_size = serde_json::to_vec(&self.payload)
            .map_err(|_| SourceDiagnosticsEnvelopeError::MalformedPayload)?
            .len();
        if payload_size > SOURCE_DIAGNOSTICS_PAYLOAD_MAX_BYTES {
            return Err(SourceDiagnosticsEnvelopeError::PayloadTooLarge);
        }
        Ok(())
    }
}

struct PayloadBudget {
    remaining: usize,
}

impl PayloadBudget {
    fn spend<E>(&mut self, bytes: usize) -> Result<(), E>
    where
        E: de::Error,
    {
        self.remaining = self
            .remaining
            .checked_sub(bytes)
            .ok_or_else(|| E::custom(SourceDiagnosticsEnvelopeError::PayloadTooLarge))?;
        Ok(())
    }
}

struct BoundedPayloadSeed<'a>(&'a mut PayloadBudget);

impl<'de> DeserializeSeed<'de> for BoundedPayloadSeed<'_> {
    type Value = Value;

    fn deserialize<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_any(BoundedPayloadVisitor(self.0))
    }
}

struct BoundedPayloadVisitor<'a>(&'a mut PayloadBudget);

impl<'de> Visitor<'de> for BoundedPayloadVisitor<'_> {
    type Value = Value;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "JSON bounded to {SOURCE_DIAGNOSTICS_PAYLOAD_MAX_BYTES} serialized bytes"
        )
    }

    fn visit_bool<E>(self, value: bool) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        self.0.spend(if value { 4 } else { 5 })?;
        Ok(Value::Bool(value))
    }

    fn visit_i64<E>(self, value: i64) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        let number = Number::from(value);
        self.0.spend(number.to_string().len())?;
        Ok(Value::Number(number))
    }

    fn visit_i128<E>(self, value: i128) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        let number = Number::from_i128(value)
            .ok_or_else(|| E::invalid_value(Unexpected::Other("i128"), &self))?;
        self.0.spend(number.to_string().len())?;
        Ok(Value::Number(number))
    }

    fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        let number = Number::from(value);
        self.0.spend(number.to_string().len())?;
        Ok(Value::Number(number))
    }

    fn visit_u128<E>(self, value: u128) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        let number = Number::from_u128(value)
            .ok_or_else(|| E::invalid_value(Unexpected::Other("u128"), &self))?;
        self.0.spend(number.to_string().len())?;
        Ok(Value::Number(number))
    }

    fn visit_f64<E>(self, value: f64) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        let number = Number::from_f64(value)
            .ok_or_else(|| E::invalid_value(Unexpected::Float(value), &self))?;
        self.0.spend(number.to_string().len())?;
        Ok(Value::Number(number))
    }

    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        self.0.spend(value.len().saturating_add(2))?;
        Ok(Value::String(value.to_owned()))
    }

    fn visit_string<E>(self, value: String) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        self.0.spend(value.len().saturating_add(2))?;
        Ok(Value::String(value))
    }

    fn visit_none<E>(self) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        self.visit_unit()
    }

    fn visit_some<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: Deserializer<'de>,
    {
        BoundedPayloadSeed(self.0).deserialize(deserializer)
    }

    fn visit_unit<E>(self) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        self.0.spend(4)?;
        Ok(Value::Null)
    }

    fn visit_newtype_struct<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: Deserializer<'de>,
    {
        BoundedPayloadSeed(self.0).deserialize(deserializer)
    }

    fn visit_seq<A>(self, mut sequence: A) -> Result<Self::Value, A::Error>
    where
        A: SeqAccess<'de>,
    {
        self.0.spend(2)?;
        let capacity = sequence
            .size_hint()
            .unwrap_or(0)
            .min(PAYLOAD_PREALLOC_MAX_ITEMS);
        let mut values = Vec::with_capacity(capacity);
        while let Some(value) = sequence.next_element_seed(BoundedPayloadSeed(self.0))? {
            if !values.is_empty() {
                self.0.spend(1)?;
            }
            values.push(value);
        }
        Ok(Value::Array(values))
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        self.0.spend(2)?;
        let mut values =
            Map::with_capacity(map.size_hint().unwrap_or(0).min(PAYLOAD_PREALLOC_MAX_ITEMS));
        while let Some(key) = map.next_key_seed(BoundedPayloadKeySeed(self.0))? {
            self.0.spend(if values.is_empty() { 1 } else { 2 })?;
            let value = map.next_value_seed(BoundedPayloadSeed(self.0))?;
            values.insert(key, value);
        }
        Ok(Value::Object(values))
    }
}

struct BoundedPayloadKeySeed<'a>(&'a mut PayloadBudget);

impl<'de> DeserializeSeed<'de> for BoundedPayloadKeySeed<'_> {
    type Value = String;

    fn deserialize<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_string(BoundedPayloadKeyVisitor(self.0))
    }
}

struct BoundedPayloadKeyVisitor<'a>(&'a mut PayloadBudget);

impl Visitor<'_> for BoundedPayloadKeyVisitor<'_> {
    type Value = String;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a payload object key within the remaining byte budget")
    }

    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        self.0.spend(value.len().saturating_add(2))?;
        Ok(value.to_owned())
    }

    fn visit_string<E>(self, value: String) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        self.0.spend(value.len().saturating_add(2))?;
        Ok(value)
    }
}

#[derive(Clone, Copy)]
enum EnvelopeField {
    Schema,
    Version,
    Display,
    Payload,
    Unknown,
}

impl<'de> Deserialize<'de> for EnvelopeField {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct FieldVisitor;

        impl Visitor<'_> for FieldVisitor {
            type Value = EnvelopeField;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a source diagnostics envelope field name")
            }

            fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                Ok(match value {
                    "schema" => EnvelopeField::Schema,
                    "version" => EnvelopeField::Version,
                    "display" => EnvelopeField::Display,
                    "payload" => EnvelopeField::Payload,
                    _ => EnvelopeField::Unknown,
                })
            }
        }

        deserializer.deserialize_identifier(FieldVisitor)
    }
}

impl<'de> Deserialize<'de> for SourceDiagnosticsEnvelope {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct EnvelopeVisitor;

        impl<'de> Visitor<'de> for EnvelopeVisitor {
            type Value = SourceDiagnosticsEnvelope;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a bounded source diagnostics envelope")
            }

            fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
            where
                A: MapAccess<'de>,
            {
                let mut schema = None;
                let mut version = None;
                let mut display = None;
                let mut payload = None;
                while let Some(field) = map.next_key::<EnvelopeField>()? {
                    match field {
                        EnvelopeField::Schema => {
                            if schema.is_some() {
                                return Err(de::Error::duplicate_field("schema"));
                            }
                            schema = Some(map.next_value_seed(BoundedStringSeed {
                                max_bytes: SOURCE_DIAGNOSTICS_SCHEMA_MAX_BYTES,
                                allow_empty: false,
                                error: SourceDiagnosticsEnvelopeError::InvalidSchema,
                            })?);
                        }
                        EnvelopeField::Version => {
                            if version.is_some() {
                                return Err(de::Error::duplicate_field("version"));
                            }
                            version = Some(map.next_value::<u16>()?);
                        }
                        EnvelopeField::Display => {
                            if display.is_some() {
                                return Err(de::Error::duplicate_field("display"));
                            }
                            display = Some(map.next_value_seed(DisplayFieldsSeed)?);
                        }
                        EnvelopeField::Payload => {
                            if payload.is_some() {
                                return Err(de::Error::duplicate_field("payload"));
                            }
                            let mut budget = PayloadBudget {
                                remaining: SOURCE_DIAGNOSTICS_PAYLOAD_MAX_BYTES,
                            };
                            payload = Some(map.next_value_seed(BoundedPayloadSeed(&mut budget))?);
                        }
                        EnvelopeField::Unknown => {
                            map.next_value::<IgnoredAny>()?;
                        }
                    }
                }
                SourceDiagnosticsEnvelope::try_new(
                    schema.ok_or_else(|| de::Error::missing_field("schema"))?,
                    version.ok_or_else(|| de::Error::missing_field("version"))?,
                    display.unwrap_or_default(),
                    payload.ok_or_else(|| de::Error::missing_field("payload"))?,
                )
                .map_err(de::Error::custom)
            }
        }

        deserializer.deserialize_struct(
            "SourceDiagnosticsEnvelope",
            &["schema", "version", "display", "payload"],
            EnvelopeVisitor,
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum SourceDiagnosticsEnvelopeError {
    #[error("diagnostic schema is empty or exceeds its bound")]
    InvalidSchema,
    #[error("diagnostic version is zero or exceeds its bound")]
    InvalidVersion,
    #[error("diagnostic payload is malformed")]
    MalformedPayload,
    #[error("diagnostic payload exceeds its byte bound")]
    PayloadTooLarge,
    #[error("diagnostic display field count exceeds its bound")]
    TooManyDisplayFields,
    #[error("diagnostic display key is empty or exceeds its bound")]
    InvalidDisplayKey,
    #[error("diagnostic display label is empty or exceeds its bound")]
    InvalidDisplayLabel,
    #[error("diagnostic display value exceeds its byte bound")]
    DisplayValueTooLarge,
}
