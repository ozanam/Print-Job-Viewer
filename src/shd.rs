//! Windows spooler shadow files (.SHD): the job record the print spooler keeps
//! next to each spool file (.SPL) — owner, document, printer, driver, port,
//! submit time and the driver settings (DEVMODE).

use crate::doc::InfoSection;

#[derive(Clone, Default)]
pub struct DevMode {
    pub device: String,
    pub spec_version: u16,
    pub driver_version: u16,
    pub fields: u32,
    pub orientation: i16,
    pub paper_size: i16,
    pub paper_length: i16,
    pub paper_width: i16,
    pub scale: i16,
    pub copies: i16,
    pub default_source: i16,
    pub print_quality: i16,
    pub color: i16,
    pub duplex: i16,
    pub y_resolution: i16,
    pub tt_option: i16,
    pub collate: i16,
    pub form_name: String,
    pub nup: u32,
    pub media_type: u32,
    pub dither_type: u32,
    pub icm_method: u32,
    pub icm_intent: u32,
    pub driver_extra: u16,
    pub driver_extra_sig: String,
}

#[derive(Clone, Default)]
pub struct Shadow {
    pub file: String,
    pub signature: u32,
    pub job_id: u32,
    pub priority: u32,
    pub status: u16,
    pub user: String,
    pub notify: String,
    pub document: String,
    pub port: String,
    pub printer: String,
    pub driver: String,
    pub print_processor: String,
    pub datatype: String,
    pub parameters: String,
    pub machine: String,
    pub user_sid: String,
    pub submitted: String,
    pub start_time: u32,
    pub until_time: u32,
    pub total_pages: u32,
    pub spool_size: u32,
    pub properties: Vec<(String, String)>,
    pub devmode: Option<DevMode>,
}

fn u16_at(d: &[u8], o: usize) -> Option<u16> {
    Some(u16::from_le_bytes([*d.get(o)?, *d.get(o + 1)?]))
}
fn u32_at(d: &[u8], o: usize) -> Option<u32> {
    Some(u32::from_le_bytes([*d.get(o)?, *d.get(o + 1)?, *d.get(o + 2)?, *d.get(o + 3)?]))
}
fn u64_at(d: &[u8], o: usize) -> Option<u64> {
    let lo = u32_at(d, o)? as u64;
    let hi = u32_at(d, o + 4)? as u64;
    Some(lo | (hi << 32))
}

/// Read a NUL-terminated UTF-16LE string.
fn wstr(d: &[u8], off: usize) -> String {
    if off == 0 || off >= d.len() {
        return String::new();
    }
    let mut v = Vec::new();
    let mut o = off;
    while o + 1 < d.len() && v.len() < 4096 {
        let c = u16::from_le_bytes([d[o], d[o + 1]]);
        if c == 0 {
            break;
        }
        v.push(c);
        o += 2;
    }
    String::from_utf16_lossy(&v)
}

fn wfixed(d: &[u8], off: usize, chars: usize) -> String {
    let mut v = Vec::new();
    for i in 0..chars {
        let Some(c) = u16_at(d, off + i * 2) else { break };
        if c == 0 {
            break;
        }
        v.push(c);
    }
    String::from_utf16_lossy(&v)
}

fn valid_str_off(d: &[u8], o: u64) -> bool {
    o == 0 || (o as usize) < d.len() && o % 2 == 0
}

pub fn parse(d: &[u8], file: &str) -> Option<Shadow> {
    let sig = u32_at(d, 0)?;
    if !matches!(sig, 0x4966 | 0x4967 | 0x4968 | 0x5123) {
        return None;
    }
    let hdr = u32_at(d, 4)? as usize;
    let mut s = Shadow { file: file.to_string(), signature: sig, ..Default::default() };
    s.status = u16_at(d, 8)?;
    s.job_id = u32_at(d, 12)?;
    s.priority = u32_at(d, 16)?;
    // 64-bit spoolers store offsets as 8-byte values.
    let wide = hdr >= 0xD0 && (24..=96).step_by(8).all(|o| u64_at(d, o).map(|v| valid_str_off(d, v)).unwrap_or(false));
    let devmode_off;
    if wide {
        let off = |o: usize| u64_at(d, o).unwrap_or(0) as usize;
        s.user = wstr(d, off(24));
        s.notify = wstr(d, off(32));
        s.document = wstr(d, off(40));
        s.port = wstr(d, off(48));
        s.printer = wstr(d, off(56));
        s.driver = wstr(d, off(64));
        devmode_off = off(72);
        s.print_processor = wstr(d, off(80));
        s.datatype = wstr(d, off(88));
        s.parameters = wstr(d, off(96));
        s.submitted = systemtime(d, 104);
        s.start_time = u32_at(d, 120).unwrap_or(0);
        s.until_time = u32_at(d, 124).unwrap_or(0);
        s.spool_size = u32_at(d, 128).unwrap_or(0);
        s.total_pages = u32_at(d, 132).unwrap_or(0);
        s.machine = wstr(d, off(168));
        s.user_sid = wstr(d, off(184));
        let psf = off(216);
        if psf > 0 && psf < d.len() {
            s.properties = property_bag(d, psf, if devmode_off > psf { devmode_off } else { (psf + 4096).min(d.len()) });
        }
    } else {
        let off = |o: usize| u32_at(d, o).unwrap_or(0) as usize;
        s.user = wstr(d, off(20));
        s.notify = wstr(d, off(24));
        s.document = wstr(d, off(28));
        s.port = wstr(d, off(32));
        s.printer = wstr(d, off(36));
        s.driver = wstr(d, off(40));
        devmode_off = off(44);
        s.print_processor = wstr(d, off(48));
        s.datatype = wstr(d, off(52));
        s.parameters = wstr(d, off(56));
        s.submitted = systemtime(d, 60);
        s.start_time = u32_at(d, 76).unwrap_or(0);
        s.until_time = u32_at(d, 80).unwrap_or(0);
        s.spool_size = u32_at(d, 84).unwrap_or(0);
        s.total_pages = u32_at(d, 88).unwrap_or(0);
        if hdr >= 0x74 {
            s.machine = wstr(d, off(112));
        }
    }
    if devmode_off > 0 && devmode_off + 0x60 < d.len() {
        s.devmode = devmode(d, devmode_off);
    }
    Some(s)
}

fn systemtime(d: &[u8], o: usize) -> String {
    let g = |i: usize| u16_at(d, o + i * 2).unwrap_or(0);
    let (y, mo, day, h, mi, sec) = (g(0), g(1), g(3), g(4), g(5), g(6));
    if y < 1990 || mo == 0 || mo > 12 {
        return String::new();
    }
    format!("{y:04}-{mo:02}-{day:02} {h:02}:{mi:02}:{sec:02} UTC")
}

/// The job's print-ticket property bag ("Printer Serialization Format"):
/// UTF-16 strings in name/value order.
fn property_bag(d: &[u8], start: usize, end: usize) -> Vec<(String, String)> {
    let end = end.min(d.len());
    let mut strs = Vec::new();
    let mut o = start & !1;
    while o + 8 < end {
        // a run of at least 3 printable UTF-16 chars
        let mut v = Vec::new();
        let mut p = o;
        while p + 1 < end {
            let c = u16::from_le_bytes([d[p], d[p + 1]]);
            if (0x20..0x7F).contains(&c) || (0xA0..0x250).contains(&c) {
                v.push(c);
                p += 2;
            } else {
                break;
            }
        }
        if v.len() >= 3 && p + 1 < end && d[p] == 0 && d[p + 1] == 0 {
            strs.push(String::from_utf16_lossy(&v));
            o = p + 2;
        } else {
            o = p + 2;
        }
    }
    let mut out = Vec::new();
    let mut i = 0;
    while i < strs.len() {
        let k = strs[i].clone();
        let v = strs.get(i + 1).cloned().unwrap_or_default();
        out.push((k, v));
        i += 2;
    }
    out
}

pub fn devmode(d: &[u8], o: usize) -> Option<DevMode> {
    let g16 = |x: usize| u16_at(d, o + x).unwrap_or(0) as i16;
    let g32 = |x: usize| u32_at(d, o + x).unwrap_or(0);
    let size = u16_at(d, o + 68)? as usize;
    if !(0x9C..=0x400).contains(&size) {
        return None;
    }
    let mut m = DevMode {
        device: wfixed(d, o, 32),
        spec_version: g16(64) as u16,
        driver_version: g16(66) as u16,
        driver_extra: g16(70) as u16,
        fields: g32(72),
        orientation: g16(76),
        paper_size: g16(78),
        paper_length: g16(80),
        paper_width: g16(82),
        scale: g16(84),
        copies: g16(86),
        default_source: g16(88),
        print_quality: g16(90),
        color: g16(92),
        duplex: g16(94),
        y_resolution: g16(96),
        tt_option: g16(98),
        collate: g16(100),
        form_name: wfixed(d, o + 102, 32),
        ..Default::default()
    };
    if size >= 220 {
        m.nup = g32(180);
        m.icm_method = g32(188);
        m.icm_intent = g32(192);
        m.media_type = g32(196);
        m.dither_type = g32(200);
    }
    let ex = o + size;
    if m.driver_extra >= 8 {
        let sig: String = d.get(ex + 4..ex + 8).map(|b| b.iter().map(|&c| c as char).collect()).unwrap_or_default();
        if sig.chars().all(|c| c.is_ascii_alphanumeric()) {
            m.driver_extra_sig = sig;
        }
    }
    Some(m)
}

pub fn paper_size_name(n: i16) -> String {
    match n {
        1 => "Letter".into(),
        2 => "Letter Small".into(),
        3 => "Tabloid".into(),
        4 => "Ledger".into(),
        5 => "Legal".into(),
        6 => "Statement".into(),
        7 => "Executive".into(),
        8 => "A3".into(),
        9 => "A4".into(),
        10 => "A4 Small".into(),
        11 => "A5".into(),
        12 => "B4 (JIS)".into(),
        13 => "B5 (JIS)".into(),
        14 => "Folio".into(),
        15 => "Quarto".into(),
        16 => "10×14 in".into(),
        17 => "11×17 in".into(),
        18 => "Note".into(),
        19 => "Envelope #9".into(),
        20 => "Envelope #10".into(),
        21 => "Envelope #11".into(),
        27 => "Envelope DL".into(),
        28 => "Envelope C5".into(),
        29 => "Envelope C3".into(),
        30 => "Envelope C4".into(),
        31 => "Envelope C6".into(),
        34 => "Envelope B5".into(),
        37 => "Envelope Monarch".into(),
        39 => "US Std Fanfold".into(),
        41 => "German Legal Fanfold".into(),
        66 => "A2".into(),
        70 => "A6".into(),
        256 => "Custom".into(),
        n if n > 256 => format!("Driver-defined ({n})"),
        n => format!("code {n}"),
    }
}

pub fn source_name(n: i16) -> String {
    match n {
        1 => "Upper / only one".into(),
        2 => "Lower".into(),
        3 => "Middle".into(),
        4 => "Manual feed".into(),
        5 => "Envelope".into(),
        6 => "Envelope manual".into(),
        7 => "Automatically select".into(),
        8 => "Tractor".into(),
        9 => "Small format".into(),
        10 => "Large format".into(),
        11 => "Large capacity".into(),
        14 => "Cassette".into(),
        15 => "Form source (paper size picks the tray)".into(),
        n if n >= 256 => format!("Driver-defined tray ({n})"),
        n => format!("code {n}"),
    }
}

impl DevMode {
    pub fn has(&self, bit: u32) -> bool {
        self.fields & bit != 0
    }

    pub fn duplex_name(&self) -> &'static str {
        match self.duplex {
            1 => "Simplex (one-sided)",
            2 => "Duplex, long edge (book)",
            3 => "Duplex, short edge (flip)",
            _ => "Not set",
        }
    }

    pub fn is_color(&self) -> Option<bool> {
        if self.has(0x800) {
            Some(self.color == 2)
        } else {
            None
        }
    }

    pub fn section(&self) -> InfoSection {
        let mut s = InfoSection::new("Driver settings (DEVMODE from the .SHD file)");
        s.add("Device name", self.device.clone());
        s.add("DEVMODE version", format!("spec {:#06X}, driver {:#06X}", self.spec_version, self.driver_version));
        if self.has(0x1) {
            s.add("Orientation", if self.orientation == 2 { "Landscape" } else { "Portrait" });
        }
        if self.has(0x2) {
            s.add("Paper size", paper_size_name(self.paper_size));
        }
        if self.paper_length > 0 && self.paper_width > 0 {
            s.add("Paper dimensions", format!("{:.1} × {:.1} mm", self.paper_width as f32 / 10.0, self.paper_length as f32 / 10.0));
        }
        if self.has(0x10000) && !self.form_name.is_empty() {
            s.add("Form name", self.form_name.clone());
        }
        if self.has(0x10) {
            s.add("Scale", format!("{}%", self.scale));
        }
        if self.has(0x100) {
            s.add("Copies", self.copies.to_string());
        }
        if self.has(0x8000) {
            s.add("Collate", if self.collate == 1 { "Yes" } else { "No" });
        }
        if self.has(0x200) {
            s.add("Paper source", source_name(self.default_source));
        }
        if self.has(0x1000) {
            s.add("Duplex", self.duplex_name());
        }
        if self.has(0x800) {
            s.add("Colour", if self.color == 2 { "Colour" } else { "Monochrome" });
        }
        if self.has(0x400) {
            let q = match self.print_quality {
                -1 => "Draft".to_string(),
                -2 => "Low".to_string(),
                -3 => "Medium".to_string(),
                -4 => "High".to_string(),
                n => format!("{n} dpi"),
            };
            let y = if self.has(0x2000) && self.y_resolution > 0 && self.y_resolution != self.print_quality { format!(" × {} dpi", self.y_resolution) } else { String::new() };
            s.add("Print quality", format!("{q}{y}"));
        }
        if self.has(0x40) && self.nup > 0 {
            s.add("N-up handling", if self.nup == 1 { "Done by the spooler" } else { "Done by the driver / printer" });
        }
        if self.has(0x4000) {
            s.add(
                "TrueType fonts",
                match self.tt_option {
                    1 => "Print as graphics",
                    2 => "Download as soft fonts",
                    3 => "Substitute device fonts",
                    4 => "Download as outline fonts",
                    _ => "Not set",
                },
            );
        }
        if self.has(0x2000000) {
            s.add(
                "Media type",
                match self.media_type {
                    1 => "Standard".to_string(),
                    2 => "Transparency".to_string(),
                    3 => "Glossy".to_string(),
                    n if n >= 256 => format!("Driver-defined ({n})"),
                    n => format!("code {n}"),
                },
            );
        }
        if self.has(0x4000000) {
            s.add(
                "Dithering",
                match self.dither_type {
                    1 => "None".to_string(),
                    2 => "Coarse".to_string(),
                    3 => "Fine".to_string(),
                    4 => "Line art".to_string(),
                    5 => "Error diffusion".to_string(),
                    10 => "Greyscale".to_string(),
                    u32::MAX => "Not set".to_string(),
                    n if n >= 256 => format!("Driver-defined ({n})"),
                    n => format!("code {n}"),
                },
            );
        }
        if self.has(0x800000) {
            s.add(
                "Colour management",
                match self.icm_method {
                    1 => "Off",
                    2 => "Handled by Windows",
                    3 => "Handled by the driver",
                    4 => "Handled by the printer",
                    _ => "Driver-defined",
                },
            );
        }
        if self.driver_extra > 0 {
            s.add(
                "Driver private data",
                format!("{} bytes{}", self.driver_extra, if self.driver_extra_sig.is_empty() { String::new() } else { format!(" (tag \"{}\")", self.driver_extra_sig) }),
            );
        }
        s
    }
}

impl Shadow {
    pub fn sections(&self) -> Vec<InfoSection> {
        let mut s = InfoSection::new("Windows print job (.SHD shadow file)");
        s.add("Shadow file", self.file.clone());
        s.add(
            "Format",
            match self.signature {
                0x4966 => "Windows NT 4",
                0x4967 => "Windows 2000",
                0x4968 => "Windows XP / Server 2003",
                0x5123 => "Windows Vista or later",
                _ => "Unknown",
            },
        );
        s.add("Job ID", self.job_id.to_string());
        s.add("Document", self.document.clone());
        s.add("Owner", self.user.clone());
        if !self.notify.is_empty() && self.notify != self.user {
            s.add("Notify", self.notify.clone());
        }
        if !self.machine.is_empty() {
            s.add("Submitted from computer", self.machine.clone());
        }
        if !self.user_sid.is_empty() {
            s.add("Owner SID", self.user_sid.clone());
        }
        if !self.submitted.is_empty() {
            s.add("Submitted", self.submitted.clone());
        }
        s.add("Printer", self.printer.clone());
        s.add("Driver", self.driver.clone());
        if !self.port.is_empty() {
            s.add("Port", self.port.clone());
        }
        s.add("Print processor", self.print_processor.clone());
        s.add(
            "Data type",
            format!(
                "{}{}",
                self.datatype,
                if self.datatype.to_ascii_uppercase().starts_with("NT EMF") {
                    " (spooled as EMF; the driver renders it later)"
                } else if self.datatype.eq_ignore_ascii_case("RAW") {
                    " (printer-ready data)"
                } else if self.datatype.to_ascii_uppercase().contains("XPS") {
                    " (XPS)"
                } else {
                    ""
                }
            ),
        );
        if !self.parameters.is_empty() {
            s.add("Print processor parameters", self.parameters.clone());
        }
        s.add("Priority", self.priority.to_string());
        if self.total_pages > 0 {
            s.add("Pages (spooler count)", self.total_pages.to_string());
        }
        if self.spool_size > 0 {
            s.add("Spool file size (spooler)", crate::doc::fmt_bytes(self.spool_size as usize));
        }
        if self.start_time != 0 || self.until_time != 0 {
            let hm = |m: u32| format!("{:02}:{:02}", m / 60, m % 60);
            s.add("Print window", format!("{} – {}", hm(self.start_time), hm(self.until_time)));
        }
        let mut out = vec![s];
        if let Some(dm) = &self.devmode {
            out.push(dm.section());
        }
        if !self.properties.is_empty() {
            let mut p = InfoSection::new("Job properties (.SHD)");
            for (k, v) in &self.properties {
                p.add(k.clone(), v.clone());
            }
            out.push(p);
        }
        out
    }
}

/// Find the shadow file that belongs with a spool file (same name, .SHD).
pub fn find_for(spl: &std::path::Path) -> Option<std::path::PathBuf> {
    let stem = spl.file_stem()?.to_string_lossy().to_string();
    let dir = spl.parent()?;
    for ext in ["SHD", "shd", "Shd"] {
        let p = dir.join(format!("{stem}.{ext}"));
        if p.exists() {
            return Some(p);
        }
    }
    None
}

/// Find the spool file that belongs with a shadow file.
pub fn find_spl(shd: &std::path::Path) -> Option<std::path::PathBuf> {
    let stem = shd.file_stem()?.to_string_lossy().to_string();
    let dir = shd.parent()?;
    for ext in ["SPL", "spl", "Spl"] {
        let p = dir.join(format!("{stem}.{ext}"));
        if p.exists() {
            return Some(p);
        }
    }
    None
}
