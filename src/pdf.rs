//! Write pages as a vector PDF (paths, strokes, clips and images).

use crate::doc::{Brush, Item, Page, RasterImage};
use std::collections::HashMap;
use std::fmt::Write as _;
use std::io::Write as _;
use std::sync::Arc;
use tiny_skia::{LineCap, LineJoin, Path, PathSegment, Transform};

struct Writer {
    out: Vec<u8>,
    offsets: Vec<usize>,
}

impl Writer {
    fn reserve(&mut self) -> usize {
        self.offsets.push(0);
        self.offsets.len()
    }
    fn begin(&mut self, id: usize) {
        self.offsets[id - 1] = self.out.len();
        let _ = write!(self.out, "{id} 0 obj\n");
    }
    fn obj(&mut self, id: usize, body: &str) {
        self.begin(id);
        self.out.extend_from_slice(body.as_bytes());
        self.out.extend_from_slice(b"\nendobj\n");
    }
    fn stream(&mut self, id: usize, dict: &str, data: &[u8], compress: bool) {
        self.begin(id);
        let data = if compress { deflate(data) } else { data.to_vec() };
        let _ = write!(self.out, "<< {dict} /Length {}{} >>\nstream\n", data.len(), if compress { " /Filter /FlateDecode" } else { "" });
        self.out.extend_from_slice(&data);
        self.out.extend_from_slice(b"\nendstream\nendobj\n");
    }
}

fn deflate(d: &[u8]) -> Vec<u8> {
    let mut e = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
    let _ = e.write_all(d);
    e.finish().unwrap_or_default()
}

fn n(v: f32) -> String {
    if !v.is_finite() {
        return "0".into();
    }
    let s = format!("{:.3}", v);
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" || s.is_empty() { "0".into() } else { s.to_string() }
}

fn path_ops(p: &Path, s: &mut String) {
    let mut last = (0.0f32, 0.0f32);
    for seg in p.segments() {
        match seg {
            PathSegment::MoveTo(a) => {
                let _ = write!(s, "{} {} m\n", n(a.x), n(a.y));
                last = (a.x, a.y);
            }
            PathSegment::LineTo(a) => {
                let _ = write!(s, "{} {} l\n", n(a.x), n(a.y));
                last = (a.x, a.y);
            }
            PathSegment::QuadTo(c, a) => {
                let c1 = (last.0 + 2.0 / 3.0 * (c.x - last.0), last.1 + 2.0 / 3.0 * (c.y - last.1));
                let c2 = (a.x + 2.0 / 3.0 * (c.x - a.x), a.y + 2.0 / 3.0 * (c.y - a.y));
                let _ = write!(s, "{} {} {} {} {} {} c\n", n(c1.0), n(c1.1), n(c2.0), n(c2.1), n(a.x), n(a.y));
                last = (a.x, a.y);
            }
            PathSegment::CubicTo(c1, c2, a) => {
                let _ = write!(s, "{} {} {} {} {} {} c\n", n(c1.x), n(c1.y), n(c2.x), n(c2.y), n(a.x), n(a.y));
                last = (a.x, a.y);
            }
            PathSegment::Close => s.push_str("h\n"),
        }
    }
}

fn cm(t: &Transform, s: &mut String) {
    if *t != Transform::identity() {
        let _ = write!(s, "{} {} {} {} {} {} cm\n", n(t.sx), n(t.ky), n(t.kx), n(t.sy), n(t.tx), n(t.ty));
    }
}

fn brush_colour(b: &Brush) -> (f32, f32, f32) {
    match b {
        Brush::Solid(c) => (c.0, c.1, c.2),
        Brush::Pattern { image, .. } => {
            // approximate tiled patterns by their average colour
            let px = image.rgba();
            let mut acc = [0f64; 4];
            for p in px.chunks_exact(4) {
                for i in 0..4 {
                    acc[i] += p[i] as f64;
                }
            }
            let a = acc[3].max(1.0);
            let k = (acc[3] / (px.len() / 4).max(1) as f64 / 255.0) as f32;
            let c = |v: f64| ((v / a) as f32 * k + (1.0 - k)).clamp(0.0, 1.0);
            (c(acc[0]), c(acc[1]), c(acc[2]))
        }
    }
}

struct ImgRef {
    id: usize,
    name: String,
}

fn write_image(w: &mut Writer, img: &RasterImage) -> usize {
    let px = img.rgba();
    let mut rgb = Vec::with_capacity((img.width * img.height * 3) as usize);
    let mut alpha = Vec::with_capacity((img.width * img.height) as usize);
    let mut has_alpha = false;
    for p in px.chunks_exact(4) {
        let a = p[3];
        if a != 255 {
            has_alpha = true;
        }
        if a == 0 {
            rgb.extend_from_slice(&[255, 255, 255]);
        } else {
            let f = 255.0 / a as f32;
            rgb.push((p[0] as f32 * f).min(255.0) as u8);
            rgb.push((p[1] as f32 * f).min(255.0) as u8);
            rgb.push((p[2] as f32 * f).min(255.0) as u8);
        }
        alpha.push(a);
    }
    let id = w.reserve();
    let smask = if has_alpha {
        let sid = w.reserve();
        w.stream(sid, &format!("/Type /XObject /Subtype /Image /Width {} /Height {} /ColorSpace /DeviceGray /BitsPerComponent 8", img.width, img.height), &alpha, true);
        format!(" /SMask {sid} 0 R")
    } else {
        String::new()
    };
    w.stream(id, &format!("/Type /XObject /Subtype /Image /Width {} /Height {} /ColorSpace /DeviceRGB /BitsPerComponent 8{smask}", img.width, img.height), &rgb, true);
    id
}

/// Build a PDF of `pages`. `title` goes into the document information.
pub fn write(pages: &[&Page], title: &str) -> Vec<u8> {
    let mut w = Writer { out: Vec::new(), offsets: Vec::new() };
    w.out.extend_from_slice(b"%PDF-1.4\n%\xE2\xE3\xCF\xD3\n");
    let catalog = w.reserve();
    let pages_id = w.reserve();
    let info = w.reserve();
    let mut images: HashMap<usize, ImgRef> = HashMap::new();
    let mut kids = Vec::new();
    for page in pages {
        let mut s = String::new();
        // page space is y-down points
        let _ = write!(s, "1 0 0 -1 0 {} cm\n", n(page.height));
        let mut used: Vec<(String, usize)> = Vec::new();
        let mut pats: Vec<(String, usize)> = Vec::new();
        for item in &page.items {
            s.push_str("q\n");
            let clip = match item {
                Item::Fill { clip, .. } | Item::Stroke { clip, .. } | Item::Image { clip, .. } => clip,
            };
            if let Some(c) = clip {
                for cp in c.iter() {
                    path_ops(&cp.path, &mut s);
                    s.push_str(if cp.even_odd { "W* n\n" } else { "W n\n" });
                }
            }
            match item {
                Item::Fill { path, transform, brush, even_odd, .. } => {
                    if let Brush::Pattern { image, transform: pt } = brush {
                        let key = Arc::as_ptr(image) as usize;
                        if !images.contains_key(&key) {
                            let id = write_image(&mut w, image);
                            images.insert(key, ImgRef { id, name: format!("Im{}", images.len() + 1) });
                        }
                        let img = &images[&key];
                        // pattern space -> default (y-up) page space
                        let m = pt.post_concat(Transform::from_row(1.0, 0.0, 0.0, -1.0, 0.0, page.height));
                        let pid = w.reserve();
                        let body = format!("q {} 0 0 -{} 0 {} cm /I Do Q", image.width, image.height, image.height);
                        w.stream(
                            pid,
                            &format!(
                                "/Type /Pattern /PatternType 1 /PaintType 1 /TilingType 1 /BBox [0 0 {} {}] /XStep {} /YStep {} /Matrix [{} {} {} {} {} {}] /Resources << /XObject << /I {} 0 R >> >>",
                                image.width, image.height, image.width, image.height, n(m.sx), n(m.ky), n(m.kx), n(m.sy), n(m.tx), n(m.ty), img.id
                            ),
                            body.as_bytes(),
                            false,
                        );
                        let name = format!("P{}", pats.len() + 1);
                        let _ = write!(s, "/Pattern cs /{name} scn\n");
                        pats.push((name, pid));
                    } else {
                        let (r, g, b) = brush_colour(brush);
                        let _ = write!(s, "{} {} {} rg\n", n(r), n(g), n(b));
                    }
                    cm(transform, &mut s);
                    path_ops(path, &mut s);
                    s.push_str(if *even_odd { "f*\n" } else { "f\n" });
                }
                Item::Stroke { path, transform, brush, style, .. } => {
                    let (r, g, b) = brush_colour(brush);
                    let _ = write!(s, "{} {} {} RG\n", n(r), n(g), n(b));
                    cm(transform, &mut s);
                    let sc = ((transform.sx * transform.sy - transform.kx * transform.ky).abs()).sqrt();
                    let width = if style.width * sc < 0.25 { 0.0 } else { style.width };
                    let _ = write!(
                        s,
                        "{} w {} J {} j {} M\n",
                        n(width),
                        match style.cap {
                            LineCap::Butt => 0,
                            LineCap::Round => 1,
                            LineCap::Square => 2,
                        },
                        match style.join {
                            LineJoin::Miter | LineJoin::MiterClip => 0,
                            LineJoin::Round => 1,
                            LineJoin::Bevel => 2,
                        },
                        n(style.miter.max(1.0))
                    );
                    if let Some((arr, off)) = &style.dash {
                        let a: Vec<String> = arr.iter().map(|v| n(*v)).collect();
                        let _ = write!(s, "[{}] {} d\n", a.join(" "), n(*off));
                    }
                    path_ops(path, &mut s);
                    s.push_str("S\n");
                }
                Item::Image { image, transform, .. } => {
                    let key = Arc::as_ptr(image) as usize;
                    if !images.contains_key(&key) {
                        let id = write_image(&mut w, image);
                        images.insert(key, ImgRef { id, name: format!("Im{}", images.len() + 1) });
                    }
                    let r = &images[&key];
                    if !used.iter().any(|(nm, _)| *nm == r.name) {
                        used.push((r.name.clone(), r.id));
                    }
                    cm(transform, &mut s);
                    let _ = write!(s, "{} 0 0 -{} 0 {} cm /{} Do\n", image.width, image.height, image.height, r.name);
                }
            }
            s.push_str("Q\n");
        }
        let content = w.reserve();
        w.stream(content, "", s.as_bytes(), true);
        let mut xo = String::new();
        for (nm, id) in &used {
            let _ = write!(xo, "/{nm} {id} 0 R ");
        }
        let mut pa = String::new();
        for (nm, id) in &pats {
            let _ = write!(pa, "/{nm} {id} 0 R ");
        }
        let pid = w.reserve();
        w.obj(
            pid,
            &format!(
                "<< /Type /Page /Parent {pages_id} 0 R /MediaBox [0 0 {} {}] /Resources << /XObject << {xo}>> /Pattern << {pa}>> >> /Contents {content} 0 R >>",
                n(page.width),
                n(page.height)
            ),
        );
        kids.push(pid);
    }
    let k: Vec<String> = kids.iter().map(|k| format!("{k} 0 R")).collect();
    w.obj(pages_id, &format!("<< /Type /Pages /Kids [{}] /Count {} >>", k.join(" "), kids.len()));
    w.obj(catalog, &format!("<< /Type /Catalog /Pages {pages_id} 0 R >>"));
    let esc: String = title.chars().filter(|c| c.is_ascii() && !c.is_control()).map(|c| if matches!(c, '(' | ')' | '\\') { format!("\\{c}") } else { c.to_string() }).collect();
    w.obj(info, &format!("<< /Title ({esc}) /Producer (Print File Viewer) >>"));
    let xref = w.out.len();
    let _ = write!(w.out, "xref\n0 {}\n0000000000 65535 f \n", w.offsets.len() + 1);
    for o in &w.offsets {
        let _ = write!(w.out, "{:010} 00000 n \n", o);
    }
    let _ = write!(w.out, "trailer\n<< /Size {} /Root {catalog} 0 R /Info {info} 0 R >>\nstartxref\n{xref}\n%%EOF\n", w.offsets.len() + 1);
    w.out
}
