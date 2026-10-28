use futures_util::future::{AbortHandle, Abortable};
use hypercolor_leptos_ext::events::Change;
use leptos::{html, prelude::*};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicU64, Ordering},
};

use crate::components::settings_controls::SectionHeader;
use crate::{api, app::WsContext, config_state::ConfigContext, icons::*};

#[component]
pub fn BackupsSection() -> impl IntoView {
    let config = expect_context::<ConfigContext>();
    let connection = expect_context::<WsContext>().connection_generation;
    let remote = crate::remote_bridge::is_available();
    let access = use_context::<api::backups::BackupAccess>();
    let can_download =
        Memo::new(move |_| !remote || access.is_some_and(|access| access.download.get()));
    let busy = RwSignal::new(false);
    let preview = RwSignal::new(None::<api::backups::BackupPreview>);
    let error = RwSignal::new(None::<String>);
    let status = RwSignal::new(None::<String>);
    let file_input = NodeRef::<html::Input>::new();
    let generation = Arc::new(AtomicU64::new(0));
    let running = Arc::new(Mutex::new(None::<AbortHandle>));
    let cleanup_generation = Arc::clone(&generation);
    let cleanup_running = Arc::clone(&running);
    on_cleanup(move || {
        cleanup_generation.fetch_add(1, Ordering::AcqRel);
        if let Some(handle) = cleanup_running.lock().expect("backup transfer lock").take() {
            handle.abort();
        }
    });
    let connection_generation = Arc::clone(&generation);
    let connection_running = Arc::clone(&running);
    Effect::new(move |_| {
        connection.get();
        connection_generation.fetch_add(1, Ordering::AcqRel);
        if let Some(handle) = connection_running
            .lock()
            .expect("backup transfer lock")
            .take()
        {
            handle.abort();
        }
        preview.set(None);
        busy.set(false);
        error.set(None);
        status.set(None);
    });

    let export_generation = Arc::clone(&generation);
    let export_running = Arc::clone(&running);
    let export = move |_| {
        if !can_download.get_untracked() {
            return;
        }
        let captured = export_generation.load(Ordering::Acquire);
        let epoch = Arc::clone(&export_generation);
        let (handle, registration) = AbortHandle::new_pair();
        *export_running.lock().expect("backup transfer lock") = Some(handle);
        busy.set(true);
        error.set(None);
        status.set(None);
        leptos::task::spawn_local(async move {
            let result = Abortable::new(api::backups::export(), registration).await;
            if epoch.load(Ordering::Acquire) != captured {
                return;
            }
            busy.set(false);
            match result {
                Ok(Ok(())) => status.set(Some("Backup downloaded.".into())),
                Ok(Err(problem)) => error.set(Some(problem.to_string())),
                Err(_) => {}
            }
        });
    };

    let upload_generation = Arc::clone(&generation);
    let upload_running = Arc::clone(&running);
    let upload = move |event: web_sys::Event| {
        if remote {
            return;
        }
        let Some(file) = Change::from_event(event)
            .files()
            .and_then(|files| files.get(0))
        else {
            return;
        };
        let captured = upload_generation.load(Ordering::Acquire);
        let epoch = Arc::clone(&upload_generation);
        let (handle, registration) = AbortHandle::new_pair();
        *upload_running.lock().expect("backup transfer lock") = Some(handle);
        busy.set(true);
        preview.set(None);
        error.set(None);
        status.set(Some("Reading backup…".into()));
        leptos::task::spawn_local(async move {
            let result = Abortable::new(api::backups::preview(file), registration).await;
            if epoch.load(Ordering::Acquire) != captured {
                return;
            }
            busy.set(false);
            status.set(None);
            if let Some(input) = file_input.get() {
                input.set_value("");
            }
            match result {
                Ok(Ok(candidate)) => preview.set(Some(candidate)),
                Ok(Err(problem)) => error.set(Some(problem.to_string())),
                Err(_) => {}
            }
        });
    };

    let apply = move |_| {
        if remote {
            return;
        }
        let Some(candidate) = preview.get_untracked() else {
            return;
        };
        let captured = generation.load(Ordering::Acquire);
        let epoch = Arc::clone(&generation);
        let (handle, registration) = AbortHandle::new_pair();
        *running.lock().expect("backup transfer lock") = Some(handle);
        busy.set(true);
        error.set(None);
        status.set(Some("Restoring lighting…".into()));
        leptos::task::spawn_local(async move {
            let result =
                Abortable::new(api::backups::apply(candidate.operation_id), registration).await;
            if epoch.load(Ordering::Acquire) != captured {
                return;
            }
            busy.set(false);
            status.set(None);
            match result {
                Ok(Ok(report)) => {
                    preview.set(None);
                    status.set(Some(if report.unresolved_devices.is_empty() {
                        "Backup restored.".into()
                    } else {
                        format!(
                            "Backup restored. Waiting for hardware: {}.",
                            report.unresolved_devices.join(", ")
                        )
                    }));
                    config.refresh.run(());
                }
                Ok(Err(problem)) => error.set(Some(problem.to_string())),
                Err(_) => {}
            }
        });
    };

    view! {
        <section id="section-backups">
            <SectionHeader title="Backup files" icon=LuSave />
            <p class="text-sm text-fg-secondary mb-4">"Save your scenes, layouts, custom effects, media, favorites, and lighting settings in one file."</p>
            <div class="flex flex-wrap gap-3">
                <Show when=move || can_download.get()>
                    <button class="px-3 py-2 rounded-lg text-sm border border-[var(--border-default)] text-fg-primary bg-[var(--surface-raised)] hover:bg-[var(--surface-hover)] disabled:opacity-50 disabled:cursor-not-allowed" disabled=move || busy.get() on:click=export.clone()>"Download backup"</button>
                </Show>
                {(!remote).then(|| view! {
                    <button class="px-3 py-2 rounded-lg text-sm border border-[var(--border-default)] text-fg-primary bg-[var(--surface-raised)] hover:bg-[var(--surface-hover)] disabled:opacity-50 disabled:cursor-not-allowed" disabled=move || busy.get() on:click=move |_| { if let Some(input) = file_input.get() { input.click(); } }>"Import backup"</button>
                    <input node_ref=file_input type="file" class="hidden" accept=".hypercolor-backup,.tar,application/x-tar" on:change=upload />
                })}
            </div>
            {remote.then(|| view! {
                <Show when=move || !can_download.get()>
                    <p class="mt-3 text-sm text-fg-secondary">"Manage backup files in Hypercolor on this computer."</p>
                </Show>
            })}
            {move || preview.get().map(|candidate| view! {
                <div class="mt-5 border border-[var(--border-default)] rounded-lg p-4 space-y-3" role="region" aria-label="Backup preview">
                    <h3 class="text-sm font-semibold text-fg-primary">{candidate.name}</h3>
                    <p class="text-sm text-fg-secondary">{format!("{} scenes, {} layouts, {} custom effects, {} media files, {} favorites",candidate.scenes,candidate.layouts,candidate.effects,candidate.media,candidate.favorites)}</p>
                    <p class="text-xs text-fg-tertiary">"Matching saved items will be updated. Other saved items stay in place."</p>
                    {(!candidate.unresolved_devices.is_empty()).then(|| view! {
                        <p class="text-sm text-fg-secondary">{format!("Waiting for hardware: {}", candidate.unresolved_devices.join(", "))}</p>
                    })}
                    <div class="flex gap-3">
                        <button class="px-3 py-2 rounded-lg text-sm font-medium border border-[var(--accent)] text-[var(--accent)] disabled:opacity-50 disabled:cursor-not-allowed" disabled=move || busy.get() on:click=apply.clone()>"Restore backup"</button>
                        <button class="px-3 py-2 rounded-lg text-sm text-fg-secondary hover:text-fg-primary disabled:opacity-50" disabled=move || busy.get() on:click=move |_| preview.set(None)>"Cancel"</button>
                    </div>
                </div>
            })}
            <div role="status" aria-live="polite" class="mt-3 text-sm text-fg-secondary">{move || status.get()}</div>
            {move || error.get().map(|message| view! { <p role="alert" class="mt-3 text-sm text-[var(--status-error)]">{message}</p> })}
        </section>
    }
}
