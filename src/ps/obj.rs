//! PostScript object model.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

pub type NameId = u32;

#[derive(Clone)]
pub struct PStr {
    pub buf: Rc<RefCell<Vec<u8>>>,
    pub start: usize,
    pub len: usize,
}

impl PStr {
    pub fn new(v: Vec<u8>) -> PStr {
        let len = v.len();
        PStr { buf: Rc::new(RefCell::new(v)), start: 0, len }
    }
    pub fn bytes(&self) -> Vec<u8> {
        let b = self.buf.borrow();
        b[self.start..(self.start + self.len).min(b.len())].to_vec()
    }
    pub fn get(&self, i: usize) -> Option<u8> {
        if i >= self.len {
            return None;
        }
        self.buf.borrow().get(self.start + i).copied()
    }
    pub fn set(&self, i: usize, v: u8) -> bool {
        if i >= self.len {
            return false;
        }
        self.buf.borrow_mut()[self.start + i] = v;
        true
    }
    pub fn sub(&self, i: usize, n: usize) -> Option<PStr> {
        if i + n > self.len {
            return None;
        }
        Some(PStr { buf: self.buf.clone(), start: self.start + i, len: n })
    }
    pub fn write_at(&self, i: usize, src: &[u8]) -> bool {
        if i + src.len() > self.len {
            return false;
        }
        let mut b = self.buf.borrow_mut();
        b[self.start + i..self.start + i + src.len()].copy_from_slice(src);
        true
    }
}

#[derive(Clone)]
pub struct PArr {
    pub buf: Rc<RefCell<Vec<Obj>>>,
    pub start: usize,
    pub len: usize,
}

impl PArr {
    pub fn new(v: Vec<Obj>) -> PArr {
        let len = v.len();
        PArr { buf: Rc::new(RefCell::new(v)), start: 0, len }
    }
    pub fn get(&self, i: usize) -> Option<Obj> {
        if i >= self.len {
            return None;
        }
        self.buf.borrow().get(self.start + i).cloned()
    }
    pub fn set(&self, i: usize, v: Obj) -> bool {
        if i >= self.len {
            return false;
        }
        self.buf.borrow_mut()[self.start + i] = v;
        true
    }
    pub fn to_vec(&self) -> Vec<Obj> {
        let b = self.buf.borrow();
        b[self.start..(self.start + self.len).min(b.len())].to_vec()
    }
    pub fn sub(&self, i: usize, n: usize) -> Option<PArr> {
        if i + n > self.len {
            return None;
        }
        Some(PArr { buf: self.buf.clone(), start: self.start + i, len: n })
    }
    pub fn ptr(&self) -> usize {
        Rc::as_ptr(&self.buf) as *const u8 as usize + self.start
    }
}

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum Key {
    Name(NameId),
    Int(i64),
    Real(u64),
    Bool(bool),
    Ptr(usize),
}

pub struct DictData {
    pub map: HashMap<Key, Obj>,
    pub cap: usize,
}

pub type PDict = Rc<RefCell<DictData>>;

pub fn new_dict(cap: usize) -> PDict {
    Rc::new(RefCell::new(DictData { map: HashMap::with_capacity(cap.min(1024)), cap: cap.max(1) }))
}

#[derive(Clone)]
pub enum Obj {
    Null,
    Int(i64),
    Real(f64),
    Bool(bool),
    Mark,
    Name(NameId, bool),
    Str(PStr, bool),
    Arr(PArr, bool),
    Dict(PDict),
    Op(u16),
    File(crate::ps::file::PFile),
    Save(usize),
    FontId(usize),
    GState(Rc<crate::ps::GState>),
}

impl Obj {
    pub fn is_exec(&self) -> bool {
        matches!(self, Obj::Name(_, true) | Obj::Str(_, true) | Obj::Arr(_, true) | Obj::Op(_) | Obj::File(_))
    }
    pub fn type_name(&self) -> &'static str {
        match self {
            Obj::Null => "nulltype",
            Obj::Int(_) => "integertype",
            Obj::Real(_) => "realtype",
            Obj::Bool(_) => "booleantype",
            Obj::Mark => "marktype",
            Obj::Name(..) => "nametype",
            Obj::Str(..) => "stringtype",
            Obj::Arr(..) => "arraytype",
            Obj::Dict(_) => "dicttype",
            Obj::Op(_) => "operatortype",
            Obj::File(_) => "filetype",
            Obj::Save(_) => "savetype",
            Obj::FontId(_) => "fonttype",
            Obj::GState(_) => "gstatetype",
        }
    }
    pub fn num(&self) -> Option<f64> {
        match self {
            Obj::Int(i) => Some(*i as f64),
            Obj::Real(r) => Some(*r),
            _ => None,
        }
    }
    pub fn cvx(self) -> Obj {
        match self {
            Obj::Name(n, _) => Obj::Name(n, true),
            Obj::Str(s, _) => Obj::Str(s, true),
            Obj::Arr(a, _) => Obj::Arr(a, true),
            o => o,
        }
    }
    pub fn cvlit(self) -> Obj {
        match self {
            Obj::Name(n, _) => Obj::Name(n, false),
            Obj::Str(s, _) => Obj::Str(s, false),
            Obj::Arr(a, _) => Obj::Arr(a, false),
            o => o,
        }
    }
}
