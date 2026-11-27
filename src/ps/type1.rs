//! Type 1 charstring decryption and interpretation.

use crate::geom::PathAcc;

pub fn decrypt(data: &[u8], key: u16, skip: usize) -> Vec<u8> {
    let mut r = key;
    let mut out = Vec::with_capacity(data.len());
    for &c in data {
        out.push(c ^ (r >> 8) as u8);
        r = (c as u16).wrapping_add(r).wrapping_mul(52845).wrapping_add(22719);
    }
    if skip <= out.len() {
        out.drain(..skip);
    } else {
        out.clear();
    }
    out
}

pub struct T1Ctx<'a> {
    pub subrs: &'a dyn Fn(usize) -> Option<Vec<u8>>,
    /// Look up a StandardEncoding glyph for `seac`.
    pub seac: &'a dyn Fn(u8) -> Option<Vec<u8>>,
    pub len_iv: i64,
}

pub struct T1Out {
    pub path: PathAcc,
    pub width: (f64, f64),
}

struct St {
    stack: Vec<f64>,
    ps: Vec<f64>,
    x: f64,
    y: f64,
    sbx: f64,
    sby: f64,
    flex: Option<Vec<(f64, f64)>>,
    done: bool,
    width: (f64, f64),
    origin: (f64, f64),
    open: bool,
}

pub fn run(cs: &[u8], ctx: &T1Ctx, encrypted: bool) -> T1Out {
    let mut path = PathAcc::new();
    let mut st = St { stack: Vec::new(), ps: Vec::new(), x: 0.0, y: 0.0, sbx: 0.0, sby: 0.0, flex: None, done: false, width: (0.0, 0.0), origin: (0.0, 0.0), open: false };
    let data = if encrypted && ctx.len_iv >= 0 { decrypt(cs, 4330, ctx.len_iv as usize) } else { cs.to_vec() };
    exec(&data, ctx, &mut st, &mut path, 0);
    T1Out { path, width: st.width }
}

fn decode_sub(ctx: &T1Ctx, d: Vec<u8>) -> Vec<u8> {
    if ctx.len_iv >= 0 {
        decrypt(&d, 4330, ctx.len_iv as usize)
    } else {
        d
    }
}

fn exec(cs: &[u8], ctx: &T1Ctx, st: &mut St, path: &mut PathAcc, depth: u32) {
    if depth > 20 {
        return;
    }
    let mut i = 0;
    while i < cs.len() && !st.done {
        let v = cs[i];
        i += 1;
        if v >= 32 {
            let n = if v <= 246 {
                v as f64 - 139.0
            } else if v <= 250 {
                let w = *cs.get(i).unwrap_or(&0) as f64;
                i += 1;
                (v as f64 - 247.0) * 256.0 + w + 108.0
            } else if v <= 254 {
                let w = *cs.get(i).unwrap_or(&0) as f64;
                i += 1;
                -(v as f64 - 251.0) * 256.0 - w - 108.0
            } else {
                if i + 4 > cs.len() {
                    return;
                }
                let n = i32::from_be_bytes([cs[i], cs[i + 1], cs[i + 2], cs[i + 3]]) as f64;
                i += 4;
                n
            };
            st.stack.push(n);
            continue;
        }
        let s = &mut st.stack;
        let arg = |s: &Vec<f64>, k: usize| s.get(k).copied().unwrap_or(0.0);
        match v {
            1 | 3 => s.clear(), // hstem / vstem
            4 => {
                let dy = arg(s, s.len().saturating_sub(1));
                s.clear();
                rmove(st, path, 0.0, dy);
            }
            5 => {
                let (dx, dy) = (arg(s, 0), arg(s, 1));
                s.clear();
                line(st, path, dx, dy);
            }
            6 => {
                let dx = arg(s, 0);
                s.clear();
                line(st, path, dx, 0.0);
            }
            7 => {
                let dy = arg(s, 0);
                s.clear();
                line(st, path, 0.0, dy);
            }
            8 => {
                let a: Vec<f64> = (0..6).map(|k| arg(s, k)).collect();
                s.clear();
                curve(st, path, a[0], a[1], a[2], a[3], a[4], a[5]);
            }
            9 => {
                s.clear();
                if st.open {
                    path.close();
                    st.open = false;
                }
            }
            10 => {
                let n = s.pop().unwrap_or(0.0) as usize;
                if let Some(sub) = (ctx.subrs)(n) {
                    let sub = decode_sub(ctx, sub);
                    exec(&sub, ctx, st, path, depth + 1);
                }
            }
            11 => return,
            13 => {
                let (sbx, wx) = (arg(s, 0), arg(s, 1));
                s.clear();
                st.sbx = sbx + st.origin.0;
                st.x = st.sbx;
                st.y = st.origin.1;
                st.width = (wx, 0.0);
            }
            14 => {
                s.clear();
                if st.open {
                    path.close();
                    st.open = false;
                }
                st.done = true;
            }
            21 => {
                let (dx, dy) = (arg(s, 0), arg(s, 1));
                s.clear();
                rmove(st, path, dx, dy);
            }
            22 => {
                let dx = arg(s, 0);
                s.clear();
                rmove(st, path, dx, 0.0);
            }
            30 => {
                let a: Vec<f64> = (0..4).map(|k| arg(s, k)).collect();
                s.clear();
                curve(st, path, 0.0, a[0], a[1], a[2], a[3], 0.0);
            }
            31 => {
                let a: Vec<f64> = (0..4).map(|k| arg(s, k)).collect();
                s.clear();
                curve(st, path, a[0], 0.0, a[1], a[2], 0.0, a[3]);
            }
            12 => {
                let e = *cs.get(i).unwrap_or(&0);
                i += 1;
                match e {
                    0 | 1 | 2 => st.stack.clear(),
                    6 => {
                        // seac
                        let a: Vec<f64> = (0..5).map(|k| arg(&st.stack, k)).collect();
                        st.stack.clear();
                        let (asb, adx, ady, bchar, achar) = (a[0], a[1], a[2], a[3] as u8, a[4] as u8);
                        let saved_w = st.width;
                        let origin = st.origin;
                        if let Some(b) = (ctx.seac)(bchar) {
                            let b = decode_sub(ctx, b);
                            st.done = false;
                            exec(&b, ctx, st, path, depth + 1);
                        }
                        if let Some(ac) = (ctx.seac)(achar) {
                            let ac = decode_sub(ctx, ac);
                            st.done = false;
                            st.origin = (origin.0 + adx - asb, origin.1 + ady);
                            exec(&ac, ctx, st, path, depth + 1);
                            st.origin = origin;
                        }
                        st.width = saved_w;
                        st.done = true;
                    }
                    7 => {
                        let a: Vec<f64> = (0..4).map(|k| arg(&st.stack, k)).collect();
                        st.stack.clear();
                        st.sbx = a[0] + st.origin.0;
                        st.sby = a[1] + st.origin.1;
                        st.x = st.sbx;
                        st.y = st.sby;
                        st.width = (a[2], a[3]);
                    }
                    12 => {
                        let b = st.stack.pop().unwrap_or(1.0);
                        let a = st.stack.pop().unwrap_or(0.0);
                        st.stack.push(if b != 0.0 { a / b } else { 0.0 });
                    }
                    16 => {
                        let othersubr = st.stack.pop().unwrap_or(0.0) as i64;
                        let n = st.stack.pop().unwrap_or(0.0) as usize;
                        let mut args = Vec::new();
                        for _ in 0..n {
                            args.push(st.stack.pop().unwrap_or(0.0));
                        }
                        // args are in reverse order (last pushed first)
                        match othersubr {
                            1 => {
                                st.flex = Some(Vec::new());
                            }
                            2 => {
                                if let Some(f) = st.flex.as_mut() {
                                    f.push((st.x, st.y));
                                }
                            }
                            0 => {
                                if let Some(f) = st.flex.take() {
                                    // f[0] is reference point; f[1..7] are curve points
                                    if f.len() >= 7 {
                                        let p = &f[1..7];
                                        path.curve_to(p[0].0, p[0].1, p[1].0, p[1].1, p[2].0, p[2].1);
                                        path.curve_to(p[3].0, p[3].1, p[4].0, p[4].1, p[5].0, p[5].1);
                                        st.x = p[5].0;
                                        st.y = p[5].1;
                                    }
                                }
                                // push back end point for the following `pop pop setcurrentpoint`
                                st.ps.push(st.y);
                                st.ps.push(st.x);
                            }
                            _ => {
                                for a in args {
                                    st.ps.push(a);
                                }
                            }
                        }
                    }
                    17 => {
                        let v = st.ps.pop().unwrap_or(0.0);
                        st.stack.push(v);
                    }
                    33 => {
                        let y = st.stack.pop().unwrap_or(0.0);
                        let x = st.stack.pop().unwrap_or(0.0);
                        st.stack.clear();
                        st.x = x + st.origin.0;
                        st.y = y + st.origin.1;
                    }
                    _ => st.stack.clear(),
                }
            }
            _ => s.clear(),
        }
    }
}

fn rmove(st: &mut St, path: &mut PathAcc, dx: f64, dy: f64) {
    st.x += dx;
    st.y += dy;
    if let Some(f) = st.flex.as_mut() {
        // in flex mode, moves just record points (via othersubr 2)
        let _ = f;
        return;
    }
    if st.open {
        path.close();
    }
    path.move_to(st.x, st.y);
    st.open = true;
}

fn line(st: &mut St, path: &mut PathAcc, dx: f64, dy: f64) {
    st.x += dx;
    st.y += dy;
    if !st.open {
        path.move_to(st.x - dx, st.y - dy);
        st.open = true;
    }
    path.line_to(st.x, st.y);
}

#[allow(clippy::too_many_arguments)]
fn curve(st: &mut St, path: &mut PathAcc, dx1: f64, dy1: f64, dx2: f64, dy2: f64, dx3: f64, dy3: f64) {
    let x1 = st.x + dx1;
    let y1 = st.y + dy1;
    let x2 = x1 + dx2;
    let y2 = y1 + dy2;
    let x3 = x2 + dx3;
    let y3 = y2 + dy3;
    if !st.open {
        path.move_to(st.x, st.y);
        st.open = true;
    }
    path.curve_to(x1, y1, x2, y2, x3, y3);
    st.x = x3;
    st.y = y3;
}
