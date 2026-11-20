//! One member of a ZIP archive: the central directory, stored and DEFLATE
//! (RFC 1951) members. Vendors such as Texas Instruments ship SPICE models
//! only inside ZIP archives, so a `sim spice` block may name the archive by
//! URL and hash and the model file inside it (`member`).

/// The bytes of the member whose name (or whose last path component) equals
/// `name`, ignoring ASCII case.
pub fn zip_member(archive: &[u8], name: &str) -> Result<Vec<u8>, String> {
    let le16 = |at: usize| -> Result<usize, String> {
        archive
            .get(at..at + 2)
            .map(|b| u16::from_le_bytes([b[0], b[1]]) as usize)
            .ok_or_else(|| "truncated ZIP archive".to_string())
    };
    let le32 = |at: usize| -> Result<usize, String> {
        archive
            .get(at..at + 4)
            .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as usize)
            .ok_or_else(|| "truncated ZIP archive".to_string())
    };
    if archive.len() < 22 {
        return Err("not a ZIP archive".into());
    }
    let lowest = archive.len().saturating_sub(22 + 65_535);
    let eocd = (lowest..=archive.len() - 22)
        .rev()
        .find(|&i| archive[i..i + 4] == [0x50, 0x4b, 0x05, 0x06])
        .ok_or("not a ZIP archive (no end of central directory)")?;
    let count = le16(eocd + 10)?;
    let mut at = le32(eocd + 16)?;
    let wanted = name.to_ascii_lowercase();
    let mut names = Vec::new();
    for _ in 0..count {
        if le32(at)? != 0x0201_4b50 {
            return Err("corrupt ZIP central directory".into());
        }
        let method = le16(at + 10)?;
        let compressed = le32(at + 20)?;
        let size = le32(at + 24)?;
        let name_len = le16(at + 28)?;
        let extra_len = le16(at + 30)?;
        let comment_len = le16(at + 32)?;
        let local = le32(at + 42)?;
        let member = String::from_utf8_lossy(archive.get(at + 46..at + 46 + name_len).ok_or("truncated ZIP archive")?)
            .into_owned();
        at += 46 + name_len + extra_len + comment_len;
        let lower = member.to_ascii_lowercase();
        let leaf = lower.rsplit('/').next().unwrap_or(&lower).to_string();
        if lower != wanted && leaf != wanted {
            names.push(member);
            continue;
        }
        if le32(local)? != 0x0403_4b50 {
            return Err(format!("corrupt ZIP local header for {member}"));
        }
        let start = local + 30 + le16(local + 26)? + le16(local + 28)?;
        let data = archive.get(start..start + compressed).ok_or("truncated ZIP member")?;
        let bytes = match method {
            0 => data.to_vec(),
            8 => inflate(data, size)?,
            other => {
                return Err(format!(
                    "ZIP member {member} uses compression method {other}; only stored and DEFLATE are supported"
                ));
            }
        };
        if bytes.len() != size {
            return Err(format!("ZIP member {member} inflated to {} bytes, not {size}", bytes.len()));
        }
        return Ok(bytes);
    }
    Err(format!("the archive has no member {name}; it holds {}", names.join(", ")))
}

struct Bits<'a> {
    data: &'a [u8],
    pos: usize,
    bit: u32,
    bits: u32,
}

impl Bits<'_> {
    fn need(&mut self, n: u32) -> Result<(), String> {
        while self.bits < n {
            let byte = *self.data.get(self.pos).ok_or("truncated DEFLATE stream")?;
            self.pos += 1;
            self.bit |= u32::from(byte) << self.bits;
            self.bits += 8;
        }
        Ok(())
    }
    fn take(&mut self, n: u32) -> Result<u32, String> {
        if n == 0 {
            return Ok(0);
        }
        self.need(n)?;
        let v = self.bit & ((1u32 << n) - 1);
        self.bit >>= n;
        self.bits -= n;
        Ok(v)
    }
}

/// A canonical Huffman code: counts per length and symbols in code order.
struct Huffman {
    counts: [u16; 16],
    symbols: Vec<u16>,
}

impl Huffman {
    fn new(lengths: &[u8]) -> Huffman {
        let mut counts = [0u16; 16];
        for &l in lengths {
            counts[l as usize] += 1;
        }
        counts[0] = 0;
        let mut offsets = [0u16; 16];
        for l in 1..16 {
            offsets[l] = offsets[l - 1] + counts[l - 1];
        }
        let mut symbols = vec![0u16; lengths.len()];
        for (s, &l) in lengths.iter().enumerate() {
            if l != 0 {
                symbols[offsets[l as usize] as usize] = s as u16;
                offsets[l as usize] += 1;
            }
        }
        Huffman { counts, symbols }
    }

    fn decode(&self, b: &mut Bits) -> Result<u16, String> {
        let (mut code, mut first, mut index) = (0i32, 0i32, 0i32);
        for len in 1..16 {
            code |= b.take(1)? as i32;
            let count = i32::from(self.counts[len]);
            if code - count < first {
                return self
                    .symbols
                    .get((index + code - first) as usize)
                    .copied()
                    .ok_or_else(|| "bad Huffman code".to_string());
            }
            index += count;
            first += count;
            first <<= 1;
            code <<= 1;
        }
        Err("bad Huffman code in DEFLATE stream".into())
    }
}

const LENGTH_BASE: [u16; 29] =
    [3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51, 59, 67, 83, 99, 115, 131, 163, 195, 227, 258];
const LENGTH_EXTRA: [u8; 29] = [0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0];
const DIST_BASE: [u16; 30] = [
    1, 2, 3, 4, 5, 7, 9, 13, 17, 25, 33, 49, 65, 97, 129, 193, 257, 385, 513, 769, 1025, 1537, 2049, 3073, 4097, 6145,
    8193, 12289, 16385, 24577,
];
const DIST_EXTRA: [u8; 30] =
    [0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13, 13];

/// Inflate a raw DEFLATE stream (RFC 1951).
pub fn inflate(data: &[u8], size_hint: usize) -> Result<Vec<u8>, String> {
    let mut out: Vec<u8> = Vec::with_capacity(size_hint);
    let mut b = Bits { data, pos: 0, bit: 0, bits: 0 };
    loop {
        let last = b.take(1)?;
        match b.take(2)? {
            0 => {
                b.bit = 0;
                b.bits = 0;
                let header = data.get(b.pos..b.pos + 4).ok_or("truncated stored block")?;
                let len = usize::from(u16::from_le_bytes([header[0], header[1]]));
                b.pos += 4;
                out.extend_from_slice(data.get(b.pos..b.pos + len).ok_or("truncated stored block")?);
                b.pos += len;
            }
            kind @ (1 | 2) => {
                let (lit, dist) = if kind == 1 {
                    let mut l = [8u8; 288];
                    l[144..256].fill(9);
                    l[256..280].fill(7);
                    (Huffman::new(&l), Huffman::new(&[5u8; 30]))
                } else {
                    let hlit = b.take(5)? as usize + 257;
                    let hdist = b.take(5)? as usize + 1;
                    let hclen = b.take(4)? as usize + 4;
                    const ORDER: [usize; 19] = [16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15];
                    let mut cl = [0u8; 19];
                    for &o in ORDER.iter().take(hclen) {
                        cl[o] = b.take(3)? as u8;
                    }
                    let code = Huffman::new(&cl);
                    let mut lengths = Vec::with_capacity(hlit + hdist);
                    while lengths.len() < hlit + hdist {
                        let sym = code.decode(&mut b)?;
                        match sym {
                            0..=15 => lengths.push(sym as u8),
                            16 => {
                                let prev = *lengths.last().ok_or("repeat with no previous length")?;
                                for _ in 0..3 + b.take(2)? {
                                    lengths.push(prev);
                                }
                            }
                            17 => lengths.extend(std::iter::repeat_n(0, 3 + b.take(3)? as usize)),
                            _ => lengths.extend(std::iter::repeat_n(0, 11 + b.take(7)? as usize)),
                        }
                    }
                    if lengths.len() > hlit + hdist {
                        return Err("bad code lengths in DEFLATE stream".into());
                    }
                    (Huffman::new(&lengths[..hlit]), Huffman::new(&lengths[hlit..]))
                };
                loop {
                    let sym = lit.decode(&mut b)? as usize;
                    if sym < 256 {
                        out.push(sym as u8);
                    } else if sym == 256 {
                        break;
                    } else {
                        let i = sym - 257;
                        if i >= 29 {
                            return Err("bad length code".into());
                        }
                        let len = usize::from(LENGTH_BASE[i]) + b.take(u32::from(LENGTH_EXTRA[i]))? as usize;
                        let d = dist.decode(&mut b)? as usize;
                        if d >= 30 {
                            return Err("bad distance code".into());
                        }
                        let distance = usize::from(DIST_BASE[d]) + b.take(u32::from(DIST_EXTRA[d]))? as usize;
                        if distance > out.len() {
                            return Err("DEFLATE distance before the start".into());
                        }
                        let start = out.len() - distance;
                        for k in 0..len {
                            out.push(out[start + k]);
                        }
                    }
                }
            }
            _ => return Err("bad DEFLATE block type".into()),
        }
        if last == 1 {
            return Ok(out);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stored_and_fixed_huffman_blocks() {
        // "abc" stored.
        assert_eq!(inflate(&[0x01, 0x03, 0x00, 0xfc, 0xff, b'a', b'b', b'c'], 3).unwrap(), b"abc");
        // "hello hello hello" with fixed codes and a back reference (zlib -15 output).
        let fixed = [0xcb, 0x48, 0xcd, 0xc9, 0xc9, 0x57, 0xc8, 0x40, 0x90, 0x00];
        assert_eq!(inflate(&fixed, 17).unwrap(), b"hello hello hello");
    }

    #[test]
    fn dynamic_huffman_block() {
        // zlib (level 9, raw) of the text below: one dynamic-Huffman block.
        let hex = [
            "65d6416a1d311004d07d4ed14718a95b23891c21500bdfc06087040206dbf72735bd2b35b8760fcc9faa91e6e532f0af",
            "59fb653fedf3fdebefd7f7c7a75df6f1dbbeffbcdbbfd7b7b7f7cf1f2fed31e8e64b5c3b5d7f10dce610d74fe70f4258",
            "53e7a78b0761d8e8e2e274e341b86d6d71e374f78330adabbb4f371f8465f72d6e9e6e3d08db94ad93edc7a05d162e70",
            "97e77c25436bb62e7dd4b593960ead5b3f6ce9850619b731d5966e689009dba1b6f44383cc303f6ce9880699dba6cead",
            "959e689099fc316a4b5734c82c8b639ea52f1a64b62d9d5e2ba5b59d0efdb27ed8d21b0d32cd6e9d6a2fbdf5960e9dbd",
            "29adaf534f86eee6bad85e6aa341266cea1c7ba98d0699c1e7a9b6d44683cc6d43d7db4b6d34c84cdb3ac95e6aa34166",
            "991fb6d446830cdf349d6f2fb5f59d0e7ed9414b6b24c8340b5daf97d66890e9b674915e6af39e0eeeec546d3d093d1d",
            "3c6ce87abdd44683ccb07d1c9ca5361a646ef3c396da6890993675925e6aa34166f1e8505b6aa341665be87cbdd4e63b",
            "1de2b2a593f4d21b0d328dbbd28ba1f446834cb75be71ba5371a64f8ba292db585274304ffa7da7a89453ac4b0a9938c",
            "521b0d32374f2fb5a5361a64a60d9d6f94da689059b67592516aa3416673db6a4b6db1d3615c76eb7ca3d446830c2f37",
            "bda64b6b24c874fe6eb5a5351a64dc962e7294da6890099ea06a4b6d23d261f0f3e3f8aca8df1f231dc66d5b17394a6d",
            "34c84cbe5e6a4b6d34c82c9b3ac9516aa34166f3a2565b6a1b3b1dee8b0f5fed59db7f",
        ]
        .concat();
        let data: Vec<u8> =
            (0..hex.len() / 2).map(|i| u8::from_str_radix(&hex[2 * i..2 * i + 2], 16).unwrap()).collect();
        let text: String = (0..60)
            .map(|i| {
                format!(
                    "R{i} N{i} N{} {}K ; resistor {i} of the ladder
",
                    i + 1,
                    (i * 37) % 97 + 1
                )
            })
            .collect();
        assert_eq!(inflate(&data, text.len()).unwrap(), text.as_bytes());
    }
}
