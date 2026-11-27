//! Format-independent page display lists and job information.

use std::sync::Arc;
use tiny_skia::{LineCap, LineJoin, Path, Transform};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rgb(pub f32, pub f32, pub f32);

impl Rgb {
    pub const BLACK: Rgb = Rgb(0.0, 0.0, 0.0);
    pub fn gray(g: f32) -> Rgb {
        Rgb(g, g, g)
    }
    pub fn clamp(self) -> Rgb {
        Rgb(self.0.clamp(0.0, 1.0), self.1.clamp(0.0, 1.0), self.2.clamp(0.0, 1.0))
    }
    pub fn from_cmyk(c: f32, m: f32, y: f32, k: f32) -> Rgb {
        Rgb((1.0 - c) * (1.0 - k), (1.0 - m) * (1.0 - k), (1.0 - y) * (1.0 - k)).clamp()
    }
}

/// An RGBA raster with premultiplied alpha. Large rasters are kept
/// zlib-compressed in memory and expanded when a page is rendered.
pub struct RasterImage {
    pub width: u32,
    pub height: u32,
    data: ImgData,
}

enum ImgData {
    Raw(Vec<u8>),
    Zip(Vec<u8>),
}

/// Rasters are reduced to at most this resolution when loaded.
pub const MAX_IMAGE_DPI: f64 = 300.0;

impl RasterImage {
    pub fn new(width: u32, height: u32, rgba: Vec<u8>) -> RasterImage {
        let data = if rgba.len() > 256 * 1024 {
            use std::io::Write;
            let mut e = flate2::write::ZlibEncoder::new(Vec::with_capacity(rgba.len() / 8), flate2::Compression::fast());
            if e.write_all(&rgba).is_ok() {
                match e.finish() {
                    Ok(z) => ImgData::Zip(z),
                    Err(_) => ImgData::Raw(rgba),
                }
            } else {
                ImgData::Raw(rgba)
            }
        } else {
            ImgData::Raw(rgba)
        };
        RasterImage { width, height, data }
    }

    pub fn rgba(&self) -> std::borrow::Cow<'_, [u8]> {
        match &self.data {
            ImgData::Raw(v) => std::borrow::Cow::Borrowed(v),
            ImgData::Zip(z) => {
                use std::io::Read;
                let mut out = Vec::with_capacity((self.width * self.height * 4) as usize);
                let _ = flate2::read::ZlibDecoder::new(&z[..]).read_to_end(&mut out);
                out.resize((self.width * self.height * 4) as usize, 0);
                std::borrow::Cow::Owned(out)
            }
        }
    }

    /// Build an image whose pixels map to the page through `t` (pixel -> points),
    /// reducing resolution above MAX_IMAGE_DPI. Returns the image and adjusted transform.
    pub fn fitted(w: u32, h: u32, rgba: Vec<u8>, t: &crate::geom::Matrix) -> (RasterImage, crate::geom::Matrix) {
        let px_w = (t.a * t.a + t.b * t.b).sqrt().max(1e-9); // points per pixel (x)
        let px_h = (t.c * t.c + t.d * t.d).sqrt().max(1e-9);
        let kx = ((72.0 / px_w) / MAX_IMAGE_DPI).floor().max(1.0) as u32;
        let ky = (((72.0 / px_h) / MAX_IMAGE_DPI).floor().max(1.0) as u32).min(h.max(1));
        if (kx <= 1 && ky <= 1) || w < 16 {
            return (RasterImage::new(w, h, rgba), *t);
        }
        let kx = kx.min(w.max(1));
        let nw = (w / kx).max(1);
        let nh = (h / ky).max(1);
        let mut out = vec![0u8; (nw * nh * 4) as usize];
        let n = (kx * ky) as u32;
        for y in 0..nh {
            for x in 0..nw {
                let mut acc = [0u32; 4];
                for yy in 0..ky {
                    let row = ((y * ky + yy) * w) as usize;
                    for xx in 0..kx {
                        let o = (row + (x * kx + xx) as usize) * 4;
                        if o + 3 < rgba.len() {
                            acc[0] += rgba[o] as u32;
                            acc[1] += rgba[o + 1] as u32;
                            acc[2] += rgba[o + 2] as u32;
                            acc[3] += rgba[o + 3] as u32;
                        }
                    }
                }
                let o = ((y * nw + x) * 4) as usize;
                for c in 0..4 {
                    out[o + c] = (acc[c] / n) as u8;
                }
            }
        }
        let adj = crate::geom::Matrix::scale((nw * kx) as f64 / nw as f64, (nh * ky) as f64 / nh as f64).then(t);
        (RasterImage::new(nw, nh, out), adj)
    }
}

#[derive(Clone)]
pub enum Brush {
    Solid(Rgb),
    /// Tiled image; `transform` maps image pixels to page points.
    Pattern { image: Arc<RasterImage>, transform: Transform },
}

#[derive(Clone)]
pub struct ClipPath {
    /// In page points.
    pub path: Path,
    pub even_odd: bool,
}

/// Intersection of clip paths; `None` means unclipped.
pub type Clip = Option<Arc<Vec<ClipPath>>>;

#[derive(Clone)]
pub struct StrokeStyle {
    pub width: f32,
    pub cap: LineCap,
    pub join: LineJoin,
    pub miter: f32,
    pub dash: Option<(Vec<f32>, f32)>,
}

pub enum Item {
    Fill { path: Path, transform: Transform, brush: Brush, even_odd: bool, clip: Clip },
    Stroke { path: Path, transform: Transform, brush: Brush, style: StrokeStyle, clip: Clip },
    /// `transform` maps image pixel space (0..w, 0..h, row 0 first) to page points.
    Image { image: Arc<RasterImage>, transform: Transform, clip: Clip },
}

/// What kind of content an item is (for layer toggles).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Vector = 1,
    Text = 2,
    Image = 4,
}

/// Where an item came from: a byte offset in the file and its content kind.
#[derive(Clone, Copy, Debug)]
pub struct ItemMeta {
    pub src: u32,
    pub kind: Kind,
}

pub const NO_SRC: u32 = u32::MAX;

/// A run of text with one box (page points, y down) per character.
#[derive(Clone, Debug, Default)]
pub struct TextRun {
    pub text: Vec<char>,
    pub boxes: Vec<[f32; 4]>,
}

impl TextRun {
    pub fn bounds(&self) -> [f32; 4] {
        let mut b = [f32::MAX, f32::MAX, f32::MIN, f32::MIN];
        for r in &self.boxes {
            b[0] = b[0].min(r[0]);
            b[1] = b[1].min(r[1]);
            b[2] = b[2].max(r[2]);
            b[3] = b[3].max(r[3]);
        }
        b
    }
}

pub struct Page {
    /// Page size in points (1/72 inch).
    pub width: f32,
    pub height: f32,
    pub items: Vec<Item>,
    /// Parallel to `items`.
    pub meta: Vec<ItemMeta>,
    /// Text found on the page, in drawing order.
    pub text: Vec<TextRun>,
    /// Short description shown in page list / info.
    pub label: String,
    /// Settings that apply to this page (paper source, duplex, copies, …).
    pub settings: Vec<(String, String)>,
    /// Source offset and kind given to items pushed with `push`.
    pub cur_src: u32,
    pub cur_kind: Kind,
    text_open: bool,
}

impl Page {
    pub fn new(width: f32, height: f32) -> Self {
        Page {
            width,
            height,
            items: Vec::new(),
            meta: Vec::new(),
            text: Vec::new(),
            label: String::new(),
            settings: Vec::new(),
            cur_src: NO_SRC,
            cur_kind: Kind::Vector,
            text_open: false,
        }
    }

    pub fn push(&mut self, item: Item) {
        let kind = match (&item, self.cur_kind) {
            (_, Kind::Text) => Kind::Text,
            (Item::Image { .. }, _) => Kind::Image,
            _ => Kind::Vector,
        };
        self.items.push(item);
        self.meta.push(ItemMeta { src: self.cur_src, kind });
    }

    pub fn meta_of(&self, i: usize) -> ItemMeta {
        self.meta.get(i).copied().unwrap_or(ItemMeta { src: NO_SRC, kind: Kind::Vector })
    }

    /// Record one character. `origin` is the baseline start, `end` the baseline
    /// end (after the advance) and `up` the vector from baseline to the top of
    /// the em square, all in page points (y down).
    pub fn text_char(&mut self, ch: char, origin: (f64, f64), end: (f64, f64), up: (f64, f64)) {
        if self.text.len() > 200_000 {
            return;
        }
        let down = (-up.0 * 0.25, -up.1 * 0.25);
        let up = (up.0 * 0.85, up.1 * 0.85);
        let pts = [
            (origin.0 + down.0, origin.1 + down.1),
            (end.0 + down.0, end.1 + down.1),
            (origin.0 + up.0, origin.1 + up.1),
            (end.0 + up.0, end.1 + up.1),
        ];
        let mut b = [f32::MAX, f32::MAX, f32::MIN, f32::MIN];
        for (x, y) in pts {
            b[0] = b[0].min(x as f32);
            b[1] = b[1].min(y as f32);
            b[2] = b[2].max(x as f32);
            b[3] = b[3].max(y as f32);
        }
        if !b.iter().all(|v| v.is_finite()) {
            return;
        }
        if !self.text_open || self.text.is_empty() {
            self.text.push(TextRun::default());
            self.text_open = true;
        }
        let r = self.text.last_mut().unwrap();
        r.text.push(ch);
        r.boxes.push(b);
    }

    /// End the current text run; the next character starts a new one.
    pub fn text_break(&mut self) {
        self.text_open = false;
    }

    /// The page's text in reading order: runs grouped into lines by position.
    pub fn plain_text(&self) -> String {
        let mut runs: Vec<(f32, f32, f32, String)> = self
            .text
            .iter()
            .filter(|r| !r.text.is_empty())
            .map(|r| {
                let b = r.bounds();
                (b[1], b[3], b[0], r.text.iter().collect::<String>())
            })
            .collect();
        runs.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
        let mut lines: Vec<(f32, f32, Vec<(f32, String)>)> = Vec::new();
        for (top, bot, x, t) in runs {
            let mid = (top + bot) / 2.0;
            if let Some(l) = lines.iter_mut().rev().take(3).find(|l| mid > l.0 && mid < l.1) {
                l.2.push((x, t));
            } else {
                lines.push((top, bot, vec![(x, t)]));
            }
        }
        lines.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
        let mut out = String::new();
        for (_, _, mut parts) in lines {
            parts.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
            let line: Vec<String> = parts.into_iter().map(|p| p.1).collect();
            let mut s = String::new();
            for p in line {
                if !s.is_empty() && !s.ends_with(' ') && !p.starts_with(' ') {
                    s.push(' ');
                }
                s.push_str(&p);
            }
            out.push_str(s.trim_end());
            out.push('\n');
        }
        out
    }
}

#[derive(Default, Clone)]
pub struct InfoSection {
    pub title: String,
    pub rows: Vec<(String, String)>,
}

impl InfoSection {
    pub fn new(title: impl Into<String>) -> Self {
        InfoSection { title: title.into(), rows: Vec::new() }
    }
    pub fn add(&mut self, k: impl Into<String>, v: impl Into<String>) {
        self.rows.push((k.into(), v.into()));
    }
}

#[derive(Default)]
pub struct Document {
    pub format: String,
    pub pages: Vec<Page>,
    pub info: Vec<InfoSection>,
    /// Decoded command listing (for the Commands tab).
    pub commands: Vec<String>,
    /// File offset of each command line (parallel to `commands`, NO_SRC if none).
    pub cmd_offsets: Vec<u32>,
    /// Warnings and unsupported features encountered.
    pub warnings: Vec<String>,
    /// The file's bytes.
    pub raw: Arc<Vec<u8>>,
    /// Where the PJL header ends and printer-language data starts (0 if no PJL).
    pub pjl_end: usize,
    /// PJL settings, last value wins (upper-case keys).
    pub pjl: Vec<(String, String)>,
    /// Job settings from a Windows shadow (.SHD) file, if one was found.
    pub shd: Option<crate::shd::Shadow>,
    /// Per-page coverage (may be shorter than `pages` if analysis was cut short).
    pub coverage: Vec<crate::analyze::Coverage>,
    pub findings: Vec<crate::analyze::Finding>,
}

impl Document {
    /// Add a command-listing line for the data at `off`.
    pub fn cmd(&mut self, off: usize, s: String) {
        if self.commands.len() < MAX_COMMAND_LINES {
            self.commands.push(s);
            self.cmd_offsets.push(off.min(NO_SRC as usize - 1) as u32);
        }
    }

    /// Index of the command line for file offset `off` (the last line at or before it).
    pub fn cmd_index_for(&self, off: u32) -> Option<usize> {
        if self.cmd_offsets.is_empty() || off == NO_SRC {
            return None;
        }
        let i = self.cmd_offsets.partition_point(|&o| o == NO_SRC || o <= off);
        Some(i.saturating_sub(1))
    }

    pub fn pjl_get(&self, key: &str) -> Option<&str> {
        self.pjl.iter().rev().find(|(k, _)| k.eq_ignore_ascii_case(key)).map(|(_, v)| v.as_str())
    }

    pub fn warn(&mut self, w: impl Into<String>) {
        let w = w.into();
        if self.warnings.len() < 2000 && !self.warnings.contains(&w) {
            self.warnings.push(w);
        }
    }
}

pub const MAX_COMMAND_LINES: usize = 500_000;

pub fn fmt_size_pt(w: f32, h: f32) -> String {
    format!(
        "{:.1} × {:.1} pt  ({:.1} × {:.1} mm, {:.2} × {:.2} in){}",
        w,
        h,
        w / 72.0 * 25.4,
        h / 72.0 * 25.4,
        w / 72.0,
        h / 72.0,
        paper_name(w, h).map(|n| format!("  [{n}]")).unwrap_or_default()
    )
}

pub fn paper_name(w: f32, h: f32) -> Option<&'static str> {
    const SIZES: &[(&str, f32, f32)] = &[
        ("Letter", 612.0, 792.0),
        ("Legal", 612.0, 1008.0),
        ("Executive", 522.0, 756.0),
        ("Ledger/Tabloid", 792.0, 1224.0),
        ("A3", 841.9, 1190.6),
        ("A4", 595.3, 841.9),
        ("A5", 419.5, 595.3),
        ("A6", 297.6, 419.5),
        ("B4 (JIS)", 728.5, 1031.8),
        ("B5 (JIS)", 515.9, 728.5),
        ("B5 (ISO)", 498.9, 708.7),
        ("Envelope #10", 297.0, 684.0),
        ("Envelope DL", 311.8, 623.6),
        ("Envelope C5", 459.2, 649.1),
        ("Envelope Monarch", 279.0, 540.0),
        ("Statement", 396.0, 612.0),
        ("Folio", 612.0, 936.0),
    ];
    let (a, b) = if w <= h { (w, h) } else { (h, w) };
    SIZES
        .iter()
        .find(|(_, sw, sh)| (sw - a).abs() < 3.0 && (sh - b).abs() < 3.0)
        .map(|(n, _, _)| *n)
}

pub fn fmt_bytes(n: usize) -> String {
    if n < 1024 {
        format!("{n} bytes")
    } else if n < 1024 * 1024 {
        format!("{:.1} KB ({n} bytes)", n as f64 / 1024.0)
    } else {
        format!("{:.2} MB ({n} bytes)", n as f64 / 1048576.0)
    }
}
