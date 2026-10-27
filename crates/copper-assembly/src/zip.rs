//! The archives `zipfile.ZipFile(path, "w")` writes with `writestr(info, data)`
//! for deflated entries dated 1980-01-01 00:00 with mode 0644.

use crate::deflate::{crc32, deflate_raw};

/// `ZipInfo.create_system`: 0 (MS-DOS) on Windows, 3 (Unix) elsewhere.
const CREATE_SYSTEM: u8 = if cfg!(windows) { 0 } else { 3 };
const VERSION: u16 = 20;
const DEFLATED: u16 = 8;
/// 1980-01-01 in MS-DOS date format; the time is 00:00:00.
const DOS_DATE: u16 = 1 << 5 | 1;
const EXTERNAL_ATTR: u32 = 0o644 << 16;

/// The archive of `entries` (archive name, content), in the given order.
pub fn archive(entries: &[(String, Vec<u8>)]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut central = Vec::new();
    for (name, data) in entries {
        // `_encodeFilenameFlags`: UTF-8 names set bit 11.
        let flags: u16 = if name.is_ascii() { 0 } else { 0x800 };
        let compressed = deflate_raw(data);
        let crc = crc32(data);
        let offset = out.len() as u32;
        out.extend_from_slice(b"PK\x03\x04");
        out.extend_from_slice(&[VERSION as u8, 0]);
        for value in [flags, DEFLATED, 0, DOS_DATE] {
            out.extend_from_slice(&value.to_le_bytes());
        }
        for value in [crc, compressed.len() as u32, data.len() as u32] {
            out.extend_from_slice(&value.to_le_bytes());
        }
        out.extend_from_slice(&(name.len() as u16).to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(name.as_bytes());
        out.extend_from_slice(&compressed);

        central.extend_from_slice(b"PK\x01\x02");
        central.extend_from_slice(&[VERSION as u8, CREATE_SYSTEM, VERSION as u8, 0]);
        for value in [flags, DEFLATED, 0, DOS_DATE] {
            central.extend_from_slice(&value.to_le_bytes());
        }
        for value in [crc, compressed.len() as u32, data.len() as u32] {
            central.extend_from_slice(&value.to_le_bytes());
        }
        // Name, extra and comment lengths, disk number, internal attributes.
        for value in [name.len() as u16, 0, 0, 0, 0] {
            central.extend_from_slice(&value.to_le_bytes());
        }
        central.extend_from_slice(&EXTERNAL_ATTR.to_le_bytes());
        central.extend_from_slice(&offset.to_le_bytes());
        central.extend_from_slice(name.as_bytes());
    }
    let start = out.len() as u32;
    out.extend_from_slice(&central);
    out.extend_from_slice(b"PK\x05\x06");
    let count = entries.len() as u16;
    for value in [0, 0, count, count] {
        out.extend_from_slice(&value.to_le_bytes());
    }
    out.extend_from_slice(&(central.len() as u32).to_le_bytes());
    out.extend_from_slice(&start.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out
}
