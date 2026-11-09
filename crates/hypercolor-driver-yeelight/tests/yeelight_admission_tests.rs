//! Per-scan admission of search replies (Spec 94 §3.3).

use std::net::{IpAddr, Ipv4Addr, SocketAddr};

use hypercolor_driver_api::CoverageGap;
use hypercolor_driver_support::discovery::{Admission, CandidateAdmission, KnownTarget};
use hypercolor_driver_yeelight::protocol::SearchReply;
use hypercolor_driver_yeelight::scanner::{YeelightDeviceProfile, offer_reply};

type SearchAdmission = CandidateAdmission<IpAddr, YeelightDeviceProfile>;

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
    let mut admission = SearchAdmission::new(256, []);
    assert_eq!(
        offer_reply(&mut admission, source(1), reply(source(1), "0x1")),
        Some(Admission::Added)
    );
    for host in 2..5_000 {
        assert_eq!(
            offer_reply(&mut admission, source(host), reply(source(host), "0x1")),
            None
        );
    }
    assert_eq!(admission.len(), 1);
}

#[test]
fn each_address_reports_only_its_most_recent_id() {
    let mut admission = SearchAdmission::new(256, []);
    assert_eq!(
        offer_reply(&mut admission, source(1), reply(source(1), "0xa")),
        Some(Admission::Added)
    );
    for id in ["0xb", "0xc", "0xd"] {
        assert_eq!(
            offer_reply(&mut admission, source(1), reply(source(1), id)),
            Some(Admission::Updated)
        );
    }
    let outcome = admission.finish();
    assert_eq!(outcome.found.len(), 1);
    assert_eq!(outcome.found[0].reply.id, "0xd");
    assert!(outcome.gaps.is_empty());
}

#[test]
fn admission_stops_at_the_limit_without_tracking_more() {
    let mut admission = SearchAdmission::new(2, []);
    assert_eq!(
        offer_reply(&mut admission, source(1), reply(source(1), "0x1")),
        Some(Admission::Added)
    );
    assert_eq!(
        offer_reply(&mut admission, source(2), reply(source(2), "0x2")),
        Some(Admission::Added)
    );
    for host in 3..1_000 {
        let id = format!("0x{host:x}");
        assert_eq!(
            offer_reply(&mut admission, source(host), reply(source(host), &id)),
            Some(Admission::Full)
        );
    }
    assert_eq!(admission.len(), 2);
    // A tracked address can still update its id at the limit.
    assert_eq!(
        offer_reply(&mut admission, source(2), reply(source(2), "0x99")),
        Some(Admission::Updated)
    );
    assert_eq!(
        admission.finish().gaps,
        vec![CoverageGap::AdmissionFull { refused: 997 }]
    );
}

#[test]
fn a_flood_cannot_crowd_out_a_tracked_light() {
    let tracked = source(9_999);
    let mut admission = SearchAdmission::new(2, [KnownTarget::Address(tracked.ip())]);
    for host in 1..1_000 {
        let id = format!("0x{host:x}");
        offer_reply(&mut admission, source(host), reply(source(host), &id));
    }
    assert_eq!(
        offer_reply(&mut admission, tracked, reply(tracked, "0xbeef")),
        Some(Admission::Added)
    );
    assert_eq!(admission.finish().found[0].reply.id, "0xbeef");
}

#[test]
fn replies_naming_another_address_are_refused() {
    let mut admission = SearchAdmission::new(256, []);
    assert_eq!(
        offer_reply(&mut admission, source(1), reply(source(7), "0x1")),
        None
    );
    assert!(admission.is_empty());
}
