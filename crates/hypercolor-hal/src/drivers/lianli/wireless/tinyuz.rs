//! Pure-Rust tinyuz codec for the wireless fan firmware's RGB payloads.
//!
//! The fan receivers decompress per-LED RGB with tinyuz (sisong/tinyuz
//! 1.1.1, commit `1d74ffa4d453796df352df470733f45dfa099bb1`) built for a
//! 4 KiB dictionary. The workspace forbids unsafe code and the HAL takes no
//! C toolchain, so the encoder is written here from the decoder's bitstream
//! definition, and a decoder written the same way proves it against streams
//! the upstream compressor produced (`tests/fixtures/tinyuz`).
//!
//! Stream layout, as the reference decoder reads it:
//!
//! * A 4-byte little-endian dictionary size: the largest match distance the
//!   stream uses, at least 1. The compressor rewrites it after encoding, so
//!   it reflects use rather than configuration.
//! * A code stream mixing "type" bytes (bit flags, consumed LSB first) with
//!   literal bytes and position bytes. A type byte is allocated the moment a
//!   flag is needed and later flags fill it in place, so byte order matches
//!   the order a sequential reader needs them in.
//! * Per item, one type bit: `1` = a literal byte follows in the stream; `0`
//!   = a dictionary reference. A reference carries a length varint (one
//!   value bit plus one continue bit per group), then, when the previous
//!   item was a literal, one "same distance as last time" bit, then, unless
//!   that bit was set, a distance: one byte whose high bit means a varint
//!   (two value bits plus one continue bit per group) of the remaining high
//!   bits follows.
//! * Distance zero is a control code whose "length" selects the kind:
//!   1 = literal line (a varint count plus 15 raw bytes follow), 2 = clip
//!   end, 3 = stream end.

const DICT_SIZE_HEADER_LEN: usize = 4;
const MIN_LITERAL_LINE_LEN: usize = 15;
const MIN_MATCH_LEN: usize = 2;
const TYPE_BITS_PER_BYTE: u8 = 8;
/// Distances above this borrow one length unit; the decoder adds it back.
const BIG_POS_FOR_LEN: usize = (1 << 11) + (1 << 9) + (1 << 7) - 1;
const LEN_PACK_BITS: u32 = 1;
const POS_PACK_BITS: u32 = 2;
const CTRL_LITERAL_LINE: usize = 1;
const CTRL_CLIP_END: usize = 2;
const CTRL_STREAM_END: usize = 3;
/// Longest match the reference encoder emits (`tuz_kMaxOfMaxSaveLength`).
const MAX_MATCH_LEN: usize = 1024 * 64 - 1;
/// The dictionary the fan firmware is built for.
pub const FIRMWARE_DICT_SIZE: usize = 4096;

/// Compression parameters.
#[derive(Debug, Clone, Copy)]
pub struct Params {
    /// Dictionary (window) size in bytes.
    pub dict_size: usize,
    /// Emit literal-line control codes for long incompressible runs.
    pub literal_lines: bool,
}

impl Default for Params {
    fn default() -> Self {
        Self {
            dict_size: FIRMWARE_DICT_SIZE,
            literal_lines: true,
        }
    }
}

/// Bit-flag and byte writer mirroring the reference `TTuzCode`.
struct CodeWriter {
    code: Vec<u8>,
    types_index: usize,
    type_count: u8,
    dict_pos_back: usize,
    have_data_back: bool,
    dict_size_max: usize,
    literal_lines: bool,
}

impl CodeWriter {
    fn new(literal_lines: bool) -> Self {
        Self {
            code: vec![0; DICT_SIZE_HEADER_LEN],
            types_index: 0,
            type_count: 0,
            dict_pos_back: 1,
            have_data_back: false,
            dict_size_max: 1,
            literal_lines,
        }
    }

    fn out_type(&mut self, bit: bool) {
        if self.type_count == 0 {
            self.types_index = self.code.len();
            self.code.push(0);
        }
        if bit {
            self.code[self.types_index] |= 1 << self.type_count;
        }
        self.type_count += 1;
        if self.type_count == TYPE_BITS_PER_BYTE {
            self.type_count = 0;
        }
    }

    /// Varint: `pack_bits` value bits then one continue bit per group, most
    /// significant group first, with the reference's per-group bias.
    fn out_len(&mut self, value: usize, pack_bits: u32) {
        let mut remaining = value;
        let mut count = 1_u32;
        loop {
            let group_max = 1_usize << (count * pack_bits);
            if remaining < group_max {
                break;
            }
            remaining -= group_max;
            count += 1;
        }
        let mut group = count;
        while group > 0 {
            group -= 1;
            for bit in 0..pack_bits {
                self.out_type((remaining >> (group * pack_bits + bit)) & 1 == 1);
            }
            self.out_type(group > 0);
        }
    }

    fn out_dict_pos(&mut self, saved_pos: usize) {
        let long = saved_pos >= (1 << 7);
        let pos = if long {
            saved_pos - (1 << 7)
        } else {
            saved_pos
        };
        let low = u8::try_from(pos & 0x7F).expect("masked to seven bits");
        self.code.push(low | if long { 0x80 } else { 0 });
        if long {
            self.out_len(pos >> 7, POS_PACK_BITS);
        }
    }

    fn out_literals(&mut self, data: &[u8]) {
        if self.literal_lines && data.len() >= MIN_LITERAL_LINE_LEN {
            self.out_ctrl(CTRL_LITERAL_LINE);
            self.out_len(data.len() - MIN_LITERAL_LINE_LEN, POS_PACK_BITS);
            self.code.extend_from_slice(data);
        } else {
            for &byte in data {
                self.out_type(true);
                self.code.push(byte);
            }
        }
        self.have_data_back = true;
    }

    /// Emit a reference to `match_len` bytes at `distance` (1-based) back.
    fn out_dict(&mut self, match_len: usize, distance: usize) {
        debug_assert!(distance >= 1);
        debug_assert!(match_len >= MIN_MATCH_LEN);
        self.out_type(false);
        self.dict_size_max = self.dict_size_max.max(distance);
        let same_pos = self.dict_pos_back == distance;
        let saved_same_pos = same_pos && self.have_data_back;
        let mut len = match_len - MIN_MATCH_LEN;
        if !saved_same_pos && distance > BIG_POS_FOR_LEN {
            debug_assert!(match_len > MIN_MATCH_LEN);
            len -= 1;
        }
        self.out_len(len, LEN_PACK_BITS);
        if self.have_data_back {
            self.out_type(saved_same_pos);
        }
        if !saved_same_pos {
            self.out_dict_pos(distance);
        }
        self.have_data_back = false;
        self.dict_pos_back = distance;
    }

    fn out_ctrl(&mut self, ctrl: usize) {
        self.out_type(false);
        self.out_len(ctrl, LEN_PACK_BITS);
        if self.have_data_back {
            self.out_type(false);
        }
        self.out_dict_pos(0);
    }

    fn finish(mut self) -> Vec<u8> {
        self.out_ctrl(CTRL_STREAM_END);
        let header = u32::try_from(self.dict_size_max).unwrap_or(u32::MAX);
        self.code[..DICT_SIZE_HEADER_LEN].copy_from_slice(&header.to_le_bytes());
        self.code
    }
}

/// Compress `input` into a tinyuz stream the firmware can decode.
///
/// Greedy longest-match parsing over the window, preferring the repeat
/// distance when it is nearly as long (it costs no position byte). Frames
/// are a few hundred bytes, so the search is a plain scan.
#[must_use]
pub fn compress(input: &[u8], params: Params) -> Vec<u8> {
    let window = params.dict_size.max(1);
    let mut writer = CodeWriter::new(params.literal_lines);
    let mut literal_start = 0_usize;
    let mut cursor = 0_usize;

    while cursor < input.len() {
        match best_match(input, cursor, window, writer.dict_pos_back) {
            Some((len, distance)) => {
                if literal_start < cursor {
                    writer.out_literals(&input[literal_start..cursor]);
                }
                writer.out_dict(len, distance);
                cursor += len;
                literal_start = cursor;
            }
            None => cursor += 1,
        }
    }
    if literal_start < input.len() {
        writer.out_literals(&input[literal_start..]);
    }
    writer.finish()
}

fn best_match(
    input: &[u8],
    cursor: usize,
    window: usize,
    repeat_distance: usize,
) -> Option<(usize, usize)> {
    let max_len = (input.len() - cursor).min(MAX_MATCH_LEN);
    if max_len < MIN_MATCH_LEN {
        return None;
    }
    let max_distance = cursor.min(window);

    let match_len_at = |distance: usize| -> usize {
        let start = cursor - distance;
        let mut len = 0;
        while len < max_len && input[start + len] == input[cursor + len] {
            len += 1;
        }
        len
    };

    let mut best: Option<(usize, usize)> = None;
    for distance in 1..=max_distance {
        let len = match_len_at(distance);
        let min_len = if distance > BIG_POS_FOR_LEN {
            MIN_MATCH_LEN + 1
        } else {
            MIN_MATCH_LEN
        };
        if len >= min_len && best.is_none_or(|(best_len, _)| len > best_len) {
            best = Some((len, distance));
            if len == max_len {
                break;
            }
        }
    }

    if repeat_distance >= 1 && repeat_distance <= max_distance {
        let len = match_len_at(repeat_distance);
        if len >= MIN_MATCH_LEN && best.is_none_or(|(best_len, _)| len + 1 >= best_len) {
            return Some((len, repeat_distance));
        }
    }
    best
}

/// Decoder failures.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DecodeError {
    /// The stream ended before its stream-end control code.
    #[error("tinyuz stream ended before its stream-end code")]
    Truncated,
    /// A reference reached before the start of the output.
    #[error("tinyuz reference reaches before the start of the output")]
    BadDistance,
    /// A control code the format does not define.
    #[error("tinyuz control code {0} is not defined")]
    UnknownControl(usize),
    /// Output exceeded the caller's limit.
    #[error("tinyuz output exceeds the {0}-byte limit")]
    OutputTooLarge(usize),
}

/// Reader mirroring `tuz_decompress_mem`.
struct CodeReader<'a> {
    code: &'a [u8],
    at: usize,
    types: u32,
    type_count: u8,
}

impl CodeReader<'_> {
    fn read_byte(&mut self) -> Result<u8, DecodeError> {
        let byte = *self.code.get(self.at).ok_or(DecodeError::Truncated)?;
        self.at += 1;
        Ok(byte)
    }

    fn read_lowbits(&mut self, bit_count: u8) -> Result<u32, DecodeError> {
        let count = self.type_count;
        let result = self.types;
        if count >= bit_count {
            self.type_count = count - bit_count;
            self.types = result >> bit_count;
            Ok(result)
        } else {
            let fresh = u32::from(self.read_byte()?);
            let needed = bit_count - count;
            self.type_count = TYPE_BITS_PER_BYTE - needed;
            self.types = fresh >> needed;
            Ok(result | (fresh << count))
        }
    }

    fn read_bit(&mut self) -> Result<bool, DecodeError> {
        Ok(self.read_lowbits(1)? & 1 == 1)
    }

    fn unpack_len(&mut self, pack_bits: u8) -> Result<usize, DecodeError> {
        let mask = (1_u32 << pack_bits) - 1;
        let mut value = 0_usize;
        loop {
            let bits = self.read_lowbits(pack_bits + 1)?;
            let group = usize::try_from(bits & mask).unwrap_or(usize::MAX);
            value = (value << pack_bits) + group;
            if bits & (1 << pack_bits) == 0 {
                return Ok(value);
            }
            value += 1;
        }
    }

    fn unpack_dict_pos(&mut self) -> Result<usize, DecodeError> {
        let byte = usize::from(self.read_byte()?);
        if byte < (1 << 7) {
            Ok(byte)
        } else {
            Ok(((byte & 0x7F) | (self.unpack_len(2)? << 7)) + (1 << 7))
        }
    }
}

/// Decompress a stream produced by [`compress`] or the reference encoder.
///
/// # Errors
///
/// Returns [`DecodeError`] for truncated or malformed streams, or when the
/// output would exceed `max_out` bytes.
pub fn decompress(code: &[u8], max_out: usize) -> Result<Vec<u8>, DecodeError> {
    if code.len() < DICT_SIZE_HEADER_LEN {
        return Err(DecodeError::Truncated);
    }
    let mut reader = CodeReader {
        code,
        at: DICT_SIZE_HEADER_LEN,
        types: 0,
        type_count: 0,
    };
    let mut out: Vec<u8> = Vec::new();
    let mut dict_pos_back = 1_usize;
    let mut have_data_back = false;

    loop {
        if reader.read_bit()? {
            if out.len() >= max_out {
                return Err(DecodeError::OutputTooLarge(max_out));
            }
            have_data_back = true;
            let byte = reader.read_byte()?;
            out.push(byte);
            continue;
        }

        let mut saved_len = reader.unpack_len(1)?;
        let saved_pos = if have_data_back && reader.read_bit()? {
            dict_pos_back
        } else {
            let pos = reader.unpack_dict_pos()?;
            if pos > BIG_POS_FOR_LEN {
                saved_len += 1;
            }
            pos
        };
        have_data_back = false;

        if saved_pos != 0 {
            let len = saved_len + MIN_MATCH_LEN;
            dict_pos_back = saved_pos;
            if saved_pos > out.len() {
                return Err(DecodeError::BadDistance);
            }
            if out.len() + len > max_out {
                return Err(DecodeError::OutputTooLarge(max_out));
            }
            let start = out.len() - saved_pos;
            for i in 0..len {
                let byte = out[start + i];
                out.push(byte);
            }
            continue;
        }

        match saved_len {
            CTRL_LITERAL_LINE => {
                let len = reader.unpack_len(2)? + MIN_LITERAL_LINE_LEN;
                let end = reader.at.checked_add(len).ok_or(DecodeError::Truncated)?;
                let bytes = reader
                    .code
                    .get(reader.at..end)
                    .ok_or(DecodeError::Truncated)?;
                if out.len() + len > max_out {
                    return Err(DecodeError::OutputTooLarge(max_out));
                }
                out.extend_from_slice(bytes);
                reader.at = end;
                have_data_back = true;
            }
            CTRL_CLIP_END => {
                dict_pos_back = 1;
                reader.type_count = 0;
            }
            CTRL_STREAM_END => return Ok(out),
            other => return Err(DecodeError::UnknownControl(other)),
        }
    }
}

/// Dictionary size the stream declares.
#[must_use]
pub fn declared_dict_size(code: &[u8]) -> Option<u32> {
    let bytes: [u8; 4] = code.get(..4)?.try_into().ok()?;
    Some(u32::from_le_bytes(bytes))
}
