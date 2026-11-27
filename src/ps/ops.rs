//! PostScript operators.

use super::file;
use super::obj::*;
use super::text::ShowOpts;
use super::*;
use crate::fonts;
use std::rc::Rc;

type OpFn = fn(&mut Interp) -> R;

macro_rules! ops {
    ($($name:literal => |$i:ident| $body:expr),* $(,)?) => {
        vec![ $( ($name, { #[allow(unused_mut, unused_variables)] fn f($i: &mut Interp) -> R { $body } f as OpFn }) ),* ]
    };
}

fn int_or_real(v: f64) -> Obj {
    if v.fract() == 0.0 && v.abs() <= i32::MAX as f64 {
        Obj::Int(v as i64)
    } else {
        Obj::Real(v)
    }
}

fn arith(i: &mut Interp, fi: fn(i64, i64) -> Option<i64>, fr: fn(f64, f64) -> f64) -> R {
    let b = i.pop()?;
    let a = i.pop()?;
    match (&a, &b) {
        (Obj::Int(x), Obj::Int(y)) => match fi(*x, *y) {
            Some(v) if v.abs() <= i32::MAX as i64 => i.push(Obj::Int(v)),
            _ => i.push(Obj::Real(fr(*x as f64, *y as f64))),
        },
        _ => match (a.num(), b.num()) {
            (Some(x), Some(y)) => i.push(Obj::Real(fr(x, y))),
            _ => return err("typecheck", "arithmetic"),
        },
    }
    Ok(())
}

pub fn obj_eq(i: &Interp, a: &Obj, b: &Obj) -> bool {
    match (a, b) {
        (Obj::Int(x), Obj::Int(y)) => x == y,
        (x, y) if x.num().is_some() && y.num().is_some() => x.num() == y.num(),
        (Obj::Bool(x), Obj::Bool(y)) => x == y,
        (Obj::Null, Obj::Null) | (Obj::Mark, Obj::Mark) => true,
        (Obj::Name(x, _), Obj::Name(y, _)) => x == y,
        (Obj::Str(..) | Obj::Name(..), Obj::Str(..) | Obj::Name(..)) => i.obj_to_bytes(a) == i.obj_to_bytes(b),
        (Obj::Arr(x, _), Obj::Arr(y, _)) => x.ptr() == y.ptr() && x.len == y.len,
        (Obj::Dict(x), Obj::Dict(y)) => Rc::ptr_eq(x, y),
        (Obj::Op(x), Obj::Op(y)) => x == y,
        (Obj::File(x), Obj::File(y)) => Rc::ptr_eq(x, y),
        (Obj::FontId(x), Obj::FontId(y)) => x == y,
        (Obj::Save(x), Obj::Save(y)) => x == y,
        _ => false,
    }
}

fn compare(i: &mut Interp) -> R<std::cmp::Ordering> {
    let b = i.pop()?;
    let a = i.pop()?;
    if let (Some(x), Some(y)) = (a.num(), b.num()) {
        return Ok(x.partial_cmp(&y).unwrap_or(std::cmp::Ordering::Equal));
    }
    match (&a, &b) {
        (Obj::Str(x, _), Obj::Str(y, _)) => Ok(x.bytes().cmp(&y.bytes())),
        _ => err("typecheck", "comparison"),
    }
}

fn fmt_real(v: f64) -> String {
    if v.fract() == 0.0 && v.abs() < 1e15 {
        format!("{:.1}", v)
    } else {
        let s = format!("{}", (v * 1e6).round() / 1e6);
        s
    }
}

pub fn obj_text(i: &Interp, o: &Obj, syntax: bool, depth: u32) -> String {
    match o {
        Obj::Int(v) => v.to_string(),
        Obj::Real(v) => fmt_real(*v),
        Obj::Bool(b) => b.to_string(),
        Obj::Null => if syntax { "null".into() } else { "--nostringval--".into() },
        Obj::Mark => "-mark-".into(),
        Obj::Name(n, x) => {
            let s = i.name_str(*n);
            if syntax && !x {
                format!("/{s}")
            } else {
                s
            }
        }
        Obj::Str(s, _) => {
            let t = String::from_utf8_lossy(&s.bytes()).to_string();
            if syntax {
                format!("({t})")
            } else {
                t
            }
        }
        Obj::Arr(a, x) => {
            if !syntax || depth > 3 {
                return "--nostringval--".into();
            }
            let inner: Vec<String> = a.to_vec().iter().take(50).map(|e| obj_text(i, e, true, depth + 1)).collect();
            if *x {
                format!("{{{}}}", inner.join(" "))
            } else {
                format!("[{}]", inner.join(" "))
            }
        }
        Obj::Dict(_) => "-dict-".into(),
        Obj::Op(k) => format!("--{}--", i.ops[*k as usize].0),
        Obj::File(_) => "-file-".into(),
        Obj::Save(_) => "-save-".into(),
        Obj::FontId(_) => "-fontid-".into(),
        Obj::GState(_) => "-gstate-".into(),
    }
}

fn out(i: &mut Interp, s: &str) {
    if i.stats.output.len() < 65536 {
        i.stats.output.push_str(s);
    }
}

fn fill_matrix(a: &PArr, m: &Matrix) -> R {
    if a.len != 6 {
        return err("rangecheck", "matrix");
    }
    for (k, v) in m.to_array().iter().enumerate() {
        a.set(k, Obj::Real(*v));
    }
    Ok(())
}

fn cur_user(i: &Interp) -> R<(f64, f64)> {
    let c = i.gs.path.cur.ok_or(PsError::Err("nocurrentpoint", String::new()))?;
    let inv = i.gs.ctm.invert().ok_or(PsError::Err("undefinedresult", String::new()))?;
    Ok(inv.apply(c.0, c.1))
}

fn transform_op(i: &mut Interp, inverse: bool, delta: bool) -> R {
    let m = if i.top_is_matrix() { i.pop_matrix()? } else { i.gs.ctm };
    let m = if inverse { m.invert().ok_or(PsError::Err("undefinedresult", String::new()))? } else { m };
    let y = i.pop_num()?;
    let x = i.pop_num()?;
    let (a, b) = if delta { m.apply_delta(x, y) } else { m.apply(x, y) };
    i.push(Obj::Real(a));
    i.push(Obj::Real(b));
    Ok(())
}

fn matrix_modify(i: &mut Interp, f: fn(&mut Interp) -> R<Matrix>) -> R {
    if i.top_is_matrix() {
        let a = i.pop_arr()?;
        let m = f(i)?;
        fill_matrix(&a, &m)?;
        i.push(Obj::Arr(a, false));
    } else {
        let m = f(i)?;
        i.gs.ctm = m.then(&i.gs.ctm);
    }
    Ok(())
}

fn rect_args(i: &mut Interp) -> R<Vec<[f64; 4]>> {
    let top = i.pop()?;
    match top {
        Obj::Arr(a, _) => {
            let v: Vec<f64> = a.to_vec().iter().filter_map(|o| o.num()).collect();
            Ok(v.chunks(4).filter(|c| c.len() == 4).map(|c| [c[0], c[1], c[2], c[3]]).collect())
        }
        Obj::Str(s, _) => {
            // encoded number string: not supported, ignore
            let _ = s;
            Ok(Vec::new())
        }
        o => {
            let h = o.num().ok_or(PsError::Err("typecheck", "rect".into()))?;
            let w = i.pop_num()?;
            let y = i.pop_num()?;
            let x = i.pop_num()?;
            Ok(vec![[x, y, w, h]])
        }
    }
}

fn rect_path(i: &Interp, rects: &[[f64; 4]]) -> PathAcc {
    let m = i.gs.ctm;
    let mut p = PathAcc::new();
    for r in rects {
        let pts = [(r[0], r[1]), (r[0] + r[2], r[1]), (r[0] + r[2], r[1] + r[3]), (r[0], r[1] + r[3])];
        for (k, (x, y)) in pts.iter().enumerate() {
            let (dx, dy) = m.apply(*x, *y);
            if k == 0 {
                p.move_to(dx, dy);
            } else {
                p.line_to(dx, dy);
            }
        }
        p.close();
    }
    p
}

fn parse_cspace(i: &mut Interp, o: &Obj) -> R<ColorSpace> {
    let (fam, arr): (Vec<u8>, Vec<Obj>) = match o {
        Obj::Name(n, _) => (i.names.get(*n).to_vec(), Vec::new()),
        Obj::Arr(a, _) => {
            let v = a.to_vec();
            let f = v.first().and_then(|x| i.obj_to_bytes(x)).unwrap_or_default();
            (f, v)
        }
        _ => return err("typecheck", "setcolorspace"),
    };
    Ok(match fam.as_slice() {
        b"DeviceGray" | b"CalGray" | b"CIEBasedA" | b"G" => ColorSpace::Gray,
        b"DeviceRGB" | b"CalRGB" | b"CIEBasedABC" | b"CIEBasedDEF" | b"RGB" => ColorSpace::Rgb,
        b"DeviceCMYK" | b"CIEBasedDEFG" | b"CMYK" => ColorSpace::Cmyk,
        b"Pattern" => {
            let under = match arr.get(1) {
                Some(u) => Some(Rc::new(parse_cspace(i, u)?)),
                None => None,
            };
            ColorSpace::Pattern { under }
        }
        b"Indexed" | b"I" => {
            let base = parse_cspace(i, arr.get(1).unwrap_or(&Obj::Null))?;
            let hival = arr.get(2).and_then(|o| o.num()).unwrap_or(0.0) as usize;
            let n = base.ncomps();
            let lookup = match arr.get(3) {
                Some(Obj::Str(s, _)) => s.bytes(),
                Some(p @ Obj::Arr(_, true)) => {
                    let mut v = Vec::new();
                    for k in 0..=hival {
                        let b = i.os.len();
                        i.push(Obj::Int(k as i64));
                        i.call(p.clone())?;
                        let mut c = Vec::new();
                        while i.os.len() > b {
                            c.push(i.pop_num().unwrap_or(0.0));
                        }
                        c.reverse();
                        c.resize(n, 0.0);
                        v.extend(c.iter().map(|x| (x * 255.0).round().clamp(0.0, 255.0) as u8));
                    }
                    v
                }
                _ => Vec::new(),
            };
            ColorSpace::Indexed { base: Rc::new(base), hival, lookup: Rc::new(lookup) }
        }
        b"Separation" | b"DeviceN" => {
            let n = if fam == b"DeviceN" {
                match arr.get(1) {
                    Some(Obj::Arr(a, _)) => a.len,
                    _ => 1,
                }
            } else {
                1
            };
            let alt = parse_cspace(i, arr.get(2).unwrap_or(&Obj::Null)).unwrap_or(ColorSpace::Gray);
            let tint = arr.get(3).cloned().unwrap_or(Obj::Null);
            ColorSpace::Separation { alt: Rc::new(alt), tint, n }
        }
        b"ICCBased" => {
            let n = match arr.get(1) {
                Some(Obj::Dict(d)) => {
                    let k = i.name("N");
                    i.dget(d, k).and_then(|o| o.num()).unwrap_or(3.0) as usize
                }
                _ => 3,
            };
            match n {
                1 => ColorSpace::Gray,
                4 => ColorSpace::Cmyk,
                _ => ColorSpace::Rgb,
            }
        }
        _ => {
            i.note_unsupported(&format!("color space {}", String::from_utf8_lossy(&fam)));
            ColorSpace::Gray
        }
    })
}

fn set_cspace(i: &mut Interp, cs: ColorSpace, obj: Obj) -> R {
    let init: Vec<f64> = match &cs {
        ColorSpace::Cmyk => vec![0.0, 0.0, 0.0, 1.0],
        ColorSpace::Separation { n, .. } => vec![1.0; *n],
        c => vec![0.0; c.ncomps()],
    };
    i.gs.cspace = cs;
    i.gs.cspace_obj = Some(obj);
    i.gs.ccomps = init;
    i.gs.pattern = None;
    i.set_color_from_comps()
}

fn bind_proc(i: &Interp, a: &PArr, seen: &mut std::collections::HashSet<usize>, depth: u32) {
    if depth > 50 || !seen.insert(a.ptr()) {
        return;
    }
    for k in 0..a.len {
        match a.get(k) {
            Some(Obj::Name(n, true)) => {
                if let Some(Obj::Op(op)) = i.lookup_name(n) {
                    a.set(k, Obj::Op(op));
                }
            }
            Some(Obj::Arr(sub, true)) => bind_proc(i, &sub, seen, depth + 1),
            _ => {}
        }
    }
}

/// Read up to `need` bytes from an image data source.
fn read_source(i: &mut Interp, src: &Obj, need: usize, string_pos: &mut usize) -> R<Vec<u8>> {
    let mut outv = Vec::with_capacity(need);
    match src {
        Obj::Str(s, _) => {
            let b = s.bytes();
            if b.is_empty() {
                return Ok(outv);
            }
            while outv.len() < need {
                let take = (need - outv.len()).min(b.len() - *string_pos % b.len());
                let st = *string_pos % b.len();
                outv.extend_from_slice(&b[st..st + take]);
                *string_pos += take;
                if *string_pos >= b.len() && outv.len() < need && b.len() < 16 {
                    // tiny strings repeat (L1 behaviour); otherwise stop
                }
                if *string_pos >= b.len() * 64 && outv.len() < need {
                    break;
                }
            }
        }
        Obj::File(f) => {
            let mut fs = f.borrow_mut();
            while outv.len() < need {
                match fs.read() {
                    Some(b) => outv.push(b),
                    None => break,
                }
            }
        }
        p => {
            let mut empties = 0;
            while outv.len() < need {
                let base = i.os.len();
                i.call(p.clone())?;
                if i.os.len() <= base {
                    break;
                }
                let r = i.pop()?;
                i.os.truncate(base);
                match r {
                    Obj::Str(s, _) => {
                        if s.len == 0 {
                            empties += 1;
                            if empties > 0 {
                                break;
                            }
                        }
                        outv.extend_from_slice(&s.bytes());
                    }
                    _ => break,
                }
            }
        }
    }
    Ok(outv)
}

struct ImageParams {
    w: usize,
    h: usize,
    bpc: usize,
    matrix: Matrix,
    sources: Vec<Obj>,
    multi: bool,
    decode: Vec<f64>,
    cs: ColorSpace,
    mask: bool,
}

fn do_image(i: &mut Interp, p: ImageParams) -> R {
    let ncomp = if p.mask { 1 } else { p.cs.ncomps() };
    if p.w == 0 || p.h == 0 {
        return Ok(());
    }
    if p.w > 30000 || p.h > 30000 || p.w * p.h > 120_000_000 {
        return err("limitcheck", "image too large");
    }
    let bpc = p.bpc.max(1);
    i.stats.images += 1;
    let kind = format!("{}{} × {} {}-bit {}", if p.mask { "Mask " } else { "" }, p.w, p.h, bpc, if p.mask { "stencil".to_string() } else { p.cs.name().to_string() });
    let kind_short = format!("{}{}-bit {}", if p.mask { "Mask " } else { "" }, bpc, if p.mask { "stencil" } else { p.cs.name() });
    *i.stats.image_kinds.entry(kind_short).or_insert(0) += 1;
    let _ = kind;
    // gather samples
    let comp_per_src = if p.multi { 1 } else { ncomp };
    let row_bytes = (p.w * bpc * comp_per_src).div_ceil(8);
    let mut planes: Vec<Vec<u8>> = Vec::new();
    for s in &p.sources {
        let mut pos = 0;
        planes.push(read_source(i, s, row_bytes * p.h, &mut pos)?);
    }
    let maxv = ((1u64 << bpc) - 1) as f64;
    let sample = |plane: &[u8], row: usize, idx: usize| -> u32 {
        let base = row * row_bytes;
        match bpc {
            8 => plane.get(base + idx).copied().unwrap_or(0) as u32,
            16 => plane.get(base + idx * 2).copied().unwrap_or(0) as u32 * 257 / 257,
            _ => {
                let bit = idx * bpc;
                let byte = plane.get(base + bit / 8).copied().unwrap_or(0) as u32;
                (byte >> (8 - bpc - bit % 8)) & ((1 << bpc) - 1)
            }
        }
    };
    let mut rgba = vec![0u8; p.w * p.h * 4];
    let color = i.gs.color;
    let dec = |k: usize, v: u32| -> f64 {
        let d0 = p.decode.get(2 * k).copied().unwrap_or(0.0);
        let d1 = p.decode.get(2 * k + 1).copied().unwrap_or(1.0);
        let maxs = if bpc == 16 { 255.0 } else { maxv };
        d0 + v as f64 * (d1 - d0) / maxs
    };
    // fast LUT for single-component spaces
    let mut lut: Option<Vec<[u8; 3]>> = None;
    if !p.mask && ncomp == 1 && bpc <= 8 {
        let mut l = Vec::with_capacity(1 << bpc);
        let cs = p.cs.clone();
        for v in 0..(1u32 << bpc) {
            let c = i.convert_color(&cs, &[dec(0, v)])?;
            l.push([(c.0 * 255.0).round() as u8, (c.1 * 255.0).round() as u8, (c.2 * 255.0).round() as u8]);
        }
        lut = Some(l);
    }
    let mut cache: std::collections::HashMap<Vec<u32>, [u8; 3]> = std::collections::HashMap::new();
    let mut comps = vec![0u32; ncomp];
    for row in 0..p.h {
        for x in 0..p.w {
            for k in 0..ncomp {
                comps[k] = if p.multi { sample(&planes[k.min(planes.len() - 1)], row, x) } else { sample(&planes[0], row, x * ncomp + k) };
            }
            let o = (row * p.w + x) * 4;
            if p.mask {
                if dec(0, comps[0]) < 0.5 {
                    rgba[o] = (color.0 * 255.0) as u8;
                    rgba[o + 1] = (color.1 * 255.0) as u8;
                    rgba[o + 2] = (color.2 * 255.0) as u8;
                    rgba[o + 3] = 255;
                }
                continue;
            }
            let c = if let Some(l) = &lut {
                l[comps[0] as usize]
            } else {
                match &p.cs {
                    ColorSpace::Rgb => [(dec(0, comps[0]) * 255.0).clamp(0.0, 255.0) as u8, (dec(1, comps[1]) * 255.0).clamp(0.0, 255.0) as u8, (dec(2, comps[2]) * 255.0).clamp(0.0, 255.0) as u8],
                    ColorSpace::Cmyk => {
                        let c = Rgb::from_cmyk(dec(0, comps[0]) as f32, dec(1, comps[1]) as f32, dec(2, comps[2]) as f32, dec(3, comps[3]) as f32);
                        [(c.0 * 255.0) as u8, (c.1 * 255.0) as u8, (c.2 * 255.0) as u8]
                    }
                    cs => {
                        if let Some(c) = cache.get(&comps) {
                            *c
                        } else {
                            let v: Vec<f64> = (0..ncomp).map(|k| dec(k, comps[k])).collect();
                            let cs2 = cs.clone();
                            let c = i.convert_color(&cs2, &v)?;
                            let r = [(c.0 * 255.0) as u8, (c.1 * 255.0) as u8, (c.2 * 255.0) as u8];
                            if cache.len() < 65536 {
                                cache.insert(comps.clone(), r);
                            }
                            r
                        }
                    }
                }
            };
            rgba[o] = c[0];
            rgba[o + 1] = c[1];
            rgba[o + 2] = c[2];
            rgba[o + 3] = 255;
        }
    }
    if i.no_paint > 0 {
        return Ok(());
    }
    let Some(inv) = p.matrix.invert() else { return Ok(()) };
    let t = inv.then(&i.gs.ctm).then(&i.dev_to_page());
    let (img, t) = RasterImage::fitted(p.w as u32, p.h as u32, rgba, &t);
    i.page.push(Item::Image { image: Arc::new(img), transform: t.to_ts(), clip: i.gs.clip.clone() });
    i.page_dirty = true;
    Ok(())
}

fn image_op(i: &mut Interp, mask: bool, color: bool) -> R {
    if matches!(i.top(), Some(Obj::Dict(_))) && !color {
        let d = i.pop_dict()?;
        let g = |i: &mut Interp, n: &str| {
            let k = i.name(n);
            i.dget(&d, k)
        };
        let w = g(i, "Width").and_then(|o| o.num()).unwrap_or(0.0) as usize;
        let h = g(i, "Height").and_then(|o| o.num()).unwrap_or(0.0) as usize;
        let bpc = if mask { 1 } else { g(i, "BitsPerComponent").and_then(|o| o.num()).unwrap_or(8.0) as usize };
        let matrix = match g(i, "ImageMatrix") {
            Some(Obj::Arr(a, _)) => arr_matrix(&a).unwrap_or_default(),
            _ => Matrix::identity(),
        };
        let multi = matches!(g(i, "MultipleDataSources"), Some(Obj::Bool(true)));
        let sources = match g(i, "DataSource") {
            Some(Obj::Arr(a, false)) if multi => a.to_vec(),
            Some(o) => vec![o],
            None => return err("undefined", "DataSource"),
        };
        let cs = i.gs.cspace.clone();
        let ncomp = if mask { 1 } else { cs.ncomps() };
        let decode = match g(i, "Decode") {
            Some(Obj::Arr(a, _)) => a.to_vec().iter().filter_map(|o| o.num()).collect(),
            _ => {
                if let ColorSpace::Indexed { .. } = cs {
                    vec![0.0, ((1u64 << bpc) - 1) as f64]
                } else {
                    (0..ncomp).flat_map(|_| [0.0, 1.0]).collect()
                }
            }
        };
        let it = g(i, "ImageType").and_then(|o| o.num()).unwrap_or(1.0) as i64;
        if it != 1 {
            // masked images (types 3/4): draw the data image only
            i.note_unsupported("masked images (ImageType 3/4)");
            if let Some(Obj::Dict(dd)) = g(i, "DataDict") {
                i.push(Obj::Dict(dd));
                return image_op(i, mask, false);
            }
        }
        return do_image(i, ImageParams { w, h, bpc, matrix, sources, multi, decode, cs, mask });
    }
    if color {
        let ncomp = i.pop_int()? as usize;
        let multi = i.pop_bool()?;
        let mut sources = Vec::new();
        let ns = if multi { ncomp } else { 1 };
        for _ in 0..ns {
            sources.push(i.pop()?);
        }
        sources.reverse();
        let matrix = i.pop_matrix()?;
        let bpc = i.pop_int()? as usize;
        let h = i.pop_int()? as usize;
        let w = i.pop_int()? as usize;
        let cs = match ncomp {
            1 => ColorSpace::Gray,
            4 => ColorSpace::Cmyk,
            _ => ColorSpace::Rgb,
        };
        let decode = (0..ncomp).flat_map(|_| [0.0, 1.0]).collect();
        return do_image(i, ImageParams { w, h, bpc, matrix, sources, multi, decode, cs, mask: false });
    }
    let src = i.pop()?;
    let matrix = i.pop_matrix()?;
    let third = i.pop()?;
    let h = i.pop_int()? as usize;
    let w = i.pop_int()? as usize;
    if mask {
        let polarity = matches!(third, Obj::Bool(true));
        let decode = if polarity { vec![1.0, 0.0] } else { vec![0.0, 1.0] };
        do_image(i, ImageParams { w, h, bpc: 1, matrix, sources: vec![src], multi: false, decode, cs: ColorSpace::Gray, mask: true })
    } else {
        let bpc = third.num().unwrap_or(8.0) as usize;
        do_image(i, ImageParams { w, h, bpc, matrix, sources: vec![src], multi: false, decode: vec![0.0, 1.0], cs: ColorSpace::Gray, mask: false })
    }
}

fn show_op(i: &mut Interp, opts: ShowOpts) -> R {
    let s = i.pop_str()?;
    if let Some(f) = i.gs.font.clone() {
        i.note_font_use(&f);
    }
    i.show(&s.bytes(), opts)?;
    Ok(())
}

fn do_showpage(i: &mut Interp) -> R {
    // EndPage procedure support
    let k_end = i.name(".EndPage");
    let sd = i.systemdict.clone();
    let mut emit = true;
    if let Some(p) = i.dget(&sd, k_end) {
        let n = i.doc.pages.len() as i64;
        i.push(Obj::Int(n));
        i.push(Obj::Int(0));
        if i.call(p).is_ok() {
            emit = !matches!(i.pop(), Ok(Obj::Bool(false)));
        }
    }
    if emit {
        i.emit_page(false);
    } else {
        i.page.items.clear();
        i.page_dirty = false;
    }
    i.initgraphics();
    let k_begin = i.name(".BeginPage");
    if let Some(p) = i.dget(&sd, k_begin) {
        let n = i.doc.pages.len() as i64;
        i.push(Obj::Int(n));
        let _ = i.call(p);
    }
    Ok(())
}

fn obj_summary(i: &Interp, o: &Obj) -> String {
    match o {
        Obj::Arr(a, false) => format!("[{}]", a.to_vec().iter().take(8).map(|e| obj_text(i, e, true, 2)).collect::<Vec<_>>().join(" ")),
        Obj::Dict(_) => "<<…>>".into(),
        Obj::Arr(_, true) => "{…}".into(),
        _ => obj_text(i, o, true, 0),
    }
}

fn setpagedevice(i: &mut Interp) -> R {
    let d = i.pop_dict()?;
    let items: Vec<(Key, Obj)> = d.borrow().map.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
    let mut desc = Vec::new();
    let mut new_size: Option<(f64, f64)> = None;
    for (k, v) in &items {
        let Key::Name(n) = k else { continue };
        let name = i.name_str(*n);
        match name.as_str() {
            "PageSize" => {
                if let Obj::Arr(a, _) = v {
                    let w = a.get(0).and_then(|o| o.num()).unwrap_or(612.0);
                    let h = a.get(1).and_then(|o| o.num()).unwrap_or(792.0);
                    if w > 1.0 && h > 1.0 {
                        new_size = Some((w, h));
                    }
                }
            }
            "NumCopies" => {
                if let Some(c) = v.num() {
                    i.stats.copies = Some(c as i64);
                }
            }
            "BeginPage" | "EndPage" => {
                let key = i.name(&format!(".{name}"));
                let sd = i.systemdict.clone();
                i.dput(&sd, key, v.clone());
            }
            "Install" => {}
            _ => {}
        }
        if !matches!(name.as_str(), "Install" | "BeginPage" | "EndPage") {
            let s = obj_summary(i, v);
            desc.push(format!("{name}={s}"));
            i.pagedev.entries.insert(name.clone(), s);
        }
    }
    desc.sort();
    let line = desc.join("  ");
    if !line.is_empty() && i.stats.setpagedevice.last() != Some(&line) && i.stats.setpagedevice.len() < 500 {
        i.stats.setpagedevice.push(line);
    }
    if let Some((w, h)) = new_size {
        if (w - i.pagedev.width).abs() > 0.5 || (h - i.pagedev.height).abs() > 0.5 || !i.page_dirty {
            i.pagedev.width = w;
            i.pagedev.height = h;
            i.page = Page::new(w as f32, h as f32);
            i.page_dirty = false;
        }
    }
    i.initgraphics();
    Ok(())
}

fn currentpagedevice(i: &mut Interp) -> R {
    let d = new_dict(30);
    let entries: Vec<(&str, Obj)> = vec![
        ("PageSize", Obj::Arr(PArr::new(vec![Obj::Real(i.pagedev.width), Obj::Real(i.pagedev.height)]), false)),
        ("HWResolution", Obj::Arr(PArr::new(vec![Obj::Real(600.0), Obj::Real(600.0)]), false)),
        ("ImagingBBox", Obj::Null),
        ("NumCopies", Obj::Int(i.stats.copies.unwrap_or(1))),
        ("Duplex", Obj::Bool(false)),
        ("Tumble", Obj::Bool(false)),
        ("Orientation", Obj::Int(0)),
        ("ManualFeed", Obj::Bool(false)),
        ("Collate", Obj::Bool(false)),
        ("MediaColor", Obj::Null),
        ("MediaType", Obj::Null),
        ("MediaWeight", Obj::Null),
        ("OutputType", Obj::Null),
        ("PageCount", Obj::Int(i.doc.pages.len() as i64)),
        ("Policies", Obj::Dict(new_dict(5))),
        ("InputAttributes", Obj::Dict(new_dict(5))),
        ("OutputAttributes", Obj::Dict(new_dict(5))),
        ("Margins", Obj::Arr(PArr::new(vec![Obj::Int(0), Obj::Int(0)]), false)),
        ("PageOffset", Obj::Arr(PArr::new(vec![Obj::Int(0), Obj::Int(0)]), false)),
        ("BeginPage", Obj::Arr(PArr::new(vec![Obj::Name(i.name("pop"), true)]), true)),
        ("EndPage", Obj::Arr(PArr::new(vec![Obj::Name(i.name("exch"), true), Obj::Name(i.name("pop"), true), Obj::Int(2), Obj::Name(i.name("ne"), true)]), true)),
        ("Install", Obj::Arr(PArr::new(vec![]), true)),
    ];
    for (k, v) in entries {
        let n = i.name(k);
        i.dput(&d, n, v);
    }
    i.push(Obj::Dict(d));
    Ok(())
}

fn find_font(i: &mut Interp, key: Obj) -> R<PDict> {
    let k = i.key(&key)?;
    if let Some(Obj::Dict(d)) = i.font_dir.borrow().map.get(&k).cloned() {
        return Ok(d);
    }
    // font resources defined via defineresource
    if let Some(cat) = i.resources.get(b"Font".as_slice()).cloned() {
        if let Some(Obj::Dict(d)) = cat.borrow().map.get(&k).cloned() {
            return Ok(d);
        }
    }
    let name = i.obj_to_bytes(&key).unwrap_or_default();
    let d = i.substitute_font(&name);
    let fd = i.font_dir.clone();
    fd.borrow_mut().map.insert(k, Obj::Dict(d.clone()));
    let nm = String::from_utf8_lossy(&name).to_string();
    let sub = fonts::system_font(&nm);
    i.stats.fonts_used.entry(nm).or_insert_with(|| format!("Not embedded — substituted with {}", sub.file));
    Ok(d)
}

fn define_font(i: &mut Interp, key: Obj, d: PDict) -> R {
    let k = i.key(&key)?;
    let fid = i.names_cache.fid;
    if i.dget(&d, fid).is_none() {
        i.next_fid += 1;
        i.dput(&d, fid, Obj::FontId(i.next_fid));
    }
    let name = i.obj_to_bytes(&key).map(|b| String::from_utf8_lossy(&b).to_string()).unwrap_or_default();
    let ft = i.font_type(&d);
    let desc = match ft {
        0 => "Type 0 (composite)",
        1 => {
            if i.dget(&d, i.names_cache.sub).is_some() {
                "Not embedded (re-encoded copy of a substituted font)"
            } else if i.dget(&d, i.names_cache.char_strings).is_some() {
                "Type 1"
            } else {
                "Type 1 (no outlines)"
            }
        }
        2 => "Type 2 (CFF) — substituted",
        3 => "Type 3",
        42 => "Type 42 (TrueType)",
        101 | 100 => "CIDFontType 0",
        102 => "CIDFontType 2",
        _ => "other",
    };
    let kdn = i.name(".DefName");
    if i.dget(&d, kdn).is_none() {
        i.dput(&d, kdn, key.clone());
    }
    if ft == 2 {
        let sub = i.names_cache.sub;
        i.dput(&d, sub, Obj::Bool(true));
    }
    if !i.stats.fonts_defined.contains_key(&name) && i.stats.fonts_defined.len() < 1000 {
        i.stats.fonts_defined.insert(name, desc.to_string());
    }
    let fd = i.font_dir.clone();
    fd.borrow_mut().map.insert(k, Obj::Dict(d.clone()));
    Ok(())
}

fn copy_dict(d: &PDict) -> PDict {
    let b = d.borrow();
    Rc::new(std::cell::RefCell::new(DictData { map: b.map.clone(), cap: b.cap }))
}

fn make_font(i: &mut Interp, f: PDict, m: Matrix) -> R {
    let nd = copy_dict(&f);
    let fm = i.font_matrix(&f).then(&m);
    let k = i.names_cache.font_matrix;
    i.dput(&nd, k, matrix_arr(&fm));
    i.next_fid += 1;
    let fid = i.names_cache.fid;
    i.dput(&nd, fid, Obj::FontId(i.next_fid));
    i.push(Obj::Dict(nd));
    Ok(())
}

fn res_category(i: &mut Interp, cat: &[u8]) -> PDict {
    i.resources.entry(cat.to_vec()).or_insert_with(|| new_dict(50)).clone()
}

fn filter_op(i: &mut Interp) -> R {
    let fname = i.pop()?;
    let fname = i.obj_to_bytes(&fname).unwrap_or_default();
    // optional parameters
    let mut params: Option<PDict> = None;
    let mut eod: Option<(i64, Vec<u8>)> = None;
    if fname == b"SubFileDecode" {
        let e = i.pop()?;
        let c = i.pop()?;
        let (count, eodstr) = match (c, e) {
            (Obj::Dict(d), _) => (0, i.obj_to_bytes(&Obj::Dict(d)).unwrap_or_default()),
            (c, e) => (c.num().unwrap_or(0.0) as i64, i.obj_to_bytes(&e).unwrap_or_default()),
        };
        eod = Some((count, eodstr));
    }
    let mut src = i.pop()?;
    if let Obj::Dict(d) = &src {
        params = Some(d.clone());
        src = i.pop()?;
    }
    let srcf: PFile = match &src {
        Obj::File(f) => f.clone(),
        Obj::Str(s, _) => file::from_vec(s.bytes()),
        p @ Obj::Arr(_, true) => {
            // gather procedure output
            let mut all = Vec::new();
            for _ in 0..100000 {
                let b = i.os.len();
                i.call(p.clone())?;
                if i.os.len() <= b {
                    break;
                }
                match i.pop()? {
                    Obj::Str(s, _) if s.len > 0 => all.extend_from_slice(&s.bytes()),
                    _ => break,
                }
            }
            file::from_vec(all)
        }
        _ => return err("typecheck", "filter source"),
    };
    let pget = |i: &mut Interp, n: &str| -> Option<Obj> {
        let d = params.as_ref()?;
        let k = i.name(n);
        i.dget(d, k)
    };
    let kind = match fname.as_slice() {
        b"ASCIIHexDecode" | b"AHx" => file::Kind::Hex,
        b"ASCII85Decode" | b"A85" => file::Kind::A85,
        b"RunLengthDecode" | b"RL" => file::Kind::RunLength,
        b"FlateDecode" | b"Fl" | b"LZWDecode" | b"LZW" => {
            let base = if fname.starts_with(b"F") { file::Kind::Flate } else { file::Kind::Lzw(pget(i, "EarlyChange").and_then(|o| o.num()).unwrap_or(1.0) != 0.0) };
            let pred = pget(i, "Predictor").and_then(|o| o.num()).unwrap_or(1.0) as i64;
            if pred > 1 {
                let colors = pget(i, "Colors").and_then(|o| o.num()).unwrap_or(1.0) as usize;
                let bpc = pget(i, "BitsPerComponent").and_then(|o| o.num()).unwrap_or(8.0) as usize;
                let cols = pget(i, "Columns").and_then(|o| o.num()).unwrap_or(1.0) as usize;
                file::Kind::Predict(Box::new(base), pred, colors, bpc, cols)
            } else {
                base
            }
        }
        b"DCTDecode" | b"DCT" => file::Kind::Dct,
        b"SubFileDecode" => {
            let (c, e) = eod.unwrap_or((0, Vec::new()));
            file::Kind::Sub(c, e)
        }
        b"NullEncode" | b"ReusableStreamDecode" => file::Kind::Null,
        b"CCITTFaxDecode" | b"CCF" => {
            i.note_unsupported("CCITTFaxDecode filter");
            file::Kind::Sub(0, Vec::new())
        }
        other => {
            if other.ends_with(b"Encode") {
                // output filters: return a sink that discards
                i.push(Obj::File(file::from_vec(Vec::new())));
                return Ok(());
            }
            return err("undefinedresource", String::from_utf8_lossy(other).to_string());
        }
    };
    i.push(Obj::File(file::lazy(srcf, kind)));
    Ok(())
}

fn current_file(i: &Interp) -> Option<PFile> {
    for f in i.es.iter().rev() {
        if let Frame::File(f) = f {
            return Some(f.clone());
        }
    }
    None
}

fn readstring(i: &mut Interp, hex: bool) -> R {
    let s = i.pop_str()?;
    let f = match i.pop()? {
        Obj::File(f) => f,
        _ => return err("typecheck", "readstring"),
    };
    let mut fs = f.borrow_mut();
    let mut n = 0;
    let mut hi: Option<u8> = None;
    while n < s.len {
        let Some(c) = fs.read() else { break };
        if hex {
            let v = match c {
                b'0'..=b'9' => c - b'0',
                b'a'..=b'f' => c - b'a' + 10,
                b'A'..=b'F' => c - b'A' + 10,
                _ => continue,
            };
            match hi {
                None => hi = Some(v),
                Some(h) => {
                    s.set(n, h << 4 | v);
                    n += 1;
                    hi = None;
                }
            }
        } else {
            s.set(n, c);
            n += 1;
        }
    }
    drop(fs);
    let full = n == s.len;
    i.push(Obj::Str(s.sub(0, n).unwrap(), false));
    i.push(Obj::Bool(full));
    Ok(())
}

fn arct(i: &mut Interp, push: bool) -> R {
    let r = i.pop_num()?;
    let y2 = i.pop_num()?;
    let x2 = i.pop_num()?;
    let y1 = i.pop_num()?;
    let x1 = i.pop_num()?;
    let (x0, y0) = cur_user(i)?;
    let ctm = i.gs.ctm;
    let (ax, ay) = (x0 - x1, y0 - y1);
    let (bx, by) = (x2 - x1, y2 - y1);
    let la = (ax * ax + ay * ay).sqrt();
    let lb = (bx * bx + by * by).sqrt();
    let cross = ax * by - ay * bx;
    if la < 1e-9 || lb < 1e-9 || cross.abs() < 1e-9 || r == 0.0 {
        let (dx, dy) = ctm.apply(x1, y1);
        i.gs.path.line_to(dx, dy);
        if push {
            for v in [x1, y1, x1, y1] {
                i.push(Obj::Real(v));
            }
        }
        return Ok(());
    }
    let (ux, uy) = (ax / la, ay / la);
    let (vx, vy) = (bx / lb, by / lb);
    let cos_t = (ux * vx + uy * vy).clamp(-1.0, 1.0);
    let theta = cos_t.acos();
    let d = r / (theta / 2.0).tan();
    let t1 = (x1 + ux * d, y1 + uy * d);
    let t2 = (x1 + vx * d, y1 + vy * d);
    let (bisx, bisy) = (ux + vx, uy + vy);
    let bl = (bisx * bisx + bisy * bisy).sqrt();
    let h = r / (theta / 2.0).sin();
    let c = (x1 + bisx / bl * h, y1 + bisy / bl * h);
    let a1 = (t1.1 - c.1).atan2(t1.0 - c.0).to_degrees();
    let a2 = (t2.1 - c.1).atan2(t2.0 - c.0).to_degrees();
    let ccw = cross < 0.0;
    i.gs.path.arc(&ctm, c.0, c.1, r, a1, a2, ccw, true);
    if push {
        for v in [t1.0, t1.1, t2.0, t2.1] {
            i.push(Obj::Real(v));
        }
    }
    Ok(())
}

fn strokepath(i: &mut Interp) -> R {
    let ctm = i.gs.ctm;
    let Some(inv) = ctm.invert() else { return Ok(()) };
    let Some(p) = i.gs.path.to_path(&inv) else { return Ok(()) };
    let st = tiny_skia::Stroke { width: i.gs.line_width.max(0.001) as f32, line_cap: i.gs.cap, line_join: i.gs.join, miter_limit: i.gs.miter as f32, dash: None };
    let Some(sp) = p.stroke(&st, 1.0) else { return Ok(()) };
    let mut acc = PathAcc::new();
    let mut last = (0.0f64, 0.0f64);
    for seg in sp.segments() {
        match seg {
            tiny_skia::PathSegment::MoveTo(pt) => {
                let (x, y) = ctm.apply(pt.x as f64, pt.y as f64);
                acc.move_to(x, y);
                last = (pt.x as f64, pt.y as f64);
            }
            tiny_skia::PathSegment::LineTo(pt) => {
                let (x, y) = ctm.apply(pt.x as f64, pt.y as f64);
                acc.line_to(x, y);
                last = (pt.x as f64, pt.y as f64);
            }
            tiny_skia::PathSegment::QuadTo(c, pt) => {
                let (c1x, c1y) = (last.0 + 2.0 / 3.0 * (c.x as f64 - last.0), last.1 + 2.0 / 3.0 * (c.y as f64 - last.1));
                let (c2x, c2y) = (pt.x as f64 + 2.0 / 3.0 * (c.x as f64 - pt.x as f64), pt.y as f64 + 2.0 / 3.0 * (c.y as f64 - pt.y as f64));
                let (a, b) = ctm.apply(c1x, c1y);
                let (cc, d) = ctm.apply(c2x, c2y);
                let (e, f) = ctm.apply(pt.x as f64, pt.y as f64);
                acc.curve_to(a, b, cc, d, e, f);
                last = (pt.x as f64, pt.y as f64);
            }
            tiny_skia::PathSegment::CubicTo(c1, c2, pt) => {
                let (a, b) = ctm.apply(c1.x as f64, c1.y as f64);
                let (cc, d) = ctm.apply(c2.x as f64, c2.y as f64);
                let (e, f) = ctm.apply(pt.x as f64, pt.y as f64);
                acc.curve_to(a, b, cc, d, e, f);
                last = (pt.x as f64, pt.y as f64);
            }
            tiny_skia::PathSegment::Close => acc.close(),
        }
    }
    i.gs.path = acc;
    Ok(())
}

fn shfill(i: &mut Interp, sh: PDict) -> R {
    i.note_unsupported("smooth shading (approximated with average colour)");
    let kcs = i.name("ColorSpace");
    let kf = i.name("Function");
    let cs = match i.dget(&sh, kcs) {
        Some(o) => parse_cspace(i, &o)?,
        None => ColorSpace::Gray,
    };
    let mut comps = vec![0.5; cs.ncomps()];
    let func = match i.dget(&sh, kf) {
        Some(Obj::Arr(a, false)) => a.get(0),
        o => o,
    };
    if let Some(Obj::Dict(fd)) = func {
        let (k0, k1, kfs) = (i.name("C0"), i.name("C1"), i.name("Functions"));
        let mut fdict = fd.clone();
        if let Some(Obj::Arr(fs, _)) = i.dget(&fd, kfs) {
            if let Some(Obj::Dict(first)) = fs.get(0) {
                fdict = first;
            }
        }
        let c0: Vec<f64> = match i.dget(&fdict, k0) {
            Some(Obj::Arr(a, _)) => a.to_vec().iter().filter_map(|o| o.num()).collect(),
            _ => vec![0.0; cs.ncomps()],
        };
        let c1: Vec<f64> = match i.dget(&fdict, k1) {
            Some(Obj::Arr(a, _)) => a.to_vec().iter().filter_map(|o| o.num()).collect(),
            _ => vec![1.0; cs.ncomps()],
        };
        comps = c0.iter().zip(c1.iter()).map(|(a, b)| (a + b) / 2.0).collect();
    }
    let c = i.convert_color(&cs, &comps)?;
    let saved = i.gs.color;
    let saved_cs = i.gs.cspace.clone();
    i.gs.color = c;
    i.gs.cspace = ColorSpace::Rgb;
    let p = i.gs.clip_path.clone().unwrap_or_else(|| i.page_rect_dev());
    i.fill_dev(&p, false);
    i.gs.color = saved;
    i.gs.cspace = saved_cs;
    Ok(())
}

const PRELUDE: &str = r#"
/=only { dup type /stringtype ne { 128 string cvs } if print } bind def
/execform { dup /PaintProc get exch gsave dup /Matrix known { dup /Matrix get concat } if
  dup /BBox get aload pop exch 3 index sub exch 2 index sub rectclip exch exec grestore } bind def
/findencoding { /Encoding findresource } bind def
/setcmykcolor where pop
/CIDInit 20 dict dup begin
  /begincmap { } def /endcmap { } def /usecmap { pop } def /usefont { pop } def
  /begincodespacerange { pop mark } def
  /endcodespacerange { counttomark 0 gt { dup length 1 eq { currentdict /.CodeBytes 1 put } if } if cleartomark } def
  /begincidrange { pop mark } def /endcidrange { cleartomark } def
  /begincidchar { pop mark } def /endcidchar { cleartomark } def
  /beginbfchar { pop mark } def /endbfchar { cleartomark } def
  /beginbfrange { pop mark } def /endbfrange { cleartomark } def
  /beginnotdefrange { pop mark } def /endnotdefrange { cleartomark } def
  /beginnotdefchar { pop mark } def /endnotdefchar { cleartomark } def
  /beginusematrix { pop } def /endusematrix { pop } def
end /ProcSet defineresource pop
/Identity-H 10 dict dup begin /CMapName /Identity-H def /CMapType 1 def /WMode 0 def end /CMap defineresource pop
/Identity-V 10 dict dup begin /CMapName /Identity-V def /CMapType 1 def /WMode 1 def end /CMap defineresource pop
statusdict begin
  /setduplexmode { pop } def /duplexmode false def /settumble { pop } def /setpapertray { pop } def /papertray 1 def
  /setmargins { pop pop } def /setjobtimeout { pop } def /waittimeout 300 def /jobname () def /setresolution { pop } def
  /manualfeed false def /setpage { pop pop pop } def /pagecount 0 def /setdefaulttimeouts { pop pop pop } def
  /printername { (PrintFileViewer) exch copy } def /product (PrintFileViewer) def /setpagetype { pop } def
  /setsoftwareiomode { pop } def /setstrokeadjust { pop } def /setdostartpage { pop } def /dostartpage false def
  /settrayswitch { pop } def /setcollate { pop } def /setmediatype { pop } def /setoutputtray { pop } def
  /ramsize 67108864 def /checkpassword { pop true } def /setidlefonts { pop } def /idlefonts [] def
end
/letter { << /PageSize [612 792] >> setpagedevice } def
/legal { << /PageSize [612 1008] >> setpagedevice } def
/a4 { << /PageSize [595 842] >> setpagedevice } def
/a3 { << /PageSize [842 1191] >> setpagedevice } def
/a5 { << /PageSize [420 595] >> setpagedevice } def
/b5 { << /PageSize [516 729] >> setpagedevice } def
/ledger { << /PageSize [1224 792] >> setpagedevice } def
/11x17 { << /PageSize [792 1224] >> setpagedevice } def
/executivepage { << /PageSize [522 756] >> setpagedevice } def
/note { } def /lettersmall /letter load def /a4small /a4 load def
statusdict begin /letter //letter def /legal //legal def /a4 //a4 def /a3 //a3 def /a5 //a5 def /b5 //b5 def /ledger //ledger def /11x17 //11x17 def /lettertray //letter def /a4tray //a4 def /legaltray //legal def end
userdict /#copies 1 put
"#;

pub fn register(i: &mut Interp) {
    let list: Vec<(&'static str, OpFn)> = ops! {
        // ---- stack ----
        "pop" => |i| { i.pop()?; Ok(()) },
        "exch" => |i| { let b = i.pop()?; let a = i.pop()?; i.push(b); i.push(a); Ok(()) },
        "dup" => |i| { let a = i.top().cloned().ok_or(PsError::Err("stackunderflow", "dup".into()))?; i.push(a); Ok(()) },
        "copy" => |i| {
            let top = i.pop()?;
            match top {
                Obj::Int(n) => {
                    let n = n as usize;
                    if n > i.os.len() { return err("stackunderflow", "copy"); }
                    let s = i.os.len() - n;
                    let v: Vec<Obj> = i.os[s..].to_vec();
                    i.os.extend(v);
                }
                Obj::Arr(dst, x) => {
                    let src = i.pop_arr()?;
                    if src.len > dst.len { return err("rangecheck", "copy"); }
                    let v = src.to_vec();
                    for (k, o) in v.into_iter().enumerate() { dst.set(k, o); }
                    i.push(Obj::Arr(dst.sub(0, src.len).unwrap(), x));
                }
                Obj::Str(dst, x) => {
                    let src = i.pop_str()?;
                    let b = src.bytes();
                    if !dst.write_at(0, &b) { return err("rangecheck", "copy"); }
                    i.push(Obj::Str(dst.sub(0, b.len()).unwrap(), x));
                }
                Obj::Dict(dst) => {
                    let src = i.pop_dict()?;
                    let items: Vec<(Key, Obj)> = src.borrow().map.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
                    for (k, v) in items { dst.borrow_mut().map.insert(k, v); }
                    i.push(Obj::Dict(dst));
                }
                Obj::GState(_) => { let _ = i.pop()?; i.push(Obj::GState(Rc::new(i.gs.clone()))); }
                _ => return err("typecheck", "copy"),
            }
            Ok(())
        },
        "index" => |i| { let n = i.pop_int()?; if n < 0 || n as usize >= i.os.len() { return err("rangecheck", "index"); } let v = i.os[i.os.len() - 1 - n as usize].clone(); i.push(v); Ok(()) },
        "roll" => |i| {
            let j = i.pop_int()?; let n = i.pop_int()?;
            if n < 0 || n as usize > i.os.len() { return err("rangecheck", "roll"); }
            if n == 0 { return Ok(()); }
            let s = i.os.len() - n as usize;
            let j = ((j % n) + n) % n;
            i.os[s..].rotate_right(j as usize);
            Ok(())
        },
        "clear" => |i| { i.os.clear(); Ok(()) },
        "count" => |i| { let n = i.os.len() as i64; i.push(Obj::Int(n)); Ok(()) },
        "mark" => |i| { i.push(Obj::Mark); Ok(()) },
        "[" => |i| { i.push(Obj::Mark); Ok(()) },
        "<<" => |i| { i.push(Obj::Mark); Ok(()) },
        "cleartomark" => |i| { while let Some(o) = i.os.pop() { if matches!(o, Obj::Mark) { return Ok(()); } } err("unmatchedmark", "cleartomark") },
        "counttomark" => |i| match i.os.iter().rposition(|o| matches!(o, Obj::Mark)) { Some(p) => { let n = (i.os.len() - p - 1) as i64; i.push(Obj::Int(n)); Ok(()) } None => err("unmatchedmark", "counttomark") },
        "]" => |i| {
            let p = i.os.iter().rposition(|o| matches!(o, Obj::Mark)).ok_or(PsError::Err("unmatchedmark", "]".into()))?;
            let v: Vec<Obj> = i.os.drain(p + 1..).collect();
            i.os.pop();
            i.push(Obj::Arr(PArr::new(v), false));
            Ok(())
        },
        ">>" => |i| {
            let p = i.os.iter().rposition(|o| matches!(o, Obj::Mark)).ok_or(PsError::Err("unmatchedmark", ">>".into()))?;
            let v: Vec<Obj> = i.os.drain(p + 1..).collect();
            i.os.pop();
            if v.len() % 2 != 0 { return err("rangecheck", ">>"); }
            let d = new_dict(v.len() / 2);
            for c in v.chunks(2) { let k = i.key(&c[0])?; d.borrow_mut().map.insert(k, c[1].clone()); }
            i.push(Obj::Dict(d));
            Ok(())
        },
        // ---- arithmetic ----
        "add" => |i| arith(i, |a, b| a.checked_add(b), |a, b| a + b),
        "sub" => |i| arith(i, |a, b| a.checked_sub(b), |a, b| a - b),
        "mul" => |i| arith(i, |a, b| a.checked_mul(b), |a, b| a * b),
        "div" => |i| { let b = i.pop_num()?; let a = i.pop_num()?; if b == 0.0 { return err("undefinedresult", "div"); } i.push(Obj::Real(a / b)); Ok(()) },
        "idiv" => |i| { let b = i.pop_int()?; let a = i.pop_int()?; if b == 0 { return err("undefinedresult", "idiv"); } i.push(Obj::Int(a / b)); Ok(()) },
        "mod" => |i| { let b = i.pop_int()?; let a = i.pop_int()?; if b == 0 { return err("undefinedresult", "mod"); } i.push(Obj::Int(a % b)); Ok(()) },
        "neg" => |i| { let a = i.pop()?; i.push(match a { Obj::Int(v) => Obj::Int(-v), o => Obj::Real(-o.num().ok_or(PsError::Err("typecheck", "neg".into()))?) }); Ok(()) },
        "abs" => |i| { let a = i.pop()?; i.push(match a { Obj::Int(v) => Obj::Int(v.abs()), o => Obj::Real(o.num().ok_or(PsError::Err("typecheck", "abs".into()))?.abs()) }); Ok(()) },
        "ceiling" => |i| { let a = i.pop()?; i.push(match a { Obj::Int(v) => Obj::Int(v), o => Obj::Real(o.num().unwrap_or(0.0).ceil()) }); Ok(()) },
        "floor" => |i| { let a = i.pop()?; i.push(match a { Obj::Int(v) => Obj::Int(v), o => Obj::Real(o.num().unwrap_or(0.0).floor()) }); Ok(()) },
        "round" => |i| { let a = i.pop()?; i.push(match a { Obj::Int(v) => Obj::Int(v), o => Obj::Real((o.num().unwrap_or(0.0) + 0.5).floor()) }); Ok(()) },
        "truncate" => |i| { let a = i.pop()?; i.push(match a { Obj::Int(v) => Obj::Int(v), o => Obj::Real(o.num().unwrap_or(0.0).trunc()) }); Ok(()) },
        "sqrt" => |i| { let a = i.pop_num()?; i.push(Obj::Real(a.max(0.0).sqrt())); Ok(()) },
        "atan" => |i| { let d = i.pop_num()?; let n = i.pop_num()?; let mut a = n.atan2(d).to_degrees(); if a < 0.0 { a += 360.0; } i.push(Obj::Real(a)); Ok(()) },
        "cos" => |i| { let a = i.pop_num()?; i.push(Obj::Real(a.to_radians().cos())); Ok(()) },
        "sin" => |i| { let a = i.pop_num()?; i.push(Obj::Real(a.to_radians().sin())); Ok(()) },
        "exp" => |i| { let e = i.pop_num()?; let b = i.pop_num()?; i.push(Obj::Real(b.powf(e))); Ok(()) },
        "ln" => |i| { let a = i.pop_num()?; i.push(Obj::Real(a.ln())); Ok(()) },
        "log" => |i| { let a = i.pop_num()?; i.push(Obj::Real(a.log10())); Ok(()) },
        "rand" => |i| { i.rand = i.rand.wrapping_mul(1103515245).wrapping_add(12345) & 0x7fffffff; let r = i.rand as i64; i.push(Obj::Int(r)); Ok(()) },
        "srand" => |i| { i.rand = i.pop_int()? as u32; Ok(()) },
        "rrand" => |i| { let r = i.rand as i64; i.push(Obj::Int(r)); Ok(()) },
        "cvi" => |i| {
            let a = i.pop()?;
            let v = match a { Obj::Str(s, _) => super::scan::parse_number(&s.bytes()).and_then(|o| o.num()).ok_or(PsError::Err("typecheck", "cvi".into()))?, o => o.num().ok_or(PsError::Err("typecheck", "cvi".into()))? };
            i.push(Obj::Int(v.trunc() as i64)); Ok(())
        },
        "cvr" => |i| {
            let a = i.pop()?;
            let v = match a { Obj::Str(s, _) => super::scan::parse_number(&s.bytes()).and_then(|o| o.num()).ok_or(PsError::Err("typecheck", "cvr".into()))?, o => o.num().ok_or(PsError::Err("typecheck", "cvr".into()))? };
            i.push(Obj::Real(v)); Ok(())
        },
        // ---- relational / boolean ----
        "eq" => |i| { let b = i.pop()?; let a = i.pop()?; let r = obj_eq(i, &a, &b); i.push(Obj::Bool(r)); Ok(()) },
        "ne" => |i| { let b = i.pop()?; let a = i.pop()?; let r = !obj_eq(i, &a, &b); i.push(Obj::Bool(r)); Ok(()) },
        "gt" => |i| { let o = compare(i)?; i.push(Obj::Bool(o == std::cmp::Ordering::Greater)); Ok(()) },
        "ge" => |i| { let o = compare(i)?; i.push(Obj::Bool(o != std::cmp::Ordering::Less)); Ok(()) },
        "lt" => |i| { let o = compare(i)?; i.push(Obj::Bool(o == std::cmp::Ordering::Less)); Ok(()) },
        "le" => |i| { let o = compare(i)?; i.push(Obj::Bool(o != std::cmp::Ordering::Greater)); Ok(()) },
        "and" => |i| { let b = i.pop()?; let a = i.pop()?; i.push(match (a, b) { (Obj::Bool(x), Obj::Bool(y)) => Obj::Bool(x && y), (Obj::Int(x), Obj::Int(y)) => Obj::Int(x & y), _ => return err("typecheck", "and") }); Ok(()) },
        "or" => |i| { let b = i.pop()?; let a = i.pop()?; i.push(match (a, b) { (Obj::Bool(x), Obj::Bool(y)) => Obj::Bool(x || y), (Obj::Int(x), Obj::Int(y)) => Obj::Int(x | y), _ => return err("typecheck", "or") }); Ok(()) },
        "xor" => |i| { let b = i.pop()?; let a = i.pop()?; i.push(match (a, b) { (Obj::Bool(x), Obj::Bool(y)) => Obj::Bool(x ^ y), (Obj::Int(x), Obj::Int(y)) => Obj::Int(x ^ y), _ => return err("typecheck", "xor") }); Ok(()) },
        "not" => |i| { let a = i.pop()?; i.push(match a { Obj::Bool(x) => Obj::Bool(!x), Obj::Int(x) => Obj::Int(!x), _ => return err("typecheck", "not") }); Ok(()) },
        "bitshift" => |i| { let s = i.pop_int()?; let v = i.pop_int()? as i32; let r = if s >= 0 { (v as u32).wrapping_shl(s as u32) as i32 } else { ((v as u32) >> ((-s) as u32).min(31)) as i32 }; i.push(Obj::Int(r as i64)); Ok(()) },
        // ---- type conversion ----
        "type" => |i| { let a = i.pop()?; let n = i.name(a.type_name()); i.push(Obj::Name(n, true)); Ok(()) },
        "cvx" => |i| { let a = i.pop()?; i.push(a.cvx()); Ok(()) },
        "cvlit" => |i| { let a = i.pop()?; i.push(a.cvlit()); Ok(()) },
        "xcheck" => |i| { let a = i.pop()?; i.push(Obj::Bool(a.is_exec())); Ok(()) },
        "rcheck" => |i| { i.pop()?; i.push(Obj::Bool(true)); Ok(()) },
        "wcheck" => |i| { i.pop()?; i.push(Obj::Bool(true)); Ok(()) },
        "gcheck" => |i| { i.pop()?; i.push(Obj::Bool(false)); Ok(()) },
        "readonly" => |i| Ok(()),
        "executeonly" => |i| Ok(()),
        "noaccess" => |i| Ok(()),
        "cvn" => |i| { let a = i.pop()?; match a { Obj::Str(s, x) => { let n = i.names.intern(&s.bytes()); i.push(Obj::Name(n, x)); } o @ Obj::Name(..) => i.push(o), _ => return err("typecheck", "cvn") } Ok(()) },
        "cvs" => |i| {
            let s = i.pop_str()?; let o = i.pop()?;
            let t = obj_text(i, &o, false, 0);
            let b = t.as_bytes();
            let n = b.len().min(s.len);
            s.write_at(0, &b[..n]);
            i.push(Obj::Str(s.sub(0, n).unwrap(), false));
            Ok(())
        },
        "cvrs" => |i| {
            let s = i.pop_str()?; let radix = i.pop_int()?; let v = i.pop_num()?;
            let t = if radix == 10 { obj_text(i, &int_or_real(v), false, 0) } else {
                let mut n = v as i64 as u32; let mut digits = Vec::new();
                if n == 0 { digits.push(b'0'); }
                while n > 0 { let d = (n % radix as u32) as u8; digits.push(if d < 10 { b'0' + d } else { b'A' + d - 10 }); n /= radix as u32; }
                digits.reverse(); String::from_utf8(digits).unwrap()
            };
            let b = t.as_bytes(); let n = b.len().min(s.len); s.write_at(0, &b[..n]);
            i.push(Obj::Str(s.sub(0, n).unwrap(), false)); Ok(())
        },
        // ---- control ----
        "exec" => |i| { let o = i.pop()?; i.exec_value(o) },
        "if" => |i| { let p = i.pop_proc()?; let c = i.pop_bool()?; if c { i.exec_value(p)?; } Ok(()) },
        "ifelse" => |i| { let p2 = i.pop_proc()?; let p1 = i.pop_proc()?; let c = i.pop_bool()?; i.exec_value(if c { p1 } else { p2 }) },
        "for" => |i| {
            let p = i.pop_arr()?; let lim = i.pop()?; let inc = i.pop()?; let init = i.pop()?;
            match (&init, &inc, &lim) {
                (Obj::Int(a), Obj::Int(b), Obj::Int(c)) => i.es.push(Frame::ForI(*a, *b, *c, p)),
                _ => i.es.push(Frame::ForR(init.num().unwrap_or(0.0), inc.num().unwrap_or(1.0), lim.num().unwrap_or(0.0), p)),
            }
            Ok(())
        },
        "repeat" => |i| { let p = i.pop_arr()?; let n = i.pop_int()?; i.es.push(Frame::Repeat(n, p)); Ok(()) },
        "loop" => |i| { let p = i.pop_arr()?; i.es.push(Frame::Loop(p)); Ok(()) },
        "exit" => |i| Err(PsError::Exit),
        "stop" => |i| Err(PsError::Stop),
        "stopped" => |i| { let p = i.pop()?; i.es.push(Frame::Stopped); i.exec_value(p) },
        "quit" => |i| Err(PsError::Quit),
        "countexecstack" => |i| { let n = i.es.len() as i64; i.push(Obj::Int(n)); Ok(()) },
        "execstack" => |i| { let a = i.pop_arr()?; i.push(Obj::Arr(a.sub(0, 0).unwrap(), false)); Ok(()) },
        "forall" => |i| {
            let p = i.pop_arr()?; let o = i.pop()?;
            match o {
                Obj::Arr(a, _) => i.es.push(Frame::ForallArr(a, 0, p)),
                Obj::Str(s, _) => i.es.push(Frame::ForallStr(s, 0, p)),
                Obj::Dict(d) => { let items: Vec<(Key, Obj)> = d.borrow().map.iter().map(|(k, v)| (k.clone(), v.clone())).collect(); i.es.push(Frame::ForallDict(items, 0, p)); }
                _ => return err("typecheck", "forall"),
            }
            Ok(())
        },
        // ---- arrays / strings / dicts ----
        "array" => |i| { let n = i.pop_int()?; if !(0..=10_000_000).contains(&n) { return err("rangecheck", "array"); } i.push(Obj::Arr(PArr::new(vec![Obj::Null; n as usize]), false)); Ok(()) },
        "packedarray" => |i| { let n = i.pop_int()? as usize; if n > i.os.len() { return err("stackunderflow", "packedarray"); } let v: Vec<Obj> = i.os.split_off(i.os.len() - n); i.push(Obj::Arr(PArr::new(v), false)); Ok(()) },
        "setpacking" => |i| { i.packing = i.pop_bool()?; Ok(()) },
        "currentpacking" => |i| { let p = i.packing; i.push(Obj::Bool(p)); Ok(()) },
        "string" => |i| { let n = i.pop_int()?; if !(0..=100_000_000).contains(&n) { return err("rangecheck", "string"); } i.push(Obj::Str(PStr::new(vec![0; n as usize]), false)); Ok(()) },
        "dict" => |i| { let n = i.pop_int()?; i.push(Obj::Dict(new_dict(n.max(0) as usize))); Ok(()) },
        "maxlength" => |i| { let d = i.pop_dict()?; let n = d.borrow().cap.max(d.borrow().map.len() + 1) as i64; i.push(Obj::Int(n)); Ok(()) },
        "length" => |i| {
            let o = i.pop()?;
            let n = match &o { Obj::Arr(a, _) => a.len, Obj::Str(s, _) => s.len, Obj::Dict(d) => d.borrow().map.len(), Obj::Name(n, _) => i.names.get(*n).len(), _ => return err("typecheck", "length") };
            i.push(Obj::Int(n as i64)); Ok(())
        },
        "get" => |i| {
            let k = i.pop()?; let o = i.pop()?;
            match o {
                Obj::Arr(a, _) => { let idx = k.num().ok_or(PsError::Err("typecheck", "get".into()))? as usize; let v = a.get(idx).ok_or(PsError::Err("rangecheck", "get".into()))?; i.push(v); }
                Obj::Str(s, _) => { let idx = k.num().ok_or(PsError::Err("typecheck", "get".into()))? as usize; let v = s.get(idx).ok_or(PsError::Err("rangecheck", "get".into()))?; i.push(Obj::Int(v as i64)); }
                Obj::Dict(d) => { let key = i.key(&k)?; let v = d.borrow().map.get(&key).cloned(); match v { Some(v) => i.push(v), None => return err("undefined", obj_text(i, &k, false, 0)) } }
                _ => return err("typecheck", "get"),
            }
            Ok(())
        },
        "put" => |i| {
            let v = i.pop()?; let k = i.pop()?; let o = i.pop()?;
            match o {
                Obj::Arr(a, _) => { let idx = k.num().ok_or(PsError::Err("typecheck", "put".into()))? as usize; if !a.set(idx, v) { return err("rangecheck", "put"); } }
                Obj::Str(s, _) => { let idx = k.num().ok_or(PsError::Err("typecheck", "put".into()))? as usize; if !s.set(idx, v.num().unwrap_or(0.0) as i64 as u8) { return err("rangecheck", "put"); } }
                Obj::Dict(d) => { let key = i.key(&k)?; d.borrow_mut().map.insert(key, v); }
                _ => return err("typecheck", "put"),
            }
            Ok(())
        },
        "getinterval" => |i| {
            let n = i.pop_int()? as usize; let idx = i.pop_int()? as usize; let o = i.pop()?;
            match o {
                Obj::Arr(a, x) => { let s = a.sub(idx, n).ok_or(PsError::Err("rangecheck", "getinterval".into()))?; i.push(Obj::Arr(s, x)); }
                Obj::Str(s, x) => { let s = s.sub(idx, n).ok_or(PsError::Err("rangecheck", "getinterval".into()))?; i.push(Obj::Str(s, x)); }
                _ => return err("typecheck", "getinterval"),
            }
            Ok(())
        },
        "putinterval" => |i| {
            let src = i.pop()?; let idx = i.pop_int()? as usize; let dst = i.pop()?;
            match (dst, src) {
                (Obj::Arr(d, _), Obj::Arr(s, _)) => { if idx + s.len > d.len { return err("rangecheck", "putinterval"); } for (k, o) in s.to_vec().into_iter().enumerate() { d.set(idx + k, o); } }
                (Obj::Str(d, _), Obj::Str(s, _)) => { if !d.write_at(idx, &s.bytes()) { return err("rangecheck", "putinterval"); } }
                _ => return err("typecheck", "putinterval"),
            }
            Ok(())
        },
        "aload" => |i| { let a = i.pop_arr()?; for o in a.to_vec() { i.push(o); } i.push(Obj::Arr(a, false)); Ok(()) },
        "astore" => |i| { let a = i.pop_arr()?; if a.len > i.os.len() { return err("stackunderflow", "astore"); } let v = i.os.split_off(i.os.len() - a.len); for (k, o) in v.into_iter().enumerate() { a.set(k, o); } i.push(Obj::Arr(a, false)); Ok(()) },
        "begin" => |i| { let d = i.pop_dict()?; if i.ds.len() > 500 { return err("dictstackoverflow", "begin"); } i.ds.push(d); Ok(()) },
        "end" => |i| { if i.ds.len() <= 3 { return err("dictstackunderflow", "end"); } i.ds.pop(); Ok(()) },
        "def" => |i| { let v = i.pop()?; let k = i.pop()?; let key = i.key(&k)?; let d = i.ds.last().unwrap().clone(); d.borrow_mut().map.insert(key, v); Ok(()) },
        "load" => |i| { let k = i.pop()?; let key = i.key(&k)?; match i.lookup(&key) { Some(v) => { i.push(v); Ok(()) } None => err("undefined", obj_text(i, &k, false, 0)) } },
        "store" => |i| {
            let v = i.pop()?; let k = i.pop()?; let key = i.key(&k)?;
            for d in i.ds.iter().rev() { if d.borrow().map.contains_key(&key) { d.borrow_mut().map.insert(key, v); return Ok(()); } }
            let d = i.ds.last().unwrap().clone(); d.borrow_mut().map.insert(key, v); Ok(())
        },
        "known" => |i| { let k = i.pop()?; let d = i.pop_dict()?; let key = i.key(&k)?; let r = d.borrow().map.contains_key(&key); i.push(Obj::Bool(r)); Ok(()) },
        "where" => |i| {
            let k = i.pop()?; let key = i.key(&k)?;
            for d in i.ds.iter().rev() { if d.borrow().map.contains_key(&key) { let d = d.clone(); i.push(Obj::Dict(d)); i.push(Obj::Bool(true)); return Ok(()); } }
            i.push(Obj::Bool(false)); Ok(())
        },
        "undef" => |i| { let k = i.pop()?; let d = i.pop_dict()?; let key = i.key(&k)?; d.borrow_mut().map.remove(&key); Ok(()) },
        "currentdict" => |i| { let d = i.ds.last().unwrap().clone(); i.push(Obj::Dict(d)); Ok(()) },
        "countdictstack" => |i| { let n = i.ds.len() as i64; i.push(Obj::Int(n)); Ok(()) },
        "cleardictstack" => |i| { i.ds.truncate(3); Ok(()) },
        "dictstack" => |i| { let a = i.pop_arr()?; let ds = i.ds.clone(); let n = ds.len().min(a.len); for (k, d) in ds.into_iter().take(n).enumerate() { a.set(k, Obj::Dict(d)); } i.push(Obj::Arr(a.sub(0, n).unwrap(), false)); Ok(()) },
        "search" => |i| {
            let seek = i.pop_str()?; let s = i.pop_str()?;
            let hay = s.bytes(); let nd = seek.bytes();
            let pos = if nd.is_empty() { Some(0) } else { hay.windows(nd.len()).position(|w| w == nd.as_slice()) };
            match pos {
                Some(p) => { i.push(Obj::Str(s.sub(p + nd.len(), hay.len() - p - nd.len()).unwrap(), false)); i.push(Obj::Str(s.sub(p, nd.len()).unwrap(), false)); i.push(Obj::Str(s.sub(0, p).unwrap(), false)); i.push(Obj::Bool(true)); }
                None => { i.push(Obj::Str(s, false)); i.push(Obj::Bool(false)); }
            }
            Ok(())
        },
        "anchorsearch" => |i| {
            let seek = i.pop_str()?; let s = i.pop_str()?;
            let hay = s.bytes(); let nd = seek.bytes();
            if hay.starts_with(&nd) { i.push(Obj::Str(s.sub(nd.len(), hay.len() - nd.len()).unwrap(), false)); i.push(Obj::Str(s.sub(0, nd.len()).unwrap(), false)); i.push(Obj::Bool(true)); }
            else { i.push(Obj::Str(s, false)); i.push(Obj::Bool(false)); }
            Ok(())
        },
        "token" => |i| {
            let src = i.pop()?;
            match src {
                Obj::Str(s, _) => {
                    let f = file::from_vec(s.bytes());
                    match i.scan_token(&f)? {
                        Some(t) => { let pos = f.borrow().position() - f.borrow().back.len(); let rest = s.sub(pos.min(s.len), s.len - pos.min(s.len)).unwrap(); i.push(Obj::Str(rest, false)); i.push(t); i.push(Obj::Bool(true)); }
                        None => i.push(Obj::Bool(false)),
                    }
                }
                Obj::File(f) => match i.scan_token(&f)? { Some(t) => { i.push(t); i.push(Obj::Bool(true)); } None => i.push(Obj::Bool(false)) },
                _ => return err("typecheck", "token"),
            }
            Ok(())
        },
        // ---- files & output ----
        "currentfile" => |i| { let f = current_file(i).unwrap_or_else(|| file::from_vec(Vec::new())); i.push(Obj::File(f)); Ok(()) },
        "read" => |i| { let f = match i.pop()? { Obj::File(f) => f, _ => return err("typecheck", "read") }; let b = f.borrow_mut().read(); match b { Some(b) => { i.push(Obj::Int(b as i64)); i.push(Obj::Bool(true)); } None => i.push(Obj::Bool(false)) } Ok(()) },
        "readstring" => |i| readstring(i, false),
        "readhexstring" => |i| readstring(i, true),
        "readline" => |i| {
            let s = i.pop_str()?; let f = match i.pop()? { Obj::File(f) => f, _ => return err("typecheck", "readline") };
            let mut fs = f.borrow_mut(); let mut n = 0; let mut eol = false;
            while let Some(c) = fs.read() {
                if c == b'\n' { eol = true; break; }
                if c == b'\r' { if fs.peek() == Some(b'\n') { fs.read(); } eol = true; break; }
                if n < s.len { s.set(n, c); n += 1; } else { fs.unread(c); break; }
            }
            drop(fs);
            i.push(Obj::Str(s.sub(0, n).unwrap(), false)); i.push(Obj::Bool(eol || n > 0)); Ok(())
        },
        "closefile" => |i| { if let Obj::File(f) = i.pop()? { let mut fs = f.borrow_mut(); fs.closed = true; fs.back.clear(); } Ok(()) },
        "flushfile" => |i| { if let Obj::File(f) = i.pop()? { let _ = f.borrow_mut().take_rest(); } Ok(()) },
        "bytesavailable" => |i| { let f = match i.pop()? { Obj::File(f) => f, _ => return err("typecheck", "bytesavailable") }; let n = f.borrow().bytes_available(); i.push(Obj::Int(n)); Ok(()) },
        "fileposition" => |i| { let f = match i.pop()? { Obj::File(f) => f, _ => return err("typecheck", "fileposition") }; let n = f.borrow().position() as i64; i.push(Obj::Int(n)); Ok(()) },
        "setfileposition" => |i| { i.pop()?; i.pop()?; Ok(()) },
        "file" => |i| {
            let _mode = i.pop()?; let name = i.pop()?; let n = i.obj_to_bytes(&name).unwrap_or_default();
            match n.as_slice() {
                b"%stdin" | b"%lineedit" | b"%statementedit" => { let f = current_file(i).unwrap_or_else(|| file::from_vec(Vec::new())); i.push(Obj::File(f)); Ok(()) }
                b"%stdout" | b"%stderr" => { i.push(Obj::File(file::from_vec(Vec::new()))); Ok(()) }
                _ => err("invalidfileaccess", String::from_utf8_lossy(&n).to_string()),
            }
        },
        "filter" => |i| filter_op(i),
        "eexec" => |i| {
            let src = i.pop()?;
            let f = match src { Obj::File(f) => f, Obj::Str(s, _) => file::from_vec(s.bytes()), _ => return err("typecheck", "eexec") };
            let sd = i.systemdict.clone();
            i.ds.push(sd);
            let depth = i.ds.len() - 1;
            i.es.push(Frame::DictRestore(depth));
            i.es.push(Frame::File(file::eexec(f)));
            Ok(())
        },
        "status" => |i| { let o = i.pop()?; if let Obj::File(f) = o { let c = !f.borrow().closed; i.push(Obj::Bool(c)); } else { i.push(Obj::Bool(false)); } Ok(()) },
        "run" => |i| { let n = i.pop()?; err("invalidfileaccess", format!("run {}", obj_text(i, &n, false, 0))) },
        "deletefile" => |i| { i.pop()?; err("invalidfileaccess", "deletefile") },
        "renamefile" => |i| { i.pop()?; i.pop()?; err("invalidfileaccess", "renamefile") },
        "write" => |i| { let c = i.pop_int()?; i.pop()?; out(i, &((c as u8) as char).to_string()); Ok(()) },
        "writestring" => |i| { let s = i.pop_str()?; i.pop()?; out(i, &String::from_utf8_lossy(&s.bytes())); Ok(()) },
        "writehexstring" => |i| { i.pop_str()?; i.pop()?; Ok(()) },
        "print" => |i| { let s = i.pop_str()?; out(i, &String::from_utf8_lossy(&s.bytes())); Ok(()) },
        "=" => |i| { let o = i.pop()?; let t = obj_text(i, &o, false, 0); out(i, &format!("{t}\n")); Ok(()) },
        "==" => |i| { let o = i.pop()?; let t = obj_text(i, &o, true, 0); out(i, &format!("{t}\n")); Ok(()) },
        "stack" => |i| { let v: Vec<String> = i.os.iter().rev().map(|o| obj_text(i, o, false, 0)).collect(); for s in v { out(i, &format!("{s}\n")); } Ok(()) },
        "pstack" => |i| { let v: Vec<String> = i.os.iter().rev().map(|o| obj_text(i, o, true, 0)).collect(); for s in v { out(i, &format!("{s}\n")); } Ok(()) },
        "flush" => |i| Ok(()),
        "echo" => |i| { i.pop()?; Ok(()) },
        "prompt" => |i| Ok(()),
        "handleerror" => |i| Ok(()),
        // ---- VM ----
        "save" => |i| {
            i.save_count += 1; let id = i.save_count;
            let mut g = i.gs.clone(); g.save_level = Some(id);
            i.gstack.push(g); i.push(Obj::Save(id)); Ok(())
        },
        "restore" => |i| {
            let id = match i.pop()? { Obj::Save(s) => s, _ => return err("typecheck", "restore") };
            while let Some(g) = i.gstack.pop() { if g.save_level == Some(id) { let mut g = g; g.save_level = None; i.gs = g; break; } }
            Ok(())
        },
        "vmstatus" => |i| { i.push(Obj::Int(1)); i.push(Obj::Int(1_000_000)); i.push(Obj::Int(64_000_000)); Ok(()) },
        "setglobal" => |i| { i.global = i.pop_bool()?; Ok(()) },
        "currentglobal" => |i| { let g = i.global; i.push(Obj::Bool(g)); Ok(()) },
        "vmreclaim" => |i| { i.pop()?; Ok(()) },
        "setvmthreshold" => |i| { i.pop()?; Ok(()) },
        "bind" => |i| { if let Some(Obj::Arr(a, _)) = i.top().cloned() { let mut seen = std::collections::HashSet::new(); bind_proc(i, &a, &mut seen, 0); } Ok(()) },
        "null" => |i| { i.push(Obj::Null); Ok(()) },
        "version" => |i| { i.push(Obj::Str(PStr::new(b"3010".to_vec()), false)); Ok(()) },
        "languagelevel" => |i| { i.push(Obj::Int(3)); Ok(()) },
        "product" => |i| { i.push(Obj::Str(PStr::new(b"PrintFileViewer".to_vec()), false)); Ok(()) },
        "revision" => |i| { i.push(Obj::Int(1)); Ok(()) },
        "serialnumber" => |i| { i.push(Obj::Int(1)); Ok(()) },
        "realtime" => |i| { let n = (i.steps / 1000) as i64; i.push(Obj::Int(n)); Ok(()) },
        "usertime" => |i| { let n = (i.steps / 1000) as i64; i.push(Obj::Int(n)); Ok(()) },
        "executive" => |i| Ok(()),
        "startjob" => |i| { i.pop()?; i.pop()?; i.push(Obj::Bool(false)); Ok(()) },
        "exitserver" => |i| { i.pop()?; Ok(()) },
        "internaldict" => |i| { i.pop()?; let d = new_dict(10); i.push(Obj::Dict(d)); Ok(()) },
        "setuserparams" => |i| { i.pop()?; Ok(()) },
        "setsystemparams" => |i| { i.pop()?; Ok(()) },
        "setdevparams" => |i| { i.pop()?; i.pop()?; Ok(()) },
        "currentuserparams" => |i| { i.push(Obj::Dict(new_dict(5))); Ok(()) },
        "currentsystemparams" => |i| { i.push(Obj::Dict(new_dict(5))); Ok(()) },
        "currentdevparams" => |i| { i.pop()?; i.push(Obj::Dict(new_dict(5))); Ok(()) },
        "setucacheparams" => |i| { let _ = i.os.iter().rposition(|o| matches!(o, Obj::Mark)).map(|p| i.os.truncate(p)); Ok(()) },
        "ucachestatus" => |i| { i.push(Obj::Mark); for _ in 0..5 { i.push(Obj::Int(0)); } Ok(()) },
        "setcachelimit" => |i| { i.pop()?; Ok(()) },
        "setcacheparams" => |i| { let _ = i.os.iter().rposition(|o| matches!(o, Obj::Mark)).map(|p| i.os.truncate(p)); Ok(()) },
        "currentcacheparams" => |i| { i.push(Obj::Mark); i.push(Obj::Int(1000000)); i.push(Obj::Int(10000)); Ok(()) },
        "cachestatus" => |i| { for _ in 0..7 { i.push(Obj::Int(0)); } Ok(()) },
        "setobjectformat" => |i| { i.pop()?; Ok(()) },
        "currentobjectformat" => |i| { i.push(Obj::Int(0)); Ok(()) },
        "ucache" => |i| Ok(()),
        "setbbox" => |i| { for _ in 0..4 { i.pop()?; } Ok(()) },
        "defineuserobject" => |i| { i.pop()?; i.pop()?; Ok(()) },
        "undefineuserobject" => |i| { i.pop()?; Ok(()) },
        "execuserobject" => |i| { i.pop()?; Ok(()) },
        // ---- graphics state ----
        "gsave" => |i| { if i.gstack.len() > 5000 { return err("limitcheck", "gsave"); } let mut g = i.gs.clone(); g.save_level = None; i.gstack.push(g); Ok(()) },
        "grestore" => |i| {
            if let Some(top) = i.gstack.last() {
                if top.save_level.is_some() { let mut g = top.clone(); g.save_level = None; i.gs = g; }
                else { i.gs = i.gstack.pop().unwrap(); }
            }
            Ok(())
        },
        "grestoreall" => |i| {
            while let Some(top) = i.gstack.last() { if top.save_level.is_some() { let mut g = top.clone(); g.save_level = None; i.gs = g; return Ok(()); } i.gs = i.gstack.pop().unwrap(); }
            Ok(())
        },
        "initgraphics" => |i| { i.initgraphics(); Ok(()) },
        "gstate" => |i| { let g = Rc::new(i.gs.clone()); i.push(Obj::GState(g)); Ok(()) },
        "currentgstate" => |i| { i.pop()?; let g = Rc::new(i.gs.clone()); i.push(Obj::GState(g)); Ok(()) },
        "setgstate" => |i| { if let Obj::GState(g) = i.pop()? { let save = i.gs.save_level; i.gs = (*g).clone(); i.gs.save_level = save; } Ok(()) },
        "setlinewidth" => |i| { i.gs.line_width = i.pop_num()?; Ok(()) },
        "currentlinewidth" => |i| { let v = i.gs.line_width; i.push(Obj::Real(v)); Ok(()) },
        "setlinecap" => |i| { i.gs.cap = match i.pop_int()? { 1 => tiny_skia::LineCap::Round, 2 => tiny_skia::LineCap::Square, _ => tiny_skia::LineCap::Butt }; Ok(()) },
        "currentlinecap" => |i| { let v = match i.gs.cap { tiny_skia::LineCap::Butt => 0, tiny_skia::LineCap::Round => 1, _ => 2 }; i.push(Obj::Int(v)); Ok(()) },
        "setlinejoin" => |i| { i.gs.join = match i.pop_int()? { 1 => tiny_skia::LineJoin::Round, 2 => tiny_skia::LineJoin::Bevel, _ => tiny_skia::LineJoin::Miter }; Ok(()) },
        "currentlinejoin" => |i| { let v = match i.gs.join { tiny_skia::LineJoin::Miter => 0, tiny_skia::LineJoin::Round => 1, _ => 2 }; i.push(Obj::Int(v)); Ok(()) },
        "setmiterlimit" => |i| { i.gs.miter = i.pop_num()?.max(1.0); Ok(()) },
        "currentmiterlimit" => |i| { let v = i.gs.miter; i.push(Obj::Real(v)); Ok(()) },
        "setdash" => |i| { let off = i.pop_num()?; let a = i.pop_arr()?; i.gs.dash = a.to_vec().iter().filter_map(|o| o.num()).collect(); i.gs.dash_off = off; Ok(()) },
        "currentdash" => |i| { let d: Vec<Obj> = i.gs.dash.iter().map(|v| Obj::Real(*v)).collect(); let off = i.gs.dash_off; i.push(Obj::Arr(PArr::new(d), false)); i.push(Obj::Real(off)); Ok(()) },
        "setflat" => |i| { i.gs.flat = i.pop_num()?; Ok(()) },
        "currentflat" => |i| { let v = i.gs.flat; i.push(Obj::Real(v)); Ok(()) },
        "setstrokeadjust" => |i| { i.gs.stroke_adjust = i.pop_bool()?; Ok(()) },
        "currentstrokeadjust" => |i| { let v = i.gs.stroke_adjust; i.push(Obj::Bool(v)); Ok(()) },
        "setoverprint" => |i| { i.pop()?; Ok(()) },
        "currentoverprint" => |i| { i.push(Obj::Bool(false)); Ok(()) },
        "setsmoothness" => |i| { i.pop()?; Ok(()) },
        "currentsmoothness" => |i| { i.push(Obj::Real(0.02)); Ok(()) },
        "settransfer" => |i| { i.gs.transfer = i.pop()?; Ok(()) },
        "currenttransfer" => |i| { let t = match &i.gs.transfer { Obj::Null => Obj::Arr(PArr::new(vec![]), true), t => t.clone() }; i.push(t); Ok(()) },
        "setcolortransfer" => |i| { for _ in 0..4 { i.pop()?; } Ok(()) },
        "currentcolortransfer" => |i| { for _ in 0..4 { i.push(Obj::Arr(PArr::new(vec![]), true)); } Ok(()) },
        "setblackgeneration" => |i| { i.pop()?; Ok(()) },
        "currentblackgeneration" => |i| { i.push(Obj::Arr(PArr::new(vec![]), true)); Ok(()) },
        "setundercolorremoval" => |i| { i.pop()?; Ok(()) },
        "currentundercolorremoval" => |i| { let p = Obj::Name(i.name("pop"), true); i.push(Obj::Arr(PArr::new(vec![p, Obj::Int(0)]), true)); Ok(()) },
        "setscreen" => |i| { for _ in 0..3 { i.pop()?; } Ok(()) },
        "currentscreen" => |i| { i.push(Obj::Real(60.0)); i.push(Obj::Real(45.0)); { let p = Obj::Name(i.name("pop"), true); i.push(Obj::Arr(PArr::new(vec![p.clone(), p, Obj::Int(0)]), true)); } Ok(()) },
        "setcolorscreen" => |i| { for _ in 0..12 { i.pop()?; } Ok(()) },
        "currentcolorscreen" => |i| { for _ in 0..4 { i.push(Obj::Real(60.0)); i.push(Obj::Real(45.0)); { let p = Obj::Name(i.name("pop"), true); i.push(Obj::Arr(PArr::new(vec![p.clone(), p, Obj::Int(0)]), true)); } } Ok(()) },
        "sethalftone" => |i| { i.pop()?; Ok(()) },
        "currenthalftone" => |i| { let d = new_dict(5); let k = i.name("HalftoneType"); i.dput(&d, k, Obj::Int(1)); i.push(Obj::Dict(d)); Ok(()) },
        "sethalftonephase" => |i| { i.pop()?; i.pop()?; Ok(()) },
        "currenthalftonephase" => |i| { i.push(Obj::Int(0)); i.push(Obj::Int(0)); Ok(()) },
        "setcolorrendering" => |i| { i.pop()?; Ok(()) },
        "currentcolorrendering" => |i| { i.push(Obj::Dict(new_dict(5))); Ok(()) },
        "findcolorrendering" => |i| { i.pop()?; let n = i.name_obj("DefaultColorRendering"); i.push(n); i.push(Obj::Bool(false)); Ok(()) },
        // ---- colour ----
        "setgray" => |i| { let g = i.pop_num()?; i.gs.cspace = ColorSpace::Gray; i.gs.cspace_obj = None; i.gs.ccomps = vec![g]; i.gs.pattern = None; i.gs.color = Rgb::gray(g as f32).clamp(); Ok(()) },
        "currentgray" => |i| { let c = i.gs.color; i.push(Obj::Real((0.3 * c.0 + 0.59 * c.1 + 0.11 * c.2) as f64)); Ok(()) },
        "setrgbcolor" => |i| { let b = i.pop_num()?; let g = i.pop_num()?; let r = i.pop_num()?; i.gs.cspace = ColorSpace::Rgb; i.gs.cspace_obj = None; i.gs.ccomps = vec![r, g, b]; i.gs.pattern = None; i.gs.color = Rgb(r as f32, g as f32, b as f32).clamp(); Ok(()) },
        "currentrgbcolor" => |i| { let c = i.gs.color; i.push(Obj::Real(c.0 as f64)); i.push(Obj::Real(c.1 as f64)); i.push(Obj::Real(c.2 as f64)); Ok(()) },
        "sethsbcolor" => |i| {
            let b = i.pop_num()?; let s = i.pop_num()?; let h = i.pop_num()?;
            let hh = (h * 6.0) % 6.0; let f = hh - hh.floor(); let p = b * (1.0 - s); let q = b * (1.0 - s * f); let t = b * (1.0 - s * (1.0 - f));
            let (r, g, bb) = match hh as i64 { 0 => (b, t, p), 1 => (q, b, p), 2 => (p, b, t), 3 => (p, q, b), 4 => (t, p, b), _ => (b, p, q) };
            i.gs.cspace = ColorSpace::Rgb; i.gs.ccomps = vec![r, g, bb]; i.gs.pattern = None; i.gs.color = Rgb(r as f32, g as f32, bb as f32).clamp(); Ok(())
        },
        "currenthsbcolor" => |i| {
            let c = i.gs.color; let (r, g, b) = (c.0 as f64, c.1 as f64, c.2 as f64);
            let mx = r.max(g).max(b); let mn = r.min(g).min(b); let d = mx - mn;
            let h = if d == 0.0 { 0.0 } else if mx == r { ((g - b) / d).rem_euclid(6.0) / 6.0 } else if mx == g { ((b - r) / d + 2.0) / 6.0 } else { ((r - g) / d + 4.0) / 6.0 };
            let s = if mx == 0.0 { 0.0 } else { d / mx };
            i.push(Obj::Real(h)); i.push(Obj::Real(s)); i.push(Obj::Real(mx)); Ok(())
        },
        "setcmykcolor" => |i| { let k = i.pop_num()?; let y = i.pop_num()?; let m = i.pop_num()?; let c = i.pop_num()?; i.gs.cspace = ColorSpace::Cmyk; i.gs.cspace_obj = None; i.gs.ccomps = vec![c, m, y, k]; i.gs.pattern = None; i.gs.color = Rgb::from_cmyk(c as f32, m as f32, y as f32, k as f32); Ok(()) },
        "currentcmykcolor" => |i| {
            if let ColorSpace::Cmyk = i.gs.cspace { let v = i.gs.ccomps.clone(); for x in v { i.push(Obj::Real(x)); } return Ok(()); }
            let c = i.gs.color; let k = 1.0 - c.0.max(c.1).max(c.2);
            let f = |x: f32| if k >= 1.0 { 0.0 } else { ((1.0 - x - k) / (1.0 - k)) as f64 };
            i.push(Obj::Real(f(c.0))); i.push(Obj::Real(f(c.1))); i.push(Obj::Real(f(c.2))); i.push(Obj::Real(k as f64)); Ok(())
        },
        "setcolorspace" => |i| { let o = i.pop()?; let cs = parse_cspace(i, &o)?; set_cspace(i, cs, o) },
        "currentcolorspace" => |i| {
            let o = match &i.gs.cspace_obj { Some(Obj::Arr(a, x)) => Obj::Arr(a.clone(), *x), Some(Obj::Name(n, _)) => Obj::Arr(PArr::new(vec![Obj::Name(*n, false)]), false), _ => { let n = i.name(i.gs.cspace.name()); Obj::Arr(PArr::new(vec![Obj::Name(n, false)]), false) } };
            i.push(o); Ok(())
        },
        "setcolor" => |i| {
            if let ColorSpace::Pattern { under } = i.gs.cspace.clone() {
                let p = i.pop()?;
                if let Obj::Dict(d) = p { i.gs.pattern = Some(d); }
                if let Some(u) = under {
                    let n = u.ncomps(); let mut c = vec![0.0; n];
                    for k in (0..n).rev() { c[k] = i.pop_num()?; }
                    i.gs.color = i.convert_color(&u, &c)?;
                }
                return Ok(());
            }
            let n = i.gs.cspace.ncomps();
            let mut c = vec![0.0; n];
            for k in (0..n).rev() { c[k] = i.pop_num()?; }
            i.gs.ccomps = c;
            i.set_color_from_comps()
        },
        "currentcolor" => |i| { let v = i.gs.ccomps.clone(); for x in v { i.push(Obj::Real(x)); } Ok(()) },
        "makepattern" => |i| {
            let m = i.pop_matrix()?; let d = i.pop_dict()?;
            let nd = copy_dict(&d);
            let full = m.then(&i.gs.ctm);
            let k = i.names_cache.implementation;
            i.dput(&nd, k, matrix_arr(&full));
            i.push(Obj::Dict(nd)); Ok(())
        },
        "setpattern" => |i| {
            let d = i.pop_dict()?;
            let kp = i.names_cache.paint_type;
            let pt = i.dget(&d, kp).and_then(|o| o.num()).unwrap_or(1.0) as i64;
            let under = if pt == 2 { Some(Rc::new(i.gs.cspace.clone())) } else { None };
            if pt == 2 {
                if let Some(u) = &under { let n = u.ncomps(); let mut c = vec![0.0; n]; for k in (0..n).rev() { c[k] = i.pop_num().unwrap_or(0.0); } i.gs.color = i.convert_color(u, &c)?; }
            }
            i.gs.cspace = ColorSpace::Pattern { under };
            i.gs.pattern = Some(d);
            Ok(())
        },
        // ---- matrices ----
        "matrix" => |i| { i.push(matrix_arr(&Matrix::identity())); Ok(()) },
        "initmatrix" => |i| { i.gs.ctm = i.default_matrix(); Ok(()) },
        "identmatrix" => |i| { let a = i.pop_arr()?; fill_matrix(&a, &Matrix::identity())?; i.push(Obj::Arr(a, false)); Ok(()) },
        "defaultmatrix" => |i| { let a = i.pop_arr()?; let m = i.default_matrix(); fill_matrix(&a, &m)?; i.push(Obj::Arr(a, false)); Ok(()) },
        "currentmatrix" => |i| { let a = i.pop_arr()?; let m = i.gs.ctm; fill_matrix(&a, &m)?; i.push(Obj::Arr(a, false)); Ok(()) },
        "setmatrix" => |i| { i.gs.ctm = i.pop_matrix()?; Ok(()) },
        "translate" => |i| matrix_modify(i, |i| { let y = i.pop_num()?; let x = i.pop_num()?; Ok(Matrix::translate(x, y)) }),
        "scale" => |i| matrix_modify(i, |i| { let y = i.pop_num()?; let x = i.pop_num()?; Ok(Matrix::scale(x, y)) }),
        "rotate" => |i| matrix_modify(i, |i| { let a = i.pop_num()?; Ok(Matrix::rotate_deg(a)) }),
        "concat" => |i| { let m = i.pop_matrix()?; i.gs.ctm = m.then(&i.gs.ctm); Ok(()) },
        "concatmatrix" => |i| { let a3 = i.pop_arr()?; let m2 = i.pop_matrix()?; let m1 = i.pop_matrix()?; fill_matrix(&a3, &m1.then(&m2))?; i.push(Obj::Arr(a3, false)); Ok(()) },
        "invertmatrix" => |i| { let a2 = i.pop_arr()?; let m1 = i.pop_matrix()?; let inv = m1.invert().ok_or(PsError::Err("undefinedresult", "invertmatrix".into()))?; fill_matrix(&a2, &inv)?; i.push(Obj::Arr(a2, false)); Ok(()) },
        "transform" => |i| transform_op(i, false, false),
        "itransform" => |i| transform_op(i, true, false),
        "dtransform" => |i| transform_op(i, false, true),
        "idtransform" => |i| transform_op(i, true, true),
        // ---- paths ----
        "newpath" => |i| { i.gs.path.clear(); Ok(()) },
        "currentpoint" => |i| { let (x, y) = cur_user(i)?; i.push(Obj::Real(x)); i.push(Obj::Real(y)); Ok(()) },
        "moveto" => |i| { let y = i.pop_num()?; let x = i.pop_num()?; let (a, b) = i.gs.ctm.apply(x, y); i.gs.path.move_to(a, b); Ok(()) },
        "rmoveto" => |i| { let dy = i.pop_num()?; let dx = i.pop_num()?; let c = i.gs.path.cur.ok_or(PsError::Err("nocurrentpoint", "rmoveto".into()))?; let (a, b) = i.gs.ctm.apply_delta(dx, dy); i.gs.path.move_to(c.0 + a, c.1 + b); Ok(()) },
        "lineto" => |i| { let y = i.pop_num()?; let x = i.pop_num()?; if i.gs.path.cur.is_none() { return err("nocurrentpoint", "lineto"); } let (a, b) = i.gs.ctm.apply(x, y); i.gs.path.line_to(a, b); Ok(()) },
        "rlineto" => |i| { let dy = i.pop_num()?; let dx = i.pop_num()?; let c = i.gs.path.cur.ok_or(PsError::Err("nocurrentpoint", "rlineto".into()))?; let (a, b) = i.gs.ctm.apply_delta(dx, dy); i.gs.path.line_to(c.0 + a, c.1 + b); Ok(()) },
        "curveto" => |i| {
            let y3 = i.pop_num()?; let x3 = i.pop_num()?; let y2 = i.pop_num()?; let x2 = i.pop_num()?; let y1 = i.pop_num()?; let x1 = i.pop_num()?;
            if i.gs.path.cur.is_none() { return err("nocurrentpoint", "curveto"); }
            let m = i.gs.ctm; let (a, b) = m.apply(x1, y1); let (c, d) = m.apply(x2, y2); let (e, f) = m.apply(x3, y3);
            i.gs.path.curve_to(a, b, c, d, e, f); Ok(())
        },
        "rcurveto" => |i| {
            let y3 = i.pop_num()?; let x3 = i.pop_num()?; let y2 = i.pop_num()?; let x2 = i.pop_num()?; let y1 = i.pop_num()?; let x1 = i.pop_num()?;
            let c0 = i.gs.path.cur.ok_or(PsError::Err("nocurrentpoint", "rcurveto".into()))?;
            let m = i.gs.ctm; let (a, b) = m.apply_delta(x1, y1); let (c, d) = m.apply_delta(x2, y2); let (e, f) = m.apply_delta(x3, y3);
            i.gs.path.curve_to(c0.0 + a, c0.1 + b, c0.0 + c, c0.1 + d, c0.0 + e, c0.1 + f); Ok(())
        },
        "closepath" => |i| { i.gs.path.close(); Ok(()) },
        "arc" => |i| { let a2 = i.pop_num()?; let a1 = i.pop_num()?; let r = i.pop_num()?; let y = i.pop_num()?; let x = i.pop_num()?; let m = i.gs.ctm; i.gs.path.arc(&m, x, y, r, a1, a2, true, true); Ok(()) },
        "arcn" => |i| { let a2 = i.pop_num()?; let a1 = i.pop_num()?; let r = i.pop_num()?; let y = i.pop_num()?; let x = i.pop_num()?; let m = i.gs.ctm; i.gs.path.arc(&m, x, y, r, a1, a2, false, true); Ok(()) },
        "arct" => |i| arct(i, false),
        "arcto" => |i| arct(i, true),
        "flattenpath" => |i| { i.gs.path = i.gs.path.flattened(); Ok(()) },
        "reversepath" => |i| { i.gs.path = i.gs.path.reversed(); Ok(()) },
        "strokepath" => |i| strokepath(i),
        "clippath" => |i| { i.gs.path = i.gs.clip_path.clone().unwrap_or_else(|| i.page_rect_dev()); Ok(()) },
        "pathbbox" => |i| {
            let b = i.gs.path.bbox().ok_or(PsError::Err("nocurrentpoint", "pathbbox".into()))?;
            let inv = i.gs.ctm.invert().unwrap_or_default();
            let pts = [inv.apply(b.0, b.1), inv.apply(b.2, b.1), inv.apply(b.0, b.3), inv.apply(b.2, b.3)];
            let x0 = pts.iter().map(|p| p.0).fold(f64::MAX, f64::min); let y0 = pts.iter().map(|p| p.1).fold(f64::MAX, f64::min);
            let x1 = pts.iter().map(|p| p.0).fold(f64::MIN, f64::max); let y1 = pts.iter().map(|p| p.1).fold(f64::MIN, f64::max);
            for v in [x0, y0, x1, y1] { i.push(Obj::Real(v)); } Ok(())
        },
        "pathforall" => |i| {
            let close = i.pop()?; let curve = i.pop()?; let line = i.pop()?; let mv = i.pop()?;
            let inv = i.gs.ctm.invert().unwrap_or_default();
            let segs = i.gs.path.segs.clone();
            for s in segs {
                match s {
                    Seg::Move(x, y) => { let (a, b) = inv.apply(x, y); i.push(Obj::Real(a)); i.push(Obj::Real(b)); i.call(mv.clone())?; }
                    Seg::Line(x, y) => { let (a, b) = inv.apply(x, y); i.push(Obj::Real(a)); i.push(Obj::Real(b)); i.call(line.clone())?; }
                    Seg::Cubic(a, b, c, d, e, f) => { for (x, y) in [(a, b), (c, d), (e, f)] { let (p, q) = inv.apply(x, y); i.push(Obj::Real(p)); i.push(Obj::Real(q)); } i.call(curve.clone())?; }
                    Seg::Close => i.call(close.clone())?,
                }
            }
            Ok(())
        },
        "clipsave" => |i| { let c = (i.gs.clip.clone(), i.gs.clip_path.clone()); i.clip_stack.push(c); Ok(()) },
        "cliprestore" => |i| { if let Some((c, p)) = i.clip_stack.pop() { i.gs.clip = c; i.gs.clip_path = p; } Ok(()) },
        "initclip" => |i| { i.gs.clip = None; i.gs.clip_path = None; Ok(()) },
        "clip" => |i| { let p = i.gs.path.clone(); i.clip_dev(&p, false); Ok(()) },
        "eoclip" => |i| { let p = i.gs.path.clone(); i.clip_dev(&p, true); Ok(()) },
        "rectclip" => |i| { let r = rect_args(i)?; let p = rect_path(i, &r); i.clip_dev(&p, false); i.gs.path.clear(); Ok(()) },
        "uappend|ufill|ueofill|ustroke|upath" => |i| { i.note_unsupported("user paths"); Ok(()) },
        // ---- painting ----
        "erasepage" => |i| { i.page.items.clear(); i.page_dirty = false; Ok(()) },
        "fill" => |i| { let p = std::mem::take(&mut i.gs.path); i.fill_dev(&p, false); Ok(()) },
        "eofill" => |i| { let p = std::mem::take(&mut i.gs.path); i.fill_dev(&p, true); Ok(()) },
        "stroke" => |i| { let p = std::mem::take(&mut i.gs.path); i.stroke_dev(&p); Ok(()) },
        "rectfill" => |i| { let r = rect_args(i)?; let p = rect_path(i, &r); i.fill_dev(&p, false); Ok(()) },
        "rectstroke" => |i| {
            let m = if i.top_is_matrix() { Some(i.pop_matrix()?) } else { None };
            let r = rect_args(i)?;
            let saved = i.gs.ctm;
            if let Some(m) = m { i.gs.ctm = m.then(&saved); }
            let p = rect_path(i, &r); i.stroke_dev(&p);
            i.gs.ctm = saved; Ok(())
        },
        "image" => |i| image_op(i, false, false),
        "imagemask" => |i| image_op(i, true, false),
        "colorimage" => |i| image_op(i, false, true),
        "shfill" => |i| { let d = i.pop_dict()?; shfill(i, d) },
        "showpage" => |i| do_showpage(i),
        "copypage" => |i| { i.note_unsupported("copypage"); Ok(()) },
        "nulldevice" => |i| { i.note_unsupported("nulldevice"); Ok(()) },
        "setpagedevice" => |i| setpagedevice(i),
        "currentpagedevice" => |i| currentpagedevice(i),
        "infill|ineofill|instroke|inufill|inueofill|inustroke" => |i| { let o = i.pop()?; if !matches!(o, Obj::Arr(..)) { i.pop()?; } i.push(Obj::Bool(false)); Ok(()) },
        // ---- fonts ----
        "findfont" => |i| { let k = i.pop()?; let d = find_font(i, k)?; i.push(Obj::Dict(d)); Ok(()) },
        "definefont" => |i| { let d = i.pop_dict()?; let k = i.pop()?; define_font(i, k, d.clone())?; i.push(Obj::Dict(d)); Ok(()) },
        "undefinefont" => |i| { let k = i.pop()?; let key = i.key(&k)?; i.font_dir.borrow_mut().map.remove(&key); Ok(()) },
        "scalefont" => |i| { let s = i.pop_num()?; let f = i.pop_dict()?; make_font(i, f, Matrix::scale(s, s)) },
        "makefont" => |i| { let m = i.pop_matrix()?; let f = i.pop_dict()?; make_font(i, f, m) },
        "setfont" => |i| { let f = i.pop_dict()?; i.gs.font = Some(f); Ok(()) },
        "currentfont" => |i| { let f = match i.gs.font.clone() { Some(f) => f, None => { let n = b"Courier".to_vec(); i.substitute_font(&n) } }; i.push(Obj::Dict(f)); Ok(()) },
        "rootfont" => |i| { let f = match i.gs.font.clone() { Some(f) => f, None => { let n = b"Courier".to_vec(); i.substitute_font(&n) } }; i.push(Obj::Dict(f)); Ok(()) },
        "selectfont" => |i| {
            let s = i.pop()?; let k = i.pop()?; let f = find_font(i, k)?;
            match s { Obj::Arr(a, _) => { let m = arr_matrix(&a).unwrap_or_default(); make_font(i, f, m)?; } o => { let v = o.num().unwrap_or(12.0); make_font(i, f, Matrix::scale(v, v))?; } }
            let f = i.pop_dict()?; i.gs.font = Some(f); Ok(())
        },
        "show" => |i| show_op(i, ShowOpts::default()),
        "ashow" => |i| { let s = i.pop()?; let ay = i.pop_num()?; let ax = i.pop_num()?; i.push(s); show_op(i, ShowOpts { ax, ay, ..Default::default() }) },
        "widthshow" => |i| { let s = i.pop()?; let c = i.pop_int()?; let cy = i.pop_num()?; let cx = i.pop_num()?; i.push(s); show_op(i, ShowOpts { cx, cy, wchar: Some(c as u32), ..Default::default() }) },
        "awidthshow" => |i| { let s = i.pop()?; let ay = i.pop_num()?; let ax = i.pop_num()?; let c = i.pop_int()?; let cy = i.pop_num()?; let cx = i.pop_num()?; i.push(s); show_op(i, ShowOpts { ax, ay, cx, cy, wchar: Some(c as u32), ..Default::default() }) },
        "kshow" => |i| { let s = i.pop()?; let p = i.pop()?; i.push(s); show_op(i, ShowOpts { kproc: Some(p), ..Default::default() }) },
        "cshow" => |i| { let s = i.pop()?; let _p = i.pop()?; i.note_unsupported("cshow (drawn as show)"); i.push(s); show_op(i, ShowOpts::default()) },
        "xshow|yshow|xyshow" => |i| {
            let nums = i.pop()?; let nv: Vec<f64> = match nums { Obj::Arr(a, _) => a.to_vec().iter().filter_map(|o| o.num()).collect(), _ => { i.note_unsupported("encoded number strings"); Vec::new() } };
            // the specific operator name is not known here; decide by count
            let s = i.pop_str()?; let n = s.len;
            let kind = if nv.len() >= 2 * n && n > 0 { 3 } else { 1 };
            i.push(Obj::Str(s, false));
            show_op(i, ShowOpts { xy: Some((nv, kind)), ..Default::default() })
        },
        "glyphshow" => |i| {
            let n = i.pop()?;
            let Some(f) = i.gs.font.clone() else { return err("invalidfont", "glyphshow") };
            // temporarily use a one-glyph encoding
            let nf = copy_dict(&f); let ke = i.names_cache.encoding;
            let name = match n { Obj::Name(x, _) => x, Obj::Int(c) => { i.push(Obj::Str(PStr::new(vec![(c & 0xFF) as u8, ]), false)); return show_op(i, ShowOpts::default()); } _ => return err("typecheck", "glyphshow") };
            let mut enc = vec![Obj::Name(i.names_cache.notdef, false); 256]; enc[0] = Obj::Name(name, false);
            i.dput(&nf, ke, Obj::Arr(PArr::new(enc), false));
            i.gs.font = Some(nf);
            let r = i.show(&[0], ShowOpts::default());
            i.gs.font = Some(f);
            r.map(|_| ())
        },
        "stringwidth" => |i| { let s = i.pop_str()?; let (w, h) = i.show(&s.bytes(), ShowOpts { measure: true, ..Default::default() })?; i.push(Obj::Real(w)); i.push(Obj::Real(h)); Ok(()) },
        "charpath" => |i| { let _stroke = i.pop_bool()?; let s = i.pop_str()?; i.show(&s.bytes(), ShowOpts { charpath: true, ..Default::default() })?; Ok(()) },
        "setcachedevice" => |i| { for _ in 0..4 { i.pop_num()?; } let wy = i.pop_num()?; let wx = i.pop_num()?; i.char_width = Some((wx, wy)); Ok(()) },
        "setcachedevice2" => |i| { for _ in 0..6 { i.pop_num()?; } let wy = i.pop_num()?; let wx = i.pop_num()?; i.char_width = Some((wx, wy)); Ok(()) },
        "setcharwidth" => |i| { let wy = i.pop_num()?; let wx = i.pop_num()?; i.char_width = Some((wx, wy)); Ok(()) },
        "composefont" => |i| {
            let fonts_o = i.pop()?; let cmap = i.pop()?; let key = i.pop()?;
            let d = new_dict(10);
            let nc = &i.names_cache; let (kft, kfm, kfmt, kfdv, kcm, kfn, kenc) = (nc.font_type, nc.font_matrix, nc.fmap_type, nc.fdep_vector, nc.cmap, nc.font_name, nc.encoding);
            i.dput(&d, kft, Obj::Int(0)); i.dput(&d, kfm, matrix_arr(&Matrix::identity())); i.dput(&d, kfmt, Obj::Int(9));
            let mut desc = Vec::new();
            if let Obj::Arr(a, _) = fonts_o { for f in a.to_vec() { match f { Obj::Dict(fd) => desc.push(Obj::Dict(fd)), o => { let cat = res_category(i, b"CIDFont"); let k = i.key(&o)?; let v = cat.borrow().map.get(&k).cloned(); match v { Some(v) => desc.push(v), None => { let fd = find_font(i, o)?; desc.push(Obj::Dict(fd)); } } } } } }
            i.dput(&d, kfdv, Obj::Arr(PArr::new(desc), false));
            let cm = match cmap { Obj::Dict(c) => Obj::Dict(c), o => { let cat = res_category(i, b"CMap"); let k = i.key(&o)?; let v = cat.borrow().map.get(&k).cloned(); v.unwrap_or(Obj::Dict(new_dict(1))) } };
            i.dput(&d, kcm, cm);
            i.dput(&d, kfn, key.clone());
            i.dput(&d, kenc, Obj::Arr(PArr::new(vec![Obj::Int(0)]), false));
            define_font(i, key, d.clone())?;
            i.push(Obj::Dict(d)); Ok(())
        },
        // ---- resources ----
        "findresource" => |i| {
            let cat = i.pop()?; let key = i.pop()?;
            let cn = i.obj_to_bytes(&cat).unwrap_or_default();
            if cn == b"Font" { let d = find_font(i, key)?; i.push(Obj::Dict(d)); return Ok(()); }
            let c = res_category(i, &cn); let k = i.key(&key)?;
            let v = c.borrow().map.get(&k).cloned();
            match v {
                Some(v) => { i.push(v); Ok(()) }
                None => {
                    let kn = i.obj_to_bytes(&key).unwrap_or_default();
                    match (cn.as_slice(), kn.as_slice()) {
                        (b"Encoding", b"StandardEncoding") => { let e = i.std_enc.clone(); i.push(Obj::Arr(e, false)); Ok(()) }
                        (b"Encoding", b"ISOLatin1Encoding") => { let e = i.iso_enc.clone(); i.push(Obj::Arr(e, false)); Ok(()) }
                        (b"CMap", _) => { let d = new_dict(5); let n = i.names_cache.cmap_name; i.dput(&d, n, key); i.push(Obj::Dict(d)); Ok(()) }
                        (b"ProcSet", _) | (b"Category", _) | (b"ColorRendering", _) | (b"Halftone", _) => { i.push(Obj::Dict(new_dict(10))); Ok(()) }
                        _ => err("undefinedresource", String::from_utf8_lossy(&kn).to_string()),
                    }
                }
            }
        },
        "defineresource" => |i| {
            let cat = i.pop()?; let inst = i.pop()?; let key = i.pop()?;
            let cn = i.obj_to_bytes(&cat).unwrap_or_default();
            if cn == b"Font" { if let Obj::Dict(d) = &inst { define_font(i, key.clone(), d.clone())?; } }
            if cn == b"CIDFont" { if let Obj::Dict(d) = &inst { let name = i.obj_to_bytes(&key).map(|b| String::from_utf8_lossy(&b).to_string()).unwrap_or_default(); let t = i.font_type(d); i.stats.fonts_defined.entry(name).or_insert_with(|| format!("CIDFontType {}", t - 100)); let fid = i.names_cache.fid; i.next_fid += 1; i.dput(d, fid, Obj::FontId(i.next_fid)); } }
            let c = res_category(i, &cn); let k = i.key(&key)?;
            c.borrow_mut().map.insert(k, inst.clone());
            i.push(inst); Ok(())
        },
        "undefineresource" => |i| { let cat = i.pop()?; let key = i.pop()?; let cn = i.obj_to_bytes(&cat).unwrap_or_default(); let c = res_category(i, &cn); let k = i.key(&key)?; c.borrow_mut().map.remove(&k); Ok(()) },
        "resourcestatus" => |i| {
            let cat = i.pop()?; let key = i.pop()?; let cn = i.obj_to_bytes(&cat).unwrap_or_default();
            let k = i.key(&key)?;
            let found = if cn == b"FontType" { matches!(key.num().map(|v| v as i64), Some(0 | 1 | 3 | 42)) } else if cn == b"Font" { i.font_dir.borrow().map.contains_key(&k) } else { res_category(i, &cn).borrow().map.contains_key(&k) };
            if found { i.push(Obj::Int(1)); i.push(Obj::Int(0)); i.push(Obj::Bool(true)); } else { i.push(Obj::Bool(false)); }
            Ok(())
        },
        "resourceforall" => |i| { for _ in 0..4 { i.pop()?; } Ok(()) },
    };
    // Some entries share a body with several names separated by '|': expand them.
    for (names, f) in list {
        for n in names.split('|') {
            let idx = i.ops.len() as u16;
            i.ops.push((Box::leak(n.to_string().into_boxed_str()), f));
            i.def_sys(n, Obj::Op(idx));
        }
    }

    // values in systemdict
    let sd = i.systemdict.clone();
    let ud = i.userdict.clone();
    let gd = i.globaldict.clone();
    let st = i.statusdict.clone();
    let fdir = i.font_dir.clone();
    let ed = i.errordict.clone();
    let de = i.dollar_error.clone();
    i.def_sys("systemdict", Obj::Dict(sd));
    i.def_sys("userdict", Obj::Dict(ud));
    i.def_sys("globaldict", Obj::Dict(gd));
    i.def_sys("statusdict", Obj::Dict(st));
    i.def_sys("FontDirectory", Obj::Dict(fdir.clone()));
    i.def_sys("GlobalFontDirectory", Obj::Dict(fdir));
    i.def_sys("errordict", Obj::Dict(ed.clone()));
    i.def_sys("$error", Obj::Dict(de));
    i.def_sys("serverdict", Obj::Dict(new_dict(10)));
    i.def_sys("shareddict", Obj::Dict(new_dict(10)));
    i.def_sys("SharedFontDirectory", Obj::Dict(new_dict(10)));
    i.def_sys("true", Obj::Bool(true));
    i.def_sys("false", Obj::Bool(false));
    for e in ["dictfull", "dictstackoverflow", "dictstackunderflow", "execstackoverflow", "handleerror", "interrupt", "invalidaccess", "invalidexit", "invalidfileaccess", "invalidfont", "invalidrestore", "ioerror", "limitcheck", "nocurrentpoint", "rangecheck", "stackoverflow", "stackunderflow", "syntaxerror", "timeout", "typecheck", "undefined", "undefinedfilename", "undefinedresult", "unmatchedmark", "unregistered", "VMerror"] {
        let k = i.name(e);
        let stop = i.lookup_name(i.names.map[b"stop".as_slice()]).unwrap();
        i.dput(&ed, k, Obj::Arr(PArr::new(vec![stop]), true));
    }
    let std: Vec<Obj> = fonts::standard_encoding().iter().map(|n| Obj::Name(i.name(n), false)).collect();
    let iso: Vec<Obj> = fonts::isolatin1_encoding().iter().map(|n| Obj::Name(i.name(n), false)).collect();
    i.std_enc = PArr::new(std);
    i.iso_enc = PArr::new(iso);
    let (se, ie) = (i.std_enc.clone(), i.iso_enc.clone());
    i.def_sys("StandardEncoding", Obj::Arr(se, false));
    i.def_sys("ISOLatin1Encoding", Obj::Arr(ie, false));

    // run the prelude with systemdict as current dict
    let saved = std::mem::take(&mut i.ds);
    let sd = i.systemdict.clone();
    i.ds = vec![sd.clone(), sd.clone(), sd];
    let f = file::from_vec(PRELUDE.as_bytes().to_vec());
    i.es.push(Frame::File(f));
    if let Err(PsError::Err(n, d)) = i.run_until(0) {
        i.warn(format!("internal prelude error: {n} {d}"));
    }
    i.os.clear();
    i.ds = saved;
    i.steps = 0;
}
