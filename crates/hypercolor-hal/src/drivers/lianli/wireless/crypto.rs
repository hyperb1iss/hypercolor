//! The DES-CBC header every command to a wireless LCD receiver starts with
//! (spec 80 section 7.1).
//!
//! A 504-byte plaintext (command, magic, a little-endian millisecond
//! timestamp, up to 496 parameter bytes) is encrypted with DES in CBC mode
//! under a fixed eight-byte key that doubles as the IV, with PKCS#7 padding.
//! 504 is block-aligned, so the padding is one full block and the ciphertext
//! is exactly 512 bytes. This is obfuscation, not security: the key ships in
//! every L-Connect install and in several public repositories.

use cbc::Encryptor;
use des::Des;
use des::cipher::block_padding::Pkcs7;
use des::cipher::{BlockModeEncrypt, KeyIvInit};

/// The key, which is also the IV.
pub const DES_KEY: [u8; 8] = *b"slv3tuzx";
/// Plaintext bytes ahead of padding.
pub const PLAINTEXT_LEN: usize = 504;
/// Ciphertext bytes: the plaintext plus one block of PKCS#7 padding.
pub const HEADER_LEN: usize = 512;
/// Where the command's parameters start in the plaintext.
pub const PARAMS_OFFSET: usize = 8;
/// Parameter bytes the plaintext can carry.
pub const MAX_PARAMS_LEN: usize = PLAINTEXT_LEN - PARAMS_OFFSET;
/// The two magic bytes at plaintext offsets 2 and 3.
pub const MAGIC: [u8; 2] = [0x1A, 0x6D];

/// Build the 512-byte encrypted header for `command` with `params` at a
/// given timestamp. Parameters past [`MAX_PARAMS_LEN`] are dropped.
#[must_use]
pub fn wrap_header(command: u8, timestamp: u32, params: &[u8]) -> [u8; HEADER_LEN] {
    let mut buffer = [0_u8; HEADER_LEN];
    buffer[0] = command;
    buffer[2..4].copy_from_slice(&MAGIC);
    buffer[4..8].copy_from_slice(&timestamp.to_le_bytes());
    let len = params.len().min(MAX_PARAMS_LEN);
    buffer[PARAMS_OFFSET..PARAMS_OFFSET + len].copy_from_slice(&params[..len]);

    let encryptor =
        Encryptor::<Des>::new_from_slices(&DES_KEY, &DES_KEY).expect("an eight-byte key and IV");
    let written = encryptor
        .encrypt_padded::<Pkcs7>(&mut buffer, PLAINTEXT_LEN)
        .expect("a 512-byte buffer holds 504 bytes plus one padding block")
        .len();
    debug_assert_eq!(written, HEADER_LEN);
    buffer
}

/// Issues the strictly increasing timestamps the receiver expects.
///
/// The value is milliseconds since the builder was made; when two headers
/// land in the same millisecond the second one is bumped by one, so no two
/// commands of a session ever share a timestamp.
#[derive(Debug)]
pub struct HeaderBuilder {
    started_at: std::time::Instant,
    last_timestamp: u32,
}

impl Default for HeaderBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl HeaderBuilder {
    /// A builder whose clock starts now.
    #[must_use]
    pub fn new() -> Self {
        Self {
            started_at: std::time::Instant::now(),
            last_timestamp: 0,
        }
    }

    /// The next timestamp: the clock reading, or one past the last issued.
    pub fn next_timestamp(&mut self) -> u32 {
        let raw = u32::try_from(self.started_at.elapsed().as_millis()).unwrap_or(u32::MAX);
        let timestamp = if raw <= self.last_timestamp {
            self.last_timestamp.wrapping_add(1)
        } else {
            raw
        };
        self.last_timestamp = timestamp;
        timestamp
    }

    /// A header for `command` at the next timestamp.
    pub fn header(&mut self, command: u8, params: &[u8]) -> [u8; HEADER_LEN] {
        let timestamp = self.next_timestamp();
        wrap_header(command, timestamp, params)
    }
}
