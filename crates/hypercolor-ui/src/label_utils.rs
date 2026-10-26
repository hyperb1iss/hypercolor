use std::cmp::Ordering;

pub fn humanize_identifier_label(identifier: &str) -> String {
    identifier
        .split(['-', '_', ' '])
        .filter(|part| !part.is_empty())
        .map(humanize_identifier_part)
        .collect::<Vec<_>>()
        .join(" ")
}

fn humanize_identifier_part(part: &str) -> String {
    if is_acronym(part) {
        return part.to_ascii_uppercase();
    }

    let lower = part.to_ascii_lowercase();
    let mut chars = lower.chars();
    chars.next().map_or_else(String::new, |first| {
        format!("{}{}", first.to_ascii_uppercase(), chars.as_str())
    })
}

fn is_acronym(part: &str) -> bool {
    matches!(
        part.to_ascii_lowercase().as_str(),
        "api" | "ddp" | "dmx" | "hid" | "http" | "https" | "midi" | "sdk" | "spi" | "usb"
    ) || part.chars().any(|ch| ch.is_ascii_digit())
}

/// Order labels the way people count: runs of digits compare by value,
/// so "TL LCD 2" sorts before "TL LCD 10", and letters compare without
/// case. Labels that differ only in case or leading zeros still order
/// consistently, by their plain text.
#[must_use]
pub fn natural_cmp(left: &str, right: &str) -> Ordering {
    let mut lhs = left.chars().peekable();
    let mut rhs = right.chars().peekable();
    loop {
        match (lhs.peek().copied(), rhs.peek().copied()) {
            (None, None) => return left.cmp(right),
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(a), Some(b)) if a.is_ascii_digit() && b.is_ascii_digit() => {
                let a_run = digit_run(&mut lhs);
                let b_run = digit_run(&mut rhs);
                let order = compare_digit_runs(&a_run, &b_run);
                if order != Ordering::Equal {
                    return order;
                }
            }
            (Some(a), Some(b)) => {
                let order = a.to_lowercase().cmp(b.to_lowercase());
                if order != Ordering::Equal {
                    return order;
                }
                lhs.next();
                rhs.next();
            }
        }
    }
}

fn digit_run(chars: &mut std::iter::Peekable<std::str::Chars<'_>>) -> String {
    let mut run = String::new();
    while let Some(digit) = chars.next_if(char::is_ascii_digit) {
        run.push(digit);
    }
    run
}

/// Compare two runs of ASCII digits by value, without parsing, so a run
/// of any length compares correctly.
fn compare_digit_runs(left: &str, right: &str) -> Ordering {
    let left = left.trim_start_matches('0');
    let right = right.trim_start_matches('0');
    left.len().cmp(&right.len()).then_with(|| left.cmp(right))
}
