//! A small, forgiving XML parser (enough for XPS markup).

#[derive(Debug, Clone, Default)]
pub struct Element {
    pub name: String,
    pub attrs: Vec<(String, String)>,
    pub children: Vec<Element>,
    pub text: String,
}

impl Element {
    /// Local name without namespace prefix.
    pub fn local(&self) -> &str {
        self.name.rsplit(':').next().unwrap_or(&self.name)
    }
    pub fn attr(&self, name: &str) -> Option<&str> {
        self.attrs.iter().find(|(k, _)| k == name || k.rsplit(':').next() == Some(name) && !k.starts_with("xmlns")).map(|(_, v)| v.as_str())
    }
    pub fn child(&self, local: &str) -> Option<&Element> {
        self.children.iter().find(|c| c.local() == local)
    }
    pub fn children_named<'a>(&'a self, local: &'a str) -> impl Iterator<Item = &'a Element> + 'a {
        self.children.iter().filter(move |c| c.local() == local)
    }
    /// All descendants with the given local name (depth-first).
    pub fn find_all<'a>(&'a self, local: &str, out: &mut Vec<&'a Element>) {
        for c in &self.children {
            if c.local() == local {
                out.push(c);
            }
            c.find_all(local, out);
        }
    }
}

fn unescape(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        let r = &rest[i..];
        if let Some(j) = r.find(';') {
            let ent = &r[1..j];
            let rep = match ent {
                "amp" => Some('&'),
                "lt" => Some('<'),
                "gt" => Some('>'),
                "quot" => Some('"'),
                "apos" => Some('\''),
                e if e.starts_with("#x") || e.starts_with("#X") => u32::from_str_radix(&e[2..], 16).ok().and_then(char::from_u32),
                e if e.starts_with('#') => e[1..].parse::<u32>().ok().and_then(char::from_u32),
                _ => None,
            };
            if let Some(c) = rep {
                out.push(c);
                rest = &r[j + 1..];
                continue;
            }
        }
        out.push('&');
        rest = &r[1..];
    }
    out.push_str(rest);
    out
}

pub fn parse(data: &[u8]) -> Option<Element> {
    // decode UTF-8 or UTF-16
    let text: String = if data.starts_with(&[0xFF, 0xFE]) || data.len() > 1 && data[1] == 0 && data[0] == b'<' {
        let start = if data.starts_with(&[0xFF, 0xFE]) { 2 } else { 0 };
        let v: Vec<u16> = data[start..].chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
        String::from_utf16_lossy(&v)
    } else if data.starts_with(&[0xFE, 0xFF]) {
        let v: Vec<u16> = data[2..].chunks_exact(2).map(|c| u16::from_be_bytes([c[0], c[1]])).collect();
        String::from_utf16_lossy(&v)
    } else {
        let d = data.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(data);
        String::from_utf8_lossy(d).to_string()
    };
    let s = text.as_str();
    let b = s.as_bytes();
    let mut stack: Vec<Element> = vec![Element { name: "#root".into(), ..Default::default() }];
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'<' {
            if s[i..].starts_with("<!--") {
                i = s[i..].find("-->").map(|j| i + j + 3).unwrap_or(b.len());
                continue;
            }
            if s[i..].starts_with("<![CDATA[") {
                let e = s[i..].find("]]>").map(|j| i + j).unwrap_or(b.len());
                if let Some(top) = stack.last_mut() {
                    top.text.push_str(&s[i + 9..e]);
                }
                i = e + 3;
                continue;
            }
            if s[i..].starts_with("<?") || s[i..].starts_with("<!") {
                i = s[i..].find('>').map(|j| i + j + 1).unwrap_or(b.len());
                continue;
            }
            if s[i..].starts_with("</") {
                let e = s[i..].find('>').map(|j| i + j).unwrap_or(b.len());
                if stack.len() > 1 {
                    let el = stack.pop().unwrap();
                    stack.last_mut().unwrap().children.push(el);
                }
                i = e + 1;
                continue;
            }
            // start tag
            let mut j = i + 1;
            while j < b.len() && !b[j].is_ascii_whitespace() && b[j] != b'>' && b[j] != b'/' {
                j += 1;
            }
            let mut el = Element { name: s[i + 1..j].to_string(), ..Default::default() };
            let mut self_close = false;
            loop {
                while j < b.len() && b[j].is_ascii_whitespace() {
                    j += 1;
                }
                if j >= b.len() {
                    break;
                }
                if b[j] == b'>' {
                    j += 1;
                    break;
                }
                if b[j] == b'/' {
                    self_close = true;
                    j += 1;
                    continue;
                }
                let ks = j;
                while j < b.len() && b[j] != b'=' && !b[j].is_ascii_whitespace() && b[j] != b'>' && b[j] != b'/' {
                    j += 1;
                }
                let key = s[ks..j].to_string();
                while j < b.len() && b[j].is_ascii_whitespace() {
                    j += 1;
                }
                if j < b.len() && b[j] == b'=' {
                    j += 1;
                    while j < b.len() && b[j].is_ascii_whitespace() {
                        j += 1;
                    }
                    if j < b.len() && (b[j] == b'"' || b[j] == b'\'') {
                        let q = b[j];
                        let vs = j + 1;
                        let ve = s[vs..].find(q as char).map(|x| vs + x).unwrap_or(b.len());
                        el.attrs.push((key, unescape(&s[vs..ve])));
                        j = ve + 1;
                    } else {
                        let vs = j;
                        while j < b.len() && !b[j].is_ascii_whitespace() && b[j] != b'>' {
                            j += 1;
                        }
                        el.attrs.push((key, unescape(&s[vs..j])));
                    }
                } else if !key.is_empty() {
                    el.attrs.push((key, String::new()));
                }
            }
            i = j;
            if self_close {
                stack.last_mut().unwrap().children.push(el);
            } else {
                stack.push(el);
            }
        } else {
            let e = s[i..].find('<').map(|j| i + j).unwrap_or(b.len());
            let t = &s[i..e];
            if !t.trim().is_empty() {
                if let Some(top) = stack.last_mut() {
                    top.text.push_str(&unescape(t));
                }
            }
            i = e;
        }
    }
    while stack.len() > 1 {
        let el = stack.pop().unwrap();
        stack.last_mut().unwrap().children.push(el);
    }
    let root = stack.pop()?;
    root.children.into_iter().next()
}
