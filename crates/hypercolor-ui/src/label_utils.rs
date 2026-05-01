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
