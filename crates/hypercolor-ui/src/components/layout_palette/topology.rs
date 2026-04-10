//! Topology icon helper — maps `ZoneTopologySummary` variants to Lucide icons.

use leptos::prelude::*;
use leptos_icons::Icon;

use crate::api::{self, ZoneTopologySummary};
use crate::icons::*;

/// Return an appropriate icon view based on zone topology.
pub(super) fn topology_icon(zone: Option<&api::ZoneSummary>) -> leptos::prelude::AnyView {
    match zone.and_then(|z| z.topology_hint.as_ref()) {
        Some(ZoneTopologySummary::Strip) => {
            view! { <Icon icon=LuMinus width="12px" height="12px" /> }.into_any()
        }
        Some(ZoneTopologySummary::Matrix { .. }) => {
            view! { <Icon icon=LuGrid2x2 width="12px" height="12px" /> }.into_any()
        }
        Some(ZoneTopologySummary::Ring { .. }) => {
            view! { <Icon icon=LuCircle width="12px" height="12px" /> }.into_any()
        }
        Some(ZoneTopologySummary::Point) => {
            view! { <Icon icon=LuCircleDot width="12px" height="12px" /> }.into_any()
        }
        _ => view! { <Icon icon=LuMinus width="12px" height="12px" /> }.into_any(),
    }
}
