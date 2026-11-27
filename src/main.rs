#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod analyze;
mod app;
mod doc;
mod emf;
mod fonts;
mod geom;
mod hpgl;
mod icon;
mod imgdec;
mod loader;
#[cfg(target_os = "macos")]
mod macos;
mod output;
mod pcl5;
mod pclxl;
mod pdf;
mod pjl;
mod ps;
mod render;
mod settings;
mod shd;
mod summary;
mod xml;
mod xps;
mod zip;

use eframe::egui;
use std::path::{Path, PathBuf};

fn load_or_exit(p: &str) -> doc::Document {
    loader::load_file(Path::new(p)).unwrap_or_else(|e| {
        eprintln!("{e}");
        std::process::exit(1)
    })
}

const USAGE: &str = "Print File Viewer — command-line use:
  printfileviewer <file>                         open the file in the viewer
  printfileviewer --info <file>                  print the job information
  printfileviewer --text <file>                  print the text found on each page
  printfileviewer --render <file> <out.png> [page] [dpi]
  printfileviewer --pdf <file> <out.pdf>         convert to PDF
  printfileviewer --batch <folder> <out.csv|out.json> [--recursive]
  printfileviewer --send <file> <host[:port]>    send the job unchanged to a network printer";

#[cfg(windows)]
fn attach_console() {
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn AttachConsole(pid: u32) -> i32;
    }
    // show command-line output in the console that started us (cmd / PowerShell)
    unsafe {
        AttachConsole(u32::MAX);
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    #[cfg(windows)]
    if args.get(1).map(|a| a.starts_with("--") || a == "/?" || a == "-h").unwrap_or(false) {
        attach_console();
    }
    let a = |i: usize| args.get(i).map(|s| s.as_str());
    match a(1) {
        Some("--help") | Some("-h") | Some("/?") => {
            println!("{USAGE}");
            return;
        }
        Some("--info") if args.len() >= 3 => {
            print!("{}", app::info_text(&load_or_exit(&args[2])));
            return;
        }
        Some("--text") if args.len() >= 3 => {
            let d = load_or_exit(&args[2]);
            for (k, p) in d.pages.iter().enumerate() {
                println!("--- Page {} ---", k + 1);
                print!("{}", p.plain_text());
            }
            return;
        }
        Some("--pdf") if args.len() >= 4 => {
            let d = load_or_exit(&args[2]);
            let pages: Vec<&doc::Page> = d.pages.iter().collect();
            std::fs::write(&args[3], pdf::write(&pages, &args[2])).expect("write pdf");
            println!("{} pages", pages.len());
            return;
        }
        Some("--render") if args.len() >= 4 => {
            let d = load_or_exit(&args[2]);
            let page: usize = a(4).and_then(|s| s.parse().ok()).unwrap_or(1);
            let dpi: f32 = a(5).and_then(|s| s.parse().ok()).unwrap_or(100.0);
            match d.pages.get(page.saturating_sub(1)).and_then(|p| render::render_page(p, dpi / 72.0)).and_then(|r| render::encode_png(&r)) {
                Some(png) => {
                    std::fs::write(&args[3], png).expect("write png");
                    println!("{} pages; wrote page {page}", d.pages.len());
                }
                None => {
                    eprintln!("no page {page} ({} pages)", d.pages.len());
                    std::process::exit(2)
                }
            }
            for w in &d.warnings {
                eprintln!("warning: {w}");
            }
            return;
        }
        Some("--batch") if args.len() >= 4 => {
            let recursive = args.iter().any(|x| x == "--recursive");
            let files = summary::list_jobs(Path::new(&args[2]), recursive);
            let mut rows = Vec::new();
            for f in &files {
                eprintln!("{}", f.display());
                rows.push(summary::summarise_file(f));
            }
            let out = if args[3].to_ascii_lowercase().ends_with(".json") { summary::to_json(&rows) } else { summary::to_csv(&rows) };
            std::fs::write(&args[3], out).expect("write report");
            println!("{} job(s) written to {}", rows.len(), args[3]);
            return;
        }
        Some("--send") if args.len() >= 4 => {
            let data = std::fs::read(&args[2]).expect("read file");
            let (host, port) = match args[3].rsplit_once(':') {
                Some((h, p)) if p.parse::<u16>().is_ok() => (h.to_string(), p.parse().unwrap()),
                _ => (args[3].clone(), 9100),
            };
            match output::send_tcp(&host, port, &data) {
                Ok(_) => println!("sent {} bytes to {host}:{port}", data.len()),
                Err(e) => {
                    eprintln!("{e}");
                    std::process::exit(1)
                }
            }
            return;
        }
        Some("--make-icon") if args.len() >= 3 => {
            let size: u32 = a(3).and_then(|s| s.parse().ok()).unwrap_or(1024);
            std::fs::write(&args[2], icon::png(size)).expect("write icon");
            return;
        }
        _ => {}
    }

    #[cfg(target_os = "macos")]
    macos::install();
    // Finder may pass "-psn_..." process serial numbers; ignore any option-like args.
    let initial = args.iter().skip(1).find(|a| !a.starts_with('-')).map(PathBuf::from);
    let app_icon = std::sync::Arc::new(egui::IconData { rgba: icon::render(256), width: 256, height: 256 });
    let size = app::App::initial_size();
    let mk_opts = |renderer| eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size(size).with_min_inner_size([640.0, 400.0]).with_title(app::APP_NAME).with_drag_and_drop(true).with_icon(app_icon.clone()),
        renderer,
        ..Default::default()
    };
    let init2 = initial.clone();
    let r = eframe::run_native(app::APP_NAME, mk_opts(eframe::Renderer::Wgpu), Box::new(move |cc| Ok(Box::new(app::App::new(cc, init2)))));
    if r.is_err() {
        // fall back to OpenGL if no Direct3D/Vulkan/Metal adapter is available
        let _ = eframe::run_native(app::APP_NAME, mk_opts(eframe::Renderer::Glow), Box::new(move |cc| Ok(Box::new(app::App::new(cc, initial)))));
    }
}
