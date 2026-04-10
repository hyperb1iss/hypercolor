use owo_colors::OwoColorize;

const LETTERS: [(&str, u8, u8, u8); 10] = [
    ("H", 255, 106, 193), // Coral
    ("Y", 240, 80, 224),
    ("P", 225, 53, 255), // Electric Purple
    ("E", 176, 103, 244),
    ("R", 152, 154, 240),
    ("C", 128, 204, 237),
    ("O", 128, 255, 234), // Neon Cyan
    ("L", 104, 252, 178),
    ("O", 80, 250, 123), // Success Green
    ("R", 160, 250, 131),
];

/// Print the startup banner to stderr, bypassing tracing.
pub fn print(version: &str, canvas: (u32, u32), bind: &str) {
    let name: String = LETTERS
        .iter()
        .map(|(ch, r, g, b)| format!("{}", ch.truecolor(*r, *g, *b).bold()))
        .collect::<Vec<_>>()
        .join(" ");

    let bar = "━━━".truecolor(80, 80, 100);
    let cap_l = "╺".truecolor(80, 80, 100);
    let cap_r = "╸".truecolor(80, 80, 100);

    let subtitle = format!("rgb orchestration engine · v{version}");
    let details = format!("canvas {}×{} · listening on {bind}", canvas.0, canvas.1);

    eprintln!();
    eprintln!("  {cap_l}{bar}{cap_r} {name} {cap_l}{bar}{cap_r}");
    eprintln!("        {}", subtitle.truecolor(120, 120, 140));
    eprintln!("        {}", details.truecolor(100, 100, 120));
    eprintln!();
}
