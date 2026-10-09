//! On-device text recognition (Apple Vision / Windows.Media.Ocr), used to
//! snap text highlights and underlines to the real line boxes. Runs only on
//! a small crop around the mark, at the fast recognition level: tens of
//! milliseconds, and nothing leaves the machine.

use image::DynamicImage;
use luma_core::geometry::{Display, Rect};
use luma_core::snap::TextLine;

/// Text lines near `around` (view points), in view points. Empty when OCR is
/// unavailable or finds nothing.
pub fn lines_near(full: &DynamicImage, display: &Display, around: &Rect) -> Vec<TextLine> {
    let (vw, vh) = display.view_size();
    let sx = full.width() as f64 / vw;
    let sy = full.height() as f64 / vh;
    let pad_x = (around.w * 0.25).max(24.0);
    let pad_y = (around.h * 0.6).max(16.0);
    let Some(px) = Rect::new((around.x - pad_x) * sx, (around.y - pad_y) * sy, (around.w + 2.0 * pad_x) * sx, (around.h + 2.0 * pad_y) * sy)
        .intersection(&Rect::new(0.0, 0.0, full.width() as f64, full.height() as f64))
    else {
        return Vec::new();
    };
    let crop = full.crop_imm(px.x as u32, px.y as u32, (px.w as u32).max(1), (px.h as u32).max(1));
    let t = std::time::Instant::now();
    let found = native::recognize(&crop);
    log::debug!("ocr: {} lines in {} ms", found.len(), t.elapsed().as_millis());
    let (cw, ch) = (crop.width() as f64, crop.height() as f64);
    found
        .into_iter()
        .map(|(f, text)| TextLine {
            // normalized crop coordinates (top-left origin) → view points
            frame: Rect::new((px.x + f.x * cw) / sx, (px.y + f.y * ch) / sy, f.w * cw / sx, f.h * ch / sy),
            text,
        })
        .collect()
}

#[cfg(target_os = "macos")]
mod native {
    use image::DynamicImage;
    use luma_core::geometry::Rect;
    use objc2::rc::Retained;
    use objc2::AllocAnyThread;
    use objc2_foundation::{NSArray, NSData, NSDictionary};
    use objc2_vision::{VNImageRequestHandler, VNRecognizeTextRequest, VNRequest, VNRequestTextRecognitionLevel};

    /// Lines as (normalized frame with top-left origin, text).
    pub fn recognize(img: &DynamicImage) -> Vec<(Rect, String)> {
        let mut png = Vec::new();
        if img.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).is_err() {
            return Vec::new();
        }
        objc2::rc::autoreleasepool(|_| {
            let data = NSData::with_bytes(&png);
            let handler = VNImageRequestHandler::initWithData_options(VNImageRequestHandler::alloc(), &data, &NSDictionary::new());
            let req = VNRecognizeTextRequest::new();
            req.setRecognitionLevel(VNRequestTextRecognitionLevel::Fast);
            req.setUsesLanguageCorrection(false);
            let as_request: Retained<VNRequest> = Retained::into_super(Retained::into_super(req.clone()));
            if let Err(e) = handler.performRequests_error(&NSArray::from_retained_slice(&[as_request])) {
                log::debug!("vision ocr failed: {e:?}");
                return Vec::new();
            }
            let Some(results) = req.results() else { return Vec::new() };
            results
                .iter()
                .map(|obs| {
                    let b = unsafe { obs.boundingBox() };
                    let text = obs.topCandidates(1).firstObject().map(|t| t.string().to_string()).unwrap_or_default();
                    // Vision's origin is bottom-left.
                    let f = Rect::new(b.origin.x, 1.0 - b.origin.y - b.size.height, b.size.width, b.size.height);
                    (f, text)
                })
                .collect()
        })
    }
}

#[cfg(target_os = "windows")]
mod native {
    use image::DynamicImage;
    use luma_core::geometry::Rect;
    use windows::Graphics::Imaging::{BitmapAlphaMode, BitmapPixelFormat, SoftwareBitmap};
    use windows::Media::Ocr::OcrEngine;
    use windows::Storage::Streams::DataWriter;

    /// Lines as (normalized frame with top-left origin, text).
    pub fn recognize(img: &DynamicImage) -> Vec<(Rect, String)> {
        recognize_inner(img).unwrap_or_else(|e| {
            log::debug!("windows ocr failed: {e}");
            Vec::new()
        })
    }

    fn recognize_inner(img: &DynamicImage) -> windows::core::Result<Vec<(Rect, String)>> {
        let rgba = img.to_rgba8();
        let (w, h) = (rgba.width(), rgba.height());
        // BGRA8 is the format the OCR engine accepts.
        let mut bgra = rgba.into_raw();
        for px in bgra.chunks_exact_mut(4) {
            px.swap(0, 2);
        }
        let writer = DataWriter::new()?;
        writer.WriteBytes(&bgra)?;
        let buffer = writer.DetachBuffer()?;
        let bitmap = SoftwareBitmap::CreateCopyWithAlphaFromBuffer(&buffer, BitmapPixelFormat::Bgra8, w as i32, h as i32, BitmapAlphaMode::Premultiplied)?;
        let engine = OcrEngine::TryCreateFromUserProfileLanguages()?;
        let result = engine.RecognizeAsync(&bitmap)?.get()?;
        let mut out = Vec::new();
        for line in result.Lines()? {
            let text = line.Text()?.to_string();
            let (mut x0, mut y0, mut x1, mut y1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
            for word in line.Words()? {
                let r = word.BoundingRect()?;
                x0 = x0.min(r.X as f64);
                y0 = y0.min(r.Y as f64);
                x1 = x1.max((r.X + r.Width) as f64);
                y1 = y1.max((r.Y + r.Height) as f64);
            }
            if x1 > x0 && y1 > y0 {
                out.push((Rect::new(x0 / w as f64, y0 / h as f64, (x1 - x0) / w as f64, (y1 - y0) / h as f64), text));
            }
        }
        Ok(out)
    }
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
mod native {
    pub fn recognize(_img: &image::DynamicImage) -> Vec<(luma_core::geometry::Rect, String)> {
        Vec::new()
    }
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;
    use luma_core::geometry::Rect;

    /// Renders nothing fancy: two dark bars on white are not text, so this
    /// checks that the Vision call runs and returns cleanly. The live check
    /// (`-- --ignored`) reads the real screen.
    #[test]
    fn vision_runs_on_a_blank_crop() {
        let img = DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(200, 80, image::Rgba([255, 255, 255, 255])));
        assert!(native::recognize(&img).is_empty());
    }

    /// `cargo test -p luma ocr -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn reads_text_on_screen() {
        let displays = crate::screen::displays().unwrap();
        let d = &displays[0];
        let full = crate::screen::capture_display(d.index).unwrap();
        let (w, h) = d.view_size();
        let t = std::time::Instant::now();
        let lines = lines_near(&full, d, &Rect::new(w * 0.25, h * 0.25, w * 0.3, h * 0.2));
        println!("{} ms", t.elapsed().as_millis());
        for l in &lines {
            println!("{:?} {:?}", l.frame, l.text);
        }
    }
}
