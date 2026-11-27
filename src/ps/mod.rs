//! A PostScript Level 2 (+ common Level 3) interpreter producing page display lists.

pub mod file;
pub mod obj;
mod ops;
mod scan;
mod text;
mod type1;

use crate::doc::*;
use crate::geom::*;
use file::PFile;
use obj::*;
use std::collections::{BTreeMap, HashMap};
use std::rc::Rc;
use std::sync::Arc;
use tiny_skia::{LineCap, LineJoin, Transform};

/// Device resolution (units per point). 600 dpi device space.
pub const DEV: f64 = 600.0 / 72.0;

pub enum PsError {
    Err(&'static str, String),
    Stop,
    Exit,
    Quit,
}

pub type R<T = ()> = Result<T, PsError>;

pub fn err<T>(name: &'static str, detail: impl Into<String>) -> R<T> {
    Err(PsError::Err(name, detail.into()))
}

#[derive(Clone)]
pub enum ColorSpace {
    Gray,
    Rgb,
    Cmyk,
    Indexed { base: Rc<ColorSpace>, hival: usize, lookup: Rc<Vec<u8>> },
    Separation { alt: Rc<ColorSpace>, tint: Obj, n: usize },
    Pattern { under: Option<Rc<ColorSpace>> },
}

impl ColorSpace {
    pub fn ncomps(&self) -> usize {
        match self {
            ColorSpace::Gray => 1,
            ColorSpace::Rgb => 3,
            ColorSpace::Cmyk => 4,
            ColorSpace::Indexed { .. } => 1,
            ColorSpace::Separation { n, .. } => *n,
            ColorSpace::Pattern { .. } => 1,
        }
    }
    pub fn name(&self) -> &'static str {
        match self {
            ColorSpace::Gray => "DeviceGray",
            ColorSpace::Rgb => "DeviceRGB",
            ColorSpace::Cmyk => "DeviceCMYK",
            ColorSpace::Indexed { .. } => "Indexed",
            ColorSpace::Separation { n, .. } => {
                if *n == 1 {
                    "Separation"
                } else {
                    "DeviceN"
                }
            }
            ColorSpace::Pattern { .. } => "Pattern",
        }
    }
}

#[derive(Clone)]
pub struct GState {
    pub ctm: Matrix,
    pub color: Rgb,
    pub cspace: ColorSpace,
    pub ccomps: Vec<f64>,
    pub cspace_obj: Option<Obj>,
    pub pattern: Option<PDict>,
    pub line_width: f64,
    pub cap: LineCap,
    pub join: LineJoin,
    pub miter: f64,
    pub dash: Vec<f64>,
    pub dash_off: f64,
    pub path: PathAcc,
    pub clip: Clip,
    pub clip_path: Option<PathAcc>,
    pub font: Option<PDict>,
    pub flat: f64,
    pub stroke_adjust: bool,
    pub save_level: Option<usize>,
    pub transfer: Obj,
}

impl GState {
    fn new(ctm: Matrix) -> GState {
        GState {
            ctm,
            color: Rgb::BLACK,
            cspace: ColorSpace::Gray,
            ccomps: vec![0.0],
            cspace_obj: None,
            pattern: None,
            line_width: 1.0,
            cap: LineCap::Butt,
            join: LineJoin::Miter,
            miter: 10.0,
            dash: Vec::new(),
            dash_off: 0.0,
            path: PathAcc::new(),
            clip: None,
            clip_path: None,
            font: None,
            flat: 1.0,
            stroke_adjust: false,
            save_level: None,
            transfer: Obj::Null,
        }
    }
}

pub enum Frame {
    Proc(PArr, usize),
    File(PFile),
    Loop(PArr),
    Repeat(i64, PArr),
    ForI(i64, i64, i64, PArr),
    ForR(f64, f64, f64, PArr),
    ForallArr(PArr, usize, PArr),
    ForallStr(PStr, usize, PArr),
    ForallDict(Vec<(Key, Obj)>, usize, PArr),
    Stopped,
    /// Pops the dictionary stack back to this depth when reached.
    DictRestore(usize),
}

impl Frame {
    fn is_loop(&self) -> bool {
        matches!(self, Frame::Loop(_) | Frame::Repeat(..) | Frame::ForI(..) | Frame::ForR(..) | Frame::ForallArr(..) | Frame::ForallStr(..) | Frame::ForallDict(..))
    }
}

pub struct Names {
    pub map: HashMap<Vec<u8>, NameId>,
    pub list: Vec<Rc<[u8]>>,
}

impl Names {
    pub fn intern(&mut self, s: &[u8]) -> NameId {
        if let Some(&i) = self.map.get(s) {
            return i;
        }
        let i = self.list.len() as NameId;
        self.list.push(Rc::from(s));
        self.map.insert(s.to_vec(), i);
        i
    }
    pub fn get(&self, id: NameId) -> &[u8] {
        &self.list[id as usize]
    }
}

pub struct PageDevice {
    pub width: f64,
    pub height: f64,
    pub entries: BTreeMap<String, String>,
}

#[derive(Default)]
pub struct Stats {
    pub fonts_defined: BTreeMap<String, String>,
    pub fonts_used: BTreeMap<String, String>,
    pub images: usize,
    pub image_kinds: BTreeMap<String, usize>,
    pub setpagedevice: Vec<String>,
    pub output: String,
    pub unsupported: BTreeMap<String, usize>,
    pub copies: Option<i64>,
}

pub struct Interp<'d> {
    pub doc: &'d mut Document,
    pub os: Vec<Obj>,
    pub ds: Vec<PDict>,
    pub es: Vec<Frame>,
    pub gs: GState,
    pub gstack: Vec<GState>,
    pub names: Names,
    pub systemdict: PDict,
    pub userdict: PDict,
    pub globaldict: PDict,
    pub statusdict: PDict,
    pub font_dir: PDict,
    pub errordict: PDict,
    pub dollar_error: PDict,
    pub resources: HashMap<Vec<u8>, PDict>,
    pub ops: Vec<(&'static str, fn(&mut Interp) -> R)>,
    pub page: Page,
    pub page_dirty: bool,
    pub pagedev: PageDevice,
    pub save_count: usize,
    pub next_fid: usize,
    pub stats: Stats,
    pub char_width: Option<(f64, f64)>,
    pub no_paint: usize,
    pub sfnt_cache: HashMap<usize, Rc<crate::fonts::SfntInfo>>,
    pub glyph_cache: HashMap<(usize, Vec<u8>), Rc<(PathAcc, f64, f64)>>,
    pub std_enc: PArr,
    pub iso_enc: PArr,
    pub steps: u64,
    pub step_limit: u64,
    /// The job's top-level file (its positions are offsets in the original file).
    pub root_file: Option<PFile>,
    pub rand: u32,
    pub packing: bool,
    pub global: bool,
    pub names_cache: NameCache,
    pub clip_stack: Vec<(Clip, Option<PathAcc>)>,
}

/// Frequently used name ids.
#[derive(Default)]
pub struct NameCache {
    pub font_matrix: NameId,
    pub font_type: NameId,
    pub encoding: NameId,
    pub char_strings: NameId,
    pub private: NameId,
    pub subrs: NameId,
    pub len_iv: NameId,
    pub font_name: NameId,
    pub fid: NameId,
    pub sfnts: NameId,
    pub glyph_directory: NameId,
    pub build_char: NameId,
    pub build_glyph: NameId,
    pub fmap_type: NameId,
    pub fdep_vector: NameId,
    pub cid_font_type: NameId,
    pub cid_map: NameId,
    pub cmap: NameId,
    pub sub: NameId,
    pub paint_type: NameId,
    pub notdef: NameId,
    pub metrics: NameId,
    pub cid_count: NameId,
    pub gdbytes: NameId,
    pub font_bbox: NameId,
    pub cmap_name: NameId,
    pub wmode: NameId,
    pub pattern_type: NameId,
    pub paint_proc: NameId,
    pub implementation: NameId,
    pub xstep: NameId,
    pub ystep: NameId,
    pub bbox: NameId,
    pub orig_font: NameId,
}

impl<'d> Interp<'d> {
    pub fn new(doc: &'d mut Document) -> Self {
        let mut names = Names { map: HashMap::new(), list: Vec::new() };
        let mut nc = NameCache::default();
        macro_rules! n {
            ($f:ident, $s:expr) => {
                nc.$f = names.intern($s);
            };
        }
        n!(font_matrix, b"FontMatrix");
        n!(font_type, b"FontType");
        n!(encoding, b"Encoding");
        n!(char_strings, b"CharStrings");
        n!(private, b"Private");
        n!(subrs, b"Subrs");
        n!(len_iv, b"lenIV");
        n!(font_name, b"FontName");
        n!(fid, b"FID");
        n!(sfnts, b"sfnts");
        n!(glyph_directory, b"GlyphDirectory");
        n!(build_char, b"BuildChar");
        n!(build_glyph, b"BuildGlyph");
        n!(fmap_type, b"FMapType");
        n!(fdep_vector, b"FDepVector");
        n!(cid_font_type, b"CIDFontType");
        n!(cid_map, b"CIDMap");
        n!(cmap, b"CMap");
        n!(sub, b".SubstituteFont");
        n!(paint_type, b"PaintType");
        n!(notdef, b".notdef");
        n!(metrics, b"Metrics");
        n!(cid_count, b"CIDCount");
        n!(gdbytes, b"GDBytes");
        n!(font_bbox, b"FontBBox");
        n!(cmap_name, b"CMapName");
        n!(wmode, b"WMode");
        n!(pattern_type, b"PatternType");
        n!(paint_proc, b"PaintProc");
        n!(implementation, b".Implementation");
        n!(xstep, b"XStep");
        n!(ystep, b"YStep");
        n!(bbox, b"BBox");
        n!(orig_font, b".OrigFont");
        let systemdict = new_dict(1000);
        let userdict = new_dict(500);
        let globaldict = new_dict(100);
        let std_enc = PArr::new(Vec::new());
        let iso_enc = PArr::new(Vec::new());
        let (w, h) = (612.0, 792.0);
        let mut me = Interp {
            doc,
            os: Vec::with_capacity(512),
            ds: vec![systemdict.clone(), globaldict.clone(), userdict.clone()],
            es: Vec::new(),
            gs: GState::new(Matrix::new(DEV, 0.0, 0.0, -DEV, 0.0, h * DEV)),
            gstack: Vec::new(),
            names,
            systemdict,
            userdict,
            globaldict,
            statusdict: new_dict(100),
            font_dir: new_dict(100),
            errordict: new_dict(50),
            dollar_error: new_dict(20),
            resources: HashMap::new(),
            ops: Vec::new(),
            page: Page::new(w as f32, h as f32),
            page_dirty: false,
            pagedev: PageDevice { width: w, height: h, entries: BTreeMap::new() },
            save_count: 0,
            next_fid: 1,
            stats: Stats::default(),
            char_width: None,
            no_paint: 0,
            sfnt_cache: HashMap::new(),
            glyph_cache: HashMap::new(),
            std_enc,
            iso_enc,
            steps: 0,
            step_limit: 400_000_000,
            root_file: None,
            rand: 1,
            packing: false,
            global: false,
            names_cache: nc,
            clip_stack: Vec::new(),
        };
        ops::register(&mut me);
        me
    }

    // ----- names & dictionaries -----

    pub fn name(&mut self, s: &str) -> NameId {
        self.names.intern(s.as_bytes())
    }
    pub fn name_obj(&mut self, s: &str) -> Obj {
        Obj::Name(self.name(s), false)
    }
    pub fn name_str(&self, id: NameId) -> String {
        String::from_utf8_lossy(self.names.get(id)).to_string()
    }

    pub fn key(&mut self, o: &Obj) -> R<Key> {
        Ok(match o {
            Obj::Name(n, _) => Key::Name(*n),
            Obj::Str(s, _) => Key::Name(self.names.intern(&s.bytes())),
            Obj::Int(i) => Key::Int(*i),
            Obj::Real(r) => {
                if r.fract() == 0.0 && r.abs() < 9e15 {
                    Key::Int(*r as i64)
                } else {
                    Key::Real(r.to_bits())
                }
            }
            Obj::Bool(b) => Key::Bool(*b),
            Obj::Null => return err("typecheck", "null key"),
            Obj::Arr(a, _) => Key::Ptr(a.ptr()),
            Obj::Dict(d) => Key::Ptr(Rc::as_ptr(d) as *const u8 as usize),
            Obj::Op(i) => Key::Ptr(0x1_0000_0000 + *i as usize),
            Obj::File(f) => Key::Ptr(Rc::as_ptr(f) as *const u8 as usize),
            Obj::FontId(i) => Key::Ptr(0x2_0000_0000 + *i),
            Obj::Mark => Key::Ptr(1),
            Obj::Save(i) => Key::Ptr(0x3_0000_0000 + *i),
            Obj::GState(g) => Key::Ptr(Rc::as_ptr(g) as *const u8 as usize),
        })
    }

    pub fn lookup(&self, k: &Key) -> Option<Obj> {
        for d in self.ds.iter().rev() {
            if let Some(v) = d.borrow().map.get(k) {
                return Some(v.clone());
            }
        }
        None
    }

    pub fn lookup_name(&self, n: NameId) -> Option<Obj> {
        self.lookup(&Key::Name(n))
    }

    pub fn dget(&self, d: &PDict, n: NameId) -> Option<Obj> {
        d.borrow().map.get(&Key::Name(n)).cloned()
    }

    pub fn dput(&self, d: &PDict, n: NameId, v: Obj) {
        d.borrow_mut().map.insert(Key::Name(n), v);
    }

    pub fn def_sys(&mut self, name: &str, v: Obj) {
        let n = self.name(name);
        let sd = self.systemdict.clone();
        self.dput(&sd, n, v);
    }

    // ----- operand stack helpers -----

    pub fn push(&mut self, o: Obj) {
        self.os.push(o);
    }
    pub fn pop(&mut self) -> R<Obj> {
        match self.os.pop() {
            Some(o) => Ok(o),
            None => err("stackunderflow", ""),
        }
    }
    pub fn pop_num(&mut self) -> R<f64> {
        match self.pop()? {
            Obj::Int(i) => Ok(i as f64),
            Obj::Real(r) => Ok(r),
            o => err("typecheck", format!("expected number, got {}", o.type_name())),
        }
    }
    pub fn pop_int(&mut self) -> R<i64> {
        match self.pop()? {
            Obj::Int(i) => Ok(i),
            Obj::Real(r) if r.fract() == 0.0 => Ok(r as i64),
            o => err("typecheck", format!("expected integer, got {}", o.type_name())),
        }
    }
    pub fn pop_bool(&mut self) -> R<bool> {
        match self.pop()? {
            Obj::Bool(b) => Ok(b),
            o => err("typecheck", format!("expected boolean, got {}", o.type_name())),
        }
    }
    pub fn pop_dict(&mut self) -> R<PDict> {
        match self.pop()? {
            Obj::Dict(d) => Ok(d),
            o => err("typecheck", format!("expected dict, got {}", o.type_name())),
        }
    }
    pub fn pop_arr(&mut self) -> R<PArr> {
        match self.pop()? {
            Obj::Arr(a, _) => Ok(a),
            o => err("typecheck", format!("expected array, got {}", o.type_name())),
        }
    }
    pub fn pop_str(&mut self) -> R<PStr> {
        match self.pop()? {
            Obj::Str(s, _) => Ok(s),
            o => err("typecheck", format!("expected string, got {}", o.type_name())),
        }
    }
    pub fn pop_proc(&mut self) -> R<Obj> {
        let o = self.pop()?;
        match &o {
            Obj::Arr(..) | Obj::Op(_) | Obj::Name(..) | Obj::Str(..) | Obj::File(_) => Ok(o),
            _ => err("typecheck", format!("expected procedure, got {}", o.type_name())),
        }
    }
    pub fn top(&self) -> Option<&Obj> {
        self.os.last()
    }
    pub fn obj_to_bytes(&self, o: &Obj) -> Option<Vec<u8>> {
        match o {
            Obj::Str(s, _) => Some(s.bytes()),
            Obj::Name(n, _) => Some(self.names.get(*n).to_vec()),
            _ => None,
        }
    }
    pub fn pop_matrix(&mut self) -> R<Matrix> {
        let a = self.pop_arr()?;
        arr_matrix(&a).ok_or_else(|| PsError::Err("rangecheck", "bad matrix".into()))
    }
    pub fn top_is_matrix(&self) -> bool {
        matches!(self.os.last(), Some(Obj::Arr(a, _)) if a.len == 6 && a.to_vec().iter().all(|o| o.num().is_some()))
    }

    // ----- execution -----

    fn log_op(&mut self, name: &str) {
        if self.doc.commands.len() >= MAX_COMMAND_LINES {
            return;
        }
        let off = self.page.cur_src;
        let mut line = format!("{:08X}  {name}", if off == NO_SRC { 0 } else { off });
        if name.contains("show") {
            let s = self.os.iter().rev().take(4).find_map(|o| if let Obj::Str(s, _) = o { Some(s.bytes()) } else { None });
            if let Some(b) = s {
                let t: String = b.iter().take(80).map(|&c| if (32..127).contains(&c) { c as char } else { '·' }).collect();
                line.push_str(&format!("  ({t}{})", if b.len() > 80 { "…" } else { "" }));
            }
        } else if name == "setpagedevice" || name == "showpage" {
            line.push_str(&format!("  [page {}]", self.doc.pages.len() + 1));
        }
        let o = if off == NO_SRC { NO_SRC as usize } else { off as usize };
        self.doc.commands.push(line);
        self.doc.cmd_offsets.push(o as u32);
    }

    pub fn exec_obj(&mut self, o: Obj) -> R {
        match o {
            Obj::Name(n, true) => match self.lookup_name(n) {
                Some(v) => self.exec_value(v),
                None => err("undefined", self.name_str(n)),
            },
            Obj::Op(i) => {
                let (name, f) = self.ops[i as usize];
                if self.no_paint == 0 && is_logged_op(name) {
                    self.log_op(name);
                }
                f(self)
            }
            Obj::Arr(a, true) => {
                if a.len > 0 {
                    self.es.push(Frame::Proc(a, 0));
                }
                Ok(())
            }
            Obj::Str(s, true) => {
                self.es.push(Frame::File(file::from_vec(s.bytes())));
                Ok(())
            }
            Obj::File(f) => {
                self.es.push(Frame::File(f));
                Ok(())
            }
            o => {
                self.push(o);
                Ok(())
            }
        }
    }

    /// Execute a value obtained from a name lookup or `exec`.
    pub fn exec_value(&mut self, v: Obj) -> R {
        match v {
            Obj::Name(_, true) | Obj::Op(_) | Obj::Arr(_, true) | Obj::Str(_, true) | Obj::File(_) => self.exec_obj(v),
            o => {
                self.push(o);
                Ok(())
            }
        }
    }

    fn step(&mut self) -> R {
        self.steps += 1;
        let Some(frame) = self.es.last_mut() else { return Ok(()) };
        match frame {
            Frame::Proc(arr, idx) => {
                let i = *idx;
                if i >= arr.len {
                    self.es.pop();
                    return Ok(());
                }
                let o = arr.get(i).unwrap_or(Obj::Null);
                *idx += 1;
                if *idx >= arr.len {
                    self.es.pop();
                }
                match o {
                    Obj::Name(_, true) | Obj::Op(_) => self.exec_obj(o),
                    o => {
                        self.push(o);
                        Ok(())
                    }
                }
            }
            Frame::File(f) => {
                let f = f.clone();
                if self.root_file.as_ref().map(|r| Rc::ptr_eq(r, &f)).unwrap_or(false) {
                    let pos = f.borrow().position();
                    self.page.cur_src = pos.min(u32::MAX as usize - 1) as u32;
                }
                match self.scan_token(&f)? {
                    None => {
                        // pop this file frame (it may no longer be on top)
                        if let Some(Frame::File(g)) = self.es.last() {
                            if Rc::ptr_eq(g, &f) {
                                self.es.pop();
                            }
                        }
                        Ok(())
                    }
                    Some(o) => match o {
                        Obj::Name(_, true) | Obj::Op(_) => self.exec_obj(o),
                        o => {
                            self.push(o);
                            Ok(())
                        }
                    },
                }
            }
            Frame::Loop(p) => {
                let p = p.clone();
                self.es.push(Frame::Proc(p, 0));
                Ok(())
            }
            Frame::Repeat(n, p) => {
                if *n <= 0 {
                    self.es.pop();
                } else {
                    *n -= 1;
                    let p = p.clone();
                    self.es.push(Frame::Proc(p, 0));
                }
                Ok(())
            }
            Frame::ForI(cur, inc, lim, p) => {
                if (*inc >= 0 && *cur > *lim) || (*inc < 0 && *cur < *lim) {
                    self.es.pop();
                } else {
                    let v = *cur;
                    *cur += *inc;
                    let p = p.clone();
                    self.os.push(Obj::Int(v));
                    self.es.push(Frame::Proc(p, 0));
                }
                Ok(())
            }
            Frame::ForR(cur, inc, lim, p) => {
                if (*inc >= 0.0 && *cur > *lim + 1e-9) || (*inc < 0.0 && *cur < *lim - 1e-9) {
                    self.es.pop();
                } else {
                    let v = *cur;
                    *cur += *inc;
                    let p = p.clone();
                    self.os.push(Obj::Real(v));
                    self.es.push(Frame::Proc(p, 0));
                }
                Ok(())
            }
            Frame::ForallArr(a, i, p) => {
                if *i >= a.len {
                    self.es.pop();
                } else {
                    let v = a.get(*i).unwrap_or(Obj::Null);
                    *i += 1;
                    let p = p.clone();
                    self.os.push(v);
                    self.es.push(Frame::Proc(p, 0));
                }
                Ok(())
            }
            Frame::ForallStr(s, i, p) => {
                if *i >= s.len {
                    self.es.pop();
                } else {
                    let v = s.get(*i).unwrap_or(0);
                    *i += 1;
                    let p = p.clone();
                    self.os.push(Obj::Int(v as i64));
                    self.es.push(Frame::Proc(p, 0));
                }
                Ok(())
            }
            Frame::ForallDict(items, i, p) => {
                if *i >= items.len() {
                    self.es.pop();
                } else {
                    let (k, v) = items[*i].clone();
                    *i += 1;
                    let p = p.clone();
                    let ko = self.key_to_obj(&k);
                    self.os.push(ko);
                    self.os.push(v);
                    self.es.push(Frame::Proc(p, 0));
                }
                Ok(())
            }
            Frame::Stopped => {
                self.es.pop();
                self.os.push(Obj::Bool(false));
                Ok(())
            }
            Frame::DictRestore(n) => {
                let n = *n;
                self.es.pop();
                while self.ds.len() > n.max(3) {
                    self.ds.pop();
                }
                Ok(())
            }
        }
    }

    pub fn key_to_obj(&self, k: &Key) -> Obj {
        match k {
            Key::Name(n) => Obj::Name(*n, false),
            Key::Int(i) => Obj::Int(*i),
            Key::Real(b) => Obj::Real(f64::from_bits(*b)),
            Key::Bool(b) => Obj::Bool(*b),
            Key::Ptr(_) => Obj::Null,
        }
    }

    /// Run until the execution stack shrinks to `base`.
    pub fn run_until(&mut self, base: usize) -> R {
        while self.es.len() > base {
            if self.steps > self.step_limit {
                return err("timeout", "execution step limit reached");
            }
            if let Err(e) = self.step() {
                self.handle_error(e, base)?;
            }
        }
        Ok(())
    }

    fn handle_error(&mut self, e: PsError, base: usize) -> R {
        match e {
            PsError::Quit => Err(PsError::Quit),
            PsError::Exit => {
                // unwind to nearest loop frame
                let mut i = self.es.len();
                while i > base {
                    i -= 1;
                    if self.es[i].is_loop() {
                        self.es.truncate(i);
                        return Ok(());
                    }
                    if matches!(self.es[i], Frame::Stopped) {
                        break;
                    }
                }
                // exit outside a loop: behave like stop
                self.unwind_stopped(base, None)
            }
            PsError::Stop => self.unwind_stopped(base, None),
            PsError::Err(name, detail) => self.unwind_stopped(base, Some((name, detail))),
        }
    }

    fn unwind_stopped(&mut self, base: usize, e: Option<(&'static str, String)>) -> R {
        if let Some((name, detail)) = &e {
            let en = self.name(name);
            let de = self.dollar_error.clone();
            let k_new = self.name("newerror");
            let k_name = self.name("errorname");
            let k_cmd = self.name("command");
            self.dput(&de, k_new, Obj::Bool(true));
            self.dput(&de, k_name, Obj::Name(en, false));
            let cmd = self.name(detail);
            self.dput(&de, k_cmd, Obj::Name(cmd, false));
        }
        let mut i = self.es.len();
        while i > base {
            i -= 1;
            if matches!(self.es[i], Frame::Stopped) {
                self.es.truncate(i);
                self.os.push(Obj::Bool(true));
                return Ok(());
            }
        }
        self.es.truncate(base);
        Err(match e {
            Some((n, d)) => PsError::Err(n, d),
            None => PsError::Stop,
        })
    }

    /// Call a procedure synchronously (used by BuildChar, image data procs...).
    pub fn call(&mut self, proc_: Obj) -> R {
        let base = self.es.len();
        match proc_ {
            Obj::Arr(a, _) => {
                if a.len > 0 {
                    self.es.push(Frame::Proc(a, 0));
                }
            }
            o => self.exec_obj(o)?,
        }
        self.run_until(base)
    }

    // ----- top level -----

    pub fn run_job(&mut self, data: Rc<Vec<u8>>, start: usize) -> Option<String> {
        let f = file::from_bytes(data, start);
        self.root_file = Some(f.clone());
        self.es.push(Frame::File(f));
        let res = self.run_until(0);
        let msg = match res {
            Ok(()) | Err(PsError::Quit) => None,
            Err(PsError::Stop) | Err(PsError::Exit) => None,
            Err(PsError::Err(n, d)) => {
                let m = if d.is_empty() { format!("PostScript error: {n}") } else { format!("PostScript error: {n} in {d}") };
                Some(m)
            }
        };
        if self.page_dirty {
            self.emit_page(true);
        }
        msg
    }

    // ----- graphics output -----

    pub fn dev_to_page(&self) -> Matrix {
        Matrix::scale(1.0 / DEV, 1.0 / DEV)
    }

    pub fn emit_page(&mut self, partial: bool) {
        let mut p = std::mem::replace(&mut self.page, Page::new(self.pagedev.width as f32, self.pagedev.height as f32));
        p.label = format!("{}{}", paper_name(p.width, p.height).unwrap_or("Custom"), if partial { " (incomplete)" } else { "" });
        let copies = self.stats.copies.unwrap_or(1).max(1);
        self.doc.pages.push(p);
        let _ = copies;
        self.page_dirty = false;
    }

    pub fn current_brush(&mut self) -> Brush {
        if let ColorSpace::Pattern { .. } = &self.gs.cspace {
            if let Some(b) = self.pattern_brush() {
                return b;
            }
        }
        Brush::Solid(self.gs.color)
    }

    pub fn fill_dev(&mut self, path: &PathAcc, even_odd: bool) {
        if self.no_paint > 0 || path.is_empty() {
            return;
        }
        let m = self.dev_to_page();
        if let Some(p) = path.to_path(&m) {
            let brush = self.current_brush();
            self.page.push(Item::Fill { path: p, transform: Transform::identity(), brush, even_odd, clip: self.gs.clip.clone() });
            self.page_dirty = true;
        }
    }

    pub fn stroke_dev(&mut self, path: &PathAcc) {
        if self.no_paint > 0 || path.is_empty() {
            return;
        }
        let ctm = self.gs.ctm;
        let Some(inv) = ctm.invert() else { return };
        if let Some(p) = path.to_path(&inv) {
            let dash = if self.gs.dash.is_empty() || self.gs.dash.iter().all(|d| *d <= 0.0) {
                None
            } else {
                let mut d: Vec<f32> = self.gs.dash.iter().map(|v| v.max(0.0) as f32).collect();
                if d.len() % 2 == 1 {
                    let c = d.clone();
                    d.extend(c);
                }
                Some((d, self.gs.dash_off as f32))
            };
            let style = StrokeStyle { width: self.gs.line_width.abs() as f32, cap: self.gs.cap, join: self.gs.join, miter: self.gs.miter as f32, dash };
            let brush = self.current_brush();
            let t = ctm.then(&self.dev_to_page()).to_ts();
            self.page.push(Item::Stroke { path: p, transform: t, brush, style, clip: self.gs.clip.clone() });
            self.page_dirty = true;
        }
    }

    pub fn clip_dev(&mut self, path: &PathAcc, even_odd: bool) {
        let m = self.dev_to_page();
        let mut v: Vec<ClipPath> = self.gs.clip.as_ref().map(|c| (**c).clone()).unwrap_or_default();
        match path.to_path(&m) {
            Some(p) => v.push(ClipPath { path: p, even_odd }),
            None => {
                let r = tiny_skia::Rect::from_xywh(-1000.0, -1000.0, 0.1, 0.1).unwrap();
                v.push(ClipPath { path: tiny_skia::PathBuilder::from_rect(r), even_odd: false });
            }
        }
        self.gs.clip = Some(Arc::new(v));
        self.gs.clip_path = Some(path.clone());
    }

    pub fn page_rect_dev(&self) -> PathAcc {
        let mut p = PathAcc::new();
        let (w, h) = (self.pagedev.width * DEV, self.pagedev.height * DEV);
        p.move_to(0.0, 0.0);
        p.line_to(w, 0.0);
        p.line_to(w, h);
        p.line_to(0.0, h);
        p.close();
        p
    }

    pub fn default_matrix(&self) -> Matrix {
        Matrix::new(DEV, 0.0, 0.0, -DEV, 0.0, self.pagedev.height * DEV)
    }

    pub fn set_color_from_comps(&mut self) -> R {
        let cs = self.gs.cspace.clone();
        let comps = self.gs.ccomps.clone();
        self.gs.color = self.convert_color(&cs, &comps)?;
        Ok(())
    }

    pub fn convert_color(&mut self, cs: &ColorSpace, c: &[f64]) -> R<Rgb> {
        let g = |i: usize| c.get(i).copied().unwrap_or(0.0) as f32;
        Ok(match cs {
            ColorSpace::Gray => Rgb::gray(g(0)).clamp(),
            ColorSpace::Rgb => Rgb(g(0), g(1), g(2)).clamp(),
            ColorSpace::Cmyk => Rgb::from_cmyk(g(0), g(1), g(2), g(3)),
            ColorSpace::Indexed { base, hival, lookup } => {
                let idx = (c.first().copied().unwrap_or(0.0).round().max(0.0) as usize).min(*hival);
                let n = base.ncomps();
                let comps: Vec<f64> = (0..n).map(|k| lookup.get(idx * n + k).copied().unwrap_or(0) as f64 / 255.0).collect();
                let b = (**base).clone();
                self.convert_color(&b, &comps)?
            }
            ColorSpace::Separation { alt, tint, n } => {
                let base = self.os.len();
                for i in 0..*n {
                    self.push(Obj::Real(c.get(i).copied().unwrap_or(0.0)));
                }
                let r = self.call(tint.clone());
                let an = alt.ncomps();
                let mut out = vec![0.0; an];
                if r.is_ok() && self.os.len() >= base + an {
                    for i in (0..an).rev() {
                        out[i] = self.pop_num().unwrap_or(0.0);
                    }
                }
                self.os.truncate(base);
                let a = (**alt).clone();
                self.convert_color(&a, &out)?
            }
            ColorSpace::Pattern { .. } => Rgb::gray(0.5),
        })
    }

    pub fn initgraphics(&mut self) {
        let font = self.gs.font.clone();
        let save = self.gs.save_level;
        self.gs = GState::new(self.default_matrix());
        self.gs.font = font;
        self.gs.save_level = save;
    }

    pub fn warn(&mut self, w: impl Into<String>) {
        self.doc.warn(w);
    }

    pub fn note_unsupported(&mut self, what: &str) {
        *self.stats.unsupported.entry(what.to_string()).or_insert(0) += 1;
    }

    /// Simple tiling pattern support: render PaintProc to a raster tile.
    fn pattern_brush(&mut self) -> Option<Brush> {
        let pd = self.gs.pattern.clone()?;
        let nc = &self.names_cache;
        let (k_impl, k_pt, k_pp, k_x, k_y, k_bb, k_paint) = (nc.implementation, nc.pattern_type, nc.paint_proc, nc.xstep, nc.ystep, nc.bbox, nc.paint_type);
        let ptype = self.dget(&pd, k_pt).and_then(|o| o.num()).unwrap_or(1.0) as i64;
        if ptype != 1 {
            self.note_unsupported("shading patterns (approximated)");
            return Some(Brush::Solid(Rgb::gray(0.6)));
        }
        let matrix = match self.dget(&pd, k_impl) {
            Some(Obj::Arr(a, _)) => arr_matrix(&a)?,
            _ => self.gs.ctm,
        };
        let xstep = self.dget(&pd, k_x).and_then(|o| o.num()).unwrap_or(1.0).abs().max(1e-6);
        let ystep = self.dget(&pd, k_y).and_then(|o| o.num()).unwrap_or(1.0).abs().max(1e-6);
        let bbox = match self.dget(&pd, k_bb) {
            Some(Obj::Arr(a, _)) => a.to_vec().iter().map(|o| o.num().unwrap_or(0.0)).collect::<Vec<_>>(),
            _ => vec![0.0, 0.0, xstep, ystep],
        };
        let paint_type = self.dget(&pd, k_paint).and_then(|o| o.num()).unwrap_or(1.0) as i64;
        let proc_ = self.dget(&pd, k_pp)?;
        // tile resolution: pattern units -> device pixels (approx 150 dpi max)
        let sc = (matrix.mean_scale() / DEV * 150.0 / 72.0).clamp(0.05, 50.0);
        let tw = (xstep * sc).ceil().clamp(1.0, 1024.0) as u32;
        let th = (ystep * sc).ceil().clamp(1.0, 1024.0) as u32;
        let sx = tw as f64 / xstep;
        let sy = th as f64 / ystep;
        // Render into temp page in "tile pixel" units (page units = tile px).
        let saved_page = std::mem::replace(&mut self.page, Page::new(tw as f32, th as f32));
        let saved_dirty = self.page_dirty;
        let saved_gs = self.gs.clone();
        let saved_np = self.no_paint;
        self.no_paint = 0;
        let uncolored_color = self.gs.color;
        // pattern space -> tile pixels (y flipped), expressed in device units since items are scaled by 1/DEV
        self.gs = GState::new(Matrix::new(sx * DEV, 0.0, 0.0, -sy * DEV, -bbox[0] * sx * DEV, (bbox[1] + ystep) * sy * DEV));
        self.gs.font = saved_gs.font.clone();
        if paint_type == 2 {
            self.gs.color = uncolored_color;
        }
        let base_os = self.os.len();
        self.push(Obj::Dict(pd.clone()));
        let _ = self.call(proc_);
        self.os.truncate(base_os);
        let tile_page = std::mem::replace(&mut self.page, saved_page);
        self.page_dirty = saved_dirty;
        self.gs = saved_gs;
        self.no_paint = saved_np;
        let r = crate::render::render_page_transparent(&tile_page, 1.0)?;
        let img = Arc::new(RasterImage::new(r.width, r.height, r.rgba));
        // tile pixel -> pattern space -> device -> page
        let tm = Matrix::new(1.0 / sx, 0.0, 0.0, -1.0 / sy, bbox[0], bbox[1] + ystep).then(&matrix).then(&self.dev_to_page());
        Some(Brush::Pattern { image: img, transform: tm.to_ts() })
    }

    pub fn finish(self, error: Option<String>) {
        let mut s = InfoSection::new("PostScript interpreter");
        s.add("Pages produced", self.doc.pages.len().to_string());
        s.add("Execution steps", self.steps.to_string());
        if let Some(c) = self.stats.copies {
            s.add("#copies / NumCopies", c.to_string());
        }
        s.add("Images drawn", self.stats.images.to_string());
        if let Some(e) = &error {
            s.add("Result", e.clone());
        } else {
            s.add("Result", "Completed without errors");
        }
        self.doc.info.push(s);
        if !self.stats.setpagedevice.is_empty() {
            let mut p = InfoSection::new("Page device requests (setpagedevice)");
            for (i, x) in self.stats.setpagedevice.iter().enumerate() {
                p.add(format!("{}", i + 1), x.clone());
            }
            self.doc.info.push(p);
        }
        if !self.pagedev.entries.is_empty() {
            let mut p = InfoSection::new("Final page device parameters");
            for (k, v) in &self.pagedev.entries {
                p.add(k.clone(), v.clone());
            }
            self.doc.info.push(p);
        }
        let mut f = InfoSection::new("Fonts (PostScript)");
        for (k, v) in &self.stats.fonts_defined {
            f.add(format!("Defined: {k}"), v.clone());
        }
        for (k, v) in &self.stats.fonts_used {
            f.add(format!("Used: {k}"), v.clone());
        }
        if !f.rows.is_empty() {
            self.doc.info.push(f);
        }
        if !self.stats.image_kinds.is_empty() {
            let mut im = InfoSection::new("Raster images (PostScript)");
            for (k, v) in &self.stats.image_kinds {
                im.add(k.clone(), v.to_string());
            }
            self.doc.info.push(im);
        }
        if !self.stats.unsupported.is_empty() {
            let mut u = InfoSection::new("Unsupported / approximated features");
            for (k, v) in &self.stats.unsupported {
                u.add(k.clone(), format!("{v} time(s)"));
            }
            self.doc.info.push(u);
        }
        if !self.stats.output.trim().is_empty() {
            let mut o = InfoSection::new("Interpreter output (stdout)");
            for (i, l) in self.stats.output.lines().take(500).enumerate() {
                o.add(format!("{}", i + 1), l.to_string());
            }
            self.doc.info.push(o);
        }
        if let Some(e) = error {
            self.doc.warn(e);
        }
    }
}

pub fn arr_matrix(a: &PArr) -> Option<Matrix> {
    if a.len != 6 {
        return None;
    }
    let v: Vec<f64> = a.to_vec().iter().map(|o| o.num()).collect::<Option<Vec<_>>>()?;
    Some(Matrix::from_array(&v))
}

pub fn matrix_arr(m: &Matrix) -> Obj {
    Obj::Arr(PArr::new(m.to_array().iter().map(|v| Obj::Real(*v)).collect()), false)
}

/// Parse DSC comments into info sections.
pub fn dsc_info(data: &[u8]) -> (Vec<InfoSection>, Option<(f64, f64)>, Option<[f64; 4]>) {
    let mut header = InfoSection::new("Document structuring comments (DSC)");
    let mut features = InfoSection::new("Printer features (%%BeginFeature)");
    let mut pages = 0;
    let mut media: Option<(f64, f64)> = None;
    let mut bbox: Option<[f64; 4]> = None;
    let mut seen = std::collections::HashSet::new();
    let mut first = true;
    let mut in_binary = 0usize;
    for line in data.split(|&c| c == b'\n' || c == b'\r') {
        if in_binary > 0 {
            in_binary = in_binary.saturating_sub(line.len() + 1);
            continue;
        }
        if first {
            if line.starts_with(b"%!") {
                header.add("Header", String::from_utf8_lossy(line).trim().to_string());
            }
            first = false;
        }
        if !line.starts_with(b"%%") {
            continue;
        }
        let l = String::from_utf8_lossy(&line[2..]).to_string();
        let (k, v) = match l.split_once(':') {
            Some((k, v)) => (k.trim().to_string(), v.trim().to_string()),
            None => (l.trim().to_string(), String::new()),
        };
        match k.as_str() {
            "Page" => {
                pages += 1;
                continue;
            }
            "BeginFeature" | "IncludeFeature" => {
                features.add(if k == "BeginFeature" { "Feature" } else { "Included feature" }, v.clone());
                continue;
            }
            "BeginBinary" | "BeginData" => {
                if let Some(n) = v.split_whitespace().next().and_then(|n| n.parse::<usize>().ok()) {
                    if k == "BeginBinary" {
                        in_binary = n;
                    }
                }
            }
            "EndFeature" | "EndComments" | "EndProlog" | "EndSetup" | "BeginSetup" | "BeginProlog" | "Trailer" | "EOF" | "EndResource" | "BeginPageSetup" | "EndPageSetup" | "PageTrailer" | "EndBinary" | "EndData" | "BeginDefaults" | "EndDefaults" => continue,
            "BeginResource" | "PageBoundingBox" | "PageMedia" | "PageOrientation" | "PageResources" | "PageFonts" | "IncludeResource" | "IncludeFont" | "BeginFont" | "EndFont" | "BeginProcSet" | "EndProcSet" | "+" => {
                if k == "BeginResource" || k == "IncludeResource" {
                    let key = format!("{k}: {v}");
                    if seen.insert(key) && features.rows.len() < 2000 {
                        header.add(k.clone(), v.clone());
                    }
                }
                continue;
            }
            _ => {}
        }
        if k == "BoundingBox" || k == "HiResBoundingBox" {
            let n: Vec<f64> = v.split_whitespace().filter_map(|x| x.parse().ok()).collect();
            if n.len() == 4 && (k == "BoundingBox" || bbox.is_none()) {
                bbox = Some([n[0], n[1], n[2], n[3]]);
            }
        }
        if k == "DocumentMedia" && media.is_none() {
            let parts: Vec<&str> = v.split_whitespace().collect();
            if parts.len() >= 3 {
                if let (Ok(w), Ok(h)) = (parts[1].parse::<f64>(), parts[2].parse::<f64>()) {
                    media = Some((w, h));
                }
            }
        }
        if k == "DocumentPaperSizes" && media.is_none() {
            let p = v.to_ascii_lowercase();
            if p.contains("a4") {
                media = Some((595.0, 842.0));
            } else if p.contains("letter") {
                media = Some((612.0, 792.0));
            }
        }
        let key = format!("{k}={v}");
        if seen.insert(key) && v != "(atend)" {
            let label = match k.as_str() {
                "Title" => "Title",
                "Creator" => "Creator (application)",
                "CreationDate" => "Creation date",
                "For" => "For (user)",
                "Pages" => "Pages (declared)",
                "BoundingBox" => "Bounding box",
                "DocumentMedia" => "Document media",
                "LanguageLevel" => "Language level",
                "Orientation" => "Orientation",
                "PageOrder" => "Page order",
                "Routing" => "Routing",
                "DocumentNeededResources" => "Needed resources",
                "DocumentSuppliedResources" => "Supplied resources",
                "DocumentData" => "Document data",
                "Requirements" => "Requirements",
                "Copyright" => "Copyright",
                _ => "",
            };
            if label.is_empty() {
                header.add(format!("%%{k}"), v);
            } else {
                header.add(label, v);
            }
        }
    }
    header.add("Pages (counted %%Page comments)", pages.to_string());
    let mut out = vec![header];
    if !features.rows.is_empty() {
        out.push(features);
    }
    (out, media, bbox)
}

fn is_logged_op(n: &str) -> bool {
    matches!(
        n,
        "fill" | "eofill" | "stroke" | "rectfill" | "rectstroke" | "show" | "ashow" | "widthshow" | "awidthshow" | "xshow" | "yshow" | "xyshow" | "kshow" | "glyphshow" | "cshow"
            | "image" | "imagemask" | "colorimage" | "showpage" | "copypage" | "setpagedevice" | "clip" | "eoclip" | "rectclip" | "shfill" | "definefont" | "setfont" | "selectfont"
    )
}
