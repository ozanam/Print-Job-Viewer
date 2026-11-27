//! Windows EMF: stand-alone metafiles and EMF spool files ("NT EMF 1.0xx"
//! data type, one metafile per page, with fonts and DEVMODE records).

use crate::doc::*;
use crate::fonts::{self, OwnedFont, SysFont};
use crate::geom::*;
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use tiny_skia::{LineCap, LineJoin, Transform};

fn u32_at(d: &[u8], o: usize) -> u32 {
    d.get(o..o + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]])).unwrap_or(0)
}
fn i32_at(d: &[u8], o: usize) -> i32 {
    u32_at(d, o) as i32
}
fn i16_at(d: &[u8], o: usize) -> i16 {
    d.get(o..o + 2).map(|b| i16::from_le_bytes([b[0], b[1]])).unwrap_or(0)
}
fn f32_at(d: &[u8], o: usize) -> f64 {
    f32::from_bits(u32_at(d, o)) as f64
}
fn colorref(v: u32) -> Rgb {
    Rgb((v & 255) as f32 / 255.0, ((v >> 8) & 255) as f32 / 255.0, ((v >> 16) & 255) as f32 / 255.0)
}
fn wstr(d: &[u8], o: usize, max_chars: usize) -> String {
    let mut v = Vec::new();
    for i in 0..max_chars {
        let Some(b) = d.get(o + i * 2..o + i * 2 + 2) else { break };
        let c = u16::from_le_bytes([b[0], b[1]]);
        if c == 0 {
            break;
        }
        v.push(c);
    }
    String::from_utf16_lossy(&v)
}

pub fn is_emf(d: &[u8]) -> bool {
    d.len() > 88 && u32_at(d, 0) == 1 && &d[40..44] == b" EMF"
}

pub fn is_emf_spool(d: &[u8]) -> bool {
    if d.len() < 24 || u32_at(d, 0) != 0x0001_0000 {
        return false;
    }
    let cb = u32_at(d, 4) as usize;
    cb >= 8 && cb < d.len() && (1..=21).contains(&u32_at(d, cb))
}

#[derive(Clone)]
struct LogFont {
    height: i32,
    width: i32,
    escapement: i32,
    weight: i32,
    italic: bool,
    underline: bool,
    strikeout: bool,
    face: String,
}

#[derive(Clone)]
enum Obj {
    Pen { null: bool, width: f64, colour: Rgb, style: u32, cosmetic: bool },
    Brush { null: bool, colour: Rgb, hatched: bool, pattern: Option<Arc<RasterImage>> },
    Font(LogFont),
    Other,
}

#[derive(Clone)]
struct Dc {
    map_mode: i32,
    win_org: (f64, f64),
    win_ext: (f64, f64),
    vp_org: (f64, f64),
    vp_ext: (f64, f64),
    world: Matrix,
    pen: Obj,
    brush: Obj,
    font: LogFont,
    text_colour: Rgb,
    bk_colour: Rgb,
    bk_mode: i32,
    text_align: u32,
    poly_fill: i32,
    cur: (f64, f64),
    clip: Clip,
    arc_ccw: bool,
    miter: f64,
}

fn default_font() -> LogFont {
    LogFont { height: -12, width: 0, escapement: 0, weight: 400, italic: false, underline: false, strikeout: false, face: "Arial".into() }
}

fn stock(i: u32) -> Obj {
    match i & 0x7FFF_FFFF {
        0 => Obj::Brush { null: false, colour: Rgb(1.0, 1.0, 1.0), hatched: false, pattern: None },
        1 => Obj::Brush { null: false, colour: Rgb::gray(0.75), hatched: false, pattern: None },
        2 => Obj::Brush { null: false, colour: Rgb::gray(0.5), hatched: false, pattern: None },
        3 => Obj::Brush { null: false, colour: Rgb::gray(0.25), hatched: false, pattern: None },
        4 => Obj::Brush { null: false, colour: Rgb::BLACK, hatched: false, pattern: None },
        5 => Obj::Brush { null: true, colour: Rgb::BLACK, hatched: false, pattern: None },
        6 => Obj::Pen { null: false, width: 0.0, colour: Rgb(1.0, 1.0, 1.0), style: 0, cosmetic: true },
        7 => Obj::Pen { null: false, width: 0.0, colour: Rgb::BLACK, style: 0, cosmetic: true },
        8 => Obj::Pen { null: true, width: 0.0, colour: Rgb::BLACK, style: 5, cosmetic: true },
        10 | 11 | 16 => Obj::Font(LogFont { face: "Courier New".into(), ..default_font() }),
        12 | 13 | 14 | 17 => Obj::Font(default_font()),
        18 => Obj::Brush { null: false, colour: Rgb(1.0, 1.0, 1.0), hatched: false, pattern: None },
        19 => Obj::Pen { null: false, width: 0.0, colour: Rgb::BLACK, style: 0, cosmetic: true },
        _ => Obj::Other,
    }
}

impl Default for Dc {
    fn default() -> Self {
        Dc {
            map_mode: 1,
            win_org: (0.0, 0.0),
            win_ext: (1.0, 1.0),
            vp_org: (0.0, 0.0),
            vp_ext: (1.0, 1.0),
            world: Matrix::identity(),
            pen: stock(7),
            brush: stock(0),
            font: default_font(),
            text_colour: Rgb::BLACK,
            bk_colour: Rgb(1.0, 1.0, 1.0),
            bk_mode: 2,
            text_align: 0,
            poly_fill: 1,
            cur: (0.0, 0.0),
            clip: None,
            arc_ccw: true,
            miter: 10.0,
        }
    }
}

#[derive(Clone)]
enum EFont {
    Owned(OwnedFont),
    Sys(Arc<SysFont>),
}

impl EFont {
    fn upem(&self) -> f64 {
        match self {
            EFont::Owned(f) => f.upem,
            EFont::Sys(f) => f.upem,
        }
    }
    fn glyph(&self, c: char) -> Option<u16> {
        match self {
            EFont::Owned(f) => f.glyph_for_char(c),
            EFont::Sys(f) => f.glyph_for_char(c).map(|g| g.0),
        }
    }
    fn outline(&self, g: u16, m: &Matrix, out: &mut PathAcc) {
        match self {
            EFont::Owned(f) => f.outline(g, m, out),
            EFont::Sys(f) => f.outline(ttf_parser::GlyphId(g), m, out),
        }
    }
    fn advance(&self, g: u16) -> f64 {
        match self {
            EFont::Owned(f) => f.advance(g),
            EFont::Sys(f) => f.advance(ttf_parser::GlyphId(g)),
        }
    }
    fn asc_desc(&self) -> (f64, f64) {
        match self {
            EFont::Owned(f) => f.ascent_descent(),
            EFont::Sys(f) => (f.face.ascender() as f64, -(f.face.descender() as f64)),
        }
    }
}

pub struct Player<'d> {
    doc: &'d mut Document,
    page: Page,
    dc: Dc,
    saved: Vec<Dc>,
    objects: HashMap<u32, Obj>,
    dev_to_pt: Matrix,
    dev_px_per_mm: (f64, f64),
    path: Option<PathAcc>,
    path_done: Option<PathAcc>,
    embedded: HashMap<String, Vec<OwnedFont>>,
    font_cache: HashMap<(String, bool, bool), (EFont, bool)>,
    stats: BTreeMap<String, usize>,
    fonts_used: BTreeMap<String, String>,
    emf_plus_only: bool,
    base: usize,
}

impl<'d> Player<'d> {
    pub fn new(doc: &'d mut Document) -> Self {
        Player {
            doc,
            page: Page::new(612.0, 792.0),
            dc: Dc::default(),
            saved: Vec::new(),
            objects: HashMap::new(),
            dev_to_pt: Matrix::identity(),
            dev_px_per_mm: (11.81, 11.81),
            path: None,
            path_done: None,
            embedded: HashMap::new(),
            font_cache: HashMap::new(),
            stats: BTreeMap::new(),
            fonts_used: BTreeMap::new(),
            emf_plus_only: false,
            base: 0,
        }
    }

    fn stat(&mut self, k: &str) {
        *self.stats.entry(k.to_string()).or_insert(0) += 1;
    }

    // ---------------------------------------------------------- transforms

    fn page_to_device(&self) -> Matrix {
        let d = &self.dc;
        let (px, py) = self.dev_px_per_mm;
        let (sx, sy) = match d.map_mode {
            2 => (px / 10.0, -py / 10.0),
            3 => (px / 100.0, -py / 100.0),
            4 => (px * 0.254, -py * 0.254),
            5 => (px * 0.0254, -py * 0.0254),
            6 => (px * 25.4 / 1440.0, -py * 25.4 / 1440.0),
            7 | 8 => {
                let sx = d.vp_ext.0 / if d.win_ext.0 == 0.0 { 1.0 } else { d.win_ext.0 };
                let sy = d.vp_ext.1 / if d.win_ext.1 == 0.0 { 1.0 } else { d.win_ext.1 };
                if d.map_mode == 7 {
                    let k = sx.abs().min(sy.abs());
                    (k * sx.signum(), k * sy.signum())
                } else {
                    (sx, sy)
                }
            }
            _ => (1.0, 1.0),
        };
        Matrix::translate(-d.win_org.0, -d.win_org.1).then(&Matrix::scale(sx, sy)).then(&Matrix::translate(d.vp_org.0, d.vp_org.1))
    }

    /// Logical → page points.
    fn xf(&self) -> Matrix {
        self.dc.world.then(&self.page_to_device()).then(&self.dev_to_pt)
    }

    // ---------------------------------------------------------- painting

    fn brush_paint(&self) -> Option<Brush> {
        match &self.dc.brush {
            Obj::Brush { null: true, .. } => None,
            Obj::Brush { pattern: Some(img), .. } => {
                let m = Matrix::scale(1.0, 1.0).then(&self.page_to_device()).then(&self.dev_to_pt);
                Some(Brush::Pattern { image: img.clone(), transform: m.to_ts() })
            }
            Obj::Brush { colour, hatched, .. } => {
                if *hatched {
                    Some(Brush::Solid(Rgb(1.0 - (1.0 - colour.0) * 0.35, 1.0 - (1.0 - colour.1) * 0.35, 1.0 - (1.0 - colour.2) * 0.35)))
                } else {
                    Some(Brush::Solid(*colour))
                }
            }
            _ => Some(Brush::Solid(Rgb(1.0, 1.0, 1.0))),
        }
    }

    fn fill(&mut self, acc: &PathAcc) {
        if acc.is_empty() {
            return;
        }
        let Some(brush) = self.brush_paint() else { return };
        if let Some(path) = acc.to_path(&Matrix::identity()) {
            self.page.push(Item::Fill { path, transform: Transform::identity(), brush, even_odd: self.dc.poly_fill == 1, clip: self.dc.clip.clone() });
        }
    }

    fn stroke(&mut self, acc: &PathAcc) {
        if acc.is_empty() {
            return;
        }
        let Obj::Pen { null, width, colour, style, cosmetic } = self.dc.pen.clone() else { return };
        if null || style & 0xF == 5 {
            return;
        }
        let scale = self.xf().mean_scale();
        let w = if cosmetic || width <= 0.0 { 0.0 } else { width * scale };
        let unit = w.max(0.5);
        let dash = match style & 0xF {
            1 => Some(vec![unit * 6.0, unit * 2.0]),
            2 => Some(vec![unit, unit * 2.0]),
            3 => Some(vec![unit * 6.0, unit * 2.0, unit, unit * 2.0]),
            4 => Some(vec![unit * 6.0, unit * 2.0, unit, unit * 2.0, unit, unit * 2.0]),
            _ => None,
        }
        .map(|v| (v.into_iter().map(|x| x as f32).collect::<Vec<f32>>(), 0.0));
        let cap = match style & 0xF00 {
            0x100 => LineCap::Square,
            0x200 => LineCap::Butt,
            _ => LineCap::Round,
        };
        let join = match style & 0xF000 {
            0x1000 => LineJoin::Bevel,
            0x2000 => LineJoin::Miter,
            _ => LineJoin::Round,
        };
        let (cap, join) = if cosmetic || style & 0x10000 == 0 { (LineCap::Round, LineJoin::Round) } else { (cap, join) };
        if let Some(path) = acc.to_path(&Matrix::identity()) {
            let style = StrokeStyle { width: w as f32, cap, join, miter: self.dc.miter as f32, dash };
            self.page.push(Item::Stroke { path, transform: Transform::identity(), brush: Brush::Solid(colour), style, clip: self.dc.clip.clone() });
        }
    }

    /// Draw (or add to the open path) a shape in page points.
    fn shape(&mut self, acc: PathAcc, fill: bool) {
        if let Some(p) = self.path.as_mut() {
            p.append(&acc);
            return;
        }
        if fill {
            self.fill(&acc);
        }
        self.stroke(&acc);
    }

    fn poly_points(&self, pts: &[(f64, f64)], close: bool) -> PathAcc {
        let m = self.xf();
        let mut acc = PathAcc::new();
        for (i, (x, y)) in pts.iter().enumerate() {
            let (a, b) = m.apply(*x, *y);
            if i == 0 {
                acc.move_to(a, b);
            } else {
                acc.line_to(a, b);
            }
        }
        if close {
            acc.close();
        }
        acc
    }

    fn bezier_points(&self, start: Option<(f64, f64)>, pts: &[(f64, f64)]) -> PathAcc {
        let m = self.xf();
        let mut acc = PathAcc::new();
        let mut it = pts.iter();
        let s = match start {
            Some(s) => s,
            None => match it.next() {
                Some(p) => *p,
                None => return acc,
            },
        };
        let (a, b) = m.apply(s.0, s.1);
        acc.move_to(a, b);
        let rest: Vec<&(f64, f64)> = it.collect();
        for c in rest.chunks(3) {
            if c.len() == 3 {
                let (p1, p2, p3) = (m.apply(c[0].0, c[0].1), m.apply(c[1].0, c[1].1), m.apply(c[2].0, c[2].1));
                acc.curve_to(p1.0, p1.1, p2.0, p2.1, p3.0, p3.1);
            }
        }
        acc
    }

    fn rect_path(&self, l: f64, t: f64, r: f64, b: f64) -> PathAcc {
        self.poly_points(&[(l, t), (r, t), (r, b), (l, b)], true)
    }

    fn ellipse_path(&self, l: f64, t: f64, r: f64, b: f64) -> PathAcc {
        let mut acc = PathAcc::new();
        ellipse_segs(&mut acc, &self.xf(), l, t, r, b);
        acc
    }

    /// Points along an elliptical arc (logical units), GDI angle convention.
    fn arc_points(&self, l: f64, t: f64, r: f64, b: f64, s: (f64, f64), e: (f64, f64)) -> Vec<(f64, f64)> {
        let (cx, cy) = ((l + r) / 2.0, (t + b) / 2.0);
        let (rx, ry) = (((r - l) / 2.0).abs().max(1e-6), ((b - t) / 2.0).abs().max(1e-6));
        let a1 = ((s.1 - cy) / ry).atan2((s.0 - cx) / rx);
        let mut a2 = ((e.1 - cy) / ry).atan2((e.0 - cx) / rx);
        // counter-clockwise on a y-down device means decreasing angle
        let ccw = self.dc.arc_ccw ^ (self.xf().det() < 0.0);
        if ccw {
            while a2 >= a1 {
                a2 -= std::f64::consts::TAU;
            }
        } else {
            while a2 <= a1 {
                a2 += std::f64::consts::TAU;
            }
        }
        let n = (((a2 - a1).abs() / std::f64::consts::TAU * 96.0).ceil() as usize).max(2);
        (0..=n).map(|i| {
            let a = a1 + (a2 - a1) * i as f64 / n as f64;
            (cx + rx * a.cos(), cy + ry * a.sin())
        }).collect()
    }

    fn fill_region(&mut self, rgn: &[u8], brush: Option<Brush>) {
        let Some(brush) = brush else { return };
        let acc = self.region_path(rgn);
        if let Some(path) = acc.to_path(&Matrix::identity()) {
            self.page.push(Item::Fill { path, transform: Transform::identity(), brush, even_odd: false, clip: self.dc.clip.clone() });
        }
    }

    /// RGNDATA (device units) → path in page points.
    fn region_path(&self, d: &[u8]) -> PathAcc {
        let n = u32_at(d, 8) as usize;
        let mut acc = PathAcc::new();
        let m = self.dev_to_pt;
        for i in 0..n.min(100_000) {
            let o = 32 + i * 16;
            if o + 16 > d.len() {
                break;
            }
            let (l, t, r, b) = (i32_at(d, o) as f64, i32_at(d, o + 4) as f64, i32_at(d, o + 8) as f64, i32_at(d, o + 12) as f64);
            let pts = [(l, t), (r, t), (r, b), (l, b)];
            for (k, (x, y)) in pts.iter().enumerate() {
                let (a, bb) = m.apply(*x, *y);
                if k == 0 {
                    acc.move_to(a, bb);
                } else {
                    acc.line_to(a, bb);
                }
            }
            acc.close();
        }
        acc
    }

    fn set_clip(&mut self, acc: Option<PathAcc>, mode: u32, even_odd: bool) {
        let path = acc.and_then(|a| a.to_path(&Matrix::identity()));
        match mode {
            1 => {
                // AND
                if let Some(p) = path {
                    let mut v = self.dc.clip.as_ref().map(|c| c.as_ref().clone()).unwrap_or_default();
                    v.push(ClipPath { path: p, even_odd });
                    self.dc.clip = Some(Arc::new(v));
                }
            }
            4 => {
                // DIFF: exclude the region from the current clip
                if let Some(p) = path {
                    let mut pb = tiny_skia::PathBuilder::new();
                    pb.push_rect(tiny_skia::Rect::from_xywh(-10000.0, -10000.0, 30000.0, 30000.0).unwrap());
                    pb.push_path(&p);
                    if let Some(np) = pb.finish() {
                        let mut v = self.dc.clip.as_ref().map(|c| c.as_ref().clone()).unwrap_or_default();
                        v.push(ClipPath { path: np, even_odd: true });
                        self.dc.clip = Some(Arc::new(v));
                    }
                }
            }
            2 => {
                // OR: widen the last clip path
                if let (Some(p), Some(c)) = (path.clone(), self.dc.clip.clone()) {
                    let mut v = c.as_ref().clone();
                    if let Some(last) = v.pop() {
                        let mut pb = tiny_skia::PathBuilder::new();
                        pb.push_path(&last.path);
                        pb.push_path(&p);
                        if let Some(np) = pb.finish() {
                            v.push(ClipPath { path: np, even_odd: false });
                        }
                    }
                    self.dc.clip = Some(Arc::new(v));
                } else {
                    self.dc.clip = path.map(|p| Arc::new(vec![ClipPath { path: p, even_odd }]));
                }
            }
            _ => {
                // COPY / XOR
                self.dc.clip = path.map(|p| Arc::new(vec![ClipPath { path: p, even_odd }]));
            }
        }
    }

    // ---------------------------------------------------------- fonts / text

    fn resolve_font(&mut self, lf: &LogFont) -> (EFont, bool) {
        let bold = lf.weight >= 600;
        let key = (lf.face.to_lowercase(), bold, lf.italic);
        if let Some(f) = self.font_cache.get(&key) {
            return f.clone();
        }
        let fam = lf.face.trim_start_matches('@').to_lowercase();
        let r = if let Some(list) = self.embedded.get(&fam) {
            (EFont::Owned(list[0].clone()), true)
        } else if let Some(f) = fonts::find_installed(&lf.face, bold, lf.italic) {
            (EFont::Owned(f), true)
        } else {
            let name = format!("{}{}{}", lf.face, if bold { " Bold" } else { "" }, if lf.italic { " Italic" } else { "" });
            (EFont::Sys(fonts::system_font(&name)), false)
        };
        let desc = match &r {
            (EFont::Owned(f), _) if self.embedded.contains_key(&fam) => format!("Embedded in the spool file ({})", f.name),
            (EFont::Owned(f), _) => format!("Installed font {}", f.name),
            (EFont::Sys(f), _) => format!("Not installed — substituted with {}", f.file.rsplit(['/', '\\']).next().unwrap_or(&f.file)),
        };
        self.fonts_used.insert(format!("{}{}{}", lf.face, if bold { " Bold" } else { "" }, if lf.italic { " Italic" } else { "" }), desc);
        self.font_cache.insert(key, r.clone());
        r
    }

    #[allow(clippy::too_many_arguments)]
    fn text(&mut self, x: f64, y: f64, chars: &[u16], dx: Option<Vec<f64>>, options: u32, rcl: Option<[f64; 4]>, glyph_index: bool) {
        self.stat("Text runs");
        let lf = self.dc.font.clone();
        let (font, exact) = self.resolve_font(&lf);
        if glyph_index && !exact {
            self.doc.warn(format!("Text in \"{}\" is stored as glyph numbers and the font isn't installed or embedded; that text can't be drawn", lf.face));
            return;
        }
        let upem = font.upem();
        let (asc, desc) = font.asc_desc();
        let em = if lf.height < 0 {
            -lf.height as f64
        } else if lf.height > 0 {
            lf.height as f64 * upem / (asc + desc).max(1.0)
        } else {
            12.0 * self.dev_px_per_mm.1 * 25.4 / 72.0
        };
        let s = em / upem;
        // advances
        let glyphs: Vec<(u16, char)> = chars
            .iter()
            .map(|&c| {
                if glyph_index {
                    (c, '\u{FFFD}')
                } else {
                    let ch = char::from_u32(c as u32).unwrap_or(' ');
                    (font.glyph(ch).unwrap_or(0), ch)
                }
            })
            .collect();
        let advs: Vec<f64> = match &dx {
            Some(v) if v.len() >= glyphs.len() => v[..glyphs.len()].to_vec(),
            _ => glyphs.iter().map(|(g, _)| font.advance(*g) * s).collect(),
        };
        let total: f64 = advs.iter().sum();
        let mut cur = if self.dc.text_align & 1 != 0 { self.dc.cur } else { (x, y) };
        let ta = self.dc.text_align;
        let off_x = match ta & 6 {
            6 => -total / 2.0,
            2 => -total,
            _ => 0.0,
        };
        let off_y = match ta & 24 {
            24 => 0.0,
            8 => -desc * s,
            _ => asc * s,
        };
        let esc = (lf.escapement as f64 / 10.0).to_radians();
        let (ce, se) = (esc.cos(), -esc.sin()); // y-down
        let m = self.xf();
        // y-down logical space: rotate the run around the reference point
        let rot = |dx: f64, dy: f64| (cur.0 + dx * ce - dy * se, cur.1 + dx * se + dy * ce);
        // opaque background
        if options & 2 != 0 {
            if let Some(r) = rcl {
                let acc = self.rect_path(r[0], r[1], r[2], r[3]);
                if let Some(path) = acc.to_path(&Matrix::identity()) {
                    self.page.push(Item::Fill { path, transform: Transform::identity(), brush: Brush::Solid(self.dc.bk_colour), even_odd: false, clip: self.dc.clip.clone() });
                }
            }
        }
        let saved_clip = self.dc.clip.clone();
        if options & 4 != 0 {
            if let Some(r) = rcl {
                let acc = self.rect_path(r[0], r[1], r[2], r[3]);
                self.set_clip(Some(acc), 1, false);
            }
        }
        let mut out = PathAcc::new();
        let mut pen = 0.0;
        let shear = if lf.italic && !exact { 0.2 } else { 0.0 };
        let rev: Option<HashMap<u16, char>> = if glyph_index { if let EFont::Owned(f) = &font { Some(f.reverse_cmap()) } else { None } } else { None };
        for (i, (g, ch)) in glyphs.iter().enumerate() {
            let o = rot(off_x + pen, off_y);
            // glyph space (font units, y up) → logical (y down)
            let gm = Matrix::new(s * ce, s * se, s * (se + shear * ce), s * (shear * se - ce), o.0, o.1).then(&m);
            let mut gacc = PathAcc::new();
            font.outline(*g, &gm, &mut gacc);
            out.append(&gacc);
            let adv = advs[i];
            let a = m.apply(o.0, o.1);
            let e = rot(off_x + pen + adv, off_y);
            let b = m.apply(e.0, e.1);
            let up = m.apply_delta(em * se, -em * ce);
            let c = if glyph_index { rev.as_ref().and_then(|r| r.get(g).copied()).unwrap_or('\u{FFFD}') } else { *ch };
            self.page.text_char(c, a, b, up);
            pen += adv;
        }
        self.page.text_break();
        let colour = self.dc.text_colour;
        if !out.is_empty() {
            if let Some(path) = out.to_path(&Matrix::identity()) {
                self.page.cur_kind = Kind::Text;
                self.page.push(Item::Fill { path, transform: Transform::identity(), brush: Brush::Solid(colour), even_odd: false, clip: self.dc.clip.clone() });
                if lf.weight >= 600 && !exact {
                    if let Some(p2) = out.to_path(&Matrix::identity()) {
                        let w = (em * 0.03 * m.mean_scale()) as f32;
                        self.page.push(Item::Stroke { path: p2, transform: Transform::identity(), brush: Brush::Solid(colour), style: StrokeStyle { width: w, cap: LineCap::Round, join: LineJoin::Round, miter: 4.0, dash: None }, clip: self.dc.clip.clone() });
                    }
                }
                self.page.cur_kind = Kind::Vector;
            }
        }
        for (flag, yoff) in [(lf.underline, desc * s * 0.4), (lf.strikeout, -asc * s * 0.3)] {
            if flag {
                let a = rot(off_x, off_y + yoff);
                let b = rot(off_x + total, off_y + yoff);
                let mut acc = PathAcc::new();
                let (p, q) = (m.apply(a.0, a.1), m.apply(b.0, b.1));
                acc.move_to(p.0, p.1);
                acc.line_to(q.0, q.1);
                if let Some(path) = acc.to_path(&Matrix::identity()) {
                    let w = (em * 0.06 * m.mean_scale()).max(0.3) as f32;
                    self.page.push(Item::Stroke { path, transform: Transform::identity(), brush: Brush::Solid(colour), style: StrokeStyle { width: w, cap: LineCap::Butt, join: LineJoin::Miter, miter: 4.0, dash: None }, clip: self.dc.clip.clone() });
                }
            }
        }
        self.dc.clip = saved_clip;
        if ta & 1 != 0 {
            cur = rot(pen, 0.0);
            self.dc.cur = cur;
        }
    }

    // ---------------------------------------------------------- bitmaps

    #[allow(clippy::too_many_arguments)]
    fn blit(&mut self, rec: &[u8], dest: [f64; 4], src: [f64; 4], off_bmi: usize, cb_bmi: usize, off_bits: usize, cb_bits: usize, rop: u32, alpha: bool, transparent: Option<u32>) {
        if cb_bmi == 0 {
            // pattern blit (no source)
            let colour = match rop {
                0x0000_0042 => Some(Rgb::BLACK),
                0x00FF_0062 => Some(Rgb(1.0, 1.0, 1.0)),
                0x00F0_0021 | 0x005A_0049 => None,
                0x00AA_0029 | 0x0055_0009 => return,
                _ => None,
            };
            let acc = self.rect_path(dest[0], dest[1], dest[0] + dest[2], dest[1] + dest[3]);
            match colour {
                Some(c) => {
                    if let Some(path) = acc.to_path(&Matrix::identity()) {
                        self.page.push(Item::Fill { path, transform: Transform::identity(), brush: Brush::Solid(c), even_odd: false, clip: self.dc.clip.clone() });
                    }
                }
                None => self.fill(&acc),
            }
            return;
        }
        let Some(bmi) = rec.get(off_bmi..off_bmi + cb_bmi) else { return };
        let bits = rec.get(off_bits..(off_bits + cb_bits).min(rec.len()));
        let dec = if alpha { bits.and_then(|b| crate::imgdec::dib_alpha(bmi, b)).map(Ok).unwrap_or_else(|| crate::imgdec::dib(bmi, bits, None)) } else { crate::imgdec::dib(bmi, bits, None) };
        let Ok(mut img) = dec else {
            self.doc.warn(format!("A bitmap could not be decoded ({})", dec.err().unwrap_or_default()));
            return;
        };
        self.stat("Bitmaps");
        // crop to the source rectangle
        let (sx, sy) = (src[0].max(0.0) as u32, src[1].max(0.0) as u32);
        let (sw, sh) = (src[2].abs().max(1.0) as u32, src[3].abs().max(1.0) as u32);
        if (sx, sy) != (0, 0) || sw != img.width || sh != img.height {
            let (sw, sh) = (sw.min(img.width.saturating_sub(sx)).max(1), sh.min(img.height.saturating_sub(sy)).max(1));
            let mut out = vec![0u8; (sw * sh * 4) as usize];
            for y in 0..sh {
                for x in 0..sw {
                    let so = (((sy + y) * img.width + sx + x) * 4) as usize;
                    let d = ((y * sw + x) * 4) as usize;
                    if so + 4 <= img.rgba.len() {
                        out[d..d + 4].copy_from_slice(&img.rgba[so..so + 4]);
                    }
                }
            }
            img.rgba = out;
            img.width = sw;
            img.height = sh;
        }
        // raster operations that act like transparency
        let white_clear = matches!(rop, 0x0088_00C6 | 0x00BB_0226 | 0x00B8_074A);
        let black_clear = matches!(rop, 0x00EE_0086 | 0x0066_0046 | 0x00E2_0746);
        let invert = rop == 0x0033_0008;
        if white_clear || black_clear || invert || transparent.is_some() {
            let tc = transparent.map(|c| [(c & 255) as u8, ((c >> 8) & 255) as u8, ((c >> 16) & 255) as u8]);
            for p in img.rgba.chunks_exact_mut(4) {
                if invert {
                    p[0] = 255 - p[0];
                    p[1] = 255 - p[1];
                    p[2] = 255 - p[2];
                }
                let clear = (white_clear && p[0] > 250 && p[1] > 250 && p[2] > 250) || (black_clear && p[0] < 5 && p[1] < 5 && p[2] < 5) || tc.map(|t| p[0] == t[0] && p[1] == t[1] && p[2] == t[2]).unwrap_or(false);
                if clear {
                    p.copy_from_slice(&[0, 0, 0, 0]);
                }
            }
        }
        let t = Matrix::scale(dest[2] / img.width as f64, dest[3] / img.height as f64).then(&Matrix::translate(dest[0], dest[1])).then(&self.xf());
        let (ri, adj) = RasterImage::fitted(img.width, img.height, img.rgba, &t);
        self.page.cur_kind = Kind::Image;
        self.page.push(Item::Image { image: Arc::new(ri), transform: adj.to_ts(), clip: self.dc.clip.clone() });
        self.page.cur_kind = Kind::Vector;
    }

    // ---------------------------------------------------------- the player

    /// Play one EMF; returns the page.
    pub fn play(&mut self, d: &[u8], base: usize, paper: Option<(f64, f64)>) {
        self.base = base;
        if !is_emf(d) {
            self.doc.warn("Not a valid EMF page");
            return;
        }
        let frame = [i32_at(d, 24) as f64, i32_at(d, 28) as f64, i32_at(d, 32) as f64, i32_at(d, 36) as f64];
        let (devx, devy) = (u32_at(d, 72) as f64, u32_at(d, 76) as f64);
        let (mmx, mmy) = (u32_at(d, 80) as f64, u32_at(d, 84) as f64);
        self.dev_px_per_mm = if devx > 0.0 && mmx > 0.0 && devy > 0.0 && mmy > 0.0 { (devx / mmx, devy / mmy) } else { (11.81, 11.81) };
        let frame_mm = [frame[0] / 100.0, frame[1] / 100.0, frame[2] / 100.0, frame[3] / 100.0];
        let (fw, fh) = ((frame_mm[2] - frame_mm[0]).abs() / 25.4 * 72.0, (frame_mm[3] - frame_mm[1]).abs() / 25.4 * 72.0);
        let (pw, ph) = match paper {
            Some(p) => {
                // match orientation to the frame
                if (fw > fh) != (p.0 > p.1) && fw > 0.0 { (p.1, p.0) } else { p }
            }
            None if fw > 36.0 && fh > 36.0 => (fw, fh),
            None => (mmx / 25.4 * 72.0, mmy / 25.4 * 72.0),
        };
        self.page = Page::new(pw as f32, ph as f32);
        self.page.label = paper_name(pw as f32, ph as f32).unwrap_or("Custom").to_string();
        // device pixels → points; frame origin at its place on the paper (centre the printable area)
        let pt_per_px = (72.0 / 25.4 / self.dev_px_per_mm.0, 72.0 / 25.4 / self.dev_px_per_mm.1);
        let (mx, my) = if paper.is_some() && fw > 0.0 && fw < pw && fh < ph { ((pw - fw) / 2.0, (ph - fh) / 2.0) } else { (0.0, 0.0) };
        let fx_px = frame_mm[0] * self.dev_px_per_mm.0;
        let fy_px = frame_mm[1] * self.dev_px_per_mm.1;
        self.dev_to_pt = Matrix::translate(-fx_px, -fy_px).then(&Matrix::scale(pt_per_px.0, pt_per_px.1)).then(&Matrix::translate(mx, my));
        self.dc = Dc::default();
        self.saved.clear();
        self.objects.clear();
        self.path = None;
        self.path_done = None;
        let mut p = 0usize;
        while p + 8 <= d.len() {
            let t = u32_at(d, p);
            let size = u32_at(d, p + 4) as usize;
            if size < 8 || p + size > d.len() {
                break;
            }
            let rec = &d[p..p + size];
            self.page.cur_src = (base + p) as u32;
            self.record(t, rec, base + p);
            if t == 14 {
                break;
            }
            p += size;
        }
        let page = std::mem::replace(&mut self.page, Page::new(1.0, 1.0));
        self.doc.pages.push(page);
    }

    fn pts32(rec: &[u8], o: usize, n: usize) -> Vec<(f64, f64)> {
        (0..n.min((rec.len().saturating_sub(o)) / 8)).map(|i| (i32_at(rec, o + i * 8) as f64, i32_at(rec, o + i * 8 + 4) as f64)).collect()
    }
    fn pts16(rec: &[u8], o: usize, n: usize) -> Vec<(f64, f64)> {
        (0..n.min((rec.len().saturating_sub(o)) / 4)).map(|i| (i16_at(rec, o + i * 4) as f64, i16_at(rec, o + i * 4 + 2) as f64)).collect()
    }

    fn record(&mut self, t: u32, r: &[u8], off: usize) {
        let name = record_name(t);
        if self.doc.commands.len() < MAX_COMMAND_LINES {
            let extra = match t {
                84 | 83 => {
                    let n = u32_at(r, 44) as usize;
                    let so = u32_at(r, 48) as usize;
                    let opts = u32_at(r, 52);
                    let s: String = if t == 84 && opts & 0x10 == 0 {
                        (0..n.min(60)).filter_map(|i| char::from_u32(u16::from_le_bytes([*r.get(so + i * 2).unwrap_or(&0), *r.get(so + i * 2 + 1).unwrap_or(&0)]) as u32)).collect()
                    } else if t == 83 {
                        r.get(so..so + n.min(60)).map(|b| b.iter().map(|&c| fonts::cp1252(c)).collect()).unwrap_or_default()
                    } else {
                        format!("{n} glyph indices")
                    };
                    format!("  \"{s}\"")
                }
                82 => format!("  \"{}\" height {}", wstr(r, 40, 32), i32_at(r, 12)),
                37 | 40 => format!("  object {:#X}", u32_at(r, 8)),
                _ => String::new(),
            };
            self.doc.cmd(off, format!("{off:08X}  EMR_{name}{extra}"));
        }
        let rect = |o: usize| [i32_at(r, o) as f64, i32_at(r, o + 4) as f64, i32_at(r, o + 8) as f64, i32_at(r, o + 12) as f64];
        match t {
            1 => {}
            // ---- polys (32-bit)
            2 | 3 | 4 | 5 | 6 => {
                let n = u32_at(r, 24) as usize;
                let pts = Self::pts32(r, 28, n);
                self.poly(t, &pts);
            }
            85..=89 => {
                let n = u32_at(r, 24) as usize;
                let pts = Self::pts16(r, 28, n);
                self.poly(t - 83, &pts);
            }
            7 | 8 | 90 | 91 => {
                let wide = t == 7 || t == 8;
                let npolys = u32_at(r, 24) as usize;
                let counts: Vec<usize> = (0..npolys.min(100_000)).map(|i| u32_at(r, 32 + i * 4) as usize).collect();
                let mut o = 32 + npolys * 4;
                let mut acc = PathAcc::new();
                let close = t == 8 || t == 91;
                for c in counts {
                    let pts = if wide { Self::pts32(r, o, c) } else { Self::pts16(r, o, c) };
                    o += c * if wide { 8 } else { 4 };
                    acc.append(&self.poly_points(&pts, close));
                }
                self.shape(acc, close);
            }
            56 | 92 => {
                let n = u32_at(r, 24) as usize;
                let pts = if t == 56 { Self::pts32(r, 28, n) } else { Self::pts16(r, 28, n) };
                let tb = 28 + n * if t == 56 { 8 } else { 4 };
                let m = self.xf();
                let mut acc = PathAcc::new();
                let c = m.apply(self.dc.cur.0, self.dc.cur.1);
                acc.move_to(c.0, c.1);
                let mut i = 0;
                while i < pts.len() {
                    let ty = r.get(tb + i).copied().unwrap_or(2);
                    let p = m.apply(pts[i].0, pts[i].1);
                    match ty & !1 {
                        6 => acc.move_to(p.0, p.1),
                        4 if i + 2 < pts.len() => {
                            let (b, e) = (m.apply(pts[i + 1].0, pts[i + 1].1), m.apply(pts[i + 2].0, pts[i + 2].1));
                            acc.curve_to(p.0, p.1, b.0, b.1, e.0, e.1);
                            i += 2;
                        }
                        _ => acc.line_to(p.0, p.1),
                    }
                    if ty & 1 != 0 {
                        acc.close();
                    }
                    i += 1;
                }
                if let Some(l) = pts.last() {
                    self.dc.cur = *l;
                }
                self.shape(acc, false);
            }
            27 => self.dc.cur = (i32_at(r, 8) as f64, i32_at(r, 12) as f64),
            54 => {
                let to = (i32_at(r, 8) as f64, i32_at(r, 12) as f64);
                let acc = self.poly_points(&[self.dc.cur, to], false);
                self.dc.cur = to;
                self.shape(acc, false);
            }
            15 => {
                let (x, y) = (i32_at(r, 8) as f64, i32_at(r, 12) as f64);
                let acc = self.rect_path(x, y, x + 1.0, y + 1.0);
                if let Some(path) = acc.to_path(&Matrix::identity()) {
                    self.page.push(Item::Fill { path, transform: Transform::identity(), brush: Brush::Solid(colorref(u32_at(r, 16))), even_odd: false, clip: self.dc.clip.clone() });
                }
            }
            43 => {
                let b = rect(8);
                let acc = self.rect_path(b[0], b[1], b[2], b[3]);
                self.shape(acc, true);
            }
            42 => {
                let b = rect(8);
                let acc = self.ellipse_path(b[0], b[1], b[2], b[3]);
                self.shape(acc, true);
            }
            44 => {
                let b = rect(8);
                let (cw, ch) = (i32_at(r, 24) as f64 / 2.0, i32_at(r, 28) as f64 / 2.0);
                let m = self.xf();
                let mut acc = PathAcc::new();
                let (l, t2, rr, bb) = (b[0], b[1], b[2], b[3]);
                let k = 0.5523;
                let p = |x: f64, y: f64| m.apply(x, y);
                let s = p(l + cw, t2);
                acc.move_to(s.0, s.1);
                let e = p(rr - cw, t2);
                acc.line_to(e.0, e.1);
                let (c1, c2, e2) = (p(rr - cw + cw * k, t2), p(rr, t2 + ch - ch * k), p(rr, t2 + ch));
                acc.curve_to(c1.0, c1.1, c2.0, c2.1, e2.0, e2.1);
                let e3 = p(rr, bb - ch);
                acc.line_to(e3.0, e3.1);
                let (c1, c2, e4) = (p(rr, bb - ch + ch * k), p(rr - cw + cw * k, bb), p(rr - cw, bb));
                acc.curve_to(c1.0, c1.1, c2.0, c2.1, e4.0, e4.1);
                let e5 = p(l + cw, bb);
                acc.line_to(e5.0, e5.1);
                let (c1, c2, e6) = (p(l + cw - cw * k, bb), p(l, bb - ch + ch * k), p(l, bb - ch));
                acc.curve_to(c1.0, c1.1, c2.0, c2.1, e6.0, e6.1);
                let e7 = p(l, t2 + ch);
                acc.line_to(e7.0, e7.1);
                let (c1, c2, e8) = (p(l, t2 + ch - ch * k), p(l + cw - cw * k, t2), s);
                acc.curve_to(c1.0, c1.1, c2.0, c2.1, e8.0, e8.1);
                acc.close();
                self.shape(acc, true);
            }
            45 | 46 | 47 | 55 => {
                let b = rect(8);
                let s = (i32_at(r, 24) as f64, i32_at(r, 28) as f64);
                let e = (i32_at(r, 32) as f64, i32_at(r, 36) as f64);
                let mut pts = self.arc_points(b[0], b[1], b[2], b[3], s, e);
                match t {
                    46 => {
                        let acc = self.poly_points(&pts, true);
                        self.shape(acc, true);
                    }
                    47 => {
                        let c = ((b[0] + b[2]) / 2.0, (b[1] + b[3]) / 2.0);
                        pts.insert(0, c);
                        let acc = self.poly_points(&pts, true);
                        self.shape(acc, true);
                    }
                    55 => {
                        pts.insert(0, self.dc.cur);
                        if let Some(l) = pts.last() {
                            self.dc.cur = *l;
                        }
                        let acc = self.poly_points(&pts, false);
                        self.shape(acc, false);
                    }
                    _ => {
                        let acc = self.poly_points(&pts, false);
                        self.shape(acc, false);
                    }
                }
            }
            41 => {
                // AngleArc
                let (cx, cy) = (i32_at(r, 8) as f64, i32_at(r, 12) as f64);
                let rad = u32_at(r, 16) as f64;
                let (sa, sw) = (f32_at(r, 20).to_radians(), f32_at(r, 24).to_radians());
                let n = ((sw.abs() / std::f64::consts::TAU * 96.0).ceil() as usize).max(2);
                let mut pts = vec![self.dc.cur];
                for i in 0..=n {
                    let a = sa + sw * i as f64 / n as f64;
                    pts.push((cx + rad * a.cos(), cy - rad * a.sin()));
                }
                self.dc.cur = *pts.last().unwrap();
                let acc = self.poly_points(&pts, false);
                self.shape(acc, false);
            }
            // ---- state
            9 => self.dc.win_ext = (i32_at(r, 8) as f64, i32_at(r, 12) as f64),
            10 => self.dc.win_org = (i32_at(r, 8) as f64, i32_at(r, 12) as f64),
            11 => self.dc.vp_ext = (i32_at(r, 8) as f64, i32_at(r, 12) as f64),
            12 => self.dc.vp_org = (i32_at(r, 8) as f64, i32_at(r, 12) as f64),
            31 | 32 => {
                let (xn, xd, yn, yd) = (i32_at(r, 8) as f64, i32_at(r, 12) as f64, i32_at(r, 16) as f64, i32_at(r, 20) as f64);
                let e = if t == 31 { &mut self.dc.vp_ext } else { &mut self.dc.win_ext };
                if xd != 0.0 && yd != 0.0 {
                    *e = (e.0 * xn / xd, e.1 * yn / yd);
                }
            }
            17 => self.dc.map_mode = i32_at(r, 8),
            18 => self.dc.bk_mode = i32_at(r, 8),
            19 => self.dc.poly_fill = i32_at(r, 8),
            22 => self.dc.text_align = u32_at(r, 8),
            24 => self.dc.text_colour = colorref(u32_at(r, 8)),
            25 => self.dc.bk_colour = colorref(u32_at(r, 8)),
            57 => self.dc.arc_ccw = i32_at(r, 8) != 2,
            58 => self.dc.miter = (u32_at(r, 8) as f64).max(1.0),
            33 => {
                if self.saved.len() < 1000 {
                    self.saved.push(self.dc.clone());
                }
            }
            34 => {
                let rel = i32_at(r, 8);
                let n = if rel < 0 { (-rel) as usize } else { self.saved.len().saturating_sub(rel.max(1) as usize - 1) };
                for _ in 0..n.saturating_sub(1) {
                    self.saved.pop();
                }
                if let Some(s) = self.saved.pop() {
                    self.dc = s;
                }
            }
            35 => self.dc.world = Matrix::new(f32_at(r, 8), f32_at(r, 12), f32_at(r, 16), f32_at(r, 20), f32_at(r, 24), f32_at(r, 28)),
            36 => {
                let x = Matrix::new(f32_at(r, 8), f32_at(r, 12), f32_at(r, 16), f32_at(r, 20), f32_at(r, 24), f32_at(r, 28));
                match u32_at(r, 32) {
                    1 => self.dc.world = Matrix::identity(),
                    2 => self.dc.world = x.then(&self.dc.world),
                    3 => self.dc.world = self.dc.world.then(&x),
                    4 => self.dc.world = x,
                    _ => {}
                }
            }
            // ---- objects
            38 => {
                let style = u32_at(r, 12);
                let w = i32_at(r, 16) as f64;
                self.objects.insert(u32_at(r, 8), Obj::Pen { null: style & 0xF == 5, width: w, colour: colorref(u32_at(r, 24)), style, cosmetic: w <= 1.0 && style & 0x10000 == 0 });
            }
            95 => {
                let style = u32_at(r, 28);
                let w = u32_at(r, 32) as f64;
                let geometric = style & 0x10000 != 0;
                self.objects.insert(u32_at(r, 8), Obj::Pen { null: style & 0xF == 5 || u32_at(r, 36) == 1, width: w, colour: colorref(u32_at(r, 40)), style, cosmetic: !geometric });
            }
            39 => {
                let style = u32_at(r, 12);
                self.objects.insert(u32_at(r, 8), Obj::Brush { null: style == 1, colour: colorref(u32_at(r, 16)), hatched: style == 2, pattern: None });
            }
            93 | 94 => {
                let (ob, cb, obits, cbits) = (u32_at(r, 16) as usize, u32_at(r, 20) as usize, u32_at(r, 24) as usize, u32_at(r, 28) as usize);
                let img = r.get(ob..ob + cb).and_then(|bmi| crate::imgdec::dib(bmi, r.get(obits..obits + cbits), None).ok());
                let obj = match img {
                    Some(mut i) => {
                        if t == 93 {
                            // mono brush: 1 = background (white), 0 = text colour
                            let tc = self.dc.text_colour;
                            for p in i.rgba.chunks_exact_mut(4) {
                                if p[0] < 128 {
                                    p.copy_from_slice(&[(tc.0 * 255.0) as u8, (tc.1 * 255.0) as u8, (tc.2 * 255.0) as u8, 255]);
                                }
                            }
                        }
                        Obj::Brush { null: false, colour: Rgb::gray(0.5), hatched: false, pattern: Some(Arc::new(RasterImage::new(i.width, i.height, i.rgba))) }
                    }
                    None => Obj::Brush { null: false, colour: Rgb::gray(0.5), hatched: false, pattern: None },
                };
                self.objects.insert(u32_at(r, 8), obj);
            }
            82 => {
                let lf = LogFont {
                    height: i32_at(r, 12),
                    width: i32_at(r, 16),
                    escapement: i32_at(r, 20),
                    weight: i32_at(r, 28),
                    italic: r.get(32).copied().unwrap_or(0) != 0,
                    underline: r.get(33).copied().unwrap_or(0) != 0,
                    strikeout: r.get(34).copied().unwrap_or(0) != 0,
                    face: wstr(r, 40, 32),
                };
                let _ = lf.width;
                self.objects.insert(u32_at(r, 8), Obj::Font(lf));
            }
            49 => {
                self.objects.insert(u32_at(r, 8), Obj::Other);
            }
            37 => {
                let ih = u32_at(r, 8);
                let o = if ih & 0x8000_0000 != 0 { stock(ih) } else { self.objects.get(&ih).cloned().unwrap_or(Obj::Other) };
                match o {
                    Obj::Pen { .. } => self.dc.pen = o,
                    Obj::Brush { .. } => self.dc.brush = o,
                    Obj::Font(f) => self.dc.font = f,
                    Obj::Other => {}
                }
            }
            40 => {
                self.objects.remove(&u32_at(r, 8));
            }
            // ---- paths
            59 => self.path = Some(PathAcc::new()),
            60 => self.path_done = self.path.take(),
            61 => {
                if let Some(p) = self.path.as_mut() {
                    p.close();
                }
            }
            68 => {
                self.path = None;
                self.path_done = None;
            }
            62 | 63 | 64 => {
                if let Some(p) = self.path_done.take().or_else(|| self.path.take()) {
                    if t != 64 {
                        self.fill(&p);
                    }
                    if t != 62 {
                        self.stroke(&p);
                    }
                }
            }
            65 | 66 => {}
            67 => {
                if let Some(p) = self.path_done.take() {
                    let eo = self.dc.poly_fill == 1;
                    self.set_clip(Some(p), u32_at(r, 8), eo);
                }
            }
            // ---- clipping
            30 => {
                let b = rect(8);
                let acc = self.rect_path(b[0], b[1], b[2], b[3]);
                self.set_clip(Some(acc), 1, false);
            }
            29 => {
                let b = rect(8);
                let acc = self.rect_path(b[0], b[1], b[2], b[3]);
                self.set_clip(Some(acc), 4, false);
            }
            75 => {
                let cb = u32_at(r, 8) as usize;
                let mode = u32_at(r, 12);
                if cb == 0 {
                    if mode == 5 {
                        self.dc.clip = None;
                    }
                } else if let Some(rgn) = r.get(16..16 + cb) {
                    let acc = self.region_path(rgn);
                    self.set_clip(Some(acc), mode, false);
                }
            }
            26 | 28 => {}
            71 => {
                let cb = u32_at(r, 24) as usize;
                let ih = u32_at(r, 28);
                let saved = self.dc.brush.clone();
                self.dc.brush = if ih & 0x8000_0000 != 0 { stock(ih) } else { self.objects.get(&ih).cloned().unwrap_or(saved.clone()) };
                let b = self.brush_paint();
                if let Some(rgn) = r.get(32..32 + cb) {
                    self.fill_region(rgn, b);
                }
                self.dc.brush = saved;
            }
            74 => {
                let cb = u32_at(r, 24) as usize;
                let b = self.brush_paint();
                if let Some(rgn) = r.get(28..28 + cb) {
                    self.fill_region(rgn, b);
                }
            }
            72 | 73 => {}
            // ---- text
            83 | 84 => {
                let gm = u32_at(r, 24);
                let (exs, eys) = (f32_at(r, 28), f32_at(r, 32));
                let (x, y) = (i32_at(r, 36) as f64, i32_at(r, 40) as f64);
                let n = u32_at(r, 44) as usize;
                let so = u32_at(r, 48) as usize;
                let opts = u32_at(r, 52);
                let rc = [i32_at(r, 56) as f64, i32_at(r, 60) as f64, i32_at(r, 64) as f64, i32_at(r, 68) as f64];
                let dxo = u32_at(r, 72) as usize;
                let chars: Vec<u16> = if t == 84 {
                    (0..n.min(100_000)).map(|i| u16::from_le_bytes([*r.get(so + i * 2).unwrap_or(&0), *r.get(so + i * 2 + 1).unwrap_or(&0)])).collect()
                } else {
                    (0..n.min(100_000)).map(|i| fonts::cp1252(*r.get(so + i).unwrap_or(&0x20)) as u16).collect()
                };
                let pdy = opts & 0x2000 != 0;
                let dx: Option<Vec<f64>> = if dxo > 0 && dxo < r.len() {
                    let k = if pdy { 2 } else { 1 };
                    Some((0..n).map(|i| i32_at(r, dxo + i * 4 * k) as f64).collect())
                } else {
                    None
                };
                let _ = (gm, exs, eys);
                let rcl = if rc[2] > rc[0] && rc[3] > rc[1] { Some(rc) } else { None };
                self.text(x, y, &chars, dx, opts, rcl, opts & 0x10 != 0 && t == 84);
            }
            108 => {
                let (x, y) = (i32_at(r, 8) as f64, i32_at(r, 12) as f64);
                let n = u32_at(r, 16) as usize;
                let opts = u32_at(r, 20);
                let mut o = 36;
                let rc = if opts & 0x100 == 0 {
                    let v = [i32_at(r, 36) as f64, i32_at(r, 40) as f64, i32_at(r, 44) as f64, i32_at(r, 48) as f64];
                    o += 16;
                    Some(v)
                } else {
                    None
                };
                let chars: Vec<u16> = if opts & 0x200 != 0 {
                    (0..n).map(|i| *r.get(o + i).unwrap_or(&32) as u16).collect()
                } else {
                    (0..n).map(|i| u16::from_le_bytes([*r.get(o + i * 2).unwrap_or(&0), *r.get(o + i * 2 + 1).unwrap_or(&0)])).collect()
                };
                self.text(x, y, &chars, None, opts, rc, opts & 0x10 != 0);
            }
            // ---- bitmaps
            76 | 77 => {
                let dest = [i32_at(r, 24) as f64, i32_at(r, 28) as f64, i32_at(r, 32) as f64, i32_at(r, 36) as f64];
                let rop = u32_at(r, 40);
                let (xs, ys) = (i32_at(r, 44) as f64, i32_at(r, 48) as f64);
                let (ob, cb, obits, cbits) = (u32_at(r, 84) as usize, u32_at(r, 88) as usize, u32_at(r, 92) as usize, u32_at(r, 96) as usize);
                let (cxs, cys) = if t == 77 { (i32_at(r, 100) as f64, i32_at(r, 104) as f64) } else { (dest[2], dest[3]) };
                self.blit(r, dest, [xs, ys, cxs, cys], ob, cb, obits, cbits, rop, false, None);
            }
            78 | 79 => {
                let dest = [i32_at(r, 24) as f64, i32_at(r, 28) as f64, i32_at(r, 32) as f64, i32_at(r, 36) as f64];
                let (ob, cb, obits, cbits) = (u32_at(r, 84) as usize, u32_at(r, 88) as usize, u32_at(r, 92) as usize, u32_at(r, 96) as usize);
                self.blit(r, dest, [i32_at(r, 44) as f64, i32_at(r, 48) as f64, dest[2], dest[3]], ob, cb, obits, cbits, 0xCC0020, false, None);
                if t == 79 {
                    self.doc.warn("PlgBlt (parallelogram bitmap) is drawn as a rectangle");
                }
            }
            81 => {
                let (xd, yd) = (i32_at(r, 24) as f64, i32_at(r, 28) as f64);
                let src = [i32_at(r, 32) as f64, i32_at(r, 36) as f64, i32_at(r, 40) as f64, i32_at(r, 44) as f64];
                let (ob, cb, obits, cbits) = (u32_at(r, 48) as usize, u32_at(r, 52) as usize, u32_at(r, 56) as usize, u32_at(r, 60) as usize);
                let rop = u32_at(r, 68);
                let (cxd, cyd) = (i32_at(r, 72) as f64, i32_at(r, 76) as f64);
                // StretchDIBits source y counts from the bottom of bottom-up DIBs
                let h = r.get(ob + 8..ob + 12).map(|b| i32::from_le_bytes([b[0], b[1], b[2], b[3]])).unwrap_or(0);
                let src = if h > 0 { [src[0], h as f64 - src[1] - src[3], src[2], src[3]] } else { src };
                self.blit(r, [xd, yd, cxd, cyd], src, ob, cb, obits, cbits, rop, false, None);
            }
            80 => {
                let (xd, yd) = (i32_at(r, 24) as f64, i32_at(r, 28) as f64);
                let src = [i32_at(r, 32) as f64, i32_at(r, 36) as f64, i32_at(r, 40) as f64, i32_at(r, 44) as f64];
                let (ob, cb, obits, cbits) = (u32_at(r, 48) as usize, u32_at(r, 52) as usize, u32_at(r, 56) as usize, u32_at(r, 60) as usize);
                self.blit(r, [xd, yd, src[2], src[3]], [0.0, 0.0, src[2], src[3]], ob, cb, obits, cbits, 0xCC0020, false, None);
            }
            114 | 116 => {
                let dest = [i32_at(r, 24) as f64, i32_at(r, 28) as f64, i32_at(r, 32) as f64, i32_at(r, 36) as f64];
                let bf = u32_at(r, 40);
                let (ob, cb, obits, cbits) = (u32_at(r, 84) as usize, u32_at(r, 88) as usize, u32_at(r, 92) as usize, u32_at(r, 96) as usize);
                let src = [i32_at(r, 44) as f64, i32_at(r, 48) as f64, i32_at(r, 100) as f64, i32_at(r, 104) as f64];
                if t == 114 {
                    self.blit(r, dest, src, ob, cb, obits, cbits, 0xCC0020, (bf >> 24) & 1 == 1, None);
                } else {
                    self.blit(r, dest, src, ob, cb, obits, cbits, 0xCC0020, false, Some(bf));
                }
            }
            118 => {
                let nv = u32_at(r, 24) as usize;
                let nt = u32_at(r, 28) as usize;
                let mode = u32_at(r, 32);
                let vx = |i: usize| {
                    let o = 36 + i * 16;
                    let c = |k: usize| (u16::from_le_bytes([*r.get(o + 8 + k * 2).unwrap_or(&0), *r.get(o + 9 + k * 2).unwrap_or(&0)]) >> 8) as u8;
                    ((i32_at(r, o) as f64, i32_at(r, o + 4) as f64), [c(0), c(1), c(2)])
                };
                let mo = 36 + nv * 16;
                for k in 0..nt.min(10_000) {
                    if mode == 2 {
                        let (a, b, c) = (u32_at(r, mo + k * 12) as usize, u32_at(r, mo + k * 12 + 4) as usize, u32_at(r, mo + k * 12 + 8) as usize);
                        if a >= nv || b >= nv || c >= nv {
                            continue;
                        }
                        let (pa, ca) = vx(a);
                        let (pb, cb2) = vx(b);
                        let (pc, cc) = vx(c);
                        let avg = Rgb((ca[0] as f32 + cb2[0] as f32 + cc[0] as f32) / 765.0, (ca[1] as f32 + cb2[1] as f32 + cc[1] as f32) / 765.0, (ca[2] as f32 + cb2[2] as f32 + cc[2] as f32) / 765.0);
                        let acc = self.poly_points(&[pa, pb, pc], true);
                        if let Some(path) = acc.to_path(&Matrix::identity()) {
                            self.page.push(Item::Fill { path, transform: Transform::identity(), brush: Brush::Solid(avg), even_odd: false, clip: self.dc.clip.clone() });
                        }
                    } else {
                        let (a, b) = (u32_at(r, mo + k * 8) as usize, u32_at(r, mo + k * 8 + 4) as usize);
                        if a >= nv || b >= nv {
                            continue;
                        }
                        let (p1, c1) = vx(a);
                        let (p2, c2) = vx(b);
                        let n = 256u32;
                        let horizontal = mode == 0;
                        let mut px = Vec::with_capacity(n as usize * 4);
                        for i in 0..n {
                            let f = i as f32 / (n - 1) as f32;
                            for ch in 0..3 {
                                px.push((c1[ch] as f32 + (c2[ch] as f32 - c1[ch] as f32) * f) as u8);
                            }
                            px.push(255);
                        }
                        let (w, h) = if horizontal { (n, 1) } else { (1, n) };
                        let (x0, y0, x1, y1) = (p1.0.min(p2.0), p1.1.min(p2.1), p1.0.max(p2.0), p1.1.max(p2.1));
                        let tr = Matrix::scale((x1 - x0) / w as f64, (y1 - y0) / h as f64).then(&Matrix::translate(x0, y0)).then(&self.xf());
                        self.page.push(Item::Image { image: Arc::new(RasterImage::new(w, h, px)), transform: tr.to_ts(), clip: self.dc.clip.clone() });
                    }
                }
            }
            70 => {
                // GDI comment: EMF+ or public comments
                let cb = u32_at(r, 8) as usize;
                if r.get(12..16) == Some(b"EMF+") {
                    let data = &r[16..(12 + cb).min(r.len())];
                    if u16::from_le_bytes([data.first().copied().unwrap_or(0), data.get(1).copied().unwrap_or(0)]) == 0x4001 {
                        let flags = u16::from_le_bytes([data.get(2).copied().unwrap_or(0), data.get(3).copied().unwrap_or(0)]);
                        if flags & 1 == 0 {
                            self.emf_plus_only = true;
                        }
                    }
                    self.stat("EMF+ records (GDI+; the EMF copy is drawn)");
                }
            }
            14 => {}
            _ => {
                self.stat(&format!("Ignored: EMR_{}", record_name(t)));
            }
        }
    }

    fn poly(&mut self, t: u32, pts: &[(f64, f64)]) {
        match t {
            2 => {
                let acc = self.bezier_points(None, pts);
                self.shape(acc, false);
            }
            3 => {
                let acc = self.poly_points(pts, true);
                self.shape(acc, true);
            }
            4 => {
                let acc = self.poly_points(pts, false);
                self.shape(acc, false);
            }
            5 => {
                let acc = self.bezier_points(Some(self.dc.cur), pts);
                if let Some(l) = pts.last() {
                    self.dc.cur = *l;
                }
                self.shape(acc, false);
            }
            6 => {
                let mut v = vec![self.dc.cur];
                v.extend_from_slice(pts);
                if let Some(l) = pts.last() {
                    self.dc.cur = *l;
                }
                let acc = self.poly_points(&v, false);
                self.shape(acc, false);
            }
            _ => {}
        }
    }

    fn add_embedded(&mut self, data: &[u8]) {
        // find a TrueType/OpenType header inside the record
        let mut found = false;
        for i in 0..data.len().saturating_sub(12).min(4096) {
            let tag = &data[i..i + 4];
            if tag == [0, 1, 0, 0] || tag == b"OTTO" || tag == b"true" || tag == b"ttcf" {
                if let Some(f) = OwnedFont::new(data[i..].to_vec(), 0) {
                    let fam = f.face().and_then(|fc| {
                        let n = |id: u16| fc.names().into_iter().find(|n| n.name_id == id && n.is_unicode()).and_then(|n| n.to_string());
                        n(ttf_parser::name_id::TYPOGRAPHIC_FAMILY).or_else(|| n(ttf_parser::name_id::FAMILY))
                    });
                    if let Some(fam) = fam {
                        self.embedded.entry(fam.to_lowercase()).or_default().push(f);
                        found = true;
                    }
                    break;
                }
            }
        }
        if found {
            self.stat("Embedded fonts");
            self.font_cache.clear();
        } else {
            self.stat("Embedded font records (unreadable)");
        }
    }

    pub fn finish(self) {
        let mut s = InfoSection::new("EMF content");
        for (k, v) in &self.stats {
            s.add(k.clone(), v.to_string());
        }
        if self.emf_plus_only {
            self.doc.warn("Some pages use EMF+ (GDI+) records only; those parts are not drawn");
        }
        self.doc.info.push(s);
        if !self.fonts_used.is_empty() {
            let mut f = InfoSection::new("Fonts (EMF)");
            for (k, v) in &self.fonts_used {
                f.add(k.clone(), v.clone());
            }
            self.doc.info.push(f);
        }
    }
}

fn spool_record_name(t: u32) -> &'static str {
    match t {
        1 => "METAFILE",
        2 => "ENGINE_FONT",
        3 => "DEVMODE",
        4 => "TYPE1_FONT",
        5 => "PRESTARTPAGE",
        6 => "DESIGNVECTOR",
        7 => "SUBSET_FONT",
        8 => "DELTA_FONT",
        9 => "FORM_METAFILE",
        10 => "BW_METAFILE",
        11 => "BW_FORM_METAFILE",
        12 => "METAFILE_DATA",
        13 => "METAFILE_EXT",
        14 => "BW_METAFILE_EXT",
        15 => "ENGINE_FONT_EXT",
        16 => "TYPE1_FONT_EXT",
        17 => "DESIGNVECTOR_EXT",
        18 => "SUBSET_FONT_EXT",
        19 => "DELTA_FONT_EXT",
        20 => "PS_JOB_DATA",
        21 => "EMBED_FONT_EXT",
        _ => "UNKNOWN",
    }
}

/// A stand-alone EMF file: one page.
pub fn run_emf(doc: &mut Document, data: &[u8]) {
    let mut p = Player::new(doc);
    p.play(data, 0, None);
    let desc_n = u32_at(data, 64) as usize;
    let desc_o = u32_at(data, 68) as usize;
    let desc = if desc_n > 0 { wstr(data, desc_o, desc_n).replace('\0', " — ") } else { String::new() };
    let mut s = InfoSection::new("EMF file");
    s.add("Version", format!("{:#X}", u32_at(data, 44)));
    s.add("Records", u32_at(data, 52).to_string());
    if !desc.trim().is_empty() {
        s.add("Description", desc.trim().trim_end_matches(" —").to_string());
    }
    s.add("Reference device", format!("{} × {} px, {} × {} mm", u32_at(data, 72), u32_at(data, 76), u32_at(data, 80), u32_at(data, 84)));
    p.doc.info.push(s);
    p.finish();
}

/// A Windows EMF spool file.
pub fn run_spool(doc: &mut Document, data: &[u8]) {
    let cb = u32_at(data, 4) as usize;
    let doc_off = u32_at(data, 8) as usize;
    let out_off = u32_at(data, 12) as usize;
    let mut info = InfoSection::new("EMF spool file");
    info.add("Spool format version", format!("{:#X}", u32_at(data, 0)));
    if doc_off > 0 {
        info.add("Document name", wstr(data, doc_off, 512));
    }
    if out_off > 0 {
        let o = wstr(data, out_off, 512);
        if !o.is_empty() {
            info.add("Output file", o);
        }
    }
    let mut paper: Option<(f64, f64)> = None;
    let mut p = Player::new(doc);
    let mut pos = cb;
    let mut counts: BTreeMap<&'static str, usize> = BTreeMap::new();
    while pos + 8 <= data.len() {
        let t = u32_at(data, pos);
        let size = u32_at(data, pos + 4) as usize;
        let body = pos + 8;
        if t == 0 || t > 64 || body + size > data.len() {
            break;
        }
        let name = spool_record_name(t);
        *counts.entry(name).or_insert(0) += 1;
        p.doc.cmd(pos, format!("{pos:08X}  EMRI_{name}  ({} bytes)", size));
        let rec = &data[body..body + size];
        match t {
            1 | 9 | 10 | 11 | 12 => {
                p.play(rec, body, paper);
            }
            3 => {
                if let Some(dm) = crate::shd::devmode(rec, 0) {
                    if dm.has(0x2) {
                        if let Some(pp) = crate::analyze::devmode_paper_pt(dm.paper_size).map(|(a, b)| (a as f64, b as f64)) {
                            let landscape = dm.has(0x1) && dm.orientation == 2;
                            paper = Some(if landscape { (pp.1, pp.0) } else { pp });
                        }
                    } else if dm.paper_width > 0 && dm.paper_length > 0 {
                        paper = Some((dm.paper_width as f64 / 254.0 * 72.0, dm.paper_length as f64 / 254.0 * 72.0));
                    }
                    let mut sec = dm.section();
                    sec.title = "Driver settings (DEVMODE in the spool file)".into();
                    p.doc.info.push(sec);
                }
            }
            2 | 4 | 7 | 8 | 15 | 16 | 18 | 19 | 21 => p.add_embedded(rec),
            _ => {}
        }
        pos = body + size;
    }
    for (k, v) in counts {
        info.add(format!("EMRI_{k} records"), v.to_string());
    }
    p.doc.info.push(info);
    p.finish();
}

fn record_name(t: u32) -> &'static str {
    const N: &[&str] = &[
        "", "HEADER", "POLYBEZIER", "POLYGON", "POLYLINE", "POLYBEZIERTO", "POLYLINETO", "POLYPOLYLINE", "POLYPOLYGON", "SETWINDOWEXTEX", "SETWINDOWORGEX", "SETVIEWPORTEXTEX", "SETVIEWPORTORGEX", "SETBRUSHORGEX", "EOF", "SETPIXELV",
        "SETMAPPERFLAGS", "SETMAPMODE", "SETBKMODE", "SETPOLYFILLMODE", "SETROP2", "SETSTRETCHBLTMODE", "SETTEXTALIGN", "SETCOLORADJUSTMENT", "SETTEXTCOLOR", "SETBKCOLOR", "OFFSETCLIPRGN", "MOVETOEX", "SETMETARGN", "EXCLUDECLIPRECT", "INTERSECTCLIPRECT",
        "SCALEVIEWPORTEXTEX", "SCALEWINDOWEXTEX", "SAVEDC", "RESTOREDC", "SETWORLDTRANSFORM", "MODIFYWORLDTRANSFORM", "SELECTOBJECT", "CREATEPEN", "CREATEBRUSHINDIRECT", "DELETEOBJECT", "ANGLEARC", "ELLIPSE", "RECTANGLE", "ROUNDRECT", "ARC", "CHORD", "PIE",
        "SELECTPALETTE", "CREATEPALETTE", "SETPALETTEENTRIES", "RESIZEPALETTE", "REALIZEPALETTE", "EXTFLOODFILL", "LINETO", "ARCTO", "POLYDRAW", "SETARCDIRECTION", "SETMITERLIMIT", "BEGINPATH", "ENDPATH", "CLOSEFIGURE", "FILLPATH", "STROKEANDFILLPATH",
        "STROKEPATH", "FLATTENPATH", "WIDENPATH", "SELECTCLIPPATH", "ABORTPATH", "UNUSED69", "GDICOMMENT", "FILLRGN", "FRAMERGN", "INVERTRGN", "PAINTRGN", "EXTSELECTCLIPRGN", "BITBLT", "STRETCHBLT", "MASKBLT", "PLGBLT", "SETDIBITSTODEVICE", "STRETCHDIBITS",
        "EXTCREATEFONTINDIRECTW", "EXTTEXTOUTA", "EXTTEXTOUTW", "POLYBEZIER16", "POLYGON16", "POLYLINE16", "POLYBEZIERTO16", "POLYLINETO16", "POLYPOLYLINE16", "POLYPOLYGON16", "POLYDRAW16", "CREATEMONOBRUSH", "CREATEDIBPATTERNBRUSHPT", "EXTCREATEPEN",
        "POLYTEXTOUTA", "POLYTEXTOUTW", "SETICMMODE", "CREATECOLORSPACE", "SETCOLORSPACE", "DELETECOLORSPACE", "GLSRECORD", "GLSBOUNDEDRECORD", "PIXELFORMAT", "DRAWESCAPE", "EXTESCAPE", "UNUSED107", "SMALLTEXTOUT", "FORCEUFIMAPPING", "NAMEDESCAPE",
        "COLORCORRECTPALETTE", "SETICMPROFILEA", "SETICMPROFILEW", "ALPHABLEND", "SETLAYOUT", "TRANSPARENTBLT", "UNUSED117", "GRADIENTFILL", "SETLINKEDUFIS", "SETTEXTJUSTIFICATION", "COLORMATCHTOTARGETW", "CREATECOLORSPACEW",
    ];
    N.get(t as usize).copied().unwrap_or("UNKNOWN")
}
