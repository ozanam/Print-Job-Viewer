//! Font support shared by the interpreters:
//! - raw TrueType `glyf` outline decoding (downloaded PCL XL / Type 42 fonts)
//! - system font substitution for resident / non-embedded fonts
//! - glyph-name and encoding tables

use crate::geom::{Matrix, PathAcc};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

// ---------------------------------------------------------------------------
// TrueType glyf decoding
// ---------------------------------------------------------------------------

fn be16(d: &[u8], o: usize) -> Option<u16> {
    Some(u16::from_be_bytes([*d.get(o)?, *d.get(o + 1)?]))
}
fn bei16(d: &[u8], o: usize) -> Option<i16> {
    be16(d, o).map(|v| v as i16)
}
fn be32(d: &[u8], o: usize) -> Option<u32> {
    Some(u32::from_be_bytes([*d.get(o)?, *d.get(o + 1)?, *d.get(o + 2)?, *d.get(o + 3)?]))
}

/// Decode a TrueType glyph description into `out`, mapping font units with `m`.
/// `lookup` fetches other glyph descriptions (for composite glyphs).
pub fn glyf_outline(data: &[u8], m: &Matrix, out: &mut PathAcc, lookup: &dyn Fn(u16) -> Option<Vec<u8>>, depth: u32) -> Option<()> {
    if data.len() < 10 || depth > 8 {
        return Some(());
    }
    let ncont = bei16(data, 0)?;
    if ncont >= 0 {
        let n = ncont as usize;
        let mut ends = Vec::with_capacity(n);
        for i in 0..n {
            ends.push(be16(data, 10 + i * 2)? as usize);
        }
        let npts = ends.last().map(|e| e + 1).unwrap_or(0);
        let ilen = be16(data, 10 + n * 2)? as usize;
        let mut p = 12 + n * 2 + ilen;
        let mut flags = Vec::with_capacity(npts);
        while flags.len() < npts {
            let f = *data.get(p)?;
            p += 1;
            flags.push(f);
            if f & 8 != 0 {
                let r = *data.get(p)?;
                p += 1;
                for _ in 0..r {
                    flags.push(f);
                }
            }
        }
        flags.truncate(npts);
        let mut xs = Vec::with_capacity(npts);
        let mut v: i32 = 0;
        for &f in &flags {
            if f & 2 != 0 {
                let dx = *data.get(p)? as i32;
                p += 1;
                v += if f & 16 != 0 { dx } else { -dx };
            } else if f & 16 == 0 {
                v += bei16(data, p)? as i32;
                p += 2;
            }
            xs.push(v);
        }
        let mut ys = Vec::with_capacity(npts);
        v = 0;
        for &f in &flags {
            if f & 4 != 0 {
                let dy = *data.get(p)? as i32;
                p += 1;
                v += if f & 32 != 0 { dy } else { -dy };
            } else if f & 32 == 0 {
                v += bei16(data, p)? as i32;
                p += 2;
            }
            ys.push(v);
        }
        let mut start = 0;
        for &end in &ends {
            if end < start || end >= npts {
                break;
            }
            let pts: Vec<(f64, f64, bool)> = (start..=end).map(|i| (xs[i] as f64, ys[i] as f64, flags[i] & 1 != 0)).collect();
            emit_quad_contour(&pts, m, out);
            start = end + 1;
        }
    } else {
        // composite
        let mut p = 10;
        loop {
            let flags = be16(data, p)?;
            let gid = be16(data, p + 2)?;
            p += 4;
            let (dx, dy) = if flags & 1 != 0 {
                let a = bei16(data, p)? as f64;
                let b = bei16(data, p + 2)? as f64;
                p += 4;
                (a, b)
            } else {
                let a = *data.get(p)? as i8 as f64;
                let b = *data.get(p + 1)? as i8 as f64;
                p += 2;
                (a, b)
            };
            let f2 = |o: usize| bei16(data, o).map(|v| v as f64 / 16384.0);
            let (mut a, mut b, mut c, mut d) = (1.0, 0.0, 0.0, 1.0);
            if flags & 8 != 0 {
                a = f2(p)?;
                d = a;
                p += 2;
            } else if flags & 0x40 != 0 {
                a = f2(p)?;
                d = f2(p + 2)?;
                p += 4;
            } else if flags & 0x80 != 0 {
                a = f2(p)?;
                b = f2(p + 2)?;
                c = f2(p + 4)?;
                d = f2(p + 6)?;
                p += 8;
            }
            let (tx, ty) = if flags & 2 != 0 { (dx, dy) } else { (0.0, 0.0) };
            let cm = Matrix::new(a, b, c, d, tx, ty).then(m);
            if let Some(g) = lookup(gid) {
                glyf_outline(&g, &cm, out, lookup, depth + 1);
            }
            if flags & 0x20 == 0 {
                break;
            }
        }
    }
    Some(())
}

fn emit_quad_contour(pts: &[(f64, f64, bool)], m: &Matrix, out: &mut PathAcc) {
    let n = pts.len();
    if n == 0 {
        return;
    }
    // find start point (an on-curve point, or midpoint of two off points)
    let first_on = pts.iter().position(|p| p.2);
    let (sx, sy, rot) = match first_on {
        Some(i) => (pts[i].0, pts[i].1, i),
        None => ((pts[0].0 + pts[n - 1].0) / 2.0, (pts[0].1 + pts[n - 1].1) / 2.0, 0),
    };
    let (mx, my) = m.apply(sx, sy);
    out.move_to(mx, my);
    let mut cur = (sx, sy);
    let mut ctrl: Option<(f64, f64)> = None;
    let quad = |out: &mut PathAcc, from: (f64, f64), c: (f64, f64), to: (f64, f64)| {
        let c1 = (from.0 + 2.0 / 3.0 * (c.0 - from.0), from.1 + 2.0 / 3.0 * (c.1 - from.1));
        let c2 = (to.0 + 2.0 / 3.0 * (c.0 - to.0), to.1 + 2.0 / 3.0 * (c.1 - to.1));
        let (a, b) = m.apply(c1.0, c1.1);
        let (c_, d) = m.apply(c2.0, c2.1);
        let (e, f) = m.apply(to.0, to.1);
        out.curve_to(a, b, c_, d, e, f);
    };
    let count = if first_on.is_some() { n } else { n };
    for k in 1..=count {
        let p = pts[(rot + k) % n];
        if first_on.is_none() && k == count {
            // close back to synthesized start
            if let Some(c) = ctrl.take() {
                quad(out, cur, c, (sx, sy));
            }
            break;
        }
        if p.2 {
            match ctrl.take() {
                Some(c) => quad(out, cur, c, (p.0, p.1)),
                None => {
                    let (x, y) = m.apply(p.0, p.1);
                    out.line_to(x, y);
                }
            }
            cur = (p.0, p.1);
        } else {
            if let Some(c) = ctrl {
                let mid = ((c.0 + p.0) / 2.0, (c.1 + p.1) / 2.0);
                quad(out, cur, c, mid);
                cur = mid;
            }
            ctrl = Some((p.0, p.1));
        }
    }
    if let Some(c) = ctrl {
        quad(out, cur, c, (sx, sy));
    }
    out.close();
}

/// Parsed pieces of an sfnt (TrueType) container that may lack some tables.
#[derive(Default, Clone)]
pub struct SfntInfo {
    pub upem: f64,
    pub index_to_loc_long: bool,
    pub loca: Option<Vec<u8>>,
    pub glyf: Option<Vec<u8>>,
    pub hmtx: Option<Vec<u8>>,
    pub num_hmetrics: u16,
    pub cmap_face: Option<Vec<u8>>, // whole sfnt bytes if parseable by ttf-parser
}

impl SfntInfo {
    pub fn parse(data: &[u8]) -> SfntInfo {
        let mut s = SfntInfo { upem: 1000.0, ..Default::default() };
        let Some(ntables) = be16(data, 4) else { return s };
        let mut tables: HashMap<[u8; 4], (usize, usize)> = HashMap::new();
        for i in 0..ntables as usize {
            let o = 12 + i * 16;
            if o + 16 > data.len() {
                break;
            }
            let tag = [data[o], data[o + 1], data[o + 2], data[o + 3]];
            let off = be32(data, o + 8).unwrap_or(0) as usize;
            let len = be32(data, o + 12).unwrap_or(0) as usize;
            tables.insert(tag, (off, len));
        }
        let get = |t: &[u8; 4]| -> Option<Vec<u8>> {
            let (o, l) = *tables.get(t)?;
            let end = (o + l).min(data.len());
            if o >= end {
                return None;
            }
            Some(data[o..end].to_vec())
        };
        if let Some(head) = get(b"head") {
            if let Some(u) = be16(&head, 18) {
                if u > 0 {
                    s.upem = u as f64;
                }
            }
            s.index_to_loc_long = bei16(&head, 50).unwrap_or(0) != 0;
        }
        if let Some(hhea) = get(b"hhea") {
            s.num_hmetrics = be16(&hhea, 34).unwrap_or(0);
        }
        s.hmtx = get(b"hmtx");
        s.loca = get(b"loca");
        s.glyf = get(b"glyf");
        if tables.contains_key(b"cmap") && ttf_parser::Face::parse(data, 0).is_ok() {
            s.cmap_face = Some(data.to_vec());
        }
        s
    }

    pub fn glyph_data(&self, gid: u16) -> Option<Vec<u8>> {
        let loca = self.loca.as_ref()?;
        let glyf = self.glyf.as_ref()?;
        let g = gid as usize;
        let (a, b) = if self.index_to_loc_long {
            (be32(loca, g * 4)? as usize, be32(loca, g * 4 + 4)? as usize)
        } else {
            (be16(loca, g * 2)? as usize * 2, be16(loca, g * 2 + 2)? as usize * 2)
        };
        if b <= a || b > glyf.len() {
            return None;
        }
        Some(glyf[a..b].to_vec())
    }

    /// Advance width in font units.
    pub fn advance(&self, gid: u16) -> Option<f64> {
        let hm = self.hmtx.as_ref()?;
        let n = self.num_hmetrics.max(1) as usize;
        let i = (gid as usize).min(n - 1);
        be16(hm, i * 4).map(|v| v as f64)
    }
}

// ---------------------------------------------------------------------------
// System fonts
// ---------------------------------------------------------------------------

pub struct SysFont {
    pub face: ttf_parser::Face<'static>,
    pub upem: f64,
    pub symbol: bool,
    pub file: String,
}

struct Builder<'a> {
    out: &'a mut PathAcc,
    m: Matrix,
    cur: (f64, f64),
}

impl ttf_parser::OutlineBuilder for Builder<'_> {
    fn move_to(&mut self, x: f32, y: f32) {
        let (a, b) = self.m.apply(x as f64, y as f64);
        self.out.move_to(a, b);
        self.cur = (x as f64, y as f64);
    }
    fn line_to(&mut self, x: f32, y: f32) {
        let (a, b) = self.m.apply(x as f64, y as f64);
        self.out.line_to(a, b);
        self.cur = (x as f64, y as f64);
    }
    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
        let (x0, y0) = self.cur;
        let (x1, y1, x, y) = (x1 as f64, y1 as f64, x as f64, y as f64);
        let c1 = (x0 + 2.0 / 3.0 * (x1 - x0), y0 + 2.0 / 3.0 * (y1 - y0));
        let c2 = (x + 2.0 / 3.0 * (x1 - x), y + 2.0 / 3.0 * (y1 - y));
        let (a, b) = self.m.apply(c1.0, c1.1);
        let (c, d) = self.m.apply(c2.0, c2.1);
        let (e, f) = self.m.apply(x, y);
        self.out.curve_to(a, b, c, d, e, f);
        self.cur = (x, y);
    }
    fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
        let (a, b) = self.m.apply(x1 as f64, y1 as f64);
        let (c, d) = self.m.apply(x2 as f64, y2 as f64);
        let (e, f) = self.m.apply(x as f64, y as f64);
        self.out.curve_to(a, b, c, d, e, f);
        self.cur = (x as f64, y as f64);
    }
    fn close(&mut self) {
        self.out.close();
    }
}

impl SysFont {
    pub fn from_bytes(data: &'static [u8], file: String) -> Option<SysFont> {
        let face = ttf_parser::Face::parse(data, 0).ok()?;
        let upem = face.units_per_em() as f64;
        let symbol = file.to_ascii_lowercase().contains("symbol");
        Some(SysFont { face, upem, symbol, file })
    }

    pub fn glyph_for_char(&self, ch: char) -> Option<ttf_parser::GlyphId> {
        if self.symbol {
            let c = ch as u32;
            if c < 256 {
                if let Some(g) = char::from_u32(0xF000 + c).and_then(|c| self.face.glyph_index(c)) {
                    return Some(g);
                }
                if let Some(g) = symbol_to_unicode(c as u8).and_then(|u| self.face.glyph_index(u)) {
                    return Some(g);
                }
            }
        }
        self.face.glyph_index(ch).filter(|g| g.0 != 0)
    }

    pub fn glyph_by_name(&self, name: &str) -> Option<ttf_parser::GlyphId> {
        self.face.glyph_index_by_name(name)
    }

    /// Outline in font units mapped through `m` (font units, y up).
    pub fn outline(&self, gid: ttf_parser::GlyphId, m: &Matrix, out: &mut PathAcc) {
        let mut b = Builder { out, m: *m, cur: (0.0, 0.0) };
        self.face.outline_glyph(gid, &mut b);
    }

    /// Advance in font units.
    pub fn advance(&self, gid: ttf_parser::GlyphId) -> f64 {
        self.face.glyph_hor_advance(gid).unwrap_or(0) as f64
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum Family {
    Sans,
    Serif,
    Mono,
    Symbol,
    Dingbats,
}

fn classify(name: &str) -> (Family, bool, bool) {
    let n = name.to_ascii_lowercase();
    let family = if n.contains("courier") || n.contains("mono") || n.contains("letter gothic") || n.contains("lettergothic") || n.contains("consol") || n.contains("lucida console") || n.contains("ocr") {
        Family::Mono
    } else if n.contains("dingbat") || n.contains("wingding") {
        Family::Dingbats
    } else if n.contains("symbol") {
        Family::Symbol
    } else if n.contains("times") || n.contains("roman") || n.contains("serif") && !n.contains("sans") || n.contains("garamond") || n.contains("georgia") || n.contains("palatino") || n.contains("bookman") || n.contains("century") || n.contains("cgtimes") || n.contains("cg times") || n.contains("schoolbook") || n.contains("cambria") || n.contains("minion") || n.contains("tms") || n.contains("baskerville") || n.contains("antiqua") {
        Family::Serif
    } else {
        Family::Sans
    };
    let bold = n.contains("bold") || n.contains("bd") || n.contains("black") || n.contains("heavy") || n.contains("demi") || n.contains("semibold") || n.ends_with("-b") || n.contains(",bold");
    let italic = n.contains("italic") || n.contains("oblique") || n.contains("ital") || n.ends_with("-i") || n.ends_with("it") || n.contains("-it") || n.ends_with("-bi") || n.contains(",italic") || n.ends_with("obl");
    (family, bold, italic)
}

fn candidates(f: Family, bold: bool, italic: bool) -> Vec<String> {
    let mut out = Vec::new();
    let style_idx = (bold as usize) * 2 + italic as usize; // 0 reg, 1 it, 2 bd, 3 bdit
    // Windows
    let windir = std::env::var("WINDIR").or_else(|_| std::env::var("SystemRoot")).unwrap_or_else(|_| "C:\\Windows".into());
    let win: &[&str; 4] = match f {
        Family::Sans => &["arial.ttf", "ariali.ttf", "arialbd.ttf", "arialbi.ttf"],
        Family::Serif => &["times.ttf", "timesi.ttf", "timesbd.ttf", "timesbi.ttf"],
        Family::Mono => &["cour.ttf", "couri.ttf", "courbd.ttf", "courbi.ttf"],
        Family::Symbol => &["symbol.ttf", "symbol.ttf", "symbol.ttf", "symbol.ttf"],
        Family::Dingbats => &["wingding.ttf", "wingding.ttf", "wingding.ttf", "wingding.ttf"],
    };
    out.push(format!("{windir}\\Fonts\\{}", win[style_idx]));
    out.push(format!("{windir}\\Fonts\\{}", win[0]));
    // macOS
    let mac: &[&str; 4] = match f {
        Family::Sans => &["Arial.ttf", "Arial Italic.ttf", "Arial Bold.ttf", "Arial Bold Italic.ttf"],
        Family::Serif => &["Times New Roman.ttf", "Times New Roman Italic.ttf", "Times New Roman Bold.ttf", "Times New Roman Bold Italic.ttf"],
        Family::Mono => &["Courier New.ttf", "Courier New Italic.ttf", "Courier New Bold.ttf", "Courier New Bold Italic.ttf"],
        Family::Symbol => &["Symbol.ttf", "Symbol.ttf", "Symbol.ttf", "Symbol.ttf"],
        Family::Dingbats => &["ZapfDingbats.ttf", "ZapfDingbats.ttf", "ZapfDingbats.ttf", "ZapfDingbats.ttf"],
    };
    for dir in ["/System/Library/Fonts/Supplemental", "/System/Library/Fonts", "/Library/Fonts"] {
        out.push(format!("{dir}/{}", mac[style_idx]));
        out.push(format!("{dir}/{}", mac[0]));
    }
    // Linux
    let lin: &[&str; 4] = match f {
        Family::Sans | Family::Symbol | Family::Dingbats => &["LiberationSans-Regular.ttf", "LiberationSans-Italic.ttf", "LiberationSans-Bold.ttf", "LiberationSans-BoldItalic.ttf"],
        Family::Serif => &["LiberationSerif-Regular.ttf", "LiberationSerif-Italic.ttf", "LiberationSerif-Bold.ttf", "LiberationSerif-BoldItalic.ttf"],
        Family::Mono => &["LiberationMono-Regular.ttf", "LiberationMono-Italic.ttf", "LiberationMono-Bold.ttf", "LiberationMono-BoldItalic.ttf"],
    };
    for dir in ["/usr/share/fonts/truetype/liberation", "/usr/share/fonts/liberation"] {
        out.push(format!("{dir}/{}", lin[style_idx]));
    }
    if f != Family::Sans {
        // fall back to sans of same style
        let mut more = candidates(Family::Sans, bold, italic);
        out.append(&mut more);
    }
    out
}

static FONT_CACHE: OnceLock<Mutex<HashMap<(Family, bool, bool), Option<Arc<SysFont>>>>> = OnceLock::new();
static FILE_CACHE: OnceLock<Mutex<HashMap<String, Option<&'static [u8]>>>> = OnceLock::new();

fn load_file(path: &str) -> Option<&'static [u8]> {
    let cache = FILE_CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    let mut c = cache.lock().unwrap();
    if let Some(v) = c.get(path) {
        return *v;
    }
    let v = std::fs::read(path).ok().map(|d| &*Box::leak(d.into_boxed_slice()));
    c.insert(path.to_string(), v);
    v
}

/// Find a system font to substitute for the named font. Always returns a font
/// (falls back to the font bundled in the executable).
pub fn system_font(name: &str) -> Arc<SysFont> {
    let key = classify(name);
    let cache = FONT_CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    {
        let c = cache.lock().unwrap();
        if let Some(Some(f)) = c.get(&key) {
            return f.clone();
        }
    }
    let mut found = None;
    for p in candidates(key.0, key.1, key.2) {
        if let Some(data) = load_file(&p) {
            if let Some(f) = SysFont::from_bytes(data, p.clone()) {
                found = Some(Arc::new(f));
                break;
            }
        }
    }
    let font = found.unwrap_or_else(|| {
        let data: &'static [u8] = if key.0 == Family::Mono { epaint_default_fonts::HACK_REGULAR } else { epaint_default_fonts::UBUNTU_LIGHT };
        Arc::new(SysFont::from_bytes(data, "(built-in)".into()).expect("bundled font"))
    });
    cache.lock().unwrap().insert(key, Some(font.clone()));
    font
}

// ---------------------------------------------------------------------------
// Glyph names & encodings
// ---------------------------------------------------------------------------

pub const LATIN1_NAMES: [&str; 96] = [
    "space", "exclamdown", "cent", "sterling", "currency", "yen", "brokenbar", "section", "dieresis", "copyright", "ordfeminine", "guillemotleft", "logicalnot", "hyphen", "registered", "macron",
    "degree", "plusminus", "twosuperior", "threesuperior", "acute", "mu", "paragraph", "periodcentered", "cedilla", "onesuperior", "ordmasculine", "guillemotright", "onequarter", "onehalf", "threequarters", "questiondown",
    "Agrave", "Aacute", "Acircumflex", "Atilde", "Adieresis", "Aring", "AE", "Ccedilla", "Egrave", "Eacute", "Ecircumflex", "Edieresis", "Igrave", "Iacute", "Icircumflex", "Idieresis",
    "Eth", "Ntilde", "Ograve", "Oacute", "Ocircumflex", "Otilde", "Odieresis", "multiply", "Oslash", "Ugrave", "Uacute", "Ucircumflex", "Udieresis", "Yacute", "Thorn", "germandbls",
    "agrave", "aacute", "acircumflex", "atilde", "adieresis", "aring", "ae", "ccedilla", "egrave", "eacute", "ecircumflex", "edieresis", "igrave", "iacute", "icircumflex", "idieresis",
    "eth", "ntilde", "ograve", "oacute", "ocircumflex", "otilde", "odieresis", "divide", "oslash", "ugrave", "uacute", "ucircumflex", "udieresis", "yacute", "thorn", "ydieresis",
];

pub const ASCII_NAMES: [&str; 95] = [
    "space", "exclam", "quotedbl", "numbersign", "dollar", "percent", "ampersand", "quotesingle", "parenleft", "parenright", "asterisk", "plus", "comma", "hyphen", "period", "slash",
    "zero", "one", "two", "three", "four", "five", "six", "seven", "eight", "nine", "colon", "semicolon", "less", "equal", "greater", "question",
    "at", "A", "B", "C", "D", "E", "F", "G", "H", "I", "J", "K", "L", "M", "N", "O", "P", "Q", "R", "S", "T", "U", "V", "W", "X", "Y", "Z",
    "bracketleft", "backslash", "bracketright", "asciicircum", "underscore", "grave",
    "a", "b", "c", "d", "e", "f", "g", "h", "i", "j", "k", "l", "m", "n", "o", "p", "q", "r", "s", "t", "u", "v", "w", "x", "y", "z",
    "braceleft", "bar", "braceright", "asciitilde",
];

const EXTRA_NAMES: &[(&str, u32)] = &[
    ("quoteright", 0x2019), ("quoteleft", 0x2018), ("fraction", 0x2044), ("florin", 0x0192), ("quotedblleft", 0x201C),
    ("guilsinglleft", 0x2039), ("guilsinglright", 0x203A), ("fi", 0xFB01), ("fl", 0xFB02), ("endash", 0x2013),
    ("dagger", 0x2020), ("daggerdbl", 0x2021), ("bullet", 0x2022), ("quotesinglbase", 0x201A), ("quotedblbase", 0x201E),
    ("quotedblright", 0x201D), ("ellipsis", 0x2026), ("perthousand", 0x2030), ("circumflex", 0x02C6), ("tilde", 0x02DC),
    ("breve", 0x02D8), ("dotaccent", 0x02D9), ("ring", 0x02DA), ("hungarumlaut", 0x02DD), ("ogonek", 0x02DB),
    ("caron", 0x02C7), ("emdash", 0x2014), ("Lslash", 0x0141), ("OE", 0x0152), ("dotlessi", 0x0131), ("lslash", 0x0142),
    ("oe", 0x0153), ("Euro", 0x20AC), ("trademark", 0x2122), ("Scaron", 0x0160), ("scaron", 0x0161), ("Zcaron", 0x017D),
    ("zcaron", 0x017E), ("Ydieresis", 0x0178), ("minus", 0x2212), ("nbspace", 0x00A0), ("sfthyphen", 0x00AD),
    ("periodcentered", 0x00B7), ("middot", 0x00B7), ("mu", 0x00B5), ("Delta", 0x2206), ("Omega", 0x2126), ("pi", 0x03C0),
    ("summation", 0x2211), ("product", 0x220F), ("radical", 0x221A), ("infinity", 0x221E), ("integral", 0x222B),
    ("approxequal", 0x2248), ("notequal", 0x2260), ("lessequal", 0x2264), ("greaterequal", 0x2265), ("lozenge", 0x25CA),
    ("partialdiff", 0x2202), ("Gcircumflex", 0x011C), ("Amacron", 0x0100), ("amacron", 0x0101), ("Cacute", 0x0106), ("cacute", 0x0107),
    ("Ccaron", 0x010C), ("ccaron", 0x010D), ("Dcaron", 0x010E), ("dcaron", 0x010F), ("Dcroat", 0x0110), ("dcroat", 0x0111),
    ("Ecaron", 0x011A), ("ecaron", 0x011B), ("Eogonek", 0x0118), ("eogonek", 0x0119), ("Aogonek", 0x0104), ("aogonek", 0x0105),
    ("Nacute", 0x0143), ("nacute", 0x0144), ("Ncaron", 0x0147), ("ncaron", 0x0148), ("Ohungarumlaut", 0x0150), ("ohungarumlaut", 0x0151),
    ("Racute", 0x0154), ("racute", 0x0155), ("Rcaron", 0x0158), ("rcaron", 0x0159), ("Sacute", 0x015A), ("sacute", 0x015B),
    ("Scedilla", 0x015E), ("scedilla", 0x015F), ("Tcaron", 0x0164), ("tcaron", 0x0165), ("Uring", 0x016E), ("uring", 0x016F),
    ("Uhungarumlaut", 0x0170), ("uhungarumlaut", 0x0171), ("Zacute", 0x0179), ("zacute", 0x017A), ("Zdotaccent", 0x017B), ("zdotaccent", 0x017C),
    ("Gbreve", 0x011E), ("gbreve", 0x011F), ("Idotaccent", 0x0130), ("Lacute", 0x0139), ("lacute", 0x013A), ("Lcaron", 0x013D), ("lcaron", 0x013E),
    ("Abreve", 0x0102), ("abreve", 0x0103), ("Tcommaaccent", 0x0162), ("tcommaaccent", 0x0163), ("Scommaaccent", 0x0218), ("scommaaccent", 0x0219),
    ("Alpha", 0x0391), ("Beta", 0x0392), ("Gamma", 0x0393), ("Epsilon", 0x0395), ("Theta", 0x0398), ("Lambda", 0x039B), ("Sigma", 0x03A3), ("Phi", 0x03A6), ("Psi", 0x03A8),
    ("alpha", 0x03B1), ("beta", 0x03B2), ("gamma", 0x03B3), ("delta", 0x03B4), ("epsilon", 0x03B5), ("theta", 0x03B8), ("lambda", 0x03BB), ("sigma", 0x03C3), ("tau", 0x03C4), ("phi", 0x03C6), ("omega", 0x03C9),
    ("arrowleft", 0x2190), ("arrowup", 0x2191), ("arrowright", 0x2192), ("arrowdown", 0x2193), ("arrowboth", 0x2194),
    ("checkmark", 0x2713), ("degree", 0x00B0), ("copyrightserif", 0x00A9), ("registerserif", 0x00AE), ("trademarkserif", 0x2122),
    ("ff", 0xFB00), ("ffi", 0xFB03), ("ffl", 0xFB04), ("quotesingle", 0x0027), ("grave", 0x0060), ("space", 0x0020),
];

static NAME_MAP: OnceLock<HashMap<&'static str, u32>> = OnceLock::new();

pub fn glyph_name_to_unicode(name: &str) -> Option<char> {
    let map = NAME_MAP.get_or_init(|| {
        let mut m = HashMap::new();
        for (i, n) in ASCII_NAMES.iter().enumerate() {
            m.insert(*n, 0x20 + i as u32);
        }
        for (i, n) in LATIN1_NAMES.iter().enumerate() {
            m.entry(*n).or_insert(0xA0 + i as u32);
        }
        for (n, u) in EXTRA_NAMES {
            m.insert(*n, *u);
        }
        m.insert("space", 0x20);
        m.insert("hyphen", 0x2D);
        m
    });
    if let Some(u) = map.get(name) {
        return char::from_u32(*u);
    }
    let base = name.split('.').next().unwrap_or(name);
    if base != name {
        if let Some(u) = map.get(base) {
            return char::from_u32(*u);
        }
    }
    if let Some(hex) = base.strip_prefix("uni") {
        if hex.len() >= 4 {
            return u32::from_str_radix(&hex[..4], 16).ok().and_then(char::from_u32);
        }
    }
    if let Some(hex) = base.strip_prefix('u') {
        if (4..=6).contains(&hex.len()) {
            return u32::from_str_radix(hex, 16).ok().and_then(char::from_u32);
        }
    }
    None
}

pub fn standard_encoding() -> Vec<&'static str> {
    let mut e = vec![".notdef"; 256];
    for (i, n) in ASCII_NAMES.iter().enumerate() {
        e[0x20 + i] = n;
    }
    e[0x27] = "quoteright";
    e[0x60] = "quoteleft";
    let hi: &[(usize, &str)] = &[
        (161, "exclamdown"), (162, "cent"), (163, "sterling"), (164, "fraction"), (165, "yen"), (166, "florin"), (167, "section"),
        (168, "currency"), (169, "quotesingle"), (170, "quotedblleft"), (171, "guillemotleft"), (172, "guilsinglleft"), (173, "guilsinglright"),
        (174, "fi"), (175, "fl"), (177, "endash"), (178, "dagger"), (179, "daggerdbl"), (180, "periodcentered"), (182, "paragraph"),
        (183, "bullet"), (184, "quotesinglbase"), (185, "quotedblbase"), (186, "quotedblright"), (187, "guillemotright"), (188, "ellipsis"),
        (189, "perthousand"), (191, "questiondown"), (193, "grave"), (194, "acute"), (195, "circumflex"), (196, "tilde"), (197, "macron"),
        (198, "breve"), (199, "dotaccent"), (200, "dieresis"), (202, "ring"), (203, "cedilla"), (205, "hungarumlaut"), (206, "ogonek"),
        (207, "caron"), (208, "emdash"), (225, "AE"), (227, "ordfeminine"), (232, "Lslash"), (233, "Oslash"), (234, "OE"),
        (235, "ordmasculine"), (241, "ae"), (245, "dotlessi"), (248, "lslash"), (249, "oslash"), (250, "oe"), (251, "germandbls"),
    ];
    for (c, n) in hi {
        e[*c] = n;
    }
    e
}

pub fn isolatin1_encoding() -> Vec<&'static str> {
    let mut e = vec![".notdef"; 256];
    for (i, n) in ASCII_NAMES.iter().enumerate() {
        e[0x20 + i] = n;
    }
    e[0x27] = "quoteright";
    e[0x60] = "quoteleft";
    let accents = ["dotlessi", "grave", "acute", "circumflex", "tilde", "macron", "breve", "dotaccent", "dieresis", ".notdef", "ring", "cedilla", ".notdef", "hungarumlaut", "ogonek", "caron"];
    for (i, n) in accents.iter().enumerate() {
        e[144 + i] = n;
    }
    for (i, n) in LATIN1_NAMES.iter().enumerate() {
        e[0xA0 + i] = n;
    }
    e
}

/// Windows-1252 to Unicode.
pub fn cp1252(b: u8) -> char {
    const HI: [u16; 32] = [
        0x20AC, 0x81, 0x201A, 0x0192, 0x201E, 0x2026, 0x2020, 0x2021, 0x02C6, 0x2030, 0x0160, 0x2039, 0x0152, 0x8D, 0x017D, 0x8F,
        0x90, 0x2018, 0x2019, 0x201C, 0x201D, 0x2022, 0x2013, 0x2014, 0x02DC, 0x2122, 0x0161, 0x203A, 0x0153, 0x9D, 0x017E, 0x0178,
    ];
    if (0x80..0xA0).contains(&b) {
        char::from_u32(HI[(b - 0x80) as usize] as u32).unwrap_or(' ')
    } else {
        b as char
    }
}

/// HP Roman-8 (symbol set 8U) to Unicode.
pub fn roman8(b: u8) -> char {
    const HI: [u16; 96] = [
        0x00A0, 0x00C0, 0x00C2, 0x00C8, 0x00CA, 0x00CB, 0x00CE, 0x00CF, 0x00B4, 0x02CB, 0x02C6, 0x00A8, 0x02DC, 0x00D9, 0x00DB, 0x20A4,
        0x00AF, 0x00DD, 0x00FD, 0x00B0, 0x00C7, 0x00E7, 0x00D1, 0x00F1, 0x00A1, 0x00BF, 0x00A4, 0x00A3, 0x00A5, 0x00A7, 0x0192, 0x00A2,
        0x00E2, 0x00EA, 0x00F4, 0x00FB, 0x00E1, 0x00E9, 0x00F3, 0x00FA, 0x00E0, 0x00E8, 0x00F2, 0x00F9, 0x00E4, 0x00EB, 0x00F6, 0x00FC,
        0x00C5, 0x00EE, 0x00D8, 0x00C6, 0x00E5, 0x00ED, 0x00F8, 0x00E6, 0x00C4, 0x00EC, 0x00D6, 0x00DC, 0x00C9, 0x00EF, 0x00DF, 0x00D4,
        0x00C1, 0x00C3, 0x00E3, 0x00D0, 0x00F0, 0x00CD, 0x00CC, 0x00D3, 0x00D2, 0x00D5, 0x00F5, 0x0160, 0x0161, 0x00DA, 0x0178, 0x00FF,
        0x00DE, 0x00FE, 0x00B7, 0x00B5, 0x00B6, 0x00BE, 0x2014, 0x00BC, 0x00BD, 0x00AA, 0x00BA, 0x00AB, 0x25A0, 0x00BB, 0x00B1, 0x0020,
    ];
    if b >= 0xA0 {
        char::from_u32(HI[(b - 0xA0) as usize] as u32).unwrap_or(' ')
    } else {
        b as char
    }
}

/// Adobe Symbol encoding to Unicode (common characters).
pub fn symbol_to_unicode(c: u8) -> Option<char> {
    const UPPER: &str = "ΑΒΧΔΕΦΓΗΙϑΚΛΜΝΟΠΘΡΣΤΥςΩΞΨΖ";
    const LOWER: &str = "αβχδεφγηιϕκλμνοπθρστυϖωξψζ";
    let u = match c {
        b'A'..=b'Z' => return UPPER.chars().nth((c - b'A') as usize),
        b'a'..=b'z' => return LOWER.chars().nth((c - b'a') as usize),
        34 => '∀',
        36 => '∃',
        39 => '∋',
        42 => '∗',
        45 => '−',
        64 => '≅',
        92 => '∴',
        94 => '⊥',
        126 => '∼',
        0xA2 => '′',
        0xA3 => '≤',
        0xA5 => '∞',
        0xA6 => 'ƒ',
        0xAB => '↔',
        0xAC => '←',
        0xAD => '↑',
        0xAE => '→',
        0xAF => '↓',
        0xB0 => '°',
        0xB1 => '±',
        0xB2 => '″',
        0xB3 => '≥',
        0xB4 => '×',
        0xB5 => '∝',
        0xB6 => '∂',
        0xB7 => '•',
        0xB8 => '÷',
        0xB9 => '≠',
        0xBA => '≡',
        0xBB => '≈',
        0xBC => '…',
        0xC5 => '⊕',
        0xC6 => '∅',
        0xC7 => '∩',
        0xC8 => '∪',
        0xC9 => '⊃',
        0xCA => '⊇',
        0xCC => '⊂',
        0xCD => '⊆',
        0xCE => '∈',
        0xCF => '∉',
        0xD0 => '∠',
        0xD1 => '∇',
        0xD2 => '®',
        0xD3 => '©',
        0xD4 => '™',
        0xD5 => '∏',
        0xD6 => '√',
        0xD7 => '⋅',
        0xD8 => '¬',
        0xD9 => '∧',
        0xDA => '∨',
        0xE0 => '◊',
        0xE5 => '∑',
        0xF2 => '∫',
        _ => return None,
    };
    Some(u)
}

// ---------------------------------------------------------------------------
// Identifying downloaded font subsets
// ---------------------------------------------------------------------------

/// A font that a downloaded subset was matched to: its name and glyph → character map.
pub struct Identified {
    pub name: String,
    pub map: HashMap<u16, char>,
}

struct IndexEntry {
    path: std::path::PathBuf,
    face: u32,
    glyphs: u16,
    family: String,
    full: String,
    bold: bool,
    italic: bool,
}

static FONT_INDEX: OnceLock<Vec<IndexEntry>> = OnceLock::new();
static IDENT_CACHE: OnceLock<Mutex<HashMap<u64, Option<Arc<Identified>>>>> = OnceLock::new();

fn font_dirs() -> Vec<std::path::PathBuf> {
    let mut v = Vec::new();
    let windir = std::env::var("WINDIR").or_else(|_| std::env::var("SystemRoot")).unwrap_or_else(|_| "C:\\Windows".into());
    v.push(std::path::PathBuf::from(format!("{windir}\\Fonts")));
    if let Ok(l) = std::env::var("LOCALAPPDATA") {
        v.push(std::path::PathBuf::from(format!("{l}\\Microsoft\\Windows\\Fonts")));
    }
    for d in ["/System/Library/Fonts", "/System/Library/Fonts/Supplemental", "/Library/Fonts", "/Applications/Microsoft Word.app/Contents/Resources/DFonts", "/usr/share/fonts"] {
        v.push(d.into());
    }
    if let Ok(h) = std::env::var("HOME") {
        v.push(std::path::PathBuf::from(format!("{h}/Library/Fonts")));
    }
    v
}

fn build_index() -> Vec<IndexEntry> {
    let mut out = Vec::new();
    let mut stack = font_dirs();
    let mut seen = 0;
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                if seen < 2000 {
                    stack.push(p);
                }
                continue;
            }
            let ext = p.extension().map(|x| x.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
            if !matches!(ext.as_str(), "ttf" | "ttc" | "otf") {
                continue;
            }
            seen += 1;
            let Ok(data) = std::fs::read(&p) else { continue };
            let n = ttf_parser::fonts_in_collection(&data).unwrap_or(1);
            for i in 0..n.min(64) {
                if let Ok(f) = ttf_parser::Face::parse(&data, i) {
                    let name = |id: u16| f.names().into_iter().find(|n| n.name_id == id && n.is_unicode()).and_then(|n| n.to_string()).unwrap_or_default();
                    let family = {
                        let typo = name(ttf_parser::name_id::TYPOGRAPHIC_FAMILY);
                        if typo.is_empty() { name(ttf_parser::name_id::FAMILY) } else { typo }
                    };
                    out.push(IndexEntry {
                        path: p.clone(),
                        face: i,
                        glyphs: if f.tables().glyf.is_some() { f.number_of_glyphs() } else { 0 },
                        family: family.to_lowercase(),
                        full: name(ttf_parser::name_id::FULL_NAME).to_lowercase(),
                        bold: f.is_bold(),
                        italic: f.is_italic(),
                    });
                }
            }
        }
    }
    out
}

fn raw_glyph<'a>(face: &ttf_parser::Face<'a>, gid: u16) -> Option<&'a [u8]> {
    let raw = face.raw_face();
    let head = raw.table(ttf_parser::Tag::from_bytes(b"head"))?;
    let loca = raw.table(ttf_parser::Tag::from_bytes(b"loca"))?;
    let glyf = raw.table(ttf_parser::Tag::from_bytes(b"glyf"))?;
    let long = bei16(head, 50).unwrap_or(0) != 0;
    let g = gid as usize;
    let (a, b) = if long { (be32(loca, g * 4)? as usize, be32(loca, g * 4 + 4)? as usize) } else { (be16(loca, g * 2)? as usize * 2, be16(loca, g * 2 + 2)? as usize * 2) };
    if b <= a || b > glyf.len() {
        return None;
    }
    Some(&glyf[a..b])
}

fn trim0(d: &[u8]) -> &[u8] {
    let mut n = d.len();
    while n > 0 && d[n - 1] == 0 {
        n -= 1;
    }
    &d[..n]
}

/// Find the installed font a subset came from by comparing raw glyph outlines
/// (`samples` are glyph id + glyf bytes). Returns its name and a reverse cmap.
pub fn identify_subset(samples: &[(u16, Vec<u8>)]) -> Option<Arc<Identified>> {
    let mut s: Vec<&(u16, Vec<u8>)> = samples.iter().filter(|(_, d)| d.len() > 12).collect();
    if s.len() < 3 {
        return None;
    }
    s.sort_by_key(|(_, d)| std::cmp::Reverse(d.len()));
    s.truncate(12);
    let key = {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        for (g, d) in &s {
            g.hash(&mut h);
            d.hash(&mut h);
        }
        h.finish()
    };
    let cache = IDENT_CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some(r) = cache.lock().unwrap().get(&key) {
        return r.clone();
    }
    let max_gid = s.iter().map(|(g, _)| *g).max().unwrap_or(0);
    let index = FONT_INDEX.get_or_init(build_index);
    let started = std::time::Instant::now();
    let mut result = None;
    let mut last_path: Option<(&std::path::Path, Vec<u8>)> = None;
    for e in index.iter().filter(|e| e.glyphs > max_gid) {
        if started.elapsed().as_secs() > 6 {
            break;
        }
        if last_path.as_ref().map(|(p, _)| *p != e.path.as_path()).unwrap_or(true) {
            let Ok(d) = std::fs::read(&e.path) else { continue };
            last_path = Some((e.path.as_path(), d));
        }
        let data = &last_path.as_ref().unwrap().1;
        let Ok(face) = ttf_parser::Face::parse(data, e.face) else { continue };
        let ok = s.iter().all(|(g, d)| raw_glyph(&face, *g).map(|r| trim0(r) == trim0(d)).unwrap_or(false));
        if !ok {
            continue;
        }
        let mut map = HashMap::new();
        if let Some(cmap) = face.tables().cmap {
            for st in cmap.subtables {
                if !st.is_unicode() {
                    continue;
                }
                st.codepoints(|cp| {
                    if let (Some(ch), Some(g)) = (char::from_u32(cp), st.glyph_index(cp)) {
                        let prefer = map.get(&g.0).map(|old: &char| (*old as u32) > cp).unwrap_or(true);
                        if prefer {
                            map.insert(g.0, ch);
                        }
                    }
                });
            }
        }
        let name = face
            .names()
            .into_iter()
            .find(|n| n.name_id == ttf_parser::name_id::FULL_NAME && n.is_unicode())
            .and_then(|n| n.to_string())
            .unwrap_or_else(|| e.path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default());
        result = Some(Arc::new(Identified { name, map }));
        break;
    }
    cache.lock().unwrap().insert(key, result.clone());
    result
}

/// PC-8 (code page 437, symbol set 10U) to Unicode.
pub fn pc8(b: u8) -> char {
    const HI: &str = "ÇüéâäàåçêëèïîìÄÅÉæÆôöòûùÿÖÜ¢£¥₧ƒáíóúñÑªº¿⌐¬½¼¡«»░▒▓│┤╡╢╖╕╣║╗╝╜╛┐└┴┬├─┼╞╟╚╔╩╦╠═╬╧╨╤╥╙╘╒╓╫╪┘┌█▄▌▐▀αßΓπΣσµτΦΘΩδ∞φε∩≡±≥≤⌠⌡÷≈°∙·√ⁿ²■\u{a0}";
    if b >= 0x80 {
        HI.chars().nth((b - 0x80) as usize).unwrap_or(' ')
    } else {
        b as char
    }
}

/// A font held in memory (embedded in a job, or an installed font file).
#[derive(Clone)]
pub struct OwnedFont {
    pub data: Arc<Vec<u8>>,
    pub index: u32,
    pub upem: f64,
    pub name: String,
}

impl OwnedFont {
    pub fn new(data: Vec<u8>, index: u32) -> Option<OwnedFont> {
        let f = ttf_parser::Face::parse(&data, index).ok()?;
        let upem = f.units_per_em() as f64;
        let name = f.names().into_iter().find(|n| n.name_id == ttf_parser::name_id::FULL_NAME && n.is_unicode()).and_then(|n| n.to_string()).unwrap_or_default();
        Some(OwnedFont { data: Arc::new(data), index, upem, name })
    }
    pub fn face(&self) -> Option<ttf_parser::Face<'_>> {
        ttf_parser::Face::parse(&self.data, self.index).ok()
    }
    pub fn glyph_for_char(&self, ch: char) -> Option<u16> {
        let f = self.face()?;
        let g = f.glyph_index(ch).or_else(|| if (ch as u32) < 256 { f.glyph_index(char::from_u32(0xF000 + ch as u32)?) } else { None })?;
        if g.0 == 0 { None } else { Some(g.0) }
    }
    pub fn outline(&self, gid: u16, m: &Matrix, out: &mut PathAcc) {
        if let Some(f) = self.face() {
            let mut b = Builder { out, m: *m, cur: (0.0, 0.0) };
            f.outline_glyph(ttf_parser::GlyphId(gid), &mut b);
        }
    }
    pub fn advance(&self, gid: u16) -> f64 {
        self.face().and_then(|f| f.glyph_hor_advance(ttf_parser::GlyphId(gid))).unwrap_or(0) as f64
    }
    pub fn ascent_descent(&self) -> (f64, f64) {
        self.face().map(|f| (f.ascender() as f64, -(f.descender() as f64))).unwrap_or((self.upem * 0.8, self.upem * 0.2))
    }
    /// Unicode → glyph map inverted (glyph → first character), for text extraction.
    pub fn reverse_cmap(&self) -> HashMap<u16, char> {
        let mut map = HashMap::new();
        if let Some(f) = self.face() {
            if let Some(cmap) = f.tables().cmap {
                for st in cmap.subtables {
                    if !st.is_unicode() {
                        continue;
                    }
                    st.codepoints(|cp| {
                        if let (Some(ch), Some(g)) = (char::from_u32(cp), st.glyph_index(cp)) {
                            map.entry(g.0).or_insert(ch);
                        }
                    });
                }
            }
        }
        map
    }
}

static OWNED_CACHE: OnceLock<Mutex<HashMap<(String, bool, bool), Option<OwnedFont>>>> = OnceLock::new();

/// Find an installed font by family name (as Windows names it, e.g. "Calibri").
pub fn find_installed(family: &str, bold: bool, italic: bool) -> Option<OwnedFont> {
    let fam = family.trim().trim_start_matches('@').to_lowercase();
    if fam.is_empty() {
        return None;
    }
    let cache = OWNED_CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    let key = (fam.clone(), bold, italic);
    if let Some(v) = cache.lock().unwrap().get(&key) {
        return v.clone();
    }
    let index = FONT_INDEX.get_or_init(build_index);
    let mut cands: Vec<&IndexEntry> = index.iter().filter(|e| e.family == fam || e.full == fam).collect();
    cands.sort_by_key(|e| (e.bold != bold) as u8 * 2 + (e.italic != italic) as u8);
    let r = cands.first().and_then(|e| std::fs::read(&e.path).ok().and_then(|d| OwnedFont::new(d, e.face)));
    cache.lock().unwrap().insert(key, r.clone());
    r
}

/// Remove XPS font obfuscation (the first 32 bytes are XORed with the GUID in the part name).
pub fn deobfuscate_odttf(data: &mut [u8], part_name: &str) {
    let stem = part_name.rsplit('/').next().unwrap_or(part_name);
    let stem = stem.split('.').next().unwrap_or(stem);
    let hex: String = stem.chars().filter(|c| c.is_ascii_hexdigit()).collect();
    if hex.len() != 32 || data.len() < 32 {
        return;
    }
    let byte = |i: usize| u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).unwrap_or(0);
    // GUID string bytes in their binary (mixed-endian) order
    let order = [3usize, 2, 1, 0, 5, 4, 7, 6, 8, 9, 10, 11, 12, 13, 14, 15];
    let key: Vec<u8> = order.iter().map(|&i| byte(i)).collect();
    let mut trial = data[..32].to_vec();
    for i in 0..32 {
        trial[i] ^= key[15 - (i % 16)];
    }
    let ok = |d: &[u8]| d.starts_with(&[0, 1, 0, 0]) || d.starts_with(b"true") || d.starts_with(b"OTTO") || d.starts_with(b"ttcf");
    if ok(&trial) {
        data[..32].copy_from_slice(&trial);
        return;
    }
    // fall back to the plain string order
    let key2: Vec<u8> = (0..16).map(byte).collect();
    let mut trial2 = data[..32].to_vec();
    for i in 0..32 {
        trial2[i] ^= key2[15 - (i % 16)];
    }
    if ok(&trial2) {
        data[..32].copy_from_slice(&trial2);
    }
}
