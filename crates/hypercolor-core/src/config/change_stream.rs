//! The persisted-config change stream.
//!
//! Every path that persists the config document (the config API, CLI
//! and MCP writers, capture persistence, migrations, and a reload after
//! an external edit) lands in [`ConfigManager`](super::ConfigManager).
//! After each one the manager diffs the document it last published
//! against the one it just persisted and publishes one
//! [`HypercolorEvent::ConfigChanged`]. Consumers such as a sync intake or
//! a UI hint see one uniform stream instead of one per writer.

use std::collections::BTreeSet;

use hypercolor_types::config::HypercolorConfig;
use hypercolor_types::config_registry;
use hypercolor_types::event::HypercolorEvent;
use serde_json::Value;

/// Derive the one event describing how `previous` became `current`.
///
/// Returns `None` when the documents serialize identically: a rewrite
/// that changes nothing is not a change. One changed leaf names its
/// key and carries its redacted before and after. Several changed
/// leaves collapse to their deepest shared prefix (the empty key for a
/// whole-document change) with a `null` payload, so consumers re-read
/// that subtree instead of trusting a partial diff; a subtree payload
/// would also be the one place a nested credential could slip past
/// per-key redaction.
pub(super) fn config_changed_event(
    previous: &HypercolorConfig,
    current: &HypercolorConfig,
) -> Option<HypercolorEvent> {
    let previous = serde_json::to_value(previous).ok()?;
    let current = serde_json::to_value(current).ok()?;
    let mut changed = Vec::new();
    collect_changed_leaves(&previous, &current, &mut Vec::new(), &mut changed);
    match changed.as_slice() {
        [] => None,
        [key] => Some(HypercolorEvent::ConfigChanged {
            key: key.clone(),
            old_value: lookup(&previous, key)
                .cloned()
                .map(|value| config_registry::redact_value(key, value)),
            new_value: config_registry::redact_value(
                key,
                lookup(&current, key).cloned().unwrap_or(Value::Null),
            ),
        }),
        keys => Some(HypercolorEvent::ConfigChanged {
            key: shared_prefix(keys),
            old_value: None,
            new_value: Value::Null,
        }),
    }
}

/// Collect the dotted paths of every leaf that differs between two
/// documents. Objects recurse; anything else (scalars, arrays, and an
/// object facing a non-object) is one leaf.
fn collect_changed_leaves(
    previous: &Value,
    current: &Value,
    path: &mut Vec<String>,
    changed: &mut Vec<String>,
) {
    match (previous, current) {
        (Value::Object(before), Value::Object(after)) => {
            let keys: BTreeSet<&String> = before.keys().chain(after.keys()).collect();
            for key in keys {
                path.push(key.clone());
                collect_changed_leaves(
                    before.get(key).unwrap_or(&Value::Null),
                    after.get(key).unwrap_or(&Value::Null),
                    path,
                    changed,
                );
                path.pop();
            }
        }
        _ if previous != current => changed.push(path.join(".")),
        _ => {}
    }
}

/// The deepest dotted prefix every key shares; empty when they share none.
fn shared_prefix(keys: &[String]) -> String {
    let Some((first, rest)) = keys.split_first() else {
        return String::new();
    };
    let mut prefix: Vec<&str> = first.split('.').collect();
    for key in rest {
        let shared = prefix
            .iter()
            .zip(key.split('.'))
            .take_while(|(a, b)| **a == *b)
            .count();
        prefix.truncate(shared);
        if prefix.is_empty() {
            break;
        }
    }
    prefix.join(".")
}

fn lookup<'a>(document: &'a Value, key: &str) -> Option<&'a Value> {
    key.split('.')
        .try_fold(document, |node, segment| node.get(segment))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shared_prefix_narrows_to_the_deepest_common_segments() {
        let keys = ["daemon.target_fps".to_owned(), "daemon.port".to_owned()];
        assert_eq!(shared_prefix(&keys), "daemon");
        let keys = ["daemon.target_fps".to_owned(), "audio.device".to_owned()];
        assert_eq!(shared_prefix(&keys), "");
        let keys = ["a.b.c".to_owned(), "a.b.d".to_owned(), "a.b".to_owned()];
        assert_eq!(shared_prefix(&keys), "a.b");
    }

    #[test]
    fn changed_leaves_treat_arrays_and_missing_sections_as_one_leaf() {
        let before = serde_json::json!({ "a": { "list": [1, 2], "x": 1 }, "gone": { "k": 1 } });
        let after = serde_json::json!({ "a": { "list": [1, 3], "x": 1 }, "new": 2 });
        let mut changed = Vec::new();
        collect_changed_leaves(&before, &after, &mut Vec::new(), &mut changed);
        assert_eq!(changed, ["a.list", "gone", "new"]);
    }
}
