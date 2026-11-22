//! Pixels (host.cs "pixels" and Ocr): a GDI capture of a rectangle as PNG,
//! and Windows.Media.Ocr over one.

use std::time::Instant;

use base64::Engine as _;
use serde_json::{json, Map, Value};
use windows::core::HSTRING;
use windows::Globalization::Language;
use windows::Graphics::Imaging::{BitmapPixelFormat, SoftwareBitmap};
use windows::Media::Ocr::OcrEngine;
use windows::Storage::Streams::DataWriter;

use super::wins::{target_window, window_rect};
use super::{bool_arg, err, int_arg, rect_arg, rect_json, str_arg, DResult, Rect};
use crate::capture::{gdi, PxRect, Rgb};

fn capture_rect(a: &Value) -> DResult<Rect> {
    let r = match rect_arg(a, "rect")? {
        Some(r) => Some(r),
        None => window_rect(target_window(a)?),
    };
    match r {
        Some(r) if r[2] > 0 && r[3] > 0 => Ok(r),
        _ => Err(err("BAD_ARGS", "nothing to capture: empty rect")),
    }
}

/// GDI capture of the screen as composed. Our own windows carry
/// WDA_EXCLUDEFROMCAPTURE, so they are already missing from it.
fn grab(r: Rect) -> DResult<(u32, u32, Vec<u8>)> {
    let f = gdi::grab(PxRect {
        x: r[0],
        y: r[1],
        width: r[2],
        height: r[3],
    })
    .map_err(|e| err("INTERNAL", format!("capture failed: {e}")))?;
    Ok((f.width, f.height, f.bgra))
}

fn scaled(n: u32, s: f64) -> u32 {
    ((f64::from(n) * s).round() as u32).max(1)
}

fn png(img: &Rgb) -> DResult<Vec<u8>> {
    let mut out = Vec::new();
    {
        let mut e = png::Encoder::new(&mut out, img.width, img.height);
        e.set_color(png::ColorType::Rgb);
        e.set_depth(png::BitDepth::Eight);
        let mut w = e
            .write_header()
            .map_err(|x| err("INTERNAL", format!("png: {x}")))?;
        w.write_image_data(&img.data)
            .map_err(|x| err("INTERNAL", format!("png: {x}")))?;
    }
    Ok(out)
}

/// The scale a capture is sent at: screenshot keeps a big capture under
/// maxEdge; zoom (enlarge) also enlarges a small region up to it, at most
/// four times, for text too small to read at 1:1.
fn capture_scale(w: i32, h: i32, max_edge: i32, enlarge: bool) -> f64 {
    let s = f64::from(max_edge) / f64::from(w.max(h));
    if enlarge {
        s.min(4.0)
    } else {
        s.min(1.0)
    }
}

pub(crate) fn capture_cmd(a: &Value) -> DResult<Value> {
    let r = capture_rect(a)?;
    let max_edge = int_arg(a, "maxEdge", 1568)?.max(64);
    let s = capture_scale(r[2], r[3], max_edge, bool_arg(a, "enlarge", false));
    let (w, h, bgra) = grab(r)?;
    let raw = Rgb::from_bgra(w, h, w as usize * 4, &bgra);
    let img = if (s - 1.0).abs() < 0.001 {
        raw
    } else {
        raw.resize(scaled(w, s), scaled(h, s))
    };
    let bytes = png(&img)?;
    Ok(json!({
        "dataUrl": format!(
            "data:image/png;base64,{}",
            base64::engine::general_purpose::STANDARD.encode(bytes)
        ),
        "rect": r,
        "scale": s,
        "width": img.width,
        "height": img.height,
    }))
}

fn no_ocr() -> super::DesktopError {
    err(
        "NO_OCR",
        "no Windows OCR language is installed (Settings > Time & language > Language > add a language with optical character recognition)",
    )
}

fn engine(lang: &str) -> DResult<OcrEngine> {
    if !lang.is_empty() && lang != "auto" {
        if let Ok(l) = Language::CreateLanguage(&HSTRING::from(lang)) {
            if let Ok(e) = OcrEngine::TryCreateFromLanguage(&l) {
                return Ok(e);
            }
        }
    }
    if let Ok(e) = OcrEngine::TryCreateFromUserProfileLanguages() {
        return Ok(e);
    }
    // A user-profile language without an OCR pack: take any installed
    // recogniser rather than none.
    if let Ok(langs) = OcrEngine::AvailableRecognizerLanguages() {
        for l in langs {
            if let Ok(e) = OcrEngine::TryCreateFromLanguage(&l) {
                return Ok(e);
            }
        }
    }
    Err(no_ocr())
}

/// The OCR scale: small regions are enlarged, since the recogniser misses
/// text under about 10px tall and doubling costs little at that size.
fn ocr_scale(w: u32, h: u32, max_dim: u32) -> f64 {
    let long = w.max(h);
    let mut s = if long < 1400 { 2.0 } else { 1.0 };
    if f64::from(long) * s > f64::from(max_dim) {
        s = f64::from(max_dim) / f64::from(long);
    }
    s
}

fn to_bgra(img: &Rgb) -> Vec<u8> {
    let mut out = Vec::with_capacity(img.data.len() / 3 * 4);
    for px in img.data.chunks_exact(3) {
        out.extend_from_slice(&[px[2], px[1], px[0], 255]);
    }
    out
}

fn recognize(w: u32, h: u32, bgra: &[u8], origin: Rect, lang: &str) -> DResult<Value> {
    let eng = engine(lang)?;
    let max_dim = OcrEngine::MaxImageDimension().unwrap_or(10000).max(1);
    let s = ocr_scale(w, h, max_dim);
    let (bw, bh, pixels) = if (s - 1.0).abs() < 0.001 {
        let mut p = bgra.to_vec();
        for px in p.chunks_exact_mut(4) {
            px[3] = 255;
        }
        (w, h, p)
    } else {
        let img = Rgb::from_bgra(w, h, w as usize * 4, bgra).resize(scaled(w, s), scaled(h, s));
        (img.width, img.height, to_bgra(&img))
    };
    let writer = DataWriter::new()?;
    writer.WriteBytes(&pixels)?;
    let buffer = writer.DetachBuffer()?;
    let bitmap = SoftwareBitmap::CreateCopyFromBuffer(
        &buffer,
        BitmapPixelFormat::Bgra8,
        bw as i32,
        bh as i32,
    )?;
    let result = eng.RecognizeAsync(&bitmap)?.join()?;
    let mut spans = Vec::new();
    let mut text = String::new();
    for line in result.Lines()? {
        let mut first = true;
        for word in line.Words()? {
            let b = word.BoundingRect()?;
            let t = word.Text()?.to_string();
            let back = |v: f32| (f64::from(v) / s).round() as i32;
            let rect = [
                origin[0] + back(b.X),
                origin[1] + back(b.Y),
                back(b.Width),
                back(b.Height),
            ];
            let mut d = Map::new();
            d.insert("text".into(), json!(t));
            d.insert("rect".into(), rect_json(Some(rect)));
            d.insert("ref".into(), Value::Null);
            d.insert("source".into(), json!("ocr"));
            spans.push(Value::Object(d));
            if !first {
                text.push(' ');
            }
            text.push_str(&t);
            first = false;
        }
        text.push('\n');
    }
    let mut res = json!({ "spans": spans, "text": text });
    if let Ok(tag) = eng.RecognizerLanguage().and_then(|l| l.LanguageTag()) {
        res["lang"] = json!(tag.to_string());
    }
    Ok(res)
}

pub(crate) fn ocr_cmd(a: &Value) -> DResult<Value> {
    let r = capture_rect(a)?;
    let t0 = Instant::now();
    let (w, h, bgra) = grab(r)?;
    let mut res = recognize(w, h, &bgra, r, &str_arg(a, "lang", "auto"))?;
    res["rect"] = json!(r);
    res["ocrMs"] = json!(t0.elapsed().as_millis() as u64);
    Ok(res)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capture_scales_like_host_cs() {
        assert_eq!(capture_scale(800, 600, 1568, false), 1.0);
        assert!((capture_scale(3136, 1000, 1568, false) - 0.5).abs() < 1e-9);
        assert_eq!(capture_scale(100, 50, 1568, true), 4.0);
        assert!((capture_scale(784, 100, 1568, true) - 2.0).abs() < 1e-9);
    }

    #[test]
    fn ocr_doubles_small_regions_within_the_limit() {
        assert_eq!(ocr_scale(800, 200, 10000), 2.0);
        assert_eq!(ocr_scale(1920, 1080, 10000), 1.0);
        assert!((ocr_scale(1000, 10, 1500) - 1.5).abs() < 1e-9);
    }

    #[test]
    fn png_round_trips_a_pixel() {
        let img = Rgb {
            width: 1,
            height: 1,
            data: vec![10, 20, 30],
        };
        let bytes = png(&img).unwrap();
        assert_eq!(&bytes[1..4], b"PNG");
    }
}
