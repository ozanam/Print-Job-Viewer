//! XPS / OpenXPS documents (also the spool format of Windows v4 printer drivers).

use crate::doc::*;
use crate::fonts::OwnedFont;
use crate::geom::*;
use crate::xml::{self, Element};
use crate::zip::Zip;
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use tiny_skia::{LineCap, LineJoin, Transform};

/// XPS units are 1/96 inch.
const U: f64 = 72.0 / 96.0;

#[derive(Clone)]
enum XBrush {
    Solid(Rgb, f64),
    Image { uri: String, viewbox: [f64; 4], viewport: [f64; 4], transform: Matrix, tile: bool },
    Gradient(Vec<(f64, Rgb)>, f64),
}

struct Ctx<'a> {
    zip: &'a Zip<'a>,
    page_dir: String,
    fonts: HashMap<String, Option<(OwnedFont, HashMap<u16, char>)>>,
    images: HashMap<String, Option<(Arc<RasterImage>, f64, f64)>>,
    resources: Vec<HashMap<String, Element>>,
    warnings: Vec<String>,
    stats: BTreeMap<String, usize>,
}

fn resolve(base_dir: &str, uri: &str) -> String {
    let uri = uri.trim();
    let uri = uri.split('#').next().unwrap_or(uri);
    if let Some(rest) = uri.strip_prefix('/') {
        return rest.to_string();
    }
    let mut parts: Vec<&str> = base_dir.split('/').filter(|s| !s.is_empty()).collect();
    for seg in uri.split('/') {
        match seg {
            ".." => {
                parts.pop();
            }
            "." | "" => {}
            s => parts.push(s),
        }
    }
    parts.join("/")
}

fn dir_of(path: &str) -> String {
    match path.rfind('/') {
        Some(i) => path[..i].to_string(),
        None => String::new(),
    }
}

fn parse_colour(s: &str) -> Option<(Rgb, f64)> {
    let s = s.trim();
    if let Some(h) = s.strip_prefix('#') {
        let v = u32::from_str_radix(h, 16).ok()?;
        return Some(match h.len() {
            8 => (Rgb(((v >> 16) & 255) as f32 / 255.0, ((v >> 8) & 255) as f32 / 255.0, (v & 255) as f32 / 255.0), ((v >> 24) & 255) as f64 / 255.0),
            6 => (Rgb(((v >> 16) & 255) as f32 / 255.0, ((v >> 8) & 255) as f32 / 255.0, (v & 255) as f32 / 255.0), 1.0),
            _ => return None,
        });
    }
    if let Some(r) = s.strip_prefix("sc#") {
        let v: Vec<f64> = r.split(',').filter_map(|x| x.trim().parse().ok()).collect();
        let lin = |c: f64| -> f32 {
            let c = c.clamp(0.0, 1.0);
            (if c <= 0.0031308 { c * 12.92 } else { 1.055 * c.powf(1.0 / 2.4) - 0.055 }) as f32
        };
        return match v.len() {
            4 => Some((Rgb(lin(v[1]), lin(v[2]), lin(v[3])), v[0])),
            3 => Some((Rgb(lin(v[0]), lin(v[1]), lin(v[2])), 1.0)),
            _ => None,
        };
    }
    if s.starts_with("ContextColor") {
        // ICC-based colour: use the trailing components as a rough guess
        let comps: Vec<f64> = s.split_whitespace().nth(1).unwrap_or("").split(',').filter_map(|x| x.trim().parse().ok()).collect();
        return match comps.len() {
            5 => Some((Rgb::from_cmyk(comps[1] as f32, comps[2] as f32, comps[3] as f32, comps[4] as f32), comps[0])),
            4 => Some((Rgb(comps[1] as f32, comps[2] as f32, comps[3] as f32), comps[0])),
            2 => Some((Rgb::gray(comps[1] as f32), comps[0])),
            _ => Some((Rgb::BLACK, 1.0)),
        };
    }
    None
}

fn nums(s: &str) -> Vec<f64> {
    s.split(|c: char| c == ',' || c.is_whitespace()).filter(|x| !x.is_empty()).filter_map(|x| x.parse().ok()).collect()
}

fn parse_matrix(s: &str) -> Option<Matrix> {
    let v = nums(s);
    if v.len() == 6 { Some(Matrix::new(v[0], v[1], v[2], v[3], v[4], v[5])) } else { None }
}

fn blend(c: Rgb, a: f64) -> Rgb {
    if a >= 0.999 {
        return c;
    }
    let a = a.clamp(0.0, 1.0) as f32;
    Rgb(1.0 - (1.0 - c.0) * a, 1.0 - (1.0 - c.1) * a, 1.0 - (1.0 - c.2) * a)
}

/// Abbreviated path geometry syntax ("F1 M 10,10 L 20,20 C … Z").
fn parse_geometry(s: &str, m: &Matrix) -> (PathAcc, bool) {
    let mut acc = PathAcc::new();
    let mut even_odd = true;
    let b: Vec<char> = s.chars().collect();
    let mut i = 0;
    let mut cmd = 'M';
    let mut cur = (0.0f64, 0.0f64);
    let mut start = (0.0f64, 0.0f64);
    let mut last_ctrl: Option<(f64, f64)> = None;
    let read_num = |i: &mut usize| -> Option<f64> {
        while *i < b.len() && (b[*i].is_whitespace() || b[*i] == ',') {
            *i += 1;
        }
        let s0 = *i;
        if *i < b.len() && (b[*i] == '-' || b[*i] == '+') {
            *i += 1;
        }
        while *i < b.len() && (b[*i].is_ascii_digit() || b[*i] == '.') {
            *i += 1;
        }
        if *i < b.len() && (b[*i] == 'e' || b[*i] == 'E') {
            *i += 1;
            if *i < b.len() && (b[*i] == '-' || b[*i] == '+') {
                *i += 1;
            }
            while *i < b.len() && b[*i].is_ascii_digit() {
                *i += 1;
            }
        }
        let t: String = b[s0..*i].iter().collect();
        t.parse().ok()
    };
    let tp = |p: (f64, f64)| m.apply(p.0, p.1);
    loop {
        while i < b.len() && (b[i].is_whitespace() || b[i] == ',') {
            i += 1;
        }
        if i >= b.len() {
            break;
        }
        if b[i].is_ascii_alphabetic() {
            cmd = b[i];
            i += 1;
            if cmd == 'F' {
                if let Some(v) = read_num(&mut i) {
                    even_odd = v == 0.0;
                }
                continue;
            }
            if cmd == 'Z' || cmd == 'z' {
                acc.close();
                cur = start;
                continue;
            }
        }
        let rel = cmd.is_ascii_lowercase();
        let base = if rel { cur } else { (0.0, 0.0) };
        let before = i;
        match cmd.to_ascii_uppercase() {
            'M' => {
                let (Some(x), Some(y)) = (read_num(&mut i), read_num(&mut i)) else { break };
                cur = (base.0 + x, base.1 + y);
                start = cur;
                let p = tp(cur);
                acc.move_to(p.0, p.1);
                cmd = if rel { 'l' } else { 'L' };
                last_ctrl = None;
            }
            'L' => {
                let (Some(x), Some(y)) = (read_num(&mut i), read_num(&mut i)) else { break };
                cur = (base.0 + x, base.1 + y);
                let p = tp(cur);
                acc.line_to(p.0, p.1);
                last_ctrl = None;
            }
            'H' => {
                let Some(x) = read_num(&mut i) else { break };
                cur.0 = base.0 + x;
                let p = tp(cur);
                acc.line_to(p.0, p.1);
            }
            'V' => {
                let Some(y) = read_num(&mut i) else { break };
                cur.1 = base.1 + y;
                let p = tp(cur);
                acc.line_to(p.0, p.1);
            }
            'C' => {
                let v: Vec<f64> = (0..6).filter_map(|_| read_num(&mut i)).collect();
                if v.len() < 6 {
                    break;
                }
                let c1 = (base.0 + v[0], base.1 + v[1]);
                let c2 = (base.0 + v[2], base.1 + v[3]);
                let e = (base.0 + v[4], base.1 + v[5]);
                let (a, b2, c) = (tp(c1), tp(c2), tp(e));
                acc.curve_to(a.0, a.1, b2.0, b2.1, c.0, c.1);
                last_ctrl = Some(c2);
                cur = e;
            }
            'S' => {
                let v: Vec<f64> = (0..4).filter_map(|_| read_num(&mut i)).collect();
                if v.len() < 4 {
                    break;
                }
                let c1 = last_ctrl.map(|c| (2.0 * cur.0 - c.0, 2.0 * cur.1 - c.1)).unwrap_or(cur);
                let c2 = (base.0 + v[0], base.1 + v[1]);
                let e = (base.0 + v[2], base.1 + v[3]);
                let (a, b2, c) = (tp(c1), tp(c2), tp(e));
                acc.curve_to(a.0, a.1, b2.0, b2.1, c.0, c.1);
                last_ctrl = Some(c2);
                cur = e;
            }
            'Q' => {
                let v: Vec<f64> = (0..4).filter_map(|_| read_num(&mut i)).collect();
                if v.len() < 4 {
                    break;
                }
                let q = (base.0 + v[0], base.1 + v[1]);
                let e = (base.0 + v[2], base.1 + v[3]);
                let c1 = (cur.0 + 2.0 / 3.0 * (q.0 - cur.0), cur.1 + 2.0 / 3.0 * (q.1 - cur.1));
                let c2 = (e.0 + 2.0 / 3.0 * (q.0 - e.0), e.1 + 2.0 / 3.0 * (q.1 - e.1));
                let (a, b2, c) = (tp(c1), tp(c2), tp(e));
                acc.curve_to(a.0, a.1, b2.0, b2.1, c.0, c.1);
                cur = e;
                last_ctrl = None;
            }
            'A' => {
                let v: Vec<f64> = (0..7).filter_map(|_| read_num(&mut i)).collect();
                if v.len() < 7 {
                    break;
                }
                let e = (base.0 + v[5], base.1 + v[6]);
                svg_arc(&mut acc, m, cur, v[0], v[1], v[2], v[3] != 0.0, v[4] != 0.0, e);
                cur = e;
                last_ctrl = None;
            }
            _ => {
                i += 1;
            }
        }
        if i == before {
            i += 1;
        }
    }
    (acc, even_odd)
}

/// SVG-style elliptical arc to Béziers.
#[allow(clippy::too_many_arguments)]
fn svg_arc(acc: &mut PathAcc, m: &Matrix, p0: (f64, f64), rx: f64, ry: f64, rot: f64, large: bool, sweep: bool, p1: (f64, f64)) {
    let (mut rx, mut ry) = (rx.abs(), ry.abs());
    if rx < 1e-9 || ry < 1e-9 {
        let p = m.apply(p1.0, p1.1);
        acc.line_to(p.0, p.1);
        return;
    }
    let phi = rot.to_radians();
    let (cp, sp) = (phi.cos(), phi.sin());
    let dx = (p0.0 - p1.0) / 2.0;
    let dy = (p0.1 - p1.1) / 2.0;
    let x1 = cp * dx + sp * dy;
    let y1 = -sp * dx + cp * dy;
    let lam = (x1 * x1) / (rx * rx) + (y1 * y1) / (ry * ry);
    if lam > 1.0 {
        rx *= lam.sqrt();
        ry *= lam.sqrt();
    }
    let num = (rx * rx * ry * ry - rx * rx * y1 * y1 - ry * ry * x1 * x1).max(0.0);
    let den = rx * rx * y1 * y1 + ry * ry * x1 * x1;
    let mut coef = if den > 0.0 { (num / den).sqrt() } else { 0.0 };
    if large == sweep {
        coef = -coef;
    }
    let cx1 = coef * rx * y1 / ry;
    let cy1 = -coef * ry * x1 / rx;
    let cx = cp * cx1 - sp * cy1 + (p0.0 + p1.0) / 2.0;
    let cy = sp * cx1 + cp * cy1 + (p0.1 + p1.1) / 2.0;
    let ang = |ux: f64, uy: f64, vx: f64, vy: f64| {
        let a = (ux * vy - uy * vx).atan2(ux * vx + uy * vy);
        a
    };
    let t1 = ang(1.0, 0.0, (x1 - cx1) / rx, (y1 - cy1) / ry);
    let mut dt = ang((x1 - cx1) / rx, (y1 - cy1) / ry, (-x1 - cx1) / rx, (-y1 - cy1) / ry);
    if !sweep && dt > 0.0 {
        dt -= std::f64::consts::TAU;
    } else if sweep && dt < 0.0 {
        dt += std::f64::consts::TAU;
    }
    let n = ((dt.abs() / (std::f64::consts::PI / 2.0)).ceil() as usize).max(1);
    let step = dt / n as f64;
    let k = 4.0 / 3.0 * (step / 4.0).tan();
    let pt = |t: f64| (cx + rx * t.cos() * cp - ry * t.sin() * sp, cy + rx * t.cos() * sp + ry * t.sin() * cp);
    let dpt = |t: f64| (-rx * t.sin() * cp - ry * t.cos() * sp, -rx * t.sin() * sp + ry * t.cos() * cp);
    let mut t = t1;
    for _ in 0..n {
        let a = pt(t);
        let b = pt(t + step);
        let da = dpt(t);
        let db = dpt(t + step);
        let c1 = (a.0 + k * da.0, a.1 + k * da.1);
        let c2 = (b.0 - k * db.0, b.1 - k * db.1);
        let (q1, q2, q3) = (m.apply(c1.0, c1.1), m.apply(c2.0, c2.1), m.apply(b.0, b.1));
        acc.curve_to(q1.0, q1.1, q2.0, q2.1, q3.0, q3.1);
        t += step;
    }
}

/// PathGeometry element form.
fn geometry_element(g: &Element, m: &Matrix) -> (PathAcc, bool) {
    let mut acc = PathAcc::new();
    let even_odd = g.attr("FillRule").map(|r| r != "NonZero").unwrap_or(true);
    let gm = g.attr("Transform").and_then(parse_matrix).map(|t| t.then(m)).unwrap_or(*m);
    if let Some(f) = g.attr("Figures") {
        let (a, _) = parse_geometry(f, &gm);
        acc.append(&a);
    }
    for fig in g.children_named("PathFigure") {
        let sp = nums(fig.attr("StartPoint").unwrap_or("0,0"));
        let mut cur = (sp.first().copied().unwrap_or(0.0), sp.get(1).copied().unwrap_or(0.0));
        let p = gm.apply(cur.0, cur.1);
        acc.move_to(p.0, p.1);
        for seg in &fig.children {
            let pts = nums(seg.attr("Points").or(seg.attr("Point")).unwrap_or(""));
            match seg.local() {
                "PolyLineSegment" => {
                    for c in pts.chunks(2) {
                        if c.len() == 2 {
                            cur = (c[0], c[1]);
                            let p = gm.apply(cur.0, cur.1);
                            acc.line_to(p.0, p.1);
                        }
                    }
                }
                "PolyBezierSegment" => {
                    for c in pts.chunks(6) {
                        if c.len() == 6 {
                            let (a, b, e) = (gm.apply(c[0], c[1]), gm.apply(c[2], c[3]), gm.apply(c[4], c[5]));
                            acc.curve_to(a.0, a.1, b.0, b.1, e.0, e.1);
                            cur = (c[4], c[5]);
                        }
                    }
                }
                "PolyQuadraticBezierSegment" => {
                    for c in pts.chunks(4) {
                        if c.len() == 4 {
                            let q = (c[0], c[1]);
                            let e = (c[2], c[3]);
                            let c1 = (cur.0 + 2.0 / 3.0 * (q.0 - cur.0), cur.1 + 2.0 / 3.0 * (q.1 - cur.1));
                            let c2 = (e.0 + 2.0 / 3.0 * (q.0 - e.0), e.1 + 2.0 / 3.0 * (q.1 - e.1));
                            let (a, b, ee) = (gm.apply(c1.0, c1.1), gm.apply(c2.0, c2.1), gm.apply(e.0, e.1));
                            acc.curve_to(a.0, a.1, b.0, b.1, ee.0, ee.1);
                            cur = e;
                        }
                    }
                }
                "ArcSegment" => {
                    let sz = nums(seg.attr("Size").unwrap_or("0,0"));
                    let e = (pts.first().copied().unwrap_or(0.0), pts.get(1).copied().unwrap_or(0.0));
                    let rot = seg.attr("RotationAngle").and_then(|v| v.parse().ok()).unwrap_or(0.0);
                    let large = seg.attr("IsLargeArc") == Some("true");
                    let sweep = seg.attr("SweepDirection") == Some("Clockwise");
                    svg_arc(&mut acc, &gm, cur, sz.first().copied().unwrap_or(0.0), sz.get(1).copied().unwrap_or(0.0), rot, large, sweep, e);
                    cur = e;
                }
                _ => {}
            }
        }
        if fig.attr("IsClosed") == Some("true") {
            acc.close();
        }
    }
    (acc, even_odd)
}

impl Ctx<'_> {
    fn lookup(&self, key: &str) -> Option<&Element> {
        self.resources.iter().rev().find_map(|r| r.get(key))
    }

    fn static_ref(&self, v: &str) -> Option<Element> {
        let k = v.trim().strip_prefix("{StaticResource")?.trim().trim_end_matches('}').trim();
        self.lookup(k).cloned()
    }

    fn load_resources(&mut self, el: &Element, dir: &str) {
        let mut map = HashMap::new();
        for rd in el.children.iter().flat_map(|c| c.children.iter()) {
            if rd.local() != "ResourceDictionary" {
                continue;
            }
            if let Some(src) = rd.attr("Source") {
                let p = resolve(dir, src);
                if let Some(x) = self.zip.read(&p).and_then(|d| xml::parse(&d)) {
                    for c in &x.children {
                        if let Some(k) = c.attr("Key") {
                            map.insert(k.to_string(), c.clone());
                        }
                    }
                }
            }
            for c in &rd.children {
                if let Some(k) = c.attr("Key") {
                    map.insert(k.to_string(), c.clone());
                }
            }
        }
        self.resources.push(map);
    }

    fn brush_from(&mut self, el: &Element) -> Option<XBrush> {
        let op = el.attr("Opacity").and_then(|v| v.parse().ok()).unwrap_or(1.0);
        match el.local() {
            "SolidColorBrush" => {
                let (c, a) = parse_colour(el.attr("Color").unwrap_or("#000000"))?;
                Some(XBrush::Solid(c, a * op))
            }
            "ImageBrush" => {
                let uri = el.attr("ImageSource").unwrap_or("").to_string();
                let vb = nums(el.attr("Viewbox").unwrap_or("0,0,0,0"));
                let vp = nums(el.attr("Viewport").unwrap_or("0,0,0,0"));
                let t = el.attr("Transform").and_then(parse_matrix).or_else(|| el.child("ImageBrush.Transform").and_then(|t| t.child("MatrixTransform")).and_then(|m| m.attr("Matrix")).and_then(parse_matrix)).unwrap_or_default();
                let tile = !matches!(el.attr("TileMode"), None | Some("None"));
                if vb.len() < 4 || vp.len() < 4 {
                    return None;
                }
                Some(XBrush::Image { uri, viewbox: [vb[0], vb[1], vb[2], vb[3]], viewport: [vp[0], vp[1], vp[2], vp[3]], transform: t, tile })
            }
            "LinearGradientBrush" | "RadialGradientBrush" => {
                let mut stops = Vec::new();
                let mut all = Vec::new();
                el.find_all("GradientStop", &mut all);
                for s in all {
                    if let Some((c, _)) = s.attr("Color").and_then(parse_colour) {
                        stops.push((s.attr("Offset").and_then(|v| v.parse().ok()).unwrap_or(0.0), c));
                    }
                }
                *self.stats.entry("Gradient fills (approximated)".into()).or_insert(0) += 1;
                Some(XBrush::Gradient(stops, op))
            }
            "VisualBrush" => {
                *self.stats.entry("Visual brushes (not drawn)".into()).or_insert(0) += 1;
                None
            }
            _ => None,
        }
    }

    /// The brush given by attribute `attr` or child element `<Elem.attr>`.
    fn brush(&mut self, el: &Element, attr: &str) -> Option<XBrush> {
        if let Some(v) = el.attr(attr) {
            if v.starts_with('{') {
                let r = self.static_ref(v)?;
                return self.brush_from(&r);
            }
            let (c, a) = parse_colour(v)?;
            return Some(XBrush::Solid(c, a));
        }
        let wrapper = format!("{}.{}", el.local(), attr);
        let w = el.children.iter().find(|c| c.local() == wrapper)?;
        let inner = w.children.first()?.clone();
        self.brush_from(&inner)
    }

    fn image(&mut self, dir: &str, uri: &str) -> Option<(Arc<RasterImage>, f64, f64)> {
        let path = resolve(dir, uri.split('{').next().unwrap_or(uri).trim());
        if let Some(v) = self.images.get(&path) {
            return v.clone();
        }
        let r = match self.zip.read(&path) {
            Some(d) => match crate::imgdec::decode(&d) {
                Ok(img) => {
                    *self.stats.entry("Images".into()).or_insert(0) += 1;
                    let (dx, dy) = img.dpi.filter(|d| d.0 > 1.0 && d.1 > 1.0).unwrap_or((96.0, 96.0));
                    Some((Arc::new(RasterImage::new(img.width, img.height, img.rgba)), dx, dy))
                }
                Err(e) => {
                    self.warnings.push(format!("Image {path}: {e}"));
                    None
                }
            },
            None => {
                self.warnings.push(format!("Image {path} is missing from the package"));
                None
            }
        };
        self.images.insert(path, r.clone());
        r
    }

    fn font(&mut self, dir: &str, uri: &str) -> Option<(OwnedFont, HashMap<u16, char>)> {
        let path = resolve(dir, uri);
        if let Some(v) = self.fonts.get(&path) {
            return v.clone();
        }
        let r = self.zip.read(&path).and_then(|mut d| {
            if path.to_ascii_lowercase().ends_with(".odttf") {
                crate::fonts::deobfuscate_odttf(&mut d, &path);
            }
            let idx = uri.split('#').nth(1).and_then(|i| i.parse().ok()).unwrap_or(0);
            let f = OwnedFont::new(d, idx)?;
            let rev = f.reverse_cmap();
            Some((f, rev))
        });
        if r.is_none() {
            self.warnings.push(format!("Font {path} could not be read"));
        } else {
            *self.stats.entry("Embedded fonts".into()).or_insert(0) += 1;
        }
        self.fonts.insert(path, r.clone());
        r
    }
}

fn path_to(acc: &PathAcc) -> Option<tiny_skia::Path> {
    acc.to_path(&Matrix::identity())
}

fn intersect_clip(clip: &Clip, add: Option<ClipPath>) -> Clip {
    let Some(c) = add else { return clip.clone() };
    let mut v: Vec<ClipPath> = clip.as_ref().map(|x| x.as_ref().clone()).unwrap_or_default();
    v.push(c);
    Some(Arc::new(v))
}

struct State {
    m: Matrix,
    clip: Clip,
    opacity: f64,
}

fn paint(ctx: &mut Ctx, page: &mut Page, acc: &PathAcc, brush: &XBrush, even_odd: bool, st: &State, dir: &str) {
    match brush {
        XBrush::Solid(c, a) => {
            let a = a * st.opacity;
            if a <= 0.004 {
                return;
            }
            if let Some(path) = path_to(acc) {
                page.push(Item::Fill { path, transform: Transform::identity(), brush: Brush::Solid(blend(*c, a)), even_odd, clip: st.clip.clone() });
            }
        }
        XBrush::Gradient(stops, a) => {
            if stops.is_empty() {
                return;
            }
            let n = stops.len() as f32;
            let avg = Rgb(stops.iter().map(|s| s.1 .0).sum::<f32>() / n, stops.iter().map(|s| s.1 .1).sum::<f32>() / n, stops.iter().map(|s| s.1 .2).sum::<f32>() / n);
            if let Some(path) = path_to(acc) {
                page.push(Item::Fill { path, transform: Transform::identity(), brush: Brush::Solid(blend(avg, a * st.opacity)), even_odd, clip: st.clip.clone() });
            }
        }
        XBrush::Image { uri, viewbox, viewport, transform, tile, .. } => {
            let Some((img, dpix, dpiy)) = ctx.image(dir, uri) else { return };
            if viewbox[2] <= 0.0 || viewbox[3] <= 0.0 {
                return;
            }
            // image pixels → image units (1/96 inch at the image's resolution) → viewbox → viewport
            let k = Matrix::translate(-viewbox[0], -viewbox[1])
                .then(&Matrix::scale(viewport[2] / viewbox[2], viewport[3] / viewbox[3]))
                .then(&Matrix::translate(viewport[0], viewport[1]))
                .then(transform)
                .then(&st.m)
                .then(&Matrix::scale(U, U));
            let t = Matrix::scale(96.0 / dpix, 96.0 / dpiy).then(&k);
            let clip = intersect_clip(&st.clip, path_to(acc).map(|p| ClipPath { path: p, even_odd }));
            if *tile {
                let brush = Brush::Pattern { image: img, transform: t.to_ts() };
                if let Some(path) = path_to(acc) {
                    page.push(Item::Fill { path, transform: Transform::identity(), brush, even_odd, clip: st.clip.clone() });
                }
            } else {
                page.cur_kind = Kind::Image;
                page.push(Item::Image { image: img, transform: t.to_ts(), clip });
                page.cur_kind = Kind::Vector;
            }
        }
    }
}

fn render_element(ctx: &mut Ctx, page: &mut Page, el: &Element, st: &State, dir: &str, depth: u32) {
    if depth > 64 {
        return;
    }
    let local_m = el
        .attr("RenderTransform")
        .and_then(|v| if v.starts_with('{') { ctx.static_ref(v).and_then(|e| e.attr("Matrix").and_then(parse_matrix)) } else { parse_matrix(v) })
        .or_else(|| el.child(&format!("{}.RenderTransform", el.local())).and_then(|t| t.child("MatrixTransform")).and_then(|m| m.attr("Matrix")).and_then(parse_matrix));
    let m = local_m.map(|l| l.then(&st.m)).unwrap_or(st.m);
    let page_m = m.then(&Matrix::scale(U, U));
    let opacity = st.opacity * el.attr("Opacity").and_then(|v| v.parse::<f64>().ok()).unwrap_or(1.0);
    let clip_geom = el
        .attr("Clip")
        .map(|v| if v.starts_with('{') { ctx.static_ref(v).map(|e| geometry_element(&e, &page_m)).unwrap_or_default() } else { parse_geometry(v, &page_m) })
        .or_else(|| el.child(&format!("{}.Clip", el.local())).and_then(|c| c.child("PathGeometry")).map(|g| geometry_element(g, &page_m)));
    let clip = match clip_geom {
        Some((acc, eo)) => intersect_clip(&st.clip, path_to(&acc).map(|p| ClipPath { path: p, even_odd: eo })),
        None => st.clip.clone(),
    };
    let st2 = State { m, clip, opacity };
    match el.local() {
        "Canvas" | "FixedPage" => {
            let has_res = el.children.iter().any(|c| c.local().ends_with(".Resources"));
            if has_res {
                ctx.load_resources(el, dir);
            }
            for c in &el.children {
                if c.local().contains('.') {
                    continue;
                }
                render_element(ctx, page, c, &st2, dir, depth + 1);
            }
            if has_res {
                ctx.resources.pop();
            }
        }
        "Path" => {
            *ctx.stats.entry("Paths".into()).or_insert(0) += 1;
            let geom = el
                .attr("Data")
                .map(|v| if v.starts_with('{') { ctx.static_ref(v).map(|e| geometry_element(&e, &page_m)).unwrap_or_default() } else { parse_geometry(v, &page_m) })
                .or_else(|| el.child("Path.Data").and_then(|d| d.child("PathGeometry")).map(|g| geometry_element(g, &page_m)));
            let Some((acc, even_odd)) = geom else { return };
            if acc.is_empty() {
                return;
            }
            if let Some(b) = ctx.brush(el, "Fill") {
                paint(ctx, page, &acc, &b, even_odd, &st2, dir);
            }
            if let Some(b) = ctx.brush(el, "Stroke") {
                let w = el.attr("StrokeThickness").and_then(|v| v.parse::<f64>().ok()).unwrap_or(1.0);
                let colour = match &b {
                    XBrush::Solid(c, a) => blend(*c, a * opacity),
                    XBrush::Gradient(s, a) => blend(s.first().map(|x| x.1).unwrap_or(Rgb::BLACK), a * opacity),
                    XBrush::Image { .. } => Rgb::gray(0.5),
                };
                let cap = match el.attr("StrokeStartLineCap").unwrap_or("Flat") {
                    "Round" => LineCap::Round,
                    "Square" => LineCap::Square,
                    _ => LineCap::Butt,
                };
                let join = match el.attr("StrokeLineJoin").unwrap_or("Miter") {
                    "Round" => LineJoin::Round,
                    "Bevel" => LineJoin::Bevel,
                    _ => LineJoin::Miter,
                };
                let dash = el.attr("StrokeDashArray").map(|d| nums(d).iter().map(|v| (v * w) as f32).collect::<Vec<f32>>()).filter(|v| !v.is_empty() && v.iter().any(|x| *x > 0.0)).map(|mut v| {
                    if v.len() % 2 == 1 {
                        let c = v.clone();
                        v.extend(c);
                    }
                    (v, 0.0)
                });
                let miter = el.attr("StrokeMiterLimit").and_then(|v| v.parse().ok()).unwrap_or(10.0);
                // the path is already in page space: stroke it with the transform's scale applied to the width
                if let (Some(inv), true) = (page_m.invert(), true) {
                    if let Some(path) = acc.to_path(&inv) {
                        let style = StrokeStyle { width: w as f32, cap, join, miter, dash };
                        page.push(Item::Stroke { path, transform: page_m.to_ts(), brush: Brush::Solid(colour), style, clip: st2.clip.clone() });
                    }
                }
            }
        }
        "Glyphs" => {
            *ctx.stats.entry("Text runs (Glyphs)".into()).or_insert(0) += 1;
            glyphs(ctx, page, el, &st2, dir);
        }
        _ => {}
    }
}

fn glyphs(ctx: &mut Ctx, page: &mut Page, el: &Element, st: &State, dir: &str) {
    let Some(uri) = el.attr("FontUri") else { return };
    let Some((font, rev)) = ctx.font(dir, uri) else { return };
    let size = el.attr("FontRenderingEmSize").and_then(|v| v.parse::<f64>().ok()).unwrap_or(12.0);
    let ox = el.attr("OriginX").and_then(|v| v.parse::<f64>().ok()).unwrap_or(0.0);
    let oy = el.attr("OriginY").and_then(|v| v.parse::<f64>().ok()).unwrap_or(0.0);
    let text: Vec<char> = {
        let u = el.attr("UnicodeString").unwrap_or("");
        let u = u.strip_prefix("{}").unwrap_or(u);
        u.chars().collect()
    };
    let rtl = el.attr("BidiLevel").and_then(|v| v.parse::<i32>().ok()).unwrap_or(0) % 2 == 1;
    let sideways = el.attr("IsSideways") == Some("true");
    let bold_sim = matches!(el.attr("StyleSimulations"), Some("BoldSimulation") | Some("BoldItalicSimulation"));
    let italic_sim = matches!(el.attr("StyleSimulations"), Some("ItalicSimulation") | Some("BoldItalicSimulation"));
    // glyph list: (gid, advance in em/100 or None, u offset, v offset, chars)
    let mut list: Vec<(u16, Option<f64>, f64, f64, Vec<char>)> = Vec::new();
    let mut ti = 0usize;
    if let Some(ind) = el.attr("Indices").filter(|s| !s.trim().is_empty()) {
        for spec in ind.split(';') {
            let mut spec = spec.trim();
            let (mut nchars, mut nglyphs) = (1usize, 1usize);
            if let Some(rest) = spec.strip_prefix('(') {
                if let Some(e) = rest.find(')') {
                    let cl: Vec<usize> = rest[..e].split(':').filter_map(|x| x.trim().parse().ok()).collect();
                    nchars = cl.first().copied().unwrap_or(1);
                    nglyphs = cl.get(1).copied().unwrap_or(1);
                    spec = &rest[e + 1..];
                }
            }
            let f: Vec<&str> = spec.split(',').collect();
            let chars: Vec<char> = text.get(ti..(ti + nchars).min(text.len())).map(|s| s.to_vec()).unwrap_or_default();
            let gid = match f.first().map(|s| s.trim()).filter(|s| !s.is_empty()) {
                Some(g) => g.parse().unwrap_or(0),
                None => chars.first().and_then(|c| font.glyph_for_char(*c)).unwrap_or(0),
            };
            let adv = f.get(1).and_then(|s| s.trim().parse::<f64>().ok());
            let uo = f.get(2).and_then(|s| s.trim().parse::<f64>().ok()).unwrap_or(0.0);
            let vo = f.get(3).and_then(|s| s.trim().parse::<f64>().ok()).unwrap_or(0.0);
            list.push((gid, adv, uo, vo, chars));
            ti += nchars;
            let _ = nglyphs;
        }
        // remaining characters without explicit indices
        while ti < text.len() {
            let c = text[ti];
            list.push((font.glyph_for_char(c).unwrap_or(0), None, 0.0, 0.0, vec![c]));
            ti += 1;
        }
    } else {
        for &c in &text {
            list.push((font.glyph_for_char(c).unwrap_or(0), None, 0.0, 0.0, vec![c]));
        }
    }
    let brush = ctx.brush(el, "Fill").unwrap_or(XBrush::Solid(Rgb::BLACK, 1.0));
    let page_m = st.m.then(&Matrix::scale(U, U));
    let s = size / font.upem;
    let mut x = ox;
    let mut out = PathAcc::new();
    for (gid, adv, uo, vo, chars) in &list {
        let adv_units = adv.map(|a| a / 100.0 * size).unwrap_or_else(|| font.advance(*gid) * s);
        let gx = if rtl { x - adv_units } else { x };
        let shear = if italic_sim { 0.33 } else { 0.0 };
        let gm = Matrix::new(s, 0.0, -s * shear, -s, gx + uo / 100.0 * size, oy - vo / 100.0 * size).then(&page_m);
        let gm = if sideways { Matrix::rotate_deg(-90.0).then(&gm) } else { gm };
        let mut g = PathAcc::new();
        font.outline(*gid, &gm, &mut g);
        out.append(&g);
        let a = page_m.apply(gx, oy);
        let b = page_m.apply(gx + adv_units, oy);
        let up = page_m.apply_delta(0.0, -size);
        let ch = chars.first().copied().or_else(|| rev.get(gid).copied()).unwrap_or('\u{FFFD}');
        page.text_char(ch, a, b, up);
        for extra in chars.iter().skip(1) {
            page.text_char(*extra, b, b, up);
        }
        x = if rtl { x - adv_units } else { x + adv_units };
    }
    page.text_break();
    if out.is_empty() {
        return;
    }
    page.cur_kind = Kind::Text;
    paint(ctx, page, &out, &brush, false, st, dir);
    if bold_sim {
        if let XBrush::Solid(c, a) = &brush {
            if let Some(inv) = page_m.invert() {
                if let Some(path) = out.to_path(&inv) {
                    let style = StrokeStyle { width: (size * 0.02) as f32, cap: LineCap::Round, join: LineJoin::Round, miter: 4.0, dash: None };
                    page.push(Item::Stroke { path, transform: page_m.to_ts(), brush: Brush::Solid(blend(*c, a * st.opacity)), style, clip: st.clip.clone() });
                }
            }
        }
    }
    page.cur_kind = Kind::Vector;
}

/// Is this a ZIP package that looks like XPS?
pub fn is_xps(data: &[u8]) -> bool {
    if !data.starts_with(b"PK\x03\x04") {
        return false;
    }
    let head = &data[..data.len().min(1 << 20)];
    let has = |n: &[u8]| head.windows(n.len()).any(|w| w.eq_ignore_ascii_case(n));
    has(b".fdseq") || has(b"FixedDocSeq") || has(b".fpage") || has(b"_rels/.rels") && has(b"Documents/")
}

pub fn run(doc: &mut Document, data: &[u8]) {
    let Some(zip) = Zip::open(data) else {
        doc.warn("The XPS package could not be opened (damaged ZIP)");
        return;
    };
    let mut info = InfoSection::new("XPS document");
    // find the fixed document sequence
    let mut fdseq: Option<String> = None;
    if let Some(rels) = zip.read("_rels/.rels").and_then(|d| xml::parse(&d)) {
        for r in rels.children_named("Relationship") {
            let t = r.attr("Type").unwrap_or("");
            if t.ends_with("/fixedrepresentation") {
                fdseq = r.attr("Target").map(|s| resolve("", s));
            }
            if t.ends_with("/core-properties") {
                if let Some(props) = r.attr("Target").and_then(|p| zip.read(&resolve("", p))).and_then(|d| xml::parse(&d)) {
                    for c in &props.children {
                        if !c.text.trim().is_empty() {
                            info.add(format!("Property: {}", c.local()), c.text.trim().to_string());
                        }
                    }
                }
            }
        }
    }
    let names = zip.names();
    if fdseq.is_none() {
        fdseq = names.iter().find(|n| n.to_ascii_lowercase().ends_with(".fdseq")).cloned();
    }
    let oxps = data.windows(30).take(1 << 16).any(|w| w.starts_with(b"http://schemas.openxps.org"));
    info.add("Flavour", if oxps { "OpenXPS (ECMA-388)" } else { "Microsoft XPS" });
    info.add("Parts in package", names.len().to_string());
    // pages
    let mut pages: Vec<(String, Option<(f64, f64)>)> = Vec::new();
    let mut docs: Vec<String> = Vec::new();
    if let Some(seq) = &fdseq {
        if let Some(x) = zip.read(seq).and_then(|d| xml::parse(&d)) {
            for r in x.children_named("DocumentReference") {
                if let Some(s) = r.attr("Source") {
                    docs.push(resolve(&dir_of(seq), s));
                }
            }
        }
    }
    if docs.is_empty() {
        docs = names.iter().filter(|n| n.to_ascii_lowercase().ends_with(".fdoc")).cloned().collect();
    }
    for d in &docs {
        if let Some(x) = zip.read(d).and_then(|b| xml::parse(&b)) {
            for pc in x.children_named("PageContent") {
                if let Some(s) = pc.attr("Source") {
                    let w = pc.attr("Width").and_then(|v| v.parse().ok());
                    let h = pc.attr("Height").and_then(|v| v.parse().ok());
                    pages.push((resolve(&dir_of(d), s), w.zip(h)));
                }
            }
        }
    }
    if pages.is_empty() {
        let mut v: Vec<String> = names.iter().filter(|n| n.to_ascii_lowercase().ends_with(".fpage")).cloned().collect();
        v.sort_by_key(|n| n.chars().filter(|c| c.is_ascii_digit()).collect::<String>().parse::<u64>().unwrap_or(0));
        pages = v.into_iter().map(|p| (p, None)).collect();
    }
    info.add("Documents", docs.len().max(1).to_string());
    info.add("Pages", pages.len().to_string());
    // print ticket
    if let Some(pt) = names.iter().find(|n| n.to_ascii_lowercase().contains("printticket") || n.to_ascii_lowercase().ends_with("ticket.xml")) {
        if let Some(x) = zip.read(pt).and_then(|d| xml::parse(&d)) {
            let mut feats = Vec::new();
            x.find_all("Feature", &mut feats);
            let mut pt_info = InfoSection::new("Print ticket (XPS)");
            for f in feats {
                let name = f.attr("name").unwrap_or("").rsplit(':').next().unwrap_or("").to_string();
                let opt = f.child("Option").and_then(|o| o.attr("name")).map(|n| n.rsplit(':').next().unwrap_or(n).to_string());
                if let Some(o) = opt {
                    pt_info.add(name, o);
                }
            }
            if !pt_info.rows.is_empty() {
                doc.info.push(pt_info);
            }
        }
    }
    let mut ctx = Ctx { zip: &zip, page_dir: String::new(), fonts: HashMap::new(), images: HashMap::new(), resources: Vec::new(), warnings: Vec::new(), stats: BTreeMap::new() };
    for (k, (p, size)) in pages.iter().enumerate() {
        let Some(x) = zip.read(p).and_then(|d| xml::parse(&d)) else {
            doc.warn(format!("Page {} ({p}) could not be read", k + 1));
            continue;
        };
        let w = x.attr("Width").and_then(|v| v.parse::<f64>().ok()).or(size.map(|s| s.0)).unwrap_or(816.0);
        let h = x.attr("Height").and_then(|v| v.parse::<f64>().ok()).or(size.map(|s| s.1)).unwrap_or(1056.0);
        let mut page = Page::new((w * U) as f32, (h * U) as f32);
        page.label = paper_name(page.width, page.height).unwrap_or("Custom").to_string();
        ctx.page_dir = dir_of(p);
        let dir = ctx.page_dir.clone();
        ctx.resources.clear();
        let st = State { m: Matrix::identity(), clip: None, opacity: 1.0 };
        render_element(&mut ctx, &mut page, &x, &st, &dir, 0);
        doc.pages.push(page);
    }
    for (k, v) in &ctx.stats {
        info.add(k.clone(), v.to_string());
    }
    let fonts: Vec<String> = ctx.fonts.iter().map(|(p, f)| format!("{} — {}", p.rsplit('/').next().unwrap_or(p), f.as_ref().map(|x| x.0.name.clone()).unwrap_or_else(|| "unreadable".into()))).collect();
    let mut fi = InfoSection::new("Fonts (XPS)");
    for f in fonts {
        fi.add("Embedded font", f);
    }
    doc.info.push(info);
    if !fi.rows.is_empty() {
        doc.info.push(fi);
    }
    for w in ctx.warnings {
        doc.warn(w);
    }
    let mut parts = InfoSection::new("XPS package parts");
    for n in names.iter().take(500) {
        parts.add(n.clone(), String::new());
    }
    doc.info.push(parts);
}
