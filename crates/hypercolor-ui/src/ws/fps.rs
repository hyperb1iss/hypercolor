//! Reconcile target FPS snapshots with queued transition events.

use serde_json::Value;

#[derive(Debug, Default)]
pub struct FpsTarget {
    events: bool,
    revision: Option<u64>,
}

pub(crate) fn events_supported(message: &Value) -> bool {
    message
        .get("capabilities")
        .and_then(Value::as_array)
        .is_some_and(|capabilities| {
            capabilities
                .iter()
                .any(|capability| capability == "fps_events")
        })
}

impl FpsTarget {
    /// Read the target from the connection's advertised source. A subscribed
    /// snapshot closes the gap after hello; revisions reject older queued events.
    pub fn observe(&mut self, message: &Value) -> Option<u32> {
        match message.get("type")?.as_str()? {
            "hello" => {
                self.events = events_supported(message);
                self.revision = None;
                target(message.get("state")?.get("fps")?.get("target")?)
            }
            "metrics" if !self.events => target(message.get("data")?.get("fps")?.get("target")?),
            "subscribed" if self.events => {
                let fps = message.get("fps")?;
                self.accept(target(fps.get("target")?)?, fps.get("revision")?.as_u64()?)
            }
            "event" if self.events && message.get("event")?.as_str()? == "fps_changed" => {
                let data = message.get("data")?;
                self.accept(
                    target(data.get("new_target")?)?,
                    data.get("revision")?.as_u64()?,
                )
            }
            _ => None,
        }
    }

    fn accept(&mut self, target: u32, revision: u64) -> Option<u32> {
        if self.revision.is_some_and(|seen| revision <= seen) {
            return None;
        }
        self.revision = Some(revision);
        Some(target)
    }
}

fn target(value: &Value) -> Option<u32> {
    u32::try_from(value.as_u64()?)
        .ok()
        .filter(|target| *target > 0)
}
