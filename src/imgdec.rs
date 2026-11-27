//! Decoding embedded images (PNG, JPEG, Windows DIB) to premultiplied RGBA.

pub struct Decoded {
    pub width: u32,
    pub height: u32,
    /// Premultiplied RGBA.
    pub rgba: Vec<u8>,
    pub dpi: Option<(f64, f64)>,
}

pub fn decode(data: &[u8]) -> Result<Decoded, String> {
    if data.starts_with(b"\x89PNG") {
        png(data)
    } else if data.starts_with(&[0xFF, 0xD8]) {
        jpeg(data)
    } else if data.starts_with(b"II*\0") || data.starts_with(b"MM\0*") {
        Err("TIFF images are not supported".into())
    } else if data.starts_with(b"BM") && data.len() > 14 {
        let off = u32::from_le_bytes([data[10], data[11], data[12], data[13]]) as usize;
        dib(&data[14..], off.checked_sub(14).and_then(|o| data.get(14 + o..)), None)
    } else if data.starts_with(b"II\xBC") {
        Err("HD Photo / JPEG XR images are not supported".into())
    } else {
        Err("Unknown image format".into())
    }
}

fn png(data: &[u8]) -> Result<Decoded, String> {
    let mut dec = ::png::Decoder::new(std::io::Cursor::new(data));
    dec.set_transformations(::png::Transformations::EXPAND | ::png::Transformations::STRIP_16);
    let mut r = dec.read_info().map_err(|e| format!("PNG: {e}"))?;
    let dpi = r.info().pixel_dims.and_then(|d| if d.unit == ::png::Unit::Meter && d.xppu > 0 { Some((d.xppu as f64 * 0.0254, d.yppu as f64 * 0.0254)) } else { None });
    let mut buf = vec![0u8; r.output_buffer_size()];
    let info = r.next_frame(&mut buf).map_err(|e| format!("PNG: {e}"))?;
    let (w, h) = (info.width, info.height);
    let n = (w * h) as usize;
    let mut out = Vec::with_capacity(n * 4);
    let src = &buf[..info.buffer_size()];
    match info.color_type {
        ::png::ColorType::Rgba => {
            for p in src.chunks_exact(4) {
                let a = p[3] as u32;
                out.extend_from_slice(&[(p[0] as u32 * a / 255) as u8, (p[1] as u32 * a / 255) as u8, (p[2] as u32 * a / 255) as u8, p[3]]);
            }
        }
        ::png::ColorType::Rgb => {
            for p in src.chunks_exact(3) {
                out.extend_from_slice(&[p[0], p[1], p[2], 255]);
            }
        }
        ::png::ColorType::GrayscaleAlpha => {
            for p in src.chunks_exact(2) {
                let g = (p[0] as u32 * p[1] as u32 / 255) as u8;
                out.extend_from_slice(&[g, g, g, p[1]]);
            }
        }
        ::png::ColorType::Grayscale => {
            for &g in src {
                out.extend_from_slice(&[g, g, g, 255]);
            }
        }
        ::png::ColorType::Indexed => return Err("PNG: unexpanded palette".into()),
    }
    out.resize(n * 4, 255);
    Ok(Decoded { width: w, height: h, rgba: out, dpi })
}

fn jpeg_density(data: &[u8]) -> Option<(f64, f64)> {
    // JFIF APP0
    if data.len() > 18 && &data[6..11] == b"JFIF\0" {
        let unit = data[13];
        let x = u16::from_be_bytes([data[14], data[15]]) as f64;
        let y = u16::from_be_bytes([data[16], data[17]]) as f64;
        if x > 0.0 && y > 0.0 {
            return match unit {
                1 => Some((x, y)),
                2 => Some((x * 2.54, y * 2.54)),
                _ => None,
            };
        }
    }
    None
}

fn jpeg(data: &[u8]) -> Result<Decoded, String> {
    let mut d = jpeg_decoder::Decoder::new(data);
    let px = d.decode().map_err(|e| format!("JPEG: {e}"))?;
    let info = d.info().ok_or("JPEG: no info")?;
    let (w, h) = (info.width as u32, info.height as u32);
    let n = (w * h) as usize;
    let mut out = Vec::with_capacity(n * 4);
    match info.pixel_format {
        jpeg_decoder::PixelFormat::RGB24 => {
            for p in px.chunks_exact(3) {
                out.extend_from_slice(&[p[0], p[1], p[2], 255]);
            }
        }
        jpeg_decoder::PixelFormat::L8 => {
            for &g in &px {
                out.extend_from_slice(&[g, g, g, 255]);
            }
        }
        jpeg_decoder::PixelFormat::L16 => {
            for g in px.chunks_exact(2) {
                out.extend_from_slice(&[g[0], g[0], g[0], 255]);
            }
        }
        jpeg_decoder::PixelFormat::CMYK32 => {
            // Adobe CMYK JPEGs are stored inverted
            for p in px.chunks_exact(4) {
                let k = p[3] as u32;
                out.extend_from_slice(&[(p[0] as u32 * k / 255) as u8, (p[1] as u32 * k / 255) as u8, (p[2] as u32 * k / 255) as u8, 255]);
            }
        }
    }
    out.resize(n * 4, 255);
    Ok(Decoded { width: w, height: h, rgba: out, dpi: jpeg_density(data) })
}

/// Decode a Windows DIB: `bmi` starts with BITMAPINFOHEADER (and colour table);
/// `bits` are the pixel bits (if None, they follow the header and colour table).
/// `usage_pal` true means the colour table holds palette indices (DIB_PAL_COLORS): treated as grey.
pub fn dib(bmi: &[u8], bits: Option<&[u8]>, _usage: Option<u32>) -> Result<Decoded, String> {
    let u32_at = |o: usize| -> u32 { bmi.get(o..o + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]])).unwrap_or(0) };
    let u16_at = |o: usize| -> u16 { bmi.get(o..o + 2).map(|b| u16::from_le_bytes([b[0], b[1]])).unwrap_or(0) };
    let hsize = u32_at(0) as usize;
    let (w, h, bpp, comp, clr_used);
    if hsize == 12 {
        w = u16_at(4) as i32;
        h = u16_at(6) as i16 as i32;
        bpp = u16_at(10);
        comp = 0;
        clr_used = 0;
    } else if hsize >= 40 {
        w = u32_at(4) as i32;
        h = u32_at(8) as i32;
        bpp = u16_at(14);
        comp = u32_at(16);
        clr_used = u32_at(32);
    } else {
        return Err("Bad bitmap header".into());
    }
    if w <= 0 || h == 0 || w > 30000 || h.abs() > 30000 {
        return Err("Bad bitmap size".into());
    }
    let top_down = h < 0;
    let (w, h) = (w as u32, h.unsigned_abs());
    let ncolors = if bpp <= 8 { if clr_used > 0 { clr_used as usize } else { 1usize << bpp } } else { clr_used as usize };
    let mut masks = None;
    let mut table_off = hsize;
    if comp == 3 && hsize == 40 {
        masks = Some([u32_at(40), u32_at(44), u32_at(48), 0]);
        table_off += 12;
    } else if comp == 3 && hsize >= 56 {
        masks = Some([u32_at(40), u32_at(44), u32_at(48), u32_at(52)]);
    }
    let entry = if hsize == 12 { 3 } else { 4 };
    let palette: Vec<[u8; 3]> = (0..ncolors.min(256))
        .map(|i| {
            let o = table_off + i * entry;
            [bmi.get(o + 2).copied().unwrap_or(0), bmi.get(o + 1).copied().unwrap_or(0), bmi.get(o).copied().unwrap_or(0)]
        })
        .collect();
    let after_table = table_off + if bpp <= 8 { ncolors * entry } else { 0 };
    let bits = match bits {
        Some(b) => b,
        None => bmi.get(after_table..).unwrap_or(&[]),
    };
    if comp == 4 || comp == 5 {
        let mut d = decode(bits)?;
        if !top_down {
            // embedded JPEG/PNG are stored top-down already
        }
        d.dpi = None;
        return Ok(d);
    }
    let stride = ((w as usize * bpp as usize + 31) / 32) * 4;
    let mut idx_rows: Option<Vec<u8>> = None;
    if comp == 1 || comp == 2 {
        idx_rows = Some(rle(bits, w as usize, h as usize, comp == 2));
    }
    let n = (w * h) as usize;
    let mut out = vec![255u8; n * 4];
    let shift = |m: u32| -> (u32, u32) {
        if m == 0 {
            return (0, 0);
        }
        let s = m.trailing_zeros();
        (s, (m >> s).count_ones())
    };
    for y in 0..h as usize {
        let src_row = if top_down { y } else { h as usize - 1 - y };
        for x in 0..w as usize {
            let o = (y * w as usize + x) * 4;
            let px: [u8; 4] = if let Some(rows) = &idx_rows {
                let i = rows.get(src_row * w as usize + x).copied().unwrap_or(0) as usize;
                let c = palette.get(i).copied().unwrap_or([0, 0, 0]);
                [c[0], c[1], c[2], 255]
            } else {
                let row = src_row * stride;
                match bpp {
                    1 | 2 | 4 | 8 => {
                        let bit = x * bpp as usize;
                        let b = bits.get(row + bit / 8).copied().unwrap_or(0);
                        let i = (b >> (8 - bpp as usize - bit % 8)) & ((1u16 << bpp) - 1) as u8;
                        let c = palette.get(i as usize).copied().unwrap_or(if i == 0 { [0, 0, 0] } else { [255, 255, 255] });
                        [c[0], c[1], c[2], 255]
                    }
                    16 => {
                        let v = bits.get(row + x * 2..row + x * 2 + 2).map(|b| u16::from_le_bytes([b[0], b[1]]) as u32).unwrap_or(0);
                        let m = masks.unwrap_or([0x7C00, 0x03E0, 0x001F, 0]);
                        let ch = |mask: u32| {
                            let (s, n) = shift(mask);
                            if n == 0 { 0 } else { (((v & mask) >> s) * 255 / ((1 << n) - 1)) as u8 }
                        };
                        [ch(m[0]), ch(m[1]), ch(m[2]), 255]
                    }
                    24 => {
                        let o2 = row + x * 3;
                        [bits.get(o2 + 2).copied().unwrap_or(0), bits.get(o2 + 1).copied().unwrap_or(0), bits.get(o2).copied().unwrap_or(0), 255]
                    }
                    32 => {
                        let o2 = row + x * 4;
                        let v = bits.get(o2..o2 + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]])).unwrap_or(0);
                        if let Some(m) = masks {
                            let ch = |mask: u32| {
                                let (s, n) = shift(mask);
                                if n == 0 { 0 } else { (((v & mask) >> s) * 255 / ((1u64 << n) - 1) as u32) as u8 }
                            };
                            [ch(m[0]), ch(m[1]), ch(m[2]), 255]
                        } else {
                            [(v >> 16) as u8, (v >> 8) as u8, v as u8, 255]
                        }
                    }
                    _ => [0, 0, 0, 255],
                }
            };
            out[o..o + 4].copy_from_slice(&px);
        }
    }
    Ok(Decoded { width: w, height: h, rgba: out, dpi: None })
}

/// 32-bit DIB with a premultiplied alpha channel (AlphaBlend sources).
pub fn dib_alpha(bmi: &[u8], bits: &[u8]) -> Option<Decoded> {
    let w = i32::from_le_bytes(bmi.get(4..8)?.try_into().ok()?);
    let h = i32::from_le_bytes(bmi.get(8..12)?.try_into().ok()?);
    let bpp = u16::from_le_bytes(bmi.get(14..16)?.try_into().ok()?);
    if bpp != 32 || w <= 0 || h == 0 {
        return None;
    }
    let (wu, hu) = (w as usize, h.unsigned_abs() as usize);
    let mut out = vec![0u8; wu * hu * 4];
    for y in 0..hu {
        let sr = if h < 0 { y } else { hu - 1 - y };
        for x in 0..wu {
            let o = (sr * wu + x) * 4;
            let b = bits.get(o..o + 4)?;
            let d = (y * wu + x) * 4;
            out[d..d + 4].copy_from_slice(&[b[2], b[1], b[0], b[3]]);
        }
    }
    Some(Decoded { width: wu as u32, height: hu as u32, rgba: out, dpi: None })
}

fn rle(d: &[u8], w: usize, h: usize, four: bool) -> Vec<u8> {
    let mut out = vec![0u8; w * h];
    let (mut x, mut y) = (0usize, 0usize);
    let mut p = 0;
    let mut put = |x: &mut usize, y: usize, v: u8| {
        if *x < w && y < h {
            out[y * w + *x] = v;
        }
        *x += 1;
    };
    while p + 1 < d.len() {
        let a = d[p] as usize;
        let b = d[p + 1];
        p += 2;
        if a > 0 {
            for i in 0..a {
                let v = if four { if i % 2 == 0 { b >> 4 } else { b & 15 } } else { b };
                put(&mut x, y, v);
            }
        } else {
            match b {
                0 => {
                    x = 0;
                    y += 1;
                }
                1 => break,
                2 => {
                    if p + 1 < d.len() {
                        x += d[p] as usize;
                        y += d[p + 1] as usize;
                    }
                    p += 2;
                }
                n => {
                    let n = n as usize;
                    for i in 0..n {
                        let v = if four {
                            let byte = d.get(p + i / 2).copied().unwrap_or(0);
                            if i % 2 == 0 { byte >> 4 } else { byte & 15 }
                        } else {
                            d.get(p + i).copied().unwrap_or(0)
                        };
                        put(&mut x, y, v);
                    }
                    let bytes = if four { n.div_ceil(2) } else { n };
                    p += bytes + (bytes & 1);
                }
            }
        }
    }
    // rows are bottom-up in the RLE stream; flip to match the uncompressed layout
    out
}
