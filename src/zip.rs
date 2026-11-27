//! Minimal ZIP reader (stored and deflated entries) for XPS packages.

use std::collections::HashMap;
use std::io::Read;

pub struct Zip<'a> {
    data: &'a [u8],
    /// Lower-case part name (no leading slash) → (original name, method, data offset, compressed size, size)
    entries: HashMap<String, (String, u16, usize, usize, usize)>,
}

fn u16_at(d: &[u8], o: usize) -> Option<u16> {
    Some(u16::from_le_bytes([*d.get(o)?, *d.get(o + 1)?]))
}
fn u32_at(d: &[u8], o: usize) -> Option<u32> {
    Some(u32::from_le_bytes([*d.get(o)?, *d.get(o + 1)?, *d.get(o + 2)?, *d.get(o + 3)?]))
}

impl<'a> Zip<'a> {
    pub fn open(data: &'a [u8]) -> Option<Zip<'a>> {
        // end of central directory
        let min = data.len().saturating_sub(65557);
        let mut eocd = None;
        let mut i = data.len().saturating_sub(22);
        while i >= min {
            if data[i..].starts_with(b"PK\x05\x06") {
                eocd = Some(i);
                break;
            }
            if i == 0 {
                break;
            }
            i -= 1;
        }
        let mut entries = HashMap::new();
        match eocd {
            Some(e) => {
                let count = u16_at(data, e + 10)? as usize;
                let mut p = u32_at(data, e + 16)? as usize;
                for _ in 0..count {
                    if !data.get(p..)?.starts_with(b"PK\x01\x02") {
                        break;
                    }
                    let method = u16_at(data, p + 10)?;
                    let csize = u32_at(data, p + 20)? as usize;
                    let size = u32_at(data, p + 24)? as usize;
                    let nlen = u16_at(data, p + 28)? as usize;
                    let elen = u16_at(data, p + 30)? as usize;
                    let clen = u16_at(data, p + 32)? as usize;
                    let lho = u32_at(data, p + 42)? as usize;
                    let name = String::from_utf8_lossy(data.get(p + 46..p + 46 + nlen)?).to_string();
                    // data starts after the local header
                    if data.get(lho..)?.starts_with(b"PK\x03\x04") {
                        let ln = u16_at(data, lho + 26)? as usize;
                        let le = u16_at(data, lho + 28)? as usize;
                        let off = lho + 30 + ln + le;
                        entries.insert(name.trim_start_matches('/').to_ascii_lowercase(), (name.clone(), method, off, csize, size));
                    }
                    p += 46 + nlen + elen + clen;
                }
            }
            None => {
                // no central directory (truncated / streamed): walk local headers
                let mut p = 0;
                while data.get(p..).map(|d| d.starts_with(b"PK\x03\x04")).unwrap_or(false) {
                    let flags = u16_at(data, p + 6)?;
                    let method = u16_at(data, p + 8)?;
                    let csize = u32_at(data, p + 18)? as usize;
                    let size = u32_at(data, p + 22)? as usize;
                    let ln = u16_at(data, p + 26)? as usize;
                    let le = u16_at(data, p + 28)? as usize;
                    let name = String::from_utf8_lossy(data.get(p + 30..p + 30 + ln)?).to_string();
                    let off = p + 30 + ln + le;
                    if flags & 8 != 0 || csize == 0 && method == 8 {
                        // sizes in a trailing descriptor: find the next header
                        let next = data[off..].windows(4).position(|w| w == b"PK\x03\x04" || w == b"PK\x01\x02").map(|x| off + x).unwrap_or(data.len());
                        entries.insert(name.trim_start_matches('/').to_ascii_lowercase(), (name, method, off, next - off, 0));
                        p = next;
                    } else {
                        entries.insert(name.trim_start_matches('/').to_ascii_lowercase(), (name, method, off, csize, size));
                        p = off + csize;
                    }
                }
            }
        }
        if entries.is_empty() {
            return None;
        }
        Some(Zip { data, entries })
    }

    pub fn names(&self) -> Vec<String> {
        let mut v: Vec<String> = self.entries.values().map(|e| e.0.clone()).collect();
        v.sort();
        v
    }

    fn read_one(&self, key: &str) -> Option<Vec<u8>> {
        let (_, method, off, csize, size) = self.entries.get(key)?;
        let raw = self.data.get(*off..(*off + *csize).min(self.data.len()))?;
        match method {
            0 => Some(raw.to_vec()),
            8 => {
                let mut out = Vec::with_capacity(*size);
                let _ = flate2::read::DeflateDecoder::new(raw).read_to_end(&mut out);
                Some(out)
            }
            _ => None,
        }
    }

    /// Read a part by name (case-insensitive, with or without a leading slash).
    /// Interleaved parts stored as pieces ("name/[0].piece" … "[n].last.piece") are joined.
    pub fn read(&self, name: &str) -> Option<Vec<u8>> {
        let key = name.trim_start_matches('/').to_ascii_lowercase();
        let key = percent_decode(&key);
        if let Some(d) = self.read_one(&key) {
            return Some(d);
        }
        let mut out = Vec::new();
        let mut i = 0;
        loop {
            if let Some(d) = self.read_one(&format!("{key}/[{i}].piece")) {
                out.extend(d);
            } else if let Some(d) = self.read_one(&format!("{key}/[{i}].last.piece")) {
                out.extend(d);
                return Some(out);
            } else {
                return if out.is_empty() { None } else { Some(out) };
            }
            i += 1;
        }
    }
}

pub fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).to_string()
}
