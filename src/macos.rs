//! macOS integration: receive files opened from Finder (double-click, "Open With",
//! dropping onto the Dock icon). macOS delivers these as Apple Events to the
//! application delegate's `application:openURLs:` method rather than as
//! command-line arguments. winit's delegate doesn't implement it, so we add the
//! method to NSObject via the Objective-C runtime before the event loop starts.

use std::ffi::{c_char, c_void, CStr};
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

type Id = *mut c_void;
type Sel = *mut c_void;

#[link(name = "objc", kind = "dylib")]
unsafe extern "C" {
    fn objc_getClass(name: *const c_char) -> Id;
    fn sel_registerName(name: *const c_char) -> Sel;
    fn class_addMethod(cls: Id, sel: Sel, imp: *const c_void, types: *const c_char) -> bool;
    fn objc_msgSend();
}

#[link(name = "Foundation", kind = "framework")]
unsafe extern "C" {}

static OPENED: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());
static CTX: OnceLock<eframe::egui::Context> = OnceLock::new();

unsafe fn sel(name: &CStr) -> Sel {
    unsafe { sel_registerName(name.as_ptr()) }
}

extern "C" fn open_urls(_this: Id, _cmd: Sel, _app: Id, urls: Id) {
    unsafe {
        let send_usize: unsafe extern "C" fn(Id, Sel) -> usize = std::mem::transmute(objc_msgSend as *const c_void);
        let send_idx: unsafe extern "C" fn(Id, Sel, usize) -> Id = std::mem::transmute(objc_msgSend as *const c_void);
        let send_id: unsafe extern "C" fn(Id, Sel) -> Id = std::mem::transmute(objc_msgSend as *const c_void);
        let send_cstr: unsafe extern "C" fn(Id, Sel) -> *const c_char = std::mem::transmute(objc_msgSend as *const c_void);
        let n = send_usize(urls, sel(c"count"));
        for i in 0..n {
            let url = send_idx(urls, sel(c"objectAtIndex:"), i);
            if url.is_null() {
                continue;
            }
            let path = send_id(url, sel(c"path"));
            if path.is_null() {
                continue;
            }
            let s = send_cstr(path, sel(c"UTF8String"));
            if !s.is_null() {
                let p = PathBuf::from(CStr::from_ptr(s).to_string_lossy().to_string());
                OPENED.lock().unwrap().push(p);
            }
        }
    }
    if let Some(ctx) = CTX.get() {
        ctx.request_repaint();
    }
}

/// Install the handler. Call before the event loop is created.
pub fn install() {
    unsafe {
        let cls = objc_getClass(c"NSObject".as_ptr());
        if !cls.is_null() {
            class_addMethod(cls, sel(c"application:openURLs:"), open_urls as *const c_void, c"v@:@@".as_ptr());
        }
    }
}

pub fn set_context(ctx: &eframe::egui::Context) {
    let _ = CTX.set(ctx.clone());
}

/// Files opened from Finder since the last call.
pub fn take_opened() -> Vec<PathBuf> {
    std::mem::take(&mut *OPENED.lock().unwrap())
}
