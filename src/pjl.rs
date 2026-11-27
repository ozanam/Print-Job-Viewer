//! PJL (Printer Job Language) header parsing and job splitting.

use crate::doc::InfoSection;

pub const UEL: &[u8] = b"\x1b%-12345X";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lang {
    PclXl,
    PostScript,
    Pcl5,
    Hpgl,
    Pdf,
    Unknown,
}

impl Lang {
    pub fn name(self) -> &'static str {
        match self {
            Lang::PclXl => "PCL XL (PCL 6 Enhanced)",
            Lang::PostScript => "PostScript",
            Lang::Pcl5 => "PCL 5 (PCL 6 Standard)",
            Lang::Hpgl => "HP-GL/2",
            Lang::Pdf => "PDF",
            Lang::Unknown => "Unknown",
        }
    }
}

pub struct Segment {
    pub lang: Lang,
    pub start: usize,
    pub end: usize,
}

#[derive(Default)]
pub struct PjlInfo {
    pub commands: Vec<String>,
    pub settings: Vec<(String, String)>,
    pub job_name: Option<String>,
    pub enter_languages: Vec<String>,
}

impl PjlInfo {
    pub fn get(&self, key: &str) -> Option<&str> {
        self.settings.iter().rev().find(|(k, _)| k.eq_ignore_ascii_case(key)).map(|(_, v)| v.as_str())
    }

    pub fn to_sections(&self) -> Vec<InfoSection> {
        let mut out = Vec::new();
        if self.commands.is_empty() {
            return out;
        }
        let mut s = InfoSection::new("PJL job settings");
        if let Some(j) = &self.job_name {
            s.add("Job name", j.clone());
        }
        let friendly: &[(&str, &str)] = &[
            ("USERNAME", "User name"),
            ("JOBNAME", "Job name"),
            ("HOLD", "Hold"),
            ("HOLDTYPE", "Hold type"),
            ("HOLDKEY", "Hold PIN"),
            ("COPIES", "Copies"),
            ("QTY", "Quantity (collated copies)"),
            ("DUPLEX", "Duplex"),
            ("BINDING", "Binding"),
            ("PAPER", "Paper size"),
            ("ORIENTATION", "Orientation"),
            ("MEDIASOURCE", "Media source"),
            ("MEDIATYPE", "Media type"),
            ("RESOLUTION", "Resolution"),
            ("RENDERMODE", "Render mode"),
            ("ECONOMODE", "EconoMode"),
            ("OUTBIN", "Output bin"),
            ("STAPLE", "Staple"),
            ("PUNCH", "Punch"),
            ("COLLATE", "Collate"),
            ("DATAMODE", "Data mode"),
            ("PLANESINUSE", "Planes in use"),
            ("STRINGCODESET", "String code set"),
            ("JOBATTR", "Job attribute"),
        ];
        for (k, v) in &self.settings {
            let label = friendly.iter().find(|(fk, _)| fk.eq_ignore_ascii_case(k)).map(|(_, l)| format!("{l} ({k})")).unwrap_or_else(|| k.clone());
            s.add(label, v.clone());
        }
        if !self.enter_languages.is_empty() {
            s.add("Language(s)", self.enter_languages.join(", "));
        }
        out.push(s);
        let mut raw = InfoSection::new("PJL commands (raw)");
        for (i, c) in self.commands.iter().enumerate() {
            raw.add(format!("{}", i + 1), c.clone());
        }
        out.push(raw);
        out
    }

    fn parse_line(&mut self, line: &str) {
        let l = line.trim();
        if l.is_empty() {
            return;
        }
        self.commands.push(l.to_string());
        let rest = l.trim_start_matches("@PJL").trim();
        let upper = rest.to_ascii_uppercase();
        if upper.starts_with("JOB") && !upper.starts_with("JOBATTR") {
            if let Some(n) = extract_value(rest, "NAME") {
                self.job_name = Some(n);
            }
            for (k, v) in parse_kv(&rest[3..]) {
                self.settings.push((format!("JOB {k}"), v));
            }
        } else if upper.starts_with("SET ") || upper.starts_with("DEFAULT ") {
            let body = rest.split_once(' ').map(|x| x.1).unwrap_or("");
            for (k, v) in parse_kv(body) {
                self.settings.push((k, v));
            }
        } else if upper.starts_with("JOBATTR") {
            let body = rest.split_once(char::is_whitespace).map(|x| x.1).unwrap_or(rest[7..].trim_start_matches('=')).trim();
            let body = body.trim_start_matches('=').trim().trim_matches('"');
            if let Some((k, v)) = body.split_once('=') {
                self.settings.push((format!("JOBATTR {}", k.trim()), v.trim().to_string()));
            } else {
                self.settings.push(("JOBATTR".into(), body.to_string()));
            }
        } else if upper.starts_with("ENTER") {
            if let Some(v) = extract_value(rest, "LANGUAGE") {
                self.enter_languages.push(v);
            }
        } else if upper.starts_with("COMMENT") {
            self.settings.push(("COMMENT".into(), rest[7..].trim().to_string()));
        } else if upper.starts_with("EOJ") {
            if let Some(n) = extract_value(rest, "NAME") {
                self.settings.push(("EOJ NAME".into(), n));
            }
        }
    }
}

fn extract_value(s: &str, key: &str) -> Option<String> {
    let up = s.to_ascii_uppercase();
    let idx = up.find(&format!("{key}"))?;
    let after = s[idx + key.len()..].trim_start();
    let after = after.strip_prefix('=')?.trim_start();
    if let Some(q) = after.strip_prefix('"') {
        Some(q.split('"').next().unwrap_or("").to_string())
    } else {
        Some(after.split_whitespace().next().unwrap_or("").to_string())
    }
}

/// Parse `KEY=VALUE KEY2="quoted value"` pairs.
fn parse_kv(s: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let b = s.as_bytes();
    let mut i = 0;
    // optional LPARM:xxx prefix
    while i < b.len() {
        while i < b.len() && b[i].is_ascii_whitespace() {
            i += 1;
        }
        let ks = i;
        while i < b.len() && b[i] != b'=' && !b[i].is_ascii_whitespace() {
            i += 1;
        }
        let key = s[ks..i].to_string();
        while i < b.len() && b[i].is_ascii_whitespace() {
            i += 1;
        }
        if i >= b.len() || b[i] != b'=' {
            if !key.is_empty() && !key.contains(':') {
                out.push((key, String::new()));
            }
            continue;
        }
        i += 1;
        while i < b.len() && b[i].is_ascii_whitespace() {
            i += 1;
        }
        let val;
        if i < b.len() && b[i] == b'"' {
            i += 1;
            let vs = i;
            while i < b.len() && b[i] != b'"' {
                i += 1;
            }
            val = s[vs..i].to_string();
            i += 1;
        } else {
            let vs = i;
            while i < b.len() && !b[i].is_ascii_whitespace() {
                i += 1;
            }
            val = s[vs..i].to_string();
        }
        if !key.is_empty() {
            out.push((key, val));
        }
    }
    out
}

fn find(hay: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    if from >= hay.len() {
        return None;
    }
    hay[from..].windows(needle.len()).position(|w| w == needle).map(|p| p + from)
}

pub fn detect(data: &[u8]) -> Lang {
    let mut i = 0;
    while i < data.len() && (data[i] == 4 || data[i].is_ascii_whitespace()) {
        i += 1;
    }
    let d = &data[i..];
    if d.starts_with(b"%!") || d.starts_with(&[0xC5, 0xD0, 0xD3, 0xC6]) {
        Lang::PostScript
    } else if d.starts_with(b"%PDF") {
        Lang::Pdf
    } else if d.len() > 12 && (d[0] == b')' || d[0] == b'(' || d[0] == b'@') && d[1..].starts_with(b" HP-PCL XL") {
        Lang::PclXl
    } else if d.starts_with(b"\x1bE") || d.starts_with(b"\x1b&") || d.starts_with(b"\x1b*") || d.starts_with(b"\x1b(") {
        Lang::Pcl5
    } else if find(&d[..d.len().min(4096)], b"HP-PCL XL", 0).is_some() {
        Lang::PclXl
    } else if crate::hpgl::looks_like_hpgl(d) {
        Lang::Hpgl
    } else if d.first() == Some(&0x1B) && d.len() > 2 && (33..=47).contains(&d[1]) {
        Lang::Pcl5
    } else {
        Lang::Unknown
    }
}

fn lang_from_name(n: &str) -> Lang {
    let u = n.to_ascii_uppercase();
    if u.contains("XL") || u == "PCL6" || u == "PCLXL" {
        Lang::PclXl
    } else if u.contains("POSTSCRIPT") || u == "PS" || u == "BR-SCRIPT" {
        Lang::PostScript
    } else if u.contains("PDF") {
        Lang::Pdf
    } else if u.starts_with("PCL") {
        Lang::Pcl5
    } else if u.contains("HPGL") || u.contains("HP-GL") {
        Lang::Hpgl
    } else {
        Lang::Unknown
    }
}

/// Split a print file into PJL and language segments.
pub fn split(data: &[u8]) -> (PjlInfo, Vec<Segment>) {
    let mut info = PjlInfo::default();
    let mut segs = Vec::new();
    let mut pos = 0;
    let n = data.len();
    loop {
        // skip UELs and whitespace/control-D
        loop {
            if data[pos..].starts_with(UEL) {
                pos += UEL.len();
            } else if pos < n && (data[pos] == 4 || data[pos] == b'\r' || data[pos] == b'\n' || data[pos] == 0 || data[pos] == b' ' || data[pos] == b'\t' || data[pos] == 0x0c) {
                pos += 1;
            } else {
                break;
            }
        }
        if pos >= n {
            break;
        }
        if data[pos..].starts_with(b"@PJL") || data[pos..].starts_with(b"@pjl") {
            // PJL command line(s)
            let eol = data[pos..].iter().position(|&c| c == b'\n').map(|p| pos + p + 1).unwrap_or(n);
            let line = String::from_utf8_lossy(&data[pos..eol]).to_string();
            let up = line.to_ascii_uppercase();
            info.parse_line(&line);
            pos = eol;
            if up.contains("ENTER") && up.contains("LANGUAGE") {
                let lang_name = extract_value(&line, "LANGUAGE").unwrap_or_default();
                let mut lang = lang_from_name(&lang_name);
                let end = find(data, UEL, pos).unwrap_or(n);
                if lang == Lang::Unknown {
                    lang = detect(&data[pos..end]);
                }
                segs.push(Segment { lang, start: pos, end });
                pos = end;
            }
            continue;
        }
        // raw language data without ENTER LANGUAGE
        let end = find(data, UEL, pos + 1).unwrap_or(n);
        let lang = detect(&data[pos..end]);
        segs.push(Segment { lang, start: pos, end });
        pos = end;
    }
    (info, segs)
}
