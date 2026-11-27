//! Sending jobs to printers: printing the rendered pages, sending the original
//! job bytes unchanged to an installed printer queue, or to a network printer's
//! raw port (9100).

use crate::doc::Page;
use std::io::Write;
use std::time::Duration;

/// Installed printers, default first.
pub fn printers() -> Vec<String> {
    imp::printers()
}

/// Send `data` unchanged to a network printer's raw port.
pub fn send_tcp(host: &str, port: u16, data: &[u8]) -> Result<(), String> {
    use std::net::ToSocketAddrs;
    let addrs: Vec<_> = (host, port).to_socket_addrs().map_err(|e| format!("Can't find {host}: {e}"))?.collect();
    let addr = addrs.first().ok_or_else(|| format!("Can't find {host}"))?;
    let mut s = std::net::TcpStream::connect_timeout(addr, Duration::from_secs(8)).map_err(|e| format!("Can't connect to {host}:{port}: {e}"))?;
    s.set_write_timeout(Some(Duration::from_secs(60))).ok();
    s.write_all(data).map_err(|e| format!("Sending failed: {e}"))?;
    s.flush().ok();
    s.shutdown(std::net::Shutdown::Write).ok();
    Ok(())
}

/// Send `data` unchanged (RAW) to an installed printer queue.
pub fn send_raw(printer: &str, doc_name: &str, data: &[u8]) -> Result<(), String> {
    imp::send_raw(printer, doc_name, data)
}

/// Print rendered pages on an installed printer.
pub fn print_pages(printer: &str, doc_name: &str, pages: &[&Page], copies: u32, fit: bool) -> Result<(), String> {
    imp::print_pages(printer, doc_name, pages, copies.max(1), fit)
}

#[cfg(windows)]
mod imp {
    use super::*;
    use std::ffi::c_void;
    type Handle = *mut c_void;

    #[repr(C)]
    struct PrinterInfo4 {
        name: *mut u16,
        server: *mut u16,
        attributes: u32,
    }
    #[repr(C)]
    struct DocInfo1 {
        doc_name: *const u16,
        output_file: *const u16,
        datatype: *const u16,
    }
    #[repr(C)]
    struct DocInfoW {
        cb_size: i32,
        doc_name: *const u16,
        output: *const u16,
        datatype: *const u16,
        fw_type: u32,
    }
    #[repr(C)]
    struct BitmapInfoHeader {
        size: u32,
        width: i32,
        height: i32,
        planes: u16,
        bit_count: u16,
        compression: u32,
        size_image: u32,
        xppm: i32,
        yppm: i32,
        clr_used: u32,
        clr_important: u32,
    }

    #[link(name = "winspool")]
    unsafe extern "system" {
        fn EnumPrintersW(flags: u32, name: *const u16, level: u32, buf: *mut u8, cb: u32, needed: *mut u32, returned: *mut u32) -> i32;
        fn GetDefaultPrinterW(buf: *mut u16, size: *mut u32) -> i32;
        fn OpenPrinterW(name: *const u16, h: *mut Handle, def: *const c_void) -> i32;
        fn StartDocPrinterW(h: Handle, level: u32, info: *const DocInfo1) -> u32;
        fn StartPagePrinter(h: Handle) -> i32;
        fn WritePrinter(h: Handle, buf: *const c_void, cb: u32, written: *mut u32) -> i32;
        fn EndPagePrinter(h: Handle) -> i32;
        fn EndDocPrinter(h: Handle) -> i32;
        fn ClosePrinter(h: Handle) -> i32;
    }
    #[link(name = "gdi32")]
    unsafe extern "system" {
        fn CreateDCW(driver: *const u16, device: *const u16, port: *const u16, dm: *const c_void) -> Handle;
        fn DeleteDC(h: Handle) -> i32;
        fn StartDocW(h: Handle, di: *const DocInfoW) -> i32;
        fn EndDoc(h: Handle) -> i32;
        fn StartPage(h: Handle) -> i32;
        fn EndPage(h: Handle) -> i32;
        fn GetDeviceCaps(h: Handle, idx: i32) -> i32;
        fn SetStretchBltMode(h: Handle, mode: i32) -> i32;
        fn StretchDIBits(h: Handle, xd: i32, yd: i32, wd: i32, hd: i32, xs: i32, ys: i32, ws: i32, hs: i32, bits: *const c_void, bmi: *const BitmapInfoHeader, usage: u32, rop: u32) -> i32;
    }

    fn w(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }
    unsafe fn from_w(p: *const u16) -> String {
        if p.is_null() {
            return String::new();
        }
        let mut n = 0;
        unsafe {
            while *p.add(n) != 0 {
                n += 1;
            }
            String::from_utf16_lossy(std::slice::from_raw_parts(p, n))
        }
    }

    pub fn printers() -> Vec<String> {
        let mut out = Vec::new();
        unsafe {
            let mut need = 0u32;
            let mut n = 0u32;
            let flags = 0x2 | 0x4; // LOCAL | CONNECTIONS
            EnumPrintersW(flags, std::ptr::null(), 4, std::ptr::null_mut(), 0, &mut need, &mut n);
            if need > 0 {
                let mut buf = vec![0u8; need as usize];
                if EnumPrintersW(flags, std::ptr::null(), 4, buf.as_mut_ptr(), need, &mut need, &mut n) != 0 {
                    let arr = buf.as_ptr() as *const PrinterInfo4;
                    for i in 0..n as usize {
                        out.push(from_w((*arr.add(i)).name));
                    }
                }
            }
            let mut sz = 512u32;
            let mut b = vec![0u16; 512];
            if GetDefaultPrinterW(b.as_mut_ptr(), &mut sz) != 0 {
                let d = from_w(b.as_ptr());
                if let Some(i) = out.iter().position(|p| *p == d) {
                    let p = out.remove(i);
                    out.insert(0, p);
                }
            }
        }
        out
    }

    pub fn send_raw(printer: &str, doc_name: &str, data: &[u8]) -> Result<(), String> {
        unsafe {
            let name = w(printer);
            let mut h: Handle = std::ptr::null_mut();
            if OpenPrinterW(name.as_ptr(), &mut h, std::ptr::null()) == 0 {
                return Err(format!("Can't open printer \"{printer}\""));
            }
            let dn = w(doc_name);
            let dt = w("RAW");
            let info = DocInfo1 { doc_name: dn.as_ptr(), output_file: std::ptr::null(), datatype: dt.as_ptr() };
            let r = (|| {
                if StartDocPrinterW(h, 1, &info) == 0 {
                    return Err("The printer refused the job (StartDocPrinter failed)".to_string());
                }
                StartPagePrinter(h);
                let mut off = 0;
                while off < data.len() {
                    let chunk = (data.len() - off).min(1 << 20);
                    let mut written = 0u32;
                    if WritePrinter(h, data[off..].as_ptr() as *const c_void, chunk as u32, &mut written) == 0 || written == 0 {
                        EndPagePrinter(h);
                        EndDocPrinter(h);
                        return Err("Writing to the printer failed".to_string());
                    }
                    off += written as usize;
                }
                EndPagePrinter(h);
                EndDocPrinter(h);
                Ok(())
            })();
            ClosePrinter(h);
            r
        }
    }

    pub fn print_pages(printer: &str, doc_name: &str, pages: &[&Page], copies: u32, fit: bool) -> Result<(), String> {
        unsafe {
            let drv = w("WINSPOOL");
            let dev = w(printer);
            let dc = CreateDCW(drv.as_ptr(), dev.as_ptr(), std::ptr::null(), std::ptr::null());
            if dc.is_null() {
                return Err(format!("Can't open printer \"{printer}\""));
            }
            let dn = w(doc_name);
            let di = DocInfoW { cb_size: std::mem::size_of::<DocInfoW>() as i32, doc_name: dn.as_ptr(), output: std::ptr::null(), datatype: std::ptr::null(), fw_type: 0 };
            if StartDocW(dc, &di) <= 0 {
                DeleteDC(dc);
                return Err("The printer refused the job".into());
            }
            let dpix = GetDeviceCaps(dc, 88).max(72) as f32;
            let dpiy = GetDeviceCaps(dc, 90).max(72) as f32;
            let offx = GetDeviceCaps(dc, 112) as f32;
            let offy = GetDeviceCaps(dc, 113) as f32;
            let hres = GetDeviceCaps(dc, 8) as f32;
            let vres = GetDeviceCaps(dc, 10) as f32;
            SetStretchBltMode(dc, 4);
            let mut result = Ok(());
            'outer: for _ in 0..copies {
                for p in pages {
                    let dpi = 300f32.min(dpix);
                    let Some(r) = crate::render::render_page(p, dpi / 72.0) else { continue };
                    let mut bgra = r.rgba.clone();
                    for px in bgra.chunks_exact_mut(4) {
                        px.swap(0, 2);
                    }
                    let (mut dw, mut dh) = (p.width / 72.0 * dpix, p.height / 72.0 * dpiy);
                    let (mut dx, mut dy) = (-offx, -offy);
                    if fit {
                        let k = (hres / dw).min(vres / dh);
                        dw *= k;
                        dh *= k;
                        dx = (hres - dw) / 2.0;
                        dy = (vres - dh) / 2.0;
                    }
                    if StartPage(dc) <= 0 {
                        result = Err("Printing failed (StartPage)".into());
                        break 'outer;
                    }
                    let bmi = BitmapInfoHeader { size: 40, width: r.width as i32, height: -(r.height as i32), planes: 1, bit_count: 32, compression: 0, size_image: 0, xppm: 0, yppm: 0, clr_used: 0, clr_important: 0 };
                    StretchDIBits(dc, dx as i32, dy as i32, dw as i32, dh as i32, 0, 0, r.width as i32, r.height as i32, bgra.as_ptr() as *const c_void, &bmi, 0, 0x00CC0020);
                    EndPage(dc);
                }
            }
            EndDoc(dc);
            DeleteDC(dc);
            result
        }
    }
}

#[cfg(not(windows))]
mod imp {
    use super::*;
    use std::process::Command;

    pub fn printers() -> Vec<String> {
        let mut out = Vec::new();
        if let Ok(o) = Command::new("lpstat").arg("-e").output() {
            for l in String::from_utf8_lossy(&o.stdout).lines() {
                let l = l.trim();
                if !l.is_empty() {
                    out.push(l.to_string());
                }
            }
        }
        if out.is_empty() {
            if let Ok(o) = Command::new("lpstat").arg("-p").output() {
                for l in String::from_utf8_lossy(&o.stdout).lines() {
                    if let Some(r) = l.strip_prefix("printer ") {
                        if let Some(n) = r.split_whitespace().next() {
                            out.push(n.to_string());
                        }
                    }
                }
            }
        }
        if let Ok(o) = Command::new("lpstat").arg("-d").output() {
            let s = String::from_utf8_lossy(&o.stdout).to_string();
            if let Some(d) = s.split(':').nth(1).map(|x| x.trim().to_string()) {
                if let Some(i) = out.iter().position(|p| *p == d) {
                    let p = out.remove(i);
                    out.insert(0, p);
                }
            }
        }
        out
    }

    fn temp_file(ext: &str, data: &[u8]) -> Result<std::path::PathBuf, String> {
        let p = std::env::temp_dir().join(format!("printfileviewer-{}-{}.{ext}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0)));
        std::fs::write(&p, data).map_err(|e| format!("Can't write a temporary file: {e}"))?;
        Ok(p)
    }

    fn lp(args: &[&str], file: &std::path::Path) -> Result<(), String> {
        let o = Command::new("lp").args(args).arg(file).output().map_err(|e| format!("Can't run lp: {e}"))?;
        let _ = std::fs::remove_file(file);
        if o.status.success() {
            Ok(())
        } else {
            Err(String::from_utf8_lossy(&o.stderr).trim().to_string())
        }
    }

    pub fn send_raw(printer: &str, doc_name: &str, data: &[u8]) -> Result<(), String> {
        let f = temp_file("prn", data)?;
        lp(&["-d", printer, "-o", "raw", "-t", doc_name], &f)
    }

    pub fn print_pages(printer: &str, doc_name: &str, pages: &[&Page], copies: u32, fit: bool) -> Result<(), String> {
        let pdf = crate::pdf::write(pages, doc_name);
        let f = temp_file("pdf", &pdf)?;
        let n = copies.to_string();
        let mut args = vec!["-d", printer, "-n", &n, "-t", doc_name];
        if fit {
            args.extend(["-o", "fit-to-page"]);
        }
        lp(&args, &f)
    }
}
