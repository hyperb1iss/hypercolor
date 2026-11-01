//! The phone Effects view and its place in the URL.

use hypercolor_ui::pages::effects::phone_view::{PhoneView, view_for_key};

#[test]
fn only_controls_in_the_query_opens_controls() {
    assert_eq!(PhoneView::from_query(Some("controls")), PhoneView::Controls);
    assert_eq!(PhoneView::from_query(None), PhoneView::Browse);
    assert_eq!(PhoneView::from_query(Some("")), PhoneView::Browse);
    assert_eq!(PhoneView::from_query(Some("grid")), PhoneView::Browse);
}

#[test]
fn each_view_has_its_own_address() {
    // Distinct addresses are what make a link to Controls land from
    // Browse: navigating there always changes the URL.
    assert_ne!(PhoneView::Browse.href(), PhoneView::Controls.href());
    for view in [PhoneView::Browse, PhoneView::Controls] {
        let value = view.href().split_once("?view=").map(|(_, value)| value);
        assert_eq!(PhoneView::from_query(value), view, "{view:?}");
    }
}

#[test]
fn tab_keys_move_between_the_two_views() {
    assert_eq!(
        view_for_key(PhoneView::Browse, "ArrowRight"),
        Some(PhoneView::Controls)
    );
    assert_eq!(
        view_for_key(PhoneView::Controls, "ArrowRight"),
        Some(PhoneView::Browse)
    );
    assert_eq!(
        view_for_key(PhoneView::Controls, "ArrowLeft"),
        Some(PhoneView::Browse)
    );
    assert_eq!(
        view_for_key(PhoneView::Controls, "Home"),
        Some(PhoneView::Browse)
    );
    assert_eq!(
        view_for_key(PhoneView::Browse, "End"),
        Some(PhoneView::Controls)
    );
    assert_eq!(view_for_key(PhoneView::Browse, "Enter"), None);
    assert_eq!(view_for_key(PhoneView::Browse, "ArrowDown"), None);
}

#[test]
fn each_tab_has_its_own_id() {
    assert_ne!(PhoneView::Browse.tab_id(), PhoneView::Controls.tab_id());
}
