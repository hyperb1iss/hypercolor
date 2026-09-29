//! Remote bridge contract negotiation and the preview states it reports.

use hypercolor_ui::remote_bridge::{
    CONTRACT_MAX, CONTRACT_MIN, PREVIEW_VIDEO_CONTRACT, PreviewState, negotiate_contract,
};

#[test]
fn this_build_speaks_contracts_one_and_two() {
    assert_eq!((CONTRACT_MIN, CONTRACT_MAX), (1, 2));
    assert_eq!(PREVIEW_VIDEO_CONTRACT, 2);
}

#[test]
fn negotiation_selects_the_highest_shared_contract() {
    assert_eq!(negotiate_contract(1, 1), Ok(1));
    assert_eq!(negotiate_contract(1, 2), Ok(2));
    assert_eq!(negotiate_contract(2, 2), Ok(2));
    assert_eq!(negotiate_contract(1, 7), Ok(2));
    assert_eq!(negotiate_contract(2, 7), Ok(2));
    for (minimum, maximum) in [(0, 1), (0, 0), (1, 0), (2, 1), (3, 3), (3, 9)] {
        assert_eq!(
            negotiate_contract(minimum, maximum),
            Err("remote_contract_mismatch"),
            "{minimum}..={maximum}"
        );
    }
}

#[test]
fn bridge_states_map_to_preview_states() {
    assert_eq!(
        PreviewState::from_bridge(Some("unsupported"), false),
        PreviewState::Unsupported
    );
    assert_eq!(
        PreviewState::from_bridge(Some("unsupported"), true),
        PreviewState::Unsupported
    );
    assert_eq!(
        PreviewState::from_bridge(Some("connecting"), true),
        PreviewState::Connecting
    );
    assert_eq!(
        PreviewState::from_bridge(Some("live"), true),
        PreviewState::Live
    );
    assert_eq!(
        PreviewState::from_bridge(Some("live"), false),
        PreviewState::Connecting,
        "live needs a stream to play"
    );
    for state in [None, Some(""), Some("LIVE"), Some("paused")] {
        assert_eq!(
            PreviewState::from_bridge(state, true),
            PreviewState::Connecting,
            "{state:?}"
        );
    }
    for state in [
        PreviewState::Unsupported,
        PreviewState::Connecting,
        PreviewState::Live,
    ] {
        assert_eq!(PreviewState::from_bridge(Some(state.as_str()), true), state);
    }
}
