//! Remembered settings, stored as simple `key=value` lines.
//!
//! Stored in the user's app-data folder, or next to the program when a
//! `PrintFileViewer.settings` file already exists there (portable use from a USB stick).

use std::path::PathBuf;

#[derive(Clone)]
pub struct Settings {
    pub recent: Vec<PathBuf>,
    pub dark: bool,
    pub window: Option<(f32, f32)>,
    pub thumbs: bool,
    pub rulers: bool,
    pub margins: bool,
    pub last_dir: Option<PathBuf>,
    pub watch_dir: Option<PathBuf>,
    pub keep_dir: Option<PathBuf>,
    pub printer: String,
    pub host: String,
    pub port: u16,
    pub batch_dir: Option<PathBuf>,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            recent: Vec::new(),
            dark: false,
            window: None,
            thumbs: true,
            rulers: false,
            margins: false,
            last_dir: None,
            watch_dir: None,
            keep_dir: None,
            printer: String::new(),
            host: String::new(),
            port: 9100,
            batch_dir: None,
        }
    }
}

const FILE: &str = "PrintFileViewer.settings";

fn location() -> Option<PathBuf> {
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let p = dir.join(FILE);
            if p.exists() {
                return Some(p);
            }
        }
    }
    #[cfg(windows)]
    {
        std::env::var("APPDATA").ok().map(|a| PathBuf::from(a).join("PrintFileViewer").join(FILE))
    }
    #[cfg(target_os = "macos")]
    {
        std::env::var("HOME").ok().map(|h| PathBuf::from(h).join("Library/Application Support/PrintFileViewer").join(FILE))
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        std::env::var("HOME").ok().map(|h| PathBuf::from(h).join(".config/printfileviewer").join(FILE))
    }
}

impl Settings {
    pub fn load() -> Settings {
        let mut s = Settings::default();
        let Some(p) = location() else { return s };
        let Ok(text) = std::fs::read_to_string(p) else { return s };
        for line in text.lines() {
            let Some((k, v)) = line.split_once('=') else { continue };
            let v = v.trim();
            let path = || if v.is_empty() { None } else { Some(PathBuf::from(v)) };
            match k.trim() {
                "recent" => s.recent.push(PathBuf::from(v)),
                "dark" => s.dark = v == "1",
                "window" => {
                    if let Some((a, b)) = v.split_once('x') {
                        if let (Ok(w), Ok(h)) = (a.parse(), b.parse()) {
                            s.window = Some((w, h));
                        }
                    }
                }
                "thumbs" => s.thumbs = v == "1",
                "rulers" => s.rulers = v == "1",
                "margins" => s.margins = v == "1",
                "last_dir" => s.last_dir = path(),
                "watch_dir" => s.watch_dir = path(),
                "keep_dir" => s.keep_dir = path(),
                "batch_dir" => s.batch_dir = path(),
                "printer" => s.printer = v.to_string(),
                "host" => s.host = v.to_string(),
                "port" => s.port = v.parse().unwrap_or(9100),
                _ => {}
            }
        }
        s
    }

    pub fn save(&self) {
        let Some(p) = location() else { return };
        if let Some(d) = p.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        let b = |x: bool| if x { "1" } else { "0" };
        let pp = |x: &Option<PathBuf>| x.as_ref().map(|p| p.display().to_string()).unwrap_or_default();
        let mut out = String::new();
        for r in &self.recent {
            out.push_str(&format!("recent={}\n", r.display()));
        }
        out.push_str(&format!("dark={}\nthumbs={}\nrulers={}\nmargins={}\n", b(self.dark), b(self.thumbs), b(self.rulers), b(self.margins)));
        if let Some((w, h)) = self.window {
            out.push_str(&format!("window={w:.0}x{h:.0}\n"));
        }
        out.push_str(&format!(
            "last_dir={}\nwatch_dir={}\nkeep_dir={}\nbatch_dir={}\nprinter={}\nhost={}\nport={}\n",
            pp(&self.last_dir),
            pp(&self.watch_dir),
            pp(&self.keep_dir),
            pp(&self.batch_dir),
            self.printer,
            self.host,
            self.port
        ));
        let _ = std::fs::write(p, out);
    }

    pub fn add_recent(&mut self, p: &std::path::Path) {
        self.recent.retain(|r| r != p);
        self.recent.insert(0, p.to_path_buf());
        self.recent.truncate(12);
        self.last_dir = p.parent().map(|d| d.to_path_buf());
    }
}
