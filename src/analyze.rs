//! Page analysis (toner coverage, colour use, ink extent) and job settings checks.

use crate::doc::{Document, InfoSection, Page};

/// Resolution used to measure coverage.
const DPI: f32 = 40.0;
/// Typical unprintable margin of a laser printer (4.2 mm).
pub const MARGIN_PT: f32 = 4.2 / 25.4 * 72.0;

#[derive(Clone, Copy, Debug, Default)]
pub struct Coverage {
    /// Average coverage of each toner, 0..1 (100% = solid over the whole page).
    pub c: f32,
    pub m: f32,
    pub y: f32,
    pub k: f32,
    /// Share of the page with any ink at all.
    #[allow(dead_code)]
    pub inked: f32,
    /// Share of the page that is in colour (not neutral grey).
    pub colour_area: f32,
    pub has_colour: bool,
    pub blank: bool,
    /// Bounding box of the ink, in page points (x0, y0, x1, y1); None if blank.
    pub ink_box: Option<[f32; 4]>,
}

impl Coverage {
    #[allow(dead_code)]
    pub fn total(&self) -> f32 {
        self.c + self.m + self.y + self.k
    }
    pub fn describe(&self) -> String {
        if self.blank {
            return "Blank".into();
        }
        if self.has_colour {
            format!(
                "Colour · C {:.1}%  M {:.1}%  Y {:.1}%  K {:.1}%  (colour on {:.1}% of the page)",
                self.c * 100.0,
                self.m * 100.0,
                self.y * 100.0,
                self.k * 100.0,
                self.colour_area * 100.0
            )
        } else {
            format!("Black & white · K {:.1}%", self.k * 100.0)
        }
    }
}

pub fn page_coverage(p: &Page) -> Coverage {
    let scale = DPI / 72.0;
    let Some(r) = crate::render::render_page(p, scale) else { return Coverage { blank: true, ..Default::default() } };
    let n = (r.width * r.height).max(1) as f32;
    let (mut c, mut m, mut y, mut k) = (0f64, 0f64, 0f64, 0f64);
    let mut inked = 0u32;
    let mut colour = 0u32;
    let (mut x0, mut y0, mut x1, mut y1) = (u32::MAX, u32::MAX, 0u32, 0u32);
    for (i, px) in r.rgba.chunks_exact(4).enumerate() {
        let (rr, gg, bb) = (px[0] as f32 / 255.0, px[1] as f32 / 255.0, px[2] as f32 / 255.0);
        let kk = 1.0 - rr.max(gg).max(bb);
        if kk < 0.02 && rr.min(gg).min(bb) > 0.98 {
            continue;
        }
        inked += 1;
        let x = i as u32 % r.width;
        let yy = i as u32 / r.width;
        x0 = x0.min(x);
        y0 = y0.min(yy);
        x1 = x1.max(x);
        y1 = y1.max(yy);
        let chroma = rr.max(gg).max(bb) - rr.min(gg).min(bb);
        if chroma > 0.09 {
            colour += 1;
            let d = (1.0 - kk).max(1e-6);
            c += ((1.0 - rr - kk) / d) as f64 * d as f64;
            m += ((1.0 - gg - kk) / d) as f64 * d as f64;
            y += ((1.0 - bb - kk) / d) as f64 * d as f64;
            k += kk as f64;
        } else {
            k += (1.0 - (rr + gg + bb) / 3.0) as f64;
        }
    }
    let nn = n as f64;
    let blank = inked == 0;
    Coverage {
        c: (c / nn) as f32,
        m: (m / nn) as f32,
        y: (y / nn) as f32,
        k: (k / nn) as f32,
        inked: inked as f32 / n,
        colour_area: colour as f32 / n,
        has_colour: colour >= 3,
        blank,
        ink_box: if blank { None } else { Some([x0 as f32 / scale, y0 as f32 / scale, (x1 + 1) as f32 / scale, (y1 + 1) as f32 / scale]) },
    }
}

pub fn coverage_section(cov: &[Coverage]) -> InfoSection {
    let mut s = InfoSection::new("Toner coverage and colour");
    let colour = cov.iter().filter(|c| c.has_colour).count();
    let blank = cov.iter().filter(|c| c.blank).count();
    s.add("Colour pages", colour.to_string());
    s.add("Black & white pages", (cov.len() - colour - blank).to_string());
    if blank > 0 {
        s.add("Blank pages", blank.to_string());
    }
    if !cov.is_empty() {
        let n = cov.len() as f32;
        let avg = |f: fn(&Coverage) -> f32| cov.iter().map(f).sum::<f32>() / n * 100.0;
        s.add("Average coverage per page", format!("C {:.1}%  M {:.1}%  Y {:.1}%  K {:.1}%", avg(|c| c.c), avg(|c| c.m), avg(|c| c.y), avg(|c| c.k)));
        s.add("Note", "Coverage is estimated from the rendered page (5% is the industry reference page). Actual toner use depends on the printer.");
    }
    for (i, c) in cov.iter().enumerate() {
        s.add(format!("Page {}", i + 1), c.describe());
    }
    s
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Level {
    Problem,
    Warning,
    Note,
}

pub struct Finding {
    pub level: Level,
    pub text: String,
}

fn yes(v: &str) -> bool {
    let u = v.trim().trim_matches('"').to_ascii_uppercase();
    !(u.is_empty() || u == "OFF" || u == "NONE" || u == "NO" || u == "0" || u == "FALSE" || u == "UNDEFINED")
}

fn pjl_paper_pt(name: &str) -> Option<(f32, f32)> {
    Some(match name.trim_matches('"').to_ascii_uppercase().as_str() {
        "LETTER" => (612.0, 792.0),
        "LEGAL" => (612.0, 1008.0),
        "A4" => (595.3, 841.9),
        "A3" => (841.9, 1190.6),
        "A5" => (419.5, 595.3),
        "A6" => (297.6, 419.5),
        "EXECUTIVE" => (522.0, 756.0),
        "LEDGER" | "11X17" | "TABLOID" => (792.0, 1224.0),
        "B5" | "JISB5" => (515.9, 728.5),
        "B4" | "JISB4" => (728.5, 1031.8),
        "COM10" => (297.0, 684.0),
        "DL" => (311.8, 623.6),
        "C5" => (459.2, 649.1),
        "MONARCH" => (279.0, 540.0),
        _ => return None,
    })
}

pub fn devmode_paper_pt(code: i16) -> Option<(f32, f32)> {
    Some(match code {
        1 => (612.0, 792.0),
        5 => (612.0, 1008.0),
        7 => (522.0, 756.0),
        8 => (841.9, 1190.6),
        9 => (595.3, 841.9),
        11 => (419.5, 595.3),
        3 | 4 | 17 => (792.0, 1224.0),
        20 => (297.0, 684.0),
        27 => (311.8, 623.6),
        28 => (459.2, 649.1),
        70 => (297.6, 419.5),
        _ => return None,
    })
}

fn same_size(a: (f32, f32), p: &Page) -> bool {
    let (w, h) = (p.width.min(p.height), p.width.max(p.height));
    let (aw, ah) = (a.0.min(a.1), a.0.max(a.1));
    (aw - w).abs() < 4.0 && (ah - h).abs() < 4.0
}

/// Check the job's settings against each other and against the page content.
pub fn check(doc: &Document, cov: Option<&[Coverage]>) -> Vec<Finding> {
    let mut out = Vec::new();
    let mut add = |level, text: String| out.push(Finding { level, text });
    let n = doc.pages.len();
    let pj = |k: &str| doc.pjl_get(k);

    // finishing
    let mut finishing = Vec::new();
    for key in ["STAPLE", "PUNCH", "FOLD", "BOOKLET"] {
        if let Some(v) = pj(key) {
            if yes(v) {
                finishing.push(format!("{} ({v})", key.to_ascii_lowercase()));
            }
        }
    }
    // HP's PROCESSINGTYPE/OPTION pairs: only an option other than NONE asks for finishing
    let mut kind: Option<String> = None;
    for (k, v) in &doc.pjl {
        if k == "PROCESSINGTYPE" {
            kind = Some(v.trim_matches('"').to_ascii_lowercase());
        } else if k == "PROCESSINGOPTION" && yes(v) {
            finishing.push(format!("{} ({})", kind.clone().unwrap_or("finishing".into()), v.trim_matches('"')));
        }
    }
    if !finishing.is_empty() {
        if n == 1 {
            add(Level::Warning, format!("Finishing is requested ({}) on a one-page job.", finishing.join(", ")));
        } else {
            add(Level::Note, format!("Finishing requested: {}.", finishing.join(", ")));
        }
    }

    // duplex
    let pjl_duplex = pj("DUPLEX").map(yes);
    let dm = doc.shd.as_ref().and_then(|s| s.devmode.as_ref());
    let dm_duplex = dm.filter(|d| d.has(0x1000)).map(|d| d.duplex >= 2);
    let xl_duplex = doc.pages.iter().any(|p| p.settings.iter().any(|(k, v)| k == "Duplex" && v.contains("Duplex")));
    if (pjl_duplex == Some(true) || dm_duplex == Some(true) || xl_duplex) && n == 1 {
        add(Level::Note, "Two-sided printing is requested, but the job has only one page.".into());
    }
    if let (Some(a), Some(b)) = (pjl_duplex, dm_duplex) {
        if a != b {
            add(Level::Warning, format!("The driver settings say {} but the PJL header says {}.", if b { "two-sided" } else { "one-sided" }, if a { "two-sided" } else { "one-sided" }));
        }
    }

    // paper size
    if let Some(v) = pj("PAPER") {
        if let Some(sz) = pjl_paper_pt(v) {
            let bad: Vec<usize> = doc.pages.iter().enumerate().filter(|(_, p)| !same_size(sz, p)).map(|(i, _)| i + 1).collect();
            if !bad.is_empty() {
                add(Level::Warning, format!("PJL asks for {} paper, but {} {} a different size.", v.trim_matches('"'), page_list(&bad), verb(&bad, "has", "have")));
            }
        }
    }
    if let Some(d) = dm.filter(|d| d.has(0x2)) {
        if let Some(sz) = devmode_paper_pt(d.paper_size) {
            let bad: Vec<usize> = doc.pages.iter().enumerate().filter(|(_, p)| !same_size(sz, p)).map(|(i, _)| i + 1).collect();
            if !bad.is_empty() {
                add(Level::Warning, format!("The driver is set to {} paper, but {} {} a different size.", crate::shd::paper_size_name(d.paper_size), page_list(&bad), verb(&bad, "has", "have")));
            }
        }
    }
    let mut sizes: Vec<String> = Vec::new();
    for p in &doc.pages {
        let s = crate::doc::paper_name(p.width, p.height).map(|s| s.to_string()).unwrap_or_else(|| format!("{:.0}×{:.0} mm", p.width / 72.0 * 25.4, p.height / 72.0 * 25.4));
        if !sizes.contains(&s) {
            sizes.push(s);
        }
    }
    if sizes.len() > 1 {
        add(Level::Note, format!("The job mixes paper sizes: {}.", sizes.join(", ")));
    }

    // copies
    let mut copies: Vec<(String, String)> = Vec::new();
    if let Some(v) = pj("COPIES") {
        copies.push(("PJL COPIES".into(), v.into()));
    }
    if let Some(v) = pj("QTY") {
        copies.push(("PJL QTY".into(), v.into()));
    }
    if let Some(d) = dm.filter(|d| d.has(0x100)) {
        copies.push(("driver".into(), d.copies.to_string()));
    }
    if let Some(v) = doc.pages.iter().find_map(|p| p.settings.iter().find(|(k, _)| k == "Copies").map(|(_, v)| v.clone())) {
        copies.push(("page data".into(), v));
    }
    let vals: Vec<i64> = copies.iter().filter_map(|(_, v)| v.trim().parse::<f64>().ok().map(|x| x as i64)).collect();
    if vals.iter().any(|&v| v > 1) && vals.iter().any(|&v| v != vals[0]) {
        add(
            Level::Warning,
            format!("Copy counts disagree: {}. The printer may multiply them.", copies.iter().map(|(a, b)| format!("{a} = {b}")).collect::<Vec<_>>().join(", ")),
        );
    } else if let Some(&c) = vals.iter().max() {
        if c > 1 {
            add(Level::Note, format!("{c} copies requested."));
        }
    }

    // colour
    let mono_pjl = pj("RENDERMODE").map(|v| v.to_ascii_uppercase().contains("GRAY") || v.to_ascii_uppercase().contains("GREY")).unwrap_or(false)
        || pj("GRAYSCALE").map(|v| yes(v) && !v.eq_ignore_ascii_case("OFF")).unwrap_or(false)
        || pj("PLANESINUSE").map(|v| v.trim() == "1").unwrap_or(false);
    let colour_dm = dm.and_then(|d| d.is_color());
    if let Some(cov) = cov {
        let cpages: Vec<usize> = cov.iter().enumerate().filter(|(_, c)| c.has_colour).map(|(i, _)| i + 1).collect();
        if mono_pjl && !cpages.is_empty() {
            add(Level::Note, format!("The job is set to print in black only, but {} {} colour (it will print as grey).", page_list(&cpages), verb(&cpages, "contains", "contain")));
        }
        if colour_dm == Some(true) && cpages.is_empty() && !cov.is_empty() && !mono_pjl {
            add(Level::Note, "The driver is set to colour, but every page is black and white. Some printers still count this as a colour job.".into());
        }
        let blank: Vec<usize> = cov.iter().enumerate().filter(|(_, c)| c.blank).map(|(i, _)| i + 1).collect();
        if !blank.is_empty() {
            add(Level::Warning, format!("{} {} blank.", cap(&page_list(&blank)), verb(&blank, "is", "are")));
        }
        let edge: Vec<usize> = cov
            .iter()
            .zip(doc.pages.iter())
            .enumerate()
            .filter(|(_, (c, p))| c.ink_box.map(|b| b[0] < MARGIN_PT - 1.0 || b[1] < MARGIN_PT - 1.0 || b[2] > p.width - MARGIN_PT + 1.0 || b[3] > p.height - MARGIN_PT + 1.0).unwrap_or(false))
            .map(|(i, _)| i + 1)
            .collect();
        if !edge.is_empty() {
            add(Level::Note, format!("{} {} content within 4.2 mm of the paper edge, which most printers can't reach (it may be cut off).", cap(&page_list(&edge)), verb(&edge, "has", "have")));
        }
    }

    // shadow file consistency
    if let Some(s) = &doc.shd {
        if s.spool_size > 0 && doc.raw.len() > 0 && s.spool_size as usize != doc.raw.len() {
            add(
                Level::Warning,
                format!(
                    "The .SHD file records a {} spool file, but this file is {}. It may belong to a different job, or the job was still spooling when copied.",
                    crate::doc::fmt_bytes(s.spool_size as usize),
                    crate::doc::fmt_bytes(doc.raw.len())
                ),
            );
        }
        if s.total_pages > 0 && n > 0 && s.total_pages as usize != n {
            add(Level::Note, format!("The spooler counted {} page(s); {} page(s) were rendered.", s.total_pages, n));
        }
    }
    if let Some(v) = pj("HOLD") {
        if yes(v) {
            add(Level::Note, format!("The job is held at the printer (HOLD={v}){}.", pj("HOLDKEY").map(|_| " and needs a PIN to release").unwrap_or("")));
        }
    }
    if !doc.warnings.is_empty() {
        add(Level::Note, format!("{} message(s) came up while reading the job; see the Messages tab.", doc.warnings.len()));
    }
    if n == 0 && !doc.raw.is_empty() && doc.shd.as_ref().map(|s| s.spool_size > 0 || doc.raw.len() > 4096).unwrap_or(true) {
        add(Level::Problem, "No pages could be rendered from this file.".into());
    }
    out.sort_by_key(|f| f.level);
    out
}

fn verb(p: &[usize], one: &'static str, many: &'static str) -> &'static str {
    if p.len() == 1 { one } else { many }
}

fn cap(s: &str) -> String {
    let mut c = s.chars();
    c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
}

fn page_list(p: &[usize]) -> String {
    if p.len() == 1 {
        return format!("page {}", p[0]);
    }
    let mut parts: Vec<String> = Vec::new();
    let mut i = 0;
    while i < p.len() {
        let mut j = i;
        while j + 1 < p.len() && p[j + 1] == p[j] + 1 {
            j += 1;
        }
        parts.push(if j > i { format!("{}–{}", p[i], p[j]) } else { p[i].to_string() });
        i = j + 1;
    }
    let s = parts.join(", ");
    if s.len() > 120 { format!("{} pages", p.len()) } else { format!("pages {s}") }
}

pub fn checks_section(f: &[Finding]) -> InfoSection {
    let mut s = InfoSection::new("Settings checks");
    if f.is_empty() {
        s.add("Result", "No problems found");
    }
    for x in f {
        let label = match x.level {
            Level::Problem => "Problem",
            Level::Warning => "Check",
            Level::Note => "Note",
        };
        s.add(label, x.text.clone());
    }
    s
}
