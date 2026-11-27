//! HP-GL/2 vector graphics, either embedded in PCL 5 (ESC%#B … ESC%#A) or as a
//! stand-alone plot file.

use crate::doc::*;
use crate::fonts::{self, SysFont};
use crate::geom::*;
use std::sync::Arc;
use tiny_skia::{LineCap, LineJoin, Transform};

/// Plotter units per inch.
const PLU: f64 = 1016.0;

#[derive(Clone, Copy)]
pub struct Frame {
    /// Top-left of the picture frame on the page, in points.
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

#[derive(Clone, Copy, PartialEq)]
enum Scale {
    Anisotropic(f64, f64, f64, f64),
    Isotropic(f64, f64, f64, f64),
    Factor(f64, f64, f64, f64),
}

pub struct Hpgl {
    pub frame: Frame,
    pos: (f64, f64), // plotter units, in the current (rotated) system
    pen_down: bool,
    absolute: bool,
    pen: usize,
    pub pens: Vec<Rgb>,
    widths: Vec<f64>, // mm or relative %
    width_relative: bool,
    p1: (f64, f64),
    p2: (f64, f64),
    scale: Option<Scale>,
    rotation: i32,
    line_type: Option<(i32, f64)>,
    cap: LineCap,
    join: LineJoin,
    miter: f64,
    fill_type: i32,
    fill_level: f64,
    polygon: Option<PathAcc>,
    poly_buffer: Option<PathAcc>,
    poly_pen_up: bool,
    window: Option<[f64; 4]>,
    // labels
    label_term: u8,
    label_term_print: bool,
    label_origin: i32,
    direction: (f64, f64),
    char_size: Option<(f64, f64)>, // cm (width, height)
    slant: f64,
    font_height: f64, // points
    font_pitch: f64,
    font_spacing: i32,
    font_typeface: i32,
    font_bold: bool,
    font_italic: bool,
    font_symset: i32,
    line: PathAcc, // pending pen-down strokes, page points
    line_key: Option<(usize, u64, Option<(i32, u64)>)>,
    sys_font: Option<(String, Arc<SysFont>)>,
    pub plot_size: Option<(f64, f64)>,
    pub used: bool,
}

fn fmt_args(a: &[f64]) -> String {
    a.iter().take(12).map(|v| if v.fract() == 0.0 { format!("{}", *v as i64) } else { format!("{v:.2}") }).collect::<Vec<_>>().join(",") + if a.len() > 12 { ",…" } else { "" }
}

pub fn default_pens() -> Vec<Rgb> {
    vec![
        Rgb(1.0, 1.0, 1.0),
        Rgb(0.0, 0.0, 0.0),
        Rgb(1.0, 0.0, 0.0),
        Rgb(0.0, 1.0, 0.0),
        Rgb(1.0, 1.0, 0.0),
        Rgb(0.0, 0.0, 1.0),
        Rgb(1.0, 0.0, 1.0),
        Rgb(0.0, 1.0, 1.0),
    ]
}

impl Hpgl {
    pub fn new(frame: Frame) -> Hpgl {
        let mut h = Hpgl {
            frame,
            pos: (0.0, 0.0),
            pen_down: false,
            absolute: true,
            pen: 1,
            pens: default_pens(),
            widths: vec![0.35; 8],
            width_relative: false,
            p1: (0.0, 0.0),
            p2: (0.0, 0.0),
            scale: None,
            rotation: 0,
            line_type: None,
            cap: LineCap::Butt,
            join: LineJoin::Miter,
            miter: 5.0,
            fill_type: 1,
            fill_level: 100.0,
            polygon: None,
            poly_buffer: None,
            poly_pen_up: false,
            window: None,
            label_term: 3,
            label_term_print: true,
            label_origin: 1,
            direction: (1.0, 0.0),
            char_size: None,
            slant: 0.0,
            font_height: 11.5,
            font_pitch: 9.0,
            font_spacing: 0,
            font_typeface: 48,
            font_bold: false,
            font_italic: false,
            font_symset: 277,
            line: PathAcc::new(),
            line_key: None,
            sys_font: None,
            plot_size: None,
            used: false,
        };
        h.default_p1p2();
        h
    }

    fn frame_plu(&self) -> (f64, f64) {
        let w = self.frame.w / 72.0 * PLU;
        let hh = self.frame.h / 72.0 * PLU;
        if self.rotation % 180 == 0 { (w, hh) } else { (hh, w) }
    }

    fn default_p1p2(&mut self) {
        let (w, h) = self.frame_plu();
        self.p1 = (0.0, 0.0);
        self.p2 = (w, h);
    }

    /// IN: initialise everything except the picture frame.
    fn init(&mut self) {
        let f = self.frame;
        let pens = std::mem::take(&mut self.pens);
        *self = Hpgl::new(f);
        self.pens = if pens.len() >= 2 { pens } else { default_pens() };
        self.used = true;
    }

    // ---- coordinate mapping

    fn user_to_plu(&self, x: f64, y: f64) -> (f64, f64) {
        match self.scale {
            None => (x, y),
            Some(Scale::Anisotropic(xmin, xmax, ymin, ymax)) => {
                let sx = (self.p2.0 - self.p1.0) / (xmax - xmin).max(1e-9);
                let sy = (self.p2.1 - self.p1.1) / (ymax - ymin).max(1e-9);
                (self.p1.0 + (x - xmin) * sx, self.p1.1 + (y - ymin) * sy)
            }
            Some(Scale::Isotropic(xmin, xmax, ymin, ymax)) => {
                let sx = (self.p2.0 - self.p1.0) / (xmax - xmin).max(1e-9);
                let sy = (self.p2.1 - self.p1.1) / (ymax - ymin).max(1e-9);
                let k = sx.abs().min(sy.abs());
                (self.p1.0 + (x - xmin) * k * sx.signum(), self.p1.1 + (y - ymin) * k * sy.signum())
            }
            Some(Scale::Factor(xmin, xf, ymin, yf)) => (self.p1.0 + (x - xmin) * xf, self.p1.1 + (y - ymin) * yf),
        }
    }

    /// Size of one user unit in plotter units (x, y), for relative moves.
    fn user_delta(&self, dx: f64, dy: f64) -> (f64, f64) {
        let (a, b) = self.user_to_plu(0.0, 0.0);
        let (c, d) = self.user_to_plu(dx, dy);
        (c - a, d - b)
    }

    /// Plotter units (current rotated system) to page points.
    fn to_page(&self, x: f64, y: f64) -> (f64, f64) {
        let (fw, fh) = (self.frame.w / 72.0 * PLU, self.frame.h / 72.0 * PLU);
        let (px, py) = match self.rotation {
            90 => (fw - y, x),
            180 => (fw - x, fh - y),
            270 => (y, fh - x),
            _ => (x, y),
        };
        (self.frame.x + px * 72.0 / PLU, self.frame.y + self.frame.h - py * 72.0 / PLU)
    }

    fn plu_per_mm(&self) -> f64 {
        PLU / 25.4
    }

    fn pen_width_pt(&self) -> f64 {
        let w = self.widths.get(self.pen).copied().unwrap_or(0.35);
        if self.width_relative {
            let d = ((self.p2.0 - self.p1.0).powi(2) + (self.p2.1 - self.p1.1).powi(2)).sqrt();
            (w / 100.0 * d) * 72.0 / PLU
        } else {
            w / 25.4 * 72.0
        }
    }

    fn pen_colour(&self) -> Rgb {
        self.pens.get(self.pen).copied().unwrap_or(Rgb::BLACK)
    }

    fn fill_colour(&self) -> Rgb {
        let c = self.pen_colour();
        if self.fill_type == 10 {
            let k = (self.fill_level / 100.0).clamp(0.0, 1.0) as f32;
            Rgb(1.0 - (1.0 - c.0) * k, 1.0 - (1.0 - c.1) * k, 1.0 - (1.0 - c.2) * k)
        } else if self.fill_type == 3 || self.fill_type == 4 {
            // hatching: show as a light tint of the pen
            Rgb(1.0 - (1.0 - c.0) * 0.35, 1.0 - (1.0 - c.1) * 0.35, 1.0 - (1.0 - c.2) * 0.35)
        } else {
            c
        }
    }

    fn dash(&self) -> Option<(Vec<f32>, f32)> {
        let (t, len_pct) = self.line_type?;
        if t == 0 {
            return None;
        }
        let pat: &[f64] = match t.abs() {
            1 => &[1.0, 99.0],
            2 => &[50.0, 50.0],
            3 => &[70.0, 30.0],
            4 => &[80.0, 10.0, 1.0, 9.0],
            5 => &[70.0, 10.0, 10.0, 10.0],
            6 => &[50.0, 10.0, 10.0, 10.0, 10.0, 10.0],
            7 => &[70.0, 10.0, 1.0, 9.0, 1.0, 9.0],
            8 => &[50.0, 10.0, 1.0, 9.0, 10.0, 10.0, 1.0, 9.0],
            _ => &[50.0, 50.0],
        };
        let d = ((self.p2.0 - self.p1.0).powi(2) + (self.p2.1 - self.p1.1).powi(2)).sqrt() * 72.0 / PLU;
        let len = (d * len_pct / 100.0).max(1.0);
        Some((pat.iter().map(|p| (p / 100.0 * len) as f32).collect(), 0.0))
    }

    // ---- output

    fn flush(&mut self, page: &mut Page) {
        if self.line.is_empty() {
            return;
        }
        let acc = std::mem::take(&mut self.line);
        self.line_key = None;
        if let Some(path) = acc.to_path(&Matrix::identity()) {
            let style = StrokeStyle { width: self.pen_width_pt().max(0.1) as f32, cap: self.cap, join: self.join, miter: self.miter as f32, dash: self.dash() };
            let clip = self.clip();
            page.push(Item::Stroke { path, transform: Transform::identity(), brush: Brush::Solid(self.pen_colour()), style, clip });
        }
    }

    fn clip(&self) -> Clip {
        let w = self.window?;
        let mut acc = PathAcc::new();
        let a = self.to_page(w[0], w[1]);
        let b = self.to_page(w[2], w[1]);
        let c = self.to_page(w[2], w[3]);
        let d = self.to_page(w[0], w[3]);
        acc.move_to(a.0, a.1);
        acc.line_to(b.0, b.1);
        acc.line_to(c.0, c.1);
        acc.line_to(d.0, d.1);
        acc.close();
        let path = acc.to_path(&Matrix::identity())?;
        Some(Arc::new(vec![ClipPath { path, even_odd: false }]))
    }

    fn line_to(&mut self, page: &mut Page, to: (f64, f64)) {
        if let Some(poly) = self.polygon.as_mut() {
            let p = (to.0, to.1);
            if self.pen_down && !self.poly_pen_up {
                if poly.cur.is_none() {
                    poly.move_to(self.pos.0, self.pos.1);
                }
                poly.line_to(p.0, p.1);
            } else {
                poly.move_to(p.0, p.1);
            }
            self.pos = to;
            return;
        }
        if self.pen_down {
            let key = (self.pen, self.pen_width_pt().to_bits(), self.line_type.map(|(a, b)| (a, b.to_bits())));
            if self.line_key.is_some() && self.line_key != Some(key) {
                self.flush(page);
            }
            self.line_key = Some(key);
            let a = self.to_page(self.pos.0, self.pos.1);
            let b = self.to_page(to.0, to.1);
            if self.line.cur.map(|c| (c.0 - a.0).abs() > 1e-6 || (c.1 - a.1).abs() > 1e-6).unwrap_or(true) {
                self.line.move_to(a.0, a.1);
            }
            if (a.0 - b.0).abs() < 1e-9 && (a.1 - b.1).abs() < 1e-9 {
                // a dot
                self.line.line_to(b.0 + 0.01, b.1);
            } else {
                self.line.line_to(b.0, b.1);
            }
            if self.line.segs.len() > 4000 {
                self.flush(page);
            }
        }
        self.pos = to;
    }

    fn fill_acc(&mut self, page: &mut Page, acc: &PathAcc, even_odd: bool) {
        self.flush(page);
        let mut dev = PathAcc::new();
        for s in &acc.segs {
            match *s {
                Seg::Move(x, y) => {
                    let p = self.to_page(x, y);
                    dev.move_to(p.0, p.1);
                }
                Seg::Line(x, y) => {
                    let p = self.to_page(x, y);
                    dev.line_to(p.0, p.1);
                }
                Seg::Cubic(a, b, c, d, e, f) => {
                    let p1 = self.to_page(a, b);
                    let p2 = self.to_page(c, d);
                    let p3 = self.to_page(e, f);
                    dev.curve_to(p1.0, p1.1, p2.0, p2.1, p3.0, p3.1);
                }
                Seg::Close => dev.close(),
            }
        }
        if let Some(path) = dev.to_path(&Matrix::identity()) {
            let clip = self.clip();
            page.push(Item::Fill { path, transform: Transform::identity(), brush: Brush::Solid(self.fill_colour()), even_odd, clip });
        }
    }

    fn edge_acc(&mut self, page: &mut Page, acc: &PathAcc) {
        self.flush(page);
        let mut dev = PathAcc::new();
        for s in &acc.segs {
            match *s {
                Seg::Move(x, y) => {
                    let p = self.to_page(x, y);
                    dev.move_to(p.0, p.1);
                }
                Seg::Line(x, y) => {
                    let p = self.to_page(x, y);
                    dev.line_to(p.0, p.1);
                }
                Seg::Cubic(a, b, c, d, e, f) => {
                    let p1 = self.to_page(a, b);
                    let p2 = self.to_page(c, d);
                    let p3 = self.to_page(e, f);
                    dev.curve_to(p1.0, p1.1, p2.0, p2.1, p3.0, p3.1);
                }
                Seg::Close => dev.close(),
            }
        }
        if let Some(path) = dev.to_path(&Matrix::identity()) {
            let style = StrokeStyle { width: self.pen_width_pt().max(0.1) as f32, cap: self.cap, join: self.join, miter: self.miter as f32, dash: self.dash() };
            let clip = self.clip();
            page.push(Item::Stroke { path, transform: Transform::identity(), brush: Brush::Solid(self.pen_colour()), style, clip });
        }
    }

    fn rect_acc(&self, a: (f64, f64), b: (f64, f64)) -> PathAcc {
        let mut acc = PathAcc::new();
        acc.move_to(a.0, a.1);
        acc.line_to(b.0, a.1);
        acc.line_to(b.0, b.1);
        acc.line_to(a.0, b.1);
        acc.close();
        acc
    }

    fn arc_acc(&self, center: (f64, f64), start: (f64, f64), sweep: f64, close_wedge: bool) -> PathAcc {
        let r = ((start.0 - center.0).powi(2) + (start.1 - center.1).powi(2)).sqrt();
        let a1 = (start.1 - center.1).atan2(start.0 - center.0).to_degrees();
        let mut acc = PathAcc::new();
        if close_wedge {
            acc.move_to(center.0, center.1);
            acc.line_to(start.0, start.1);
        }
        acc.arc(&Matrix::identity(), center.0, center.1, r, a1, a1 + sweep, sweep >= 0.0, close_wedge);
        if close_wedge {
            acc.close();
        }
        acc
    }

    // ---- text

    fn font(&mut self) -> Arc<SysFont> {
        let mono = self.font_spacing == 0;
        let name = format!(
            "{}{}{}",
            match self.font_typeface {
                3 | 4099 | 0 => "Courier",
                5 | 4101 | 16901 | 25093 => "Times",
                _ if mono => "Courier",
                _ => "Arial",
            },
            if self.font_bold { " Bold" } else { "" },
            if self.font_italic { " Italic" } else { "" }
        );
        if let Some((n, f)) = &self.sys_font {
            if *n == name {
                return f.clone();
            }
        }
        let f = fonts::system_font(&name);
        self.sys_font = Some((name, f.clone()));
        f
    }

    fn label(&mut self, page: &mut Page, text: &[u8]) {
        self.flush(page);
        let f = self.font();
        // character size in plotter units
        let (cw, ch) = match self.char_size {
            Some((w, h)) => (w * 10.0 * self.plu_per_mm(), h * 10.0 * self.plu_per_mm()),
            None => {
                let h = self.font_height / 72.0 * PLU;
                let w = if self.font_spacing == 0 { PLU / self.font_pitch.max(0.1) } else { h * 0.55 };
                (w, h)
            }
        };
        let size = ch / 0.7; // cap height ≈ 0.7 em
        let ang = self.direction.1.atan2(self.direction.0);
        let (ca, sa) = (ang.cos(), ang.sin());
        // width of the label for origin adjustment
        let adv_of = |c: char| -> f64 {
            if self.font_spacing == 0 || self.char_size.is_some() {
                cw * 1.5 / 1.5
            } else {
                f.glyph_for_char(c).map(|g| f.advance(g) / f.upem * size).unwrap_or(size * 0.5)
            }
        };
        let chars: Vec<char> = text.iter().map(|&b| fonts::roman8(b)).collect();
        let lines: Vec<&[char]> = chars.split(|c| *c == '\n').collect();
        let width: f64 = lines.first().map(|l| l.iter().filter(|c| **c != '\r').map(|c| adv_of(*c)).sum()).unwrap_or(0.0);
        let lo = self.label_origin % 10;
        let (ox, oy) = match lo {
            1 => (0.0, 0.0),
            2 => (0.0, -ch / 2.0),
            3 => (0.0, -ch),
            4 => (-width / 2.0, 0.0),
            5 => (-width / 2.0, -ch / 2.0),
            6 => (-width / 2.0, -ch),
            7 => (-width, 0.0),
            8 => (-width, -ch / 2.0),
            9 => (-width, -ch),
            _ => (0.0, 0.0),
        };
        let start = self.pos;
        let mut x = start.0 + ox * ca - oy * sa;
        let mut y = start.1 + ox * sa + oy * ca;
        let line_start = (x, y);
        let mut lineno = 0.0;
        let mut out = PathAcc::new();
        let prev_kind = page.cur_kind;
        for c in chars {
            match c {
                '\r' => {
                    x = line_start.0 + lineno * ch * 1.6 * sa;
                    y = line_start.1 - lineno * ch * 1.6 * ca;
                    continue;
                }
                '\n' => {
                    lineno += 1.0;
                    x = line_start.0 + lineno * ch * 1.6 * sa;
                    y = line_start.1 - lineno * ch * 1.6 * ca;
                    page.text_break();
                    continue;
                }
                _ => {}
            }
            let adv = adv_of(c);
            // glyph matrix: font units -> plu (y up) -> page
            let s = size / f.upem;
            let m = Matrix::new(s * ca, s * sa, s * (-sa + self.slant * ca), s * (ca + self.slant * sa), x, y);
            let mut g = PathAcc::new();
            if let Some(gid) = f.glyph_for_char(c) {
                f.outline(gid, &m, &mut g);
            }
            out.append(&g);
            let a = self.to_page(x, y);
            let nx = x + adv * ca;
            let ny = y + adv * sa;
            let b = self.to_page(nx, ny);
            let up_a = self.to_page(x - size * sa, y + size * ca);
            page.text_char(c, a, b, (up_a.0 - a.0, up_a.1 - a.1));
            x = nx;
            y = ny;
        }
        page.text_break();
        if !out.is_empty() {
            page.cur_kind = Kind::Text;
            let fill = self.fill_type;
            self.fill_type = 1;
            self.fill_acc(page, &out, false);
            self.fill_type = fill;
            page.cur_kind = prev_kind;
        }
        // the pen ends after the label
        self.pos = (x, y);
    }

    // ---- parsing

    /// Interpret HP-GL/2 starting at `start`; stops at an ESC (returns its
    /// position) or the end of the data.
    pub fn run(&mut self, d: &[u8], start: usize, base: usize, page: &mut Page, doc: &mut Document) -> usize {
        self.used = true;
        let mut p = start;
        let n = d.len();
        while p < n {
            let c = d[p];
            if c == 0x1B {
                break;
            }
            if !c.is_ascii_alphabetic() {
                p += 1;
                continue;
            }
            if p + 1 >= n || !d[p + 1].is_ascii_alphabetic() {
                p += 1;
                continue;
            }
            let op = [c.to_ascii_uppercase(), d[p + 1].to_ascii_uppercase()];
            let op_off = p;
            p += 2;
            page.cur_src = (base + op_off) as u32;
            // special-syntax instructions
            match &op {
                b"LB" => {
                    let s = p;
                    while p < n && d[p] != self.label_term && d[p] != 0x1B {
                        p += 1;
                    }
                    let text = d[s..p].to_vec();
                    if p < n && d[p] == self.label_term {
                        p += 1;
                    }
                    doc.cmd(base + op_off, format!("{:08X}  HP-GL/2 LB  \"{}\"", base + op_off, String::from_utf8_lossy(&text).chars().take(80).collect::<String>()));
                    self.label(page, &text);
                    continue;
                }
                b"DT" => {
                    // DT t[,mode];
                    if p < n && d[p] != b';' {
                        self.label_term = d[p];
                        p += 1;
                        if p < n && d[p] == b',' {
                            p += 1;
                            self.label_term_print = d.get(p).map(|c| *c == b'0').unwrap_or(true);
                            while p < n && d[p] != b';' && d[p] != 0x1B {
                                p += 1;
                            }
                        }
                    } else {
                        self.label_term = 3;
                    }
                    if p < n && d[p] == b';' {
                        p += 1;
                    }
                    continue;
                }
                b"PE" => {
                    let s = p;
                    while p < n && d[p] != b';' && d[p] != 0x1B {
                        p += 1;
                    }
                    let body = d[s..p].to_vec();
                    if p < n && d[p] == b';' {
                        p += 1;
                    }
                    doc.cmd(base + op_off, format!("{:08X}  HP-GL/2 PE  (encoded polyline, {} bytes)", base + op_off, body.len()));
                    self.polyline_encoded(page, &body);
                    continue;
                }
                b"CO" | b"BP" | b"WD" => {
                    // strings in quotes
                    let mut q = false;
                    while p < n && d[p] != 0x1B {
                        if d[p] == b'"' {
                            q = !q;
                        } else if d[p] == b';' && !q {
                            break;
                        }
                        p += 1;
                    }
                    if p < n && d[p] == b';' {
                        p += 1;
                    }
                    continue;
                }
                _ => {}
            }
            // numeric parameters
            let mut args: Vec<f64> = Vec::new();
            loop {
                while p < n && (d[p] == b' ' || d[p] == b',' || d[p] == b'\r' || d[p] == b'\n' || d[p] == b'\t') {
                    p += 1;
                }
                if p >= n {
                    break;
                }
                let c = d[p];
                if c == b';' {
                    p += 1;
                    break;
                }
                if c.is_ascii_digit() || c == b'-' || c == b'+' || c == b'.' {
                    let s = p;
                    p += 1;
                    while p < n && (d[p].is_ascii_digit() || d[p] == b'.' || d[p] == b'e' && false) {
                        p += 1;
                    }
                    if let Ok(v) = std::str::from_utf8(&d[s..p]).unwrap_or("0").parse::<f64>() {
                        args.push(v);
                    }
                    continue;
                }
                break;
            }
            if doc.commands.len() < MAX_COMMAND_LINES && !matches!(&op, b"PD" | b"PU" | b"PA" | b"PR") || args.len() > 0 && doc.commands.len() < MAX_COMMAND_LINES {
                doc.cmd(base + op_off, format!("{:08X}  HP-GL/2 {}{}  {}", base + op_off, op[0] as char, op[1] as char, fmt_args(&args)));
            }
            self.exec(&op, &args, page, doc);
        }
        self.flush(page);
        p
    }

    fn move_points(&mut self, page: &mut Page, args: &[f64]) {
        for pair in args.chunks(2) {
            if pair.len() < 2 {
                break;
            }
            let to = if self.absolute {
                self.user_to_plu(pair[0], pair[1])
            } else {
                let (dx, dy) = self.user_delta(pair[0], pair[1]);
                (self.pos.0 + dx, self.pos.1 + dy)
            };
            self.line_to(page, to);
        }
    }

    fn exec(&mut self, op: &[u8; 2], a: &[f64], page: &mut Page, doc: &mut Document) {
        let g = |i: usize, def: f64| a.get(i).copied().unwrap_or(def);
        match op {
            b"IN" => {
                self.flush(page);
                self.init();
            }
            b"DF" => {
                self.flush(page);
                self.absolute = true;
                self.line_type = None;
                self.fill_type = 1;
                self.scale = None;
                self.window = None;
                self.label_term = 3;
                self.polygon = None;
            }
            b"PU" => {
                if self.polygon.is_some() {
                    self.poly_pen_up = true;
                }
                self.pen_down = false;
                self.flush(page);
                self.move_points(page, a);
            }
            b"PD" => {
                self.pen_down = true;
                self.poly_pen_up = false;
                self.move_points(page, a);
            }
            b"PA" => {
                self.absolute = true;
                self.move_points(page, a);
            }
            b"PR" => {
                self.absolute = false;
                self.move_points(page, a);
            }
            b"SP" => {
                self.flush(page);
                self.pen = g(0, 0.0).max(0.0) as usize;
            }
            b"NP" => {
                let k = g(0, 8.0).max(2.0) as usize;
                while self.pens.len() < k {
                    self.pens.push(Rgb::BLACK);
                }
                while self.widths.len() < k {
                    self.widths.push(0.35);
                }
            }
            b"PC" => {
                self.flush(page);
                if a.is_empty() {
                    self.pens = default_pens();
                } else {
                    let i = g(0, 0.0) as usize;
                    while self.pens.len() <= i {
                        self.pens.push(Rgb::BLACK);
                    }
                    if a.len() >= 4 {
                        self.pens[i] = Rgb((g(1, 0.0) / 255.0) as f32, (g(2, 0.0) / 255.0) as f32, (g(3, 0.0) / 255.0) as f32).clamp();
                    } else if let Some(c) = default_pens().get(i) {
                        self.pens[i] = *c;
                    }
                }
            }
            b"PW" => {
                self.flush(page);
                let w = g(0, if self.width_relative { 0.1 } else { 0.35 });
                if a.len() >= 2 {
                    let i = g(1, 1.0) as usize;
                    while self.widths.len() <= i {
                        self.widths.push(0.35);
                    }
                    self.widths[i] = w;
                } else {
                    for x in self.widths.iter_mut() {
                        *x = w;
                    }
                }
            }
            b"WU" => {
                self.width_relative = g(0, 0.0) != 0.0;
                let w = if self.width_relative { 0.1 } else { 0.35 };
                for x in self.widths.iter_mut() {
                    *x = w;
                }
            }
            b"LT" => {
                self.flush(page);
                if a.is_empty() {
                    self.line_type = None;
                } else {
                    let t = g(0, 0.0) as i32;
                    self.line_type = if t == 0 && a.len() == 1 { Some((0, 4.0)) } else { Some((t, g(1, 4.0))) };
                    if t == 0 {
                        self.line_type = None;
                    }
                }
            }
            b"LA" => {
                self.flush(page);
                for kv in a.chunks(2) {
                    if kv.len() < 2 {
                        break;
                    }
                    match kv[0] as i32 {
                        1 => {
                            self.cap = match kv[1] as i32 {
                                2 => LineCap::Square,
                                3 => LineCap::Round,
                                4 => LineCap::Round,
                                _ => LineCap::Butt,
                            }
                        }
                        2 => {
                            self.join = match kv[1] as i32 {
                                1 | 2 => LineJoin::Miter,
                                4 => LineJoin::Round,
                                5 | 3 => LineJoin::Bevel,
                                _ => LineJoin::Miter,
                            }
                        }
                        3 => self.miter = kv[1].max(1.0),
                        _ => {}
                    }
                }
            }
            b"FT" => {
                self.fill_type = g(0, 1.0) as i32;
                if self.fill_type == 10 {
                    self.fill_level = g(1, 100.0);
                }
            }
            b"SC" => {
                self.flush(page);
                if a.len() < 4 {
                    self.scale = None;
                } else {
                    let t = g(4, 0.0) as i32;
                    self.scale = Some(match t {
                        1 => Scale::Isotropic(a[0], a[1], a[2], a[3]),
                        2 => Scale::Factor(a[0], a[1], a[2], a[3]),
                        _ => Scale::Anisotropic(a[0], a[1], a[2], a[3]),
                    });
                }
            }
            b"IP" => {
                if a.len() >= 4 {
                    self.p1 = (a[0], a[1]);
                    self.p2 = (a[2], a[3]);
                } else if a.len() >= 2 {
                    let (dx, dy) = (self.p2.0 - self.p1.0, self.p2.1 - self.p1.1);
                    self.p1 = (a[0], a[1]);
                    self.p2 = (a[0] + dx, a[1] + dy);
                } else {
                    self.default_p1p2();
                }
            }
            b"IR" => {
                let (fw, fh) = self.frame_plu();
                if a.len() >= 2 {
                    self.p1 = (a[0] / 100.0 * fw, a[1] / 100.0 * fh);
                    if a.len() >= 4 {
                        self.p2 = (a[2] / 100.0 * fw, a[3] / 100.0 * fh);
                    }
                } else {
                    self.default_p1p2();
                }
            }
            b"IW" => {
                self.flush(page);
                if a.len() >= 4 {
                    let p = self.user_to_plu(a[0], a[1]);
                    let q = self.user_to_plu(a[2], a[3]);
                    self.window = Some([p.0.min(q.0), p.1.min(q.1), p.0.max(q.0), p.1.max(q.1)]);
                } else {
                    self.window = None;
                }
            }
            b"RO" => {
                self.flush(page);
                let r = (g(0, 0.0) as i32).rem_euclid(360);
                self.rotation = if r % 90 == 0 { r } else { 0 };
                self.default_p1p2();
            }
            b"PS" => {
                if a.len() >= 2 {
                    self.plot_size = Some((a[0].max(a[1]) / PLU * 72.0, a[0].min(a[1]) / PLU * 72.0));
                } else if a.len() == 1 {
                    self.plot_size = Some((a[0] / PLU * 72.0, a[0] / PLU * 72.0 * 0.7));
                }
            }
            b"PM" => {
                let m = g(0, 0.0) as i32;
                match m {
                    0 => {
                        self.flush(page);
                        self.poly_buffer = None;
                        let mut acc = PathAcc::new();
                        acc.move_to(self.pos.0, self.pos.1);
                        self.polygon = Some(acc);
                        self.poly_pen_up = !self.pen_down;
                    }
                    1 => {
                        if let Some(p) = self.polygon.as_mut() {
                            p.close();
                        }
                    }
                    _ => {
                        if let Some(p) = self.polygon.as_mut() {
                            p.close();
                        }
                        // keep the buffer for FP/EP but leave polygon mode
                        if let Some(poly) = self.polygon.take() {
                            self.poly_buffer = Some(poly);
                        }
                    }
                }
            }
            b"FP" => {
                if let Some(poly) = self.poly_buffer.clone().or_else(|| self.polygon.clone()) {
                    self.fill_acc(page, &poly, g(0, 0.0) as i32 == 0);
                }
            }
            b"EP" => {
                if let Some(poly) = self.poly_buffer.clone().or_else(|| self.polygon.clone()) {
                    self.edge_acc(page, &poly);
                }
            }
            b"RA" | b"RR" | b"EA" | b"ER" => {
                if a.len() < 2 {
                    return;
                }
                let corner = if op[1] == b'A' {
                    self.user_to_plu(a[0], a[1])
                } else {
                    let (dx, dy) = self.user_delta(a[0], a[1]);
                    (self.pos.0 + dx, self.pos.1 + dy)
                };
                let acc = self.rect_acc(self.pos, corner);
                if let Some(poly) = self.polygon.as_mut() {
                    poly.append(&acc);
                } else if op[0] == b'R' {
                    self.fill_acc(page, &acc, false);
                } else {
                    self.edge_acc(page, &acc);
                }
            }
            b"CI" => {
                let r = g(0, 0.0);
                let (rx, _) = self.user_delta(r, 0.0);
                let r = rx.abs();
                let c = self.pos;
                let mut acc = PathAcc::new();
                acc.arc(&Matrix::identity(), c.0, c.1, r, 0.0, 360.0, true, false);
                acc.close();
                if let Some(poly) = self.polygon.as_mut() {
                    poly.append(&acc);
                } else {
                    self.edge_acc(page, &acc);
                }
            }
            b"WG" | b"EW" => {
                if a.len() < 3 {
                    return;
                }
                let (rx, _) = self.user_delta(a[0], 0.0);
                let r = rx.abs();
                let c = self.pos;
                let st = a[1].to_radians();
                let start = (c.0 + r * st.cos(), c.1 + r * st.sin());
                let acc = self.arc_acc(c, start, a[2], true);
                if let Some(poly) = self.polygon.as_mut() {
                    poly.append(&acc);
                } else if op == b"WG" {
                    self.fill_acc(page, &acc, false);
                } else {
                    self.edge_acc(page, &acc);
                }
            }
            b"AA" | b"AR" => {
                if a.len() < 3 {
                    return;
                }
                let center = if op == b"AA" {
                    self.user_to_plu(a[0], a[1])
                } else {
                    let (dx, dy) = self.user_delta(a[0], a[1]);
                    (self.pos.0 + dx, self.pos.1 + dy)
                };
                let acc = self.arc_acc(center, self.pos, a[2], false);
                // walk the arc as line segments so polygons and pen state work
                let flat = acc.transformed(&Matrix::identity());
                let mut last = self.pos;
                for s in &flat.segs {
                    match *s {
                        Seg::Line(x, y) => {
                            self.line_to(page, (x, y));
                            last = (x, y);
                        }
                        Seg::Cubic(x1, y1, x2, y2, x, y) => {
                            let p0 = last;
                            for k in 1..=12 {
                                let t = k as f64 / 12.0;
                                let u = 1.0 - t;
                                let px = u * u * u * p0.0 + 3.0 * u * u * t * x1 + 3.0 * u * t * t * x2 + t * t * t * x;
                                let py = u * u * u * p0.1 + 3.0 * u * u * t * y1 + 3.0 * u * t * t * y2 + t * t * t * y;
                                self.line_to(page, (px, py));
                            }
                            last = (x, y);
                        }
                        _ => {}
                    }
                }
            }
            b"SD" | b"AD" => {
                let is_primary = op == b"SD";
                for kv in a.chunks(2) {
                    if kv.len() < 2 || !is_primary {
                        break;
                    }
                    match kv[0] as i32 {
                        1 => self.font_symset = kv[1] as i32,
                        2 => self.font_spacing = kv[1] as i32,
                        3 => self.font_pitch = kv[1],
                        4 => self.font_height = kv[1],
                        5 => self.font_italic = kv[1] as i32 != 0,
                        6 => self.font_bold = kv[1] > 0.0,
                        7 => self.font_typeface = kv[1] as i32,
                        _ => {}
                    }
                }
                self.char_size = None;
            }
            b"SI" => self.char_size = if a.len() >= 2 { Some((a[0], a[1])) } else { None },
            b"SR" => {
                if a.len() >= 2 {
                    let (fw, fh) = (self.p2.0 - self.p1.0, self.p2.1 - self.p1.1);
                    self.char_size = Some((a[0] / 100.0 * fw / self.plu_per_mm() / 10.0, a[1] / 100.0 * fh / self.plu_per_mm() / 10.0));
                }
            }
            b"SL" => self.slant = g(0, 0.0),
            b"DI" | b"DR" => self.direction = if a.len() >= 2 && (a[0] != 0.0 || a[1] != 0.0) { (a[0], a[1]) } else { (1.0, 0.0) },
            b"LO" => self.label_origin = g(0, 1.0) as i32,
            b"CP" => {
                // character plot: spaces/lines
                let h = self.char_size.map(|(_, h)| h * 10.0 * self.plu_per_mm()).unwrap_or(self.font_height / 72.0 * PLU);
                if a.len() >= 2 {
                    self.pos.0 += a[0] * h * 0.6;
                    self.pos.1 -= a[1] * h * 1.6;
                } else {
                    self.pos.1 -= h * 1.6;
                }
            }
            b"TR" | b"MC" | b"PP" | b"CR" | b"SV" | b"SS" | b"SA" | b"ES" | b"SM" | b"LM" | b"QL" | b"VS" | b"CT" | b"FN" | b"FR" | b"PG" | b"RP" | b"MG" | b"OI" | b"OP" | b"OS" | b"OE" | b"OH" | b"OW" | b"DV" | b"UL" | b"RF" | b"FI" | b"FS" | b"BR" | b"BZ" | b"PL" | b"LU" | b"KY" | b"AC" | b"AT" | b"RT" | b"EC" | b"NR" | b"PT" | b"CF" | b"DL" | b"CC" => {
                if op == b"AT" || op == b"RT" {
                    // three-point arc: approximate with straight lines through the points
                    if a.len() >= 4 {
                        let pts = if op == b"AT" { vec![(a[0], a[1]), (a[2], a[3])] } else { vec![(a[0], a[1]), (a[2], a[3])] };
                        for (x, y) in pts {
                            let to = if op == b"AT" {
                                self.user_to_plu(x, y)
                            } else {
                                let (dx, dy) = self.user_delta(x, y);
                                (self.pos.0 + dx, self.pos.1 + dy)
                            };
                            self.line_to(page, to);
                        }
                    }
                } else if matches!(op, b"BR" | b"BZ") {
                    // Bézier: approximate through control points
                    let rel = op == b"BR";
                    for pts in a.chunks(6) {
                        if pts.len() < 6 {
                            break;
                        }
                        let p0 = self.pos;
                        let cp = |i: usize| if rel { let (dx, dy) = self.user_delta(pts[i], pts[i + 1]); (p0.0 + dx, p0.1 + dy) } else { self.user_to_plu(pts[i], pts[i + 1]) };
                        let (c1, c2, e) = (cp(0), cp(2), cp(4));
                        for k in 1..=16 {
                            let t = k as f64 / 16.0;
                            let u = 1.0 - t;
                            let x = u * u * u * p0.0 + 3.0 * u * u * t * c1.0 + 3.0 * u * t * t * c2.0 + t * t * t * e.0;
                            let y = u * u * u * p0.1 + 3.0 * u * u * t * c1.1 + 3.0 * u * t * t * c2.1 + t * t * t * e.1;
                            self.line_to(page, (x, y));
                        }
                    }
                } else if op == b"UL" || op == b"RF" {
                    doc.warn(format!("HP-GL/2 {} (user line type / raster fill) is approximated", String::from_utf8_lossy(op)));
                }
            }
            _ => {
                doc.warn(format!("HP-GL/2 instruction {} is not supported", String::from_utf8_lossy(op)));
            }
        }
    }

    /// PE: polyline encoded.
    fn polyline_encoded(&mut self, page: &mut Page, body: &[u8]) {
        let mut p = 0;
        let mut seven = false;
        let mut frac_bits = 0u32;
        let mut pen_up_next = false;
        let mut abs_next = false;
        let n = body.len();
        let read_num = |p: &mut usize, seven: bool| -> Option<i64> {
            let mut v: i64 = 0;
            let mut mult: i64 = 1;
            while *p < n {
                let c = body[*p] as i64;
                *p += 1;
                if seven {
                    if (63..=94).contains(&c) {
                        v += (c - 63) * mult;
                        mult *= 32;
                    } else if (95..=126).contains(&c) {
                        v += (c - 95) * mult;
                        return Some(if v & 1 == 1 { -(v >> 1) } else { v >> 1 });
                    }
                } else if (63..=126).contains(&c) {
                    v += (c - 63) * mult;
                    mult *= 64;
                } else if (191..=254).contains(&c) {
                    v += (c - 191) * mult;
                    return Some(if v & 1 == 1 { -(v >> 1) } else { v >> 1 });
                }
                if mult > 1 << 50 {
                    return None;
                }
            }
            None
        };
        let was_down = self.pen_down;
        while p < n {
            let c = body[p];
            match c {
                b':' => {
                    p += 1;
                    if let Some(pen) = read_num(&mut p, seven) {
                        self.flush(page);
                        self.pen = pen.max(0) as usize;
                    }
                }
                b'<' => {
                    p += 1;
                    pen_up_next = true;
                }
                b'>' => {
                    p += 1;
                    if let Some(f) = read_num(&mut p, seven) {
                        frac_bits = f.clamp(0, 26) as u32;
                    }
                }
                b'=' => {
                    p += 1;
                    abs_next = true;
                }
                b'7' => {
                    p += 1;
                    seven = true;
                }
                b' ' | b'\r' | b'\n' | b'\t' => p += 1,
                _ => {
                    let Some(x) = read_num(&mut p, seven) else { break };
                    let Some(y) = read_num(&mut p, seven) else { break };
                    let k = (1u64 << frac_bits) as f64;
                    let (x, y) = (x as f64 / k, y as f64 / k);
                    let to = if abs_next {
                        self.user_to_plu(x, y)
                    } else {
                        let (dx, dy) = self.user_delta(x, y);
                        (self.pos.0 + dx, self.pos.1 + dy)
                    };
                    if pen_up_next {
                        self.pen_down = false;
                        if self.polygon.is_some() {
                            self.poly_pen_up = true;
                        }
                    } else {
                        self.pen_down = true;
                        self.poly_pen_up = false;
                    }
                    self.line_to(page, to);
                    pen_up_next = false;
                    abs_next = false;
                }
            }
        }
        let _ = was_down;
    }
}

/// A stand-alone HP-GL/2 (plot) file: draw it, then size the page to the drawing.
pub fn run_standalone(doc: &mut Document, data: &[u8], start: usize) {
    let big = Frame { x: 0.0, y: 0.0, w: 48.0 * 72.0, h: 48.0 * 72.0 };
    let mut h = Hpgl::new(big);
    let mut page = Page::new(big.w as f32, big.h as f32);
    let mut p = start;
    while p < data.len() {
        p = h.run(data, p, 0, &mut page, doc);
        if p < data.len() {
            // skip escape sequences (e.g. RTL/PJL) inside a plot file
            p += 1;
        }
    }
    h.flush(&mut page);
    if page.items.is_empty() {
        doc.warn("The HP-GL/2 plot drew nothing");
        return;
    }
    // bounds of the drawing (y-down page points); plotted from the frame's bottom-left
    let mut b = [f32::MAX, f32::MAX, f32::MIN, f32::MIN];
    for it in &page.items {
        if let Some(r) = crate::render::item_bounds(it) {
            b = [b[0].min(r[0]), b[1].min(r[1]), b[2].max(r[2]), b[3].max(r[3])];
        }
    }
    let (w, hh) = match h.plot_size {
        Some((pw, ph)) => (pw as f32, ph as f32),
        None => {
            // drawing from the plot origin, with a small margin
            let w = (b[2] - 0.0).max(72.0) + 18.0;
            let hh = (big.h as f32 - b[1]).max(72.0) + 18.0;
            (w, hh)
        }
    };
    let dy = hh - big.h as f32;
    let t = Transform::from_translate(0.0, dy);
    let mut out = Page::new(w, hh);
    out.label = "HP-GL/2 plot".into();
    for (i, it) in std::mem::take(&mut page.items).into_iter().enumerate() {
        let meta = page.meta_of(i);
        let moved = match it {
            Item::Fill { path, transform, brush, even_odd, clip } => Item::Fill { path, transform: transform.post_concat(t), brush, even_odd, clip: move_clip(clip, t) },
            Item::Stroke { path, transform, brush, style, clip } => Item::Stroke { path, transform: transform.post_concat(t), brush, style, clip: move_clip(clip, t) },
            Item::Image { image, transform, clip } => Item::Image { image, transform: transform.post_concat(t), clip: move_clip(clip, t) },
        };
        out.cur_src = meta.src;
        out.cur_kind = meta.kind;
        out.push(moved);
    }
    for r in &page.text {
        let mut r2 = r.clone();
        for bx in r2.boxes.iter_mut() {
            bx[1] += dy;
            bx[3] += dy;
        }
        out.text.push(r2);
    }
    doc.pages.push(out);
}

fn move_clip(c: Clip, t: Transform) -> Clip {
    let c = c?;
    let v: Vec<ClipPath> = c.iter().filter_map(|cp| Some(ClipPath { path: cp.path.clone().transform(t)?, even_odd: cp.even_odd })).collect();
    Some(Arc::new(v))
}

/// Does this data look like a stand-alone HP-GL/2 plot?
pub fn looks_like_hpgl(d: &[u8]) -> bool {
    let s: Vec<u8> = d.iter().take(64).copied().filter(|c| !c.is_ascii_whitespace() && *c != 0x1B).collect();
    let up: Vec<u8> = s.iter().map(|c| c.to_ascii_uppercase()).collect();
    up.starts_with(b"IN;") || up.starts_with(b"IN ;") || up.starts_with(b"BP") || up.starts_with(b"%-1BIN") || up.starts_with(b"BPIN") || up.starts_with(b"DF;") || (up.starts_with(b"CO\"") && up.windows(3).any(|w| w == b"IN;"))
}
