use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::Widget;

/// A single-line effect list item widget.
///
/// Renders as: `▸ Rainbow Wave          ✦ native  ♪  ★`
///
/// Components (left to right):
/// - Selection cursor (`▸`) if selected, space otherwise
/// - Effect name (bold if active)
/// - Source badge: `✦ native` or `◈ web`
/// - `♪` if audio-reactive
/// - `★` if favourite, `☆` otherwise
#[allow(clippy::struct_excessive_bools)]
pub struct EffectCard<'a> {
    name: &'a str,
    category: &'a str,
    source: &'a str,
    audio_reactive: bool,
    is_active: bool,
    is_favorite: bool,
    is_selected: bool,
}

/// Accent colour for the selection cursor and active highlight.
const ACCENT: Color = Color::Rgb(0, 200, 255);

/// Muted text colour for secondary information.
const MUTED: Color = Color::Rgb(120, 120, 120);

/// Favourite star colour.
const STAR_COLOR: Color = Color::Rgb(255, 200, 50);

impl<'a> EffectCard<'a> {
    #[must_use]
    #[allow(clippy::fn_params_excessive_bools)]
    pub fn new(
        name: &'a str,
        category: &'a str,
        source: &'a str,
        audio_reactive: bool,
        is_active: bool,
        is_favorite: bool,
        is_selected: bool,
    ) -> Self {
        Self {
            name,
            category,
            source,
            audio_reactive,
            is_active,
            is_favorite,
            is_selected,
        }
    }
}

/// Write a string into the buffer starting at `(x, y)`, returning the number
/// of columns consumed. Characters that would exceed `max_x` are clipped.
fn write_str(buf: &mut Buffer, x: u16, y: u16, max_x: u16, text: &str, style: Style) -> u16 {
    let mut cx = x;
    for ch in text.chars() {
        if cx >= max_x {
            break;
        }
        let cell = &mut buf[(cx, y)];
        cell.set_char(ch);
        cell.set_style(style);
        cx += 1;
    }
    cx - x
}

impl Widget for EffectCard<'_> {
    #[allow(clippy::cast_possible_truncation, clippy::as_conversions)]
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.width == 0 || area.height == 0 {
            return;
        }

        let y = area.y;
        let max_x = area.x + area.width;
        let mut x = area.x;

        // -- Selection cursor --
        let cursor_ch = if self.is_selected { '\u{25B8}' } else { ' ' }; // ▸
        let cursor_style = Style::default().fg(ACCENT);
        if x < max_x {
            let cell = &mut buf[(x, y)];
            cell.set_char(cursor_ch);
            cell.set_style(cursor_style);
            x += 1;
        }

        // Space after cursor.
        if x < max_x {
            buf[(x, y)].set_char(' ');
            x += 1;
        }

        // -- Effect name --
        let name_style = if self.is_active {
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::White)
        };
        let name_used = write_str(buf, x, y, max_x, self.name, name_style);
        x += name_used;

        // -- Right-side badges --
        // Build the badge string so we can right-align it.
        let source_badge = if self.source == "native" {
            "\u{2726} native" // ✦ native
        } else {
            "\u{25C8} web" // ◈ web
        };

        let audio_badge = if self.audio_reactive { " \u{266A}" } else { "" }; // ♪
        let fav_badge = if self.is_favorite {
            " \u{2605}" // ★
        } else {
            " \u{2606}" // ☆
        };

        // Category in parentheses.
        let category_str = if self.category.is_empty() {
            String::new()
        } else {
            format!(" {}", self.category)
        };

        let right_text = format!("{category_str}  {source_badge}{audio_badge}{fav_badge}");
        let right_len = right_text.chars().count() as u16;

        // Pad between name and right-side badges.
        let right_start = if max_x > right_len {
            max_x - right_len
        } else {
            x
        };

        // Fill gap with spaces.
        while x < right_start && x < max_x {
            buf[(x, y)].set_char(' ');
            x += 1;
        }

        // -- Render right-side text with per-segment styles --
        // Category (muted).
        if !self.category.is_empty() {
            let cat_style = Style::default().fg(MUTED);
            x += write_str(buf, x, y, max_x, " ", cat_style);
            x += write_str(buf, x, y, max_x, self.category, cat_style);
        }

        // Double-space separator.
        let sep_style = Style::default();
        x += write_str(buf, x, y, max_x, "  ", sep_style);

        // Source badge.
        let source_style = if self.source == "native" {
            Style::default().fg(Color::Rgb(180, 140, 255))
        } else {
            Style::default().fg(Color::Rgb(255, 180, 80))
        };
        x += write_str(buf, x, y, max_x, source_badge, source_style);

        // Audio badge.
        if self.audio_reactive {
            let audio_style = Style::default().fg(Color::Rgb(100, 255, 180));
            x += write_str(buf, x, y, max_x, " \u{266A}", audio_style);
        }

        // Favourite badge.
        let fav_style = if self.is_favorite {
            Style::default().fg(STAR_COLOR)
        } else {
            Style::default().fg(MUTED)
        };
        write_str(buf, x, y, max_x, fav_badge, fav_style);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[allow(clippy::fn_params_excessive_bools)]
    fn make_card(selected: bool, active: bool, favorite: bool, audio: bool) -> EffectCard<'static> {
        EffectCard::new(
            "Rainbow Wave",
            "ambient",
            "native",
            audio,
            active,
            favorite,
            selected,
        )
    }

    #[test]
    fn render_zero_area_does_not_panic() {
        let card = make_card(false, false, false, false);
        let area = Rect::new(0, 0, 0, 0);
        let mut buf = Buffer::empty(area);
        card.render(area, &mut buf);
    }

    #[test]
    fn selected_card_shows_cursor() {
        let card = make_card(true, false, false, false);
        let area = Rect::new(0, 0, 60, 1);
        let mut buf = Buffer::empty(area);
        card.render(area, &mut buf);

        assert_eq!(buf[(0, 0)].symbol(), "\u{25B8}");
    }

    #[test]
    fn unselected_card_shows_space() {
        let card = make_card(false, false, false, false);
        let area = Rect::new(0, 0, 60, 1);
        let mut buf = Buffer::empty(area);
        card.render(area, &mut buf);

        assert_eq!(buf[(0, 0)].symbol(), " ");
    }

    #[test]
    fn active_card_name_is_bold() {
        let card = make_card(false, true, false, false);
        let area = Rect::new(0, 0, 60, 1);
        let mut buf = Buffer::empty(area);
        card.render(area, &mut buf);

        // The name starts at column 2.
        let cell = &buf[(2, 0)];
        assert!(
            cell.modifier.contains(Modifier::BOLD),
            "active card name should be bold"
        );
    }

    #[test]
    fn favorite_card_shows_filled_star() {
        let card = make_card(false, false, true, false);
        let area = Rect::new(0, 0, 60, 1);
        let mut buf = Buffer::empty(area);
        card.render(area, &mut buf);

        let has_star = (0..60).any(|x| buf[(x, 0)].symbol() == "\u{2605}");
        assert!(has_star, "expected filled star for favourite");
    }

    #[test]
    fn non_favorite_card_shows_empty_star() {
        let card = make_card(false, false, false, false);
        let area = Rect::new(0, 0, 60, 1);
        let mut buf = Buffer::empty(area);
        card.render(area, &mut buf);

        let has_star = (0..60).any(|x| buf[(x, 0)].symbol() == "\u{2606}");
        assert!(has_star, "expected empty star for non-favourite");
    }

    #[test]
    fn audio_reactive_shows_note() {
        let card = make_card(false, false, false, true);
        let area = Rect::new(0, 0, 60, 1);
        let mut buf = Buffer::empty(area);
        card.render(area, &mut buf);

        let has_note = (0..60).any(|x| buf[(x, 0)].symbol() == "\u{266A}");
        assert!(has_note, "expected musical note for audio-reactive card");
    }

    #[test]
    fn web_source_shows_diamond() {
        let card = EffectCard::new("Test", "fx", "web", false, false, false, false);
        let area = Rect::new(0, 0, 60, 1);
        let mut buf = Buffer::empty(area);
        card.render(area, &mut buf);

        let has_diamond = (0..60).any(|x| buf[(x, 0)].symbol() == "\u{25C8}");
        assert!(has_diamond, "expected diamond for web source");
    }
}
