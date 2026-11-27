//! One-line job summaries (for the batch report, folder watcher and comparisons),
//! and the folder batch report itself.

use crate::doc::Document;
use std::path::{Path, PathBuf};

#[derive(Clone, Default)]
pub struct JobSummary {
    pub file: String,
    pub path: PathBuf,
    pub size: u64,
    pub modified: String,
    pub format: String,
    pub pages: usize,
    pub colour_pages: usize,
    pub mono_pages: usize,
    pub blank_pages: usize,
    pub paper: String,
    pub duplex: String,
    pub copies: String,
    pub user: String,
    pub job_name: String,
    pub application: String,
    pub computer: String,
    pub driver: String,
    pub printer: String,
    pub submitted: String,
    pub coverage_k: f32,
    pub coverage_cmy: f32,
    pub checks: usize,
    pub check_text: String,
    pub warnings: usize,
    pub error: String,
}

fn info_get(doc: &Document, section_contains: &str, key: &str) -> Option<String> {
    for s in &doc.info {
        if !s.title.contains(section_contains) {
            continue;
        }
        for (k, v) in &s.rows {
            if k == key || k.starts_with(&format!("{key} (")) {
                if !v.trim().is_empty() {
                    return Some(v.trim().trim_matches('"').to_string());
                }
            }
        }
    }
    None
}

fn jobattr(doc: &Document, name: &str) -> Option<String> {
    doc.pjl.iter().rev().find_map(|(k, v)| {
        if k == "JOBATTR" {
            v.split_once('=').filter(|(a, _)| a.trim().eq_ignore_ascii_case(name)).map(|(_, b)| b.trim().to_string())
        } else if k.starts_with("JOBATTR ") && k[8..].trim().eq_ignore_ascii_case(name) {
            Some(v.trim().to_string())
        } else {
            None
        }
    })
    .filter(|v| !v.is_empty())
}

pub fn summarise(doc: &Document, path: &Path) -> JobSummary {
    let md = std::fs::metadata(path).ok();
    let mut s = JobSummary {
        file: path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default(),
        path: path.to_path_buf(),
        size: md.as_ref().map(|m| m.len()).unwrap_or(doc.raw.len() as u64),
        modified: info_get(doc, "File", "Modified").unwrap_or_default(),
        format: doc.format.clone(),
        pages: doc.pages.len(),
        ..Default::default()
    };
    s.colour_pages = doc.coverage.iter().filter(|c| c.has_colour).count();
    s.blank_pages = doc.coverage.iter().filter(|c| c.blank).count();
    s.mono_pages = doc.coverage.len() - s.colour_pages - s.blank_pages;
    if !doc.coverage.is_empty() {
        let n = doc.coverage.len() as f32;
        s.coverage_k = doc.coverage.iter().map(|c| c.k).sum::<f32>() / n * 100.0;
        s.coverage_cmy = doc.coverage.iter().map(|c| c.c + c.m + c.y).sum::<f32>() / n * 100.0;
    }
    let mut sizes: Vec<String> = Vec::new();
    for p in &doc.pages {
        let n = crate::doc::paper_name(p.width, p.height).map(|s| s.to_string()).unwrap_or_else(|| format!("{:.0}×{:.0} mm", p.width / 72.0 * 25.4, p.height / 72.0 * 25.4));
        if !sizes.contains(&n) {
            sizes.push(n);
        }
    }
    s.paper = sizes.join(", ");
    let shd = doc.shd.as_ref();
    let dm = shd.and_then(|x| x.devmode.as_ref());
    s.duplex = doc
        .pjl_get("DUPLEX")
        .map(|v| if v.eq_ignore_ascii_case("ON") { format!("On{}", doc.pjl_get("BINDING").map(|b| format!(" ({b})")).unwrap_or_default()) } else { "Off".into() })
        .or_else(|| doc.pages.iter().find_map(|p| p.settings.iter().find(|(k, _)| k == "Duplex").map(|(_, v)| v.clone())))
        .or_else(|| dm.filter(|d| d.has(0x1000)).map(|d| d.duplex_name().to_string()))
        .or_else(|| info_get(doc, "page device", "Duplex").map(|v| match v.as_str() { "true" => "On".to_string(), "false" => "Off".to_string(), _ => v }))
        .unwrap_or_default();
    s.copies = doc
        .pjl_get("COPIES")
        .or(doc.pjl_get("QTY"))
        .map(|v| v.to_string())
        .or_else(|| dm.filter(|d| d.has(0x100)).map(|d| d.copies.to_string()))
        .or_else(|| doc.pages.iter().find_map(|p| p.settings.iter().find(|(k, _)| k == "Copies").map(|(_, v)| v.clone())))
        .unwrap_or_default();
    s.user = shd
        .map(|x| x.user.clone())
        .filter(|v| !v.is_empty())
        .or_else(|| doc.pjl_get("USERNAME").map(|v| v.trim_matches('"').to_string()))
        .or_else(|| jobattr(doc, "JobAcct1"))
        .or_else(|| info_get(doc, "DSC", "For (user)"))
        .unwrap_or_default();
    s.job_name = shd
        .map(|x| x.document.clone())
        .filter(|v| !v.is_empty())
        .or_else(|| info_get(doc, "PJL", "Job name"))
        .or_else(|| doc.pjl_get("JOBNAME").map(|v| v.trim_matches('"').to_string()))
        .or_else(|| info_get(doc, "DSC", "Title"))
        .unwrap_or_default();
    s.application = jobattr(doc, "JobAcct6").or_else(|| info_get(doc, "DSC", "Creator (application)")).unwrap_or_default();
    s.computer = shd.map(|x| x.machine.trim_start_matches('\\').to_string()).filter(|v| !v.is_empty()).or_else(|| jobattr(doc, "JobAcct2")).unwrap_or_default();
    s.driver = shd.map(|x| x.driver.clone()).filter(|v| !v.is_empty()).or_else(|| jobattr(doc, "Render Name")).unwrap_or_default();
    s.printer = shd.map(|x| x.printer.clone()).unwrap_or_default();
    s.submitted = shd.map(|x| x.submitted.clone()).unwrap_or_default();
    let real: Vec<&crate::analyze::Finding> = doc.findings.iter().filter(|f| f.level != crate::analyze::Level::Note).collect();
    s.checks = real.len();
    s.check_text = real.iter().map(|f| f.text.clone()).collect::<Vec<_>>().join(" | ");
    s.warnings = doc.warnings.len();
    s
}

pub const COLUMNS: &[&str] = &[
    "File", "Folder", "Size (bytes)", "Modified", "Format", "Pages", "Colour pages", "B&W pages", "Blank pages", "Paper", "Duplex", "Copies", "User", "Job name", "Application",
    "Computer", "Printer", "Driver", "Submitted", "Avg K coverage %", "Avg CMY coverage %", "Checks", "Check details", "Messages", "Error",
];

impl JobSummary {
    pub fn values(&self) -> Vec<String> {
        vec![
            self.file.clone(),
            self.path.parent().map(|p| p.display().to_string()).unwrap_or_default(),
            self.size.to_string(),
            self.modified.clone(),
            self.format.clone(),
            self.pages.to_string(),
            self.colour_pages.to_string(),
            self.mono_pages.to_string(),
            self.blank_pages.to_string(),
            self.paper.clone(),
            self.duplex.clone(),
            self.copies.clone(),
            self.user.clone(),
            self.job_name.clone(),
            self.application.clone(),
            self.computer.clone(),
            self.printer.clone(),
            self.driver.clone(),
            self.submitted.clone(),
            format!("{:.2}", self.coverage_k),
            format!("{:.2}", self.coverage_cmy),
            self.checks.to_string(),
            self.check_text.clone(),
            self.warnings.to_string(),
            self.error.clone(),
        ]
    }
}

pub fn to_csv(rows: &[JobSummary]) -> String {
    let q = |s: &str| if s.contains([',', '"', '\n', '\r']) { format!("\"{}\"", s.replace('"', "\"\"")) } else { s.to_string() };
    let mut out = String::from("\u{FEFF}");
    out.push_str(&COLUMNS.iter().map(|c| q(c)).collect::<Vec<_>>().join(","));
    out.push_str("\r\n");
    for r in rows {
        out.push_str(&r.values().iter().map(|v| q(v)).collect::<Vec<_>>().join(","));
        out.push_str("\r\n");
    }
    out
}

fn json_str(s: &str) -> String {
    let mut o = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            '\n' => o.push_str("\\n"),
            '\r' => o.push_str("\\r"),
            '\t' => o.push_str("\\t"),
            c if (c as u32) < 0x20 => o.push_str(&format!("\\u{:04x}", c as u32)),
            c => o.push(c),
        }
    }
    o.push('"');
    o
}

pub fn to_json(rows: &[JobSummary]) -> String {
    let mut out = String::from("[\n");
    for (i, r) in rows.iter().enumerate() {
        out.push_str("  {");
        let vals = r.values();
        let numeric = |c: &str| matches!(c, "Size (bytes)" | "Pages" | "Colour pages" | "B&W pages" | "Blank pages" | "Avg K coverage %" | "Avg CMY coverage %" | "Checks" | "Messages");
        for (j, (c, v)) in COLUMNS.iter().zip(vals.iter()).enumerate() {
            if j > 0 {
                out.push_str(", ");
            }
            let val = if numeric(c) && v.parse::<f64>().is_ok() { v.clone() } else { json_str(v) };
            out.push_str(&format!("{}: {}", json_str(c), val));
        }
        out.push('}');
        if i + 1 < rows.len() {
            out.push(',');
        }
        out.push('\n');
    }
    out.push_str("]\n");
    out
}

/// Files in `dir` that look like print jobs (shadow files are read with their .SPL).
pub fn list_jobs(dir: &Path, recursive: bool) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        let mut entries: Vec<PathBuf> = rd.flatten().map(|e| e.path()).collect();
        entries.sort();
        for p in entries {
            if p.is_dir() {
                if recursive {
                    stack.push(p);
                }
                continue;
            }
            if is_job_file(&p) {
                out.push(p);
            }
        }
    }
    out
}

pub fn is_job_file(p: &Path) -> bool {
    let ext = p.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    let name = p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    if name.starts_with('.') {
        return false;
    }
    if ext == "shd" {
        // only when there is no spool file to go with it
        return crate::shd::find_spl(p).is_none();
    }
    if matches!(ext.as_str(), "prn" | "pcl" | "pxl" | "pcl6" | "xl" | "ps" | "eps" | "epsf" | "spl" | "plt" | "hpgl" | "hpg" | "emf" | "xps" | "oxps" | "print" | "job" | "out" | "bin" | "dat") {
        return true;
    }
    if !ext.is_empty() {
        return false;
    }
    // no extension: sniff the start
    let Ok(mut f) = std::fs::File::open(p) else { return false };
    let mut buf = [0u8; 64];
    use std::io::Read;
    let n = f.read(&mut buf).unwrap_or(0);
    let b = &buf[..n];
    b.starts_with(b"\x1b%-12345X") || b.starts_with(b"%!") || b.starts_with(b"@PJL") || b.starts_with(b"\x1bE") || b.windows(9).any(|w| w == b"HP-PCL XL")
}

/// Load and summarise one file (never panics).
pub fn summarise_file(p: &Path) -> JobSummary {
    let r = std::panic::catch_unwind(|| crate::loader::load_file(p));
    match r {
        Ok(Ok(d)) => summarise(&d, p),
        Ok(Err(e)) => JobSummary { file: p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default(), path: p.to_path_buf(), error: e, ..Default::default() },
        Err(_) => JobSummary { file: p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default(), path: p.to_path_buf(), error: "Internal error while reading the file".into(), ..Default::default() },
    }
}
