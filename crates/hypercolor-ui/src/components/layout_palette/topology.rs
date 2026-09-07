//! Topology icon helper for device segment topology variants.

use leptos::prelude::*;
use leptos_icons::Icon;

use crate::api::{self, SegmentTopologySummary};
use crate::icons::*;

/// Return an appropriate icon view based on zone topology.
pub(super) fn topology_icon(zone: Option<&api::SegmentSummary>) -> leptos::prelude::AnyView {
    match zone.and_then(|z| z.topology_hint.as_ref()) {
        Some(SegmentTopologySummary::Strip) => {
            view! { <Icon icon=LuMinus width="12px" height="12px" /> }.into_any()
        }
        Some(SegmentTopologySummary::Matrix { .. }) => {
            view! { <Icon icon=LuGrid2x2 width="12px" height="12px" /> }.into_any()
        }
        Some(SegmentTopologySummary::Ring { .. }) => {
            view! { <Icon icon=LuCircle width="12px" height="12px" /> }.into_any()
        }
        Some(SegmentTopologySummary::Point) => {
            view! { <Icon icon=LuCircleDot width="12px" height="12px" /> }.into_any()
        }
        _ => view! { <Icon icon=LuMinus width="12px" height="12px" /> }.into_any(),
    }
}
