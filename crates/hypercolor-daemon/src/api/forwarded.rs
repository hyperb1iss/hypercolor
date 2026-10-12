//! What a loopback reverse proxy says about the client it relays.
//!
//! Forwarding headers are claims, and a proxy writes only its own part of
//! them: it appends the address it accepted to `X-Forwarded-For` or
//! `Forwarded` and passes every earlier entry through exactly as the
//! client sent it. Only the last entry is the proxy's word, so that is
//! the only one read here.
//!
//! The daemon also cannot tell which header its proxy owns. A proxy that
//! sets `X-Real-IP` forwards a client's own `X-Forwarded-For` untouched,
//! and one that appends to `X-Forwarded-For` forwards a client's
//! `X-Real-IP`. Every header's claim is therefore kept, and callers that
//! judge an address judge all of them.

use std::net::{IpAddr, Ipv6Addr};

use axum::http::{HeaderMap, HeaderValue};

const X_FORWARDED_FOR: &str = "x-forwarded-for";
const X_REAL_IP: &str = "x-real-ip";
const FORWARDED: &str = "forwarded";

/// Headers whose presence on a loopback connection means a proxy relayed
/// a client. `X-Forwarded-Host` and `X-Forwarded-Proto` name no client,
/// so they mark nothing: the UI dev server's proxy sends the former for
/// the local browser it relays.
const FORWARDING_HEADERS: [&str; 3] = [FORWARDED, X_FORWARDED_FOR, X_REAL_IP];

/// Whether the request carries a header naming the client a proxy relays.
#[must_use]
pub(crate) fn forwarding_present(headers: &HeaderMap) -> bool {
    FORWARDING_HEADERS
        .iter()
        .any(|name| headers.contains_key(*name))
}

/// The client address each address-bearing header claims for the hop
/// nearest the daemon, in the order `X-Forwarded-For`, `X-Real-IP`,
/// `Forwarded`.
///
/// Absent headers contribute nothing. A header that is present but
/// names no readable address (not UTF-8, empty, `unknown`, or an
/// obfuscated `Forwarded` node) claims `None`, so a caller that needs an
/// address can refuse rather than guess.
#[must_use]
pub(crate) fn nearest_hop_claims(headers: &HeaderMap) -> Vec<Option<IpAddr>> {
    let mut claims = Vec::with_capacity(3);
    if let Some(value) = last_header(headers, X_FORWARDED_FOR) {
        claims.push(
            value
                .to_str()
                .ok()
                .and_then(|value| parse_node(last_list_element(value))),
        );
    }
    if let Some(value) = last_header(headers, X_REAL_IP) {
        claims.push(value.to_str().ok().and_then(parse_node));
    }
    if let Some(value) = last_header(headers, FORWARDED) {
        claims.push(value.to_str().ok().and_then(last_forwarded_for));
    }
    claims
}

/// The last instance of `name`, the one the nearest hop wrote.
fn last_header<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a HeaderValue> {
    headers.get_all(name).iter().next_back()
}

/// The last entry of `X-Forwarded-For`, which has no quoting.
fn last_list_element(value: &str) -> &str {
    value.rsplit(',').next().unwrap_or(value).trim()
}

/// The `for=` node of a `Forwarded` value's last element (RFC 7239).
///
/// Elements and pairs are split outside quoted strings, so a quoted
/// parameter cannot smuggle in a second element or a `for=` of its own.
/// Unbalanced quotes, a pair without `=`, or two `for=` pairs in the
/// element name no address.
fn last_forwarded_for(value: &str) -> Option<IpAddr> {
    let elements = split_unquoted(value, ',')?;
    let element = elements.last()?;
    let mut node = None;
    for pair in split_unquoted(element, ';')? {
        if pair.trim().is_empty() {
            continue;
        }
        let (key, value) = pair.split_once('=')?;
        if key.trim().eq_ignore_ascii_case("for") {
            if node.is_some() {
                return None;
            }
            node = Some(value);
        }
    }
    parse_node(node?)
}

/// Split on `separator` outside quoted strings, honoring backslash
/// escapes inside them. `None` when a quoted string never closes.
fn split_unquoted(value: &str, separator: char) -> Option<Vec<&str>> {
    let mut parts = Vec::new();
    let mut start = 0;
    let mut quoted = false;
    let mut escaped = false;
    for (index, ch) in value.char_indices() {
        if escaped {
            escaped = false;
        } else if quoted && ch == '\\' {
            escaped = true;
        } else if ch == '"' {
            quoted = !quoted;
        } else if !quoted && ch == separator {
            parts.push(&value[start..index]);
            start = index + ch.len_utf8();
        }
    }
    if quoted {
        return None;
    }
    parts.push(&value[start..]);
    Some(parts)
}

/// One node: an address, optionally quoted, with an optional port, and
/// with IPv6 bracketed whenever a port follows.
fn parse_node(raw: &str) -> Option<IpAddr> {
    let node = raw.trim();
    let node = node
        .strip_prefix('"')
        .and_then(|inner| inner.strip_suffix('"'))
        .unwrap_or(node);
    if let Ok(address) = node.parse::<IpAddr>() {
        return Some(address);
    }
    if let Some(bracketed) = node.strip_prefix('[') {
        let (inner, rest) = bracketed.split_once(']')?;
        if !(rest.is_empty() || is_port_suffix(rest)) {
            return None;
        }
        return inner.parse::<Ipv6Addr>().ok().map(IpAddr::V6);
    }
    let (address, _) = node.split_once(':')?;
    if !is_port_suffix(&node[address.len()..]) {
        return None;
    }
    address.parse::<IpAddr>().ok().filter(IpAddr::is_ipv4)
}

fn is_port_suffix(rest: &str) -> bool {
    rest.strip_prefix(':')
        .is_some_and(|port| !port.is_empty() && port.bytes().all(|byte| byte.is_ascii_digit()))
}

#[cfg(test)]
mod tests {
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

    use axum::http::{HeaderMap, HeaderValue};

    use super::{forwarding_present, nearest_hop_claims, parse_node};

    fn headers(pairs: &[(&'static str, &'static str)]) -> HeaderMap {
        let mut map = HeaderMap::new();
        for (name, value) in pairs {
            map.append(*name, HeaderValue::from_static(value));
        }
        map
    }

    const CLIENT: IpAddr = IpAddr::V4(Ipv4Addr::new(203, 0, 113, 7));

    #[test]
    fn nodes_parse_with_ports_quotes_and_brackets() {
        let v6 = IpAddr::V6(Ipv6Addr::new(0x2001, 0xdb8, 0, 0, 0, 0, 0, 0x17));
        assert_eq!(parse_node("203.0.113.7"), Some(CLIENT));
        assert_eq!(parse_node(" 203.0.113.7:4711 "), Some(CLIENT));
        assert_eq!(parse_node("\"203.0.113.7\""), Some(CLIENT));
        assert_eq!(parse_node("2001:db8::17"), Some(v6));
        assert_eq!(parse_node("[2001:db8::17]"), Some(v6));
        assert_eq!(parse_node("\"[2001:db8::17]:4711\""), Some(v6));
        for unreadable in [
            "",
            "unknown",
            "_hidden",
            "203.0.113.7:",
            "203.0.113.7:port",
            "[2001:db8::17]x",
            "[203.0.113.7]",
            "fe80::1%eth0",
            "example.com",
        ] {
            assert_eq!(parse_node(unreadable), None, "{unreadable:?}");
        }
    }

    #[test]
    fn only_the_nearest_hop_entry_counts() {
        let claims = nearest_hop_claims(&headers(&[(
            "x-forwarded-for",
            "127.0.0.1, 10.0.0.1, 203.0.113.7",
        )]));
        assert_eq!(claims, vec![Some(CLIENT)]);

        let claims = nearest_hop_claims(&headers(&[
            ("x-forwarded-for", "127.0.0.1"),
            ("x-forwarded-for", "203.0.113.7"),
        ]));
        assert_eq!(claims, vec![Some(CLIENT)]);

        let claims = nearest_hop_claims(&headers(&[(
            "forwarded",
            "for=127.0.0.1, For=\"203.0.113.7:80\";proto=https;by=10.0.0.1",
        )]));
        assert_eq!(claims, vec![Some(CLIENT)]);
    }

    #[test]
    fn quoted_forwarded_parameters_cannot_smuggle_a_node() {
        // A quoted extension holding a comma and a `for=` is one value.
        let claims = nearest_hop_claims(&headers(&[(
            "forwarded",
            "for=203.0.113.7;ext=\"x,for=127.0.0.1;z=y\"",
        )]));
        assert_eq!(claims, vec![Some(CLIENT)]);

        for unreadable in [
            "for=\"203.0.113.7",
            "for=127.0.0.1;for=203.0.113.7",
            "for=203.0.113.7;proto",
            "for=\"x\\\", for=203.0.113.7",
        ] {
            let claims = nearest_hop_claims(&headers(&[("forwarded", unreadable)]));
            assert_eq!(claims, vec![None], "{unreadable:?}");
        }
    }

    #[test]
    fn every_address_header_keeps_its_own_claim() {
        let claims = nearest_hop_claims(&headers(&[
            ("forwarded", "for=192.0.2.60"),
            ("x-real-ip", "unknown"),
            ("x-forwarded-for", "203.0.113.7"),
        ]));
        assert_eq!(
            claims,
            vec![
                Some(CLIENT),
                None,
                Some(IpAddr::V4(Ipv4Addr::new(192, 0, 2, 60))),
            ]
        );
        assert_eq!(
            nearest_hop_claims(&headers(&[("forwarded", "proto=https")])),
            vec![None]
        );
    }

    #[test]
    fn only_client_naming_headers_mark_a_relay() {
        let relayed = headers(&[("forwarded", "proto=https")]);
        assert!(forwarding_present(&relayed));
        assert_eq!(nearest_hop_claims(&relayed), vec![None]);
        for unrelayed in [
            ("x-forwarded-host", "localhost:9430"),
            ("x-forwarded-proto", "https"),
            ("host", "localhost:9420"),
        ] {
            assert!(!forwarding_present(&headers(&[unrelayed])), "{unrelayed:?}");
        }
    }
}
