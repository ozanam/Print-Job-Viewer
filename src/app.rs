//! The application window.

use crate::doc::{self, Brush, Document, Item, Kind, NO_SRC};
use crate::settings::Settings;
use crate::summary::{self, JobSummary};
use crate::{loader, render};
use eframe::egui;
use egui::{Color32, Pos2, Rect, RichText, Stroke, Vec2};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver};
use std::sync::Arc;
use std::time::{Duration, Instant};

pub const APP_NAME: &str = "Print File Viewer";

#[derive(PartialEq, Clone, Copy)]
enum Tab {
    View,
    Info,
    Commands,
    Hex,
    Compare,
    Warnings,
}

#[derive(PartialEq, Clone, Copy)]
enum Fit {
    Width,
    Page,
    Custom,
}

#[derive(PartialEq, Clone, Copy)]
enum CmpMode {
    Side,
    Diff,
    Settings,
}

type RenderKey = (u64, usize, u32, u8);
type LoadRx = Receiver<Result<Document, String>>;

struct CmpImages {
    a: Option<render::Rendered>,
    b: Option<render::Rendered>,
    diff: Option<render::Rendered>,
    pct: f32,
}

struct Compare {
    path: Option<PathBuf>,
    doc: Option<Arc<Document>>,
    loading: Option<(PathBuf, LoadRx)>,
    mode: CmpMode,
    generation: u64,
    key: Option<(u64, u64, usize, u32)>,
    tex: Option<(egui::TextureHandle, egui::TextureHandle, Option<egui::TextureHandle>, f32)>,
    pending: Option<((u64, u64, usize, u32), Receiver<CmpImages>)>,
    page_diffs: Vec<Option<f32>>,
    diffs_rx: Option<Receiver<(u64, usize, f32)>>,
    only_diff: bool,
    rows: Option<(u64, u64, Vec<[String; 4]>)>,
}

struct PrintDlg {
    printers: Vec<String>,
    printer: String,
    range: u8, // 0 all, 1 current, 2 custom
    from: usize,
    to: usize,
    copies: u32,
    fit: bool,
}

struct SendDlg {
    data: Arc<Vec<u8>>,
    what: String,
    to_queue: bool,
    printers: Vec<String>,
    printer: String,
    host: String,
    port: u16,
}

struct PjlDlg {
    text: String,
    original: String,
}

struct Batch {
    dir: Option<PathBuf>,
    recursive: bool,
    rows: Vec<JobSummary>,
    total: usize,
    rx: Option<Receiver<(usize, JobSummary)>>,
    cancel: Arc<AtomicBool>,
    sort: usize,
    sel: Option<usize>,
}

struct WatchEntry {
    path: PathBuf,
    size: u64,
    stable: u32,
    first_seen: String,
    kept: Option<PathBuf>,
    gone: bool,
    note: String,
}

struct Watch {
    dir: PathBuf,
    keep_dir: Option<PathBuf>,
    keep: bool,
    running: bool,
    entries: Vec<WatchEntry>,
    last_scan: Option<Instant>,
    error: Option<String>,
}

pub struct App {
    s: Settings,
    doc: Option<Arc<Document>>,
    path: Option<PathBuf>,
    loading: Option<(PathBuf, LoadRx)>,
    error: Option<String>,
    status: Option<(String, Instant)>,
    task_rx: Option<Receiver<Result<String, String>>>,
    generation: u64,
    page: usize,
    zoom: f32,
    fit: Fit,
    layers: u8,
    tex: Option<(RenderKey, egui::TextureHandle)>,
    pending: Option<(RenderKey, Receiver<Option<render::Rendered>>)>,
    thumbs: Vec<Option<egui::TextureHandle>>,
    thumb_rx: Option<Receiver<(u64, usize, render::Rendered)>>,
    tab: Tab,
    filter: String,
    cmd_filter: String,
    cmd_filtered: Option<(String, Vec<usize>)>,
    scroll_to_top: bool,
    // view tools
    bounds: Option<((u64, usize), Vec<Option<[f32; 4]>>)>,
    selected: Option<(usize, usize)>,
    highlight: Vec<(usize, [f32; 4])>,
    scroll_to_highlight: bool,
    measure_mode: bool,
    measure: Option<((f32, f32), (f32, f32))>,
    hover_pt: Option<(f32, f32)>,
    search: String,
    search_done: String,
    search_hits: Vec<(usize, [f32; 4])>,
    search_idx: usize,
    focus_search: bool,
    // commands / hex
    cmd_sel: Option<usize>,
    cmd_scroll: Option<usize>,
    hex_goto: String,
    hex_scroll: Option<usize>,
    hex_sel: Option<(usize, usize)>,
    // compare and tool windows
    cmp: Compare,
    print_dlg: Option<PrintDlg>,
    send_dlg: Option<SendDlg>,
    pjl_dlg: Option<PjlDlg>,
    batch: Option<Batch>,
    watch: Option<Watch>,
    about: bool,
    applied_dark: Option<bool>,
    #[cfg(debug_assertions)]
    test_script: Option<Vec<String>>,
}

fn file_name(p: &Path) -> String {
    p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default()
}

fn spawn_load(p: PathBuf, ctx: &egui::Context) -> LoadRx {
    let (tx, rx) = channel();
    let ctx2 = ctx.clone();
    std::thread::Builder::new()
        .stack_size(64 * 1024 * 1024)
        .spawn(move || {
            let r = std::panic::catch_unwind(|| loader::load_file(&p)).unwrap_or_else(|_| Err("The file could not be processed (internal error)".into()));
            let _ = tx.send(r);
            ctx2.request_repaint();
        })
        .ok();
    rx
}

pub fn info_text(d: &Document) -> String {
    let mut s = String::new();
    for sec in &d.info {
        s.push_str(&format!("== {} ==\n", sec.title));
        for (k, v) in &sec.rows {
            s.push_str(&format!("{k}: {v}\n"));
        }
        s.push('\n');
    }
    if !d.warnings.is_empty() {
        s.push_str("== Messages ==\n");
        for w in &d.warnings {
            s.push_str(&format!("{w}\n"));
        }
    }
    s
}

fn mm(pt: f32) -> f32 {
    pt / 72.0 * 25.4
}

fn colour_hex(b: &Brush) -> String {
    match b {
        Brush::Solid(c) => format!("#{:02X}{:02X}{:02X}", (c.0 * 255.0) as u8, (c.1 * 255.0) as u8, (c.2 * 255.0) as u8),
        Brush::Pattern { .. } => "Pattern".into(),
    }
}

fn texture(ctx: &egui::Context, name: &str, r: &render::Rendered) -> egui::TextureHandle {
    let img = egui::ColorImage::from_rgba_unmultiplied([r.width as usize, r.height as usize], &r.rgba);
    ctx.load_texture(name, img, egui::TextureOptions::LINEAR)
}

fn now_string() -> String {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
    loader::fmt_unix(secs)
}

/// Differences between two renders: red = only in A, blue = only in B.
fn diff_images(a: &render::Rendered, b: &render::Rendered) -> (render::Rendered, f32) {
    let w = a.width.max(b.width);
    let h = a.height.max(b.height);
    let px = |r: &render::Rendered, x: u32, y: u32| -> [u8; 3] {
        if x < r.width && y < r.height {
            let o = ((y * r.width + x) * 4) as usize;
            [r.rgba[o], r.rgba[o + 1], r.rgba[o + 2]]
        } else {
            [255, 255, 255]
        }
    };
    let mut out = vec![0u8; (w * h * 4) as usize];
    let mut changed = 0u64;
    let mut inked = 0u64;
    for y in 0..h {
        for x in 0..w {
            let pa = px(a, x, y);
            let pb = px(b, x, y);
            let la = (pa[0] as i32 + pa[1] as i32 + pa[2] as i32) / 3;
            let lb = (pb[0] as i32 + pb[1] as i32 + pb[2] as i32) / 3;
            let d = (0..3).map(|i| (pa[i] as i32 - pb[i] as i32).abs()).max().unwrap_or(0);
            let o = ((y * w + x) * 4) as usize;
            if la < 250 || lb < 250 {
                inked += 1;
            }
            let c = if d > 48 {
                changed += 1;
                if la < lb { [220, 30, 30] } else { [30, 90, 230] }
            } else {
                let g = (255 - (255 - la.min(lb)) / 4) as u8;
                [g, g, g]
            };
            out[o..o + 3].copy_from_slice(&c);
            out[o + 3] = 255;
        }
    }
    let pct = if inked == 0 { 0.0 } else { changed as f32 / inked as f32 * 100.0 };
    (render::Rendered { width: w, height: h, rgba: out }, pct)
}

impl App {
    pub fn new(cc: &eframe::CreationContext, initial: Option<PathBuf>) -> Self {
        #[cfg(target_os = "macos")]
        crate::macos::set_context(&cc.egui_ctx);
        let s = Settings::load();
        let mut app = App {
            s,
            doc: None,
            path: None,
            loading: None,
            error: None,
            status: None,
            task_rx: None,
            generation: 0,
            page: 0,
            zoom: 1.0,
            fit: Fit::Page,
            layers: render::ALL_LAYERS,
            tex: None,
            pending: None,
            thumbs: Vec::new(),
            thumb_rx: None,
            tab: Tab::View,
            filter: String::new(),
            cmd_filter: String::new(),
            cmd_filtered: None,
            scroll_to_top: false,
            bounds: None,
            selected: None,
            highlight: Vec::new(),
            scroll_to_highlight: false,
            measure_mode: false,
            measure: None,
            hover_pt: None,
            search: String::new(),
            search_done: String::new(),
            search_hits: Vec::new(),
            search_idx: 0,
            focus_search: false,
            cmd_sel: None,
            cmd_scroll: None,
            hex_goto: String::new(),
            hex_scroll: None,
            hex_sel: None,
            cmp: Compare {
                path: None,
                doc: None,
                loading: None,
                mode: CmpMode::Diff,
                generation: 0,
                key: None,
                tex: None,
                pending: None,
                page_diffs: Vec::new(),
                diffs_rx: None,
                only_diff: true,
                rows: None,
            },
            print_dlg: None,
            send_dlg: None,
            pjl_dlg: None,
            batch: None,
            watch: None,
            about: false,
            applied_dark: None,
            #[cfg(debug_assertions)]
            test_script: std::env::var("PFV_TEST").ok().map(|v| v.split(';').map(|x| x.to_string()).collect()),
        };
        if let Some(p) = initial {
            app.open(p, &cc.egui_ctx);
        }
        app
    }

    pub fn initial_size() -> [f32; 2] {
        Settings::load().window.map(|(w, h)| [w.max(640.0), h.max(400.0)]).unwrap_or([1280.0, 900.0])
    }

    fn flash(&mut self, m: impl Into<String>) {
        self.status = Some((m.into(), Instant::now()));
    }

    fn open(&mut self, p: PathBuf, ctx: &egui::Context) {
        if self.tab == Tab::Compare && self.doc.is_some() {
            self.open_compare(p, ctx);
            return;
        }
        let rx = spawn_load(p.clone(), ctx);
        self.loading = Some((p, rx));
        self.error = None;
    }

    fn open_compare(&mut self, p: PathBuf, ctx: &egui::Context) {
        let rx = spawn_load(p.clone(), ctx);
        self.cmp.loading = Some((p, rx));
    }

    fn dialog(&self) -> rfd::FileDialog {
        let mut d = rfd::FileDialog::new();
        if let Some(dir) = &self.s.last_dir {
            if dir.is_dir() {
                d = d.set_directory(dir);
            }
        }
        d
    }

    fn pick_file(&mut self, ctx: &egui::Context, compare: bool) {
        if let Some(p) = self
            .dialog()
            .set_title(if compare { "Choose a print file to compare with" } else { "Open print file" })
            .add_filter("Print files", &["prn", "pcl", "pxl", "pcl6", "xl", "ps", "eps", "epsf", "spl", "shd", "plt", "hpgl", "emf", "xps", "oxps", "bin", "out", "print", "job", "dat"])
            .add_filter("All files", &["*"])
            .pick_file()
        {
            if compare {
                self.open_compare(p, ctx);
            } else {
                let t = self.tab;
                self.tab = Tab::View;
                self.open(p, ctx);
                if t != Tab::Compare {
                    self.tab = t;
                }
            }
        }
    }

    fn poll_loading(&mut self, ctx: &egui::Context) {
        if let Some((p, rx)) = &self.loading {
            let p = p.clone();
            match rx.try_recv() {
                Ok(Ok(d)) => self.set_doc(d, p, ctx),
                Ok(Err(e)) => {
                    self.error = Some(e);
                    self.loading = None;
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {}
                Err(_) => {
                    self.error = Some("Loading failed".into());
                    self.loading = None;
                }
            }
        }
        if let Some((p, rx)) = &self.cmp.loading {
            let p = p.clone();
            match rx.try_recv() {
                Ok(Ok(d)) => {
                    self.cmp.loading = None;
                    self.cmp.doc = Some(Arc::new(d));
                    self.cmp.path = Some(p);
                    self.cmp.generation += 1;
                    self.cmp.tex = None;
                    self.cmp.key = None;
                    self.cmp.pending = None;
                    self.cmp.rows = None;
                    self.tab = Tab::Compare;
                    self.start_page_diffs(ctx);
                }
                Ok(Err(e)) => {
                    self.error = Some(e);
                    self.cmp.loading = None;
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {}
                Err(_) => self.cmp.loading = None,
            }
        }
        if let Some(rx) = &self.task_rx {
            if let Ok(r) = rx.try_recv() {
                match r {
                    Ok(m) => self.flash(m),
                    Err(e) => self.error = Some(e),
                }
                self.task_rx = None;
            }
        }
    }

    fn set_doc(&mut self, d: Document, p: PathBuf, ctx: &egui::Context) {
        self.s.add_recent(&p);
        self.s.save();
        self.path = Some(p.clone());
        self.generation += 1;
        let d = Arc::new(d);
        self.thumbs = vec![None; d.pages.len()];
        self.doc = Some(d.clone());
        self.page = 0;
        self.tex = None;
        self.pending = None;
        self.cmd_filtered = None;
        self.loading = None;
        self.selected = None;
        self.highlight.clear();
        self.bounds = None;
        self.measure = None;
        self.cmd_sel = None;
        self.hex_sel = None;
        self.search_done.clear();
        self.search_hits.clear();
        self.pjl_dlg = None;
        self.cmp.generation += 1;
        self.cmp.tex = None;
        self.cmp.key = None;
        self.cmp.rows = None;
        if self.cmp.doc.is_some() {
            self.start_page_diffs(ctx);
        }
        if self.tab != Tab::Compare {
            self.tab = if d.pages.is_empty() { Tab::Info } else { Tab::View };
        }
        ctx.send_viewport_cmd(egui::ViewportCommand::Title(format!("{} — {}", file_name(&p), APP_NAME)));
        let (tx, trx) = channel();
        let generation = self.generation;
        let ctx2 = ctx.clone();
        std::thread::spawn(move || {
            for (k, pg) in d.pages.iter().enumerate() {
                let sc = 110.0 / pg.width.max(pg.height).max(1.0);
                if let Some(r) = render::render_page(pg, sc) {
                    if tx.send((generation, k, r)).is_err() {
                        return;
                    }
                    ctx2.request_repaint();
                }
            }
        });
        self.thumb_rx = Some(trx);
    }

    fn poll_thumbs(&mut self, ctx: &egui::Context) {
        let Some(rx) = &self.thumb_rx else { return };
        while let Ok((g, k, r)) = rx.try_recv() {
            if g == self.generation && k < self.thumbs.len() {
                self.thumbs[k] = Some(texture(ctx, &format!("thumb{k}"), &r));
            }
        }
    }

    fn page_count(&self) -> usize {
        self.doc.as_ref().map(|d| d.pages.len()).unwrap_or(0)
    }

    fn go(&mut self, p: isize) {
        let n = self.page_count() as isize;
        if n == 0 {
            return;
        }
        let np = p.clamp(0, n - 1) as usize;
        if np != self.page {
            self.page = np;
            self.scroll_to_top = true;
            self.measure = None;
        }
    }

    fn stem(&self) -> String {
        self.path.as_ref().and_then(|p| p.file_stem()).map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| "job".into())
    }

    // ---------------------------------------------------------------- actions

    fn save_png(&mut self) {
        let Some(d) = self.doc.clone() else { return };
        let Some(pg) = d.pages.get(self.page) else { return };
        if let Some(out) = self.dialog().set_file_name(format!("{}-page{}.png", self.stem(), self.page + 1)).add_filter("PNG image", &["png"]).save_file() {
            match render::render_page_layers(pg, 200.0 / 72.0, self.layers).and_then(|r| render::encode_png(&r)) {
                Some(png) => match std::fs::write(&out, png) {
                    Ok(_) => self.flash(format!("Saved {}", file_name(&out))),
                    Err(e) => self.error = Some(format!("Could not save: {e}")),
                },
                None => self.error = Some("Could not render page".into()),
            }
        }
    }

    fn export_pdf(&mut self, ctx: &egui::Context) {
        let Some(d) = self.doc.clone() else { return };
        if d.pages.is_empty() {
            return;
        }
        if let Some(out) = self.dialog().set_file_name(format!("{}.pdf", self.stem())).add_filter("PDF document", &["pdf"]).save_file() {
            let (tx, rx) = channel();
            let title = self.stem();
            let ctx2 = ctx.clone();
            std::thread::spawn(move || {
                let pages: Vec<&doc::Page> = d.pages.iter().collect();
                let bytes = crate::pdf::write(&pages, &title);
                let r = std::fs::write(&out, bytes).map(|_| format!("Exported {} page(s) to {}", pages.len(), file_name(&out))).map_err(|e| format!("Could not save: {e}"));
                let _ = tx.send(r);
                ctx2.request_repaint();
            });
            self.task_rx = Some(rx);
            self.flash("Exporting PDF…");
        }
    }

    fn export_text(&mut self) {
        let Some(d) = self.doc.clone() else { return };
        if let Some(out) = self.dialog().set_file_name(format!("{}-text.txt", self.stem())).add_filter("Text", &["txt"]).save_file() {
            let mut s = String::new();
            for (k, p) in d.pages.iter().enumerate() {
                s.push_str(&format!("--- Page {} ---\n{}\n", k + 1, p.plain_text()));
            }
            match std::fs::write(&out, s) {
                Ok(_) => self.flash(format!("Saved {}", file_name(&out))),
                Err(e) => self.error = Some(format!("Could not save: {e}")),
            }
        }
    }

    fn copy_page_text(&mut self, ctx: &egui::Context) {
        if let Some(p) = self.doc.as_ref().and_then(|d| d.pages.get(self.page)) {
            let t = p.plain_text();
            if t.trim().is_empty() || t.chars().all(|c| c == '\u{FFFD}' || c.is_whitespace()) {
                self.flash("No readable text on this page (text may be drawn as graphics, or its font has no character information)");
            } else {
                ctx.copy_text(t);
                self.flash("Page text copied");
            }
        }
    }

    fn open_print(&mut self) {
        if self.page_count() == 0 {
            return;
        }
        let printers = crate::output::printers();
        let printer = if printers.contains(&self.s.printer) { self.s.printer.clone() } else { printers.first().cloned().unwrap_or_default() };
        let n = self.page_count();
        self.print_dlg = Some(PrintDlg { printers, printer, range: 0, from: 1, to: n, copies: 1, fit: false });
    }

    fn open_send(&mut self, data: Arc<Vec<u8>>, what: String) {
        let printers = crate::output::printers();
        let printer = if printers.contains(&self.s.printer) { self.s.printer.clone() } else { printers.first().cloned().unwrap_or_default() };
        self.send_dlg = Some(SendDlg { data, what, to_queue: self.s.host.is_empty() && !printers.is_empty(), printers, printer, host: self.s.host.clone(), port: self.s.port });
    }

    fn open_pjl(&mut self) {
        let Some(d) = self.doc.clone() else { return };
        let head = &d.raw[..d.pjl_end.min(d.raw.len())];
        let text = String::from_utf8_lossy(head).replace("\x1b%-12345X", "").replace("\r\n", "\n").replace('\r', "\n");
        let text: String = text.lines().map(|l| l.trim_end()).filter(|l| !l.is_empty()).collect::<Vec<_>>().join("\n");
        let text = if text.is_empty() { String::new() } else { text + "\n" };
        self.pjl_dlg = Some(PjlDlg { original: text.clone(), text });
    }

    fn pjl_bytes(d: &Document, text: &str) -> Vec<u8> {
        let lines: Vec<&str> = text.lines().map(|l| l.trim_end()).filter(|l| !l.is_empty()).collect();
        if lines.is_empty() {
            return d.raw[d.pjl_end..].to_vec();
        }
        let mut out = crate::pjl::UEL.to_vec();
        for l in &lines {
            out.extend_from_slice(l.as_bytes());
            out.extend_from_slice(b"\r\n");
        }
        out.extend_from_slice(&d.raw[d.pjl_end..]);
        if d.pjl_end == 0 && !d.raw.ends_with(crate::pjl::UEL) {
            out.extend_from_slice(crate::pjl::UEL);
            out.extend_from_slice(b"@PJL EOJ\r\n");
            out.extend_from_slice(crate::pjl::UEL);
        }
        out
    }

    // ------------------------------------------------------------ search/pick

    fn run_search(&mut self) {
        let q = self.search.trim().to_lowercase();
        self.search_done = self.search.clone();
        self.search_hits.clear();
        self.search_idx = 0;
        let Some(d) = self.doc.clone() else { return };
        if q.is_empty() {
            return;
        }
        let qc: Vec<char> = q.chars().collect();
        for (pi, p) in d.pages.iter().enumerate() {
            for run in &p.text {
                let lower: Vec<char> = run.text.iter().map(|c| c.to_lowercase().next().unwrap_or(*c)).collect();
                if lower.len() < qc.len() {
                    continue;
                }
                let mut i = 0;
                while i + qc.len() <= lower.len() {
                    if lower[i..i + qc.len()] == qc[..] {
                        let mut b = [f32::MAX, f32::MAX, f32::MIN, f32::MIN];
                        for r in &run.boxes[i..i + qc.len()] {
                            b = [b[0].min(r[0]), b[1].min(r[1]), b[2].max(r[2]), b[3].max(r[3])];
                        }
                        self.search_hits.push((pi, b));
                        i += qc.len();
                    } else {
                        i += 1;
                    }
                }
            }
            if self.search_hits.len() > 10000 {
                break;
            }
        }
        // start at the first hit on or after the current page
        self.search_idx = self.search_hits.iter().position(|(p, _)| *p >= self.page).unwrap_or(0);
        self.show_hit();
    }

    fn show_hit(&mut self) {
        if let Some(&(p, b)) = self.search_hits.get(self.search_idx) {
            self.go(p as isize);
            self.highlight = vec![(p, b)];
            self.scroll_to_highlight = true;
            self.tab = Tab::View;
        }
    }

    fn next_hit(&mut self, back: bool) {
        if self.search != self.search_done {
            self.run_search();
            return;
        }
        let n = self.search_hits.len();
        if n == 0 {
            return;
        }
        self.search_idx = if back { (self.search_idx + n - 1) % n } else { (self.search_idx + 1) % n };
        self.show_hit();
    }

    fn page_bounds(&mut self) -> Vec<Option<[f32; 4]>> {
        let key = (self.generation, self.page);
        if let Some((k, b)) = &self.bounds {
            if *k == key {
                return b.clone();
            }
        }
        let b: Vec<Option<[f32; 4]>> = self.doc.as_ref().and_then(|d| d.pages.get(self.page)).map(|p| p.items.iter().map(render::item_bounds).collect()).unwrap_or_default();
        self.bounds = Some((key, b.clone()));
        b
    }

    fn pick(&mut self, pt: (f32, f32)) {
        let b = self.page_bounds();
        let Some(d) = self.doc.clone() else { return };
        let Some(p) = d.pages.get(self.page) else { return };
        let mut best: Option<(usize, f32)> = None;
        for (i, bb) in b.iter().enumerate() {
            let Some(bb) = bb else { continue };
            if self.layers & p.meta_of(i).kind as u8 == 0 {
                continue;
            }
            let pad = 1.5;
            if pt.0 >= bb[0] - pad && pt.0 <= bb[2] + pad && pt.1 >= bb[1] - pad && pt.1 <= bb[3] + pad {
                let area = (bb[2] - bb[0]).max(0.5) * (bb[3] - bb[1]).max(0.5);
                // smallest wins; among equal sizes the topmost (later) one
                if best.map(|(_, a)| area <= a).unwrap_or(true) {
                    best = Some((i, area));
                }
            }
        }
        match best {
            Some((i, _)) => {
                self.selected = Some((self.page, i));
                self.highlight = b[i].map(|bb| vec![(self.page, bb)]).unwrap_or_default();
                let src = p.meta_of(i).src;
                if let Some(ci) = d.cmd_index_for(src) {
                    self.cmd_sel = Some(ci);
                    self.cmd_scroll = Some(ci);
                }
            }
            None => {
                self.selected = None;
                self.highlight.clear();
            }
        }
    }

    /// Select the command at `ci`: highlight what it drew, and its bytes.
    fn select_command(&mut self, ci: usize) {
        self.cmd_sel = Some(ci);
        let Some(d) = self.doc.clone() else { return };
        let off = d.cmd_offsets.get(ci).copied().unwrap_or(NO_SRC);
        if off != NO_SRC {
            let end = d.cmd_offsets.iter().skip(ci + 1).find(|&&o| o != NO_SRC && o > off).copied().unwrap_or(off + 1) as usize;
            self.hex_sel = Some((off as usize, end.min(d.raw.len()).max(off as usize + 1)));
        }
    }

    fn show_command_on_page(&mut self, ci: usize) -> bool {
        let Some(d) = self.doc.clone() else { return false };
        let Some(&off) = d.cmd_offsets.get(ci) else { return false };
        if off == NO_SRC {
            return false;
        }
        for (pi, p) in d.pages.iter().enumerate() {
            let hits: Vec<usize> = (0..p.items.len()).filter(|&i| p.meta_of(i).src == off).collect();
            if hits.is_empty() {
                continue;
            }
            self.go(pi as isize);
            self.page = pi;
            let b = self.page_bounds();
            self.highlight = hits.iter().filter_map(|&i| b[i].map(|bb| (pi, bb))).collect();
            self.selected = Some((pi, hits[0]));
            self.scroll_to_highlight = true;
            self.tab = Tab::View;
            return true;
        }
        false
    }

    fn show_bytes(&mut self, ci: usize) {
        self.select_command(ci);
        if let Some((a, _)) = self.hex_sel {
            self.hex_scroll = Some(a / 16);
        }
        self.tab = Tab::Hex;
    }

    // ---------------------------------------------------------------- menus

    fn menu_bar(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let has = self.doc.is_some();
        let pages = self.page_count() > 0;
        egui::menu::bar(ui, |ui| {
            ui.menu_button("File", |ui| {
                if ui.button("Open…            Ctrl+O").clicked() {
                    ui.close_menu();
                    self.pick_file(ctx, false);
                }
                ui.menu_button("Open recent", |ui| {
                    if self.s.recent.is_empty() {
                        ui.weak("No recent files");
                    }
                    for r in self.s.recent.clone() {
                        if ui.button(r.display().to_string()).clicked() {
                            ui.close_menu();
                            let t = self.tab;
                            self.tab = Tab::View;
                            self.open(r, ctx);
                            self.tab = if t == Tab::Compare { Tab::View } else { t };
                        }
                    }
                    if !self.s.recent.is_empty() {
                        ui.separator();
                        if ui.button("Clear list").clicked() {
                            self.s.recent.clear();
                            self.s.save();
                            ui.close_menu();
                        }
                    }
                });
                ui.add_enabled_ui(has, |ui| {
                    if ui.button("Compare with another file…").clicked() {
                        ui.close_menu();
                        self.tab = Tab::Compare;
                        self.pick_file(ctx, true);
                    }
                });
                ui.separator();
                ui.add_enabled_ui(pages, |ui| {
                    if ui.button("Export as PDF…   Ctrl+E").clicked() {
                        ui.close_menu();
                        self.export_pdf(ctx);
                    }
                    if ui.button("Save page as PNG…").clicked() {
                        ui.close_menu();
                        self.save_png();
                    }
                    if ui.button("Export text…").clicked() {
                        ui.close_menu();
                        self.export_text();
                    }
                    if ui.button("Copy page text").clicked() {
                        ui.close_menu();
                        self.copy_page_text(ctx);
                    }
                });
                ui.separator();
                ui.add_enabled_ui(pages, |ui| {
                    if ui.button("Print…            Ctrl+P").clicked() {
                        ui.close_menu();
                        self.open_print();
                    }
                });
                ui.add_enabled_ui(has, |ui| {
                    if ui.button("Send job to a printer…").on_hover_text("Send the original job, unchanged, to a printer queue or a network printer (port 9100)").clicked() {
                        ui.close_menu();
                        if let Some(d) = &self.doc {
                            let data = d.raw.clone();
                            self.open_send(data, "the original job".into());
                        }
                    }
                    if ui.button("Edit PJL header…").clicked() {
                        ui.close_menu();
                        self.open_pjl();
                    }
                });
                ui.separator();
                if ui.button("Quit").clicked() {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            });
            ui.menu_button("View", |ui| {
                if ui.checkbox(&mut self.s.dark, "Dark mode").changed() {
                    self.s.save();
                }
                if ui.checkbox(&mut self.s.thumbs, "Thumbnails").changed() {
                    self.s.save();
                }
                if ui.checkbox(&mut self.s.rulers, "Rulers").changed() {
                    self.s.save();
                }
                if ui.checkbox(&mut self.s.margins, "Printable area (4.2 mm margin)").changed() {
                    self.s.save();
                }
                ui.separator();
                ui.label("Show on the page:");
                for (k, name) in [(Kind::Text, "Text"), (Kind::Vector, "Graphics (lines and shapes)"), (Kind::Image, "Images")] {
                    let mut on = self.layers & k as u8 != 0;
                    if ui.checkbox(&mut on, name).changed() {
                        if on {
                            self.layers |= k as u8;
                        } else {
                            self.layers &= !(k as u8);
                        }
                    }
                }
                ui.separator();
                if ui.button("Fit page").clicked() {
                    self.fit = Fit::Page;
                    ui.close_menu();
                }
                if ui.button("Fit width").clicked() {
                    self.fit = Fit::Width;
                    ui.close_menu();
                }
                if ui.button("Actual size (100%)").clicked() {
                    self.zoom = 1.0;
                    self.fit = Fit::Custom;
                    ui.close_menu();
                }
            });
            ui.menu_button("Tools", |ui| {
                if ui.button("Batch report for a folder…").clicked() {
                    ui.close_menu();
                    if self.batch.is_none() {
                        self.batch = Some(Batch { dir: self.s.batch_dir.clone(), recursive: false, rows: Vec::new(), total: 0, rx: None, cancel: Arc::new(AtomicBool::new(false)), sort: 0, sel: None });
                    }
                }
                if ui.button("Watch spool folder…").clicked() {
                    ui.close_menu();
                    if self.watch.is_none() {
                        let dir = self.s.watch_dir.clone().unwrap_or_else(default_spool_dir);
                        self.watch = Some(Watch { dir, keep_dir: self.s.keep_dir.clone(), keep: self.s.keep_dir.is_some(), running: false, entries: Vec::new(), last_scan: None, error: None });
                    }
                }
            });
            ui.menu_button("Help", |ui| {
                if ui.button("About and keyboard shortcuts").clicked() {
                    self.about = true;
                    ui.close_menu();
                }
            });
        });
    }

    fn toolbar(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        ui.horizontal_wrapped(|ui| {
            if ui.button("📂 Open…").on_hover_text("Open a print file (Ctrl+O)").clicked() {
                self.pick_file(ctx, false);
            }
            ui.separator();
            ui.selectable_value(&mut self.tab, Tab::View, "🖼 View");
            ui.selectable_value(&mut self.tab, Tab::Info, "ℹ Info");
            ui.selectable_value(&mut self.tab, Tab::Commands, "☰ Commands");
            ui.selectable_value(&mut self.tab, Tab::Hex, "# Hex");
            ui.selectable_value(&mut self.tab, Tab::Compare, "↔ Compare");
            let wn = self.doc.as_ref().map(|d| d.warnings.len()).unwrap_or(0);
            ui.selectable_value(&mut self.tab, Tab::Warnings, if wn > 0 { format!("⚠ Messages ({wn})") } else { "⚠ Messages".to_string() });
            ui.separator();
            let n = self.page_count();
            ui.add_enabled_ui(n > 0, |ui| {
                if ui.button("⏮").on_hover_text("First page (Home)").clicked() {
                    self.go(0);
                }
                if ui.button("◀").on_hover_text("Previous page (PgUp)").clicked() {
                    self.go(self.page as isize - 1);
                }
                let mut pn = self.page + 1;
                ui.add(egui::DragValue::new(&mut pn).range(1..=n.max(1)).speed(0.1));
                ui.label(format!("/ {n}"));
                if pn != self.page + 1 {
                    self.go(pn as isize - 1);
                }
                if ui.button("▶").on_hover_text("Next page (PgDn)").clicked() {
                    self.go(self.page as isize + 1);
                }
                if ui.button("⏭").on_hover_text("Last page (End)").clicked() {
                    self.go(n as isize - 1);
                }
                ui.separator();
                if ui.button("−").on_hover_text("Zoom out (Ctrl −)").clicked() {
                    self.zoom = (self.zoom / 1.25).max(0.05);
                    self.fit = Fit::Custom;
                }
                ui.label(format!("{:.0}%", self.zoom * 100.0));
                if ui.button("+").on_hover_text("Zoom in (Ctrl +)").clicked() {
                    self.zoom = (self.zoom * 1.25).min(16.0);
                    self.fit = Fit::Custom;
                }
                ui.selectable_value(&mut self.fit, Fit::Width, "Fit width");
                ui.selectable_value(&mut self.fit, Fit::Page, "Fit page");
                ui.separator();
                ui.toggle_value(&mut self.measure_mode, "📏 Measure").on_hover_text("Drag on the page to measure a distance (M)");
                ui.separator();
                let r = ui.add(egui::TextEdit::singleline(&mut self.search).hint_text("🔍 Find text (Ctrl+F)").desired_width(170.0));
                if self.focus_search {
                    r.request_focus();
                    self.focus_search = false;
                }
                if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    let back = ui.input(|i| i.modifiers.shift);
                    self.next_hit(back);
                    r.request_focus();
                }
                if !self.search_done.is_empty() && self.search == self.search_done {
                    if self.search_hits.is_empty() {
                        ui.weak("not found");
                    } else {
                        ui.label(format!("{} of {}", self.search_idx + 1, self.search_hits.len()));
                        if ui.small_button("⏶").on_hover_text("Previous (Shift+Enter)").clicked() {
                            self.next_hit(true);
                        }
                        if ui.small_button("⏷").on_hover_text("Next (Enter / F3)").clicked() {
                            self.next_hit(false);
                        }
                    }
                }
            });
        });
    }

    fn handle_keys(&mut self, ctx: &egui::Context) {
        let typing = ctx.wants_keyboard_input();
        let mut acts: Vec<&str> = Vec::new();
        ctx.input(|i| {
            let cmd = i.modifiers.command;
            if cmd && i.key_pressed(egui::Key::O) {
                acts.push("open");
            }
            if cmd && i.key_pressed(egui::Key::P) {
                acts.push("print");
            }
            if cmd && i.key_pressed(egui::Key::E) {
                acts.push("pdf");
            }
            if cmd && i.key_pressed(egui::Key::F) {
                acts.push("find");
            }
            if i.key_pressed(egui::Key::F3) {
                acts.push(if i.modifiers.shift { "prevhit" } else { "nexthit" });
            }
            if !typing {
                let view = matches!(self.tab, Tab::View | Tab::Compare);
                if i.key_pressed(egui::Key::PageDown) || (i.key_pressed(egui::Key::ArrowRight) && view) {
                    acts.push("next");
                }
                if i.key_pressed(egui::Key::PageUp) || (i.key_pressed(egui::Key::ArrowLeft) && view) {
                    acts.push("prev");
                }
                if i.key_pressed(egui::Key::Home) && view {
                    acts.push("first");
                }
                if i.key_pressed(egui::Key::End) && view {
                    acts.push("last");
                }
                if i.key_pressed(egui::Key::M) && !cmd {
                    acts.push("measure");
                }
                if i.key_pressed(egui::Key::Escape) {
                    acts.push("esc");
                }
            }
            if cmd && (i.key_pressed(egui::Key::Plus) || i.key_pressed(egui::Key::Equals)) {
                acts.push("zin");
            }
            if cmd && i.key_pressed(egui::Key::Minus) {
                acts.push("zout");
            }
        });
        for a in acts {
            match a {
                "open" => self.pick_file(ctx, false),
                "print" => self.open_print(),
                "pdf" => self.export_pdf(ctx),
                "find" => {
                    self.focus_search = true;
                    if self.tab != Tab::View {
                        self.tab = Tab::View;
                    }
                }
                "nexthit" => self.next_hit(false),
                "prevhit" => self.next_hit(true),
                "next" => self.go(self.page as isize + 1),
                "prev" => self.go(self.page as isize - 1),
                "first" => self.go(0),
                "last" => self.go(self.page_count() as isize - 1),
                "measure" => self.measure_mode = !self.measure_mode,
                "esc" => {
                    self.selected = None;
                    self.highlight.clear();
                    self.measure = None;
                }
                "zin" => {
                    self.zoom = (self.zoom * 1.25).min(16.0);
                    self.fit = Fit::Custom;
                }
                "zout" => {
                    self.zoom = (self.zoom / 1.25).max(0.05);
                    self.fit = Fit::Custom;
                }
                _ => {}
            }
        }
        #[allow(unused_mut)]
        let mut dropped: Vec<PathBuf> = ctx.input(|i| i.raw.dropped_files.iter().filter_map(|f| f.path.clone()).collect());
        #[cfg(target_os = "macos")]
        dropped.extend(crate::macos::take_opened());
        if let Some(p) = dropped.into_iter().next() {
            self.open(p, ctx);
        }
    }

    // ---------------------------------------------------------------- view

    fn request_render(&mut self, ctx: &egui::Context, d: &Arc<Document>, scale: f32) -> RenderKey {
        let key: RenderKey = (self.generation, self.page, (scale * 1000.0) as u32, self.layers);
        if let Some((k, rx)) = &self.pending {
            if let Ok(r) = rx.try_recv() {
                if let Some(r) = r {
                    self.tex = Some((*k, texture(ctx, "page", &r)));
                }
                self.pending = None;
            }
        }
        let have = self.tex.as_ref().map(|(k, _)| *k == key).unwrap_or(false);
        let pending_ok = self.pending.as_ref().map(|(k, _)| *k == key).unwrap_or(false);
        if !have && !pending_ok {
            let (tx, rx) = channel();
            let d2 = d.clone();
            let pi = self.page;
            let layers = self.layers;
            let ctx2 = ctx.clone();
            std::thread::spawn(move || {
                let r = render::render_page_layers(&d2.pages[pi], scale, layers);
                let _ = tx.send(r);
                ctx2.request_repaint();
            });
            self.pending = Some((key, rx));
        }
        key
    }

    fn view_tab(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let Some(d) = self.doc.clone() else {
            self.empty_state(ui, ctx);
            return;
        };
        if d.pages.is_empty() {
            ui.centered_and_justified(|ui| {
                ui.label("No pages could be rendered from this file. See the Info and Messages tabs for details.");
            });
            return;
        }
        self.page = self.page.min(d.pages.len() - 1);
        let page = &d.pages[self.page];
        if self.selected.is_some() {
            self.selection_panel(ui, &d);
        }
        let avail = ui.available_size();
        let base = 96.0 / 72.0;
        let margin = if self.s.rulers { 44.0 } else { 20.0 };
        match self.fit {
            Fit::Width => self.zoom = ((avail.x - 2.0 * margin) / (page.width * base)).max(0.05),
            Fit::Page => self.zoom = ((avail.x - 2.0 * margin) / (page.width * base)).min((avail.y - 2.0 * margin) / (page.height * base)).max(0.05),
            Fit::Custom => {}
        }
        let zd = ui.input(|i| i.zoom_delta());
        if zd != 1.0 && ui.rect_contains_pointer(ui.max_rect()) {
            self.zoom = (self.zoom * zd).clamp(0.05, 16.0);
            self.fit = Fit::Custom;
        }
        let pt_to_px = base * self.zoom;
        let disp = egui::vec2(page.width * pt_to_px, page.height * pt_to_px);
        let ppp = ctx.pixels_per_point();
        let mut scale = pt_to_px * ppp;
        let maxdim = page.width.max(page.height) * scale;
        if maxdim > 8000.0 {
            scale *= 8000.0 / maxdim;
        }
        let key = self.request_render(ctx, &d, scale);
        let have = self.tex.as_ref().map(|(k, _)| *k == key).unwrap_or(false);
        let mut sa = egui::ScrollArea::both().auto_shrink([false, false]);
        if self.scroll_to_top {
            sa = sa.vertical_scroll_offset(0.0);
            self.scroll_to_top = false;
        }
        let dark = self.s.dark;
        let mut clicked_pt: Option<(f32, f32)> = None;
        sa.show(ui, |ui| {
            let w = ui.available_width().max(disp.x + 2.0 * margin);
            ui.allocate_ui_with_layout(egui::vec2(w, disp.y + 2.0 * margin), egui::Layout::top_down(egui::Align::Center), |ui| {
                ui.add_space(margin);
                let sense = egui::Sense::click_and_drag();
                let (rect, resp) = ui.allocate_exact_size(disp, sense);
                let painter = ui.painter_at(rect.expand(margin));
                let to_screen = |x: f32, y: f32| Pos2::new(rect.min.x + x * pt_to_px, rect.min.y + y * pt_to_px);
                painter.rect_filled(rect.translate(egui::vec2(3.0, 3.0)), 0.0, Color32::from_black_alpha(if dark { 120 } else { 60 }));
                painter.rect_filled(rect, 0.0, Color32::WHITE);
                let same_page = self.tex.as_ref().map(|((g, p, _, l), _)| *g == self.generation && *p == self.page && *l == self.layers).unwrap_or(false);
                if let Some((_, t)) = &self.tex {
                    if same_page {
                        painter.image(t.id(), rect, Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), Color32::WHITE);
                    }
                }
                if !same_page || !have && self.tex.is_none() {
                    ui.put(rect, egui::Spinner::new().size(32.0));
                }
                if self.s.margins {
                    let m = crate::analyze::MARGIN_PT;
                    let r = Rect::from_min_max(to_screen(m, m), to_screen(page.width - m, page.height - m));
                    painter.rect_stroke(r, 0.0, Stroke::new(1.0_f32, Color32::from_rgba_unmultiplied(0, 150, 255, 180)));
                    if let Some(c) = d.coverage.get(self.page).and_then(|c| c.ink_box) {
                        let ib = Rect::from_min_max(to_screen(c[0], c[1]), to_screen(c[2], c[3]));
                        painter.rect_stroke(ib, 0.0, Stroke::new(1.0_f32, Color32::from_rgba_unmultiplied(255, 120, 0, 160)));
                    }
                }
                // search hits on this page (all), current hit stronger
                let cur_hit = self.search_hits.get(self.search_idx).copied();
                if self.search == self.search_done && !self.search_done.is_empty() {
                    for (p, b) in &self.search_hits {
                        if *p == self.page {
                            let r = Rect::from_min_max(to_screen(b[0], b[1]), to_screen(b[2], b[3]));
                            painter.rect_filled(r, 1.0, Color32::from_rgba_unmultiplied(255, 220, 0, 70));
                        }
                    }
                }
                for (p, b) in &self.highlight {
                    if *p != self.page {
                        continue;
                    }
                    let r = Rect::from_min_max(to_screen(b[0], b[1]), to_screen(b[2], b[3])).expand(2.0);
                    let is_hit = cur_hit.map(|(hp, hb)| hp == *p && hb == *b).unwrap_or(false);
                    let col = if is_hit { Color32::from_rgb(255, 140, 0) } else { Color32::from_rgb(230, 0, 120) };
                    painter.rect_filled(r, 2.0, col.gamma_multiply(0.12));
                    painter.rect_stroke(r, 2.0, Stroke::new(2.0_f32, col));
                    if self.scroll_to_highlight {
                        ui.scroll_to_rect(r.expand(60.0), Some(egui::Align::Center));
                    }
                }
                self.scroll_to_highlight = false;
                if self.s.rulers {
                    draw_rulers(&painter, rect, page.width, page.height, pt_to_px, dark, self.hover_pt);
                }
                // pointer
                let to_pt = |p: Pos2| ((p.x - rect.min.x) / pt_to_px, (p.y - rect.min.y) / pt_to_px);
                self.hover_pt = resp.hover_pos().map(to_pt);
                if self.measure_mode {
                    if resp.drag_started() {
                        if let Some(p) = resp.interact_pointer_pos() {
                            let q = to_pt(p);
                            self.measure = Some((q, q));
                        }
                    }
                    if resp.dragged() {
                        if let (Some(p), Some(m)) = (resp.interact_pointer_pos(), self.measure.as_mut()) {
                            let mut e = to_pt(p);
                            if ui.input(|i| i.modifiers.shift) {
                                // constrain to horizontal / vertical
                                if (e.0 - m.0 .0).abs() > (e.1 - m.0 .1).abs() {
                                    e.1 = m.0 .1;
                                } else {
                                    e.0 = m.0 .0;
                                }
                            }
                            m.1 = e;
                        }
                    }
                } else if resp.clicked() {
                    if let Some(p) = resp.interact_pointer_pos() {
                        clicked_pt = Some(to_pt(p));
                    }
                }
                if let Some((pa, pb)) = self.measure {
                    let a = to_screen(pa.0, pa.1);
                    let b = to_screen(pb.0, pb.1);
                    painter.line_segment([a, b], Stroke::new(2.0_f32, Color32::from_rgb(0, 160, 80)));
                    painter.circle_filled(a, 3.0, Color32::from_rgb(0, 160, 80));
                    painter.circle_filled(b, 3.0, Color32::from_rgb(0, 160, 80));
                    let dx = pb.0 - pa.0;
                    let dy = pb.1 - pa.1;
                    let dist = (dx * dx + dy * dy).sqrt();
                    let label = format!("{:.1} mm  ({:.2} in, {:.1} pt)\nΔx {:.1} mm  Δy {:.1} mm", mm(dist), dist / 72.0, dist, mm(dx.abs()), mm(dy.abs()));
                    let pos = b + egui::vec2(10.0, 6.0);
                    let galley = painter.layout_no_wrap(label, egui::FontId::proportional(13.0), Color32::WHITE);
                    let bg = Rect::from_min_size(pos, galley.size()).expand(4.0);
                    painter.rect_filled(bg, 3.0, Color32::from_rgba_unmultiplied(0, 110, 60, 230));
                    painter.galley(pos, galley, Color32::WHITE);
                }
                if self.measure_mode {
                    ctx.set_cursor_icon(egui::CursorIcon::Crosshair);
                }
                ui.add_space(margin);
            });
        });
        if let Some(pt) = clicked_pt {
            self.pick(pt);
        }
    }

    fn selection_panel(&mut self, ui: &mut egui::Ui, d: &Arc<Document>) {
        let Some((pi, ii)) = self.selected else { return };
        let Some(p) = d.pages.get(pi) else { return };
        let Some(item) = p.items.get(ii) else { return };
        let meta = p.meta_of(ii);
        let bb = render::item_bounds(item);
        let mut show_cmd: Option<usize> = None;
        let mut show_bytes: Option<usize> = None;
        let mut clear = false;
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                let (kind, brush) = match item {
                    Item::Fill { brush, .. } => (if meta.kind == Kind::Text { "Text" } else { "Filled shape" }, Some(brush)),
                    Item::Stroke { brush, .. } => (if meta.kind == Kind::Text { "Text (outline)" } else { "Line" }, Some(brush)),
                    Item::Image { image, .. } => {
                        ui.label(RichText::new(if meta.kind == Kind::Text { "Text (bitmap glyphs)" } else { "Image" }).strong());
                        ui.label(format!("{} × {} px", image.width, image.height));
                        ("", None)
                    }
                };
                if !kind.is_empty() {
                    ui.label(RichText::new(kind).strong());
                }
                if let Some(b) = brush {
                    if let Brush::Solid(c) = b {
                        let (r, _) = ui.allocate_exact_size(egui::vec2(14.0, 14.0), egui::Sense::hover());
                        ui.painter().rect_filled(r, 2.0, Color32::from_rgb((c.0 * 255.0) as u8, (c.1 * 255.0) as u8, (c.2 * 255.0) as u8));
                        ui.painter().rect_stroke(r, 2.0, Stroke::new(1.0_f32, Color32::GRAY));
                    }
                    ui.label(colour_hex(b));
                }
                if let Item::Stroke { style, .. } = item {
                    ui.label(format!("width {:.2} pt", style.width));
                }
                if let Some(b) = bb {
                    ui.label(format!("at {:.1}, {:.1} mm · {:.1} × {:.1} mm", mm(b[0]), mm(b[1]), mm(b[2] - b[0]), mm(b[3] - b[1])));
                }
                if meta.kind == Kind::Text {
                    if let Some(b) = bb {
                        let t: String = p
                            .text
                            .iter()
                            .flat_map(|r| r.text.iter().zip(r.boxes.iter()))
                            .filter(|(_, cb)| cb[0] >= b[0] - 2.0 && cb[2] <= b[2] + 2.0 && cb[1] >= b[1] - 4.0 && cb[3] <= b[3] + 4.0)
                            .map(|(c, _)| *c)
                            .take(120)
                            .collect();
                        if !t.trim().is_empty() {
                            ui.label(format!("“{}”", t.trim()));
                        }
                    }
                }
                if meta.src != NO_SRC {
                    ui.label(format!("offset {:#X}", meta.src));
                    if let Some(ci) = d.cmd_index_for(meta.src) {
                        if ui.button("☰ Show command").clicked() {
                            show_cmd = Some(ci);
                        }
                        if ui.button("# Show bytes").clicked() {
                            show_bytes = Some(ci);
                        }
                    }
                }
                if ui.button("✖").on_hover_text("Clear selection (Esc)").clicked() {
                    clear = true;
                }
            });
            if meta.src != NO_SRC {
                if let Some(ci) = d.cmd_index_for(meta.src) {
                    if let Some(c) = d.commands.get(ci) {
                        let short: String = c.chars().take(220).collect();
                        ui.add(egui::Label::new(RichText::new(short).monospace().small()).truncate());
                    }
                }
            }
        });
        if let Some(ci) = show_cmd {
            self.select_command(ci);
            self.cmd_scroll = Some(ci);
            self.tab = Tab::Commands;
        }
        if let Some(ci) = show_bytes {
            self.show_bytes(ci);
        }
        if clear {
            self.selected = None;
            self.highlight.clear();
        }
    }

    fn empty_state(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        ui.vertical_centered(|ui| {
            ui.add_space(ui.available_height() * 0.28);
            ui.heading(APP_NAME);
            ui.add_space(8.0);
            ui.label("View PCL 6 / PCL XL, PCL 5, HP-GL/2, PostScript, EMF and XPS print files, and inspect everything about the job.");
            ui.add_space(16.0);
            if ui.button(RichText::new("📂  Open a print file…").size(18.0)).clicked() {
                self.pick_file(ctx, false);
            }
            ui.add_space(8.0);
            ui.weak("…or drag and drop a file onto this window.");
            if !self.s.recent.is_empty() {
                ui.add_space(24.0);
                ui.label(RichText::new("Recent files").strong());
                for r in self.s.recent.clone().into_iter().take(8) {
                    if ui.link(r.display().to_string()).clicked() {
                        self.open(r, ctx);
                    }
                }
            }
        });
    }

    // ---------------------------------------------------------------- info

    fn info_tab(&mut self, ui: &mut egui::Ui) {
        let Some(d) = self.doc.clone() else {
            ui.label("Open a file to see its job information.");
            return;
        };
        ui.horizontal(|ui| {
            ui.label("🔍");
            ui.add(egui::TextEdit::singleline(&mut self.filter).hint_text("Filter information…").desired_width(260.0));
            if ui.button("📋 Copy all").clicked() {
                ui.ctx().copy_text(info_text(&d));
            }
            if ui.button("💾 Export…").clicked() {
                if let Some(out) = self.dialog().set_file_name(format!("{}-info.txt", self.stem())).add_filter("Text", &["txt"]).save_file() {
                    let _ = std::fs::write(out, info_text(&d));
                }
            }
        });
        ui.separator();
        let f = self.filter.to_lowercase();
        let dark = self.s.dark;
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            for (si, sec) in d.info.iter().enumerate() {
                let rows: Vec<&(String, String)> = sec.rows.iter().filter(|(k, v)| f.is_empty() || k.to_lowercase().contains(&f) || v.to_lowercase().contains(&f) || sec.title.to_lowercase().contains(&f)).collect();
                if rows.is_empty() {
                    continue;
                }
                let big = sec.rows.len() > 60;
                let checks = sec.title == "Settings checks";
                egui::CollapsingHeader::new(RichText::new(format!("{}  ({})", sec.title, rows.len())).strong())
                    .id_salt(si)
                    .default_open(!big || !f.is_empty())
                    .show(ui, |ui| {
                        egui::Grid::new(format!("g{si}")).num_columns(2).striped(true).spacing([16.0, 4.0]).min_col_width(180.0).show(ui, |ui| {
                            for (k, v) in rows.iter().take(5000) {
                                let mut kt = RichText::new(k.as_str()).strong();
                                if checks {
                                    kt = match k.as_str() {
                                        "Problem" => kt.color(Color32::from_rgb(200, 30, 30)),
                                        "Check" => kt.color(if dark { Color32::from_rgb(255, 180, 60) } else { Color32::from_rgb(190, 110, 0) }),
                                        "Result" => kt.color(Color32::from_rgb(30, 150, 60)),
                                        _ => kt,
                                    };
                                }
                                ui.label(kt);
                                ui.add(egui::Label::new(v.as_str()).wrap().selectable(true));
                                ui.end_row();
                            }
                        });
                    });
                ui.add_space(4.0);
            }
        });
    }

    // ---------------------------------------------------------------- commands

    fn commands_tab(&mut self, ui: &mut egui::Ui) {
        let Some(d) = self.doc.clone() else {
            ui.label("Open a file to see its command stream.");
            return;
        };
        if d.commands.is_empty() {
            ui.label("No decoded command listing is available for this file. See the Info tab for the job's settings.");
            return;
        }
        let mut on_page: Option<usize> = None;
        let mut bytes: Option<usize> = None;
        ui.horizontal(|ui| {
            ui.label("🔍");
            ui.add(egui::TextEdit::singleline(&mut self.cmd_filter).hint_text("Filter commands (e.g. BeginPage, Text, ReadImage, show)…").desired_width(360.0));
            ui.label(format!("{} commands", d.commands.len()));
            if ui.button("💾 Export…").clicked() {
                if let Some(out) = self.dialog().set_file_name(format!("{}-commands.txt", self.stem())).add_filter("Text", &["txt"]).save_file() {
                    let _ = std::fs::write(out, d.commands.join("\n"));
                }
            }
            if let Some(ci) = self.cmd_sel {
                ui.separator();
                if ui.button("🖼 Show on page").on_hover_text("Highlight what this command drew").clicked() {
                    on_page = Some(ci);
                }
                if ui.button("# Show bytes").clicked() {
                    bytes = Some(ci);
                }
            }
        });
        ui.separator();
        let f = self.cmd_filter.to_lowercase();
        let idx: Vec<usize> = if f.is_empty() {
            Vec::new()
        } else {
            match &self.cmd_filtered {
                Some((ff, v)) if *ff == f => v.clone(),
                _ => {
                    let v: Vec<usize> = d.commands.iter().enumerate().filter(|(_, c)| c.to_lowercase().contains(&f)).map(|(i, _)| i).collect();
                    self.cmd_filtered = Some((f.clone(), v.clone()));
                    v
                }
            }
        };
        let n = if f.is_empty() { d.commands.len() } else { idx.len() };
        let row_h = ui.text_style_height(&egui::TextStyle::Monospace) + ui.spacing().item_spacing.y;
        let mut sa = egui::ScrollArea::both().auto_shrink([false, false]);
        if let Some(target) = self.cmd_scroll.take() {
            let row = if f.is_empty() { Some(target) } else { idx.iter().position(|&i| i == target) };
            if let Some(r) = row {
                sa = sa.vertical_scroll_offset((r as f32 * row_h - ui.available_height() / 3.0).max(0.0));
            }
        }
        let mut clicked: Option<usize> = None;
        let mut dbl: Option<usize> = None;
        sa.show_rows(ui, row_h - ui.spacing().item_spacing.y, n, |ui, range| {
            for r in range {
                let i = if f.is_empty() { r } else { idx[r] };
                let sel = self.cmd_sel == Some(i);
                let resp = ui.add(egui::SelectableLabel::new(sel, RichText::new(&d.commands[i]).monospace()));
                if resp.clicked() {
                    clicked = Some(i);
                }
                if resp.double_clicked() {
                    dbl = Some(i);
                }
            }
        });
        if let Some(i) = clicked {
            self.select_command(i);
        }
        if let Some(i) = dbl.or(on_page) {
            if !self.show_command_on_page(i) {
                self.flash("This command didn't draw anything visible");
            }
        }
        if let Some(i) = bytes {
            self.show_bytes(i);
        }
    }

    // ---------------------------------------------------------------- hex

    fn hex_tab(&mut self, ui: &mut egui::Ui) {
        let Some(d) = self.doc.clone() else {
            ui.label("Open a file to see its bytes.");
            return;
        };
        let data = &d.raw;
        ui.horizontal(|ui| {
            ui.label("Go to offset:");
            let r = ui.add(egui::TextEdit::singleline(&mut self.hex_goto).hint_text("hex, e.g. 1A2F").desired_width(110.0));
            if (r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter))) || ui.button("Go").clicked() {
                let t = self.hex_goto.trim().trim_start_matches("0x").trim_start_matches("0X");
                if let Ok(o) = usize::from_str_radix(t, 16) {
                    let o = o.min(data.len().saturating_sub(1));
                    self.hex_scroll = Some(o / 16);
                    self.hex_sel = Some((o, o + 1));
                }
            }
            ui.label(format!("{} bytes", data.len()));
            if d.pjl_end > 0 {
                ui.separator();
                if ui.link(format!("PJL header: 0–{:X}", d.pjl_end)).clicked() {
                    self.hex_scroll = Some(0);
                    self.hex_sel = Some((0, d.pjl_end));
                }
            }
            if let Some((a, b)) = self.hex_sel {
                ui.separator();
                ui.label(format!("Selected {:X}–{:X} ({} bytes)", a, b, b - a));
                if let Some(ci) = self.cmd_sel {
                    if let Some(c) = d.commands.get(ci) {
                        ui.weak(c.chars().take(80).collect::<String>());
                    }
                }
            }
        });
        ui.separator();
        let rows = data.len().div_ceil(16);
        let row_h = ui.text_style_height(&egui::TextStyle::Monospace);
        let mut sa = egui::ScrollArea::vertical().auto_shrink([false, false]);
        if let Some(r) = self.hex_scroll.take() {
            sa = sa.vertical_scroll_offset((r as f32 * (row_h + ui.spacing().item_spacing.y) - 60.0).max(0.0));
        }
        let sel = self.hex_sel;
        let selc = if self.s.dark { Color32::from_rgb(90, 70, 0) } else { Color32::from_rgb(255, 236, 160) };
        let mut clicked: Option<usize> = None;
        sa.show_rows(ui, row_h, rows, |ui, range| {
            for r in range {
                let o = r * 16;
                let end = (o + 16).min(data.len());
                let bytes = &data[o..end];
                let mut job = egui::text::LayoutJob::default();
                let font = egui::FontId::monospace(row_h * 0.9);
                let normal = ui.visuals().text_color();
                let weak = ui.visuals().weak_text_color();
                let fmt = |c: Color32, bg: Color32| egui::TextFormat { font_id: font.clone(), color: c, background: bg, ..Default::default() };
                job.append(&format!("{o:08X}  "), 0.0, fmt(weak, Color32::TRANSPARENT));
                for i in 0..16 {
                    let in_sel = sel.map(|(a, b)| o + i >= a && o + i < b).unwrap_or(false);
                    let bg = if in_sel { selc } else { Color32::TRANSPARENT };
                    if i < bytes.len() {
                        job.append(&format!("{:02X}", bytes[i]), 0.0, fmt(normal, bg));
                    } else {
                        job.append("  ", 0.0, fmt(normal, Color32::TRANSPARENT));
                    }
                    job.append(if i == 7 { "  " } else { " " }, 0.0, fmt(normal, if in_sel && i + 1 < bytes.len() && sel.map(|(_, b)| o + i + 1 < b).unwrap_or(false) { selc } else { Color32::TRANSPARENT }));
                }
                job.append(" ", 0.0, fmt(normal, Color32::TRANSPARENT));
                for (i, &b) in bytes.iter().enumerate() {
                    let in_sel = sel.map(|(a, e)| o + i >= a && o + i < e).unwrap_or(false);
                    let c = if (32..127).contains(&b) { b as char } else { '·' };
                    job.append(&c.to_string(), 0.0, fmt(if (32..127).contains(&b) { normal } else { weak }, if in_sel { selc } else { Color32::TRANSPARENT }));
                }
                let resp = ui.add(egui::Label::new(job).sense(egui::Sense::click()).extend());
                if resp.clicked() {
                    clicked = Some(o);
                }
            }
        });
        if let Some(o) = clicked {
            if let Some(ci) = d.cmd_index_for(o as u32) {
                self.select_command(ci);
            } else {
                self.hex_sel = Some((o, (o + 16).min(data.len())));
            }
        }
    }

    // ---------------------------------------------------------------- compare

    fn start_page_diffs(&mut self, ctx: &egui::Context) {
        let (Some(a), Some(b)) = (self.doc.clone(), self.cmp.doc.clone()) else { return };
        let n = a.pages.len().max(b.pages.len());
        self.cmp.page_diffs = vec![None; n];
        let (tx, rx) = channel();
        let generation = self.cmp.generation;
        let ctx2 = ctx.clone();
        std::thread::spawn(move || {
            for i in 0..n {
                let pct = match (a.pages.get(i), b.pages.get(i)) {
                    (Some(pa), Some(pb)) => {
                        let sc = 36.0 / 72.0;
                        match (render::render_page(pa, sc), render::render_page(pb, sc)) {
                            (Some(ra), Some(rb)) => diff_images(&ra, &rb).1,
                            _ => 100.0,
                        }
                    }
                    _ => 100.0,
                };
                if tx.send((generation, i, pct)).is_err() {
                    return;
                }
                ctx2.request_repaint();
            }
        });
        self.cmp.diffs_rx = Some(rx);
    }

    fn compare_rows(a: &Document, b: &Document) -> Vec<[String; 4]> {
        use std::collections::BTreeMap;
        let collect = |d: &Document| {
            let mut m: BTreeMap<(usize, String, String), String> = BTreeMap::new();
            let mut order: BTreeMap<String, usize> = BTreeMap::new();
            for (si, s) in d.info.iter().enumerate() {
                if s.title.contains("(raw)") || s.title == "Rendered pages" && false {
                    continue;
                }
                let so = *order.entry(s.title.clone()).or_insert(si);
                let mut seen: BTreeMap<String, usize> = BTreeMap::new();
                for (k, v) in &s.rows {
                    let c = seen.entry(k.clone()).or_insert(0);
                    *c += 1;
                    let key = if *c > 1 { format!("{k} #{c}") } else { k.clone() };
                    m.insert((so, s.title.clone(), key), v.clone());
                }
            }
            m
        };
        let ma = collect(a);
        let mb = collect(b);
        let mut keys: Vec<(usize, String, String)> = ma.keys().cloned().collect();
        for k in mb.keys() {
            if !ma.keys().any(|x| x.1 == k.1 && x.2 == k.2) {
                keys.push((k.0 + 1000, k.1.clone(), k.2.clone()));
            }
        }
        keys.sort();
        let find = |m: &BTreeMap<(usize, String, String), String>, t: &str, k: &str| m.iter().find(|(x, _)| x.1 == t && x.2 == k).map(|(_, v)| v.clone());
        keys.into_iter()
            .map(|(_, t, k)| {
                let va = find(&ma, &t, &k).unwrap_or_else(|| "—".into());
                let vb = find(&mb, &t, &k).unwrap_or_else(|| "—".into());
                [t, k, va, vb]
            })
            .collect()
    }

    fn compare_tab(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let Some(a) = self.doc.clone() else {
            ui.label("Open a file first, then choose a second file to compare it with.");
            return;
        };
        if let Some(rx) = &self.cmp.diffs_rx {
            while let Ok((g, i, pct)) = rx.try_recv() {
                if g == self.cmp.generation && i < self.cmp.page_diffs.len() {
                    self.cmp.page_diffs[i] = Some(pct);
                }
            }
        }
        ui.horizontal(|ui| {
            ui.label(RichText::new("A:").strong());
            ui.label(self.path.as_deref().map(file_name).unwrap_or_default());
            ui.separator();
            ui.label(RichText::new("B:").strong());
            if let Some((p, _)) = &self.cmp.loading {
                ui.spinner();
                ui.label(file_name(p));
            } else {
                ui.label(self.cmp.path.as_deref().map(file_name).unwrap_or_else(|| "(none)".into()));
            }
            if ui.button("Choose file B…").clicked() {
                self.pick_file(ctx, true);
            }
            if self.cmp.doc.is_some() && ui.button("↔ Swap").on_hover_text("Make B the main file").clicked() {
                if let (Some(pb), Some(pa)) = (self.cmp.path.clone(), self.path.clone()) {
                    self.cmp.loading = None;
                    self.cmp.path = None;
                    self.cmp.doc = None;
                    self.open_compare(pa, ctx);
                    let rx = spawn_load(pb.clone(), ctx);
                    self.loading = Some((pb, rx));
                }
            }
        });
        let Some(b) = self.cmp.doc.clone() else {
            ui.add_space(20.0);
            ui.label("Choose a second print file (or drop one here) to see the differences page by page, and in the job settings.");
            return;
        };
        ui.horizontal(|ui| {
            ui.selectable_value(&mut self.cmp.mode, CmpMode::Diff, "Differences");
            ui.selectable_value(&mut self.cmp.mode, CmpMode::Side, "Side by side");
            ui.selectable_value(&mut self.cmp.mode, CmpMode::Settings, "Job settings");
            ui.separator();
            ui.label(format!("A: {} page(s) · B: {} page(s)", a.pages.len(), b.pages.len()));
            let done = self.cmp.page_diffs.iter().filter(|x| x.is_some()).count();
            let diff: Vec<usize> = self.cmp.page_diffs.iter().enumerate().filter(|(_, x)| x.map(|p| p > 0.05).unwrap_or(false)).map(|(i, _)| i).collect();
            ui.separator();
            if done < self.cmp.page_diffs.len() {
                ui.spinner();
            }
            if diff.is_empty() && done == self.cmp.page_diffs.len() {
                ui.colored_label(Color32::from_rgb(30, 150, 60), "✔ The pages look the same");
            } else if !diff.is_empty() {
                ui.label("Pages that differ:");
                for &i in diff.iter().take(30) {
                    if ui.selectable_label(i == self.page, format!("{}", i + 1)).clicked() {
                        self.page = i;
                    }
                }
                if diff.len() > 30 {
                    ui.label(format!("… ({} in all)", diff.len()));
                }
            }
        });
        ui.separator();
        if self.cmp.mode == CmpMode::Settings {
            let key = (self.generation, self.cmp.generation);
            if self.cmp.rows.as_ref().map(|(g, c, _)| (*g, *c) != key).unwrap_or(true) {
                self.cmp.rows = Some((key.0, key.1, Self::compare_rows(&a, &b)));
            }
            ui.checkbox(&mut self.cmp.only_diff, "Show only differences");
            let rows: Vec<[String; 4]> = self.cmp.rows.as_ref().map(|(_, _, r)| r.iter().filter(|r| !self.cmp.only_diff || r[2] != r[3]).cloned().collect()).unwrap_or_default();
            if rows.is_empty() {
                ui.label("No differences in the job settings.");
                return;
            }
            use egui_extras::{Column, TableBuilder};
            let diff_col = if self.s.dark { Color32::from_rgb(255, 200, 90) } else { Color32::from_rgb(170, 80, 0) };
            TableBuilder::new(ui)
                .striped(true)
                .resizable(true)
                .column(Column::initial(200.0))
                .column(Column::initial(220.0))
                .column(Column::initial(300.0))
                .column(Column::remainder())
                .header(22.0, |mut h| {
                    for t in ["Section", "Item", "A", "B"] {
                        h.col(|ui| {
                            ui.strong(t);
                        });
                    }
                })
                .body(|body| {
                    body.rows(20.0, rows.len(), |mut row| {
                        let r = &rows[row.index()];
                        let differs = r[2] != r[3];
                        row.col(|ui| {
                            ui.label(&r[0]);
                        });
                        row.col(|ui| {
                            ui.label(&r[1]);
                        });
                        for v in [&r[2], &r[3]] {
                            row.col(|ui| {
                                let t = RichText::new(v.as_str());
                                ui.add(egui::Label::new(if differs { t.color(diff_col) } else { t }).truncate()).on_hover_text(v.as_str());
                            });
                        }
                    });
                });
            return;
        }
        let n = a.pages.len().max(b.pages.len());
        if n == 0 {
            ui.label("Neither file has pages to compare.");
            return;
        }
        self.page = self.page.min(n - 1);
        let pa = a.pages.get(self.page);
        let pb = b.pages.get(self.page);
        let (pw, ph) = pa.or(pb).map(|p| (p.width, p.height)).unwrap_or((612.0, 792.0));
        let avail = ui.available_size() - egui::vec2(0.0, 40.0);
        let cols = if self.cmp.mode == CmpMode::Side { 2.0 } else { 1.0 };
        let fit = ((avail.x / cols - 30.0) / pw).min((avail.y - 10.0) / ph).max(0.05);
        let scale = fit * ctx.pixels_per_point();
        let key = (self.generation, self.cmp.generation, self.page, (scale * 1000.0) as u32);
        if let Some((k, rx)) = &self.cmp.pending {
            if let Ok(imgs) = rx.try_recv() {
                let k = *k;
                let blank = render::Rendered { width: 1, height: 1, rgba: vec![255; 4] };
                let ta = texture(ctx, "cmpA", imgs.a.as_ref().unwrap_or(&blank));
                let tb = texture(ctx, "cmpB", imgs.b.as_ref().unwrap_or(&blank));
                let td = imgs.diff.as_ref().map(|d| texture(ctx, "cmpD", d));
                self.cmp.tex = Some((ta, tb, td, imgs.pct));
                self.cmp.key = Some(k);
                self.cmp.pending = None;
            }
        }
        if self.cmp.key != Some(key) && self.cmp.pending.as_ref().map(|(k, _)| *k != key).unwrap_or(true) {
            let (tx, rx) = channel();
            let (a2, b2, pi) = (a.clone(), b.clone(), self.page);
            let ctx2 = ctx.clone();
            std::thread::spawn(move || {
                let ra = a2.pages.get(pi).and_then(|p| render::render_page(p, scale));
                let rb = b2.pages.get(pi).and_then(|p| render::render_page(p, scale));
                let (diff, pct) = match (&ra, &rb) {
                    (Some(x), Some(y)) => {
                        let (d, p) = diff_images(x, y);
                        (Some(d), p)
                    }
                    _ => (None, 100.0),
                };
                let _ = tx.send(CmpImages { a: ra, b: rb, diff, pct });
                ctx2.request_repaint();
            });
            self.cmp.pending = Some((key, rx));
        }
        let Some((ta, tb, td, pct)) = &self.cmp.tex else {
            ui.spinner();
            return;
        };
        let uv = Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0));
        let draw = |ui: &mut egui::Ui, t: &egui::TextureHandle, p: Option<&doc::Page>, label: &str| {
            ui.vertical(|ui| {
                ui.label(RichText::new(label).strong());
                match p {
                    Some(p) => {
                        let size = egui::vec2(p.width * fit, p.height * fit);
                        let (r, _) = ui.allocate_exact_size(size, egui::Sense::hover());
                        ui.painter().rect_filled(r, 0.0, Color32::WHITE);
                        ui.painter().image(t.id(), r, uv, Color32::WHITE);
                        ui.painter().rect_stroke(r, 0.0, Stroke::new(1.0_f32, Color32::GRAY));
                    }
                    None => {
                        ui.label("(no such page)");
                    }
                }
            });
        };
        if self.cmp.mode == CmpMode::Side {
            ui.horizontal_top(|ui| {
                draw(ui, ta, pa, &format!("A — page {}", self.page + 1));
                ui.add_space(12.0);
                draw(ui, tb, pb, &format!("B — page {}", self.page + 1));
            });
        } else {
            ui.horizontal(|ui| {
                if pa.is_none() || pb.is_none() {
                    ui.label(format!("Page {} exists only in {}", self.page + 1, if pa.is_some() { "A" } else { "B" }));
                } else if *pct <= 0.05 {
                    ui.colored_label(Color32::from_rgb(30, 150, 60), "✔ No visible differences on this page");
                } else {
                    ui.label(format!("{:.1}% of the printed area differs.", pct));
                }
                ui.colored_label(Color32::from_rgb(220, 30, 30), "■ only in A");
                ui.colored_label(Color32::from_rgb(30, 90, 230), "■ only in B");
            });
            match td {
                Some(t) => {
                    let size = egui::vec2(pw * fit, ph * fit);
                    let tsz = t.size_vec2() / ctx.pixels_per_point();
                    let size = if tsz.x > 1.0 { tsz } else { size };
                    let (r, _) = ui.allocate_exact_size(size, egui::Sense::hover());
                    ui.painter().image(t.id(), r, uv, Color32::WHITE);
                    ui.painter().rect_stroke(r, 0.0, Stroke::new(1.0_f32, Color32::GRAY));
                }
                None => {
                    let t = if pa.is_some() { ta } else { tb };
                    draw(ui, t, pa.or(pb), "");
                }
            }
        }
    }

    // ---------------------------------------------------------------- messages

    fn warnings_tab(&mut self, ui: &mut egui::Ui) {
        let Some(d) = self.doc.clone() else { return };
        if d.warnings.is_empty() {
            ui.label("✔ No messages. Everything in this file was understood.");
            return;
        }
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            for w in &d.warnings {
                ui.add(egui::Label::new(format!("⚠ {w}")).wrap().selectable(true));
            }
        });
    }

    fn thumbs_panel(&mut self, ctx: &egui::Context) {
        let n = self.page_count();
        if !self.s.thumbs || n == 0 || self.tab != Tab::View {
            return;
        }
        let d = self.doc.clone();
        egui::SidePanel::left("thumbs").resizable(false).exact_width(150.0).show(ctx, |ui| {
            egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                for k in 0..n {
                    let selected = k == self.page;
                    let resp = ui
                        .vertical_centered(|ui| {
                            let (w, h) = d.as_ref().map(|d| (d.pages[k].width, d.pages[k].height)).unwrap_or((612.0, 792.0));
                            let sc = 110.0 / w.max(h);
                            let size = egui::vec2(w * sc, h * sc);
                            let (rect, resp) = ui.allocate_exact_size(size + egui::vec2(8.0, 8.0), egui::Sense::click());
                            let inner = rect.shrink(4.0);
                            if selected {
                                ui.painter().rect_filled(rect, 3.0, ui.visuals().selection.bg_fill);
                            }
                            ui.painter().rect_filled(inner, 0.0, Color32::WHITE);
                            if let Some(Some(t)) = self.thumbs.get(k) {
                                ui.painter().image(t.id(), inner, Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), Color32::WHITE);
                            }
                            ui.painter().rect_stroke(inner, 0.0, Stroke::new(1.0_f32, Color32::from_gray(160)));
                            let cov = d.as_ref().and_then(|d| d.coverage.get(k).copied());
                            ui.horizontal(|ui| {
                                ui.add_space((ui.available_width() - 40.0).max(0.0) / 2.0);
                                ui.label(format!("{}", k + 1));
                                if let Some(c) = cov {
                                    if c.has_colour || c.blank {
                                        let (r, resp) = ui.allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
                                        if c.has_colour {
                                            let cols = [Color32::from_rgb(0, 170, 230), Color32::from_rgb(230, 0, 140), Color32::from_rgb(250, 210, 0)];
                                            for (i, col) in cols.iter().enumerate() {
                                                ui.painter().circle_filled(r.center() + egui::vec2(-3.0 + 3.0 * i as f32, 0.0), 3.0, *col);
                                            }
                                            resp.on_hover_text("Colour page");
                                        } else {
                                            ui.painter().circle_stroke(r.center(), 3.5, Stroke::new(1.0_f32, Color32::GRAY));
                                            resp.on_hover_text("Blank page");
                                        }
                                    }
                                }
                            });
                            resp
                        })
                        .inner;
                    if resp.clicked() {
                        self.go(k as isize);
                    }
                    if selected && self.scroll_to_top {
                        resp.scroll_to_me(None);
                    }
                }
            });
        });
    }

    // ---------------------------------------------------------------- windows

    fn print_window(&mut self, ctx: &egui::Context) {
        let Some(dlg) = self.print_dlg.as_mut() else { return };
        let mut open = true;
        let mut go = false;
        let n = self.doc.as_ref().map(|d| d.pages.len()).unwrap_or(0);
        egui::Window::new("Print").open(&mut open).collapsible(false).resizable(false).show(ctx, |ui| {
            if dlg.printers.is_empty() {
                ui.label("No printers are installed.");
                return;
            }
            egui::Grid::new("pg").num_columns(2).spacing([12.0, 8.0]).show(ui, |ui| {
                ui.label("Printer:");
                egui::ComboBox::from_id_salt("printer").selected_text(&dlg.printer).width(280.0).show_ui(ui, |ui| {
                    for p in &dlg.printers {
                        ui.selectable_value(&mut dlg.printer, p.clone(), p);
                    }
                });
                ui.end_row();
                ui.label("Pages:");
                ui.horizontal(|ui| {
                    ui.radio_value(&mut dlg.range, 0, format!("All ({n})"));
                    ui.radio_value(&mut dlg.range, 1, "Current");
                    ui.radio_value(&mut dlg.range, 2, "From");
                    ui.add_enabled(dlg.range == 2, egui::DragValue::new(&mut dlg.from).range(1..=n.max(1)));
                    ui.label("to");
                    ui.add_enabled(dlg.range == 2, egui::DragValue::new(&mut dlg.to).range(1..=n.max(1)));
                });
                ui.end_row();
                ui.label("Copies:");
                ui.add(egui::DragValue::new(&mut dlg.copies).range(1..=999));
                ui.end_row();
                ui.label("");
                ui.checkbox(&mut dlg.fit, "Fit to the printer's paper");
                ui.end_row();
            });
            ui.weak("This prints the pages as shown here. To send the original job unchanged, use File → Send job to a printer.");
            ui.add_space(6.0);
            if ui.button("⎙ Print").clicked() {
                go = true;
            }
        });
        if go {
            let dlg = self.print_dlg.take().unwrap();
            let Some(d) = self.doc.clone() else { return };
            let (a, b) = match dlg.range {
                0 => (0, n),
                1 => (self.page, self.page + 1),
                _ => (dlg.from.min(dlg.to).max(1) - 1, dlg.to.max(dlg.from).min(n)),
            };
            self.s.printer = dlg.printer.clone();
            self.s.save();
            let name = self.path.as_deref().map(file_name).unwrap_or_else(|| "Print File Viewer".into());
            let (tx, rx) = channel();
            let ctx2 = ctx.clone();
            std::thread::spawn(move || {
                let pages: Vec<&doc::Page> = d.pages[a..b].iter().collect();
                let r = crate::output::print_pages(&dlg.printer, &name, &pages, dlg.copies, dlg.fit).map(|_| format!("Sent {} page(s) to {}", pages.len(), dlg.printer));
                let _ = tx.send(r);
                ctx2.request_repaint();
            });
            self.task_rx = Some(rx);
            self.flash("Printing…");
            return;
        }
        if !open {
            self.print_dlg = None;
        }
    }

    fn send_window(&mut self, ctx: &egui::Context) {
        let Some(dlg) = self.send_dlg.as_mut() else { return };
        let mut open = true;
        let mut go = false;
        egui::Window::new("Send job to a printer").open(&mut open).collapsible(false).resizable(false).show(ctx, |ui| {
            ui.label(format!("Sends {} ({}) to the printer exactly as it is, without changing it.", dlg.what, doc::fmt_bytes(dlg.data.len())));
            ui.add_space(6.0);
            ui.radio_value(&mut dlg.to_queue, true, "An installed printer (sent as RAW data)");
            ui.add_enabled_ui(dlg.to_queue, |ui| {
                ui.indent("q", |ui| {
                    if dlg.printers.is_empty() {
                        ui.weak("No printers are installed.");
                    } else {
                        egui::ComboBox::from_id_salt("sendprinter").selected_text(&dlg.printer).width(300.0).show_ui(ui, |ui| {
                            for p in &dlg.printers {
                                ui.selectable_value(&mut dlg.printer, p.clone(), p);
                            }
                        });
                    }
                });
            });
            ui.radio_value(&mut dlg.to_queue, false, "A network printer by address (raw port / JetDirect)");
            ui.add_enabled_ui(!dlg.to_queue, |ui| {
                ui.indent("n", |ui| {
                    ui.horizontal(|ui| {
                        ui.label("Address:");
                        ui.add(egui::TextEdit::singleline(&mut dlg.host).hint_text("e.g. 192.168.1.50").desired_width(180.0));
                        ui.label("Port:");
                        ui.add(egui::DragValue::new(&mut dlg.port).range(1..=65535));
                    });
                });
            });
            ui.add_space(4.0);
            ui.weak("The printer must understand the job's language (PCL, PostScript…). The job will print with the settings it already contains.");
            ui.add_space(6.0);
            let ok = if dlg.to_queue { !dlg.printer.is_empty() } else { !dlg.host.trim().is_empty() };
            if ui.add_enabled(ok, egui::Button::new("📤 Send")).clicked() {
                go = true;
            }
        });
        if go {
            let dlg = self.send_dlg.take().unwrap();
            if dlg.to_queue {
                self.s.printer = dlg.printer.clone();
            } else {
                self.s.host = dlg.host.trim().to_string();
                self.s.port = dlg.port;
            }
            self.s.save();
            let name = self.path.as_deref().map(file_name).unwrap_or_else(|| "Print File Viewer".into());
            let (tx, rx) = channel();
            let ctx2 = ctx.clone();
            std::thread::spawn(move || {
                let r = if dlg.to_queue {
                    crate::output::send_raw(&dlg.printer, &name, &dlg.data).map(|_| format!("Sent to {}", dlg.printer))
                } else {
                    crate::output::send_tcp(dlg.host.trim(), dlg.port, &dlg.data).map(|_| format!("Sent to {}:{}", dlg.host.trim(), dlg.port))
                };
                let _ = tx.send(r);
                ctx2.request_repaint();
            });
            self.task_rx = Some(rx);
            self.flash("Sending…");
            return;
        }
        if !open {
            self.send_dlg = None;
        }
    }

    fn pjl_window(&mut self, ctx: &egui::Context) {
        let Some(dlg) = self.pjl_dlg.as_mut() else { return };
        let Some(d) = self.doc.clone() else { return };
        let mut open = true;
        let mut save = false;
        let mut send = false;
        egui::Window::new("Edit PJL header").open(&mut open).default_size([640.0, 460.0]).resizable(true).show(ctx, |ui| {
            if d.pjl_end == 0 {
                ui.label("This job has no PJL header. Lines you add here are placed in front of the job. End with an ENTER LANGUAGE line, for example:");
                ui.monospace("@PJL ENTER LANGUAGE=PCLXL");
            } else {
                ui.label("Change, add or remove @PJL lines. The rest of the job is kept exactly as it is.");
            }
            ui.horizontal_wrapped(|ui| {
                ui.label("Insert:");
                let snippets: &[(&str, &str)] = &[
                    ("Copies", "@PJL SET COPIES=2"),
                    ("Duplex", "@PJL SET DUPLEX=ON\n@PJL SET BINDING=LONGEDGE"),
                    ("Simplex", "@PJL SET DUPLEX=OFF"),
                    ("Paper", "@PJL SET PAPER=A4"),
                    ("Tray", "@PJL SET MEDIASOURCE=TRAY2"),
                    ("Output bin", "@PJL SET OUTBIN=UPPER"),
                    ("Mono", "@PJL SET RENDERMODE=GRAYSCALE"),
                    ("Hold with PIN", "@PJL SET HOLD=ON\n@PJL SET HOLDTYPE=PRIVATE\n@PJL SET HOLDKEY=\"1234\""),
                    ("Staple", "@PJL SET STAPLE=LEFTTOPANGLEDONE"),
                    ("Job name", "@PJL SET JOBNAME=\"My job\""),
                ];
                for (label, text) in snippets {
                    if ui.small_button(*label).clicked() {
                        // insert before the ENTER LANGUAGE line
                        let mut lines: Vec<String> = dlg.text.lines().map(|l| l.to_string()).collect();
                        let at = lines.iter().position(|l| l.to_ascii_uppercase().contains("ENTER LANGUAGE")).unwrap_or(lines.len());
                        for (k, l) in text.lines().enumerate() {
                            lines.insert(at + k, l.to_string());
                        }
                        dlg.text = lines.join("\n") + "\n";
                    }
                }
            });
            ui.add_space(4.0);
            egui::ScrollArea::vertical().max_height(ui.available_height() - 40.0).show(ui, |ui| {
                ui.add(egui::TextEdit::multiline(&mut dlg.text).code_editor().desired_rows(16).desired_width(f32::INFINITY));
            });
            ui.horizontal(|ui| {
                if ui.button("💾 Save as new file…").clicked() {
                    save = true;
                }
                if ui.button("📤 Send to a printer…").clicked() {
                    send = true;
                }
                if ui.add_enabled(dlg.text != dlg.original, egui::Button::new("↺ Undo changes")).clicked() {
                    dlg.text = dlg.original.clone();
                }
            });
        });
        if save || send {
            let text = self.pjl_dlg.as_ref().map(|x| x.text.clone()).unwrap_or_default();
            let bytes = Self::pjl_bytes(&d, &text);
            if save {
                let ext = self.path.as_ref().and_then(|p| p.extension()).map(|e| e.to_string_lossy().to_string()).unwrap_or_else(|| "prn".into());
                if let Some(out) = self.dialog().set_file_name(format!("{}-edited.{ext}", self.stem())).save_file() {
                    match std::fs::write(&out, &bytes) {
                        Ok(_) => self.flash(format!("Saved {}", file_name(&out))),
                        Err(e) => self.error = Some(format!("Could not save: {e}")),
                    }
                }
            } else {
                self.open_send(Arc::new(bytes), "the job with your PJL changes".into());
            }
        }
        if !open {
            self.pjl_dlg = None;
        }
    }

    fn batch_window(&mut self, ctx: &egui::Context) {
        let Some(b) = self.batch.as_mut() else { return };
        if let Some(rx) = &b.rx {
            loop {
                match rx.try_recv() {
                    Ok((_, row)) => b.rows.push(row),
                    Err(std::sync::mpsc::TryRecvError::Empty) => break,
                    Err(_) => {
                        b.rx = None;
                        break;
                    }
                }
            }
        }
        let mut open = true;
        let mut start = false;
        let mut open_file: Option<PathBuf> = None;
        let mut save_dir = None;
        egui::Window::new("Batch report").open(&mut open).default_size([980.0, 560.0]).resizable(true).show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label("Folder:");
                ui.label(b.dir.as_ref().map(|d| d.display().to_string()).unwrap_or_else(|| "(choose a folder)".into()));
                if ui.button("Choose…").clicked() {
                    let mut dlg = rfd::FileDialog::new();
                    if let Some(d) = &b.dir {
                        dlg = dlg.set_directory(d);
                    }
                    if let Some(d) = dlg.pick_folder() {
                        b.dir = Some(d.clone());
                        save_dir = Some(d);
                    }
                }
                ui.checkbox(&mut b.recursive, "Include subfolders");
                let running = b.rx.is_some();
                if running {
                    if ui.button("■ Stop").clicked() {
                        b.cancel.store(true, Ordering::Relaxed);
                    }
                    ui.spinner();
                    ui.label(format!("{} of {}", b.rows.len(), b.total));
                } else if ui.add_enabled(b.dir.is_some(), egui::Button::new("▶ Run")).clicked() {
                    start = true;
                }
                ui.separator();
                ui.add_enabled_ui(!b.rows.is_empty() && !running, |ui| {
                    if ui.button("Export CSV…").clicked() {
                        if let Some(out) = rfd::FileDialog::new().set_file_name("print-jobs.csv").add_filter("CSV", &["csv"]).save_file() {
                            let _ = std::fs::write(out, summary::to_csv(&b.rows));
                        }
                    }
                    if ui.button("Export JSON…").clicked() {
                        if let Some(out) = rfd::FileDialog::new().set_file_name("print-jobs.json").add_filter("JSON", &["json"]).save_file() {
                            let _ = std::fs::write(out, summary::to_json(&b.rows));
                        }
                    }
                });
            });
            if !b.rows.is_empty() {
                let pages: usize = b.rows.iter().map(|r| r.pages).sum();
                let colour: usize = b.rows.iter().map(|r| r.colour_pages).sum();
                let checks = b.rows.iter().filter(|r| r.checks > 0).count();
                let errors = b.rows.iter().filter(|r| !r.error.is_empty()).count();
                ui.label(format!("{} job(s) · {} page(s) · {} colour page(s) · {} job(s) with checks to look at · {} unreadable", b.rows.len(), pages, colour, checks, errors));
            }
            ui.separator();
            use egui_extras::{Column, TableBuilder};
            let cols: &[&str] = &["File", "Format", "Pages", "Colour", "Paper", "Duplex", "Copies", "User", "Job name", "Application", "Printer / driver", "Checks"];
            let mut sort_click: Option<usize> = None;
            let mut order: Vec<usize> = (0..b.rows.len()).collect();
            let key = |r: &JobSummary, c: usize| -> (String, f64) {
                match c {
                    0 => (r.file.to_lowercase(), 0.0),
                    1 => (r.format.clone(), 0.0),
                    2 => (String::new(), r.pages as f64),
                    3 => (String::new(), r.colour_pages as f64),
                    4 => (r.paper.clone(), 0.0),
                    5 => (r.duplex.clone(), 0.0),
                    6 => (r.copies.clone(), 0.0),
                    7 => (r.user.to_lowercase(), 0.0),
                    8 => (r.job_name.to_lowercase(), 0.0),
                    9 => (r.application.to_lowercase(), 0.0),
                    10 => (format!("{} {}", r.printer, r.driver), 0.0),
                    _ => (String::new(), r.checks as f64),
                }
            };
            let sc = b.sort;
            order.sort_by(|&x, &y| {
                let (a1, a2) = key(&b.rows[x], sc % 100);
                let (b1, b2) = key(&b.rows[y], sc % 100);
                let o = a1.cmp(&b1).then(a2.partial_cmp(&b2).unwrap_or(std::cmp::Ordering::Equal));
                if sc >= 100 { o.reverse() } else { o }
            });
            let warn_col = if self.s.dark { Color32::from_rgb(255, 180, 60) } else { Color32::from_rgb(190, 110, 0) };
            TableBuilder::new(ui)
                .striped(true)
                .resizable(true)
                .sense(egui::Sense::click())
                .column(Column::initial(150.0))
                .column(Column::initial(150.0))
                .columns(Column::initial(50.0), 2)
                .column(Column::initial(80.0))
                .column(Column::initial(90.0))
                .column(Column::initial(50.0))
                .column(Column::initial(90.0))
                .column(Column::initial(180.0))
                .column(Column::initial(110.0))
                .column(Column::initial(160.0))
                .column(Column::remainder())
                .header(22.0, |mut h| {
                    for (i, c) in cols.iter().enumerate() {
                        h.col(|ui| {
                            let arrow = if sc % 100 == i { if sc >= 100 { " ⏷" } else { " ⏶" } } else { "" };
                            if ui.add(egui::Label::new(RichText::new(format!("{c}{arrow}")).strong()).sense(egui::Sense::click())).clicked() {
                                sort_click = Some(i);
                            }
                        });
                    }
                })
                .body(|body| {
                    body.rows(20.0, order.len(), |mut row| {
                        let ri = order[row.index()];
                        let r = &b.rows[ri];
                        row.set_selected(b.sel == Some(ri));
                        let vals = [
                            r.file.clone(),
                            if r.error.is_empty() { r.format.clone() } else { format!("⚠ {}", r.error) },
                            r.pages.to_string(),
                            r.colour_pages.to_string(),
                            r.paper.clone(),
                            r.duplex.clone(),
                            r.copies.clone(),
                            r.user.clone(),
                            r.job_name.clone(),
                            r.application.clone(),
                            if r.printer.is_empty() { r.driver.clone() } else { format!("{} ({})", r.printer, r.driver) },
                            if r.checks == 0 { String::new() } else { format!("⚠ {}", r.check_text) },
                        ];
                        for (i, v) in vals.iter().enumerate() {
                            row.col(|ui| {
                                let t = RichText::new(v.as_str());
                                let t = if i == 11 { t.color(warn_col) } else { t };
                                ui.add(egui::Label::new(t).truncate().selectable(false)).on_hover_text(v.as_str());
                            });
                        }
                        let resp = row.response();
                        if resp.clicked() {
                            b.sel = Some(ri);
                        }
                        if resp.double_clicked() {
                            open_file = Some(r.path.clone());
                        }
                    });
                });
            if let Some(i) = sort_click {
                b.sort = if b.sort == i { i + 100 } else { i };
            }
            ui.weak("Double-click a job to open it.");
        });
        if let Some(d) = save_dir {
            self.s.batch_dir = Some(d);
            self.s.save();
        }
        if start {
            let b = self.batch.as_mut().unwrap();
            let files = summary::list_jobs(b.dir.as_ref().unwrap(), b.recursive);
            b.total = files.len();
            b.rows.clear();
            b.sel = None;
            b.cancel = Arc::new(AtomicBool::new(false));
            let cancel = b.cancel.clone();
            let (tx, rx) = channel();
            let ctx2 = ctx.clone();
            std::thread::Builder::new()
                .stack_size(64 * 1024 * 1024)
                .spawn(move || {
                    for (i, f) in files.iter().enumerate() {
                        if cancel.load(Ordering::Relaxed) {
                            break;
                        }
                        let s = summary::summarise_file(f);
                        if tx.send((i, s)).is_err() {
                            break;
                        }
                        ctx2.request_repaint();
                    }
                    ctx2.request_repaint();
                })
                .ok();
            b.rx = Some(rx);
        }
        if let Some(p) = open_file {
            self.tab = Tab::View;
            self.open(p, ctx);
        }
        if !open {
            if let Some(b) = &self.batch {
                b.cancel.store(true, Ordering::Relaxed);
            }
            self.batch = None;
        }
    }

    fn watch_scan(w: &mut Watch) {
        w.last_scan = Some(Instant::now());
        let rd = match std::fs::read_dir(&w.dir) {
            Ok(r) => r,
            Err(e) => {
                w.error = Some(if e.kind() == std::io::ErrorKind::PermissionDenied {
                    "Access denied. The Windows spool folder can only be read by administrators: run Print File Viewer as administrator.".into()
                } else {
                    format!("Can't read the folder: {e}")
                });
                return;
            }
        };
        w.error = None;
        let mut present: Vec<PathBuf> = Vec::new();
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                continue;
            }
            let ext = p.extension().map(|x| x.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
            if ext == "shd" || ext == "tmp" {
                continue;
            }
            if !summary::is_job_file(&p) {
                continue;
            }
            present.push(p.clone());
            let size = e.metadata().map(|m| m.len()).unwrap_or(0);
            match w.entries.iter_mut().find(|x| x.path == p) {
                Some(x) => {
                    x.gone = false;
                    if x.size == size {
                        x.stable += 1;
                    } else {
                        x.size = size;
                        x.stable = 0;
                    }
                }
                None => w.entries.insert(0, WatchEntry { path: p.clone(), size, stable: 0, first_seen: now_string(), kept: None, gone: false, note: String::new() }),
            }
        }
        for x in w.entries.iter_mut() {
            if !present.contains(&x.path) {
                x.gone = true;
            }
        }
        if w.keep {
            if let Some(kd) = w.keep_dir.clone() {
                for x in w.entries.iter_mut() {
                    if x.kept.is_none() && !x.gone && x.stable >= 2 && x.size > 0 {
                        let stamp = now_string().replace([':', ' '], "-").replace("-UTC", "");
                        let name = format!("{stamp}_{}", file_name(&x.path));
                        let dest = kd.join(&name);
                        match std::fs::copy(&x.path, &dest) {
                            Ok(_) => {
                                if let Some(shd) = crate::shd::find_for(&x.path) {
                                    let sname = format!("{stamp}_{}", file_name(&shd));
                                    let _ = std::fs::copy(&shd, kd.join(sname));
                                }
                                x.kept = Some(dest);
                            }
                            Err(e) => x.note = format!("copy failed: {e}"),
                        }
                    }
                }
            }
        }
        w.entries.truncate(500);
    }

    fn watch_window(&mut self, ctx: &egui::Context) {
        let Some(w) = self.watch.as_mut() else { return };
        if w.running && w.last_scan.map(|t| t.elapsed() > Duration::from_millis(1000)).unwrap_or(true) {
            Self::watch_scan(w);
        }
        if w.running {
            ctx.request_repaint_after(Duration::from_millis(1000));
        }
        let mut open = true;
        let mut open_file: Option<PathBuf> = None;
        let mut save = false;
        egui::Window::new("Watch spool folder").open(&mut open).default_size([760.0, 460.0]).resizable(true).show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label("Folder:");
                ui.label(w.dir.display().to_string());
                if ui.button("Change…").clicked() {
                    if let Some(d) = rfd::FileDialog::new().set_directory(&w.dir).pick_folder() {
                        w.dir = d;
                        w.entries.clear();
                        save = true;
                    }
                }
                if w.running {
                    if ui.button("■ Stop").clicked() {
                        w.running = false;
                    }
                    ui.spinner();
                } else if ui.button("▶ Start watching").clicked() {
                    w.running = true;
                    w.last_scan = None;
                }
            });
            ui.horizontal(|ui| {
                if ui.checkbox(&mut w.keep, "Keep a copy of every new job in:").changed() {
                    save = true;
                }
                ui.label(w.keep_dir.as_ref().map(|d| d.display().to_string()).unwrap_or_else(|| "(choose a folder)".into()));
                if ui.button("Choose…").clicked() {
                    if let Some(d) = rfd::FileDialog::new().pick_folder() {
                        w.keep_dir = Some(d);
                        w.keep = true;
                        save = true;
                    }
                }
            });
            ui.weak("Windows deletes spool files once they have printed. Keeping copies (with their .SHD files) lets you open them later. Reading the spool folder needs administrator rights; pausing the printer queue, or turning on \"Keep printed documents\" in the printer's Advanced properties, keeps jobs there longer.");
            if let Some(e) = &w.error {
                ui.colored_label(Color32::from_rgb(200, 40, 40), e);
            }
            ui.separator();
            use egui_extras::{Column, TableBuilder};
            TableBuilder::new(ui)
                .striped(true)
                .resizable(true)
                .sense(egui::Sense::click())
                .column(Column::initial(200.0))
                .column(Column::initial(90.0))
                .column(Column::initial(170.0))
                .column(Column::remainder())
                .header(22.0, |mut h| {
                    for t in ["Job file", "Size", "First seen", "Status"] {
                        h.col(|ui| {
                            ui.strong(t);
                        });
                    }
                })
                .body(|body| {
                    body.rows(20.0, w.entries.len(), |mut row| {
                        let e = &w.entries[row.index()];
                        row.col(|ui| {
                            ui.label(file_name(&e.path));
                        });
                        row.col(|ui| {
                            ui.label(doc::fmt_bytes(e.size as usize).split(" (").next().unwrap_or("").to_string());
                        });
                        row.col(|ui| {
                            ui.label(&e.first_seen);
                        });
                        row.col(|ui| {
                            let s = if !e.note.is_empty() {
                                e.note.clone()
                            } else if let Some(k) = &e.kept {
                                format!("{}kept as {}", if e.gone { "printed · " } else { "" }, file_name(k))
                            } else if e.gone {
                                "gone (printed or deleted)".into()
                            } else if e.stable < 2 {
                                "spooling…".into()
                            } else {
                                "ready".into()
                            };
                            ui.label(s);
                        });
                        if row.response().double_clicked() {
                            open_file = e.kept.clone().or_else(|| if e.gone { None } else { Some(e.path.clone()) });
                        }
                    });
                });
            ui.weak("Double-click a job to open it.");
        });
        if save {
            self.s.watch_dir = Some(w.dir.clone());
            self.s.keep_dir = if w.keep { w.keep_dir.clone() } else { None };
            self.s.save();
        }
        if let Some(p) = open_file {
            // open a private copy so the spooler isn't blocked
            let tmp = std::env::temp_dir().join(format!("pfv-{}", file_name(&p)));
            let p = if p.starts_with(&self.watch.as_ref().unwrap().dir) && std::fs::copy(&p, &tmp).is_ok() {
                if let Some(shd) = crate::shd::find_for(&p) {
                    let _ = std::fs::copy(&shd, std::env::temp_dir().join(format!("pfv-{}", file_name(&shd))));
                }
                tmp
            } else {
                p
            };
            self.tab = Tab::View;
            self.open(p, ctx);
        }
        if !open {
            self.watch = None;
        }
    }

    fn about_window(&mut self, ctx: &egui::Context) {
        if !self.about {
            return;
        }
        egui::Window::new("About").open(&mut self.about).collapsible(false).resizable(false).show(ctx, |ui| {
            ui.heading(APP_NAME);
            ui.label(format!("Version {}", env!("CARGO_PKG_VERSION")));
            ui.add_space(6.0);
            ui.label("Reads PCL XL (PCL 6), PCL 5, HP-GL/2, PostScript, EMF spool and XPS print files, with Windows .SHD job records.");
            ui.add_space(8.0);
            egui::Grid::new("keys").num_columns(2).spacing([16.0, 4.0]).show(ui, |ui| {
                for (k, v) in [
                    ("Ctrl+O", "Open"),
                    ("Ctrl+P", "Print"),
                    ("Ctrl+E", "Export PDF"),
                    ("Ctrl+F", "Find text"),
                    ("Enter / F3", "Next match (Shift: previous)"),
                    ("PgUp / PgDn, ← →", "Previous / next page"),
                    ("Home / End", "First / last page"),
                    ("Ctrl + / Ctrl −, Ctrl+scroll", "Zoom"),
                    ("M", "Measure mode (Shift: straight lines)"),
                    ("Click on the page", "Identify an object and its command"),
                    ("Double-click a command", "Show what it drew"),
                    ("Esc", "Clear selection"),
                ] {
                    ui.strong(k);
                    ui.label(v);
                    ui.end_row();
                }
            });
        });
    }

    fn status_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            if let Some((p, _)) = &self.loading {
                ui.spinner();
                ui.label(format!("Loading {} …", p.display()));
                return;
            }
            if let Some(e) = self.error.clone() {
                ui.colored_label(Color32::from_rgb(200, 40, 40), &e);
                if ui.small_button("✖").clicked() {
                    self.error = None;
                }
                ui.separator();
            }
            if let Some((m, t)) = &self.status {
                if t.elapsed() < Duration::from_secs(6) || self.task_rx.is_some() {
                    if self.task_rx.is_some() {
                        ui.spinner();
                    }
                    ui.label(m);
                    ui.separator();
                }
            }
            if let Some(d) = &self.doc {
                let name = self.path.as_deref().map(file_name).unwrap_or_default();
                ui.label(format!("{name}  ·  {}  ·  {} page(s)", d.format, d.pages.len()));
                if let Some(p) = d.pages.get(self.page) {
                    ui.separator();
                    let pn = doc::paper_name(p.width, p.height).map(|s| s.to_string()).unwrap_or_else(|| format!("{:.0}×{:.0} mm", mm(p.width), mm(p.height)));
                    ui.label(format!("Page {}: {pn}", self.page + 1));
                    if let Some(c) = d.coverage.get(self.page) {
                        ui.separator();
                        ui.label(c.describe());
                    }
                }
                if self.tab == Tab::View {
                    if let Some((x, y)) = self.hover_pt {
                        ui.separator();
                        ui.label(format!("{:.1}, {:.1} mm  ({:.0}, {:.0} pt · {:.0}, {:.0} @600 dpi)", mm(x), mm(y), x, y, x / 72.0 * 600.0, y / 72.0 * 600.0));
                    }
                }
            } else if self.error.is_none() {
                ui.label("No file open");
            }
        });
    }
}

#[cfg(debug_assertions)]
impl App {
    /// Development aid: PFV_TEST="tab=hex;search=PCL;pick=100,100;…" puts the UI in a state for screenshots.
    fn run_test_script(&mut self, ctx: &egui::Context) {
        let Some(script) = self.test_script.clone() else { return };
        if self.doc.is_none() || self.loading.is_some() || self.cmp.loading.is_some() {
            ctx.request_repaint();
            return;
        }
        self.test_script = None;
        for cmd in script {
            let (k, v) = cmd.split_once('=').unwrap_or((cmd.as_str(), ""));
            let nums: Vec<f32> = v.split(',').filter_map(|x| x.trim().parse().ok()).collect();
            match k {
                "tab" => {
                    self.tab = match v {
                        "info" => Tab::Info,
                        "commands" => Tab::Commands,
                        "hex" => Tab::Hex,
                        "compare" => Tab::Compare,
                        "warnings" => Tab::Warnings,
                        _ => Tab::View,
                    }
                }
                "page" => self.page = nums.first().map(|n| *n as usize).unwrap_or(0),
                "search" => {
                    self.search = v.to_string();
                    self.run_search();
                }
                "pick" if nums.len() == 2 => self.pick((nums[0], nums[1])),
                "measure" if nums.len() == 4 => {
                    self.measure_mode = true;
                    self.measure = Some(((nums[0], nums[1]), (nums[2], nums[3])));
                }
                "cmd" => {
                    if let Some(i) = nums.first() {
                        self.select_command(*i as usize);
                        self.cmd_scroll = Some(*i as usize);
                    }
                }
                "showcmd" => {
                    if let Some(i) = nums.first() {
                        self.show_command_on_page(*i as usize);
                    }
                }
                "bytes" => {
                    if let Some(i) = nums.first() {
                        self.show_bytes(*i as usize);
                    }
                }
                "compare" => {
                    self.tab = Tab::Compare;
                    self.open_compare(PathBuf::from(v), ctx);
                }
                "cmpmode" => self.cmp.mode = match v { "side" => CmpMode::Side, "settings" => CmpMode::Settings, _ => CmpMode::Diff },
                "rulers" => self.s.rulers = true,
                "margins" => self.s.margins = true,
                "dark" => self.s.dark = true,
                "nothumbs" => self.s.thumbs = false,
                "layers" => self.layers = nums.first().map(|n| *n as u8).unwrap_or(7),
                "print" => self.open_print(),
                "pjl" => self.open_pjl(),
                "send" => {
                    if let Some(d) = &self.doc {
                        let data = d.raw.clone();
                        self.open_send(data, "the original job".into());
                    }
                }
                "about" => self.about = true,
                "batch" => {
                    self.batch = Some(Batch { dir: Some(PathBuf::from(v)), recursive: false, rows: Vec::new(), total: 0, rx: None, cancel: Arc::new(AtomicBool::new(false)), sort: 0, sel: None });
                    for f in summary::list_jobs(Path::new(v), false) {
                        let s = summary::summarise_file(&f);
                        self.batch.as_mut().unwrap().rows.push(s);
                    }
                }
                "watch" => {
                    self.watch = Some(Watch { dir: PathBuf::from(v), keep_dir: None, keep: false, running: true, entries: Vec::new(), last_scan: None, error: None });
                }
                _ => {}
            }
        }
        ctx.request_repaint();
    }
}

pub fn default_spool_dir() -> PathBuf {
    #[cfg(windows)]
    {
        let windir = std::env::var("WINDIR").or_else(|_| std::env::var("SystemRoot")).unwrap_or_else(|_| "C:\\Windows".into());
        PathBuf::from(format!("{windir}\\System32\\spool\\PRINTERS"))
    }
    #[cfg(not(windows))]
    {
        PathBuf::from("/var/spool/cups")
    }
}

fn draw_rulers(p: &egui::Painter, rect: Rect, w_pt: f32, h_pt: f32, pt_to_px: f32, dark: bool, hover: Option<(f32, f32)>) {
    let bg = if dark { Color32::from_gray(45) } else { Color32::from_gray(235) };
    let fg = if dark { Color32::from_gray(200) } else { Color32::from_gray(70) };
    let t = 18.0;
    let top = Rect::from_min_max(Pos2::new(rect.min.x, rect.min.y - t - 2.0), Pos2::new(rect.max.x, rect.min.y - 2.0));
    let left = Rect::from_min_max(Pos2::new(rect.min.x - t - 2.0, rect.min.y), Pos2::new(rect.min.x - 2.0, rect.max.y));
    p.rect_filled(top, 0.0, bg);
    p.rect_filled(left, 0.0, bg);
    let mm_px = pt_to_px * 72.0 / 25.4;
    // label spacing in mm so labels don't collide
    let step = [1.0, 2.0, 5.0, 10.0, 20.0, 50.0, 100.0].into_iter().find(|s| s * mm_px >= 28.0).unwrap_or(100.0);
    let minor = if mm_px >= 4.0 { 1.0 } else if mm_px >= 1.5 { 5.0 } else { 10.0 };
    let font = egui::FontId::proportional(9.5);
    let w_mm = mm(w_pt);
    let h_mm = mm(h_pt);
    let mut v = 0.0;
    while v <= w_mm + 0.01 {
        let x = rect.min.x + v * mm_px;
        let major = (v / step).fract().abs() < 1e-3 || (v / step).fract() > 0.999;
        let len = if major { t * 0.6 } else if (v / 5.0).fract().abs() < 1e-3 { t * 0.4 } else { t * 0.22 };
        p.line_segment([Pos2::new(x, top.max.y - len), Pos2::new(x, top.max.y)], Stroke::new(1.0_f32, fg));
        if major && v > 0.0 {
            p.text(Pos2::new(x + 2.0, top.min.y + 1.0), egui::Align2::LEFT_TOP, format!("{v:.0}"), font.clone(), fg);
        }
        v += minor;
    }
    let mut v = 0.0;
    while v <= h_mm + 0.01 {
        let y = rect.min.y + v * mm_px;
        let major = (v / step).fract().abs() < 1e-3 || (v / step).fract() > 0.999;
        let len = if major { t * 0.6 } else if (v / 5.0).fract().abs() < 1e-3 { t * 0.4 } else { t * 0.22 };
        p.line_segment([Pos2::new(left.max.x - len, y), Pos2::new(left.max.x, y)], Stroke::new(1.0_f32, fg));
        if major && v > 0.0 {
            p.text(Pos2::new(left.min.x + 1.0, y + 1.0), egui::Align2::LEFT_TOP, format!("{v:.0}"), font.clone(), fg);
        }
        v += minor;
    }
    if let Some((x, y)) = hover {
        let mark = Color32::from_rgb(230, 0, 120);
        let sx = rect.min.x + x * pt_to_px;
        let sy = rect.min.y + y * pt_to_px;
        if sx >= rect.min.x && sx <= rect.max.x {
            p.line_segment([Pos2::new(sx, top.min.y), Pos2::new(sx, top.max.y)], Stroke::new(1.0_f32, mark));
        }
        if sy >= rect.min.y && sy <= rect.max.y {
            p.line_segment([Pos2::new(left.min.x, sy), Pos2::new(left.max.x, sy)], Stroke::new(1.0_f32, mark));
        }
    }
    let _ = Vec2::ZERO;
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        #[cfg(debug_assertions)]
        self.run_test_script(ctx);
        if self.applied_dark != Some(self.s.dark) {
            ctx.set_visuals(if self.s.dark { egui::Visuals::dark() } else { egui::Visuals::light() });
            self.applied_dark = Some(self.s.dark);
        }
        self.poll_loading(ctx);
        self.poll_thumbs(ctx);
        self.handle_keys(ctx);
        let (close, size) = ctx.input(|i| (i.viewport().close_requested(), i.viewport().inner_rect.map(|r| r.size())));
        if let Some(s) = size {
            self.s.window = Some((s.x, s.y));
        }
        if close {
            self.s.save();
        }
        egui::TopBottomPanel::top("menu").show(ctx, |ui| {
            self.menu_bar(ui, ctx);
        });
        egui::TopBottomPanel::top("toolbar").show(ctx, |ui| {
            ui.add_space(3.0);
            self.toolbar(ui, ctx);
            ui.add_space(3.0);
        });
        egui::TopBottomPanel::bottom("status").show(ctx, |ui| {
            self.status_bar(ui);
        });
        self.thumbs_panel(ctx);
        let view_bg = if self.s.dark { Color32::from_gray(38) } else { Color32::from_gray(200) };
        egui::CentralPanel::default()
            .frame(egui::Frame::default().fill(if self.tab == Tab::View { view_bg } else { ctx.style().visuals.panel_fill }).inner_margin(6.0))
            .show(ctx, |ui| match self.tab {
                Tab::View => self.view_tab(ui, ctx),
                Tab::Info => self.info_tab(ui),
                Tab::Commands => self.commands_tab(ui),
                Tab::Hex => self.hex_tab(ui),
                Tab::Compare => self.compare_tab(ui, ctx),
                Tab::Warnings => self.warnings_tab(ui),
            });
        self.print_window(ctx);
        self.send_window(ctx);
        self.pjl_window(ctx);
        self.batch_window(ctx);
        self.watch_window(ctx);
        self.about_window(ctx);
    }
}
