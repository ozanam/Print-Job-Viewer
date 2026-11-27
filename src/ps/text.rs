//! Fonts and text rendering for the PostScript interpreter.

use super::obj::*;
use super::type1;
use super::*;
use crate::fonts::{self, SfntInfo};
use std::rc::Rc;

#[derive(Default)]
pub struct ShowOpts {
    pub ax: f64,
    pub ay: f64,
    pub cx: f64,
    pub cy: f64,
    pub wchar: Option<u32>,
    pub kproc: Option<Obj>,
    /// xshow (1), yshow (2), xyshow (3) displacement numbers
    pub xy: Option<(Vec<f64>, u8)>,
    pub charpath: bool,
    pub measure: bool,
}

struct GlyphRef {
    font: PDict,
    fm: Matrix,
    code: u32,
    ftype: i64,
}

impl Interp<'_> {
    pub fn font_matrix(&self, f: &PDict) -> Matrix {
        match self.dget(f, self.names_cache.font_matrix) {
            Some(Obj::Arr(a, _)) => arr_matrix(&a).unwrap_or(Matrix::scale(0.001, 0.001)),
            _ => Matrix::scale(0.001, 0.001),
        }
    }

    pub fn font_type(&self, f: &PDict) -> i64 {
        if let Some(t) = self.dget(f, self.names_cache.cid_font_type).and_then(|o| o.num()) {
            return 100 + t as i64; // CIDFont
        }
        self.dget(f, self.names_cache.font_type).and_then(|o| o.num()).unwrap_or(1.0) as i64
    }

    pub fn font_display_name(&self, f: &PDict) -> String {
        match self.dget(f, self.names_cache.font_name) {
            Some(o) => self.obj_to_bytes(&o).map(|b| String::from_utf8_lossy(&b).to_string()).unwrap_or_else(|| "?".into()),
            None => match self.dget(f, self.names.map.get(b"CIDFontName".as_slice()).copied().unwrap_or(u32::MAX)).or_else(|| self.dget(f, self.names.map.get(b".DefName".as_slice()).copied().unwrap_or(u32::MAX))) {
                Some(o) => self.obj_to_bytes(&o).map(|b| String::from_utf8_lossy(&b).to_string()).unwrap_or_default(),
                None => "(unnamed)".into(),
            },
        }
    }

    fn encoding_name(&self, f: &PDict, code: u32) -> Option<NameId> {
        match self.dget(f, self.names_cache.encoding) {
            Some(Obj::Arr(a, _)) => match a.get(code as usize) {
                Some(Obj::Name(n, _)) => Some(n),
                Some(Obj::Str(s, _)) => self.names.map.get(&s.bytes()).copied(),
                _ => None,
            },
            _ => None,
        }
    }

    fn decode_string(&mut self, font: &PDict, s: &[u8]) -> Vec<GlyphRef> {
        let ft = self.font_type(font);
        let fm = self.font_matrix(font);
        let mut out = Vec::new();
        if ft != 0 {
            for &b in s {
                out.push(GlyphRef { font: font.clone(), fm, code: b as u32, ftype: ft });
            }
            return out;
        }
        let fmap = self.dget(font, self.names_cache.fmap_type).and_then(|o| o.num()).unwrap_or(2.0) as i64;
        let fdv: Vec<Obj> = match self.dget(font, self.names_cache.fdep_vector) {
            Some(Obj::Arr(a, _)) => a.to_vec(),
            _ => Vec::new(),
        };
        let desc = |i: usize| -> Option<PDict> {
            match fdv.get(i) {
                Some(Obj::Dict(d)) => Some(d.clone()),
                _ => None,
            }
        };
        if fmap == 9 {
            let Some(d) = desc(0) else { return out };
            let dfm = self.font_matrix(&d).then(&fm);
            let dft = self.font_type(&d);
            let one_byte = self.cmap_is_one_byte(font);
            let mut i = 0;
            while i < s.len() {
                let code = if one_byte || i + 1 >= s.len() {
                    i += 1;
                    s[i - 1] as u32
                } else {
                    i += 2;
                    (s[i - 2] as u32) << 8 | s[i - 1] as u32
                };
                out.push(GlyphRef { font: d.clone(), fm: dfm, code, ftype: dft });
            }
        } else {
            // 8/8 mapping (and approximation for other FMapTypes)
            let enc: Vec<Obj> = match self.dget(font, self.names_cache.encoding) {
                Some(Obj::Arr(a, _)) => a.to_vec(),
                _ => Vec::new(),
            };
            let mut i = 0;
            while i + 1 < s.len() {
                let fi = enc.get(s[i] as usize).and_then(|o| o.num()).unwrap_or(s[i] as f64) as usize;
                if let Some(d) = desc(fi) {
                    let dfm = self.font_matrix(&d).then(&fm);
                    let dft = self.font_type(&d);
                    out.push(GlyphRef { font: d, fm: dfm, code: s[i + 1] as u32, ftype: dft });
                }
                i += 2;
            }
        }
        out
    }

    fn cmap_is_one_byte(&self, font: &PDict) -> bool {
        if let Some(Obj::Dict(cm)) = self.dget(font, self.names_cache.cmap) {
            if let Some(Obj::Int(n)) = cm.borrow().map.get(&Key::Name(self.names.map.get(b".CodeBytes".as_slice()).copied().unwrap_or(u32::MAX))) {
                return *n == 1;
            }
        }
        false
    }

    fn sfnt_for(&mut self, font: &PDict) -> Option<Rc<SfntInfo>> {
        let Some(Obj::Arr(a, _)) = self.dget(font, self.names_cache.sfnts) else { return None };
        let key = a.ptr();
        if let Some(s) = self.sfnt_cache.get(&key) {
            return Some(s.clone());
        }
        let mut all = Vec::new();
        for o in a.to_vec() {
            if let Obj::Str(s, _) = o {
                let b = s.bytes();
                let n = if b.len() % 2 == 1 { b.len() - 1 } else { b.len() };
                all.extend_from_slice(&b[..n]);
            }
        }
        let info = Rc::new(SfntInfo::parse(&all));
        self.sfnt_cache.insert(key, info.clone());
        Some(info)
    }

    fn glyph_dir_data(&self, font: &PDict, gid: u16) -> Option<Vec<u8>> {
        let gd = self.dget(font, self.names_cache.glyph_directory)?;
        let mc = self.dget(font, self.names.map.get(b"MetricsCount".as_slice()).copied().unwrap_or(u32::MAX)).and_then(|o| o.num()).unwrap_or(0.0) as usize;
        let s = match gd {
            Obj::Dict(d) => d.borrow().map.get(&Key::Int(gid as i64)).cloned(),
            Obj::Arr(a, _) => a.get(gid as usize),
            _ => None,
        }?;
        match s {
            Obj::Str(s, _) => {
                let b = s.bytes();
                Some(b[(mc * 2).min(b.len())..].to_vec())
            }
            _ => None,
        }
    }

    fn tt_glyph(&mut self, font: &PDict, gid: u16) -> Rc<(PathAcc, f64, f64)> {
        let sf = self.sfnt_for(font).unwrap_or_else(|| Rc::new(SfntInfo { upem: 1000.0, ..Default::default() }));
        let key = (Rc::as_ptr(&sf) as usize ^ (Rc::as_ptr(font) as usize).rotate_left(17), gid.to_be_bytes().to_vec());
        if let Some(g) = self.glyph_cache.get(&key) {
            return g.clone();
        }
        let has_gd = self.dget(font, self.names_cache.glyph_directory).is_some();
        let data = if has_gd { self.glyph_dir_data(font, gid) } else { None }.or_else(|| sf.glyph_data(gid));
        let mut p = PathAcc::new();
        let upem = sf.upem;
        let m = Matrix::scale(1.0 / upem, 1.0 / upem);
        if let Some(d) = data {
            let lookup = |g: u16| -> Option<Vec<u8>> {
                if has_gd {
                    if let Some(x) = self.glyph_dir_data(font, g) {
                        return Some(x);
                    }
                }
                sf.glyph_data(g)
            };
            fonts::glyf_outline(&d, &m, &mut p, &lookup, 0);
        }
        let w = sf.advance(gid).unwrap_or(upem * 0.5) / upem;
        let g = Rc::new((p, w, 0.0));
        self.glyph_cache.insert(key, g.clone());
        g
    }

    fn sub_glyph(&mut self, font: &PDict, code: u32, gname: Option<NameId>, em1: bool) -> Rc<(PathAcc, f64, f64)> {
        let fname = match self.dget(font, self.names_cache.orig_font).or_else(|| self.dget(font, self.names_cache.font_name)) {
            Some(o) => self.obj_to_bytes(&o).map(|b| String::from_utf8_lossy(&b).to_string()).unwrap_or_default(),
            None => String::new(),
        };
        let sf = fonts::system_font(&fname);
        let nm = gname.filter(|n| *n != self.names_cache.notdef).map(|n| self.names.get(n).to_vec()).unwrap_or_default();
        let key = (Arc::as_ptr(&sf) as *const u8 as usize + em1 as usize, [nm.clone(), code.to_be_bytes().to_vec()].concat());
        if let Some(g) = self.glyph_cache.get(&key) {
            return g.clone();
        }
        let gid = if sf.symbol {
            char::from_u32(code).and_then(|c| sf.glyph_for_char(c))
        } else {
            let s = String::from_utf8_lossy(&nm).to_string();
            fonts::glyph_name_to_unicode(&s).and_then(|c| sf.glyph_for_char(c)).or_else(|| sf.glyph_by_name(&s)).or_else(|| if nm.is_empty() { char::from_u32(code).and_then(|c| sf.glyph_for_char(c)) } else { None })
        };
        let scale = if em1 { 1.0 / sf.upem } else { 1000.0 / sf.upem };
        let mut p = PathAcc::new();
        let mut w = if em1 { 0.5 } else { 500.0 };
        if let Some(g) = gid {
            sf.outline(g, &Matrix::scale(scale, scale), &mut p);
            w = sf.advance(g) * scale;
        } else if nm == b"space" || code == 32 {
            w = if em1 { 0.25 } else { 250.0 };
        }
        let g = Rc::new((p, w, 0.0));
        self.glyph_cache.insert(key, g.clone());
        g
    }

    /// Glyph outline in glyph space plus advance width (glyph space).
    fn glyph_outline(&mut self, gr: &GlyphRef) -> Rc<(PathAcc, f64, f64)> {
        let font = &gr.font;
        let nc = &self.names_cache;
        let (k_cs, k_sub, k_priv, k_subrs, k_leniv, k_notdef) = (nc.char_strings, nc.sub, nc.private, nc.subrs, nc.len_iv, nc.notdef);
        if self.dget(font, k_sub).is_some() {
            let gname = self.encoding_name(font, gr.code);
            return self.sub_glyph(font, gr.code, gname, false);
        }
        match gr.ftype {
            102 => {
                // CIDFontType 2: CID -> GID
                let gid = self.cid_to_gid(font, gr.code);
                return self.tt_glyph(font, gid);
            }
            100 | 101 => {
                self.note_unsupported("CIDFontType 0 (CFF) fonts — substituted");
                return self.sub_glyph(font, gr.code, None, true);
            }
            _ => {}
        }
        let gname = self.encoding_name(font, gr.code).unwrap_or(k_notdef);
        let cs = match self.dget(font, k_cs) {
            Some(Obj::Dict(d)) => d,
            _ => {
                let em1 = gr.ftype == 42;
                return self.sub_glyph(font, gr.code, Some(gname), em1);
            }
        };
        if gr.ftype == 42 {
            let gid = cs.borrow().map.get(&Key::Name(gname)).and_then(|o| o.num()).or_else(|| cs.borrow().map.get(&Key::Name(k_notdef)).and_then(|o| o.num())).unwrap_or(0.0) as u16;
            if self.dget(font, self.names_cache.sfnts).is_none() && self.dget(font, self.names_cache.glyph_directory).is_none() {
                return self.sub_glyph(font, gr.code, Some(gname), true);
            }
            return self.tt_glyph(font, gid);
        }
        let key = (Rc::as_ptr(&cs) as *const u8 as usize, self.names.get(gname).to_vec());
        if let Some(g) = self.glyph_cache.get(&key) {
            return g.clone();
        }
        let entry = cs.borrow().map.get(&Key::Name(gname)).cloned().or_else(|| cs.borrow().map.get(&Key::Name(k_notdef)).cloned());
        let res = match entry {
            Some(Obj::Str(s, _)) => {
                let (subrs, len_iv) = match self.dget(font, k_priv) {
                    Some(Obj::Dict(p)) => {
                        let subrs = match self.dget(&p, k_subrs) {
                            Some(Obj::Arr(a, _)) => a.to_vec().into_iter().map(|o| if let Obj::Str(s, _) = o { Some(s.bytes()) } else { None }).collect::<Vec<_>>(),
                            _ => Vec::new(),
                        };
                        let liv = self.dget(&p, k_leniv).and_then(|o| o.num()).unwrap_or(4.0) as i64;
                        (subrs, liv)
                    }
                    _ => (Vec::new(), 4),
                };
                let std = fonts::standard_encoding();
                let seac_lookup = |c: u8| -> Option<Vec<u8>> {
                    let n = std.get(c as usize)?;
                    let id = self.names.map.get(n.as_bytes())?;
                    match cs.borrow().map.get(&Key::Name(*id)) {
                        Some(Obj::Str(s, _)) => Some(s.bytes()),
                        _ => None,
                    }
                };
                let subr = |i: usize| subrs.get(i).cloned().flatten();
                let ctx = type1::T1Ctx { subrs: &subr, seac: &seac_lookup, len_iv };
                let o = type1::run(&s.bytes(), &ctx, true);
                Rc::new((o.path, o.width.0, o.width.1))
            }
            Some(Obj::Arr(proc_, true)) => {
                // charstring as procedure (rare) — not supported
                let _ = proc_;
                Rc::new((PathAcc::new(), 500.0, 0.0))
            }
            _ => self.sub_glyph(font, gr.code, Some(gname), false),
        };
        self.glyph_cache.insert(key, res.clone());
        res
    }

    fn cid_to_gid(&self, font: &PDict, cid: u32) -> u16 {
        let gdb = self.dget(font, self.names_cache.gdbytes).and_then(|o| o.num()).unwrap_or(2.0) as usize;
        match self.dget(font, self.names_cache.cid_map) {
            Some(Obj::Int(n)) => (cid as i64 + n) as u16,
            Some(Obj::Str(s, _)) => {
                let o = cid as usize * gdb;
                let mut v = 0u32;
                for k in 0..gdb {
                    v = v << 8 | s.get(o + k).unwrap_or(0) as u32;
                }
                v as u16
            }
            Some(Obj::Arr(a, _)) => {
                let mut o = cid as usize * gdb;
                for item in a.to_vec() {
                    if let Obj::Str(s, _) = item {
                        let l = s.len & !1;
                        if o < l {
                            let mut v = 0u32;
                            for k in 0..gdb {
                                v = v << 8 | s.get(o + k).unwrap_or(0) as u32;
                            }
                            return v as u16;
                        }
                        o -= l;
                    }
                }
                cid as u16
            }
            Some(Obj::Dict(d)) => d.borrow().map.get(&Key::Int(cid as i64)).and_then(|o| o.num()).unwrap_or(cid as f64) as u16,
            _ => cid as u16,
        }
    }

    pub fn note_font_use(&mut self, f: &PDict) {
        let name = self.font_display_name(f);
        if self.stats.fonts_used.contains_key(&name) || self.stats.fonts_used.len() > 500 {
            return;
        }
        let ft = self.font_type(f);
        let desc = if self.dget(f, self.names_cache.sub).is_some() {
            let orig = self.dget(f, self.names_cache.orig_font).and_then(|o| self.obj_to_bytes(&o)).map(|b| String::from_utf8_lossy(&b).to_string()).unwrap_or(name.clone());
            format!("Not embedded — substituted with {}", fonts::system_font(&orig).file)
        } else {
            match ft {
                0 => "Type 0 (composite)".into(),
                1 => "Type 1 (embedded)".into(),
                3 => "Type 3 (user-defined procedures)".into(),
                42 => "Type 42 (embedded TrueType)".into(),
                102 => "CIDFontType 2 (TrueType CID)".into(),
                100 => "CIDFontType 0 (CFF CID) — not supported".into(),
                t => format!("FontType {t}"),
            }
        };
        self.stats.fonts_used.insert(name, desc);
    }

    pub fn show(&mut self, s: &[u8], opts: ShowOpts) -> R<(f64, f64)> {
        let Some(font) = self.gs.font.clone() else { return err("invalidfont", "no current font") };
        let cur = match self.gs.path.cur {
            Some(c) => c,
            None if opts.measure => (0.0, 0.0),
            None => return err("nocurrentpoint", "show"),
        };
        let glyphs = self.decode_string(&font, s);
        let ctm = self.gs.ctm;
        let lin = Matrix::new(ctm.a, ctm.b, ctm.c, ctm.d, 0.0, 0.0);
        let mut pos = cur;
        let mut out = PathAcc::new();
        let mut total = (0.0, 0.0);
        let mut xyi = 0;
        let n = glyphs.len();
        for (i, gr) in glyphs.iter().enumerate() {
            let (wx, wy);
            if gr.ftype == 3 {
                let (w, h) = self.type3_glyph(gr, &lin, pos, opts.measure || opts.charpath)?;
                wx = w;
                wy = h;
            } else {
                let g = self.glyph_outline(gr);
                wx = g.1;
                wy = g.2;
                if !opts.measure {
                    let m = gr.fm.then(&lin).then(&Matrix::translate(pos.0, pos.1));
                    let dp = g.0.transformed(&m);
                    if opts.charpath {
                        self.gs.path.segs.extend_from_slice(&dp.segs);
                    } else {
                        out.segs.extend_from_slice(&dp.segs);
                    }
                }
            }
            let glyph_start = pos;
            let (mut ux, mut uy) = gr.fm.apply_delta(wx, wy);
            ux += opts.ax;
            uy += opts.ay;
            if opts.wchar == Some(gr.code) {
                ux += opts.cx;
                uy += opts.cy;
            }
            if let Some((nums, kind)) = &opts.xy {
                match kind {
                    1 => {
                        ux = nums.get(xyi).copied().unwrap_or(0.0);
                        uy = 0.0;
                        xyi += 1;
                    }
                    2 => {
                        ux = 0.0;
                        uy = nums.get(xyi).copied().unwrap_or(0.0);
                        xyi += 1;
                    }
                    _ => {
                        ux = nums.get(xyi).copied().unwrap_or(0.0);
                        uy = nums.get(xyi + 1).copied().unwrap_or(0.0);
                        xyi += 2;
                    }
                }
            }
            total.0 += ux;
            total.1 += uy;
            let (dx, dy) = lin.apply_delta(ux, uy);
            pos = (pos.0 + dx, pos.1 + dy);
            if !opts.measure && !opts.charpath && self.no_paint == 0 {
                let ch = if gr.ftype == 0 || gr.ftype >= 100 {
                    char::from_u32(gr.code).filter(|c| !c.is_control()).unwrap_or('\u{FFFD}')
                } else {
                    self.encoding_name(&gr.font, gr.code)
                        .and_then(|n| crate::fonts::glyph_name_to_unicode(&self.name_str(n)))
                        .unwrap_or_else(|| crate::fonts::cp1252(gr.code as u8))
                };
                let d2p = self.dev_to_page();
                let up = gr.fm.then(&lin).then(&d2p).apply_delta(0.0, 1.0);
                let a = d2p.apply(glyph_start.0, glyph_start.1);
                let b = d2p.apply(pos.0, pos.1);
                self.page.text_char(ch, a, b, up);
            }
            if let Some(kp) = &opts.kproc {
                if i + 1 < n {
                    if !out.is_empty() {
                        self.fill_dev(&out, false);
                        out.clear();
                    }
                    self.gs.path.move_to(pos.0, pos.1);
                    self.push(Obj::Int(gr.code as i64));
                    self.push(Obj::Int(glyphs[i + 1].code as i64));
                    self.call(kp.clone())?;
                    if let Some(c) = self.gs.path.cur {
                        pos = c;
                    }
                }
            }
        }
        if !opts.measure {
            self.page.text_break();
            if !out.is_empty() {
                let k = self.page.cur_kind;
                self.page.cur_kind = Kind::Text;
                self.fill_dev(&out, false);
                self.page.cur_kind = k;
            }
            if opts.charpath {
                self.gs.path.cur = Some(pos);
            } else {
                self.gs.path.move_to(pos.0, pos.1);
            }
        }
        Ok(total)
    }

    fn type3_glyph(&mut self, gr: &GlyphRef, lin: &Matrix, pos: (f64, f64), measure: bool) -> R<(f64, f64)> {
        let font = gr.font.clone();
        let bg = self.dget(&font, self.names_cache.build_glyph);
        let bc = self.dget(&font, self.names_cache.build_char);
        let saved = self.gs.clone();
        self.gs.ctm = gr.fm.then(lin).then(&Matrix::translate(pos.0, pos.1));
        self.gs.path.clear();
        if measure {
            self.no_paint += 1;
        }
        self.char_width = None;
        self.push(Obj::Dict(font.clone()));
        let r = if let Some(p) = bg {
            let n = self.encoding_name(&font, gr.code).unwrap_or(self.names_cache.notdef);
            self.push(Obj::Name(n, false));
            self.call(p)
        } else if let Some(p) = bc {
            self.push(Obj::Int(gr.code as i64));
            self.call(p)
        } else {
            self.os.pop();
            Ok(())
        };
        if measure {
            self.no_paint -= 1;
        }
        self.gs = saved;
        r?;
        Ok(self.char_width.take().unwrap_or((0.0, 0.0)))
    }

    /// `findfont` without a FontDirectory hit: build a substitute font.
    pub fn substitute_font(&mut self, name: &[u8]) -> PDict {
        let d = new_dict(20);
        let nc = &self.names_cache;
        let (kfn, kft, kfm, kenc, ksub, korig, kpt, kbb) = (nc.font_name, nc.font_type, nc.font_matrix, nc.encoding, nc.sub, nc.orig_font, nc.paint_type, nc.font_bbox);
        let n = self.names.intern(name);
        self.dput(&d, kfn, Obj::Name(n, false));
        self.dput(&d, korig, Obj::Name(n, false));
        self.dput(&d, kft, Obj::Int(1));
        self.dput(&d, kpt, Obj::Int(0));
        self.dput(&d, kfm, matrix_arr(&Matrix::scale(0.001, 0.001)));
        self.dput(&d, kbb, Obj::Arr(PArr::new(vec![Obj::Int(-200), Obj::Int(-250), Obj::Int(1200), Obj::Int(1000)]), false));
        let lower = String::from_utf8_lossy(name).to_ascii_lowercase();
        let enc = if lower.contains("symbol") || lower.contains("dingbat") {
            // symbolic: use codes directly (built-in encoding)
            let v: Vec<Obj> = (0..256).map(|_| Obj::Name(self.names_cache.notdef, false)).collect();
            Obj::Arr(PArr::new(v), false)
        } else {
            Obj::Arr(self.std_enc.clone(), false)
        };
        self.dput(&d, kenc, enc);
        self.dput(&d, ksub, Obj::Bool(true));
        let fid = self.names_cache.fid;
        self.next_fid += 1;
        self.dput(&d, fid, Obj::FontId(self.next_fid));
        d
    }
}
