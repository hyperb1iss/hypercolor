use std::io::{Read, Seek, SeekFrom};

use sha2::{Digest as _, Sha256};

use super::super::InstallPlatformError;
use super::model::error;

const MACH_HEADER_64_BYTES: u64 = 32;
const MH_MAGIC_64: u32 = 0xfeed_facf;
const CPU_TYPE_X86_64: u32 = 0x0100_0007;
const CPU_TYPE_ARM64: u32 = 0x0100_000c;
const LC_CODE_SIGNATURE: u32 = 0x1d;
const LINKEDIT_DATA_COMMAND_BYTES: u32 = 16;
const CSMAGIC_EMBEDDED_SIGNATURE: u32 = 0xfade_0cc0;
const CSMAGIC_CODEDIRECTORY: u32 = 0xfade_0c02;
const CSSLOT_CODEDIRECTORY: u32 = 0;
const CSSLOT_ALTERNATE_CODEDIRECTORIES: std::ops::RangeInclusive<u32> = 0x1000..=0x1005;
const CS_HASHTYPE_SHA256: u8 = 2;
const CS_HASHTYPE_SHA256_TRUNCATED: u8 = 3;
const MAX_LOAD_COMMANDS: u32 = 4096;
const MAX_LOAD_COMMAND_BYTES: u64 = 4 * 1024 * 1024;
const MAX_CODE_SIGNATURE_BYTES: u64 = 16 * 1024 * 1024;
const MAX_CODE_DIRECTORIES: usize = 7;

pub(crate) fn thin_macho_cdhash(
    reader: &mut (impl Read + Seek),
    file_size: u64,
) -> Result<String, InstallPlatformError> {
    if file_size < MACH_HEADER_64_BYTES {
        return Err(error(
            "macOS executable is smaller than a thin Mach-O header",
        ));
    }
    let header = read_exact_at(reader, 0, MACH_HEADER_64_BYTES, file_size)?;
    if le_u32(&header, 0)? != MH_MAGIC_64 {
        return Err(error(
            "macOS executable is not a native-endian thin 64-bit Mach-O",
        ));
    }
    if le_u32(&header, 4)? != native_cpu_type()? {
        return Err(error(
            "macOS executable architecture does not match this process",
        ));
    }
    let command_count = le_u32(&header, 16)?;
    let command_bytes = u64::from(le_u32(&header, 20)?);
    if command_count == 0
        || command_count > MAX_LOAD_COMMANDS
        || command_bytes > MAX_LOAD_COMMAND_BYTES
    {
        return Err(error("macOS executable load commands exceed their bounds"));
    }
    let commands_end = MACH_HEADER_64_BYTES
        .checked_add(command_bytes)
        .filter(|end| *end <= file_size)
        .ok_or_else(|| error("macOS executable load commands exceed the file"))?;
    let commands = read_exact_at(reader, MACH_HEADER_64_BYTES, command_bytes, file_size)?;
    let mut cursor = 0_usize;
    let mut signature = None;
    for _ in 0..command_count {
        let command = le_u32(&commands, cursor)?;
        let size = le_u32(&commands, cursor + 4)?;
        if size < 8 || size % 4 != 0 {
            return Err(error("macOS executable has a malformed load command"));
        }
        let next = cursor
            .checked_add(size as usize)
            .filter(|next| *next <= commands.len())
            .ok_or_else(|| error("macOS executable load command exceeds its table"))?;
        if command == LC_CODE_SIGNATURE {
            if size != LINKEDIT_DATA_COMMAND_BYTES || signature.is_some() {
                return Err(error(
                    "macOS executable has an ambiguous code signature command",
                ));
            }
            let offset = u64::from(le_u32(&commands, cursor + 8)?);
            let length = u64::from(le_u32(&commands, cursor + 12)?);
            if offset < commands_end
                || length == 0
                || length > MAX_CODE_SIGNATURE_BYTES
                || offset.checked_add(length) != Some(file_size)
            {
                return Err(error(
                    "macOS executable code signature range is not canonical",
                ));
            }
            signature = Some((offset, length));
        }
        cursor = next;
    }
    if cursor != commands.len() {
        return Err(error("macOS executable load command count is inconsistent"));
    }
    let (offset, length) = signature
        .ok_or_else(|| error("macOS executable has no embedded code signature command"))?;
    let signature = read_exact_at(reader, offset, length, file_size)?;
    cdhash_from_superblob(&signature)
}

fn cdhash_from_superblob(signature: &[u8]) -> Result<String, InstallPlatformError> {
    if be_u32(signature, 0)? != CSMAGIC_EMBEDDED_SIGNATURE {
        return Err(error(
            "macOS embedded code signature is not a canonical superblob",
        ));
    }
    let blob_length = be_u32(signature, 4)? as usize;
    if blob_length < 12
        || blob_length > signature.len()
        || signature[blob_length..].iter().any(|byte| *byte != 0)
    {
        return Err(error(
            "macOS embedded code signature has noncanonical padding",
        ));
    }
    let signature = &signature[..blob_length];
    let count = be_u32(signature, 8)? as usize;
    if count == 0 || count > 64 {
        return Err(error("macOS embedded signature index exceeds its bound"));
    }
    let index_end = 12_usize
        .checked_add(
            count
                .checked_mul(8)
                .ok_or_else(|| error("macOS embedded signature index overflowed"))?,
        )
        .filter(|end| *end <= signature.len())
        .ok_or_else(|| error("macOS embedded signature index exceeds the blob"))?;
    let mut seen_slots = std::collections::BTreeSet::new();
    let mut ranges = Vec::with_capacity(count);
    let mut code_directories = Vec::new();
    for index in 0..count {
        let entry = 12 + index * 8;
        let slot = be_u32(signature, entry)?;
        let offset = be_u32(signature, entry + 4)? as usize;
        if !seen_slots.insert(slot) || offset < index_end {
            return Err(error(
                "macOS embedded signature index is duplicate or overlapping",
            ));
        }
        let length = be_u32(signature, offset + 4)? as usize;
        let end = offset
            .checked_add(length)
            .filter(|end| length >= 8 && *end <= signature.len())
            .ok_or_else(|| error("macOS embedded signature member exceeds the blob"))?;
        ranges.push(offset..end);
        if slot == CSSLOT_CODEDIRECTORY || CSSLOT_ALTERNATE_CODEDIRECTORIES.contains(&slot) {
            if be_u32(signature, offset)? != CSMAGIC_CODEDIRECTORY
                || length < 40
                || code_directories.len() >= MAX_CODE_DIRECTORIES
            {
                return Err(error("macOS embedded CodeDirectory is malformed"));
            }
            let hash_size = signature[offset + 36];
            let hash_type = signature[offset + 37];
            if matches!(
                (hash_type, hash_size),
                (CS_HASHTYPE_SHA256, 32) | (CS_HASHTYPE_SHA256_TRUNCATED, 20)
            ) {
                code_directories.push(&signature[offset..end]);
            }
        }
    }
    ranges.sort_by_key(|range| range.start);
    if ranges.windows(2).any(|pair| pair[0].end > pair[1].start) {
        return Err(error("macOS embedded signature members overlap"));
    }
    let [code_directory] = code_directories.as_slice() else {
        return Err(error(
            "macOS embedded signature lacks one canonical SHA-256 CodeDirectory",
        ));
    };
    let digest = Sha256::digest(code_directory);
    Ok(hex_encode(&digest[..20]))
}

fn read_exact_at(
    reader: &mut (impl Read + Seek),
    offset: u64,
    length: u64,
    file_size: u64,
) -> Result<Vec<u8>, InstallPlatformError> {
    let _end = offset
        .checked_add(length)
        .filter(|end| *end <= file_size)
        .ok_or_else(|| error("macOS executable read exceeds the retained file"))?;
    let capacity = usize::try_from(length)
        .map_err(|_| error("macOS executable region does not fit in memory"))?;
    reader.seek(SeekFrom::Start(offset)).map_err(io_error)?;
    let mut bytes = vec![0; capacity];
    reader.read_exact(&mut bytes).map_err(io_error)?;
    Ok(bytes)
}

fn native_cpu_type() -> Result<u32, InstallPlatformError> {
    match std::env::consts::ARCH {
        "aarch64" => Ok(CPU_TYPE_ARM64),
        "x86_64" => Ok(CPU_TYPE_X86_64),
        _ => Err(error("unsupported macOS executable architecture")),
    }
}

fn le_u32(bytes: &[u8], offset: usize) -> Result<u32, InstallPlatformError> {
    Ok(u32::from_le_bytes(
        bytes
            .get(offset..offset + 4)
            .ok_or_else(|| error("macOS executable structure is truncated"))?
            .try_into()
            .expect("four-byte slice"),
    ))
}

fn be_u32(bytes: &[u8], offset: usize) -> Result<u32, InstallPlatformError> {
    Ok(u32::from_be_bytes(
        bytes
            .get(offset..offset + 4)
            .ok_or_else(|| error("macOS code signature structure is truncated"))?
            .try_into()
            .expect("four-byte slice"),
    ))
}

fn io_error(source: std::io::Error) -> InstallPlatformError {
    error(source.to_string())
}

fn hex_encode(bytes: &[u8]) -> String {
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write as _;
        write!(&mut output, "{byte:02x}").expect("writing into a String cannot fail");
    }
    output
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use sha2::{Digest as _, Sha256};

    use super::{CSMAGIC_CODEDIRECTORY, CSMAGIC_EMBEDDED_SIGNATURE, thin_macho_cdhash};

    fn thin_macho(hash_type: u8, hash_size: u8) -> (Vec<u8>, String) {
        let mut code_directory = vec![0_u8; 40];
        code_directory[0..4].copy_from_slice(&CSMAGIC_CODEDIRECTORY.to_be_bytes());
        code_directory[4..8].copy_from_slice(&40_u32.to_be_bytes());
        code_directory[36] = hash_size;
        code_directory[37] = hash_type;
        let mut signature = vec![0_u8; 20];
        signature[0..4].copy_from_slice(&CSMAGIC_EMBEDDED_SIGNATURE.to_be_bytes());
        signature[4..8].copy_from_slice(&60_u32.to_be_bytes());
        signature[8..12].copy_from_slice(&1_u32.to_be_bytes());
        signature[12..16].copy_from_slice(&0_u32.to_be_bytes());
        signature[16..20].copy_from_slice(&20_u32.to_be_bytes());
        signature.extend_from_slice(&code_directory);
        let mut macho = vec![0_u8; 48];
        macho[0..4].copy_from_slice(&super::MH_MAGIC_64.to_le_bytes());
        macho[4..8].copy_from_slice(
            &super::native_cpu_type()
                .expect("native test architecture")
                .to_le_bytes(),
        );
        macho[16..20].copy_from_slice(&1_u32.to_le_bytes());
        macho[20..24].copy_from_slice(&16_u32.to_le_bytes());
        macho[32..36].copy_from_slice(&super::LC_CODE_SIGNATURE.to_le_bytes());
        macho[36..40].copy_from_slice(&16_u32.to_le_bytes());
        macho[40..44].copy_from_slice(&48_u32.to_le_bytes());
        macho[44..48].copy_from_slice(&60_u32.to_le_bytes());
        macho.extend_from_slice(&signature);
        let digest = Sha256::digest(code_directory);
        (macho, super::hex_encode(&digest[..20]))
    }

    #[test]
    fn parses_one_architecture_exact_sha256_code_directory() {
        let (macho, expected) = thin_macho(2, 32);
        assert_eq!(
            thin_macho_cdhash(&mut Cursor::new(&macho), macho.len() as u64)
                .expect("canonical thin Mach-O"),
            expected
        );
    }

    #[test]
    fn rejects_wrong_architecture_signature_kind_and_truncation() {
        let (mut wrong_arch, _) = thin_macho(2, 32);
        wrong_arch[4..8].copy_from_slice(&0_u32.to_le_bytes());
        assert!(thin_macho_cdhash(&mut Cursor::new(&wrong_arch), wrong_arch.len() as u64).is_err());

        let (sha1, _) = thin_macho(1, 20);
        assert!(thin_macho_cdhash(&mut Cursor::new(&sha1), sha1.len() as u64).is_err());

        let (truncated, _) = thin_macho(2, 32);
        assert!(
            thin_macho_cdhash(
                &mut Cursor::new(&truncated[..truncated.len() - 1]),
                truncated.len() as u64 - 1,
            )
            .is_err()
        );
    }

    #[test]
    fn rejects_noncanonical_and_oversized_regions_before_allocation() {
        let (mut oversized_commands, _) = thin_macho(2, 32);
        oversized_commands[20..24]
            .copy_from_slice(&(super::MAX_LOAD_COMMAND_BYTES as u32 + 1).to_le_bytes());
        assert!(
            thin_macho_cdhash(
                &mut Cursor::new(&oversized_commands),
                oversized_commands.len() as u64,
            )
            .is_err()
        );

        let (mut oversized_signature, _) = thin_macho(2, 32);
        oversized_signature[44..48]
            .copy_from_slice(&(super::MAX_CODE_SIGNATURE_BYTES as u32 + 1).to_le_bytes());
        assert!(
            thin_macho_cdhash(
                &mut Cursor::new(&oversized_signature),
                48 + super::MAX_CODE_SIGNATURE_BYTES + 1,
            )
            .is_err()
        );

        let (mut trailing_bytes, _) = thin_macho(2, 32);
        trailing_bytes.push(0);
        assert!(
            thin_macho_cdhash(
                &mut Cursor::new(&trailing_bytes),
                trailing_bytes.len() as u64
            )
            .is_err()
        );
    }

    #[test]
    fn accepts_only_zero_filled_linkedit_padding() {
        let (mut padded, expected) = thin_macho(2, 32);
        padded[44..48].copy_from_slice(&64_u32.to_le_bytes());
        padded.extend_from_slice(&[0; 4]);
        assert_eq!(
            thin_macho_cdhash(&mut Cursor::new(&padded), padded.len() as u64)
                .expect("zero-filled code signature padding"),
            expected
        );

        *padded.last_mut().expect("padding byte") = 1;
        assert!(thin_macho_cdhash(&mut Cursor::new(&padded), padded.len() as u64).is_err());
    }
}
