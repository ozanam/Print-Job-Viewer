//! The application icon, drawn with tiny-skia so no image files are needed at runtime.

use tiny_skia::*;

/// Render the icon as straight RGBA at `size` × `size` pixels.
pub fn render(size: u32) -> Vec<u8> {
    let mut pm = Pixmap::new(size, size).expect("icon pixmap");
    let s = size as f32 / 1024.0;
    let ts = Transform::from_scale(s, s);

    // Background: rounded square with a blue gradient (macOS icon grid: 824 px body, 100 px margin)
    let bg = rounded_rect(100.0, 100.0, 824.0, 824.0, 185.0);
    let mut paint = Paint { anti_alias: true, ..Default::default() };
    paint.shader = LinearGradient::new(
        Point::from_xy(512.0, 100.0),
        Point::from_xy(512.0, 924.0),
        vec![GradientStop::new(0.0, Color::from_rgba8(58, 134, 255, 255)), GradientStop::new(1.0, Color::from_rgba8(23, 64, 170, 255))],
        SpreadMode::Pad,
        Transform::identity(),
    )
    .unwrap();
    pm.fill_path(&bg, &paint, FillRule::Winding, ts, None);

    // Page shadow
    let mut shadow = Paint { anti_alias: true, ..Default::default() };
    shadow.set_color_rgba8(0, 0, 40, 70);
    pm.fill_path(&page_path(282.0, 222.0), &shadow, FillRule::Winding, ts, None);

    // Page with folded corner
    let mut white = Paint { anti_alias: true, ..Default::default() };
    white.set_color_rgba8(255, 255, 255, 255);
    pm.fill_path(&page_path(270.0, 205.0), &white, FillRule::Winding, ts, None);
    let mut fold = PathBuilder::new();
    fold.move_to(620.0, 205.0);
    fold.line_to(620.0, 325.0);
    fold.line_to(740.0, 325.0);
    fold.close();
    let mut foldp = Paint { anti_alias: true, ..Default::default() };
    foldp.set_color_rgba8(200, 214, 240, 255);
    pm.fill_path(&fold.finish().unwrap(), &foldp, FillRule::Winding, ts, None);

    // Text lines
    let mut grey = Paint { anti_alias: true, ..Default::default() };
    grey.set_color_rgba8(150, 162, 185, 255);
    for (i, w) in [250.0, 330.0, 300.0, 340.0, 220.0].iter().enumerate() {
        let y = 400.0 + i as f32 * 62.0;
        pm.fill_path(&rounded_rect(340.0, y, *w, 26.0, 13.0), &grey, FillRule::Winding, ts, None);
    }
    // Coloured graphic block (pages contain more than text)
    let mut accent = Paint { anti_alias: true, ..Default::default() };
    accent.set_color_rgba8(255, 166, 0, 255);
    pm.fill_path(&rounded_rect(340.0, 330.0, 170.0, 34.0, 8.0), &accent, FillRule::Winding, ts, None);

    // Magnifying glass
    let mut ring = Paint { anti_alias: true, ..Default::default() };
    ring.set_color_rgba8(20, 30, 60, 255);
    let stroke = Stroke { width: 46.0, line_cap: LineCap::Round, ..Default::default() };
    let mut handle = PathBuilder::new();
    handle.move_to(735.0, 735.0);
    handle.line_to(800.0, 800.0);
    let h = handle.finish().unwrap();
    pm.stroke_path(&h, &ring, &Stroke { width: 70.0, ..stroke.clone() }, ts, None);
    let lens = PathBuilder::from_circle(620.0, 620.0, 120.0).unwrap();
    let mut glass = Paint { anti_alias: true, ..Default::default() };
    glass.set_color_rgba8(225, 240, 255, 255);
    pm.fill_path(&lens, &glass, FillRule::Winding, ts, None);
    pm.stroke_path(&lens, &ring, &stroke, ts, None);

    // demultiply
    let mut v = pm.take();
    for px in v.chunks_exact_mut(4) {
        let a = px[3] as u32;
        if a > 0 && a < 255 {
            for c in px.iter_mut().take(3) {
                *c = ((*c as u32 * 255 + a / 2) / a).min(255) as u8;
            }
        }
    }
    v
}

fn page_path(x: f32, y: f32) -> Path {
    let mut pb = PathBuilder::new();
    pb.move_to(x, y);
    pb.line_to(x + 350.0, y);
    pb.line_to(x + 470.0, y + 120.0);
    pb.line_to(x + 470.0, y + 615.0);
    pb.line_to(x, y + 615.0);
    pb.close();
    pb.finish().unwrap()
}

fn rounded_rect(x: f32, y: f32, w: f32, h: f32, r: f32) -> Path {
    let k = 0.5523 * r;
    let mut pb = PathBuilder::new();
    pb.move_to(x + r, y);
    pb.line_to(x + w - r, y);
    pb.cubic_to(x + w - r + k, y, x + w, y + r - k, x + w, y + r);
    pb.line_to(x + w, y + h - r);
    pb.cubic_to(x + w, y + h - r + k, x + w - r + k, y + h, x + w - r, y + h);
    pb.line_to(x + r, y + h);
    pb.cubic_to(x + r - k, y + h, x, y + h - r + k, x, y + h - r);
    pb.line_to(x, y + r);
    pb.cubic_to(x, y + r - k, x + r - k, y, x + r, y);
    pb.close();
    pb.finish().unwrap()
}

/// Encode straight RGBA as PNG.
pub fn png(size: u32) -> Vec<u8> {
    let rgba = render(size);
    let mut pm = Pixmap::new(size, size).unwrap();
    // re-premultiply for tiny-skia's encoder
    for (dst, src) in pm.pixels_mut().iter_mut().zip(rgba.chunks_exact(4)) {
        *dst = ColorU8::from_rgba(src[0], src[1], src[2], src[3]).premultiply();
    }
    pm.encode_png().unwrap()
}
