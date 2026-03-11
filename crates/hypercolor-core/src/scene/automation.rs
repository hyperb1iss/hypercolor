//! Automation rule engine — event-driven reactive scene control.
//!
//! The [`AutomationEngine`] stores automation rules, evaluates incoming
//! triggers against them, and returns the actions that should fire.
//! Cooldown tracking prevents rapid re-firing of the same rule.

use std::collections::HashMap;
use std::time::Instant;

use crate::types::scene::{ActionKind, AutomationRule, TriggerSource};

// ── RuleId ──────────────────────────────────────────────────────────────

/// Lightweight string identifier for automation rules within the engine.
///
/// Rules are keyed by their `name` field for simplicity. In production,
/// this would be a UUID — but the types crate uses string names, so
/// we follow that convention.
type RuleId = String;

// ── AutomationEngine ────────────────────────────────────────────────────

/// Stores and evaluates automation rules against incoming triggers.
///
/// The engine is synchronous and designed to be called from the main
/// event loop. It does not own a runtime or spawn tasks — the caller
/// is responsible for dispatching the returned actions.
#[derive(Debug)]
pub struct AutomationEngine {
    /// Registered rules, keyed by name.
    rules: HashMap<RuleId, AutomationRule>,

    /// Per-rule last-fired timestamp for cooldown enforcement.
    last_fired: HashMap<RuleId, Instant>,
}

impl AutomationEngine {
    /// Create a new empty automation engine.
    #[must_use]
    pub fn new() -> Self {
        Self {
            rules: HashMap::new(),
            last_fired: HashMap::new(),
        }
    }

    // ── Rule CRUD ────────────────────────────────────────────────────

    /// Add a rule to the engine.
    ///
    /// If a rule with the same name already exists, it is replaced.
    pub fn add_rule(&mut self, rule: AutomationRule) {
        self.rules.insert(rule.name.clone(), rule);
    }

    /// Remove a rule by name.
    ///
    /// Returns the removed rule if it existed.
    pub fn remove_rule(&mut self, name: &str) -> Option<AutomationRule> {
        self.last_fired.remove(name);
        self.rules.remove(name)
    }

    /// Enable a rule by name.
    ///
    /// Returns `true` if the rule was found and enabled.
    pub fn enable_rule(&mut self, name: &str) -> bool {
        if let Some(rule) = self.rules.get_mut(name) {
            rule.enabled = true;
            return true;
        }
        false
    }

    /// Disable a rule by name.
    ///
    /// Returns `true` if the rule was found and disabled.
    pub fn disable_rule(&mut self, name: &str) -> bool {
        if let Some(rule) = self.rules.get_mut(name) {
            rule.enabled = false;
            return true;
        }
        false
    }

    /// Get a reference to a rule by name.
    #[must_use]
    pub fn get_rule(&self, name: &str) -> Option<&AutomationRule> {
        self.rules.get(name)
    }

    /// List all registered rules.
    #[must_use]
    pub fn rules(&self) -> Vec<&AutomationRule> {
        self.rules.values().collect()
    }

    /// Number of registered rules.
    #[must_use]
    pub fn rule_count(&self) -> usize {
        self.rules.len()
    }

    // ── Evaluation ──────────────────────────────────────────────────

    /// Evaluate all rules against an incoming trigger.
    ///
    /// Returns a list of `(rule_name, action)` pairs for rules that:
    /// 1. Are enabled.
    /// 2. Match the given trigger.
    /// 3. Pass condition evaluation.
    /// 4. Are not within their cooldown window.
    ///
    /// Matching rules have their last-fired timestamp updated.
    pub fn evaluate(&mut self, trigger: &TriggerSource) -> Vec<(String, ActionKind)> {
        let now = Instant::now();
        let mut fired = Vec::new();

        // Collect matching rule names + actions first to avoid borrow issues.
        let candidates: Vec<(String, ActionKind, u64)> = self
            .rules
            .values()
            .filter(|rule| rule.enabled)
            .filter(|rule| trigger_matches(&rule.trigger, trigger))
            .filter(|rule| evaluate_conditions(&rule.conditions))
            .map(|rule| (rule.name.clone(), rule.action.clone(), rule.cooldown_secs))
            .collect();

        for (name, action, cooldown_secs) in candidates {
            // Cooldown check.
            if cooldown_secs > 0
                && let Some(last) = self.last_fired.get(&name)
            {
                let elapsed = now.duration_since(*last);
                if elapsed.as_secs() < cooldown_secs {
                    continue;
                }
            }

            self.last_fired.insert(name.clone(), now);
            fired.push((name, action));
        }

        fired
    }

    /// Reset the cooldown timer for a specific rule.
    ///
    /// Useful for testing or manual override.
    pub fn reset_cooldown(&mut self, name: &str) {
        self.last_fired.remove(name);
    }

    /// Reset all cooldown timers.
    pub fn reset_all_cooldowns(&mut self) {
        self.last_fired.clear();
    }
}

impl Default for AutomationEngine {
    fn default() -> Self {
        Self::new()
    }
}

// ── Trigger Matching ────────────────────────────────────────────────────

/// Check whether an incoming trigger matches a rule's trigger source.
///
/// Matching is based on variant discrimination — we compare the
/// discriminant and relevant fields.
fn trigger_matches(rule_trigger: &TriggerSource, incoming: &TriggerSource) -> bool {
    match (rule_trigger, incoming) {
        (
            TriggerSource::TimeOfDay {
                hour: rh,
                minute: rm,
            },
            TriggerSource::TimeOfDay {
                hour: ih,
                minute: im,
            },
        ) => rh == ih && rm == im,

        (TriggerSource::Sunset, TriggerSource::Sunset)
        | (TriggerSource::Sunrise, TriggerSource::Sunrise)
        | (TriggerSource::GameDetected, TriggerSource::GameDetected)
        | (TriggerSource::Manual, TriggerSource::Manual) => true,

        (TriggerSource::AppLaunched(rule_app), TriggerSource::AppLaunched(incoming_app)) => {
            rule_app == incoming_app
        }

        (
            TriggerSource::AudioLevel {
                threshold: rule_thresh,
            },
            TriggerSource::AudioLevel {
                threshold: incoming_level,
            },
        ) => {
            // The incoming "threshold" is the current level; it fires if
            // the current level exceeds the rule's threshold.
            incoming_level >= rule_thresh
        }

        _ => false,
    }
}

/// Evaluate string-based conditions.
///
/// For now, conditions are simple string expressions. An empty condition
/// list always passes. Non-empty strings are treated as "truthy" if they
/// equal `"true"` (case-insensitive). This is a placeholder for a full
/// expression evaluator in a future iteration.
fn evaluate_conditions(conditions: &[String]) -> bool {
    conditions.iter().all(|cond| {
        let trimmed = cond.trim();
        trimmed.is_empty() || trimmed.eq_ignore_ascii_case("true")
    })
}
