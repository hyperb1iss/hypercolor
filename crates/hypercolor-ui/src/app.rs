use leptos::prelude::*;
use leptos_meta::*;
use leptos_router::components::{Route, Router, Routes};
use leptos_router::path;

use crate::components::shell::Shell;
use crate::pages::dashboard::DashboardPage;
use crate::pages::effects::EffectsPage;

#[component]
pub fn App() -> impl IntoView {
    provide_meta_context();

    view! {
        <Meta charset="UTF-8" />
        <Meta name="viewport" content="width=device-width, initial-scale=1.0" />
        <Title text="Hypercolor" />

        <Router>
            <Shell>
                <Routes fallback=|| view! { <p class="text-zinc-400 p-8">"Not found"</p> }>
                    <Route path=path!("/") view=DashboardPage />
                    <Route path=path!("/effects") view=EffectsPage />
                    <Route path=path!("/effects/:id") view=EffectsPage />
                </Routes>
            </Shell>
        </Router>
    }
}
