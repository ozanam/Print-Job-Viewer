//! PostScript file objects and decoding filters.

use std::cell::RefCell;
use std::io::Read;
use std::rc::Rc;

pub enum Src {
    Bytes { data: Rc<Vec<u8>>, pos: usize },
    Eexec { inner: PFile, r: u16, hex: Option<bool>, skip: usize },
    /// A decoding filter; decodes its whole input on first read.
    Lazy { source: PFile, kind: Kind, data: Option<Rc<Vec<u8>>>, pos: usize },
}

#[derive(Clone)]
pub enum Kind {
    Hex,
    A85,
    RunLength,
    Flate,
    Lzw(bool),
    Dct,
    Sub(i64, Vec<u8>),
    Null,
    /// predictor, colors, bpc, columns applied after the inner decoder
    Predict(Box<Kind>, i64, usize, usize, usize),
}

pub fn lazy(source: PFile, kind: Kind) -> PFile {
    Rc::new(RefCell::new(FileState { src: Src::Lazy { source, kind, data: None, pos: 0 }, closed: false, back: Vec::new(), _id: NEXT_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed) }))
}

fn decode(kind: &Kind, f: &PFile) -> Vec<u8> {
    match kind {
        Kind::Hex => ascii_hex(f),
        Kind::A85 => ascii85(f),
        Kind::RunLength => run_length(f),
        Kind::Flate => flate(f),
        Kind::Lzw(e) => lzw(f, *e),
        Kind::Dct => dct(f).0,
        Kind::Sub(c, e) => sub_file(f, *c, e),
        Kind::Null => f.borrow_mut().take_rest(),
        Kind::Predict(k, p, c, b, w) => predict(decode(k, f), *p, *c, *b, *w),
    }
}

pub struct FileState {
    pub src: Src,
    pub closed: bool,
    pub back: Vec<u8>,
    pub _id: usize,
}

pub type PFile = Rc<RefCell<FileState>>;

static NEXT_ID: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(1);

pub fn from_bytes(data: Rc<Vec<u8>>, pos: usize) -> PFile {
    Rc::new(RefCell::new(FileState { src: Src::Bytes { data, pos }, closed: false, back: Vec::new(), _id: NEXT_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed) }))
}

pub fn from_vec(v: Vec<u8>) -> PFile {
    from_bytes(Rc::new(v), 0)
}

pub fn eexec(inner: PFile) -> PFile {
    Rc::new(RefCell::new(FileState { src: Src::Eexec { inner, r: 55665, hex: None, skip: 4 }, closed: false, back: Vec::new(), _id: NEXT_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed) }))
}

fn hexval(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    }
}

impl FileState {
    pub fn read(&mut self) -> Option<u8> {
        if let Some(b) = self.back.pop() {
            return Some(b);
        }
        if self.closed {
            return None;
        }
        match &mut self.src {
            Src::Bytes { data, pos } => {
                let b = data.get(*pos).copied();
                if b.is_some() {
                    *pos += 1;
                }
                b
            }
            Src::Lazy { source, kind, data, pos } => {
                if data.is_none() {
                    *data = Some(Rc::new(decode(kind, source)));
                }
                let d = data.as_ref().unwrap();
                let b = d.get(*pos).copied();
                if b.is_some() {
                    *pos += 1;
                }
                b
            }
            Src::Eexec { inner, r, hex, skip } => loop {
                let mut inr = inner.borrow_mut();
                if hex.is_none() {
                    // skip leading whitespace, then sniff 4 bytes
                    let mut c = inr.read()?;
                    while c == b' ' || c == b'\t' || c == b'\r' || c == b'\n' {
                        c = inr.read()?;
                    }
                    let mut first = vec![c];
                    for _ in 0..3 {
                        if let Some(x) = inr.read() {
                            first.push(x);
                        }
                    }
                    let is_hex = first.iter().all(|&b| hexval(b).is_some());
                    for &b in first.iter().rev() {
                        inr.back.push(b);
                    }
                    *hex = Some(is_hex);
                }
                let cipher = if hex == &Some(true) {
                    let mut hi = None;
                    let val;
                    loop {
                        let c = inr.read()?;
                        if let Some(v) = hexval(c) {
                            match hi {
                                None => hi = Some(v),
                                Some(h) => {
                                    val = h << 4 | v;
                                    break;
                                }
                            }
                        } else if !c.is_ascii_whitespace() {
                            return None;
                        }
                    }
                    val
                } else {
                    inr.read()?
                };
                drop(inr);
                let plain = cipher ^ (*r >> 8) as u8;
                *r = (cipher as u16).wrapping_add(*r).wrapping_mul(52845).wrapping_add(22719);
                if *skip > 0 {
                    *skip -= 1;
                    continue;
                }
                return Some(plain);
            },
        }
    }

    pub fn unread(&mut self, b: u8) {
        self.back.push(b);
    }

    pub fn peek(&mut self) -> Option<u8> {
        let b = self.read()?;
        self.back.push(b);
        Some(b)
    }

    /// Remaining bytes as a slice when the source is in-memory; advances to end.
    pub fn take_rest(&mut self) -> Vec<u8> {
        let mut out: Vec<u8> = self.back.drain(..).rev().collect();
        match &mut self.src {
            Src::Bytes { data, pos } => {
                out.extend_from_slice(&data[(*pos).min(data.len())..]);
                *pos = data.len();
            }
            Src::Lazy { source, kind, data, pos } => {
                if data.is_none() {
                    *data = Some(Rc::new(decode(kind, source)));
                }
                let d = data.as_ref().unwrap();
                out.extend_from_slice(&d[(*pos).min(d.len())..]);
                *pos = d.len();
            }
            _ => {
                while let Some(b) = self.read() {
                    out.push(b);
                }
            }
        }
        out
    }

    /// Give back `n` bytes from the end of what take_rest returned.
    pub fn give_back(&mut self, rest: &[u8], consumed: usize) {
        let consumed = consumed.min(rest.len());
        match &mut self.src {
            Src::Bytes { data, pos } if self.back.is_empty() => {
                let unconsumed = rest.len() - consumed;
                *pos = data.len() - unconsumed.min(data.len());
            }
            Src::Lazy { data: Some(data), pos, .. } if self.back.is_empty() => {
                let unconsumed = rest.len() - consumed;
                *pos = data.len() - unconsumed.min(data.len());
            }
            _ => {
                for &b in rest[consumed..].iter().rev() {
                    self.back.push(b);
                }
            }
        }
    }

    pub fn position(&self) -> usize {
        match &self.src {
            Src::Bytes { pos, .. } | Src::Lazy { pos, .. } => *pos,
            _ => 0,
        }
    }

    pub fn bytes_available(&self) -> i64 {
        match &self.src {
            Src::Bytes { data, pos } => (data.len() - (*pos).min(data.len()) + self.back.len()) as i64,
            Src::Lazy { data: Some(data), pos, .. } => (data.len() - (*pos).min(data.len()) + self.back.len()) as i64,
            _ => -1,
        }
    }
}

// ---------------------------------------------------------------------------
// Eager decoders: they consume from a source file up to their EOD and return
// the decoded bytes.
// ---------------------------------------------------------------------------

pub fn ascii_hex(f: &PFile) -> Vec<u8> {
    let mut f = f.borrow_mut();
    let mut out = Vec::new();
    let mut hi: Option<u8> = None;
    while let Some(c) = f.read() {
        if c == b'>' {
            break;
        }
        if let Some(v) = hexval(c) {
            match hi {
                None => hi = Some(v),
                Some(h) => {
                    out.push(h << 4 | v);
                    hi = None;
                }
            }
        }
    }
    if let Some(h) = hi {
        out.push(h << 4);
    }
    out
}

pub fn ascii85(f: &PFile) -> Vec<u8> {
    let mut f = f.borrow_mut();
    let mut out = Vec::new();
    let mut group = [0u32; 5];
    let mut n = 0;
    while let Some(c) = f.read() {
        match c {
            b'~' => {
                f.read(); // '>'
                break;
            }
            b'z' if n == 0 => out.extend_from_slice(&[0, 0, 0, 0]),
            b'!'..=b'u' => {
                group[n] = (c - b'!') as u32;
                n += 1;
                if n == 5 {
                    let v = group.iter().fold(0u32, |a, &d| a.wrapping_mul(85).wrapping_add(d));
                    out.extend_from_slice(&v.to_be_bytes());
                    n = 0;
                }
            }
            _ => {}
        }
    }
    if n > 1 {
        for g in group.iter_mut().skip(n) {
            *g = 84;
        }
        let v = group.iter().fold(0u32, |a, &d| a.wrapping_mul(85).wrapping_add(d));
        out.extend_from_slice(&v.to_be_bytes()[..n - 1]);
    }
    out
}

pub fn run_length(f: &PFile) -> Vec<u8> {
    let mut f = f.borrow_mut();
    let mut out = Vec::new();
    while let Some(l) = f.read() {
        if l == 128 {
            break;
        }
        if l < 128 {
            for _ in 0..=l {
                match f.read() {
                    Some(b) => out.push(b),
                    None => return out,
                }
            }
        } else {
            let Some(b) = f.read() else { break };
            for _ in 0..(257 - l as usize) {
                out.push(b);
            }
        }
    }
    out
}

pub fn flate(f: &PFile) -> Vec<u8> {
    let mut fs = f.borrow_mut();
    let rest = fs.take_rest();
    let mut out = Vec::new();
    let mut d = flate2::Decompress::new(true);
    let mut buf = vec![0u8; 1 << 16];
    let mut inpos = 0usize;
    loop {
        let before_in = d.total_in() as usize;
        let before_out = d.total_out() as usize;
        let res = d.decompress(&rest[inpos..], &mut buf, flate2::FlushDecompress::None);
        let used = d.total_in() as usize - before_in;
        let produced = d.total_out() as usize - before_out;
        inpos += used;
        out.extend_from_slice(&buf[..produced]);
        match res {
            Ok(flate2::Status::StreamEnd) => break,
            Ok(_) if used == 0 && produced == 0 => break,
            Ok(_) => {}
            Err(_) => {
                // Try raw deflate as a fallback when there's no zlib header.
                if out.is_empty() && inpos == 0 {
                    let mut dec = flate2::read::DeflateDecoder::new(&rest[..]);
                    let mut v = Vec::new();
                    let _ = dec.read_to_end(&mut v);
                    out = v;
                    inpos = rest.len();
                }
                break;
            }
        }
    }
    fs.give_back(&rest, inpos);
    out
}

pub fn lzw(f: &PFile, early: bool) -> Vec<u8> {
    let mut fs = f.borrow_mut();
    let mut out = Vec::new();
    let mut table: Vec<Vec<u8>> = (0..256u32).map(|i| vec![i as u8]).collect();
    table.push(vec![]);
    table.push(vec![]);
    let mut width = 9;
    let mut bitbuf: u32 = 0;
    let mut nbits = 0;
    let mut prev: Option<Vec<u8>> = None;
    loop {
        while nbits < width {
            let Some(b) = fs.read() else { return out };
            bitbuf = bitbuf << 8 | b as u32;
            nbits += 8;
        }
        let code = (bitbuf >> (nbits - width)) & ((1 << width) - 1);
        nbits -= width;
        if code == 256 {
            table.truncate(258);
            width = 9;
            prev = None;
            continue;
        }
        if code == 257 {
            break;
        }
        let entry = if (code as usize) < table.len() {
            table[code as usize].clone()
        } else if let Some(p) = &prev {
            let mut e = p.clone();
            e.push(p[0]);
            e
        } else {
            break;
        };
        out.extend_from_slice(&entry);
        if let Some(p) = prev {
            let mut ne = p;
            ne.push(entry[0]);
            table.push(ne);
        }
        prev = Some(entry);
        let lim = table.len() + if early { 1 } else { 0 };
        if lim >= 4096 {
            width = 12;
        } else if lim >= 2048 {
            width = 12;
        } else if lim >= 1024 {
            width = 11;
        } else if lim >= 512 {
            width = 10;
        }
    }
    out
}

/// Find the end of a JPEG stream in `d` (index after EOI).
fn jpeg_end(d: &[u8]) -> usize {
    let mut p = 2;
    while p + 4 <= d.len() {
        if d[p] != 0xFF {
            p += 1;
            continue;
        }
        let m = d[p + 1];
        if m == 0xD9 {
            return p + 2;
        }
        if m == 0xFF || m == 0x00 || (0xD0..=0xD7).contains(&m) || m == 0x01 {
            p += if m == 0xFF { 1 } else { 2 };
            continue;
        }
        let len = u16::from_be_bytes([d[p + 2], d[p + 3]]) as usize;
        p += 2 + len;
        if m == 0xDA {
            // scan entropy-coded data
            while p + 1 < d.len() {
                if d[p] == 0xFF && d[p + 1] != 0 && !(0xD0..=0xD7).contains(&d[p + 1]) {
                    break;
                }
                p += 1;
            }
        }
    }
    d.len()
}

/// Returns decoded samples and number of components.
pub fn dct(f: &PFile) -> (Vec<u8>, usize, String) {
    let mut fs = f.borrow_mut();
    let rest = fs.take_rest();
    let end = jpeg_end(&rest);
    fs.give_back(&rest, end);
    drop(fs);
    let mut dec = jpeg_decoder::Decoder::new(std::io::Cursor::new(&rest[..end]));
    match dec.decode() {
        Ok(px) => {
            let info = dec.info().unwrap();
            let n = match info.pixel_format {
                jpeg_decoder::PixelFormat::L8 => 1,
                jpeg_decoder::PixelFormat::L16 => 2,
                jpeg_decoder::PixelFormat::RGB24 => 3,
                jpeg_decoder::PixelFormat::CMYK32 => 4,
            };
            (px, n, String::new())
        }
        Err(e) => (Vec::new(), 0, e.to_string()),
    }
}

pub fn sub_file(f: &PFile, count: i64, eod: &[u8]) -> Vec<u8> {
    let mut fs = f.borrow_mut();
    let mut out = Vec::new();
    if eod.is_empty() {
        for _ in 0..count.max(0) {
            match fs.read() {
                Some(b) => out.push(b),
                None => break,
            }
        }
        return out;
    }
    let mut seen = 0;
    while let Some(b) = fs.read() {
        out.push(b);
        if out.ends_with(eod) {
            if seen >= count {
                out.truncate(out.len() - eod.len());
                break;
            }
            seen += 1;
        }
    }
    out
}

/// Undo PNG / TIFF predictors.
pub fn predict(data: Vec<u8>, predictor: i64, colors: usize, bpc: usize, columns: usize) -> Vec<u8> {
    if predictor < 2 {
        return data;
    }
    let bpp = (colors * bpc).div_ceil(8).max(1);
    let row = (colors * bpc * columns).div_ceil(8);
    if predictor == 2 {
        let mut d = data;
        if bpc == 8 {
            for r in d.chunks_mut(row) {
                for i in bpp..r.len() {
                    r[i] = r[i].wrapping_add(r[i - bpp]);
                }
            }
        }
        return d;
    }
    let mut out = Vec::with_capacity(data.len());
    let mut prev = vec![0u8; row];
    for chunk in data.chunks(row + 1) {
        if chunk.is_empty() {
            break;
        }
        let t = chunk[0];
        let mut cur: Vec<u8> = chunk[1..].to_vec();
        cur.resize(row, 0);
        for i in 0..row {
            let a = if i >= bpp { cur[i - bpp] as i32 } else { 0 };
            let b = prev[i] as i32;
            let c = if i >= bpp { prev[i - bpp] as i32 } else { 0 };
            let v = match t {
                1 => a,
                2 => b,
                3 => (a + b) / 2,
                4 => {
                    let p = a + b - c;
                    let (pa, pb, pc) = ((p - a).abs(), (p - b).abs(), (p - c).abs());
                    if pa <= pb && pa <= pc {
                        a
                    } else if pb <= pc {
                        b
                    } else {
                        c
                    }
                }
                _ => 0,
            };
            cur[i] = (cur[i] as i32 + v) as u8;
        }
        out.extend_from_slice(&cur);
        prev = cur;
    }
    out
}
