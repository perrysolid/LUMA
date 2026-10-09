//! Displays, pointer, active window and screen capture.
//!
//! This is the only place that knows how each OS reports coordinates; it
//! converts everything into `luma_core::geometry` types.

use anyhow::{anyhow, Context, Result};
use image::DynamicImage;
use luma_core::geometry::{display_at, Display, Point, Rect};
pub use luma_net::vision::EncodedImage;
use tauri::{AppHandle, Runtime};

#[cfg(target_os = "macos")]
#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGPreflightScreenCaptureAccess() -> bool;
    fn CGRequestScreenCaptureAccess() -> bool;
}

/// Without Screen Recording permission macOS does not fail a capture: it
/// silently returns only the wallpaper and menu bar. So check explicitly.
#[derive(Debug)]
pub struct NoScreenPermission {
    pub host: String,
}

impl std::fmt::Display for NoScreenPermission {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "I can't see your screen yet. In System Settings → Privacy & Security → Screen & System Audio Recording, turn on {}, then quit and reopen it.",
            self.host
        )
    }
}
impl std::error::Error for NoScreenPermission {}

/// The app macOS attributes the permission to: LUMA itself when bundled,
/// the terminal or editor when running `npm run tauri dev`.
fn permission_host() -> String {
    let bundled = std::env::current_exe()
        .ok()
        .is_some_and(|p| p.to_string_lossy().contains(".app/Contents/MacOS/"));
    if bundled {
        return "LUMA".into();
    }
    match std::env::var("TERM_PROGRAM").unwrap_or_default().as_str() {
        "Apple_Terminal" => "Terminal".into(),
        "iTerm.app" => "iTerm".into(),
        "vscode" => "Visual Studio Code (or Cursor)".into(),
        "WarpTerminal" => "Warp".into(),
        "ghostty" => "Ghostty".into(),
        "" => "the app you started LUMA from".into(),
        other => other.to_string(),
    }
}

pub fn has_screen_permission() -> bool {
    #[cfg(target_os = "macos")]
    unsafe {
        CGPreflightScreenCaptureAccess()
    }
    #[cfg(not(target_os = "macos"))]
    true
}

/// Ask once (shows the system prompt the first time) and open the settings
/// pane so the user can flip the switch.
pub fn request_screen_permission() -> NoScreenPermission {
    #[cfg(target_os = "macos")]
    unsafe {
        let _ = CGRequestScreenCaptureAccess();
        let _ = std::process::Command::new("open")
            .arg("x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture")
            .spawn();
    }
    NoScreenPermission { host: permission_host() }
}

/// Enumerate displays in OS input space.
pub fn displays() -> Result<Vec<Display>> {
    let monitors = xcap::Monitor::all().map_err(|e| anyhow!("listing displays: {e}"))?;
    let mut out = Vec::with_capacity(monitors.len());
    for (index, m) in monitors.iter().enumerate() {
        let scale = m.scale_factor().unwrap_or(1.0) as f64;
        out.push(Display {
            index,
            name: m.friendly_name().or_else(|_| m.name()).unwrap_or_else(|_| format!("Display {}", index + 1)),
            // xcap reports CGDisplayBounds (points) on macOS and monitor rects
            // in physical pixels on per-monitor-DPI-aware Windows processes:
            // exactly our input space on each OS.
            input_frame: Rect::new(
                m.x().unwrap_or(0) as f64,
                m.y().unwrap_or(0) as f64,
                m.width().unwrap_or(1) as f64,
                m.height().unwrap_or(1) as f64,
            ),
            input_per_point: if cfg!(target_os = "macos") { 1.0 } else { scale },
            scale_factor: scale,
            is_primary: m.is_primary().unwrap_or(index == 0),
        });
    }
    if out.is_empty() {
        return Err(anyhow!("no displays found"));
    }
    Ok(out)
}

#[cfg(target_os = "macos")]
mod native {
    use std::ffi::c_void;
    #[repr(C)]
    struct CGPoint {
        x: f64,
        y: f64,
    }
    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        fn CGEventCreate(source: *const c_void) -> *mut c_void;
        fn CGEventGetLocation(event: *mut c_void) -> CGPoint;
        fn CGEventSourceButtonState(state: i32, button: u32) -> bool;
    }
    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        fn CFRelease(cf: *const c_void);
    }
    /// Global display coordinates in points, origin top-left of the primary
    /// display: exactly LUMA's input space. Safe off the main thread.
    pub fn pointer() -> Option<(f64, f64)> {
        unsafe {
            let e = CGEventCreate(std::ptr::null());
            if e.is_null() {
                return None;
            }
            let p = CGEventGetLocation(e);
            CFRelease(e);
            Some((p.x, p.y))
        }
    }
    pub fn left_button_down() -> bool {
        // kCGEventSourceStateCombinedSessionState = 0, kCGMouseButtonLeft = 0
        unsafe { CGEventSourceButtonState(0, 0) }
    }
}

#[cfg(target_os = "windows")]
mod native {
    use windows_sys::Win32::Foundation::POINT;
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_LBUTTON};
    use windows_sys::Win32::UI::WindowsAndMessaging::GetCursorPos;
    /// Virtual-desktop physical pixels (process is per-monitor DPI aware).
    pub fn pointer() -> Option<(f64, f64)> {
        let mut p = POINT { x: 0, y: 0 };
        (unsafe { GetCursorPos(&mut p) } != 0).then_some((p.x as f64, p.y as f64))
    }
    pub fn left_button_down() -> bool {
        (unsafe { GetAsyncKeyState(VK_LBUTTON as i32) } as u16 & 0x8000) != 0
    }
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
mod native {
    pub fn pointer() -> Option<(f64, f64)> {
        None
    }
    pub fn left_button_down() -> bool {
        false
    }
}

/// Whether the primary mouse / trackpad button is held right now.
pub fn left_button_down() -> bool {
    native::left_button_down()
}

/// Pointer position in OS input space, read directly from the OS (cheap,
/// any thread).
pub fn pointer_native() -> Option<Point> {
    native::pointer().map(|(x, y)| Point::new(x, y))
}

/// Pointer position in OS input space.
pub fn pointer<R: Runtime>(app: &AppHandle<R>, displays: &[Display]) -> Option<Point> {
    if let Some(p) = pointer_native() {
        return Some(p);
    }
    let p = app.cursor_position().ok()?;
    if cfg!(target_os = "macos") {
        // tao converts NSEvent.mouseLocation (global points) to "physical"
        // using the *primary* display's scale; undo exactly that.
        let primary = displays.iter().find(|d| d.is_primary).or(displays.first())?;
        Some(Point::new(p.x / primary.scale_factor, p.y / primary.scale_factor))
    } else {
        Some(Point::new(p.x, p.y))
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ActiveWindow {
    pub app: String,
    pub title: String,
    /// Window bounds in OS input space.
    pub frame: Option<Rect>,
}

impl ActiveWindow {
    /// Identity used to scope memory and to notice "the user switched away".
    pub fn key(&self) -> String {
        format!("{} — {}", self.app, self.title)
    }
}

pub fn active_window() -> Option<ActiveWindow> {
    let w = active_win_pos_rs::get_active_window().ok()?;
    let p = w.position;
    let frame = (p.width > 0.0 && p.height > 0.0).then(|| Rect::new(p.x, p.y, p.width, p.height));
    Some(ActiveWindow { app: w.app_name, title: w.title, frame })
}

/// The raw capture taken the instant the user invoked LUMA. Encoding for the
/// model happens later, once we know which mode the turn is in.
pub struct RawSnapshot {
    pub displays: Vec<Display>,
    pub display: Display,
    /// Password fields are already blacked out.
    pub full: DynamicImage,
    pub pointer_view: Option<Point>,
    pub window: ActiveWindow,
    /// Keyboard focus and selection in the front app, when readable.
    pub focus: Option<crate::ax::Focus>,
}

/// What a turn sends to the model.
pub struct Snapshot {
    pub displays: Vec<Display>,
    pub display: Display,
    pub images: Vec<EncodedImage>,
    pub window: ActiveWindow,
    pub focus: Option<crate::ax::Focus>,
    /// Pointer in image-1 model coordinates `(y, x)`.
    pub pointer_norm: Option<(f64, f64)>,
    pub pointer_norm_closeup: Option<(f64, f64)>,
}

impl Snapshot {
    pub fn context_key(&self) -> String {
        self.window.key()
    }
}

pub struct CaptureOptions {
    pub max_edge: u32,
    pub closeup: bool,
}

/// Capture one display's pixels.
pub fn capture_display(index: usize) -> Result<DynamicImage> {
    if !has_screen_permission() {
        return Err(anyhow!(request_screen_permission()));
    }
    let monitors = xcap::Monitor::all().map_err(|e| anyhow!("listing displays: {e}"))?;
    let monitor = monitors.get(index).context("display disappeared")?;
    let raw = monitor
        .capture_image()
        .map_err(|e| anyhow!("screen capture failed (is Screen Recording permission granted?): {e}"))?;
    Ok(DynamicImage::ImageRgba8(raw))
}

/// Capture the display under the pointer (or `prefer`), unencoded.
pub fn capture(displays: Vec<Display>, pointer: Option<Point>, prefer: Option<usize>) -> Result<RawSnapshot> {
    let display = prefer
        .and_then(|i| displays.iter().find(|d| d.index == i))
        .or_else(|| pointer.and_then(|p| display_at(&displays, p)))
        .or_else(|| displays.iter().find(|d| d.is_primary))
        .unwrap_or(&displays[0])
        .clone();
    let mut full = capture_display(display.index)?;
    let redacted = redact(&mut full, &display, &crate::ax::secure_fields());
    if redacted > 0 {
        log::debug!("blacked out {redacted} password field(s)");
    }
    Ok(RawSnapshot {
        pointer_view: pointer.map(|p| display.input_to_view(p)).filter(|p| display.view_bounds().contains(*p)),
        window: active_window().unwrap_or_default(),
        focus: crate::ax::focus(),
        displays,
        display,
        full,
    })
}

/// Paint `fields` (OS input space) solid black in a capture of `display`.
/// Returns how many touched the capture.
pub fn redact(img: &mut DynamicImage, display: &Display, fields: &[Rect]) -> usize {
    let cap = luma_core::geometry::Capture { display_index: display.index, width_px: img.width(), height_px: img.height() };
    let mut n = 0;
    for f in fields {
        let Some(r) = luma_core::geometry::input_rect_to_capture_px(f, display, &cap) else { continue };
        // a little margin so no glyph edges survive
        let r = Rect::new(r.x - 2.0, r.y - 2.0, r.w + 4.0, r.h + 4.0)
            .intersection(&Rect::new(0.0, 0.0, cap.width_px as f64, cap.height_px as f64));
        let Some(r) = r else { continue };
        let black = image::Rgba([0, 0, 0, 255]);
        let rgba = match img.as_mut_rgba8() {
            Some(b) => b,
            None => {
                *img = DynamicImage::ImageRgba8(img.to_rgba8());
                img.as_mut_rgba8().expect("rgba")
            }
        };
        for y in r.y.floor() as u32..(r.bottom().ceil() as u32).min(cap.height_px) {
            for x in r.x.floor() as u32..(r.right().ceil() as u32).min(cap.width_px) {
                rgba.put_pixel(x, y, black);
            }
        }
        n += 1;
    }
    n
}

impl RawSnapshot {
    pub fn encode(&self, opts: &CaptureOptions) -> Result<Snapshot> {
        let prepared =
            luma_net::vision::prepare(&self.full, &self.display, self.pointer_view, opts.max_edge, opts.closeup)?;
        Ok(Snapshot {
            displays: self.displays.clone(),
            display: self.display.clone(),
            images: prepared.images,
            window: self.window.clone(),
            focus: self.focus.clone(),
            pointer_norm: prepared.pointer_norm,
            pointer_norm_closeup: prepared.pointer_norm_closeup,
        })
    }
}

#[cfg(test)]
mod redact_tests {
    use super::*;

    #[test]
    fn password_fields_are_painted_black_in_capture_pixels() {
        // Retina: 2 capture px per point; field at input (100, 50) 40×10 pt.
        let d = Display {
            index: 0,
            name: "d".into(),
            input_frame: Rect::new(0.0, 0.0, 400.0, 200.0),
            input_per_point: 1.0,
            scale_factor: 2.0,
            is_primary: true,
        };
        let mut img = DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(800, 400, image::Rgba([200, 200, 200, 255])));
        assert_eq!(redact(&mut img, &d, &[Rect::new(100.0, 50.0, 40.0, 10.0), Rect::new(900.0, 0.0, 5.0, 5.0)]), 1);
        let px = |x, y| img.as_rgba8().unwrap().get_pixel(x, y).0;
        assert_eq!(px(200, 100), [0, 0, 0, 255]);
        assert_eq!(px(279, 119), [0, 0, 0, 255]);
        assert_eq!(px(290, 130), [200, 200, 200, 255]);
    }
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    #[test]
    fn native_pointer_reads_without_main_thread() {
        let h = std::thread::spawn(|| (super::pointer_native(), super::left_button_down()));
        let (p, down) = h.join().unwrap();
        let p = p.expect("pointer");
        assert!(p.x.is_finite() && p.y.is_finite());
        let displays = super::displays().unwrap();
        assert!(luma_core::geometry::display_at(&displays, p).is_some());
        println!("pointer at {p:?}, button down: {down}, displays: {:?}", displays);
    }
}

#[cfg(all(test, target_os = "macos"))]
mod bench {
    /// `cargo test -p luma bench_capture -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn bench_capture_and_encode() {
        let displays = super::displays().unwrap();
        let p = super::pointer_native();
        let t = std::time::Instant::now();
        let raw = super::capture(displays, p, None).unwrap();
        let cap = t.elapsed().as_millis();
        for (edge, closeup) in [(1280, true), (1920, true)] {
            let t = std::time::Instant::now();
            let s = raw.encode(&super::CaptureOptions { max_edge: edge, closeup }).unwrap();
            let bytes: usize = s.images.iter().map(|i| i.jpeg.len()).sum();
            println!("capture {cap} ms; encode {edge}: {} ms, {} KB", t.elapsed().as_millis(), bytes / 1024);
        }
    }
}
