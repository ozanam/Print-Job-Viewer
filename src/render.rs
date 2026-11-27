//! Rasterize a page display list with tiny-skia.

use crate::doc::{Brush, Clip, Item, Page, RasterImage};
use std::collections::HashMap;
use std::sync::Arc;
use tiny_skia::{
    Color, FillRule, FilterQuality, IntSize, Mask, Paint, Pattern, Pixmap, PixmapPaint, SpreadMode, Stroke,
    StrokeDash, Transform,
};

pub struct Rendered {
    pub width: u32,
    pub height: u32,
    /// Straight (non-premultiplied) RGBA; page background is opaque white.
    pub rgba: Vec<u8>,
}

fn to_pixmap(img: &RasterImage) -> Option<Pixmap> {
    Pixmap::from_vec(img.rgba().into_owned(), IntSize::from_wh(img.width, img.height)?)
}

/// Render `page` at `scale` pixels per point on a white background.
pub fn render_page(page: &Page, scale: f32) -> Option<Rendered> {
    render_impl(page, scale, true, ALL_LAYERS)
}

/// Render only the content kinds in `layers` (a mask of `Kind` values).
pub fn render_page_layers(page: &Page, scale: f32, layers: u8) -> Option<Rendered> {
    render_impl(page, scale, true, layers)
}

/// Render with a transparent background; output stays premultiplied.
#[allow(dead_code)]
pub fn render_page_transparent(page: &Page, scale: f32) -> Option<Rendered> {
    render_impl(page, scale, false, ALL_LAYERS)
}

pub const ALL_LAYERS: u8 = 7;

fn render_impl(page: &Page, scale: f32, white: bool, layers: u8) -> Option<Rendered> {
    let w = ((page.width * scale).round() as u32).clamp(1, 16384);
    let h = ((page.height * scale).round() as u32).clamp(1, 16384);
    let mut pm = Pixmap::new(w, h)?;
    if white {
        pm.fill(Color::WHITE);
    }
    let base = Transform::from_scale(scale, scale);

    let mut masks: HashMap<usize, Option<Mask>> = HashMap::new();
    let mut pixmaps: HashMap<usize, Option<Pixmap>> = HashMap::new();

    let get_mask = |clip: &Clip, masks: &mut HashMap<usize, Option<Mask>>| -> Option<usize> {
        let c = clip.as_ref()?;
        let key = Arc::as_ptr(c) as usize;
        masks.entry(key).or_insert_with(|| {
            let mut m = Mask::new(w, h)?;
            for (i, cp) in c.iter().enumerate() {
                let rule = if cp.even_odd { FillRule::EvenOdd } else { FillRule::Winding };
                if i == 0 {
                    m.fill_path(&cp.path, rule, true, base);
                } else {
                    m.intersect_path(&cp.path, rule, true, base);
                }
            }
            Some(m)
        });
        Some(key)
    };

    for (idx, item) in page.items.iter().enumerate() {
        if layers != ALL_LAYERS && layers & page.meta_of(idx).kind as u8 == 0 {
            continue;
        }
        let clip = match item {
            Item::Fill { clip, .. } | Item::Stroke { clip, .. } | Item::Image { clip, .. } => clip,
        };
        let mkey = get_mask(clip, &mut masks);
        let mask = mkey.and_then(|k| masks.get(&k).and_then(|m| m.as_ref()));
        if clip.is_some() && mask.is_none() {
            // degenerate clip: nothing visible
            continue;
        }
        match item {
            Item::Fill { path, transform, brush, even_odd, .. } => {
                let ts = base.pre_concat(*transform);
                if let Some(paint) = make_paint(brush, &mut pixmaps) {
                    let rule = if *even_odd { FillRule::EvenOdd } else { FillRule::Winding };
                    pm.fill_path(path, &paint, rule, ts, mask);
                }
            }
            Item::Stroke { path, transform, brush, style, .. } => {
                let ts = base.pre_concat(*transform);
                let sc = ((ts.sx * ts.sy - ts.kx * ts.ky).abs()).sqrt();
                let mut width = style.width;
                if width * sc < 1.0 {
                    // hairline: tiny-skia draws a 1px line for width 0
                    width = 0.0;
                }
                let mut stroke = Stroke { width, line_cap: style.cap, line_join: style.join, miter_limit: style.miter.max(1.0), dash: None };
                if let Some((arr, off)) = &style.dash {
                    // hairlines are stroked in device space, so scale the dash pattern
                    let k = if width == 0.0 { sc } else { 1.0 };
                    stroke.dash = StrokeDash::new(arr.iter().map(|d| d * k).collect(), *off * k);
                }
                if let Some(paint) = make_paint(brush, &mut pixmaps) {
                    if width == 0.0 {
                        // Hairlines ignore the transform scale; stroke the transformed path.
                        if let Some(p) = path.clone().transform(ts) {
                            pm.stroke_path(&p, &paint, &stroke, Transform::identity(), mask);
                        }
                    } else {
                        pm.stroke_path(path, &paint, &stroke, ts, mask);
                    }
                }
            }
            Item::Image { image, transform, .. } => {
                let key = Arc::as_ptr(image) as usize;
                let src = pixmaps.entry(key).or_insert_with(|| to_pixmap(image));
                if let Some(src) = src {
                    let ts = base.pre_concat(*transform);
                    // pixels of the image per device pixel
                    let dev_w = ((ts.sx * ts.sx + ts.ky * ts.ky).sqrt()).max(1e-6);
                    let quality = if dev_w > 1.5 { FilterQuality::Nearest } else { FilterQuality::Bilinear };
                    let paint = PixmapPaint { quality, ..PixmapPaint::default() };
                    pm.draw_pixmap(0, 0, src.as_ref(), &paint, ts, mask);
                }
            }
        }
    }

    let mut rgba = pm.take();
    if !white {
        return Some(Rendered { width: w, height: h, rgba });
    }
    // Demultiply (background is opaque, so this is nearly a copy).
    for px in rgba.chunks_exact_mut(4) {
        let a = px[3];
        if a != 255 && a != 0 {
            let f = 255.0 / a as f32;
            px[0] = (px[0] as f32 * f).min(255.0) as u8;
            px[1] = (px[1] as f32 * f).min(255.0) as u8;
            px[2] = (px[2] as f32 * f).min(255.0) as u8;
        }
    }
    Some(Rendered { width: w, height: h, rgba })
}

fn make_paint<'a>(brush: &Brush, pixmaps: &'a mut HashMap<usize, Option<Pixmap>>) -> Option<Paint<'a>> {
    let mut paint = Paint { anti_alias: true, ..Paint::default() };
    match brush {
        Brush::Solid(c) => {
            paint.set_color_rgba8((c.0 * 255.0).round() as u8, (c.1 * 255.0).round() as u8, (c.2 * 255.0).round() as u8, 255);
        }
        Brush::Pattern { image, transform } => {
            let key = Arc::as_ptr(image) as usize;
            let entry = pixmaps.entry(key).or_insert_with(|| to_pixmap(image));
            let pm = entry.as_ref()?;
            paint.shader = Pattern::new(pm.as_ref(), SpreadMode::Repeat, FilterQuality::Nearest, 1.0, *transform);
        }
    }
    Some(paint)
}

/// Encode a rendered page to PNG bytes.
pub fn encode_png(r: &Rendered) -> Option<Vec<u8>> {
    let pm = Pixmap::from_vec(r.rgba.clone(), IntSize::from_wh(r.width, r.height)?)?;
    pm.encode_png().ok()
}

/// Bounding box of an item on the page, in points (x0, y0, x1, y1).
pub fn item_bounds(item: &Item) -> Option<[f32; 4]> {
    let (r, ts, pad) = match item {
        Item::Fill { path, transform, .. } => (path.bounds(), *transform, 0.0),
        Item::Stroke { path, transform, style, .. } => (path.bounds(), *transform, style.width / 2.0),
        Item::Image { image, transform, .. } => (tiny_skia::Rect::from_xywh(0.0, 0.0, image.width as f32, image.height as f32)?, *transform, 0.0),
    };
    let (l, t, rr, b) = (r.left() - pad, r.top() - pad, r.right() + pad, r.bottom() + pad);
    let mut out = [f32::MAX, f32::MAX, f32::MIN, f32::MIN];
    for (x, y) in [(l, t), (rr, t), (l, b), (rr, b)] {
        let px = ts.sx * x + ts.kx * y + ts.tx;
        let py = ts.ky * x + ts.sy * y + ts.ty;
        out[0] = out[0].min(px);
        out[1] = out[1].min(py);
        out[2] = out[2].max(px);
        out[3] = out[3].max(py);
    }
    if out.iter().all(|v| v.is_finite()) { Some(out) } else { None }
}
