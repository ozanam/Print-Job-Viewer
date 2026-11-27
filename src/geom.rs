//! Shared geometry: affine matrices and device-space path segments.

use tiny_skia::{Path, PathBuilder, Transform};

/// Affine matrix in PostScript order: x' = a*x + c*y + e, y' = b*x + d*y + f.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Matrix {
    pub a: f64,
    pub b: f64,
    pub c: f64,
    pub d: f64,
    pub e: f64,
    pub f: f64,
}

impl Default for Matrix {
    fn default() -> Self {
        Matrix::identity()
    }
}

impl Matrix {
    pub const fn identity() -> Self {
        Matrix { a: 1.0, b: 0.0, c: 0.0, d: 1.0, e: 0.0, f: 0.0 }
    }
    pub const fn new(a: f64, b: f64, c: f64, d: f64, e: f64, f: f64) -> Self {
        Matrix { a, b, c, d, e, f }
    }
    pub fn from_array(v: &[f64]) -> Self {
        Matrix::new(v[0], v[1], v[2], v[3], v[4], v[5])
    }
    pub fn to_array(&self) -> [f64; 6] {
        [self.a, self.b, self.c, self.d, self.e, self.f]
    }
    pub fn translate(tx: f64, ty: f64) -> Self {
        Matrix::new(1.0, 0.0, 0.0, 1.0, tx, ty)
    }
    pub fn scale(sx: f64, sy: f64) -> Self {
        Matrix::new(sx, 0.0, 0.0, sy, 0.0, 0.0)
    }
    pub fn rotate_deg(deg: f64) -> Self {
        let r = deg.to_radians();
        let (s, c) = r.sin_cos();
        Matrix::new(c, s, -s, c, 0.0, 0.0)
    }
    /// Returns `self` followed by `other` (apply self first, then other).
    /// This is PostScript's `self other concatmatrix`.
    pub fn then(&self, o: &Matrix) -> Matrix {
        Matrix {
            a: self.a * o.a + self.b * o.c,
            b: self.a * o.b + self.b * o.d,
            c: self.c * o.a + self.d * o.c,
            d: self.c * o.b + self.d * o.d,
            e: self.e * o.a + self.f * o.c + o.e,
            f: self.e * o.b + self.f * o.d + o.f,
        }
    }
    pub fn apply(&self, x: f64, y: f64) -> (f64, f64) {
        (self.a * x + self.c * y + self.e, self.b * x + self.d * y + self.f)
    }
    pub fn apply_delta(&self, x: f64, y: f64) -> (f64, f64) {
        (self.a * x + self.c * y, self.b * x + self.d * y)
    }
    pub fn det(&self) -> f64 {
        self.a * self.d - self.b * self.c
    }
    pub fn invert(&self) -> Option<Matrix> {
        let det = self.det();
        if det.abs() < 1e-12 || !det.is_finite() {
            return None;
        }
        let a = self.d / det;
        let b = -self.b / det;
        let c = -self.c / det;
        let d = self.a / det;
        let e = -(self.e * a + self.f * c);
        let f = -(self.e * b + self.f * d);
        Some(Matrix { a, b, c, d, e, f })
    }
    pub fn to_ts(&self) -> Transform {
        Transform::from_row(
            self.a as f32,
            self.b as f32,
            self.c as f32,
            self.d as f32,
            self.e as f32,
            self.f as f32,
        )
    }
    /// Approximate uniform scale factor of the matrix.
    pub fn mean_scale(&self) -> f64 {
        self.det().abs().sqrt()
    }
}

#[derive(Clone, Copy, Debug)]
pub enum Seg {
    Move(f64, f64),
    Line(f64, f64),
    Cubic(f64, f64, f64, f64, f64, f64),
    Close,
}

/// A path accumulated in some coordinate space (usually device space).
#[derive(Clone, Debug, Default)]
pub struct PathAcc {
    pub segs: Vec<Seg>,
    pub cur: Option<(f64, f64)>,
    pub start: Option<(f64, f64)>,
}

impl PathAcc {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn clear(&mut self) {
        self.segs.clear();
        self.cur = None;
        self.start = None;
    }
    pub fn is_empty(&self) -> bool {
        self.segs.is_empty()
    }
    pub fn move_to(&mut self, x: f64, y: f64) {
        if let Some(Seg::Move(..)) = self.segs.last() {
            self.segs.pop();
        }
        self.segs.push(Seg::Move(x, y));
        self.cur = Some((x, y));
        self.start = Some((x, y));
    }
    pub fn line_to(&mut self, x: f64, y: f64) {
        if self.cur.is_none() {
            self.move_to(x, y);
            return;
        }
        self.segs.push(Seg::Line(x, y));
        self.cur = Some((x, y));
    }
    pub fn curve_to(&mut self, x1: f64, y1: f64, x2: f64, y2: f64, x: f64, y: f64) {
        if self.cur.is_none() {
            self.move_to(x1, y1);
        }
        self.segs.push(Seg::Cubic(x1, y1, x2, y2, x, y));
        self.cur = Some((x, y));
    }
    pub fn close(&mut self) {
        if self.cur.is_some() && !matches!(self.segs.last(), Some(Seg::Close) | None) {
            self.segs.push(Seg::Close);
            self.cur = self.start;
        }
    }
    pub fn append(&mut self, other: &PathAcc) {
        self.segs.extend_from_slice(&other.segs);
        if other.cur.is_some() {
            self.cur = other.cur;
            self.start = other.start;
        }
    }
    pub fn transformed(&self, m: &Matrix) -> PathAcc {
        let segs = self
            .segs
            .iter()
            .map(|s| match *s {
                Seg::Move(x, y) => {
                    let (x, y) = m.apply(x, y);
                    Seg::Move(x, y)
                }
                Seg::Line(x, y) => {
                    let (x, y) = m.apply(x, y);
                    Seg::Line(x, y)
                }
                Seg::Cubic(a, b, c, d, e, f) => {
                    let (a, b) = m.apply(a, b);
                    let (c, d) = m.apply(c, d);
                    let (e, f) = m.apply(e, f);
                    Seg::Cubic(a, b, c, d, e, f)
                }
                Seg::Close => Seg::Close,
            })
            .collect();
        PathAcc {
            segs,
            cur: self.cur.map(|(x, y)| m.apply(x, y)),
            start: self.start.map(|(x, y)| m.apply(x, y)),
        }
    }
    pub fn bbox(&self) -> Option<(f64, f64, f64, f64)> {
        let mut b: Option<(f64, f64, f64, f64)> = None;
        let mut add = |x: f64, y: f64| {
            b = Some(match b {
                None => (x, y, x, y),
                Some((x0, y0, x1, y1)) => (x0.min(x), y0.min(y), x1.max(x), y1.max(y)),
            })
        };
        for s in &self.segs {
            match *s {
                Seg::Move(x, y) | Seg::Line(x, y) => add(x, y),
                Seg::Cubic(a, b2, c, d, e, f) => {
                    add(a, b2);
                    add(c, d);
                    add(e, f);
                }
                Seg::Close => {}
            }
        }
        b
    }
    /// Build a tiny-skia path, applying `m` to every point.
    pub fn to_path(&self, m: &Matrix) -> Option<Path> {
        let mut pb = PathBuilder::new();
        let mut open = false;
        for s in &self.segs {
            match *s {
                Seg::Move(x, y) => {
                    let (x, y) = m.apply(x, y);
                    if !(x.is_finite() && y.is_finite()) {
                        return None;
                    }
                    pb.move_to(x as f32, y as f32);
                    open = true;
                }
                Seg::Line(x, y) => {
                    let (x, y) = m.apply(x, y);
                    if open {
                        pb.line_to(x as f32, y as f32);
                    }
                }
                Seg::Cubic(a, b, c, d, e, f) => {
                    let (a, b) = m.apply(a, b);
                    let (c, d) = m.apply(c, d);
                    let (e, f) = m.apply(e, f);
                    if open {
                        pb.cubic_to(a as f32, b as f32, c as f32, d as f32, e as f32, f as f32);
                    }
                }
                Seg::Close => {
                    if open {
                        pb.close();
                    }
                }
            }
        }
        pb.finish()
    }

    /// Flatten curves to lines.
    pub fn flattened(&self) -> PathAcc {
        let mut out = PathAcc::new();
        let mut cur = (0.0, 0.0);
        for s in &self.segs {
            match *s {
                Seg::Move(x, y) => {
                    out.move_to(x, y);
                    cur = (x, y);
                }
                Seg::Line(x, y) => {
                    out.line_to(x, y);
                    cur = (x, y);
                }
                Seg::Cubic(x1, y1, x2, y2, x, y) => {
                    let n = 16;
                    for i in 1..=n {
                        let t = i as f64 / n as f64;
                        let mt = 1.0 - t;
                        let px = mt * mt * mt * cur.0 + 3.0 * mt * mt * t * x1 + 3.0 * mt * t * t * x2 + t * t * t * x;
                        let py = mt * mt * mt * cur.1 + 3.0 * mt * mt * t * y1 + 3.0 * mt * t * t * y2 + t * t * t * y;
                        out.line_to(px, py);
                    }
                    cur = (x, y);
                }
                Seg::Close => out.close(),
            }
        }
        out
    }

    /// Append an elliptical/circular arc (in the path's own space) using `m`
    /// to map from arc space. Angles in degrees, counter-clockwise when `ccw`
    /// in the arc space's y-up convention.
    pub fn arc(&mut self, m: &Matrix, cx: f64, cy: f64, r: f64, a1: f64, a2: f64, ccw: bool, connect: bool) {
        let mut a1 = a1;
        let mut a2 = a2;
        if ccw {
            while a2 < a1 {
                a2 += 360.0;
            }
        } else {
            while a2 > a1 {
                a2 -= 360.0;
            }
        }
        let (sx, sy) = m.apply(cx + r * a1.to_radians().cos(), cy + r * a1.to_radians().sin());
        if connect && self.cur.is_some() {
            self.line_to(sx, sy);
        } else {
            self.move_to(sx, sy);
        }
        let total = a2 - a1;
        let n = ((total.abs() / 90.0).ceil() as usize).max(1);
        let step = total / n as f64;
        for _ in 0..n {
            let t1 = a1.to_radians();
            let t2 = (a1 + step).to_radians();
            let k = 4.0 / 3.0 * ((t2 - t1) / 4.0).tan();
            let (s1, c1) = t1.sin_cos();
            let (s2, c2) = t2.sin_cos();
            let p1 = (cx + r * (c1 - k * s1), cy + r * (s1 + k * c1));
            let p2 = (cx + r * (c2 + k * s2), cy + r * (s2 - k * c2));
            let p3 = (cx + r * c2, cy + r * s2);
            let (x1, y1) = m.apply(p1.0, p1.1);
            let (x2, y2) = m.apply(p2.0, p2.1);
            let (x3, y3) = m.apply(p3.0, p3.1);
            self.curve_to(x1, y1, x2, y2, x3, y3);
            a1 += step;
        }
    }

    /// Reverse each subpath.
    pub fn reversed(&self) -> PathAcc {
        let mut out = PathAcc::new();
        let mut i = 0;
        let segs = &self.segs;
        while i < segs.len() {
            // collect a subpath
            let Seg::Move(sx, sy) = segs[i] else {
                i += 1;
                continue;
            };
            let mut j = i + 1;
            let mut pts = vec![(sx, sy)];
            let mut kinds: Vec<Seg> = Vec::new();
            let mut closed = false;
            while j < segs.len() {
                match segs[j] {
                    Seg::Move(..) => break,
                    Seg::Line(x, y) => {
                        kinds.push(segs[j]);
                        pts.push((x, y));
                    }
                    Seg::Cubic(_, _, _, _, x, y) => {
                        kinds.push(segs[j]);
                        pts.push((x, y));
                    }
                    Seg::Close => {
                        closed = true;
                        j += 1;
                        break;
                    }
                }
                j += 1;
            }
            let last = *pts.last().unwrap();
            out.move_to(last.0, last.1);
            for k in (0..kinds.len()).rev() {
                let prev = pts[k];
                match kinds[k] {
                    Seg::Line(..) => out.line_to(prev.0, prev.1),
                    Seg::Cubic(x1, y1, x2, y2, _, _) => out.curve_to(x2, y2, x1, y1, prev.0, prev.1),
                    _ => {}
                }
            }
            if closed {
                out.close();
            }
            i = j;
        }
        out
    }
}

/// Standard 4-curve ellipse inscribed in a box.
pub fn ellipse_segs(acc: &mut PathAcc, m: &Matrix, x0: f64, y0: f64, x1: f64, y1: f64) {
    let cx = (x0 + x1) / 2.0;
    let cy = (y0 + y1) / 2.0;
    let rx = (x1 - x0).abs() / 2.0;
    let ry = (y1 - y0).abs() / 2.0;
    let em = Matrix::scale(rx, ry).then(&Matrix::translate(cx, cy)).then(m);
    acc.arc(&em, 0.0, 0.0, 1.0, 0.0, 360.0, true, false);
    acc.close();
}
