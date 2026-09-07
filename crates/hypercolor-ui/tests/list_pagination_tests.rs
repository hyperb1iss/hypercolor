use hypercolor_ui::api::client::{LIST_PAGE_LIMIT, paged_list_url};

#[test]
fn page_url_requests_the_route_ceiling() {
    assert_eq!(LIST_PAGE_LIMIT, 200);
    assert_eq!(
        paged_list_url("/api/v1/layouts", 0),
        "/api/v1/layouts?limit=200&offset=0"
    );
}

#[test]
fn page_url_appends_to_an_existing_query_string() {
    assert_eq!(
        paged_list_url("/api/v1/devices?include=attachments", 200),
        "/api/v1/devices?include=attachments&limit=200&offset=200"
    );
}

#[test]
fn page_url_advances_the_offset_between_pages() {
    assert_eq!(
        paged_list_url("/api/v1/attachments/templates?category=fan", 400),
        "/api/v1/attachments/templates?category=fan&limit=200&offset=400"
    );
}
