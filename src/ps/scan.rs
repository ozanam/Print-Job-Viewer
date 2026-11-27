//! PostScript tokenizer.

use super::file::PFile;
use super::obj::*;
use super::{err, Interp, R};

fn is_ws(c: u8) -> bool {
    matches!(c, b' ' | b'\t' | b'\r' | b'\n' | 0x0C | 0x00)
}
fn is_delim(c: u8) -> bool {
    matches!(c, b'(' | b')' | b'<' | b'>' | b'[' | b']' | b'{' | b'}' | b'/' | b'%')
}

pub fn parse_number(s: &[u8]) -> Option<Obj> {
    if s.is_empty() {
        return None;
    }
    let st = std::str::from_utf8(s).ok()?;
    // radix number
    if let Some((base, digits)) = st.split_once('#') {
        let b: u32 = base.parse().ok()?;
        if (2..=36).contains(&b) && !digits.is_empty() {
            return u64::from_str_radix(digits, b).ok().map(|v| Obj::Int(v as u32 as i32 as i64));
        }
        return None;
    }
    let c0 = s[0];
    if !(c0.is_ascii_digit() || c0 == b'+' || c0 == b'-' || c0 == b'.') {
        return None;
    }
    if s.iter().all(|c| c.is_ascii_digit()) || ((c0 == b'-' || c0 == b'+') && s.len() > 1 && s[1..].iter().all(|c| c.is_ascii_digit())) {
        return match st.parse::<i64>() {
            Ok(v) if v.abs() <= i32::MAX as i64 => Some(Obj::Int(v)),
            _ => st.parse::<f64>().ok().map(Obj::Real),
        };
    }
    // real: must contain digit
    if !s.iter().any(|c| c.is_ascii_digit()) {
        return None;
    }
    if s.iter().all(|c| c.is_ascii_digit() || matches!(c, b'+' | b'-' | b'.' | b'e' | b'E')) {
        let st2 = if st.ends_with('.') { format!("{st}0") } else { st.to_string() };
        return st2.parse::<f64>().ok().map(Obj::Real);
    }
    None
}

impl Interp<'_> {
    /// Read the next object from `f`. Procedures are returned as executable arrays.
    pub fn scan_token(&mut self, f: &PFile) -> R<Option<Obj>> {
        let mut stack: Vec<Vec<Obj>> = Vec::new();
        loop {
            let tok = self.scan_one(f)?;
            let Some(tok) = tok else {
                if !stack.is_empty() {
                    return err("syntaxerror", "unterminated procedure");
                }
                return Ok(None);
            };
            match tok {
                Tok::ProcStart => stack.push(Vec::new()),
                Tok::ProcEnd => {
                    let Some(items) = stack.pop() else { return err("syntaxerror", "unmatched }") };
                    let arr = Obj::Arr(PArr::new(items), true);
                    match stack.last_mut() {
                        Some(parent) => parent.push(arr),
                        None => return Ok(Some(arr)),
                    }
                }
                Tok::Obj(o) => match stack.last_mut() {
                    Some(parent) => parent.push(o),
                    None => return Ok(Some(o)),
                },
                Tok::Immediate(n) => {
                    let v = match self.lookup_name(n) {
                        Some(v) => v,
                        None => return err("undefined", self.name_str(n)),
                    };
                    match stack.last_mut() {
                        Some(parent) => parent.push(v),
                        None => return Ok(Some(v)),
                    }
                }
            }
        }
    }

    fn scan_one(&mut self, f: &PFile) -> R<Option<Tok>> {
        let mut fs = f.borrow_mut();
        // skip whitespace & comments
        let c = loop {
            let Some(c) = fs.read() else { return Ok(None) };
            if is_ws(c) {
                continue;
            }
            if c == b'%' {
                while let Some(c) = fs.read() {
                    if c == b'\n' || c == b'\r' {
                        break;
                    }
                }
                continue;
            }
            break c;
        };
        match c {
            b'{' => Ok(Some(Tok::ProcStart)),
            b'}' => Ok(Some(Tok::ProcEnd)),
            b'[' | b']' => {
                let n = self.names.intern(&[c]);
                Ok(Some(Tok::Obj(Obj::Name(n, true))))
            }
            b'(' => {
                let mut s = Vec::new();
                let mut depth = 1;
                loop {
                    let Some(c) = fs.read() else { break };
                    match c {
                        b'(' => {
                            depth += 1;
                            s.push(c);
                        }
                        b')' => {
                            depth -= 1;
                            if depth == 0 {
                                break;
                            }
                            s.push(c);
                        }
                        b'\\' => {
                            let Some(e) = fs.read() else { break };
                            match e {
                                b'n' => s.push(b'\n'),
                                b'r' => s.push(b'\r'),
                                b't' => s.push(b'\t'),
                                b'b' => s.push(8),
                                b'f' => s.push(12),
                                b'\r' => {
                                    if fs.peek() == Some(b'\n') {
                                        fs.read();
                                    }
                                }
                                b'\n' => {}
                                b'0'..=b'7' => {
                                    let mut v = (e - b'0') as u32;
                                    for _ in 0..2 {
                                        match fs.peek() {
                                            Some(d @ b'0'..=b'7') => {
                                                fs.read();
                                                v = v * 8 + (d - b'0') as u32;
                                            }
                                            _ => break,
                                        }
                                    }
                                    s.push(v as u8);
                                }
                                other => s.push(other),
                            }
                        }
                        b'\r' => {
                            // EOL normalisation
                            if fs.peek() == Some(b'\n') {
                                fs.read();
                            }
                            s.push(b'\n');
                        }
                        _ => s.push(c),
                    }
                }
                Ok(Some(Tok::Obj(Obj::Str(PStr::new(s), false))))
            }
            b'<' => {
                match fs.peek() {
                    Some(b'<') => {
                        fs.read();
                        let n = self.names.intern(b"<<");
                        return Ok(Some(Tok::Obj(Obj::Name(n, true))));
                    }
                    Some(b'~') => {
                        fs.read();
                        drop(fs);
                        let v = super::file::ascii85(f);
                        return Ok(Some(Tok::Obj(Obj::Str(PStr::new(v), false))));
                    }
                    _ => {}
                }
                let mut s = Vec::new();
                let mut hi: Option<u8> = None;
                while let Some(c) = fs.read() {
                    if c == b'>' {
                        break;
                    }
                    let v = match c {
                        b'0'..=b'9' => c - b'0',
                        b'a'..=b'f' => c - b'a' + 10,
                        b'A'..=b'F' => c - b'A' + 10,
                        _ => continue,
                    };
                    match hi {
                        None => hi = Some(v),
                        Some(h) => {
                            s.push(h << 4 | v);
                            hi = None;
                        }
                    }
                }
                if let Some(h) = hi {
                    s.push(h << 4);
                }
                Ok(Some(Tok::Obj(Obj::Str(PStr::new(s), false))))
            }
            b'>' => {
                if fs.peek() == Some(b'>') {
                    fs.read();
                }
                let n = self.names.intern(b">>");
                Ok(Some(Tok::Obj(Obj::Name(n, true))))
            }
            b')' => err("syntaxerror", "unmatched )"),
            b'/' => {
                let immediate = if fs.peek() == Some(b'/') {
                    fs.read();
                    true
                } else {
                    false
                };
                let mut s = Vec::new();
                while let Some(c) = fs.read() {
                    if is_ws(c) {
                        if c == b'\r' && fs.peek() == Some(b'\n') {
                            fs.read();
                        }
                        break;
                    }
                    if is_delim(c) {
                        fs.unread(c);
                        break;
                    }
                    s.push(c);
                }
                let n = self.names.intern(&s);
                if immediate {
                    Ok(Some(Tok::Immediate(n)))
                } else {
                    Ok(Some(Tok::Obj(Obj::Name(n, false))))
                }
            }
            _ => {
                let mut s = vec![c];
                while let Some(c) = fs.read() {
                    if is_ws(c) {
                        if c == b'\r' && fs.peek() == Some(b'\n') {
                            fs.read();
                        }
                        break;
                    }
                    if is_delim(c) {
                        fs.unread(c);
                        break;
                    }
                    s.push(c);
                }
                if let Some(n) = parse_number(&s) {
                    return Ok(Some(Tok::Obj(n)));
                }
                let n = self.names.intern(&s);
                Ok(Some(Tok::Obj(Obj::Name(n, true))))
            }
        }
    }
}

enum Tok {
    ProcStart,
    ProcEnd,
    Obj(Obj),
    Immediate(NameId),
}
