//! Decoding report bytes as Python does before parsing them.

use crate::py::{Py, PyErr};

/// Python's message for a failed strict `bytes.decode("utf-8")`.
pub fn utf8_error(data: &[u8], error: std::str::Utf8Error, codec: &str) -> PyErr {
    let start = error.valid_up_to();
    let count = error.error_len().unwrap_or(data.len() - start);
    let reason = match error.error_len() {
        None => "unexpected end of data",
        Some(_) if (0xc2..=0xf4).contains(&data[start]) => "invalid continuation byte",
        Some(_) => "invalid start byte",
    };
    let what = if count > 1 {
        format!("bytes in position {start}-{}", start + count - 1)
    } else {
        format!("byte 0x{:02x} in position {start}", data[start])
    };
    PyErr::new("UnicodeDecodeError", format!("'{codec}' codec can't decode {what}: {reason}"))
}

/// `bytes.decode("utf-8")`.
pub fn decode_utf8(data: &[u8]) -> Py<String> {
    std::str::from_utf8(data).map(str::to_owned).map_err(|error| utf8_error(data, error, "utf-8"))
}

fn decode_wide(data: &[u8], codec: &str, width: usize, big_endian: bool, skip_bom: bool) -> Py<String> {
    let unit = |chunk: &[u8]| -> u32 {
        let mut value = 0u32;
        for index in 0..width {
            let byte = u32::from(if big_endian { chunk[index] } else { chunk[width - 1 - index] });
            value = (value << 8) | byte;
        }
        value
    };
    let whole = data.len() / width * width;
    let mut units: Vec<u32> = data[..whole].chunks(width).map(unit).collect();
    if skip_bom && units.first() == Some(&0xfeff) {
        units.remove(0);
    }
    let failed = |position: usize, reason: &str| {
        PyErr::new(
            "UnicodeDecodeError",
            format!("'{codec}' codec can't decode bytes in position {position}-{}: {reason}", data.len() - 1),
        )
    };
    let text: Option<String> = if width == 2 {
        let words: Vec<u16> = units.iter().map(|unit| *unit as u16).collect();
        char::decode_utf16(words).collect::<Result<String, _>>().ok()
    } else {
        units.iter().map(|unit| char::from_u32(*unit)).collect()
    };
    let text = text.ok_or_else(|| failed(0, "illegal encoding"))?;
    if whole != data.len() {
        return Err(failed(whole, "truncated data"));
    }
    Ok(text)
}

/// `json.loads(bytes)`'s `detect_encoding` and decode.
pub fn decode_json_bytes(data: &[u8]) -> Py<String> {
    let starts = |prefix: &[u8]| data.starts_with(prefix);
    if starts(&[0, 0, 0xfe, 0xff]) {
        return decode_wide(data, "utf-32", 4, true, true);
    }
    if starts(&[0xff, 0xfe, 0, 0]) {
        return decode_wide(data, "utf-32", 4, false, true);
    }
    if starts(&[0xfe, 0xff]) {
        return decode_wide(data, "utf-16", 2, true, true);
    }
    if starts(&[0xff, 0xfe]) {
        return decode_wide(data, "utf-16", 2, false, true);
    }
    if let Some(rest) = data.strip_prefix(&[0xef, 0xbb, 0xbf]) {
        // Positions count from after the BOM.
        return decode_utf8(rest);
    }
    if data.len() >= 4 {
        if data[0] == 0 {
            return if data[1] != 0 {
                decode_wide(data, "utf-16-be", 2, true, false)
            } else {
                decode_wide(data, "utf-32-be", 4, true, false)
            };
        }
        if data[1] == 0 {
            return if data[2] != 0 || data[3] != 0 {
                decode_wide(data, "utf-16-le", 2, false, false)
            } else {
                decode_wide(data, "utf-32-le", 4, false, false)
            };
        }
    } else if data.len() == 2 {
        if data[0] == 0 {
            return decode_wide(data, "utf-16-be", 2, true, false);
        }
        if data[1] == 0 {
            return decode_wide(data, "utf-16-le", 2, false, false);
        }
    }
    decode_utf8(data)
}
