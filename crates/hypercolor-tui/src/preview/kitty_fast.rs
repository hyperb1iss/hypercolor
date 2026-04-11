use std::env;
use std::fmt::Write as _;
use std::fs::{self, OpenOptions};
use std::io::Write as _;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use flate2::Compression;
use flate2::write::ZlibEncoder;
use image::RgbImage;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui_image::picker::cap_parser::Parser;

static NEXT_TEMP_FILE_ID: AtomicU64 = AtomicU64::new(1);

pub(crate) struct KittyFrame {
    rect: Rect,
    image_id: u32,
    id_color: String,
    id_extra: u16,
    transmit: Option<String>,
    temp_path: Option<PathBuf>,
}

impl KittyFrame {
    pub(crate) fn new(
        image: RgbImage,
        area: Rect,
        image_id: u32,
        is_tmux: bool,
    ) -> Result<Self, String> {
        Self::new_with_medium(image, area, image_id, is_tmux, preferred_medium(area))
    }

    fn new_with_medium(
        image: RgbImage,
        area: Rect,
        image_id: u32,
        is_tmux: bool,
        medium: KittyMedium,
    ) -> Result<Self, String> {
        let image_width = image.width();
        let image_height = image.height();
        let (transmit, temp_path) = build_transmit(
            image.as_raw(),
            image_width,
            image_height,
            area,
            image_id,
            is_tmux,
            medium,
        )?;
        let [id_extra, id_r, id_g, id_b] = image_id.to_be_bytes();
        Ok(Self {
            rect: area,
            image_id,
            id_color: format!("\x1b[38;2;{id_r};{id_g};{id_b}m"),
            id_extra: u16::from(id_extra),
            transmit: Some(transmit),
            temp_path,
        })
    }

    pub(crate) fn area(&self) -> Rect {
        self.rect
    }

    pub(crate) fn render(&mut self, area: Rect, buf: &mut Buffer) {
        let seq = self.transmit.take();
        render(
            area,
            self.rect,
            buf,
            self.image_id,
            &self.id_color,
            self.id_extra,
            seq.as_deref(),
        );
    }
}

impl Drop for KittyFrame {
    fn drop(&mut self) {
        if let Some(path) = self.temp_path.take()
            && let Err(error) = fs::remove_file(&path)
            && error.kind() != std::io::ErrorKind::NotFound
        {
            tracing::debug!(
                "failed to remove kitty temp payload {}: {error}",
                path.display()
            );
        }
    }
}

fn build_transmit(
    pixels: &[u8],
    image_width: u32,
    image_height: u32,
    area: Rect,
    image_id: u32,
    is_tmux: bool,
    medium: KittyMedium,
) -> Result<(String, Option<PathBuf>), String> {
    if medium == KittyMedium::TempFile {
        match transmit_temp_file(pixels, image_width, image_height, area, image_id, is_tmux) {
            Ok(result) => return Ok(result),
            Err(error) => {
                tracing::debug!(
                    "kitty temp-file transport unavailable, falling back to direct: {error}"
                );
            }
        }
    }

    transmit_direct(pixels, image_width, image_height, area, image_id, is_tmux)
        .map(|transmit| (transmit, None))
}

fn transmit_direct(
    pixels: &[u8],
    image_width: u32,
    image_height: u32,
    _area: Rect,
    image_id: u32,
    is_tmux: bool,
) -> Result<String, String> {
    let compressed = compress_pixels(pixels)?;
    let encoded = BASE64_STANDARD.encode(compressed);
    let (start, escape, end) = Parser::escape_tmux(is_tmux);

    const CHARS_PER_CHUNK: usize = 4096;
    let chunk_count = encoded.len().div_ceil(CHARS_PER_CHUNK);
    let mut data =
        String::with_capacity(encoded.len() + (chunk_count * (64 + escape.len() * 2 + end.len())));

    for (index, chunk) in encoded.as_bytes().chunks(CHARS_PER_CHUNK).enumerate() {
        data.push_str(start);
        write!(data, "{escape}_Gq=2,").map_err(|error| error.to_string())?;
        if index == 0 {
            write!(
                data,
                "i={image_id},a=T,U=1,f=24,o=z,s={},v={},",
                image_width, image_height
            )
            .map_err(|error| error.to_string())?;
        }
        let more = u8::from(index + 1 < chunk_count);
        write!(data, "m={more};").map_err(|error| error.to_string())?;
        data.push_str(std::str::from_utf8(chunk).map_err(|error| error.to_string())?);
        write!(data, "{escape}\\").map_err(|error| error.to_string())?;
        data.push_str(end);
    }

    Ok(data)
}

fn transmit_temp_file(
    pixels: &[u8],
    image_width: u32,
    image_height: u32,
    _area: Rect,
    image_id: u32,
    is_tmux: bool,
) -> Result<(String, Option<PathBuf>), String> {
    let compressed = compress_pixels(pixels)?;
    let path = write_temp_payload(&compressed)?;
    let encoded_path = BASE64_STANDARD.encode(path.to_string_lossy().as_bytes());
    let (start, escape, end) = Parser::escape_tmux(is_tmux);
    let mut data = String::with_capacity(encoded_path.len() + 128 + escape.len() * 2 + end.len());

    data.push_str(start);
    write!(
        data,
        "{escape}_Gq=2,i={image_id},a=T,U=1,f=24,t=t,o=z,s={},v={},S={};",
        image_width,
        image_height,
        compressed.len()
    )
    .map_err(|error| error.to_string())?;
    data.push_str(&encoded_path);
    write!(data, "{escape}\\").map_err(|error| error.to_string())?;
    data.push_str(end);

    Ok((data, Some(path)))
}

fn compress_pixels(pixels: &[u8]) -> Result<Vec<u8>, String> {
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::fast());
    encoder
        .write_all(pixels)
        .map_err(|error| error.to_string())?;
    encoder.finish().map_err(|error| error.to_string())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum KittyMedium {
    Direct,
    TempFile,
}

fn preferred_medium(area: Rect) -> KittyMedium {
    match env::var("HYPERCOLOR_TUI_KITTY_TRANSPORT").ok().as_deref() {
        Some("direct") => return KittyMedium::Direct,
        Some("temp") => return KittyMedium::TempFile,
        _ => {}
    }

    let _ = area;
    KittyMedium::Direct
}

fn write_temp_payload(payload: &[u8]) -> Result<PathBuf, String> {
    let temp_dir = env::temp_dir();
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| error.to_string())?
        .as_millis();

    for _attempt in 0..8 {
        let file_id = NEXT_TEMP_FILE_ID.fetch_add(1, Ordering::Relaxed);
        let path = temp_dir.join(format!(
            "hypercolor-tty-graphics-protocol-{}-{timestamp}-{file_id}.rgbz",
            std::process::id()
        ));

        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(mut file) => {
                file.write_all(payload).map_err(|error| error.to_string())?;
                file.flush().map_err(|error| error.to_string())?;
                return Ok(path);
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error.to_string()),
        }
    }

    Err("failed to allocate kitty temp payload".to_string())
}

fn render(
    area: Rect,
    rect: Rect,
    buf: &mut Buffer,
    _image_id: u32,
    id_color: &str,
    id_extra: u16,
    mut seq: Option<&str>,
) {
    let full_width = area.width.min(rect.width);
    let width_usize = usize::from(full_width);
    if width_usize == 0 {
        return;
    }

    let estimated_placeholder_row_size = id_color.len() + 30 + (width_usize * 4) + 30;
    let estimated_transmit_row_size = estimated_placeholder_row_size + seq.map_or(0, str::len);
    let mut symbol = String::with_capacity(estimated_transmit_row_size);

    let row_diacritics: String = std::iter::repeat_n('\u{10EEEE}', width_usize - 1).collect();
    let right = area.width.saturating_sub(1);
    let down = area.height.saturating_sub(1);
    let restore_cursor = format!("\x1b[u\x1b[{right}C\x1b[{down}B");

    let height = area.height.min(rect.height).min(DIACRITICS.len() as u16);
    for y in 0..height {
        symbol.clear();
        if let Some(transmit) = seq.take() {
            symbol.push_str(transmit);
        }

        write!(
            symbol,
            "\x1b[s{id_color}\u{10EEEE}{}{}{}",
            diacritic(y),
            diacritic(0),
            diacritic(id_extra)
        )
        .expect("string write cannot fail");

        symbol.push_str(&row_diacritics);

        for x in 1..full_width {
            if let Some(cell) = buf.cell_mut((area.left() + x, area.top() + y)) {
                cell.set_skip(true);
            }
        }

        symbol.push_str(&restore_cursor);
        if let Some(cell) = buf.cell_mut((area.left(), area.top() + y)) {
            cell.set_symbol(&symbol);
        }
    }
}

fn diacritic(index: u16) -> char {
    DIACRITICS
        .get(usize::from(index))
        .copied()
        .unwrap_or('\u{305}')
}

// From kitty rowcolumn-diacritics.txt.
static DIACRITICS: [char; 297] = [
    '\u{305}',
    '\u{30D}',
    '\u{30E}',
    '\u{310}',
    '\u{312}',
    '\u{33D}',
    '\u{33E}',
    '\u{33F}',
    '\u{346}',
    '\u{34A}',
    '\u{34B}',
    '\u{34C}',
    '\u{350}',
    '\u{351}',
    '\u{352}',
    '\u{357}',
    '\u{35B}',
    '\u{363}',
    '\u{364}',
    '\u{365}',
    '\u{366}',
    '\u{367}',
    '\u{368}',
    '\u{369}',
    '\u{36A}',
    '\u{36B}',
    '\u{36C}',
    '\u{36D}',
    '\u{36E}',
    '\u{36F}',
    '\u{483}',
    '\u{484}',
    '\u{485}',
    '\u{486}',
    '\u{487}',
    '\u{592}',
    '\u{593}',
    '\u{594}',
    '\u{595}',
    '\u{597}',
    '\u{598}',
    '\u{599}',
    '\u{59C}',
    '\u{59D}',
    '\u{59E}',
    '\u{59F}',
    '\u{5A0}',
    '\u{5A1}',
    '\u{5A8}',
    '\u{5A9}',
    '\u{5AB}',
    '\u{5AC}',
    '\u{5AF}',
    '\u{5C4}',
    '\u{610}',
    '\u{611}',
    '\u{612}',
    '\u{613}',
    '\u{614}',
    '\u{615}',
    '\u{616}',
    '\u{617}',
    '\u{657}',
    '\u{658}',
    '\u{659}',
    '\u{65A}',
    '\u{65B}',
    '\u{65D}',
    '\u{65E}',
    '\u{6D6}',
    '\u{6D7}',
    '\u{6D8}',
    '\u{6D9}',
    '\u{6DA}',
    '\u{6DB}',
    '\u{6DC}',
    '\u{6DF}',
    '\u{6E0}',
    '\u{6E1}',
    '\u{6E2}',
    '\u{6E4}',
    '\u{6E7}',
    '\u{6E8}',
    '\u{6EB}',
    '\u{6EC}',
    '\u{730}',
    '\u{732}',
    '\u{733}',
    '\u{735}',
    '\u{736}',
    '\u{73A}',
    '\u{73D}',
    '\u{73F}',
    '\u{740}',
    '\u{741}',
    '\u{743}',
    '\u{745}',
    '\u{747}',
    '\u{749}',
    '\u{74A}',
    '\u{7EB}',
    '\u{7EC}',
    '\u{7ED}',
    '\u{7EE}',
    '\u{7EF}',
    '\u{7F0}',
    '\u{7F1}',
    '\u{7F3}',
    '\u{816}',
    '\u{817}',
    '\u{818}',
    '\u{819}',
    '\u{81B}',
    '\u{81C}',
    '\u{81D}',
    '\u{81E}',
    '\u{81F}',
    '\u{820}',
    '\u{821}',
    '\u{822}',
    '\u{823}',
    '\u{825}',
    '\u{826}',
    '\u{827}',
    '\u{829}',
    '\u{82A}',
    '\u{82B}',
    '\u{82C}',
    '\u{82D}',
    '\u{951}',
    '\u{953}',
    '\u{954}',
    '\u{F82}',
    '\u{F83}',
    '\u{F86}',
    '\u{F87}',
    '\u{135D}',
    '\u{135E}',
    '\u{135F}',
    '\u{17DD}',
    '\u{193A}',
    '\u{1A17}',
    '\u{1A75}',
    '\u{1A76}',
    '\u{1A77}',
    '\u{1A78}',
    '\u{1A79}',
    '\u{1A7A}',
    '\u{1A7B}',
    '\u{1A7C}',
    '\u{1B6B}',
    '\u{1B6D}',
    '\u{1B6E}',
    '\u{1B6F}',
    '\u{1B70}',
    '\u{1B71}',
    '\u{1B72}',
    '\u{1B73}',
    '\u{1CD0}',
    '\u{1CD1}',
    '\u{1CD2}',
    '\u{1CDA}',
    '\u{1CDB}',
    '\u{1CE0}',
    '\u{1DC0}',
    '\u{1DC1}',
    '\u{1DC3}',
    '\u{1DC4}',
    '\u{1DC5}',
    '\u{1DC6}',
    '\u{1DC7}',
    '\u{1DC8}',
    '\u{1DC9}',
    '\u{1DCB}',
    '\u{1DCC}',
    '\u{1DD1}',
    '\u{1DD2}',
    '\u{1DD3}',
    '\u{1DD4}',
    '\u{1DD5}',
    '\u{1DD6}',
    '\u{1DD7}',
    '\u{1DD8}',
    '\u{1DD9}',
    '\u{1DDA}',
    '\u{1DDB}',
    '\u{1DDC}',
    '\u{1DDD}',
    '\u{1DDE}',
    '\u{1DDF}',
    '\u{1DE0}',
    '\u{1DE1}',
    '\u{1DE2}',
    '\u{1DE3}',
    '\u{1DE4}',
    '\u{1DE5}',
    '\u{1DE6}',
    '\u{1DFE}',
    '\u{20D0}',
    '\u{20D1}',
    '\u{20D4}',
    '\u{20D5}',
    '\u{20D6}',
    '\u{20D7}',
    '\u{20DB}',
    '\u{20DC}',
    '\u{20E1}',
    '\u{20E7}',
    '\u{20E9}',
    '\u{20F0}',
    '\u{2CEF}',
    '\u{2CF0}',
    '\u{2CF1}',
    '\u{2DE0}',
    '\u{2DE1}',
    '\u{2DE2}',
    '\u{2DE3}',
    '\u{2DE4}',
    '\u{2DE5}',
    '\u{2DE6}',
    '\u{2DE7}',
    '\u{2DE8}',
    '\u{2DE9}',
    '\u{2DEA}',
    '\u{2DEB}',
    '\u{2DEC}',
    '\u{2DED}',
    '\u{2DEE}',
    '\u{2DEF}',
    '\u{2DF0}',
    '\u{2DF1}',
    '\u{2DF2}',
    '\u{2DF3}',
    '\u{2DF4}',
    '\u{2DF5}',
    '\u{2DF6}',
    '\u{2DF7}',
    '\u{2DF8}',
    '\u{2DF9}',
    '\u{2DFA}',
    '\u{2DFB}',
    '\u{2DFC}',
    '\u{2DFD}',
    '\u{2DFE}',
    '\u{2DFF}',
    '\u{A66F}',
    '\u{A67C}',
    '\u{A67D}',
    '\u{A6F0}',
    '\u{A6F1}',
    '\u{A8E0}',
    '\u{A8E1}',
    '\u{A8E2}',
    '\u{A8E3}',
    '\u{A8E4}',
    '\u{A8E5}',
    '\u{A8E6}',
    '\u{A8E7}',
    '\u{A8E8}',
    '\u{A8E9}',
    '\u{A8EA}',
    '\u{A8EB}',
    '\u{A8EC}',
    '\u{A8ED}',
    '\u{A8EE}',
    '\u{A8EF}',
    '\u{A8F0}',
    '\u{A8F1}',
    '\u{AAB0}',
    '\u{AAB2}',
    '\u{AAB3}',
    '\u{AAB7}',
    '\u{AAB8}',
    '\u{AABE}',
    '\u{AABF}',
    '\u{AAC1}',
    '\u{FE20}',
    '\u{FE21}',
    '\u{FE22}',
    '\u{FE23}',
    '\u{FE24}',
    '\u{FE25}',
    '\u{FE26}',
    '\u{10A0F}',
    '\u{10A38}',
    '\u{1D185}',
    '\u{1D186}',
    '\u{1D187}',
    '\u{1D188}',
    '\u{1D189}',
    '\u{1D1AA}',
    '\u{1D1AB}',
    '\u{1D1AC}',
    '\u{1D1AD}',
    '\u{1D242}',
    '\u{1D243}',
    '\u{1D244}',
];

#[cfg(test)]
mod tests {
    use super::{KittyFrame, KittyMedium, preferred_medium};
    use image::RgbImage;
    use ratatui::buffer::Buffer;
    use ratatui::layout::Rect;
    use std::path::PathBuf;

    fn solid_image(width: u32, height: u32) -> RgbImage {
        RgbImage::from_raw(width, height, vec![0; (width * height * 3) as usize])
            .expect("valid image buffer")
    }

    #[test]
    fn transmit_sequence_uses_compressed_rgb_payload() {
        let image = RgbImage::from_raw(2, 2, vec![0, 0, 0, 255, 0, 0, 0, 255, 0, 255, 255, 255])
            .expect("valid image buffer");

        let kitty = KittyFrame::new(image, Rect::new(0, 0, 2, 2), 42, false)
            .expect("kitty frame should build");

        let transmit = kitty.transmit.as_deref().expect("transmit should exist");
        assert!(transmit.contains("i=42"));
        assert!(transmit.contains("f=24"));
        assert!(transmit.contains("o=z"));
        assert!(!transmit.contains("t=t"));
    }

    #[test]
    fn render_marks_trailing_cells_as_skip() {
        let mut kitty = KittyFrame::new(solid_image(2, 2), Rect::new(0, 0, 2, 2), 7, false)
            .expect("kitty frame should build");
        let mut buf = Buffer::empty(Rect::new(0, 0, 2, 2));
        kitty.render(Rect::new(0, 0, 2, 2), &mut buf);

        assert!(buf[(1, 0)].skip);
        assert!(buf[(1, 1)].skip);
        assert!(!buf[(0, 0)].symbol().is_empty());
    }

    #[test]
    fn large_area_defaults_to_direct_transport() {
        assert_eq!(
            preferred_medium(Rect::new(0, 0, 50, 30)),
            KittyMedium::Direct
        );
    }

    #[test]
    fn temp_file_transport_cleans_up_payload() {
        let payload_path: PathBuf;
        {
            let kitty = KittyFrame::new_with_medium(
                solid_image(2, 2),
                Rect::new(0, 0, 50, 30),
                17,
                false,
                KittyMedium::TempFile,
            )
            .expect("kitty frame should build");
            let transmit = kitty.transmit.as_deref().expect("transmit should exist");
            assert!(transmit.contains("t=t"));
            payload_path = kitty
                .temp_path
                .clone()
                .expect("temp-file transport should keep a payload path");
            assert!(payload_path.exists());
        }

        assert!(!payload_path.exists());
    }
}
