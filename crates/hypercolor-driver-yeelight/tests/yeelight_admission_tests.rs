//! Per-scan admission of search replies (Spec 94 §3.3).

use std::net::{IpAddr, Ipv4Addr, SocketAddr};

use hypercolor_driver_yeelight::protocol::SearchReply;
use hypercolor_driver_yeelight::scanner::{Offer, SearchAdmission};

fn source(host: u32) -> SocketAddr {
    SocketAddr::new(IpAddr::V4(Ipv4Addr::from(0x0a00_0000 + host)), 1982)
}

fn reply(at: SocketAddr, id: &str) -> SearchReply {
    SearchReply {
        location: SocketAddr::new(at.ip(), 55443),
        id: id.to_owned(),
        model: Some("color".to_owned()),
        fw_ver: None,
        support: vec!["set_scene".to_owned(), "set_rgb".to_owned()],
        power: None,
        bright: None,
        name: None,
    }
}

#[test]
fn spoofed_sources_repeating_one_id_never_grow_state() {
    let mut admission = SearchAdmission::new(256);
    assert_eq!(
        admission.offer(source(1), reply(source(1), "0x1")),
        Offer::Admitted
    );
    for host in 2..5_000 {
        assert_eq!(
            admission.offer(source(host), reply(source(host), "0x1")),
            Offer::Duplicate
        );
    }
    assert_eq!(admission.len(), 1);
}

#[test]
fn each_address_reports_only_its_most_recent_id() {
    let mut admission = SearchAdmission::new(256);
    assert_eq!(
        admission.offer(source(1), reply(source(1), "0xa")),
        Offer::Admitted
    );
    for id in ["0xb", "0xc", "0xd"] {
        assert_eq!(
            admission.offer(source(1), reply(source(1), id)),
            Offer::Replaced
        );
    }
    let profiles = admission.into_profiles();
    assert_eq!(profiles.len(), 1);
    assert_eq!(profiles[0].reply.id, "0xd");
}

#[test]
fn admission_stops_at_the_limit_without_tracking_more() {
    let mut admission = SearchAdmission::new(2);
    assert_eq!(
        admission.offer(source(1), reply(source(1), "0x1")),
        Offer::Admitted
    );
    assert_eq!(
        admission.offer(source(2), reply(source(2), "0x2")),
        Offer::Admitted
    );
    for host in 3..1_000 {
        let id = format!("0x{host:x}");
        assert_eq!(
            admission.offer(source(host), reply(source(host), &id)),
            Offer::Full
        );
    }
    assert_eq!(admission.len(), 2);
    // A tracked address can still update its id at the limit.
    assert_eq!(
        admission.offer(source(2), reply(source(2), "0x99")),
        Offer::Replaced
    );
}

#[test]
fn replies_naming_another_address_are_foreign() {
    let mut admission = SearchAdmission::new(256);
    assert_eq!(
        admission.offer(source(1), reply(source(7), "0x1")),
        Offer::Foreign
    );
    assert!(admission.is_empty());
}
