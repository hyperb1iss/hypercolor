//! The bridged main canvas preview in a browser: each preview state, the
//! video element under a live bridge, and the demand the page reports.
#![cfg(target_arch = "wasm32")]

use hypercolor_ui::components::remote_video_preview::RemoteVideoPreview;
use hypercolor_ui::remote_bridge::{PreviewChannel, PreviewDemand, PreviewState};
use hypercolor_ui::remote_preview::RemotePreviewContext;
use leptos::prelude::*;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::JsFuture;
use wasm_bindgen_test::*;

wasm_bindgen_test_configure!(run_in_browser);

#[wasm_bindgen(inline_js = r##"
function paintedStream() {
  const canvas = document.createElement("canvas");
  canvas.width = 64;
  canvas.height = 48;
  const context = canvas.getContext("2d");
  let hue = 0;
  const paint = () => {
    hue = (hue + 7) % 360;
    context.fillStyle = `hsl(${hue}, 100%, 55%)`;
    context.fillRect(0, 0, 64, 48);
    requestAnimationFrame(paint);
  };
  paint();
  return canvas.captureStream(30);
}

export function previewBridge(state, withStream) {
  const probe = {
    state, stream: withStream ? paintedStream() : null, calls: [], refusing: false, refused: 0,
    announcing: false,
  };
  window.__previewProbe = probe;
  return {
    previewState() {
      if (probe.state === "throw") throw new Error("bridge failed");
      return probe.state;
    },
    previewStream() { return probe.stream; },
    setPreview(demand) {
      if (probe.refusing) {
        probe.refused += 1;
        throw new Error("no session yet");
      }
      probe.calls.push({ enabled: demand.enabled, maxWidth: demand.maxWidth });
      if (probe.announcing) {
        window.dispatchEvent(new CustomEvent("hypercolor:remote-preview-state"));
      }
    },
  };
}

export function announceFromSetPreview() { window.__previewProbe.announcing = true; }

export function refusePreview(refusing) { window.__previewProbe.refusing = refusing; }
export function refusedCalls() { return window.__previewProbe.refused; }

export function announceState() {
  window.dispatchEvent(new CustomEvent("hypercolor:remote-preview-state"));
}

export function setPageHidden(hidden) {
  Object.defineProperty(document, "hidden", { configurable: true, get: () => hidden });
  document.dispatchEvent(new Event("visibilitychange"));
}

export function restorePageVisibility() {
  delete document.hidden;
  document.dispatchEvent(new Event("visibilitychange"));
}

export function moveHost(host, top) { host.style.top = top; }

export function goLive() {
  const probe = window.__previewProbe;
  probe.state = "live";
  probe.stream = paintedStream();
  window.dispatchEvent(new CustomEvent("hypercolor:remote-preview-state"));
}

export function probeStream() { return window.__previewProbe.stream; }
export function probeCalls() { return JSON.stringify(window.__previewProbe.calls); }

export function surfaceHost() {
  const host = document.createElement("div");
  host.style.cssText = "position: fixed; top: 0; left: 0; width: 320px;";
  document.body.appendChild(host);
  return host;
}

export function sleep(ms) { return new Promise((resolve) => setTimeout(resolve, ms)); }
"##)]
extern "C" {
    #[wasm_bindgen(js_name = previewBridge)]
    fn preview_bridge(state: &str, with_stream: bool) -> JsValue;
    #[wasm_bindgen(js_name = goLive)]
    fn go_live();
    #[wasm_bindgen(js_name = probeStream)]
    fn probe_stream() -> JsValue;
    #[wasm_bindgen(js_name = probeCalls)]
    fn probe_calls() -> String;
    #[wasm_bindgen(js_name = refusePreview)]
    fn refuse_preview(refusing: bool);
    #[wasm_bindgen(js_name = refusedCalls)]
    fn refused_calls() -> u32;
    #[wasm_bindgen(js_name = announceState)]
    fn announce_state();
    #[wasm_bindgen(js_name = announceFromSetPreview)]
    fn announce_from_set_preview();
    #[wasm_bindgen(js_name = setPageHidden)]
    fn set_page_hidden(hidden: bool);
    #[wasm_bindgen(js_name = restorePageVisibility)]
    fn restore_page_visibility();
    #[wasm_bindgen(js_name = moveHost)]
    fn move_host(host: &web_sys::HtmlElement, top: &str);
    #[wasm_bindgen(js_name = surfaceHost)]
    fn surface_host() -> web_sys::HtmlElement;
    fn sleep(ms: u32) -> js_sys::Promise;
}

async fn wait_until(what: &str, mut ready: impl FnMut() -> bool) {
    for _ in 0..200 {
        if ready() {
            return;
        }
        JsFuture::from(sleep(20)).await.expect("timer");
    }
    panic!("timed out waiting for {what}");
}

fn channel(state: &str, with_stream: bool) -> PreviewChannel {
    PreviewChannel::from_bridge(&preview_bridge(state, with_stream)).expect("preview members")
}

fn calls() -> Vec<PreviewDemand> {
    let calls: Vec<serde_json::Value> =
        serde_json::from_str(&probe_calls()).expect("recorded calls");
    calls
        .into_iter()
        .map(|call| PreviewDemand {
            enabled: call["enabled"].as_bool().expect("enabled"),
            max_width: u32::try_from(call["maxWidth"].as_u64().expect("maxWidth")).expect("width"),
        })
        .collect()
}

fn surface(host: &web_sys::HtmlElement) -> web_sys::Element {
    host.query_selector("[data-remote-preview-state]")
        .expect("query")
        .expect("preview surface")
}

fn video(host: &web_sys::HtmlElement) -> web_sys::HtmlVideoElement {
    host.query_selector("video")
        .expect("query")
        .expect("video element")
        .dyn_into()
        .expect("a video element")
}

fn attribute(element: &web_sys::Element, name: &str) -> String {
    element.get_attribute(name).unwrap_or_default()
}

#[wasm_bindgen_test]
fn the_channel_reads_each_bridge_state() {
    assert_eq!(
        channel("unsupported", false).state(),
        PreviewState::Unsupported
    );
    assert_eq!(
        channel("connecting", false).state(),
        PreviewState::Connecting
    );
    assert_eq!(channel("live", true).state(), PreviewState::Live);
    assert_eq!(
        channel("live", false).state(),
        PreviewState::Connecting,
        "live without a stream cannot play"
    );
    assert_eq!(channel("paused", true).state(), PreviewState::Connecting);
    assert_eq!(channel("throw", true).state(), PreviewState::Connecting);

    let live = channel("live", true);
    let stream = live.stream().expect("stream");
    assert!(js_sys::Object::is(&stream, &probe_stream()));

    assert!(live.set_preview(PreviewDemand {
        enabled: true,
        max_width: 640,
    }));
    assert_eq!(
        calls(),
        [PreviewDemand {
            enabled: true,
            max_width: 640
        }]
    );
}

#[wasm_bindgen_test]
async fn a_contract_one_page_shows_the_update_still() {
    let owner = Owner::new();
    let context = owner.with(|| RemotePreviewContext::new(None, None));
    let host = surface_host();
    let mounted = leptos::mount::mount_to(host.clone(), move || {
        view! { <RemoteVideoPreview context=context /> }
    });
    wait_until("the still", || {
        host.text_content()
            .unwrap_or_default()
            .contains("Live preview needs an update")
    })
    .await;
    let element = surface(&host);
    assert_eq!(
        attribute(&element, "data-remote-preview-state"),
        "unsupported"
    );
    assert_eq!(attribute(&element, "data-preview-runtime"), "still");
    assert!(video(&host).src_object().is_none());
    drop(mounted);
    host.remove();
}

#[wasm_bindgen_test]
async fn a_connecting_bridge_shows_a_still_then_plays_the_live_track() {
    let owner = Owner::new();
    let context =
        owner.with(|| RemotePreviewContext::new(Some(channel("connecting", false)), None));
    let host = surface_host();
    let mounted = leptos::mount::mount_to(host.clone(), move || {
        view! { <RemoteVideoPreview context=context /> }
    });
    wait_until("the connecting note", || {
        host.text_content()
            .unwrap_or_default()
            .contains("Connecting live preview")
    })
    .await;
    assert_eq!(
        attribute(&surface(&host), "data-remote-preview-state"),
        "connecting"
    );
    assert!(video(&host).src_object().is_none());

    go_live();
    wait_until("the live state", || {
        attribute(&surface(&host), "data-remote-preview-state") == "live"
    })
    .await;
    let element = video(&host);
    let attached = element.src_object().expect("stream attached");
    assert!(js_sys::Object::is(&attached, &probe_stream()));
    assert!(element.muted());
    assert!(element.autoplay());
    assert!(element.has_attribute("playsinline"));

    wait_until("a presented video frame", || {
        attribute(&surface(&host), "data-preview-runtime") == "video"
    })
    .await;
    assert!(
        !host
            .text_content()
            .unwrap_or_default()
            .contains("Connecting live preview"),
        "the note clears once video plays"
    );
    drop(mounted);
    host.remove();
}

#[wasm_bindgen_test]
async fn a_visible_surface_reports_its_demand_until_it_unmounts() {
    let owner = Owner::new();
    let context = owner.with(|| RemotePreviewContext::new(Some(channel("live", true)), None));
    let host = surface_host();
    let mounted = leptos::mount::mount_to(host.clone(), move || {
        view! { <RemoteVideoPreview context=context /> }
    });
    let ratio = web_sys::window().expect("window").device_pixel_ratio();
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "test widths are small and positive"
    )]
    let expected_width = (320.0 * ratio).ceil() as u32;
    let on_screen = PreviewDemand {
        enabled: true,
        max_width: expected_width,
    };
    wait_until("demand from the observers", || {
        calls().last() == Some(&on_screen)
    })
    .await;
    assert_eq!(context.sent_demand(), on_screen);

    context.set_page_visible(false);
    assert_eq!(
        calls().last(),
        Some(&PreviewDemand {
            enabled: false,
            max_width: expected_width,
        })
    );
    context.set_page_visible(true);
    assert_eq!(calls().last(), Some(&on_screen));

    let before = calls().len();
    context.set_page_visible(true);
    assert_eq!(calls().len(), before, "an unchanged demand is not resent");

    drop(mounted);
    wait_until("the release", || {
        calls().last() == Some(&PreviewDemand::default())
    })
    .await;
    host.remove();
}

fn expected_width() -> u32 {
    let ratio = web_sys::window().expect("window").device_pixel_ratio();
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "test widths are small and positive"
    )]
    let width = (320.0 * ratio).ceil() as u32;
    width
}

#[wasm_bindgen_test]
async fn a_contract_two_bridge_without_video_shows_the_update_still() {
    let owner = Owner::new();
    let context =
        owner.with(|| RemotePreviewContext::new(Some(channel("unsupported", false)), None));
    let host = surface_host();
    let mounted = leptos::mount::mount_to(host.clone(), move || {
        view! { <RemoteVideoPreview context=context /> }
    });
    wait_until("the update note", || {
        host.text_content()
            .unwrap_or_default()
            .contains("Live preview needs an update")
    })
    .await;
    assert_eq!(
        attribute(&surface(&host), "data-remote-preview-state"),
        "unsupported"
    );
    assert!(video(&host).src_object().is_none());
    drop(mounted);
    host.remove();
}

#[wasm_bindgen_test]
async fn a_hidden_page_and_a_scrolled_away_surface_turn_the_demand_off() {
    let owner = Owner::new();
    let context = owner.with(|| RemotePreviewContext::new(Some(channel("live", true)), None));
    let host = surface_host();
    let mounted = leptos::mount::mount_to(host.clone(), move || {
        view! { <RemoteVideoPreview context=context /> }
    });
    let on_screen = PreviewDemand {
        enabled: true,
        max_width: expected_width(),
    };
    wait_until("demand from the observers", || {
        calls().last() == Some(&on_screen)
    })
    .await;

    set_page_hidden(true);
    assert_eq!(
        calls().last(),
        Some(&PreviewDemand {
            enabled: false,
            max_width: expected_width(),
        }),
        "a real visibilitychange stops the preview at once"
    );
    restore_page_visibility();
    assert_eq!(calls().last(), Some(&on_screen));

    move_host(&host, "5000px");
    wait_until("the off-screen report", || {
        calls().last() == Some(&PreviewDemand::default())
    })
    .await;
    move_host(&host, "0px");
    wait_until("the on-screen report", || {
        calls().last() == Some(&on_screen)
    })
    .await;
    drop(mounted);
    host.remove();
}

#[wasm_bindgen_test]
async fn a_refused_demand_is_offered_again() {
    let owner = Owner::new();
    let context = owner.with(|| RemotePreviewContext::new(Some(channel("live", true)), None));
    refuse_preview(true);
    let host = surface_host();
    let mounted = leptos::mount::mount_to(host.clone(), move || {
        view! { <RemoteVideoPreview context=context /> }
    });
    let on_screen = PreviewDemand {
        enabled: true,
        max_width: expected_width(),
    };
    wait_until("a refused call", || refused_calls() > 0).await;
    assert!(calls().is_empty());
    assert_eq!(
        context.sent_demand(),
        PreviewDemand::default(),
        "a refused demand is not recorded as sent"
    );

    refuse_preview(false);
    announce_state();
    assert_eq!(calls(), [on_screen], "the state event offers it again");
    assert_eq!(context.sent_demand(), on_screen);
    drop(mounted);
    host.remove();
}

#[wasm_bindgen_test]
async fn a_bridge_announcing_from_inside_set_preview_gets_each_demand_once() {
    let owner = Owner::new();
    let context = owner.with(|| RemotePreviewContext::new(Some(channel("live", true)), None));
    announce_from_set_preview();
    let host = surface_host();
    let mounted = leptos::mount::mount_to(host.clone(), move || {
        view! { <RemoteVideoPreview context=context /> }
    });
    let on_screen = PreviewDemand {
        enabled: true,
        max_width: expected_width(),
    };
    wait_until("the first report", || !calls().is_empty()).await;
    JsFuture::from(sleep(100)).await.expect("timer");
    assert_eq!(calls(), [on_screen], "no nested duplicate");
    drop(mounted);
    wait_until("the release", || calls().len() == 2).await;
    assert_eq!(calls().last(), Some(&PreviewDemand::default()));
    host.remove();
}
