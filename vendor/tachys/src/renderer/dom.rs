#![allow(missing_docs)]

//! See [`Renderer`](crate::renderer::Renderer) and [`Rndr`](crate::renderer::Rndr) for additional information.

use super::{CastFrom, RemoveEventHandler};
use crate::{
    dom::{document, window},
    ok_or_debug, or_debug,
    view::{Mountable, ToTemplate},
};
use rustc_hash::FxHashSet;
use std::{
    any::TypeId,
    borrow::Cow,
    cell::{LazyCell, RefCell},
};
use wasm_bindgen::{intern, prelude::{wasm_bindgen, Closure}, JsCast, JsValue};
use web_sys::{AddEventListenerOptions, Comment, HtmlTemplateElement};

/// A [`Renderer`](crate::renderer::Renderer) that uses `web-sys` to manipulate DOM elements in the browser.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Dom;

thread_local! {
    pub(crate) static GLOBAL_EVENTS: RefCell<FxHashSet<Cow<'static, str>>> = Default::default();
    pub static TEMPLATE_CACHE: RefCell<Vec<(Cow<'static, str>, web_sys::Element)>> = Default::default();
}

pub type Node = web_sys::Node;
pub type Text = web_sys::Text;
pub type Element = web_sys::Element;
pub type Placeholder = web_sys::Comment;
pub type Event = wasm_bindgen::JsValue;
pub type ClassList = web_sys::DomTokenList;
pub type CssStyleDeclaration = web_sys::CssStyleDeclaration;
pub type TemplateElement = web_sys::HtmlTemplateElement;

#[wasm_bindgen(inline_js = r#"
let hcStaticPolicy;
const hcStaticWorkerUrls = new Set();

function staticPolicy() {
    hcStaticPolicy ??= globalThis.trustedTypes.createPolicy("hc-static", {
        createHTML(value) {
            return value;
        },
        createScriptURL(value) {
            if (!hcStaticWorkerUrls.delete(value)) {
                throw new TypeError("unregistered static worker URL");
            }
            return value;
        },
    });
    return hcStaticPolicy;
}

export function setStaticInnerHtml(element, html) {
    const host = globalThis[Symbol.for("tachys.static-dom.v1")];
    if (host) {
        host.setStaticInnerHtml(element, html);
        return;
    }
    if (!globalThis.trustedTypes) {
        element.innerHTML = html;
        return;
    }

    Reflect.set(element, "innerHTML", staticPolicy().createHTML(html));
}

export function createStaticWorker(source) {
    const host = globalThis[Symbol.for("tachys.static-dom.v1")];
    if (host) return host.createStaticWorker(source);
    const blob = new Blob([source], { type: "text/javascript" });
    const url = URL.createObjectURL(blob);
    try {
        if (!globalThis.trustedTypes) {
            return [new Worker(url), url];
        }

        hcStaticWorkerUrls.add(url);
        const trustedUrl = staticPolicy().createScriptURL(url);
        return [new Worker(trustedUrl), url];
    } catch (error) {
        hcStaticWorkerUrls.delete(url);
        URL.revokeObjectURL(url);
        throw error;
    }
}
"#)]
extern "C" {
    #[wasm_bindgen(js_name = setStaticInnerHtml)]
    fn set_static_inner_html(element: &Element, html: &str);

    #[wasm_bindgen(catch, js_name = createStaticWorker)]
    fn create_static_worker_js(source: &str) -> Result<js_sys::Array, JsValue>;
}

/// Creates a classic worker from embedded static source and returns its blob
/// URL for later revocation.
pub fn create_static_worker(source: &'static str) -> Result<(JsValue, String), JsValue> {
    let result = create_static_worker_js(source)?;
    let worker = result.get(0);
    let url = result
        .get(1)
        .as_string()
        .ok_or_else(|| JsValue::from_str("static worker helper returned no URL"))?;
    Ok((worker, url))
}

fn set_borrowed_static_inner_html(
    element: &Element,
    html: &Cow<'static, str>,
    allow_owned_svg: bool,
) {
    match html {
        Cow::Borrowed(html) => set_static_inner_html(element, html),
        Cow::Owned(html) if allow_owned_svg && is_safe_static_svg_fragment(html) => {
            set_static_inner_html(element, html);
        }
        Cow::Owned(html) => element.set_inner_html(html),
    }
}

fn is_safe_static_svg_fragment(html: &str) -> bool {
    const TAGS: &[&str] = &[
        "circle", "ellipse", "g", "line", "path", "polygon", "polyline", "rect",
    ];
    const ATTRIBUTES: &[&str] = &[
        "clip-rule",
        "cx",
        "cy",
        "d",
        "fill",
        "fill-rule",
        "height",
        "opacity",
        "points",
        "r",
        "rx",
        "ry",
        "stroke",
        "stroke-linecap",
        "stroke-linejoin",
        "stroke-width",
        "transform",
        "width",
        "x",
        "x1",
        "x2",
        "y",
        "y1",
        "y2",
    ];

    let mut rest = html;
    let mut stack = Vec::new();
    let mut saw_root = false;
    while !rest.is_empty() {
        let Some(open) = rest.find('<') else {
            return rest.trim().is_empty() && stack.is_empty() && saw_root;
        };
        if !rest[..open].trim().is_empty() {
            return false;
        }
        rest = &rest[open + 1..];
        let Some(close) = rest.find('>') else {
            return false;
        };
        let mut tag = rest[..close].trim();
        rest = &rest[close + 1..];

        if let Some(name) = tag.strip_prefix('/') {
            let name = name.trim();
            if name.is_empty()
                || name.bytes().any(|byte| byte.is_ascii_whitespace())
                || stack.pop() != Some(name)
            {
                return false;
            }
            continue;
        }

        let self_closing = tag.ends_with('/');
        if self_closing {
            tag = tag[..tag.len() - 1].trim_end();
        }
        let name_end = tag
            .find(|character: char| character.is_ascii_whitespace())
            .unwrap_or(tag.len());
        let name = &tag[..name_end];
        if !TAGS.contains(&name) {
            return false;
        }
        if !saw_root {
            if name != "g" || self_closing {
                return false;
            }
            saw_root = true;
        } else if stack.is_empty() {
            return false;
        }

        let mut attributes = tag[name_end..].trim_start();
        while !attributes.is_empty() {
            let Some(equals) = attributes.find('=') else {
                return false;
            };
            let attribute = attributes[..equals].trim_end();
            if attribute.is_empty()
                || attribute.bytes().any(|byte| byte.is_ascii_whitespace())
                || !ATTRIBUTES.contains(&attribute)
            {
                return false;
            }
            attributes = attributes[equals + 1..].trim_start();
            let Some(quote) = attributes.as_bytes().first().copied() else {
                return false;
            };
            if !matches!(quote, b'\'' | b'"') {
                return false;
            }
            attributes = &attributes[1..];
            let Some(value_end) = attributes.as_bytes().iter().position(|byte| *byte == quote)
            else {
                return false;
            };
            let value = &attributes[..value_end];
            if value
                .bytes()
                .any(|byte| byte.is_ascii_control() || matches!(byte, b'<' | b'>' | b'&' | b'\\'))
                || value.to_ascii_lowercase().contains("url(")
                || (matches!(attribute, "fill" | "stroke")
                    && !value
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'#'))
            {
                return false;
            }
            attributes = attributes[value_end + 1..].trim_start();
        }

        if !self_closing {
            stack.push(name);
        }
    }
    stack.is_empty() && saw_root
}

#[cfg(test)]
mod trusted_types_tests {
    use super::is_safe_static_svg_fragment;

    #[test]
    fn accepts_icondata_svg_fragments() {
        assert!(is_safe_static_svg_fragment(
            r#"<g><path d="M3 12h18" /><circle cx="12" cy="12" r="3" /></g>"#,
        ));
    }

    #[test]
    fn rejects_active_or_malformed_svg_fragments() {
        for fragment in [
            r#"<g onload="alert(1)"></g>"#,
            r#"<g><script>alert(1)</script></g>"#,
            r#"<g><foreignObject><p>no</p></foreignObject></g>"#,
            r#"<g><use href="https://example.test/icon" /></g>"#,
            r#"<g><path style="display:none" /></g>"#,
            r#"<g><path fill="url(#paint)" /></g>"#,
            r#"<g><path fill="u\72l(https://example.test/paint)" /></g>"#,
            r#"<g><path stroke="var(--paint)" /></g>"#,
            r#"<g><svg:path d="M0 0" /></g>"#,
            r#"<g><path d="M0 &amp; 0" /></g>"#,
            r#"<g><path d="M0 0 /></g>"#,
            r#"<g><path d="M0 0"></g>"#,
            r#"<path d="M0 0" />"#,
        ] {
            assert!(!is_safe_static_svg_fragment(fragment), "{fragment}");
        }
    }
}

/// A microtask is a short function which will run after the current task has
/// completed its work and when there is no other code waiting to be run before
/// control of the execution context is returned to the browser's event loop.
///
/// Microtasks are especially useful for libraries and frameworks that need
/// to perform final cleanup or other just-before-rendering tasks.
///
/// [MDN queueMicrotask](https://developer.mozilla.org/en-US/docs/Web/API/queueMicrotask)
pub fn queue_microtask(task: impl FnOnce() + 'static) {
    use js_sys::{Function, Reflect};

    let task = Closure::once_into_js(task);
    let window = window();
    let queue_microtask =
        Reflect::get(&window, &JsValue::from_str("queueMicrotask"))
            .expect("queueMicrotask not available");
    let queue_microtask = queue_microtask.unchecked_into::<Function>();
    _ = queue_microtask.call1(&JsValue::UNDEFINED, &task);
}

fn queue(fun: Box<dyn FnOnce()>) {
    use std::cell::{Cell, RefCell};

    thread_local! {
        static PENDING: Cell<bool> = const { Cell::new(false) };
        static QUEUE: RefCell<Vec<Box<dyn FnOnce()>>> = RefCell::new(Vec::new());
    }

    QUEUE.with_borrow_mut(|q| q.push(fun));
    if !PENDING.replace(true) {
        queue_microtask(|| {
            let tasks = QUEUE.take();
            for task in tasks {
                task();
            }
            PENDING.set(false);
        })
    }
}

impl Dom {
    pub fn intern(text: &str) -> &str {
        intern(text)
    }

    pub fn create_element(tag: &str, namespace: Option<&str>) -> Element {
        if let Some(namespace) = namespace {
            document()
                .create_element_ns(
                    Some(Self::intern(namespace)),
                    Self::intern(tag),
                )
                .unwrap()
        } else {
            document().create_element(Self::intern(tag)).unwrap()
        }
    }

    #[cfg_attr(feature = "tracing", tracing::instrument(level = "trace"))]
    pub fn create_text_node(text: &str) -> Text {
        document().create_text_node(text)
    }

    pub fn create_placeholder() -> Placeholder {
        thread_local! {
            static COMMENT: LazyCell<Comment> = LazyCell::new(|| {
                document().create_comment("")
            });
        }
        COMMENT.with(|n| n.clone_node().unwrap().unchecked_into())
    }

    #[cfg_attr(feature = "tracing", tracing::instrument(level = "trace"))]
    pub fn set_text(node: &Text, text: &str) {
        node.set_node_value(Some(text));
    }

    #[cfg_attr(feature = "tracing", tracing::instrument(level = "trace"))]
    pub fn set_attribute(node: &Element, name: &str, value: &str) {
        or_debug!(node.set_attribute(name, value), node, "setAttribute");
    }

    #[cfg_attr(feature = "tracing", tracing::instrument(level = "trace"))]
    pub fn remove_attribute(node: &Element, name: &str) {
        or_debug!(node.remove_attribute(name), node, "removeAttribute");
    }

    #[cfg_attr(feature = "tracing", tracing::instrument(level = "trace"))]
    pub fn insert_node(
        parent: &Element,
        new_child: &Node,
        anchor: Option<&Node>,
    ) {
        ok_or_debug!(
            parent.insert_before(new_child, anchor),
            parent,
            "insertNode"
        );
    }

    #[cfg_attr(feature = "tracing", tracing::instrument(level = "trace"))]
    pub fn try_insert_node(
        parent: &Element,
        new_child: &Node,
        anchor: Option<&Node>,
    ) -> bool {
        parent.insert_before(new_child, anchor).is_ok()
    }

    #[cfg_attr(feature = "tracing", tracing::instrument(level = "trace"))]
    pub fn remove_node(parent: &Element, child: &Node) -> Option<Node> {
        ok_or_debug!(parent.remove_child(child), parent, "removeNode")
    }

    #[cfg_attr(feature = "tracing", tracing::instrument(level = "trace"))]
    pub fn remove(node: &Node) {
        node.unchecked_ref::<Element>().remove();
    }

    pub fn get_parent(node: &Node) -> Option<Node> {
        node.parent_node()
    }

    pub fn first_child(node: &Node) -> Option<Node> {
        #[cfg(debug_assertions)]
        {
            let node = node.first_child();
            // if it's a comment node that starts with hot-reload, it's a marker that should be
            // ignored
            if let Some(node) = node.as_ref() {
                if node.node_type() == 8
                    && node
                        .text_content()
                        .unwrap_or_default()
                        .starts_with("hot-reload")
                {
                    return Self::next_sibling(node);
                }
            }

            node
        }
        #[cfg(not(debug_assertions))]
        {
            node.first_child()
        }
    }

    pub fn next_sibling(node: &Node) -> Option<Node> {
        #[cfg(debug_assertions)]
        {
            let node = node.next_sibling();
            // if it's a comment node that starts with hot-reload, it's a marker that should be
            // ignored
            if let Some(node) = node.as_ref() {
                if node.node_type() == 8
                    && node
                        .text_content()
                        .unwrap_or_default()
                        .starts_with("hot-reload")
                {
                    return Self::next_sibling(node);
                }
            }

            node
        }
        #[cfg(not(debug_assertions))]
        {
            node.next_sibling()
        }
    }

    pub fn log_node(node: &Node) {
        web_sys::console::log_1(node);
    }

    #[cfg_attr(feature = "tracing", tracing::instrument(level = "trace"))]
    pub fn clear_children(parent: &Element) {
        parent.set_text_content(Some(""));
    }

    /// Mounts the new child before the marker as its sibling.
    ///
    /// ## Panics
    /// The default implementation panics if `before` does not have a parent [`crate::renderer::types::Element`].
    pub fn mount_before<M>(new_child: &mut M, before: &Node)
    where
        M: Mountable,
    {
        let parent = Element::cast_from(
            Self::get_parent(before).expect("could not find parent element"),
        )
        .expect("placeholder parent should be Element");
        new_child.mount(&parent, Some(before));
    }

    /// Tries to mount the new child before the marker as its sibling.
    ///
    /// Returns `false` if the child did not have a valid parent.
    #[track_caller]
    pub fn try_mount_before<M>(new_child: &mut M, before: &Node) -> bool
    where
        M: Mountable,
    {
        if let Some(parent) =
            Self::get_parent(before).and_then(Element::cast_from)
        {
            new_child.mount(&parent, Some(before));
            true
        } else {
            false
        }
    }

    pub fn set_property_or_value(el: &Element, key: &str, value: &JsValue) {
        if key == "value" {
            queue(Box::new({
                let el = el.clone();
                let value = value.clone();
                move || {
                    Self::set_property(&el, "value", &value);
                }
            }))
        } else {
            Self::set_property(el, key, value);
        }
    }

    pub fn set_property(el: &Element, key: &str, value: &JsValue) {
        or_debug!(
            js_sys::Reflect::set(
                el,
                &wasm_bindgen::JsValue::from_str(key),
                value,
            ),
            el,
            "setProperty"
        );
    }

    pub fn add_event_listener(
        el: &Element,
        name: &str,
        cb: Box<dyn FnMut(Event)>,
    ) -> RemoveEventHandler<Element> {
        let cb = wasm_bindgen::closure::Closure::wrap(cb);
        let name = intern(name);
        or_debug!(
            el.add_event_listener_with_callback(
                name,
                cb.as_ref().unchecked_ref()
            ),
            el,
            "addEventListener"
        );

        // return the remover
        RemoveEventHandler::new({
            let name = name.to_owned();
            let el = el.clone();
            // safe to construct this here, because it will only run in the browser
            // so it will always be accessed or dropped from the main thread
            let cb = send_wrapper::SendWrapper::new(move || {
                or_debug!(
                    el.remove_event_listener_with_callback(
                        intern(&name),
                        cb.as_ref().unchecked_ref()
                    ),
                    &el,
                    "removeEventListener"
                )
            });
            move || cb()
        })
    }

    pub fn add_event_listener_use_capture(
        el: &Element,
        name: &str,
        cb: Box<dyn FnMut(Event)>,
    ) -> RemoveEventHandler<Element> {
        let cb = wasm_bindgen::closure::Closure::wrap(cb);
        let name = intern(name);
        let options = AddEventListenerOptions::new();
        options.set_capture(true);
        or_debug!(
            el.add_event_listener_with_callback_and_add_event_listener_options(
                name,
                cb.as_ref().unchecked_ref(),
                &options
            ),
            el,
            "addEventListenerUseCapture"
        );

        // return the remover
        RemoveEventHandler::new({
            let name = name.to_owned();
            let el = el.clone();
            // safe to construct this here, because it will only run in the browser
            // so it will always be accessed or dropped from the main thread
            let cb = send_wrapper::SendWrapper::new(move || {
                or_debug!(
                    el.remove_event_listener_with_callback_and_bool(
                        intern(&name),
                        cb.as_ref().unchecked_ref(),
                        true
                    ),
                    &el,
                    "removeEventListener"
                )
            });
            move || cb()
        })
    }

    pub fn event_target<T>(ev: &Event) -> T
    where
        T: CastFrom<Element>,
    {
        let el = ev
            .unchecked_ref::<web_sys::Event>()
            .target()
            .expect("event.target not found")
            .unchecked_into::<Element>();
        T::cast_from(el).expect("incorrect element type")
    }

    pub fn add_event_listener_delegated(
        el: &Element,
        name: Cow<'static, str>,
        delegation_key: Cow<'static, str>,
        cb: Box<dyn FnMut(Event)>,
    ) -> RemoveEventHandler<Element> {
        let cb = Closure::wrap(cb);
        let key = intern(&delegation_key);
        or_debug!(
            js_sys::Reflect::set(el, &JsValue::from_str(key), cb.as_ref()),
            el,
            "set property"
        );

        GLOBAL_EVENTS.with_borrow_mut(|events| {
            if !events.contains(&name) {
                // create global handler
                let key = JsValue::from_str(key);
                let handler = move |ev: web_sys::Event| {
                    let target = ev.target();
                    let node = ev.composed_path().get(0);
                    let mut node = if node.is_undefined() || node.is_null() {
                        JsValue::from(target)
                    } else {
                        node
                    };

                    // TODO reverse Shadow DOM retargetting
                    // TODO simulate currentTarget

                    while !node.is_null() {
                        let node_is_disabled = js_sys::Reflect::get(
                            &node,
                            &JsValue::from_str("disabled"),
                        )
                        .unwrap()
                        .is_truthy();
                        if !node_is_disabled {
                            let maybe_handler =
                                js_sys::Reflect::get(&node, &key).unwrap();
                            if !maybe_handler.is_undefined() {
                                let f = maybe_handler
                                    .unchecked_ref::<js_sys::Function>();
                                let _ = f.call1(&node, &ev);

                                if ev.cancel_bubble() {
                                    return;
                                }
                            }
                        }

                        // navigate up tree
                        if let Some(parent) =
                            node.unchecked_ref::<web_sys::Node>().parent_node()
                        {
                            node = parent.into()
                        } else if let Some(root) =
                            node.dyn_ref::<web_sys::ShadowRoot>()
                        {
                            node = root.host().unchecked_into();
                        } else {
                            node = JsValue::null()
                        }
                    }
                };

                let handler =
                    Box::new(handler) as Box<dyn FnMut(web_sys::Event)>;
                let handler = Closure::wrap(handler).into_js_value();
                window()
                    .add_event_listener_with_callback(
                        &name,
                        handler.unchecked_ref(),
                    )
                    .unwrap();

                // register that we've created handler
                events.insert(name);
            }
        });

        // return the remover
        RemoveEventHandler::new({
            let key = key.to_owned();
            let el = el.clone();
            // safe to construct this here, because it will only run in the browser
            // so it will always be accessed or dropped from the main thread
            let el_cb = send_wrapper::SendWrapper::new((el, cb));
            move || {
                let (el, cb) = el_cb.take();
                drop(cb);
                or_debug!(
                    js_sys::Reflect::delete_property(
                        &el,
                        &JsValue::from_str(&key)
                    ),
                    &el,
                    "delete property"
                );
            }
        })
    }

    pub fn class_list(el: &Element) -> ClassList {
        el.class_list()
    }

    pub fn add_class(list: &ClassList, name: &str) {
        or_debug!(list.add_1(name), list.unchecked_ref(), "add()");
    }

    pub fn remove_class(list: &ClassList, name: &str) {
        or_debug!(list.remove_1(name), list.unchecked_ref(), "remove()");
    }

    pub fn style(el: &Element) -> CssStyleDeclaration {
        el.unchecked_ref::<web_sys::HtmlElement>().style()
    }

    pub fn set_css_property(
        style: &CssStyleDeclaration,
        name: &str,
        value: &str,
    ) {
        or_debug!(
            style.set_property(name, value),
            style.unchecked_ref(),
            "setProperty"
        );
    }

    pub fn remove_css_property(style: &CssStyleDeclaration, name: &str) {
        or_debug!(
            style.remove_property(name),
            style.unchecked_ref(),
            "removeProperty"
        );
    }

    pub fn set_inner_html(el: &Element, html: &str) {
        el.set_inner_html(html);
    }

    pub fn get_template<V>() -> TemplateElement
    where
        V: ToTemplate + 'static,
    {
        thread_local! {
            static TEMPLATE_ELEMENT: LazyCell<HtmlTemplateElement> =
                LazyCell::new(|| document().create_element(Dom::intern("template")).unwrap().unchecked_into());
            static TEMPLATES: RefCell<Vec<(TypeId, HtmlTemplateElement)>> = Default::default();
        }

        TEMPLATES.with_borrow_mut(|t| {
            let id = TypeId::of::<V>();
            t.iter()
                .find_map(|entry| (entry.0 == id).then(|| entry.1.clone()))
                .unwrap_or_else(|| {
                    let tpl = TEMPLATE_ELEMENT.with(|t| {
                        t.clone_node()
                            .unwrap()
                            .unchecked_into::<HtmlTemplateElement>()
                    });
                    let mut buf = String::new();
                    V::to_template(
                        &mut buf,
                        &mut String::new(),
                        &mut String::new(),
                        &mut String::new(),
                        &mut Default::default(),
                    );
                    set_static_inner_html(tpl.unchecked_ref(), &buf);
                    t.push((id, tpl.clone()));
                    tpl
                })
        })
    }

    pub fn clone_template(tpl: &TemplateElement) -> Element {
        tpl.content()
            .clone_node_with_deep(true)
            .unwrap()
            .unchecked_into()
    }

    pub fn create_element_from_html(html: Cow<'static, str>) -> Element {
        let tpl = TEMPLATE_CACHE.with_borrow_mut(|cache| {
            if let Some(tpl_content) = cache.iter().find_map(|(key, tpl)| {
                (html == *key)
                    .then_some(Self::clone_template(tpl.unchecked_ref()))
            }) {
                tpl_content
            } else {
                let tpl = document()
                    .create_element(Self::intern("template"))
                    .unwrap();
                set_borrowed_static_inner_html(&tpl, &html, false);
                let tpl_content = Self::clone_template(tpl.unchecked_ref());
                cache.push((html, tpl));
                tpl_content
            }
        });
        tpl.first_element_child().unwrap_or(tpl)
    }

    pub fn create_svg_element_from_html(html: Cow<'static, str>) -> Element {
        let tpl = TEMPLATE_CACHE.with_borrow_mut(|cache| {
            if let Some(tpl_content) = cache.iter().find_map(|(key, tpl)| {
                (html == *key)
                    .then_some(Self::clone_template(tpl.unchecked_ref()))
            }) {
                tpl_content
            } else {
                let tpl = document()
                    .create_element(Self::intern("template"))
                    .unwrap();
                let svg = document()
                    .create_element_ns(
                        Some(Self::intern("http://www.w3.org/2000/svg")),
                        Self::intern("svg"),
                    )
                    .unwrap();
                let g = document()
                    .create_element_ns(
                        Some(Self::intern("http://www.w3.org/2000/svg")),
                        Self::intern("g"),
                    )
                    .unwrap();
                set_borrowed_static_inner_html(&g, &html, true);
                svg.append_child(&g).unwrap();
                tpl.unchecked_ref::<TemplateElement>()
                    .content()
                    .append_child(&svg)
                    .unwrap();
                let tpl_content = Self::clone_template(tpl.unchecked_ref());
                cache.push((html, tpl));
                tpl_content
            }
        });

        let svg = tpl.first_element_child().unwrap();
        svg.first_element_child().unwrap_or(svg)
    }
}

impl Mountable for Node {
    fn unmount(&mut self) {
        todo!()
    }

    fn mount(&mut self, parent: &Element, marker: Option<&Node>) {
        Dom::insert_node(parent, self, marker);
    }

    fn try_mount(&mut self, parent: &Element, marker: Option<&Node>) -> bool {
        Dom::try_insert_node(parent, self, marker)
    }

    fn insert_before_this(&self, child: &mut dyn Mountable) -> bool {
        let parent = Dom::get_parent(self).and_then(Element::cast_from);
        if let Some(parent) = parent {
            child.mount(&parent, Some(self));
            return true;
        }
        false
    }

    fn elements(&self) -> Vec<crate::renderer::types::Element> {
        vec![]
    }
}

impl Mountable for Text {
    fn unmount(&mut self) {
        self.remove();
    }

    fn mount(&mut self, parent: &Element, marker: Option<&Node>) {
        Dom::insert_node(parent, self, marker);
    }

    fn try_mount(&mut self, parent: &Element, marker: Option<&Node>) -> bool {
        Dom::try_insert_node(parent, self, marker)
    }

    fn insert_before_this(&self, child: &mut dyn Mountable) -> bool {
        let parent =
            Dom::get_parent(self.as_ref()).and_then(Element::cast_from);
        if let Some(parent) = parent {
            child.mount(&parent, Some(self));
            return true;
        }
        false
    }

    fn elements(&self) -> Vec<crate::renderer::types::Element> {
        vec![]
    }
}

impl Mountable for Comment {
    fn unmount(&mut self) {
        self.remove();
    }

    fn mount(&mut self, parent: &Element, marker: Option<&Node>) {
        Dom::insert_node(parent, self, marker);
    }

    fn try_mount(&mut self, parent: &Element, marker: Option<&Node>) -> bool {
        Dom::try_insert_node(parent, self, marker)
    }

    fn insert_before_this(&self, child: &mut dyn Mountable) -> bool {
        let parent =
            Dom::get_parent(self.as_ref()).and_then(Element::cast_from);
        if let Some(parent) = parent {
            child.mount(&parent, Some(self));
            return true;
        }
        false
    }

    fn elements(&self) -> Vec<crate::renderer::types::Element> {
        vec![]
    }
}

impl Mountable for Element {
    fn unmount(&mut self) {
        self.remove();
    }

    fn mount(&mut self, parent: &Element, marker: Option<&Node>) {
        Dom::insert_node(parent, self, marker);
    }

    fn insert_before_this(&self, child: &mut dyn Mountable) -> bool {
        let parent =
            Dom::get_parent(self.as_ref()).and_then(Element::cast_from);
        if let Some(parent) = parent {
            child.mount(&parent, Some(self));
            return true;
        }
        false
    }

    fn elements(&self) -> Vec<crate::renderer::types::Element> {
        vec![self.clone()]
    }
}

impl CastFrom<Node> for Text {
    fn cast_from(node: Node) -> Option<Text> {
        node.clone().dyn_into().ok()
    }
}

impl CastFrom<Node> for Comment {
    fn cast_from(node: Node) -> Option<Comment> {
        node.clone().dyn_into().ok()
    }
}

impl CastFrom<Node> for Element {
    fn cast_from(node: Node) -> Option<Element> {
        node.clone().dyn_into().ok()
    }
}

impl<T> CastFrom<JsValue> for T
where
    T: JsCast,
{
    fn cast_from(source: JsValue) -> Option<Self> {
        source.dyn_into::<T>().ok()
    }
}

impl<T> CastFrom<Element> for T
where
    T: JsCast,
{
    fn cast_from(source: Element) -> Option<Self> {
        source.dyn_into::<T>().ok()
    }
}
