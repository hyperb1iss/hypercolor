//! Deterministic selector resolution for MCP's human-friendly arguments.

use std::cmp::Ordering;

use serde::Serialize;

/// One candidate exposed to callers when a selector cannot resolve uniquely.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SelectorCandidateDetail {
    /// Canonical serialized resource identifier.
    pub id: String,
    /// Human-readable name, absent for resources that only resolve by id.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

/// A selectable resource and the value returned when it resolves.
#[derive(Debug, Clone)]
pub struct SelectorCandidate<T> {
    detail: SelectorCandidateDetail,
    value: T,
}

impl<T> SelectorCandidate<T> {
    /// Build a candidate that resolves by serialized id or human-readable name.
    pub fn named(id: impl Into<String>, name: impl Into<String>, value: T) -> Self {
        Self {
            detail: SelectorCandidateDetail {
                id: id.into(),
                name: Some(name.into()),
            },
            value,
        }
    }

    /// Build a candidate that resolves only by serialized id.
    pub fn unnamed(id: impl Into<String>, value: T) -> Self {
        Self {
            detail: SelectorCandidateDetail {
                id: id.into(),
                name: None,
            },
            value,
        }
    }
}

/// Why a selector did not resolve to exactly one resource.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SelectorError {
    /// No id or name matched the query.
    #[error("no resource matches '{query}'")]
    NoMatch {
        /// The caller's normalized query.
        query: String,
        /// Available resources in deterministic order.
        candidates: Vec<SelectorCandidateDetail>,
    },
    /// More than one name matched at the same precedence level.
    #[error("selector '{query}' is ambiguous")]
    Ambiguous {
        /// The caller's normalized query.
        query: String,
        /// Matching resources in deterministic order.
        candidates: Vec<SelectorCandidateDetail>,
    },
}

impl SelectorError {
    /// Stable machine-readable failure kind.
    #[must_use]
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::NoMatch { .. } => "no_match",
            Self::Ambiguous { .. } => "ambiguous",
        }
    }

    /// The normalized query that failed.
    #[must_use]
    pub fn query(&self) -> &str {
        match self {
            Self::NoMatch { query, .. } | Self::Ambiguous { query, .. } => query,
        }
    }

    /// Deterministically sorted available or ambiguous candidates.
    #[must_use]
    pub fn candidates(&self) -> &[SelectorCandidateDetail] {
        match self {
            Self::NoMatch { candidates, .. } | Self::Ambiguous { candidates, .. } => candidates,
        }
    }
}

/// Resolve one MCP selector with a single deterministic policy.
///
/// Precedence is exact serialized id, exact case-insensitive name, then a
/// unique case-insensitive name substring. Unnamed candidates participate only
/// in the id stage.
pub fn resolve<T>(
    query: &str,
    mut candidates: Vec<SelectorCandidate<T>>,
) -> Result<T, SelectorError> {
    let query = query.trim().to_owned();
    candidates.sort_by(|left, right| compare_details(&left.detail, &right.detail));

    let id_matches = matching_indices(&candidates, |candidate| candidate.detail.id == query);
    if let Some(result) = select_match(&query, &mut candidates, id_matches) {
        return result;
    }

    let lowercase_query = query.to_lowercase();
    let exact_name_matches = matching_indices(&candidates, |candidate| {
        candidate
            .detail
            .name
            .as_ref()
            .is_some_and(|name| name.to_lowercase() == lowercase_query)
    });
    if let Some(result) = select_match(&query, &mut candidates, exact_name_matches) {
        return result;
    }

    let substring_matches = if lowercase_query.is_empty() {
        Vec::new()
    } else {
        matching_indices(&candidates, |candidate| {
            candidate
                .detail
                .name
                .as_ref()
                .is_some_and(|name| name.to_lowercase().contains(&lowercase_query))
        })
    };
    if let Some(result) = select_match(&query, &mut candidates, substring_matches) {
        return result;
    }

    Err(SelectorError::NoMatch {
        query,
        candidates: candidates
            .into_iter()
            .map(|candidate| candidate.detail)
            .collect(),
    })
}

fn matching_indices<T>(
    candidates: &[SelectorCandidate<T>],
    predicate: impl Fn(&SelectorCandidate<T>) -> bool,
) -> Vec<usize> {
    candidates
        .iter()
        .enumerate()
        .filter_map(|(index, candidate)| predicate(candidate).then_some(index))
        .collect()
}

fn select_match<T>(
    query: &str,
    candidates: &mut Vec<SelectorCandidate<T>>,
    indices: Vec<usize>,
) -> Option<Result<T, SelectorError>> {
    match indices.as_slice() {
        [] => None,
        [index] => Some(Ok(candidates.remove(*index).value)),
        _ => Some(Err(SelectorError::Ambiguous {
            query: query.to_owned(),
            candidates: indices
                .into_iter()
                .map(|index| candidates[index].detail.clone())
                .collect(),
        })),
    }
}

fn compare_details(left: &SelectorCandidateDetail, right: &SelectorCandidateDetail) -> Ordering {
    let left_name = left.name.as_deref().unwrap_or_default();
    let right_name = right.name.as_deref().unwrap_or_default();
    left_name
        .to_lowercase()
        .cmp(&right_name.to_lowercase())
        .then_with(|| left_name.cmp(right_name))
        .then_with(|| left.id.cmp(&right.id))
}

#[cfg(test)]
mod tests {
    use super::{SelectorCandidate, SelectorError, resolve};

    #[test]
    fn exact_id_precedes_an_exact_name() {
        let resolved = resolve(
            "shared",
            vec![
                SelectorCandidate::named("first", "shared", 1),
                SelectorCandidate::named("shared", "different", 2),
            ],
        )
        .expect("the serialized id must win");

        assert_eq!(resolved, 2);
    }

    #[test]
    fn exact_name_precedes_substrings() {
        let resolved = resolve(
            "AURORA",
            vec![
                SelectorCandidate::named("first", "Aurora Borealis", 1),
                SelectorCandidate::named("second", "Aurora", 2),
            ],
        )
        .expect("the exact name must win");

        assert_eq!(resolved, 2);
    }

    #[test]
    fn ambiguity_candidates_have_canonical_order() {
        let error = resolve(
            "glow",
            vec![
                SelectorCandidate::named("z", "glow", 1),
                SelectorCandidate::named("b", "Glow", 2),
                SelectorCandidate::named("a", "Glow", 3),
                SelectorCandidate::named("c", "afterglow", 4),
            ],
        )
        .expect_err("duplicate exact names must be ambiguous");

        let SelectorError::Ambiguous { candidates, .. } = error else {
            panic!("expected ambiguity");
        };
        assert_eq!(
            candidates
                .iter()
                .map(|candidate| candidate.id.as_str())
                .collect::<Vec<_>>(),
            ["a", "b", "z"]
        );
    }

    #[test]
    fn unnamed_candidates_only_resolve_by_id() {
        let error = resolve("layer", vec![SelectorCandidate::unnamed("layer-id", 1)])
            .expect_err("unnamed layers must not resolve by substring");
        assert!(matches!(error, SelectorError::NoMatch { .. }));

        let resolved = resolve("layer-id", vec![SelectorCandidate::unnamed("layer-id", 1)])
            .expect("the exact layer id must resolve");
        assert_eq!(resolved, 1);
    }
}
