//! PCL 5 (PCL 5e / 5c) interpreter: text with resident and downloaded fonts,
//! cursor positioning, raster graphics, rectangles and patterns, macros,
//! colour palettes, and embedded HP-GL/2.

use crate::doc::*;
use crate::fonts::{self, SfntInfo, SysFont};
use crate::geom::*;
use crate::hpgl::{Frame, Hpgl};
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use tiny_skia::Transform;

#[derive(Clone)]
struct FontSel {
    symset: u32,
    spacing: i32,
    pitch: f64,
    height: f64,
    style: i32,
    weight: i32,
    typeface: i32,
    id: Option<u32>,
}

impl Default for FontSel {
    fn default() -> Self {
        // Courier 12 pt, 10 pitch, Roman-8
        FontSel { symset: 277, spacing: 0, pitch: 10.0, height: 12.0, style: 0, weight: 0, typeface: 4099, id: None }
    }
}

struct BitmapGlyph {
    left: i32,
    top: i32,
    w: u32,
    h: u32,
    delta_qd: f64,
    bits: Vec<u8>,
}

enum SoftKind {
    Bitmap { res: f64, chars: HashMap<u32, BitmapGlyph> },
    TrueType { sfnt: SfntInfo, chars: HashMap<u32, u16>, glyphs: HashMap<u16, Vec<u8>> },
    Unsupported,
}

struct SoftFont {
    format: u8,
    spacing: i32,
    pitch_qd: f64,
    height_qd: f64,
    symset: u32,
    typeface: i32,
    name: String,
    kind: SoftKind,
    ident: Option<Arc<fonts::Identified>>,
    ident_tried: usize,
    used: bool,
}

#[derive(Clone, Copy, PartialEq)]
enum PixMode {
    Mono,
    SimpleRgb,
    SimpleCmy,
    Cid { cmy: bool, mode: u8, bits_index: u8, bits: [u8; 3] },
}

struct Raster {
    active: bool,
    res: f64,
    width: u32,
    height: u32,
    compression: i32,
    x: f64, // decipoints
    y: f64,
    rows: Vec<u8>, // RGBA
    row_count: u32,
    row_width: u32,
    seeds: Vec<Vec<u8>>,
    planes: Vec<Vec<u8>>,
    left_at_cursor: bool,
    dest: (Option<f64>, Option<f64>),
}

pub struct Pcl5<'d> {
    doc: &'d mut Document,
    page: Page,
    marked: bool,
    paper_code: i32,
    paper: (f64, f64), // portrait points
    orientation: i32,
    units: f64,
    x: f64,
    y: f64,
    left_margin: f64,
    right_margin: f64,
    top_margin: f64,
    text_length: f64,
    vmi: f64,
    hmi: f64,
    line_term: i32,
    print_dir: i32,
    wrap: bool,
    cursor_stack: Vec<(f64, f64)>,
    primary: FontSel,
    secondary: FontSel,
    use_secondary: bool,
    soft: BTreeMap<u32, SoftFont>,
    font_id: u32,
    char_code: u32,
    underline: Option<f64>,
    raster: Raster,
    rect_w: f64,
    rect_h: f64,
    pattern_id: i32,
    palette: Vec<Rgb>,
    fg: Rgb,
    comps: [f64; 3],
    pix: PixMode,
    source_transparent: bool,
    current_pattern: i32,
    macros: HashMap<u32, Vec<u8>>,
    macro_id: u32,
    overlay: Option<u32>,
    depth: u32,
    hpgl: Hpgl,
    frame_size: (Option<f64>, Option<f64>),
    frame_anchor: Option<(f64, f64)>,
    sys_fonts: HashMap<String, Arc<SysFont>>,
    glyph_cache: HashMap<(u32, u32, u32), Arc<RasterImage>>,
    last_adv: f64,
    text_log: Option<(usize, String)>,
    // info
    stats: BTreeMap<String, usize>,
    page_settings: Vec<(String, String)>,
    job: BTreeMap<String, String>,
    fonts_used: BTreeMap<String, usize>,
    images: usize,
    rects: usize,
    base: usize,
}

fn paper_points(code: i32) -> Option<(f64, f64, &'static str)> {
    Some(match code {
        1 => (522.0, 756.0, "Executive"),
        2 => (612.0, 792.0, "Letter"),
        3 => (612.0, 1008.0, "Legal"),
        6 => (792.0, 1224.0, "Ledger"),
        25 => (419.5, 595.3, "A5"),
        26 => (595.3, 841.9, "A4"),
        27 => (841.9, 1190.6, "A3"),
        45 => (515.9, 728.5, "JIS B5"),
        46 => (728.5, 1031.8, "JIS B4"),
        71 => (283.5, 419.5, "Hagaki"),
        72 => (419.5, 566.9, "Oufuku-Hagaki"),
        80 => (279.0, 540.0, "Monarch envelope"),
        81 => (297.0, 684.0, "COM-10 envelope"),
        90 => (311.8, 623.6, "DL envelope"),
        91 => (459.2, 649.1, "C5 envelope"),
        100 => (498.9, 708.7, "B5 envelope"),
        _ => return None,
    })
}

fn symset_code(num: f64, letter: u8) -> u32 {
    (num.max(0.0) as u32) * 32 + (letter.to_ascii_uppercase().saturating_sub(64)) as u32
}

pub fn symset_name(s: u32) -> String {
    let n = s / 32;
    let l = (s % 32 + 64) as u8 as char;
    let known = match s {
        277 => " (Roman-8)",
        341 => " (PC-8)",
        14 => " (ISO 8859-1 Latin 1)",
        629 => " (Windows Latin 1)",
        21 => " (ASCII)",
        621 => " (Symbol)",
        18540 => " (Wingdings)",
        590 => " (Unicode)",
        _ => "",
    };
    format!("{n}{l}{known}")
}

fn decode_char(symset: u32, code: u32) -> char {
    if code > 255 {
        return char::from_u32(code).unwrap_or('\u{FFFD}');
    }
    let b = code as u8;
    match symset {
        341 | 853 => fonts::pc8(b),
        14 | 590 => b as char,
        629 | 18 | 309 => fonts::cp1252(b),
        621 => fonts::symbol_to_unicode(b).unwrap_or(b as char),
        18540 => char::from_u32(0xF000 + code).unwrap_or(' '),
        _ => fonts::roman8(b),
    }
}

impl<'d> Pcl5<'d> {
    pub fn new(doc: &'d mut Document) -> Self {
        let frame = Frame { x: 0.0, y: 0.0, w: 612.0, h: 792.0 };
        let mut s = Pcl5 {
            doc,
            page: Page::new(612.0, 792.0),
            marked: false,
            paper_code: 2,
            paper: (612.0, 792.0),
            orientation: 0,
            units: 300.0,
            x: 0.0,
            y: 0.0,
            left_margin: 0.0,
            right_margin: 0.0,
            top_margin: 360.0,
            text_length: 0.0,
            vmi: 120.0,
            hmi: 72.0,
            line_term: 0,
            print_dir: 0,
            wrap: false,
            cursor_stack: Vec::new(),
            primary: FontSel::default(),
            secondary: FontSel::default(),
            use_secondary: false,
            soft: BTreeMap::new(),
            font_id: 0,
            char_code: 0,
            underline: None,
            raster: Raster { active: false, res: 75.0, width: 0, height: 0, compression: 0, x: 0.0, y: 0.0, rows: Vec::new(), row_count: 0, row_width: 0, seeds: Vec::new(), planes: Vec::new(), left_at_cursor: false, dest: (None, None) },
            rect_w: 0.0,
            rect_h: 0.0,
            pattern_id: 0,
            palette: vec![Rgb(1.0, 1.0, 1.0), Rgb::BLACK],
            fg: Rgb::BLACK,
            comps: [0.0; 3],
            pix: PixMode::Mono,
            source_transparent: true,
            current_pattern: 0,
            macros: HashMap::new(),
            macro_id: 0,
            overlay: None,
            depth: 0,
            hpgl: Hpgl::new(frame),
            frame_size: (None, None),
            frame_anchor: None,
            sys_fonts: HashMap::new(),
            glyph_cache: HashMap::new(),
            last_adv: 0.0,
            text_log: None,
            stats: BTreeMap::new(),
            page_settings: Vec::new(),
            job: BTreeMap::new(),
            fonts_used: BTreeMap::new(),
            images: 0,
            rects: 0,
            base: 0,
        };
        s.reset_page_setup();
        s
    }

    // ------------------------------------------------------------ page setup

    /// Physical page in points for the current orientation.
    fn phys(&self) -> (f64, f64) {
        if self.orientation % 2 == 1 { (self.paper.1, self.paper.0) } else { self.paper }
    }

    /// Logical page left offset (decipoints) and width (decipoints).
    fn logical(&self) -> (f64, f64) {
        let metric = matches!(self.paper_code, 25 | 26 | 27 | 45 | 46);
        let off = if self.orientation % 2 == 1 { if metric { 141.6 } else { 144.0 } } else if metric { 170.4 } else { 180.0 };
        let (w, _) = self.phys();
        (off, w * 10.0 - 2.0 * off)
    }

    fn reset_page_setup(&mut self) {
        let (_, lw) = self.logical();
        let (_, h) = self.phys();
        self.left_margin = 0.0;
        self.right_margin = lw;
        self.top_margin = 360.0;
        self.vmi = 120.0;
        self.text_length = ((h * 10.0 - 360.0 - 360.0) / self.vmi).floor().max(1.0);
        self.x = 0.0;
        self.y = self.top_margin + 0.75 * self.vmi;
        self.hpgl = Hpgl::new(self.default_frame());
        self.frame_size = (None, None);
        self.frame_anchor = None;
        self.new_page();
    }

    fn new_page(&mut self) {
        let (w, h) = self.phys();
        let src = self.page.cur_src;
        self.page = Page::new(w as f32, h as f32);
        self.page.cur_src = src;
        self.marked = false;
    }

    fn default_frame(&self) -> Frame {
        let (off, lw) = self.logical();
        let (_, h) = self.phys();
        let top = self.top_margin;
        let len = (self.text_length * self.vmi).min(h * 10.0 - top);
        Frame { x: off / 10.0, y: top / 10.0, w: lw / 10.0, h: len.max(10.0) / 10.0 }
    }


    fn dir_matrix(&self) -> Matrix {
        let (off, lw) = self.logical();
        let (_, h) = self.phys();
        let lh = h * 10.0;
        let base = Matrix::scale(0.1, 0.1).then(&Matrix::translate(off / 10.0, 0.0));
        let rot = match self.print_dir {
            90 => Matrix::new(0.0, -1.0, 1.0, 0.0, 0.0, lh),
            180 => Matrix::new(-1.0, 0.0, 0.0, -1.0, lw, lh),
            270 => Matrix::new(0.0, 1.0, -1.0, 0.0, lw, 0.0),
            _ => Matrix::identity(),
        };
        rot.then(&base)
    }

    fn eject(&mut self, force: bool) {
        if !self.marked && !force {
            return;
        }
        self.end_raster();
        self.flush_underline();
        if let Some(id) = self.overlay {
            if let Some(m) = self.macros.get(&id).cloned() {
                let saved = (self.x, self.y, self.primary.clone(), self.fg);
                self.depth += 1;
                self.run_inner(&m, usize::MAX);
                self.depth -= 1;
                self.x = saved.0;
                self.y = saved.1;
                self.primary = saved.2;
                self.fg = saved.3;
            }
        }
        let name = paper_points(self.paper_code).map(|p| p.2.to_string()).unwrap_or_else(|| format!("code {}", self.paper_code));
        let mut p = std::mem::replace(&mut self.page, Page::new(1.0, 1.0));
        p.label = format!("{name} {}", if self.orientation % 2 == 1 { "landscape" } else { "portrait" });
        p.settings = self.page_settings.clone();
        p.settings.insert(0, ("Media".into(), name));
        p.settings.insert(1, ("Orientation".into(), if self.orientation % 2 == 1 { "Landscape".into() } else { "Portrait".into() }));
        self.doc.pages.push(p);
        self.new_page();
        self.x = self.left_margin;
        self.y = self.top_margin + 0.75 * self.vmi;
    }

    fn mark(&mut self) {
        self.marked = true;
    }

    // ------------------------------------------------------------ fonts

    fn sel(&self) -> &FontSel {
        if self.use_secondary { &self.secondary } else { &self.primary }
    }

    fn sys_font(&mut self) -> Arc<SysFont> {
        let s = self.sel().clone();
        let fam = match s.typeface {
            0 | 3 | 4099 | 24579 => "Courier",
            6 | 4102 => "Letter Gothic Mono",
            5 | 4101 | 16901 | 25093 | 24607 | 4197 => "Times New Roman",
            _ if s.symset == 621 => "Symbol",
            _ if s.symset == 18540 => "Wingdings",
            _ if s.spacing == 0 => "Courier",
            _ => "Arial",
        };
        let name = format!("{fam}{}{}", if s.weight > 0 { " Bold" } else { "" }, if s.style & 3 != 0 { " Italic" } else { "" });
        if let Some(f) = self.sys_fonts.get(&name) {
            return f.clone();
        }
        let f = fonts::system_font(&name);
        self.sys_fonts.insert(name, f.clone());
        f
    }

    fn font_desc(&self) -> String {
        let s = self.sel();
        if let Some(id) = s.id {
            if let Some(f) = self.soft.get(&id) {
                return format!("Soft font {id}{}", if f.name.is_empty() { String::new() } else { format!(" \"{}\"", f.name) });
            }
        }
        format!(
            "Typeface {} {}{}{}, {}",
            s.typeface,
            if s.spacing == 0 { format!("{} pitch", s.pitch) } else { format!("{} pt", s.height) },
            if s.weight > 0 { " bold" } else { "" },
            if s.style & 3 != 0 { " italic" } else { "" },
            symset_name(s.symset)
        )
    }

    fn update_hmi(&mut self) {
        let s = self.sel().clone();
        if let Some(f) = s.id.and_then(|id| self.soft.get(&id)) {
            if f.spacing == 0 && f.pitch_qd > 0.0 {
                // pitch in quarter-dots at 300 dpi → decipoints
                self.hmi = f.pitch_qd / 4.0 / 300.0 * 720.0;
                return;
            }
        }
        if s.spacing == 0 && s.pitch > 0.0 {
            self.hmi = 720.0 / s.pitch;
        } else {
            self.hmi = s.height * 10.0 * 0.5;
        }
    }

    // ------------------------------------------------------------ drawing

    fn brush_colour(&self) -> Rgb {
        match self.current_pattern {
            1 => Rgb(1.0, 1.0, 1.0),
            2 => {
                let k = (self.pattern_id as f32 / 100.0).clamp(0.0, 1.0);
                Rgb(1.0 - (1.0 - self.fg.0) * k, 1.0 - (1.0 - self.fg.1) * k, 1.0 - (1.0 - self.fg.2) * k)
            }
            _ => self.fg,
        }
    }

    fn fill_rect_dp(&mut self, x: f64, y: f64, w: f64, h: f64, c: Rgb) {
        let m = self.dir_matrix();
        let mut acc = PathAcc::new();
        let pts = [(x, y), (x + w, y), (x + w, y + h), (x, y + h)];
        for (i, (px, py)) in pts.iter().enumerate() {
            let (a, b) = m.apply(*px, *py);
            if i == 0 {
                acc.move_to(a, b);
            } else {
                acc.line_to(a, b);
            }
        }
        acc.close();
        if let Some(path) = acc.to_path(&Matrix::identity()) {
            self.page.push(Item::Fill { path, transform: Transform::identity(), brush: Brush::Solid(c), even_odd: false, clip: None });
            self.mark();
        }
    }

    fn flush_underline(&mut self) {
        if let Some(x0) = self.underline {
            if self.x > x0 + 0.5 {
                let y = self.y + 0.03 * 720.0 / 7.2 * 0.75;
                let c = self.brush_colour();
                self.fill_rect_dp(x0, y, self.x - x0, 7.2, c);
            }
            self.underline = Some(self.x);
        }
    }

    fn print_char(&mut self, code: u32) {
        let sel = self.sel().clone();
        let colour = self.brush_colour();
        let m = self.dir_matrix();
        let mut adv = self.hmi;
        let mut ch = decode_char(sel.symset, code);
        // soft font?
        if let Some(id) = sel.id {
            if self.soft.contains_key(&id) {
                self.print_soft(id, code, colour, &m, &mut adv, &mut ch);
                self.finish_char(ch, adv, &m, sel.height.max(4.0) * 10.0);
                return;
            }
        }
        let f = self.sys_font();
        *self.fonts_used.entry(self.font_desc()).or_insert(0) += 1;
        let size_pt = if sel.spacing == 0 { 120.0 / sel.pitch.max(0.5) } else { sel.height.max(0.25) };
        let gch = if f.symbol || sel.symset == 18540 { char::from_u32(code).unwrap_or(' ') } else { ch };
        if let Some(gid) = f.glyph_for_char(gch) {
            let s = size_pt * 10.0 / f.upem;
            let gm = Matrix::new(s, 0.0, 0.0, -s, self.x, self.y).then(&m);
            let mut acc = PathAcc::new();
            f.outline(gid, &gm, &mut acc);
            if sel.spacing != 0 {
                adv = f.advance(gid) * s;
            }
            if !acc.is_empty() {
                if let Some(path) = acc.to_path(&Matrix::identity()) {
                    self.page.cur_kind = Kind::Text;
                    self.page.push(Item::Fill { path, transform: Transform::identity(), brush: Brush::Solid(colour), even_odd: false, clip: None });
                    self.page.cur_kind = Kind::Vector;
                }
            }
        } else if sel.spacing != 0 {
            adv = size_pt * 10.0 * if ch == ' ' { 0.28 } else { 0.5 };
        }
        self.finish_char(ch, adv, &m, size_pt * 10.0);
    }

    fn finish_char(&mut self, ch: char, adv: f64, m: &Matrix, em_dp: f64) {
        let a = m.apply(self.x, self.y);
        let b = m.apply(self.x + adv, self.y);
        let up = m.apply_delta(0.0, -em_dp);
        if !ch.is_control() {
            self.page.text_char(ch, a, b, up);
        }
        self.x += adv;
        self.last_adv = adv;
        self.mark();
    }

    fn print_soft(&mut self, id: u32, code: u32, colour: Rgb, m: &Matrix, adv: &mut f64, ch: &mut char) {
        let height_pt = self.sel().height;
        let font_label;
        {
            let f = self.soft.get_mut(&id).unwrap();
            f.used = true;
            font_label = format!("Soft font {id}{}", if f.name.is_empty() { String::new() } else { format!(" \"{}\"", f.name) });
            if let SoftKind::TrueType { glyphs, .. } = &f.kind {
                let n = glyphs.len();
                if f.ident.is_none() && n >= 3 && (f.ident_tried == 0 || n >= f.ident_tried * 2) && f.ident_tried < 1000 {
                    f.ident_tried = n.max(1);
                    let samples: Vec<(u16, Vec<u8>)> = glyphs.iter().map(|(g, d)| (*g, d.clone())).collect();
                    f.ident = fonts::identify_subset(&samples);
                }
            }
        }
        *self.fonts_used.entry(font_label).or_insert(0) += 1;
        let f = self.soft.get(&id).unwrap();
        *ch = decode_char(f.symset, code);
        match &f.kind {
            SoftKind::Bitmap { res, chars } => {
                if let Some(g) = chars.get(&code) {
                    let px = 720.0 / res; // decipoints per dot
                    if f.spacing != 0 || g.delta_qd > 0.0 {
                        *adv = g.delta_qd / 4.0 * px;
                    }
                    if g.w > 0 && g.h > 0 {
                        let key = (id, code, ((colour.0 * 255.0) as u32) << 16 | ((colour.1 * 255.0) as u32) << 8 | (colour.2 * 255.0) as u32);
                        let img = self
                            .glyph_cache
                            .entry(key)
                            .or_insert_with(|| {
                                let stride = (g.w as usize).div_ceil(8);
                                let mut rgba = vec![0u8; (g.w * g.h * 4) as usize];
                                let (r, gg, b) = ((colour.0 * 255.0) as u8, (colour.1 * 255.0) as u8, (colour.2 * 255.0) as u8);
                                for y in 0..g.h as usize {
                                    for x in 0..g.w as usize {
                                        if g.bits.get(y * stride + x / 8).map(|v| v & (0x80 >> (x % 8)) != 0).unwrap_or(false) {
                                            let o = (y * g.w as usize + x) * 4;
                                            rgba[o..o + 4].copy_from_slice(&[r, gg, b, 255]);
                                        }
                                    }
                                }
                                Arc::new(RasterImage::new(g.w, g.h, rgba))
                            })
                            .clone();
                        let t = Matrix::scale(px, px).then(&Matrix::translate(self.x + g.left as f64 * px, self.y - g.top as f64 * px)).then(m);
                        self.page.cur_kind = Kind::Text;
                        self.page.push(Item::Image { image: img, transform: t.to_ts(), clip: None });
                        self.page.cur_kind = Kind::Vector;
                    }
                } else if f.spacing != 0 {
                    *adv = f.pitch_qd.max(4.0) / 4.0 * 720.0 / 300.0;
                }
            }
            SoftKind::TrueType { sfnt, chars, glyphs } => {
                if let Some(&gid) = chars.get(&code) {
                    if let Some(i) = &f.ident {
                        if let Some(c) = i.map.get(&gid) {
                            *ch = *c;
                        }
                    }
                    let size_dp = if f.spacing == 0 && self.sel().spacing == 0 { 1200.0 / self.sel().pitch.max(0.5) } else { height_pt.max(0.25) * 10.0 };
                    let s = size_dp / sfnt.upem;
                    let gm = Matrix::new(s, 0.0, 0.0, -s, self.x, self.y).then(m);
                    let mut acc = PathAcc::new();
                    if let Some(gd) = glyphs.get(&gid) {
                        let lookup = |g: u16| glyphs.get(&g).cloned().or_else(|| sfnt.glyph_data(g));
                        fonts::glyf_outline(gd, &gm, &mut acc, &lookup, 0);
                    }
                    if f.spacing != 0 {
                        if let Some(a) = sfnt.advance(gid) {
                            *adv = a * s;
                        }
                    }
                    if !acc.is_empty() {
                        if let Some(path) = acc.to_path(&Matrix::identity()) {
                            self.page.cur_kind = Kind::Text;
                            self.page.push(Item::Fill { path, transform: Transform::identity(), brush: Brush::Solid(colour), even_odd: false, clip: None });
                            self.page.cur_kind = Kind::Vector;
                        }
                    }
                }
            }
            SoftKind::Unsupported => {}
        }
    }

    // ------------------------------------------------------------ soft fonts

    fn define_font(&mut self, d: &[u8]) {
        if d.len() < 64 {
            self.doc.warn("Soft font header too short");
            return;
        }
        let be16 = |o: usize| u16::from_be_bytes([d[o], d[o + 1]]) as f64;
        let format = d[2];
        let spacing = d[13] as i32;
        let symset = be16(14) as u32;
        let pitch_qd = be16(16);
        let height_qd = be16(18);
        let typeface = d[26] as i32 * 256 + d[25] as i32;
        let name: String = d[48..64].iter().take_while(|c| **c != 0).map(|&c| c as char).collect::<String>().trim().to_string();
        let kind = match format {
            0 | 20 => {
                let res = if format == 20 && d.len() >= 70 { be16(64).max(75.0) } else { 300.0 };
                SoftKind::Bitmap { res, chars: HashMap::new() }
            }
            15 | 16 => {
                // segmented data follows the fixed header; find the GT (global TrueType) segment
                let hsize = be16(0) as usize;
                let mut sfnt = SfntInfo::default();
                sfnt.upem = if format == 15 && d.len() >= 72 { be16(64).max(16.0) } else { 2048.0 };
                let mut p = hsize.min(d.len());
                while p + 4 <= d.len() {
                    let id = [d[p], d[p + 1]];
                    let size = if format == 16 { u32::from_be_bytes([d.get(p + 2).copied().unwrap_or(0), d.get(p + 3).copied().unwrap_or(0), d.get(p + 4).copied().unwrap_or(0), d.get(p + 5).copied().unwrap_or(0)]) as usize } else { be16(p + 2) as usize };
                    let hl = if format == 16 { 6 } else { 4 };
                    let s = p + hl;
                    let e = (s + size).min(d.len());
                    if &id == b"GT" {
                        let parsed = SfntInfo::parse(&d[s..e]);
                        let upem = sfnt.upem;
                        sfnt = parsed;
                        if sfnt.upem <= 16.0 {
                            sfnt.upem = upem;
                        }
                    }
                    if id == [0xFF, 0xFF] || size == 0 && id != *b"GT" {
                        break;
                    }
                    p = e;
                }
                SoftKind::TrueType { sfnt, chars: HashMap::new(), glyphs: HashMap::new() }
            }
            _ => {
                self.doc.warn(format!("Soft font format {format} (e.g. Intellifont) is not supported; text in it is substituted"));
                SoftKind::Unsupported
            }
        };
        self.soft.insert(self.font_id, SoftFont { format, spacing, pitch_qd, height_qd, symset, typeface, name, kind, ident: None, ident_tried: 0, used: false });
    }

    fn define_char(&mut self, d: &[u8]) {
        let code = self.char_code;
        let Some(f) = self.soft.get_mut(&self.font_id) else { return };
        if d.len() < 4 {
            return;
        }
        match &mut f.kind {
            SoftKind::Bitmap { chars, .. } => {
                if d[0] != 4 || d.len() < 16 {
                    return;
                }
                if d[1] != 0 {
                    // continuation block: append raw bits to the last char
                    if let Some(g) = chars.get_mut(&code) {
                        g.bits.extend_from_slice(&d[2..]);
                    }
                    return;
                }
                let class = d[3];
                let be = |o: usize| i16::from_be_bytes([d[o], d[o + 1]]) as i32;
                let left = be(6);
                let top = be(8);
                let w = be(10).max(0) as u32;
                let h = be(12).max(0) as u32;
                let delta = be(14) as f64;
                let data = &d[16.min(d.len())..];
                let bits = if class == 2 {
                    // compressed bitmap: rows of alternating run lengths with a repeat count
                    let stride = (w as usize).div_ceil(8);
                    let mut out = Vec::with_capacity(stride * h as usize);
                    let mut p = 0;
                    while p < data.len() && out.len() < stride * h as usize {
                        let rep = data[p] as usize + 1;
                        p += 1;
                        let mut row = vec![0u8; stride];
                        let mut x = 0usize;
                        let mut on = false;
                        while x < w as usize && p < data.len() {
                            let run = data[p] as usize;
                            p += 1;
                            if on {
                                for k in x..(x + run).min(w as usize) {
                                    row[k / 8] |= 0x80 >> (k % 8);
                                }
                            }
                            x += run;
                            on = !on;
                        }
                        for _ in 0..rep {
                            out.extend_from_slice(&row);
                        }
                    }
                    out
                } else {
                    data.to_vec()
                };
                chars.insert(code, BitmapGlyph { left, top, w, h, delta_qd: delta, bits });
            }
            SoftKind::TrueType { chars, glyphs, .. } => {
                // format 15: [15, cont, desc size, class 15, data size(2), glyph id(2), glyph data]
                if d[0] != 15 && d[0] != 10 && d[0] != 16 {
                    return;
                }
                let ds = d[2] as usize;
                let p = 2 + ds;
                if p + 4 > d.len() {
                    return;
                }
                let gid = u16::from_be_bytes([d[p + 2], d[p + 3]]);
                let gdata = d[(p + 4).min(d.len())..].to_vec();
                chars.insert(code, gid);
                if !gdata.is_empty() {
                    glyphs.insert(gid, gdata);
                }
            }
            SoftKind::Unsupported => {}
        }
    }

    // ------------------------------------------------------------ raster

    fn start_raster(&mut self, mode: i32) {
        if self.raster.active {
            return;
        }
        self.raster.active = true;
        self.raster.left_at_cursor = mode & 1 == 1;
        self.raster.x = if self.raster.left_at_cursor { self.x } else { 0.0 };
        self.raster.y = self.y;
        self.raster.rows.clear();
        self.raster.row_count = 0;
        self.raster.row_width = 0;
        self.raster.seeds.clear();
        self.raster.planes.clear();
    }

    fn planes_per_row(&self) -> usize {
        match self.pix {
            PixMode::Mono => 1,
            PixMode::SimpleRgb | PixMode::SimpleCmy => 3,
            PixMode::Cid { mode, bits_index, .. } => match mode {
                0 => bits_index.max(1) as usize,
                2 => 3,
                _ => 1,
            },
        }
    }

    fn decompress(&mut self, data: &[u8], plane: usize) -> Vec<u8> {
        while self.raster.seeds.len() <= plane {
            self.raster.seeds.push(Vec::new());
        }
        let row = match self.raster.compression {
            0 => data.to_vec(),
            1 => {
                let mut out = Vec::new();
                for pair in data.chunks(2) {
                    if pair.len() == 2 {
                        out.extend(std::iter::repeat(pair[1]).take(pair[0] as usize + 1));
                    }
                }
                out
            }
            2 => crate::pclxl::packbits(data, 0),
            3 => {
                let seed = &mut self.raster.seeds[plane];
                let need = data.len() * 8 + 64;
                if seed.len() < need.min(65536) {
                    seed.resize(seed.len().max(self.raster.width as usize * 3 + 8), 0);
                }
                crate::pclxl::delta_row(data, seed);
                seed.clone()
            }
            9 => {
                let seed = &mut self.raster.seeds[plane];
                if seed.len() < self.raster.width as usize * 3 + 8 {
                    seed.resize(self.raster.width as usize * 3 + 8, 0);
                }
                replacement_delta(data, seed);
                seed.clone()
            }
            5 => {
                // adaptive: handled per block elsewhere; treat as uncompressed here
                data.to_vec()
            }
            c => {
                self.doc.warn(format!("PCL raster compression method {c} is not supported"));
                data.to_vec()
            }
        };
        if self.raster.compression != 3 && self.raster.compression != 9 {
            self.raster.seeds[plane] = row.clone();
        }
        row
    }

    fn raster_plane(&mut self, data: &[u8], last: bool) {
        if !self.raster.active {
            self.start_raster(0);
        }
        if self.raster.compression == 5 && last {
            self.adaptive_block(data);
            return;
        }
        let plane = self.raster.planes.len();
        let row = self.decompress(data, plane);
        self.raster.planes.push(row);
        if last {
            self.emit_row();
        }
    }

    fn adaptive_block(&mut self, d: &[u8]) {
        let mut p = 0;
        while p + 3 <= d.len() {
            let cmd = d[p];
            let n = u16::from_be_bytes([d[p + 1], d[p + 2]]) as usize;
            p += 3;
            match cmd {
                0..=3 => {
                    let e = (p + n).min(d.len());
                    let saved = self.raster.compression;
                    self.raster.compression = cmd as i32;
                    let row = self.decompress(&d[p..e], 0);
                    self.raster.compression = saved;
                    self.raster.planes.push(row);
                    self.emit_row();
                    p = e;
                }
                4 => {
                    for _ in 0..n {
                        self.raster.planes.push(Vec::new());
                        self.emit_row();
                    }
                }
                5 => {
                    for _ in 0..n {
                        let seed = self.raster.seeds.first().cloned().unwrap_or_default();
                        self.raster.planes.push(seed);
                        self.emit_row();
                    }
                }
                _ => break,
            }
        }
    }

    fn emit_row(&mut self) {
        let planes = std::mem::take(&mut self.raster.planes);
        if self.raster.height > 0 && self.raster.row_count >= self.raster.height {
            // rows beyond the declared raster height are clipped
            return;
        }
        let bit = |v: &Vec<u8>, i: usize| v.get(i / 8).map(|b| b & (0x80 >> (i % 8)) != 0).unwrap_or(false);
        let nominal = if self.raster.width > 0 { self.raster.width as usize } else { 0 };
        let (w, px): (usize, Vec<[u8; 4]>) = match self.pix {
            PixMode::Mono => {
                let w = if nominal > 0 { nominal } else { planes[0].len() * 8 };
                let fg = self.fg;
                let c = [(fg.0 * 255.0) as u8, (fg.1 * 255.0) as u8, (fg.2 * 255.0) as u8, 255];
                (w, (0..w).map(|i| if bit(&planes[0], i) { c } else { [255, 255, 255, if self.source_transparent { 0 } else { 255 }] }).collect())
            }
            PixMode::SimpleRgb | PixMode::SimpleCmy | PixMode::Cid { mode: 0, .. } => {
                let w = if nominal > 0 { nominal } else { planes.first().map(|p| p.len() * 8).unwrap_or(0) };
                let pal = self.palette.clone();
                (
                    w,
                    (0..w)
                        .map(|i| {
                            let mut idx = 0usize;
                            for (k, p) in planes.iter().enumerate() {
                                if bit(p, i) {
                                    idx |= 1 << k;
                                }
                            }
                            let c = pal.get(idx).copied().unwrap_or(Rgb::BLACK);
                            let white = c.0 > 0.99 && c.1 > 0.99 && c.2 > 0.99;
                            [(c.0 * 255.0) as u8, (c.1 * 255.0) as u8, (c.2 * 255.0) as u8, if white && self.source_transparent { 0 } else { 255 }]
                        })
                        .collect(),
                )
            }
            PixMode::Cid { mode: 1, bits_index, .. } => {
                let b = bits_index.clamp(1, 8) as usize;
                let row = planes.first().cloned().unwrap_or_default();
                let w = if nominal > 0 { nominal } else { row.len() * 8 / b };
                let pal = self.palette.clone();
                (
                    w,
                    (0..w)
                        .map(|i| {
                            let bitpos = i * b;
                            let byte = row.get(bitpos / 8).copied().unwrap_or(0) as usize;
                            let idx = (byte >> (8 - b - bitpos % 8)) & ((1 << b) - 1);
                            let c = pal.get(idx).copied().unwrap_or(Rgb::BLACK);
                            let white = c.0 > 0.99 && c.1 > 0.99 && c.2 > 0.99;
                            [(c.0 * 255.0) as u8, (c.1 * 255.0) as u8, (c.2 * 255.0) as u8, if white && self.source_transparent { 0 } else { 255 }]
                        })
                        .collect(),
                )
            }
            PixMode::Cid { mode: 2, cmy, .. } => {
                let w = if nominal > 0 { nominal } else { planes.first().map(|p| p.len() * 8).unwrap_or(0) };
                (
                    w,
                    (0..w)
                        .map(|i| {
                            let mut c = [0u8; 3];
                            for k in 0..3 {
                                let on = planes.get(k).map(|p| bit(p, i)).unwrap_or(false);
                                c[k] = if on != cmy { 255 } else { 0 };
                            }
                            let white = c == [255, 255, 255];
                            [c[0], c[1], c[2], if white && self.source_transparent { 0 } else { 255 }]
                        })
                        .collect(),
                )
            }
            PixMode::Cid { cmy, bits, .. } => {
                let row = planes.first().cloned().unwrap_or_default();
                let bpp = (bits.iter().map(|b| *b as usize).sum::<usize>()).max(24) / 8;
                let w = if nominal > 0 { nominal } else { row.len() / bpp.max(1) };
                (
                    w,
                    (0..w)
                        .map(|i| {
                            let o = i * bpp;
                            let g = |k: usize| row.get(o + k).copied().unwrap_or(if cmy { 0 } else { 255 });
                            let mut c = [g(0), g(1), g(2)];
                            if cmy {
                                c = [255 - c[0], 255 - c[1], 255 - c[2]];
                            }
                            let white = c == [255, 255, 255];
                            [c[0], c[1], c[2], if white && self.source_transparent { 0 } else { 255 }]
                        })
                        .collect(),
                )
            }
        };
        if w == 0 {
            self.raster.row_count += 1;
            return;
        }
        let w = w.min(20000);
        if self.raster.row_width == 0 {
            self.raster.row_width = w as u32;
        }
        let rw = self.raster.row_width as usize;
        // pad rows that were skipped / shorter
        for i in 0..rw {
            let p = px.get(i).copied().unwrap_or([255, 255, 255, 0]);
            self.raster.rows.extend_from_slice(&p);
        }
        self.raster.row_count += 1;
        if self.raster.rows.len() > 200 * 1024 * 1024 {
            self.end_raster();
        }
    }

    fn skip_rows(&mut self, n: f64) {
        if !self.raster.active {
            self.start_raster(0);
        }
        let rw = self.raster.row_width as usize;
        for _ in 0..(n.max(0.0) as usize).min(20000) {
            if rw > 0 {
                self.raster.rows.extend(std::iter::repeat([255u8, 255, 255, 0]).take(rw).flatten());
            }
            self.raster.row_count += 1;
            for s in self.raster.seeds.iter_mut() {
                s.iter_mut().for_each(|b| *b = 0);
            }
        }
    }

    fn end_raster(&mut self) {
        if !self.raster.active {
            return;
        }
        self.raster.active = false;
        let rows = self.raster.row_count;
        let w = self.raster.row_width;
        let dp = 720.0 / self.raster.res.max(1.0);
        let (sx, sy) = match self.raster.dest {
            (Some(dw), Some(dh)) if w > 0 && rows > 0 => (dw / w as f64, dh / rows as f64),
            (Some(dw), None) if w > 0 => (dw / w as f64, dw / w as f64),
            _ => (dp, dp),
        };
        if w > 0 && rows > 0 && self.raster.rows.len() >= (w * rows * 4) as usize {
            let mut rgba = std::mem::take(&mut self.raster.rows);
            rgba.truncate((w * rows * 4) as usize);
            // premultiply alpha
            for p in rgba.chunks_exact_mut(4) {
                if p[3] == 0 {
                    p[0] = 0;
                    p[1] = 0;
                    p[2] = 0;
                }
            }
            let t = Matrix::scale(sx, sy).then(&Matrix::translate(self.raster.x, self.raster.y)).then(&self.dir_matrix());
            let (img, adj) = RasterImage::fitted(w, rows, rgba, &t);
            self.page.push(Item::Image { image: Arc::new(img), transform: adj.to_ts(), clip: None });
            self.images += 1;
            self.mark();
        }
        self.raster.rows.clear();
        self.y = self.raster.y + rows as f64 * sy;
    }

    // ------------------------------------------------------------ colour

    fn set_simple_colour(&mut self, v: i32) {
        match v {
            3 => {
                self.pix = PixMode::SimpleRgb;
                self.palette = crate::hpgl::default_pens();
                self.palette[0] = Rgb::BLACK;
                self.palette[7] = Rgb(1.0, 1.0, 1.0);
                // RGB order: index bits = R | G<<1 | B<<2
                self.palette = (0..8).map(|i| Rgb((i & 1) as f32, ((i >> 1) & 1) as f32, ((i >> 2) & 1) as f32)).collect();
            }
            -3 => {
                self.pix = PixMode::SimpleCmy;
                self.palette = (0..8).map(|i| Rgb(1.0 - (i & 1) as f32, 1.0 - ((i >> 1) & 1) as f32, 1.0 - ((i >> 2) & 1) as f32)).collect();
            }
            _ => {
                self.pix = PixMode::Mono;
                self.palette = vec![Rgb(1.0, 1.0, 1.0), Rgb::BLACK];
            }
        }
        self.fg = Rgb::BLACK;
    }

    fn configure_cid(&mut self, d: &[u8]) {
        if d.len() < 6 {
            return;
        }
        let cmy = d[0] == 1;
        let mode = d[1];
        let bits_index = d[2];
        let bits = [d[3], d[4], d[5]];
        self.pix = PixMode::Cid { cmy, mode, bits_index, bits };
        let n = 1usize << bits_index.min(8);
        let mut pal: Vec<Rgb> = vec![Rgb::BLACK; n.max(2)];
        if n >= 8 {
            for i in 0..8 {
                let c = Rgb((i & 1) as f32, ((i >> 1) & 1) as f32, ((i >> 2) & 1) as f32);
                pal[i] = if cmy { Rgb(1.0 - c.0, 1.0 - c.1, 1.0 - c.2) } else { c };
            }
        } else {
            pal[0] = Rgb(1.0, 1.0, 1.0);
        }
        self.palette = pal;
        self.fg = Rgb::BLACK;
    }

    // ------------------------------------------------------------ HP-GL/2

    fn enter_hpgl(&mut self, mode: i32, d: &[u8], p: usize) -> usize {
        self.end_raster();
        self.flush_text_log();
        let (off, _) = self.logical();
        let def = self.default_frame();
        let anchor = self.frame_anchor.map(|(x, y)| ((off + x) / 10.0, y / 10.0)).unwrap_or((def.x, def.y));
        let frame = Frame { x: anchor.0, y: anchor.1, w: self.frame_size.0.map(|v| v / 10.0).unwrap_or(def.w), h: self.frame_size.1.map(|v| v / 10.0).unwrap_or(def.h) };
        let changed = (frame.x - self.hpgl.frame.x).abs() > 0.01 || (frame.y - self.hpgl.frame.y).abs() > 0.01 || (frame.w - self.hpgl.frame.w).abs() > 0.01 || (frame.h - self.hpgl.frame.h).abs() > 0.01;
        if changed {
            self.hpgl = Hpgl::new(frame);
        }
        self.hpgl.pens = self.palette.clone();
        if self.hpgl.pens.len() < 2 {
            self.hpgl.pens = vec![Rgb(1.0, 1.0, 1.0), Rgb::BLACK];
        }
        let _ = mode;
        *self.stats.entry("HP-GL/2 sections".into()).or_insert(0) += 1;
        let before = self.page.items.len();
        let end = self.hpgl.run(d, p, self.base, &mut self.page, self.doc);
        if self.page.items.len() > before {
            self.mark();
        }
        self.palette = self.hpgl.pens.clone();
        end
    }

    // ------------------------------------------------------------ main loop

    fn flush_text_log(&mut self) {
        if let Some((off, s)) = self.text_log.take() {
            self.doc.cmd(off, format!("{off:08X}  Text  \"{s}\""));
        }
    }

    fn log(&mut self, off: usize, s: String) {
        self.flush_text_log();
        self.doc.cmd(off, format!("{off:08X}  {s}"));
    }

    pub fn run(&mut self, d: &[u8], base: usize) {
        self.base = base;
        self.run_inner(d, base);
        self.end_raster();
        self.flush_text_log();
        if self.marked {
            self.eject(false);
        }
        self.finish_info();
    }

    fn run_inner(&mut self, d: &[u8], base: usize) {
        let in_macro = base == usize::MAX;
        let off_of = |p: usize| if in_macro { NO_SRC as usize } else { base + p };
        let mut p = 0;
        let n = d.len();
        while p < n {
            let c = d[p];
            if c == 0x1B {
                self.end_text_run_if_needed();
                let start = p;
                if !in_macro {
                    self.page.cur_src = (base + p) as u32;
                }
                p = self.escape(d, p + 1, off_of(start), in_macro);
                continue;
            }
            match c {
                b'\r' => {
                    self.flush_underline();
                    self.x = self.left_margin;
                    if self.line_term == 1 || self.line_term == 3 {
                        self.y += self.vmi;
                    }
                }
                b'\n' => {
                    self.flush_underline();
                    self.y += self.vmi;
                    if self.line_term >= 2 {
                        self.x = self.left_margin;
                    }
                }
                0x0C => {
                    self.flush_text_log();
                    self.log(off_of(p), "Form feed (page eject)".into());
                    self.eject(true);
                    if self.line_term >= 2 {
                        self.x = self.left_margin;
                    }
                }
                0x08 => self.x = (self.x - self.last_adv).max(0.0),
                0x09 => {
                    let tab = self.hmi * 8.0;
                    self.x = ((self.x - self.left_margin) / tab + 1.0).floor() * tab + self.left_margin;
                }
                0x0E => self.use_secondary = true,
                0x0F => self.use_secondary = false,
                0x00 | 0x07 | 0x11..=0x13 | 0x7F => {}
                _ => {
                    if !in_macro {
                        let ch = decode_char(self.sel().symset, c as u32);
                        match &mut self.text_log {
                            Some((_, s)) if s.len() < 200 => s.push(ch),
                            Some(_) => {}
                            None => self.text_log = Some((base + p, ch.to_string())),
                        }
                        self.page.cur_src = self.text_log.as_ref().map(|t| t.0 as u32).unwrap_or(NO_SRC);
                    }
                    if self.wrap && self.x + self.hmi > self.right_margin + 1.0 {
                        self.x = self.left_margin;
                        self.y += self.vmi;
                    }
                    self.print_char(c as u32);
                }
            }
            p += 1;
        }
    }

    fn end_text_run_if_needed(&mut self) {
        self.page.text_break();
    }

    /// Parse one escape sequence starting after ESC; returns the new position.
    fn escape(&mut self, d: &[u8], mut p: usize, off: usize, in_macro: bool) -> usize {
        let n = d.len();
        if p >= n {
            return p;
        }
        let c = d[p];
        // two-character sequences
        if (48..=126).contains(&c) {
            p += 1;
            match c {
                b'E' => {
                    self.log(off, "Printer reset (ESC E)".into());
                    self.reset();
                }
                b'9' => {
                    let (_, lw) = self.logical();
                    self.left_margin = 0.0;
                    self.right_margin = lw;
                }
                b'=' => self.y += self.vmi / 2.0,
                b'Y' | b'Z' => {}
                _ => {}
            }
            return p;
        }
        if !(33..=47).contains(&c) {
            return p;
        }
        let prefix = c;
        p += 1;
        // UEL: ESC%-12345X
        if prefix == b'%' && d[p..].starts_with(b"-12345X") {
            return n;
        }
        let mut group = 0u8;
        let font_prefix = prefix == b'(' || prefix == b')';
        if p < n && ((96..=126).contains(&d[p]) && !font_prefix || font_prefix && (d[p] == b's' || d[p] == b'f')) {
            group = d[p];
            p += 1;
        }
        loop {
            // value
            let vs = p;
            if p < n && (d[p] == b'+' || d[p] == b'-') {
                p += 1;
            }
            while p < n && (d[p].is_ascii_digit() || d[p] == b'.') {
                p += 1;
            }
            let txt = std::str::from_utf8(&d[vs..p]).unwrap_or("");
            let relative = txt.starts_with('+') || txt.starts_with('-');
            let has_val = !txt.is_empty() && txt != "+" && txt != "-";
            let val: f64 = txt.parse().unwrap_or(0.0);
            if p >= n {
                return p;
            }
            let t = d[p];
            p += 1;
            if !(64..=126).contains(&t) {
                return p;
            }
            let term = t.to_ascii_uppercase();
            let last = t.is_ascii_uppercase() || t == b'@' || !(96..=126).contains(&t);
            // binary data?
            let takes_data = (term == b'W' && !(prefix == b'&' && group == b'l'))
                || (prefix == b'*' && group == b'b' && term == b'V')
                || (prefix == b'&' && group == b'p' && term == b'X');
            let mut data: &[u8] = &[];
            if takes_data {
                let len = val.max(0.0) as usize;
                let e = (p + len).min(n);
                data = &d[p..e];
                p = e;
            }
            let key = format!("{}{}{}", prefix as char, if group > 0 { group as char } else { ' ' }, term as char);
            *self.stats.entry(key.trim().replace(' ', "")).or_insert(0) += 1;
            let consumed = self.command(prefix, group, term, val, relative, has_val, data, off, d, p, in_macro);
            if let Some(np) = consumed {
                return np;
            }
            if last {
                return p;
            }
        }
    }

    fn reset(&mut self) {
        self.end_raster();
        if self.marked {
            self.eject(false);
        }
        self.units = 300.0;
        self.primary = FontSel::default();
        self.secondary = FontSel::default();
        self.use_secondary = false;
        self.line_term = 0;
        self.print_dir = 0;
        self.wrap = false;
        self.underline = None;
        self.set_simple_colour(1);
        self.source_transparent = true;
        self.current_pattern = 0;
        self.raster.res = 75.0;
        self.raster.compression = 0;
        self.raster.width = 0;
        self.raster.height = 0;
        self.raster.dest = (None, None);
        self.overlay = None;
        self.update_hmi();
        self.reset_page_setup();
        self.macros.retain(|_, _| true);
    }

    #[allow(clippy::too_many_arguments)]
    fn command(&mut self, prefix: u8, group: u8, term: u8, v: f64, rel: bool, has: bool, data: &[u8], off: usize, d: &[u8], p: usize, in_macro: bool) -> Option<usize> {
        let _ = has;
        let units_to_dp = 720.0 / self.units;
        let desc: Option<String>;
        match (prefix, group, term) {
            // ---------------- page / job control
            (b'&', b'l', b'A') => {
                let code = v as i32;
                if let Some((w, h, name)) = paper_points(code) {
                    if self.marked {
                        self.eject(false);
                    }
                    self.paper_code = code;
                    self.paper = (w, h);
                    self.reset_page_setup();
                    desc = Some(format!("Page size: {name}"));
                } else {
                    desc = Some(format!("Page size code {code}"));
                    if code == 101 {
                        self.paper_code = 101;
                    }
                }
                let name = desc.clone().unwrap_or_default().replace("Page size: ", "");
                let e = self.job.entry("Page sizes".into()).or_default();
                if !e.split(", ").any(|x| x == name) {
                    if !e.is_empty() {
                        e.push_str(", ");
                    }
                    e.push_str(&name);
                }
            }
            (b'&', b'l', b'O') => {
                let o = v as i32;
                if o != self.orientation {
                    if self.marked {
                        self.eject(false);
                    }
                    self.orientation = o.clamp(0, 3);
                    self.reset_page_setup();
                }
                let s = match o {
                    1 => "Landscape",
                    2 => "Reverse portrait",
                    3 => "Reverse landscape",
                    _ => "Portrait",
                };
                desc = Some(format!("Orientation: {s}"));
            }
            (b'&', b'l', b'X') => {
                desc = Some(format!("Copies: {v}"));
                self.set_page_setting("Copies", format!("{v}"));
                self.job.insert("Copies".into(), format!("{v}"));
            }
            (b'&', b'l', b'S') => {
                let s = match v as i32 {
                    1 => "Duplex, long edge",
                    2 => "Duplex, short edge",
                    _ => "Simplex",
                };
                desc = Some(format!("Duplex: {s}"));
                self.set_page_setting("Duplex", s.into());
                self.job.insert("Duplex".into(), s.into());
            }
            (b'&', b'l', b'H') => {
                let s = match v as i32 {
                    0 => "Eject page".to_string(),
                    1 => "Main tray (tray 2)".into(),
                    2 => "Manual feed".into(),
                    3 => "Manual envelope feed".into(),
                    4 => "Alternate tray (tray 3)".into(),
                    5 => "Optional large tray".into(),
                    6 => "Envelope feeder".into(),
                    7 => "Auto select".into(),
                    8 => "Tray 1 (multi-purpose)".into(),
                    n => format!("Tray code {n}"),
                };
                if v as i32 == 0 {
                    self.eject(false);
                }
                desc = Some(format!("Paper source: {s}"));
                if v as i32 != 0 {
                    self.set_page_setting("Source", s.clone());
                    self.job.insert("Paper source".into(), s);
                }
            }
            (b'&', b'l', b'M') => {
                desc = Some(format!("Media type: {v}"));
                self.set_page_setting("Media type", format!("{v}"));
            }
            (b'&', b'l', b'G') => {
                let s = match v as i32 {
                    1 => "Upper (face down)".to_string(),
                    2 => "Rear (face up)".into(),
                    n => format!("Bin {n}"),
                };
                desc = Some(format!("Output bin: {s}"));
                self.set_page_setting("Output", s.clone());
                self.job.insert("Output bin".into(), s);
            }
            (b'&', b'a', b'G') => {
                desc = Some(format!("Duplex page side: {}", match v as i32 { 1 => "front", 2 => "back", _ => "next side" }));
                if self.marked {
                    self.eject(false);
                }
            }
            (b'&', b'l', b'E') => {
                self.top_margin = v * self.vmi;
                let (_, h) = self.phys();
                self.text_length = ((h * 10.0 - self.top_margin - 360.0) / self.vmi).floor().max(1.0);
                desc = Some(format!("Top margin: {v} lines"));
            }
            (b'&', b'l', b'F') => {
                self.text_length = v.max(1.0);
                desc = Some(format!("Text length: {v} lines"));
            }
            (b'&', b'l', b'L') => desc = Some(format!("Perforation skip: {}", if v as i32 == 1 { "on" } else { "off" })),
            (b'&', b'l', b'C') => {
                self.vmi = v * 720.0 / 48.0;
                desc = Some(format!("Line spacing (VMI): {v}/48 inch"));
            }
            (b'&', b'l', b'D') => {
                if v > 0.0 {
                    self.vmi = 720.0 / v;
                }
                desc = Some(format!("Lines per inch: {v}"));
            }
            (b'&', b'l', b'U') | (b'&', b'l', b'Z') => desc = Some(format!("Registration offset: {v} decipoints")),
            (b'&', b'l', b'T') => desc = Some("Job separation".into()),
            (b'&', b'l', b'P') => desc = Some(format!("Page length: {v} lines")),
            (b'&', b'u', b'D') => {
                if v > 0.0 {
                    self.units = v;
                }
                desc = Some(format!("Unit of measure: {v} units per inch"));
            }
            (b'&', b'k', b'G') => {
                self.line_term = v as i32;
                desc = Some(format!("Line termination: mode {v}"));
            }
            (b'&', b'k', b'H') => {
                self.hmi = v * 720.0 / 120.0;
                desc = Some(format!("Character spacing (HMI): {v}/120 inch"));
            }
            (b'&', b'k', b'S') => {
                let pitch = match v as i32 {
                    2 => 16.67,
                    4 => 12.0,
                    _ => 10.0,
                };
                self.primary.pitch = pitch;
                self.update_hmi();
                desc = Some(format!("Pitch mode: {pitch}"));
            }
            (b'&', b's', b'C') => {
                self.wrap = v as i32 == 0;
                desc = Some(format!("End-of-line wrap: {}", if self.wrap { "on" } else { "off" }));
            }
            (b'&', b'a', b'L') => {
                self.left_margin = v * self.hmi;
                desc = Some(format!("Left margin: column {v}"));
            }
            (b'&', b'a', b'M') => {
                self.right_margin = (v + 1.0) * self.hmi;
                desc = Some(format!("Right margin: column {v}"));
            }
            (b'&', b'a', b'P') => {
                self.print_dir = (v as i32).rem_euclid(360);
                desc = Some(format!("Print direction: {v}°"));
            }
            // ---------------- cursor
            (b'&', b'a', b'C') => {
                let x = v * self.hmi;
                self.flush_underline();
                self.x = if rel { self.x + x } else { x };
                desc = Some(format!("Cursor to column {}{v}", if rel { "(relative) " } else { "" }));
            }
            (b'&', b'a', b'R') => {
                let y = v * self.vmi;
                self.y = if rel { self.y + y } else { self.top_margin + y + 0.75 * self.vmi };
                desc = Some(format!("Cursor to row {}{v}", if rel { "(relative) " } else { "" }));
            }
            (b'&', b'a', b'H') => {
                self.flush_underline();
                self.x = if rel { self.x + v } else { v };
                desc = Some(format!("Horizontal position {}{v} decipoints", if rel { "(relative) " } else { "" }));
            }
            (b'&', b'a', b'V') => {
                self.y = if rel { self.y + v } else { self.top_margin + v };
                desc = Some(format!("Vertical position {}{v} decipoints", if rel { "(relative) " } else { "" }));
            }
            (b'*', b'p', b'X') => {
                self.flush_underline();
                self.x = if rel { self.x + v * units_to_dp } else { v * units_to_dp };
                desc = Some(format!("Horizontal position {}{v} units", if rel { "(relative) " } else { "" }));
            }
            (b'*', b'p', b'Y') => {
                self.y = if rel { self.y + v * units_to_dp } else { self.top_margin + v * units_to_dp };
                desc = Some(format!("Vertical position {}{v} units", if rel { "(relative) " } else { "" }));
            }
            (b'&', b'f', b'S') => {
                if v as i32 == 0 {
                    if self.cursor_stack.len() < 20 {
                        self.cursor_stack.push((self.x, self.y));
                    }
                    desc = Some("Push cursor position".into());
                } else {
                    if let Some((x, y)) = self.cursor_stack.pop() {
                        self.x = x;
                        self.y = y;
                    }
                    desc = Some("Pop cursor position".into());
                }
            }
            (b'&', b'd', b'D') => {
                self.underline = Some(self.x);
                desc = Some("Underline on".into());
            }
            (b'&', b'd', b'@') => {
                self.flush_underline();
                self.underline = None;
                desc = Some("Underline off".into());
            }
            (b'&', b'p', b'X') => {
                // transparent print data
                for &b in data {
                    self.print_char(b as u32);
                }
                desc = Some(format!("Transparent print data ({} bytes)", data.len()));
            }
            // ---------------- fonts
            (b'(', 0, t) | (b')', 0, t) => {
                let secondary = prefix == b')';
                if t == b'X' {
                    let s = if secondary { &mut self.secondary } else { &mut self.primary };
                    s.id = Some(v as u32);
                    if let Some(f) = self.soft.get(&(v as u32)) {
                        let (sp, pitch, h, ss) = (f.spacing, f.pitch_qd, f.height_qd, f.symset);
                        let s = if secondary { &mut self.secondary } else { &mut self.primary };
                        s.spacing = sp;
                        s.symset = ss;
                        if pitch > 0.0 && sp == 0 {
                            s.pitch = 300.0 * 4.0 / pitch;
                        }
                        if h > 0.0 && matches!(self.soft.get(&(v as u32)).map(|f| f.format), Some(0) | Some(20)) {
                            let s = if secondary { &mut self.secondary } else { &mut self.primary };
                            s.height = h / 4.0 / 300.0 * 72.0;
                        }
                    }
                    self.update_hmi();
                    desc = Some(format!("Select {} font by ID {v}", if secondary { "secondary" } else { "primary" }));
                } else if t == b'@' {
                    let s = if secondary { &mut self.secondary } else { &mut self.primary };
                    *s = FontSel::default();
                    self.update_hmi();
                    desc = Some("Default font".into());
                } else if t.is_ascii_alphabetic() {
                    let s = if secondary { &mut self.secondary } else { &mut self.primary };
                    s.symset = symset_code(v, t);
                    s.id = None;
                    desc = Some(format!("Symbol set {}", symset_name(s.symset)));
                } else {
                    desc = None;
                }
            }
            (b'(', b's', t) | (b')', b's', t) => {
                let secondary = prefix == b')';
                if t == b'W' {
                    if prefix == b')' {
                        self.define_font(data);
                        desc = Some(format!("Download font header for ID {} ({} bytes)", self.font_id, data.len()));
                    } else {
                        self.define_char(data);
                        desc = Some(format!("Download character {} ({} bytes)", self.char_code, data.len()));
                    }
                } else {
                    let s = if secondary { &mut self.secondary } else { &mut self.primary };
                    let mut clear_id = true;
                    desc = Some(match t {
                        b'P' => {
                            s.spacing = v as i32;
                            format!("Spacing: {}", if v as i32 == 0 { "fixed" } else { "proportional" })
                        }
                        b'H' => {
                            s.pitch = v;
                            if s.id.is_some() {
                                clear_id = false;
                            }
                            format!("Pitch: {v} cpi")
                        }
                        b'V' => {
                            s.height = v;
                            if s.id.is_some() {
                                clear_id = false;
                            }
                            format!("Height: {v} pt")
                        }
                        b'S' => {
                            s.style = v as i32;
                            format!("Style: {v}")
                        }
                        b'B' => {
                            s.weight = v as i32;
                            format!("Stroke weight: {v}")
                        }
                        b'T' => {
                            s.typeface = v as i32;
                            format!("Typeface: {v}")
                        }
                        _ => format!("Font attribute {}", t as char),
                    });
                    if clear_id && t != b'W' {
                        let s = if secondary { &mut self.secondary } else { &mut self.primary };
                        if matches!(t, b'P' | b'S' | b'B' | b'T') {
                            s.id = None;
                        }
                    }
                    self.update_hmi();
                }
            }
            (b'*', b'c', b'D') => {
                self.font_id = v as u32;
                desc = Some(format!("Font ID {v}"));
            }
            (b'*', b'c', b'E') => {
                self.char_code = v as u32;
                desc = Some(format!("Character code {v}"));
            }
            (b'*', b'c', b'F') => {
                match v as i32 {
                    0 => self.soft.clear(),
                    1 => {}
                    2 => {
                        self.soft.remove(&self.font_id);
                    }
                    3 => {
                        let (id, code) = (self.font_id, self.char_code);
                        if let Some(f) = self.soft.get_mut(&id) {
                            match &mut f.kind {
                                SoftKind::Bitmap { chars, .. } => {
                                    chars.remove(&code);
                                }
                                SoftKind::TrueType { chars, .. } => {
                                    chars.remove(&code);
                                }
                                _ => {}
                            }
                        }
                    }
                    _ => {}
                }
                desc = Some(format!("Font control {v}"));
            }
            // ---------------- rectangles / patterns
            (b'*', b'c', b'A') => {
                self.rect_w = v * units_to_dp;
                desc = Some(format!("Rectangle width {v} units"));
            }
            (b'*', b'c', b'B') => {
                self.rect_h = v * units_to_dp;
                desc = Some(format!("Rectangle height {v} units"));
            }
            (b'*', b'c', b'H') => {
                self.rect_w = v;
                desc = Some(format!("Rectangle width {v} decipoints"));
            }
            (b'*', b'c', b'V') => {
                self.rect_h = v;
                desc = Some(format!("Rectangle height {v} decipoints"));
            }
            (b'*', b'c', b'G') => {
                self.pattern_id = v as i32;
                desc = Some(format!("Pattern ID {v}"));
            }
            (b'*', b'c', b'P') => {
                let c = match v as i32 {
                    1 => Rgb(1.0, 1.0, 1.0),
                    2 => {
                        let k = (self.pattern_id as f32 / 100.0).clamp(0.0, 1.0);
                        Rgb(1.0 - (1.0 - self.fg.0) * k, 1.0 - (1.0 - self.fg.1) * k, 1.0 - (1.0 - self.fg.2) * k)
                    }
                    3 => Rgb(1.0 - (1.0 - self.fg.0) * 0.4, 1.0 - (1.0 - self.fg.1) * 0.4, 1.0 - (1.0 - self.fg.2) * 0.4),
                    5 => self.brush_colour(),
                    _ => self.fg,
                };
                let (x, y, w, h) = (self.x, self.y, self.rect_w, self.rect_h);
                if w > 0.0 && h > 0.0 {
                    self.fill_rect_dp(x, y, w, h, c);
                    self.rects += 1;
                }
                desc = Some(format!(
                    "Fill rectangle {:.1} × {:.1} mm, {}",
                    w / 720.0 * 25.4,
                    h / 720.0 * 25.4,
                    match v as i32 {
                        0 => "solid".to_string(),
                        1 => "white".into(),
                        2 => format!("{}% shading", self.pattern_id),
                        3 => format!("cross-hatch {}", self.pattern_id),
                        _ => "pattern".into(),
                    }
                ));
            }
            (b'*', b'c', b'Q') => desc = Some(format!("Pattern control {v}")),
            (b'*', b'c', b'W') => desc = Some(format!("User-defined pattern ({} bytes)", data.len())),
            (b'*', b'p', b'R') => desc = Some(format!("Pattern reference point: {}", if v as i32 == 0 { "rotate with page" } else { "fixed" })),
            (b'*', b'v', b'T') => {
                self.current_pattern = v as i32;
                desc = Some(format!("Current pattern: {}", match v as i32 { 0 => "solid", 1 => "white", 2 => "shading", 3 => "cross-hatch", _ => "user pattern" }));
            }
            (b'*', b'v', b'N') => {
                self.source_transparent = v as i32 == 0;
                desc = Some(format!("Source transparency: {}", if self.source_transparent { "transparent" } else { "opaque" }));
            }
            (b'*', b'v', b'O') => desc = Some(format!("Pattern transparency: {}", if v as i32 == 0 { "transparent" } else { "opaque" })),
            (b'*', b'l', b'O') => desc = Some(format!("Logical operation (ROP3) {v}")),
            (b'*', b'l', b'R') => desc = Some(format!("Pixel placement {v}")),
            // ---------------- colour
            (b'*', b'r', b'U') => {
                self.set_simple_colour(v as i32);
                desc = Some(format!("Simple colour: {}", match v as i32 { 3 => "RGB palette", -3 => "CMY palette", _ => "black and white" }));
            }
            (b'*', b'v', b'W') => {
                self.configure_cid(data);
                desc = Some(format!(
                    "Configure image data: {}",
                    if data.len() >= 6 {
                        format!(
                            "{}, {}, {} bits/index, {}/{}/{} bits",
                            if data[0] == 1 { "CMY" } else { "RGB" },
                            match data[1] {
                                0 => "indexed by plane",
                                1 => "indexed by pixel",
                                2 => "direct by plane",
                                _ => "direct by pixel",
                            },
                            data[2],
                            data[3],
                            data[4],
                            data[5]
                        )
                    } else {
                        format!("{} bytes", data.len())
                    }
                ));
            }
            (b'*', b'v', b'A') | (b'*', b'v', b'B') | (b'*', b'v', b'C') => {
                self.comps[(term - b'A') as usize] = v;
                desc = Some(format!("Colour component {} = {v}", (term - b'A' + 1)));
            }
            (b'*', b'v', b'I') => {
                let i = v.max(0.0) as usize;
                let cmy = matches!(self.pix, PixMode::Cid { cmy: true, .. } | PixMode::SimpleCmy);
                let mut c = Rgb((self.comps[0] / 255.0) as f32, (self.comps[1] / 255.0) as f32, (self.comps[2] / 255.0) as f32).clamp();
                if cmy {
                    c = Rgb(1.0 - c.0, 1.0 - c.1, 1.0 - c.2);
                }
                if i < 4096 {
                    while self.palette.len() <= i {
                        self.palette.push(Rgb::BLACK);
                    }
                    self.palette[i] = c;
                }
                self.comps = [0.0; 3];
                desc = Some(format!("Assign colour index {i}"));
            }
            (b'*', b'v', b'S') => {
                let i = v.max(0.0) as usize;
                self.fg = self.palette.get(i).copied().unwrap_or(Rgb::BLACK);
                desc = Some(format!("Foreground colour: index {i}"));
            }
            (b'*', b'p', b'P') => desc = Some(format!("Palette {}", if v as i32 == 0 { "push" } else { "pop" })),
            (b'&', b'p', b'S') | (b'&', b'p', b'I') | (b'&', b'p', b'C') | (b'*', b't', b'J') | (b'*', b'o', b'W') | (b'*', b'i', b'W') | (b'*', b'l', b'W') | (b'*', b'm', b'W') | (b'&', b'b', b'W') | (b'&', b'n', b'W') | (b'(', b'f', b'W') => {
                desc = Some(format!("{}{}{} {}", prefix as char, group as char, term as char, if data.is_empty() { format!("{v}") } else { format!("({} bytes of data)", data.len()) }));
            }
            // ---------------- raster
            (b'*', b't', b'R') => {
                if v > 0.0 {
                    self.raster.res = v;
                }
                desc = Some(format!("Raster resolution: {v} dpi"));
            }
            (b'*', b'r', b'S') => {
                self.raster.width = v.max(0.0) as u32;
                desc = Some(format!("Raster width: {v} pixels"));
            }
            (b'*', b'r', b'T') => {
                self.raster.height = v.max(0.0) as u32;
                desc = Some(format!("Raster height: {v} rows"));
            }
            (b'*', b't', b'H') => {
                self.raster.dest.0 = Some(v);
                desc = Some(format!("Raster destination width: {v} decipoints"));
            }
            (b'*', b't', b'V') => {
                self.raster.dest.1 = Some(v);
                desc = Some(format!("Raster destination height: {v} decipoints"));
            }
            (b'*', b'r', b'A') => {
                self.start_raster(v as i32);
                desc = Some(format!("Start raster graphics ({})", if v as i32 & 1 == 1 { "at cursor" } else { "at left edge" }));
            }
            (b'*', b'r', b'B') | (b'*', b'r', b'C') => {
                self.end_raster();
                desc = Some("End raster graphics".into());
            }
            (b'*', b'r', b'F') => desc = Some(format!("Raster presentation {v}")),
            (b'*', b'b', b'M') => {
                self.raster.compression = v as i32;
                desc = Some(format!(
                    "Raster compression: {}",
                    match v as i32 {
                        0 => "none",
                        1 => "run-length",
                        2 => "TIFF PackBits",
                        3 => "delta row",
                        5 => "adaptive",
                        9 => "replacement delta row",
                        _ => "other",
                    }
                ));
            }
            (b'*', b'b', b'Y') => {
                self.skip_rows(v);
                desc = Some(format!("Raster Y offset: {v} rows"));
            }
            (b'*', b'b', b'V') => {
                self.raster_plane(data, false);
                if in_macro || self.doc.commands.len() > MAX_COMMAND_LINES / 2 {
                    return None;
                }
                desc = Some(format!("Raster plane ({} bytes)", data.len()));
            }
            (b'*', b'b', b'W') => {
                let planes = self.planes_per_row();
                let last = self.raster.planes.len() + 1 >= planes || self.raster.compression == 5;
                self.raster_plane(data, last);
                if in_macro || self.doc.commands.len() > MAX_COMMAND_LINES / 2 {
                    return None;
                }
                desc = Some(format!("Raster row ({} bytes)", data.len()));
            }
            // ---------------- macros
            (b'&', b'f', b'Y') => {
                self.macro_id = v as u32;
                desc = Some(format!("Macro ID {v}"));
            }
            (b'&', b'f', b'X') => {
                match v as i32 {
                    0 => {
                        // record until ESC&f1X
                        let rest = &d[p..];
                        let end = find_macro_end(rest);
                        let body = rest[..end.0].to_vec();
                        self.log(off, format!("Start macro {} definition ({} bytes)", self.macro_id, body.len()));
                        self.macros.insert(self.macro_id, body);
                        return Some(p + end.1);
                    }
                    2 | 3 => {
                        if let Some(m) = self.macros.get(&self.macro_id).cloned() {
                            if self.depth < 3 {
                                self.log(off, format!("{} macro {}", if v as i32 == 2 { "Execute" } else { "Call" }, self.macro_id));
                                let saved = if v as i32 == 3 { Some((self.x, self.y)) } else { None };
                                self.depth += 1;
                                self.run_inner(&m, usize::MAX);
                                self.depth -= 1;
                                if let Some((x, y)) = saved {
                                    let _ = (x, y);
                                }
                                return Some(p);
                            }
                        } else {
                            self.doc.warn(format!("Macro {} is used but was never defined", self.macro_id));
                        }
                        desc = Some(format!("Run macro {}", self.macro_id));
                    }
                    4 => {
                        self.overlay = Some(self.macro_id);
                        desc = Some(format!("Enable overlay macro {}", self.macro_id));
                    }
                    5 => {
                        self.overlay = None;
                        desc = Some("Disable overlay".into());
                    }
                    6 => {
                        self.macros.clear();
                        desc = Some("Delete all macros".into());
                    }
                    8 => {
                        self.macros.remove(&self.macro_id);
                        desc = Some(format!("Delete macro {}", self.macro_id));
                    }
                    n => desc = Some(format!("Macro control {n}")),
                }
            }
            // ---------------- HP-GL/2
            (b'%', 0, b'B') => {
                self.log(off, format!("Enter HP-GL/2 (mode {v})"));
                let end = self.enter_hpgl(v as i32, d, p);
                return Some(end);
            }
            (b'%', 0, b'A') => desc = Some(format!("Return to PCL (mode {v})")),
            (b'*', b'c', b'X') => {
                self.frame_size.0 = Some(v);
                desc = Some(format!("Picture frame width: {v} decipoints"));
            }
            (b'*', b'c', b'Y') => {
                self.frame_size.1 = Some(v);
                desc = Some(format!("Picture frame height: {v} decipoints"));
            }
            (b'*', b'c', b'T') => {
                self.frame_anchor = Some((self.x, self.y));
                desc = Some("Picture frame anchor at cursor".into());
            }
            (b'*', b'c', b'K') | (b'*', b'c', b'L') => desc = Some(format!("HP-GL/2 plot size {v} inches")),
            (b'%', 0, b'X') => desc = Some("Universal exit".into()),
            _ => {
                desc = Some(format!(
                    "{}{}{}{}  (not interpreted)",
                    prefix as char,
                    if group > 0 { (group as char).to_string() } else { String::new() },
                    if data.is_empty() { format!("{v}") } else { format!("{} bytes", data.len()) },
                    term as char
                ));
            }
        }
        if !in_macro {
            if let Some(s) = desc {
                self.log(off, s);
            }
        }
        None
    }

    fn set_page_setting(&mut self, k: &str, v: String) {
        self.page_settings.retain(|(a, _)| a != k);
        self.page_settings.push((k.to_string(), v));
    }

    fn finish_info(&mut self) {
        let mut s = InfoSection::new("PCL 5 job");
        for (k, v) in &self.job {
            s.add(k.clone(), v.clone());
        }
        s.add("Raster images", self.images.to_string());
        s.add("Filled rectangles", self.rects.to_string());
        if !self.macros.is_empty() {
            s.add("Macros defined", self.macros.keys().map(|k| k.to_string()).collect::<Vec<_>>().join(", "));
        }
        if self.hpgl.used {
            s.add("HP-GL/2 vector graphics", "Yes");
        }
        self.doc.info.push(s);
        let mut f = InfoSection::new("Fonts (PCL 5)");
        for (id, sf) in &self.soft {
            let kind = match &sf.kind {
                SoftKind::Bitmap { chars, res } => format!("bitmap, {} chars at {res} dpi", chars.len()),
                SoftKind::TrueType { chars, .. } => format!("TrueType, {} glyphs", chars.len()),
                SoftKind::Unsupported => format!("format {} (not supported)", sf.format),
            };
            f.add(
                format!("Soft font {id}"),
                format!(
                    "{}{} — {}, {}, symbol set {}, typeface {}{}",
                    if sf.name.is_empty() { "(no name)" } else { &sf.name },
                    if sf.used { "" } else { " (not used)" },
                    kind,
                    if sf.spacing == 0 { format!("fixed {:.2} cpi", 1200.0 / sf.pitch_qd.max(1.0)) } else { format!("proportional{}", if sf.height_qd > 0.0 { format!(" {:.1} pt", sf.height_qd / 4.0 / 300.0 * 72.0) } else { String::new() }) },
                    symset_name(sf.symset),
                    sf.typeface,
                    sf.ident.as_ref().map(|i| format!(" — glyphs match installed font \"{}\"", i.name)).unwrap_or_default()
                ),
            );
        }
        for (k, v) in &self.fonts_used {
            f.add(format!("Used: {k}"), format!("{v} character(s)"));
        }
        if !f.rows.is_empty() {
            self.doc.info.push(f);
        }
        let mut st = InfoSection::new("Command statistics (PCL 5)");
        let mut v: Vec<(&String, &usize)> = self.stats.iter().collect();
        v.sort_by(|a, b| b.1.cmp(a.1));
        for (k, n) in v {
            st.add(format!("ESC{k}"), n.to_string());
        }
        self.doc.info.push(st);
    }
}

/// Find the end of a macro definition: returns (body length, bytes to skip incl. the stop command).
fn find_macro_end(d: &[u8]) -> (usize, usize) {
    let mut i = 0;
    while i + 4 < d.len() {
        if d[i] == 0x1B && d[i + 1] == b'&' && d[i + 2] == b'f' {
            // parse the value/terminator group for "1X"
            let mut j = i + 3;
            loop {
                let s = j;
                while j < d.len() && (d[j].is_ascii_digit() || d[j] == b'+' || d[j] == b'-' || d[j] == b'.') {
                    j += 1;
                }
                if j >= d.len() {
                    break;
                }
                let val = std::str::from_utf8(&d[s..j]).unwrap_or("").parse::<i32>().unwrap_or(-1);
                let t = d[j];
                j += 1;
                if (t == b'X' || t == b'x') && val == 1 {
                    return (i, j);
                }
                if t.is_ascii_uppercase() {
                    break;
                }
            }
        }
        i += 1;
    }
    (d.len(), d.len())
}

/// Compression method 9 (replacement delta row).
fn replacement_delta(d: &[u8], seed: &mut Vec<u8>) {
    let mut p = 0;
    let mut pos = 0usize;
    while p < d.len() {
        let c = d[p];
        p += 1;
        let rle = c & 0x80 != 0;
        let (mut off, mut cnt, off_max, cnt_max) = if rle { (((c >> 5) & 3) as usize, (c & 0x1F) as usize, 3, 31) } else { (((c >> 3) & 0x0F) as usize, (c & 7) as usize, 15, 7) };
        if off == off_max {
            while p < d.len() {
                let b = d[p] as usize;
                p += 1;
                off += b;
                if b != 255 {
                    break;
                }
            }
        }
        if cnt == cnt_max {
            while p < d.len() {
                let b = d[p] as usize;
                p += 1;
                cnt += b;
                if b != 255 {
                    break;
                }
            }
        }
        pos += off;
        if !rle {
            let n = cnt + 1;
            for _ in 0..n {
                if p >= d.len() {
                    break;
                }
                if pos >= seed.len() {
                    seed.resize(pos + 1, 0);
                }
                seed[pos] = d[p];
                pos += 1;
                p += 1;
            }
        } else {
            let n = cnt + 2;
            if p >= d.len() {
                break;
            }
            let b = d[p];
            p += 1;
            for _ in 0..n {
                if pos >= seed.len() {
                    seed.resize(pos + 1, 0);
                }
                seed[pos] = b;
                pos += 1;
            }
        }
    }
}
