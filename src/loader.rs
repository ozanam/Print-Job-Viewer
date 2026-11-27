//! Load a print file: split into PJL + language segments and interpret each.

use crate::doc::*;
use crate::pjl::{self, Lang};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::Path;
use std::rc::Rc;

pub fn load_file(path: &Path) -> Result<Document, String> {
    let is_shd = path.extension().map(|e| e.eq_ignore_ascii_case("shd")).unwrap_or(false);
    let (path, shd_path) = if is_shd {
        match crate::shd::find_spl(path) {
            Some(spl) => (spl, Some(path.to_path_buf())),
            None => {
                // a shadow file on its own: show its job record
                let d = std::fs::read(path).map_err(|e| format!("Cannot read {}: {e}", path.display()))?;
                let sh = crate::shd::parse(&d, &path.display().to_string()).ok_or("This is not a Windows spooler shadow (.SHD) file")?;
                let mut doc = Document::default();
                doc.format = "Windows spooler shadow file (.SHD)".into();
                let mut f = InfoSection::new("File");
                f.add("Name", path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default());
                f.add("Location", path.parent().map(|p| p.display().to_string()).unwrap_or_default());
                f.add("Size", fmt_bytes(d.len()));
                f.add("Note", "The matching .SPL spool file was not found in the same folder, so there are no pages to show.");
                doc.info.push(f);
                doc.info.extend(sh.sections());
                doc.shd = Some(sh);
                doc.raw = std::sync::Arc::new(d);
                return Ok(doc);
            }
        }
    } else {
        (path.to_path_buf(), crate::shd::find_for(path))
    };
    let path = path.as_path();
    let data = std::fs::read(path).map_err(|e| format!("Cannot read {}: {e}", path.display()))?;
    let mut doc = load_bytes(&data);
    if let Some(sp) = shd_path {
        if let Ok(d) = std::fs::read(&sp) {
            if let Some(sh) = crate::shd::parse(&d, &sp.display().to_string()) {
                let at = 1.min(doc.info.len());
                for (k, sec) in sh.sections().into_iter().enumerate() {
                    doc.info.insert(at + 1 + k, sec);
                }
                doc.shd = Some(sh);
            }
        }
    }
    let mut f = InfoSection::new("File");
    f.add("Name", path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default());
    f.add("Location", path.parent().map(|p| p.display().to_string()).unwrap_or_default());
    f.add("Size", fmt_bytes(data.len()));
    if let Ok(md) = std::fs::metadata(path) {
        if let Ok(m) = md.modified() {
            if let Ok(d) = m.duration_since(std::time::UNIX_EPOCH) {
                f.add("Modified", fmt_unix(d.as_secs() as i64));
            }
        }
    }
    doc.info[0].rows.splice(0..0, f.rows);
    analyse(&mut doc);
    Ok(doc)
}

/// Measure coverage and run the settings checks, adding their Info sections.
pub fn analyse(doc: &mut Document) {
    let t = std::time::Instant::now();
    let mut cov = Vec::new();
    for p in &doc.pages {
        if t.elapsed().as_secs_f32() > 10.0 {
            break;
        }
        cov.push(crate::analyze::page_coverage(p));
    }
    let complete = cov.len() == doc.pages.len();
    doc.findings = crate::analyze::check(doc, if complete { Some(&cov) } else { None });
    let at = 1.min(doc.info.len());
    doc.info.insert(at, crate::analyze::checks_section(&doc.findings));
    if !cov.is_empty() {
        let mut s = crate::analyze::coverage_section(&cov);
        if !complete {
            s.add("Note", format!("Only the first {} pages were measured (time limit).", cov.len()));
        }
        let at = doc.info.iter().position(|s| s.title == "Rendered pages").map(|i| i + 1).unwrap_or(doc.info.len());
        doc.info.insert(at, s);
    }
    doc.coverage = cov;
}

pub fn fmt_unix(secs: i64) -> String {
    // civil-from-days (UTC)
    let days = secs.div_euclid(86400);
    let rem = secs.rem_euclid(86400);
    let z = days + 719468;
    let era = z.div_euclid(146097);
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02} {:02}:{:02}:{:02} UTC", rem / 3600, rem % 3600 / 60, rem % 60)
}

pub fn load_bytes(data: &[u8]) -> Document {
    let mut doc = Document::default();
    doc.raw = std::sync::Arc::new(data.to_vec());
    doc.info.push(InfoSection::new("File"));
    // container formats that aren't printer languages
    let special: Option<(&str, fn(&mut Document, &[u8]))> = if crate::emf::is_emf_spool(data) {
        Some(("Windows EMF spool file (NT EMF)", crate::emf::run_spool))
    } else if crate::emf::is_emf(data) {
        Some(("Windows Enhanced Metafile (EMF)", crate::emf::run_emf))
    } else if crate::xps::is_xps(data) {
        Some(("XPS document", crate::xps::run))
    } else {
        None
    };
    if let Some((fmt, f)) = special {
        doc.format = fmt.into();
        doc.info[0].add("Detected format", fmt);
        if catch_unwind(AssertUnwindSafe(|| f(&mut doc, data))).is_err() {
            doc.warn(format!("Internal error while interpreting the {fmt} (partial result shown)"));
        }
        let mut pages = InfoSection::new("Rendered pages");
        pages.add("Total", doc.pages.len().to_string());
        for (k, p) in doc.pages.iter().enumerate() {
            pages.add(format!("Page {}", k + 1), format!("{}  · {} drawing objects", fmt_size_pt(p.width, p.height), p.items.len()));
        }
        doc.info.insert(1, pages);
        return doc;
    }
    let (pjl_info, segs) = pjl::split(data);
    doc.pjl = pjl_info.settings.iter().map(|(k, v)| (k.to_ascii_uppercase(), v.clone())).collect();
    if !pjl_info.commands.is_empty() {
        doc.pjl_end = segs.first().map(|s| s.start).unwrap_or(data.len());
    }
    let mut langs: Vec<&'static str> = Vec::new();
    if !pjl_info.commands.is_empty() {
        langs.push("PJL");
    }
    for s in &segs {
        if !langs.contains(&s.lang.name()) {
            langs.push(s.lang.name());
        }
    }
    doc.format = if langs.is_empty() { "Unknown".into() } else { langs.join(" + ") };
    doc.info[0].add("Detected format", doc.format.clone());
    let mut seg_info = InfoSection::new("Job structure");
    for (k, s) in segs.iter().enumerate() {
        seg_info.add(format!("Segment {}", k + 1), format!("{} — offset {:#X}, {}", s.lang.name(), s.start, fmt_bytes(s.end - s.start)));
    }
    doc.info.extend(pjl_info.to_sections());
    if !seg_info.rows.is_empty() {
        doc.info.push(seg_info);
    }

    let shared = Rc::new(data.to_vec());
    for s in &segs {
        let bytes = &data[s.start..s.end];
        match s.lang {
            Lang::PclXl => {
                let r = catch_unwind(AssertUnwindSafe(|| {
                    let mut x = crate::pclxl::XlInterp::new(&mut doc);
                    x.run(bytes, s.start);
                    x.finish();
                }));
                if r.is_err() {
                    doc.warn("Internal error while interpreting PCL XL (partial result shown)");
                }
            }
            Lang::PostScript => {
                let r = catch_unwind(AssertUnwindSafe(|| run_ps(&mut doc, &shared, s.start, s.end, &pjl_info)));
                if r.is_err() {
                    doc.warn("Internal error while interpreting PostScript (partial result shown)");
                }
            }
            Lang::Pcl5 => {
                let r = catch_unwind(AssertUnwindSafe(|| {
                    let mut x = crate::pcl5::Pcl5::new(&mut doc);
                    x.run(bytes, s.start);
                }));
                if r.is_err() {
                    doc.warn("Internal error while interpreting PCL 5 (partial result shown)");
                }
            }
            Lang::Hpgl => {
                let r = catch_unwind(AssertUnwindSafe(|| crate::hpgl::run_standalone(&mut doc, data, s.start)));
                if r.is_err() {
                    doc.warn("Internal error while interpreting HP-GL/2 (partial result shown)");
                }
            }
            Lang::Pdf => doc.warn("PDF data is not supported"),
            Lang::Unknown => {
                if bytes.iter().any(|b| !b.is_ascii_whitespace()) {
                    doc.warn(format!("Unrecognised data at offset {:#X} ({})", s.start, fmt_bytes(bytes.len())));
                }
            }
        }
    }
    let mut pages = InfoSection::new("Rendered pages");
    pages.add("Total", doc.pages.len().to_string());
    for (k, p) in doc.pages.iter().enumerate() {
        pages.add(format!("Page {}", k + 1), format!("{}  · {} drawing objects", fmt_size_pt(p.width, p.height), p.items.len()));
    }
    doc.info.insert(1.min(doc.info.len()), pages);
    doc
}

fn run_ps(doc: &mut Document, data: &Rc<Vec<u8>>, start: usize, end: usize, pjl_info: &pjl::PjlInfo) {
    let mut start = start;
    let mut end = end;
    // skip ^D and whitespace
    while start < end && (data[start] == 4 || data[start].is_ascii_whitespace()) {
        start += 1;
    }
    // DOS EPS binary header
    if data[start..].starts_with(&[0xC5, 0xD0, 0xD3, 0xC6]) && end - start > 30 {
        let off = u32::from_le_bytes([data[start + 4], data[start + 5], data[start + 6], data[start + 7]]) as usize;
        let len = u32::from_le_bytes([data[start + 8], data[start + 9], data[start + 10], data[start + 11]]) as usize;
        let s2 = start + off;
        if s2 + len <= end {
            start = s2;
            end = s2 + len;
        }
    }
    let slice = &data[start..end];
    let (dsc, media, bbox) = crate::ps::dsc_info(slice);
    doc.info.extend(dsc);
    let header = slice.split(|&c| c == b'\n' || c == b'\r').next().unwrap_or(b"");
    let is_eps = header.windows(4).any(|w| w == b"EPSF");
    let job = Rc::new(data[..end].to_vec());
    let mut it = crate::ps::Interp::new(doc);
    let mut size = media;
    if size.is_none() {
        if let Some(p) = pjl_info.get("PAPER") {
            size = match p.to_ascii_uppercase().as_str() {
                "A4" => Some((595.0, 842.0)),
                "A3" => Some((842.0, 1191.0)),
                "A5" => Some((420.0, 595.0)),
                "LEGAL" => Some((612.0, 1008.0)),
                "LETTER" => Some((612.0, 792.0)),
                "LEDGER" | "11X17" => Some((792.0, 1224.0)),
                "EXECUTIVE" => Some((522.0, 756.0)),
                _ => None,
            };
        }
    }
    let mut eps_offset = (0.0, 0.0);
    if is_eps {
        if let Some(b) = bbox {
            if b[2] > b[0] && b[3] > b[1] {
                size = Some((b[2] - b[0], b[3] - b[1]));
                eps_offset = (-b[0], -b[1]);
            }
        }
    } else if size.is_none() {
        if let Some(b) = bbox {
            if b[2] <= 600.0 && b[3] > 795.0 && b[3] <= 845.0 {
                size = Some((595.0, 842.0));
            }
        }
    }
    if let Some((w, h)) = size {
        it.pagedev.width = w;
        it.pagedev.height = h;
        it.page = Page::new(w as f32, h as f32);
    }
    it.initgraphics();
    if eps_offset != (0.0, 0.0) {
        it.gs.ctm = crate::geom::Matrix::translate(eps_offset.0, eps_offset.1).then(&it.gs.ctm);
    }
    let _ = data;
    let err = it.run_job(job, start);
    it.finish(err);
}
