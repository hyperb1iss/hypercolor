use std::collections::HashSet;

use hypercolor_core::spatial::SpatialEngine;
use hypercolor_types::api::PageInfo;
use hypercolor_types::api::layouts::{
    ApplyLayoutResponse, CreateLayoutRequest, DeleteLayoutResponse, LayoutListResponse,
    LayoutSummary, PreviewLayoutResponse, UpdateLayoutRequest,
};
use hypercolor_types::identity::LayoutId;
use hypercolor_types::spatial::{Output, SamplingMode, SpatialLayout};

use crate::domain::DomainError;
use crate::scene_transactions::LayoutUpdateGuard;

use super::{
    LayoutContext, LayoutMutationResult, LayoutMutationTestOperation, LayoutMutationTestPoint,
    LayoutPersistenceStatus, LayoutRuntime, await_layout_workflow, empty_default_layout,
    layout_store_persistence_error, layout_summary, layout_update_domain_error,
    normalize_layout_name, resolve_layout_key, validate_canvas_dimensions,
    validate_layout_sampling_radii, validate_output_sampling_radii,
};

impl LayoutContext {
    /// List stored layouts in stable display order with bounded pagination.
    pub async fn list(&self, limit: usize, offset: usize, active_only: bool) -> LayoutListResponse {
        let active_layout_id = self.current().id;
        let layouts = self.catalog.entries().read().await;
        let mut items: Vec<LayoutSummary> = layouts
            .values()
            .map(|layout| layout_summary(layout, layout.id == active_layout_id))
            .collect();
        items.sort_by_cached_key(|item| item.name.to_lowercase());
        if active_only {
            items.retain(|layout| layout.is_active);
        }

        let total = items.len();
        let items = items.into_iter().skip(offset).take(limit).collect();
        LayoutListResponse {
            items,
            total: u64::try_from(total).expect("layout count fits in u64"),
            page: Some(PageInfo {
                offset: u64::try_from(offset).expect("layout offset fits in u64"),
                limit: u64::try_from(limit).expect("layout limit fits in u64"),
                has_more: offset.saturating_add(limit) < total,
            }),
        }
    }

    /// Resolve a stored layout by canonical id or exact case-insensitive name.
    ///
    /// # Errors
    ///
    /// Returns not-found or conflict when the selector has no unique match.
    pub async fn resolve(&self, id_or_name: &str) -> Result<SpatialLayout, DomainError> {
        let layouts = self.catalog.entries().read().await;
        let key = resolve_layout_key(&layouts, id_or_name)?;
        Ok(layouts
            .get(&key)
            .expect("resolved layout key must exist")
            .clone())
    }

    pub(crate) async fn get(&self, layout_id: &LayoutId) -> Option<SpatialLayout> {
        self.catalog
            .entries()
            .read()
            .await
            .get(layout_id.as_str())
            .cloned()
    }

    /// Capture the currently published layout.
    #[must_use]
    pub fn current(&self) -> SpatialLayout {
        self.publication.current()
    }

    pub(crate) fn active_layout_id(&self) -> Result<LayoutId, DomainError> {
        LayoutId::new(self.current().id).map_err(|error| {
            DomainError::Internal(anyhow::anyhow!("active layout has an invalid id: {error}"))
        })
    }

    /// Create and durably store one layout.
    ///
    /// # Errors
    ///
    /// Returns validation, conflict, or persistence failures without leaving
    /// an in-memory catalog mutation behind.
    pub async fn create(&self, body: CreateLayoutRequest) -> Result<LayoutSummary, DomainError> {
        let context = self.clone();
        await_layout_workflow(crate::audit_log::spawn_attributed(async move {
            context.create_workflow(body).await
        }))
        .await
    }

    async fn create_workflow(
        &self,
        body: CreateLayoutRequest,
    ) -> Result<LayoutSummary, DomainError> {
        let normalized_name = normalize_layout_name(&body.name)?;
        self.wait_test_hook(
            LayoutMutationTestPoint::BeforeGuard,
            LayoutMutationTestOperation::Create,
            &normalized_name,
        )
        .await;
        let guard = self.acquire_update_guard().await;
        let (canvas_width, canvas_height) = self.render_canvas.get();

        let mut layouts = self.catalog.entries().write().await;
        if layouts
            .values()
            .any(|layout| layout.name.eq_ignore_ascii_case(&normalized_name))
        {
            return Err(DomainError::conflict(format!(
                "Layout already exists: {normalized_name}"
            )));
        }

        let mutation_reference = normalized_name.clone();
        let id = format!("layout_{}", uuid::Uuid::now_v7());
        let layout = SpatialLayout {
            id: id.clone(),
            name: normalized_name,
            description: body.description,
            canvas_width,
            canvas_height,
            zones: Vec::new(),
            default_sampling_mode: SamplingMode::Bilinear,
            default_edge_behavior: hypercolor_types::spatial::EdgeBehavior::Clamp,
            version: 1,
        };
        let summary = layout_summary(&layout, false);
        layouts.insert(id.clone(), layout);
        drop(layouts);
        self.wait_test_hook(
            LayoutMutationTestPoint::AfterMemoryMutation,
            LayoutMutationTestOperation::Create,
            &mutation_reference,
        )
        .await;

        if let Err(error) = self.catalog.persist().await {
            self.catalog.entries().write().await.remove(&id);
            let rollback = self
                .catalog
                .persist()
                .await
                .err()
                .map(|error| format!("layout store rollback failed: {error}"));
            drop(guard);
            return Err(layout_store_persistence_error("create", error, rollback));
        }
        self.publish_layout_changed(None, id);
        drop(guard);
        self.wait_test_hook(
            LayoutMutationTestPoint::AfterWorkflow,
            LayoutMutationTestOperation::Create,
            &mutation_reference,
        )
        .await;
        Ok(summary)
    }

    /// Update and durably store one selected layout.
    ///
    /// # Errors
    ///
    /// Returns selector, validation, or persistence failures. Failed writes
    /// restore the previous catalog entry before the method returns.
    pub async fn update(
        &self,
        selector: String,
        body: UpdateLayoutRequest,
    ) -> Result<LayoutSummary, DomainError> {
        let context = self.clone();
        await_layout_workflow(crate::audit_log::spawn_attributed(async move {
            context.update_workflow(&selector, body).await
        }))
        .await
    }

    async fn update_workflow(
        &self,
        selector: &str,
        body: UpdateLayoutRequest,
    ) -> Result<LayoutSummary, DomainError> {
        if let Some(zones) = &body.zones {
            for output in zones {
                validate_output_sampling_radii(output)?;
            }
        }
        self.wait_test_hook(
            LayoutMutationTestPoint::BeforeGuard,
            LayoutMutationTestOperation::Update,
            selector,
        )
        .await;
        let guard = self.acquire_update_guard().await;
        let active_layout_id = self.current().id;
        let mut layouts = self.catalog.entries().write().await;
        let key = resolve_layout_key(&layouts, selector)?;
        let existing = layouts
            .get(&key)
            .expect("resolved layout key must exist")
            .clone();

        let UpdateLayoutRequest {
            name,
            description,
            zones,
        } = body;
        let previous_zones = zones.as_ref().map(|_| existing.zones.clone());
        let updated_zones_for_exclusions = zones.clone();
        let layout_id = existing.id.clone();
        let previous_layout = existing.clone();
        let mut updated = existing;
        if let Some(name) = name {
            updated.name = normalize_layout_name(&name)?;
        }
        updated.description = description;
        if let Some(zones) = zones {
            updated.zones = zones;
        }
        SpatialEngine::try_new(updated.clone())
            .map_err(|error| DomainError::validation(error.to_string()))?;

        let summary = layout_summary(&updated, updated.id == active_layout_id);
        layouts.insert(key, updated);
        drop(layouts);
        self.wait_test_hook(
            LayoutMutationTestPoint::AfterMemoryMutation,
            LayoutMutationTestOperation::Update,
            &layout_id,
        )
        .await;

        if let Err(error) = self.catalog.persist().await {
            self.catalog
                .entries()
                .write()
                .await
                .insert(layout_id.clone(), previous_layout);
            let rollback = self
                .catalog
                .persist()
                .await
                .err()
                .map(|error| format!("layout store rollback failed: {error}"));
            drop(guard);
            return Err(layout_store_persistence_error("update", error, rollback));
        }
        if let (Some(previous_zones), Some(updated_zones)) =
            (previous_zones, updated_zones_for_exclusions)
        {
            self.exclusions
                .reconcile_layout(&layout_id, &previous_zones, &updated_zones)
                .await;
        }
        self.publish_layout_changed(None, layout_id.clone());
        drop(guard);
        self.wait_test_hook(
            LayoutMutationTestPoint::AfterWorkflow,
            LayoutMutationTestOperation::Update,
            &layout_id,
        )
        .await;
        Ok(summary)
    }

    pub(crate) async fn apply(
        &self,
        selector: String,
        runtime: LayoutRuntime,
    ) -> Result<LayoutMutationResult<ApplyLayoutResponse>, DomainError> {
        let context = self.clone();
        await_layout_workflow(crate::audit_log::spawn_attributed(async move {
            context.apply_workflow(&selector, &runtime).await
        }))
        .await
    }

    async fn apply_workflow(
        &self,
        selector: &str,
        runtime: &LayoutRuntime,
    ) -> Result<LayoutMutationResult<ApplyLayoutResponse>, DomainError> {
        self.wait_test_hook(
            LayoutMutationTestPoint::BeforeGuard,
            LayoutMutationTestOperation::Apply,
            selector,
        )
        .await;
        let guard = self.acquire_update_guard().await;
        let previous_active_id = self.current().id;
        let mut layout = self.resolve(selector).await?;
        self.render_canvas.stamp(&mut layout);
        self.admit_persisted_update_under_guard(&guard, layout.clone(), runtime)
            .await
            .map_err(layout_update_domain_error)?;
        self.publish_layout_changed(Some(previous_active_id), layout.id.clone());
        drop(guard);
        let persistence = self.converge_persisted_update(runtime).await;
        self.wait_test_hook(
            LayoutMutationTestPoint::AfterWorkflow,
            LayoutMutationTestOperation::Apply,
            &layout.id,
        )
        .await;
        Ok(LayoutMutationResult {
            data: ApplyLayoutResponse {
                layout,
                applied: true,
                persistence_pending: persistence == LayoutPersistenceStatus::Pending,
            },
            persistence,
        })
    }

    /// Carry a Studio save on a primary zone that follows the active
    /// layout into that layout.
    ///
    /// Effect applies, layout publications, and the startup restore
    /// rebuild such a zone from the active layout, so a placement saved
    /// only on the zone is lost at the next rebuild. Only what the save
    /// changed travels: for every output whose placement differs between
    /// `before` and `after`, the placement fields are copied by output id
    /// onto the stored entry and onto the published layout, wherever each
    /// already holds that output. Outputs the zone holds that neither
    /// layout knows (members assigned in Studio) are added to both, as the
    /// startup restore would. Nothing is removed, output order and every
    /// other field stay as they were, auto-layout exclusions are left
    /// alone, and a stored edit that was never applied stays stored and
    /// unapplied. An active layout with no stored entry is only published.
    ///
    /// The stored entry is persisted first, then the published layout goes
    /// out under the caller's guard; a rejected publication restores the
    /// stored entry. Returns whether a layout was published. The caller
    /// runs connectivity convergence only after releasing the guard,
    /// because that pass takes the guard itself.
    ///
    /// # Errors
    ///
    /// Persistence and publication failures, after the stored entry has
    /// been rolled back, or naming the rollback failure as well when it
    /// could not be.
    pub(crate) async fn mirror_primary_zone_save_under_guard(
        &self,
        guard: &LayoutUpdateGuard,
        before: &[Output],
        after: &[Output],
        runtime: &LayoutRuntime,
    ) -> Result<bool, DomainError> {
        let current = self.current();
        let layout_id = current.id.clone();
        let stored = self.catalog.entries().read().await.get(&layout_id).cloned();

        let mut published = current.clone();
        let mut stored_next = stored.clone();
        for saved in after {
            let moved = before
                .iter()
                .find(|output| output.id == saved.id)
                .is_none_or(|output| placement_differs(output, saved));
            let in_published = copy_placement_if_held(&mut published, saved, moved);
            let in_stored = stored_next
                .as_mut()
                .is_some_and(|layout| copy_placement_if_held(layout, saved, moved));
            if !in_published && !in_stored {
                published.zones.push(saved.clone());
                if let Some(layout) = stored_next.as_mut() {
                    layout.zones.push(saved.clone());
                }
            }
        }

        let stored_changed = stored_next != stored;
        let published_changed = published != current;
        if stored_changed && let Some(layout) = stored_next {
            self.catalog
                .entries()
                .write()
                .await
                .insert(layout_id.clone(), layout);
            if let Err(error) = self.catalog.persist().await {
                let rollback = self
                    .restore_catalog_entry(&layout_id, stored.clone())
                    .await
                    .err()
                    .map(|error| format!("layout store rollback failed: {error}"));
                return Err(layout_store_persistence_error(
                    "zone layout save",
                    error,
                    rollback,
                ));
            }
        }
        if published_changed
            && let Err(error) = self
                .admit_persisted_update_under_guard(guard, published, runtime)
                .await
        {
            if stored_changed
                && let Err(rollback_error) = self.restore_catalog_entry(&layout_id, stored).await
            {
                return Err(DomainError::Internal(anyhow::anyhow!(
                    "{error}; layout store rollback failed: {rollback_error}"
                )));
            }
            return Err(layout_update_domain_error(error));
        }

        if stored_changed || published_changed {
            self.publish_layout_changed(None, layout_id);
        }
        Ok(published_changed)
    }

    async fn restore_catalog_entry(
        &self,
        layout_id: &str,
        previous: Option<SpatialLayout>,
    ) -> anyhow::Result<()> {
        {
            let mut layouts = self.catalog.entries().write().await;
            match previous {
                Some(previous) => {
                    layouts.insert(layout_id.to_owned(), previous);
                }
                None => {
                    layouts.remove(layout_id);
                }
            }
        }
        self.catalog.persist().await
    }

    pub(crate) async fn preview(
        &self,
        layout: SpatialLayout,
        runtime: LayoutRuntime,
    ) -> Result<PreviewLayoutResponse, DomainError> {
        let context = self.clone();
        await_layout_workflow(crate::audit_log::spawn_attributed(async move {
            context.preview_workflow(layout, &runtime).await
        }))
        .await
    }

    async fn preview_workflow(
        &self,
        layout: SpatialLayout,
        runtime: &LayoutRuntime,
    ) -> Result<PreviewLayoutResponse, DomainError> {
        validate_layout_sampling_radii(&layout).map_err(DomainError::validation)?;
        self.wait_test_hook(
            LayoutMutationTestPoint::BeforeGuard,
            LayoutMutationTestOperation::Preview,
            &layout.id,
        )
        .await;
        let guard = self.acquire_update_guard().await;
        let reference = layout.id.clone();
        self.publication
            .apply_prepared_under_guard(&guard, layout)
            .await
            .map_err(layout_update_domain_error)?;
        drop(guard);
        self.wait_test_hook(
            LayoutMutationTestPoint::AfterRendererMutation,
            LayoutMutationTestOperation::Preview,
            &reference,
        )
        .await;
        self.sync_connectivity(runtime.discovery().clone(), None)
            .await;
        self.wait_test_hook(
            LayoutMutationTestPoint::AfterWorkflow,
            LayoutMutationTestOperation::Preview,
            &reference,
        )
        .await;
        Ok(PreviewLayoutResponse { previewing: true })
    }

    pub(crate) async fn delete(
        &self,
        selector: String,
        runtime: LayoutRuntime,
    ) -> Result<LayoutMutationResult<DeleteLayoutResponse>, DomainError> {
        let context = self.clone();
        await_layout_workflow(crate::audit_log::spawn_attributed(async move {
            context.delete_workflow(&selector, &runtime).await
        }))
        .await
    }

    async fn delete_workflow(
        &self,
        selector: &str,
        runtime: &LayoutRuntime,
    ) -> Result<LayoutMutationResult<DeleteLayoutResponse>, DomainError> {
        self.wait_test_hook(
            LayoutMutationTestPoint::BeforeGuard,
            LayoutMutationTestOperation::Delete,
            selector,
        )
        .await;
        let guard = self.acquire_update_guard().await;
        let active_layout = self.current();
        let mut layouts = self.catalog.entries().write().await;
        let key = resolve_layout_key(&layouts, selector)?;
        let removed_layout = layouts
            .remove(&key)
            .expect("resolved layout key must exist");
        let next_active_layout = if key == active_layout.id {
            let mut candidates: Vec<SpatialLayout> = layouts.values().cloned().collect();
            candidates
                .sort_by(|left, right| left.name.cmp(&right.name).then(left.id.cmp(&right.id)));
            Some(
                candidates
                    .into_iter()
                    .next()
                    .unwrap_or_else(|| empty_default_layout(&active_layout)),
            )
        } else {
            None
        };
        drop(layouts);
        self.wait_test_hook(
            LayoutMutationTestPoint::AfterMemoryMutation,
            LayoutMutationTestOperation::Delete,
            &key,
        )
        .await;

        let next_active_id = next_active_layout.as_ref().map(|layout| layout.id.clone());
        let active_layout_changed = next_active_layout.is_some();
        if let Some(layout) = next_active_layout
            && let Err(error) = self
                .admit_persisted_update_under_guard(&guard, layout, runtime)
                .await
        {
            self.catalog
                .entries()
                .write()
                .await
                .insert(key.clone(), removed_layout);
            drop(guard);
            return Err(layout_update_domain_error(error));
        }

        if let Err(error) = self.catalog.persist().await {
            self.catalog
                .entries()
                .write()
                .await
                .insert(key.clone(), removed_layout);
            let mut rollback_errors = Vec::new();
            if let Err(rollback_error) = self.catalog.persist().await {
                rollback_errors.push(format!("layout store rollback failed: {rollback_error}"));
            }
            if active_layout_changed
                && let Err(rollback_error) = self
                    .admit_persisted_update_under_guard(&guard, active_layout, runtime)
                    .await
            {
                rollback_errors.push(format!("active layout rollback failed: {rollback_error}"));
            }
            drop(guard);
            if active_layout_changed
                && self.converge_persisted_update(runtime).await == LayoutPersistenceStatus::Pending
            {
                rollback_errors
                    .push("active layout rollback persistence remains pending".to_owned());
            }
            return Err(layout_store_persistence_error(
                "delete",
                error,
                rollback_errors,
            ));
        }

        self.exclusions.remove_layout(&key).await;
        match next_active_id {
            Some(next_active_id) => {
                self.publish_layout_changed(Some(key.clone()), next_active_id);
            }
            None => self.publish_layout_changed(None, key.clone()),
        }
        drop(guard);
        let persistence = if active_layout_changed {
            self.converge_persisted_update(runtime).await
        } else {
            LayoutPersistenceStatus::Synchronized
        };
        self.wait_test_hook(
            LayoutMutationTestPoint::AfterWorkflow,
            LayoutMutationTestOperation::Delete,
            &key,
        )
        .await;
        Ok(LayoutMutationResult {
            data: DeleteLayoutResponse {
                id: key,
                deleted: true,
                persistence_pending: persistence == LayoutPersistenceStatus::Pending,
            },
            persistence,
        })
    }

    pub(crate) async fn resize_active_canvas(
        &self,
        width: u32,
        height: u32,
    ) -> Result<bool, DomainError> {
        let reference = format!("{width}x{height}");
        self.wait_test_hook(
            LayoutMutationTestPoint::BeforeGuard,
            LayoutMutationTestOperation::ConfigResize,
            &reference,
        )
        .await;
        validate_canvas_dimensions(width, height)?;
        let guard = self.acquire_update_guard().await;
        let current = self.current();
        let previous = self.render_canvas.get();
        if previous == (width, height)
            && current.canvas_width == width
            && current.canvas_height == height
        {
            return Ok(false);
        }
        // The render resolution moves first, so the active layout and every
        // stored one land on it together and none keeps a size of its own.
        self.render_canvas.set(width, height);
        let mut updated = current;
        updated.canvas_width = width;
        updated.canvas_height = height;
        if let Err(error) = self
            .publication
            .apply_prepared_under_guard(&guard, updated.clone())
            .await
        {
            self.render_canvas.set(previous.0, previous.1);
            return Err(layout_update_domain_error(error));
        }

        let persisted_layout_updated = {
            let mut layouts = self.catalog.entries().write().await;
            super::stamp_catalog(&mut layouts, &self.render_canvas) > 0
        };
        self.wait_test_hook(
            LayoutMutationTestPoint::AfterMemoryMutation,
            LayoutMutationTestOperation::ConfigResize,
            &reference,
        )
        .await;
        if persisted_layout_updated {
            self.persist_catalog_best_effort().await;
            self.publish_layout_changed(None, updated.id);
        }
        self.wait_test_hook(
            LayoutMutationTestPoint::AfterWorkflow,
            LayoutMutationTestOperation::ConfigResize,
            &reference,
        )
        .await;
        drop(guard);
        Ok(true)
    }

    pub(crate) async fn prune_targets(
        &self,
        target_ids: HashSet<String>,
        mutation_reference: &str,
    ) -> Result<(), DomainError> {
        self.wait_test_hook(
            LayoutMutationTestPoint::BeforeGuard,
            LayoutMutationTestOperation::SimulatorPrune,
            mutation_reference,
        )
        .await;
        let guard = self.acquire_update_guard().await;
        let previous_layouts = self.catalog.entries().read().await.clone();
        let mut pruned_layout_ids = Vec::new();
        {
            let mut layouts = self.catalog.entries().write().await;
            for layout in layouts.values_mut() {
                let zone_count = layout.zones.len();
                layout
                    .zones
                    .retain(|zone| !target_ids.contains(zone.device_id.as_str()));
                if layout.zones.len() == zone_count {
                    continue;
                }
                pruned_layout_ids.push(layout.id.clone());
            }
        }
        self.wait_test_hook(
            LayoutMutationTestPoint::AfterMemoryMutation,
            LayoutMutationTestOperation::SimulatorPrune,
            mutation_reference,
        )
        .await;
        if let Err(error) = self.catalog.persist().await {
            *self.catalog.entries().write().await = previous_layouts;
            return Err(layout_store_persistence_error("device removal", error, []));
        }
        for layout_id in &pruned_layout_ids {
            let updated = self.catalog.entries().read().await.get(layout_id).cloned();
            if let (Some(previous), Some(updated)) = (previous_layouts.get(layout_id), updated) {
                self.exclusions
                    .reconcile_layout(layout_id, &previous.zones, &updated.zones)
                    .await;
            }
        }
        let active_layout = {
            let mut active = self.current();
            let previous = active.zones.clone();
            let before = active.zones.len();
            active
                .zones
                .retain(|output| !target_ids.contains(&output.device_id));
            if before != active.zones.len() {
                self.exclusions
                    .reconcile_layout(&active.id, &previous, &active.zones)
                    .await;
            }
            (before != active.zones.len()).then_some(active)
        };
        self.exclusions
            .persist_durable()
            .await
            .map_err(DomainError::Internal)?;
        let active_layout_error = if let Some(layout) = active_layout {
            self.publication
                .apply_prepared_under_guard(&guard, layout)
                .await
                .err()
                .map(layout_update_domain_error)
        } else {
            None
        };
        for layout_id in pruned_layout_ids {
            self.publish_layout_changed(None, layout_id);
        }
        drop(guard);
        active_layout_error.map_or(Ok(()), Err)
    }
}

/// Whether the fields a zone layout save carries differ between two copies
/// of one output.
fn placement_differs(before: &Output, after: &Output) -> bool {
    before.position != after.position
        || before.size != after.size
        || before.rotation.to_bits() != after.rotation.to_bits()
        || before.scale.to_bits() != after.scale.to_bits()
        || before.orientation != after.orientation
}

/// Copy a saved placement onto `layout`'s copy of the output when `moved`.
/// Returns whether `layout` holds the output at all.
fn copy_placement_if_held(layout: &mut SpatialLayout, saved: &Output, moved: bool) -> bool {
    let Some(output) = layout.zones.iter_mut().find(|output| output.id == saved.id) else {
        return false;
    };
    if moved {
        output.position = saved.position;
        output.size = saved.size;
        output.rotation = saved.rotation;
        output.scale = saved.scale;
        output.orientation = saved.orientation;
    }
    true
}
