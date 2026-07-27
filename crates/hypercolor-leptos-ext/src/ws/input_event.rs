use serde_json::{Map, Value};
use thiserror::Error;

/// Current schema for JSON payloads on the control-authorized `input_events` channel.
pub const INPUT_EVENT_PAYLOAD_SCHEMA: u8 = 1;

/// Canonical input-event data carried inside a WebSocket `event` message.
#[derive(Debug, Clone, PartialEq)]
pub struct TimedInputEventPayload {
    /// Tagged logical input event, retained as JSON for forward-compatible kinds.
    pub event: Value,
    /// Monotonic capture time in milliseconds.
    pub at_ms: u64,
    /// Daemon-wide delivery sequence.
    pub seq: u64,
    /// Portable producer code when the capture backend exposes one.
    pub physical_code: Option<String>,
    /// Number of equivalent repeat edges represented by this payload.
    pub repeat_count: u32,
    extensions: Map<String, Value>,
}

impl TimedInputEventPayload {
    /// Decode input-event data from the daemon's JSON `event` envelope.
    ///
    /// Timing and metadata fields default to the prior event-only payload,
    /// while unknown fields survive a decode/re-encode round trip.
    ///
    /// # Errors
    ///
    /// Returns [`InputEventPayloadDecodeError`] when the payload is not an
    /// object or a known field has the wrong type or range.
    pub fn decode(data: &Value) -> Result<Self, InputEventPayloadDecodeError> {
        let mut fields = data
            .as_object()
            .cloned()
            .ok_or(InputEventPayloadDecodeError::ExpectedObject)?;
        let event = fields
            .remove("event")
            .filter(Value::is_object)
            .ok_or(InputEventPayloadDecodeError::InvalidField("event"))?;
        let at_ms = take_optional_u64(&mut fields, "at_ms")?.unwrap_or(0);
        let seq = take_optional_u64(&mut fields, "seq")?.unwrap_or(0);
        let physical_code = take_optional_string(&mut fields, "physical_code")?;
        let repeat_count =
            take_optional_u64(&mut fields, "repeat_count")?.map_or(Ok(1), |value| {
                u32::try_from(value)
                    .ok()
                    .filter(|count| *count > 0)
                    .ok_or(InputEventPayloadDecodeError::InvalidField("repeat_count"))
            })?;

        Ok(Self {
            event,
            at_ms,
            seq,
            physical_code,
            repeat_count,
            extensions: fields,
        })
    }

    #[must_use]
    pub fn encode(self) -> Value {
        let mut fields = self.extensions;
        fields.insert("event".to_owned(), self.event);
        fields.insert("at_ms".to_owned(), Value::from(self.at_ms));
        fields.insert("seq".to_owned(), Value::from(self.seq));
        if let Some(physical_code) = self.physical_code {
            fields.insert("physical_code".to_owned(), Value::from(physical_code));
        }
        fields.insert("repeat_count".to_owned(), Value::from(self.repeat_count));
        Value::Object(fields)
    }
}

/// Why a JSON input-event payload could not be decoded.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum InputEventPayloadDecodeError {
    #[error("input event payload must be an object")]
    ExpectedObject,
    #[error("input event payload field '{0}' has an invalid value")]
    InvalidField(&'static str),
}

fn take_optional_u64(
    fields: &mut Map<String, Value>,
    name: &'static str,
) -> Result<Option<u64>, InputEventPayloadDecodeError> {
    match fields.remove(name) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => value
            .as_u64()
            .map(Some)
            .ok_or(InputEventPayloadDecodeError::InvalidField(name)),
    }
}

fn take_optional_string(
    fields: &mut Map<String, Value>,
    name: &'static str,
) -> Result<Option<String>, InputEventPayloadDecodeError> {
    match fields.remove(name) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) => Ok(Some(value)),
        Some(_) => Err(InputEventPayloadDecodeError::InvalidField(name)),
    }
}
