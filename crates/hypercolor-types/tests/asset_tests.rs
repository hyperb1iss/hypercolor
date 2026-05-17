use std::collections::HashSet;
use std::str::FromStr;

use hypercolor_types::asset::AssetId;
use uuid::Uuid;

#[test]
fn asset_id_new_creates_unique_ids() {
    let a = AssetId::new();
    let b = AssetId::new();

    assert_ne!(a, b);
}

#[test]
fn asset_id_round_trips_through_uuid_and_display() {
    let uuid = Uuid::now_v7();
    let id = AssetId::from_uuid(uuid);

    assert_eq!(id.as_uuid(), uuid);
    assert_eq!(id.to_string(), uuid.to_string());
    assert_eq!(
        AssetId::from_str(&id.to_string()).expect("valid asset id"),
        id
    );
}

#[test]
fn asset_id_hashes_in_collections() {
    let id = AssetId::new();
    let mut ids = HashSet::new();

    assert!(ids.insert(id));
    assert!(!ids.insert(id));
}
