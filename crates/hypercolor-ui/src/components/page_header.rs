use icondata_core::Icon as IconData;
use leptos::prelude::*;
use leptos_icons::Icon;

#[component]
pub fn PageHeader(
    icon: IconData,
    #[prop(into)] title: String,
    #[prop(into)] subtitle: String,
    #[prop(into)] accent_rgb: String,
    #[prop(into)] gradient: String,
) -> impl IntoView {
    let icon_style =
        format!("color: rgb({accent_rgb}); filter: drop-shadow(0 0 10px rgba({accent_rgb}, 0.55))");
    let title_style = format!(
        "font-family:'Orbitron',sans-serif; font-weight:900; font-size:22px; \
         letter-spacing:-0.01em; background-image:{gradient}"
    );
    let show_subtitle = !subtitle.trim().is_empty();

    view! {
        <div class="min-w-0">
            <div class="flex items-center gap-2.5">
                <span class="shrink-0" style=icon_style>
                    <Icon icon=icon width="20px" height="20px" />
                </span>
                <h1 class="leading-none logo-gradient-text" style=title_style>
                    {title}
                </h1>
            </div>

            {show_subtitle.then(|| {
                view! {
                    <p class="mt-2 max-w-3xl text-sm leading-relaxed text-fg-tertiary/82">
                        {subtitle}
                    </p>
                }
            })}
        </div>
    }
}
