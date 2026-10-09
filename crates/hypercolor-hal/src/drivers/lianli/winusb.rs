//! The DES-CBC header that opens every command to a wired Lian Li WinUSB
//! LCD (spec 84 section 3).
//!
//! L-Connect drives its wired `0x1CBE` panels (Universal Screen, Vision,
//! HydroShift II LCD, Lancool 207) through a transport class it calls
//! "WinUsb". Its header differs from the wireless receivers' in
//! [`super::wireless::crypto`] in one respect: the plaintext is 500 bytes,
//! not 504. PKCS#7 pads 500 to 504, so the ciphertext no longer fills the
//! 512-byte header; bytes 504 to 509 stay zero and the last two carry a
//! fixed `A1 1A` trailer the firmware checks.
//!
//! The key, magic, and timestamp rules are the receivers' own, and are
//! reused from there. This is obfuscation, not security: the key ships in
//! every L-Connect install and in several public repositories.

use cbc::Encryptor;
use des::Des;
use des::cipher::block_padding::Pkcs7;
use des::cipher::{BlockModeEncrypt, KeyIvInit};
use zerocopy::byteorder::{LittleEndian, U32};
use zerocopy::{FromZeros, Immutable, IntoBytes, KnownLayout};

use super::wireless::crypto::{DES_KEY, HeaderBuilder, MAGIC, PARAMS_OFFSET};

/// Plaintext bytes ahead of padding.
pub const WINUSB_PLAINTEXT_LEN: usize = 500;
/// Ciphertext bytes: the plaintext plus four bytes of PKCS#7 padding.
pub const WINUSB_CIPHERTEXT_LEN: usize = 504;
/// On-wire header length.
pub const WINUSB_HEADER_LEN: usize = 512;
/// Parameter bytes the plaintext can carry after its eight-byte preamble.
pub const WINUSB_MAX_PARAMS_LEN: usize = WINUSB_PLAINTEXT_LEN - PARAMS_OFFSET;
/// The two bytes that close every header.
pub const WINUSB_TRAILER: [u8; 2] = [0xA1, 0x1A];

/// The plaintext a header encrypts (500 bytes).
#[derive(FromZeros, IntoBytes, KnownLayout, Immutable)]
#[repr(C)]
struct WinUsbPlaintext {
    /// Command byte.
    command: u8,
    /// Always zero.
    reserved: u8,
    /// `1A 6D`.
    magic: [u8; 2],
    /// Milliseconds since the session began, strictly increasing.
    timestamp: U32<LittleEndian>,
    /// Command parameters, zero-padded.
    params: [u8; WINUSB_MAX_PARAMS_LEN],
}

const _: () = assert!(
    size_of::<WinUsbPlaintext>() == WINUSB_PLAINTEXT_LEN,
    "WinUsbPlaintext must match the 500-byte plaintext"
);

/// The header as it goes on the wire (512 bytes).
#[derive(FromZeros, IntoBytes, KnownLayout, Immutable)]
#[repr(C)]
struct WinUsbHeader {
    /// DES-CBC ciphertext of the plaintext, PKCS#7 padded.
    ciphertext: [u8; WINUSB_CIPHERTEXT_LEN],
    /// Always zero.
    reserved: [u8; 6],
    /// [`WINUSB_TRAILER`].
    trailer: [u8; 2],
}

const _: () = assert!(
    size_of::<WinUsbHeader>() == WINUSB_HEADER_LEN,
    "WinUsbHeader must match the 512-byte wire header"
);

/// Build the 512-byte header for `command` with `params` at a given
/// timestamp. Parameters past [`WINUSB_MAX_PARAMS_LEN`] are dropped.
#[must_use]
pub fn wrap_winusb_header(command: u8, timestamp: u32, params: &[u8]) -> [u8; WINUSB_HEADER_LEN] {
    let mut plaintext = WinUsbPlaintext::new_zeroed();
    plaintext.command = command;
    plaintext.magic = MAGIC;
    plaintext.timestamp = U32::new(timestamp);
    let len = params.len().min(WINUSB_MAX_PARAMS_LEN);
    plaintext.params[..len].copy_from_slice(&params[..len]);

    let mut header = WinUsbHeader::new_zeroed();
    header.ciphertext[..WINUSB_PLAINTEXT_LEN].copy_from_slice(plaintext.as_bytes());
    let encryptor =
        Encryptor::<Des>::new_from_slices(&DES_KEY, &DES_KEY).expect("an eight-byte key and IV");
    let written = encryptor
        .encrypt_padded::<Pkcs7>(&mut header.ciphertext, WINUSB_PLAINTEXT_LEN)
        .expect("504 bytes hold the 500-byte plaintext plus its four padding bytes")
        .len();
    debug_assert_eq!(written, WINUSB_CIPHERTEXT_LEN);
    header.trailer = WINUSB_TRAILER;

    let mut wire = [0_u8; WINUSB_HEADER_LEN];
    wire.copy_from_slice(header.as_bytes());
    wire
}

/// Issues WinUSB headers on the session clock the firmware expects.
#[derive(Debug, Default)]
pub struct WinUsbHeaderBuilder {
    clock: HeaderBuilder,
}

impl WinUsbHeaderBuilder {
    /// A builder whose clock starts now.
    #[must_use]
    pub fn new() -> Self {
        Self {
            clock: HeaderBuilder::new(),
        }
    }

    /// A header for `command` at the next timestamp.
    pub fn header(&mut self, command: u8, params: &[u8]) -> [u8; WINUSB_HEADER_LEN] {
        let timestamp = self.clock.next_timestamp();
        wrap_winusb_header(command, timestamp, params)
    }
}
