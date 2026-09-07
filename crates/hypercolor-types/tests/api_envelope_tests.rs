//! Wire-shape coverage for the canonical envelope conventions
//! (Spec 76 §4.3).

use hypercolor_types::api::envelope::{
    ApiErrorBody, ApiErrorDetail, ApiResponse, ListResponse, PageInfo, ResponseMeta,
};

fn meta() -> ResponseMeta {
    ResponseMeta {
        api_version: "1.0".to_owned(),
        request_id: "req_test".to_owned(),
        timestamp: "2026-08-16T00:00:00Z".to_owned(),
    }
}

#[test]
fn success_envelope_has_the_canonical_field_set() {
    let body = ApiResponse {
        data: serde_json::json!({"value": 1}),
        meta: meta(),
    };
    let json = serde_json::to_value(&body).expect("serializes");
    let keys: Vec<&str> = json
        .as_object()
        .expect("object")
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(keys, ["data", "meta"]);
    let meta_keys: Vec<&str> = json["meta"]
        .as_object()
        .expect("meta object")
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(meta_keys, ["api_version", "request_id", "timestamp"]);
}

#[test]
fn success_envelope_rejects_unknown_top_level_and_meta_fields() {
    let unknown_top_level = serde_json::json!({
        "data": {"value": 1},
        "meta": serde_json::to_value(meta()).expect("meta serializes"),
        "legacy": true,
    });
    assert!(serde_json::from_value::<ApiResponse<serde_json::Value>>(unknown_top_level).is_err());

    let unknown_meta = serde_json::json!({
        "data": {"value": 1},
        "meta": {
            "api_version": "1.0",
            "request_id": "req_test",
            "timestamp": "2026-08-16T00:00:00Z",
            "legacy": true,
        },
    });
    assert!(serde_json::from_value::<ApiResponse<serde_json::Value>>(unknown_meta).is_err());
}

#[test]
fn error_envelope_shape_is_code_message_details() {
    let body = ApiErrorBody {
        error: ApiErrorDetail {
            code: "not_found".to_owned(),
            message: "scene not found".to_owned(),
            details: None,
        },
        meta: meta(),
    };
    let json = serde_json::to_value(&body).expect("serializes");
    assert_eq!(json["error"]["code"], "not_found");
    assert!(
        !json["error"]
            .as_object()
            .expect("error object")
            .contains_key("details"),
        "None details must be omitted, not emitted as null"
    );
    let with_details = ApiErrorBody {
        error: ApiErrorDetail {
            code: "precondition_failed".to_owned(),
            message: "version mismatch".to_owned(),
            details: Some(serde_json::json!({"current": 4})),
        },
        meta: meta(),
    };
    let json = serde_json::to_value(&with_details).expect("serializes");
    assert_eq!(json["error"]["details"]["current"], 4);
}

#[test]
fn complete_lists_omit_the_page_block_honestly() {
    let complete: ListResponse<u32> = ListResponse {
        items: vec![1, 2, 3],
        total: 3,
        page: None,
    };
    let json = serde_json::to_value(&complete).expect("serializes");
    assert!(
        !json.as_object().expect("object").contains_key("page"),
        "a complete response must not fabricate paging"
    );

    let paged: ListResponse<u32> = ListResponse {
        items: vec![1, 2],
        total: 10,
        page: Some(PageInfo {
            offset: 0,
            limit: 2,
            has_more: true,
        }),
    };
    let json = serde_json::to_value(&paged).expect("serializes");
    assert_eq!(json["page"]["has_more"], true);
    let round: ListResponse<u32> = serde_json::from_value(json).expect("deserializes");
    assert_eq!(round, paged);
}
