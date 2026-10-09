//! Reads UI element frames from the OS accessibility tree, for snapping the
//! model's boxes to exact control edges (see `luma_core::snap`).
//!
//! One hit test at the target plus a short walk up its ancestors: a few
//! milliseconds, never a walk of the whole tree. AXUIElement on macOS (needs
//! Accessibility permission; without it this quietly returns nothing and the
//! model's own boxes are used), UI Automation on Windows.

use luma_core::geometry::Point;
use luma_core::snap::Element;

/// The element at `at` (OS input space) and its ancestors, innermost first.
pub fn elements_at(at: Point) -> Vec<Element> {
    if !crate::input::has_control_permission() {
        return Vec::new();
    }
    native::elements_at(at)
}

/// The keyboard focus in the app the user is working in.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Focus {
    pub role: String,
    pub name: String,
    /// Selected text, trimmed to `MAX_SELECTION` characters.
    pub selection: Option<String>,
    /// A password field: nothing about it is sent to the model.
    pub secure: bool,
    /// Frame in OS input space.
    pub frame: Option<luma_core::geometry::Rect>,
}

pub const MAX_SELECTION: usize = 2000;

/// What has keyboard focus, and what is selected in it.
pub fn focus() -> Option<Focus> {
    if !crate::input::has_control_permission() {
        return None;
    }
    let mut f = native::focus()?;
    if f.secure {
        f.selection = None;
    }
    f.selection = f.selection.map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).map(|s| {
        if s.chars().count() > MAX_SELECTION {
            s.chars().take(MAX_SELECTION).collect::<String>() + "…"
        } else {
            s
        }
    });
    Some(f)
}

/// Frames (OS input space) of password fields in the front window, so they
/// can be blacked out before a screenshot leaves the machine. Bounded walk:
/// at most a few hundred elements and ~80 ms.
pub fn secure_fields() -> Vec<luma_core::geometry::Rect> {
    if !crate::input::has_control_permission() {
        return Vec::new();
    }
    native::secure_fields()
}

/// Press the control at `at` (OS input space) through accessibility
/// (AXPress / UIA Invoke) instead of a synthetic click. Only buttons, links,
/// menu items and similar; returns false when nothing suitable is there, so
/// the caller falls back to a real click.
pub fn press_at(at: Point) -> bool {
    crate::input::has_control_permission() && native::press_at(at)
}

/// The text value of the focused field (for checking that typing landed).
/// None for password fields or when unreadable.
pub fn focused_value() -> Option<String> {
    if !crate::input::has_control_permission() {
        return None;
    }
    native::focused_value()
}

/// Roles that a press can safely stand in for a click on.
pub const PRESSABLE: &[&str] = &[
    "AXButton", "AXMenuItem", "AXMenuBarItem", "AXMenuButton", "AXPopUpButton", "AXLink", "AXCheckBox", "AXRadioButton",
    "AXTab", "AXDisclosureTriangle", "AXToolbarButton",
    "Button", "MenuItem", "Hyperlink", "SplitButton",
];

/// Walk budget for `secure_fields`.
const WALK_MAX_NODES: usize = 600;
const WALK_MAX_MS: u128 = 80;

#[cfg(target_os = "macos")]
mod native {
    use super::{Element, Focus, Point};
    use luma_core::geometry::Rect;
    use std::ffi::c_void;

    type CFTypeRef = *const c_void;
    type AXUIElementRef = *const c_void;

    #[repr(C)]
    #[derive(Default)]
    struct CGPoint {
        x: f64,
        y: f64,
    }
    #[repr(C)]
    #[derive(Default)]
    struct CGSize {
        w: f64,
        h: f64,
    }

    const AX_OK: i32 = 0;
    const AX_VALUE_CGPOINT: u32 = 1;
    const AX_VALUE_CGSIZE: u32 = 2;
    const UTF8: u32 = 0x0800_0100;
    /// Ancestors past this are panes and windows, which never win a snap.
    const MAX_DEPTH: usize = 8;

    #[link(name = "ApplicationServices", kind = "framework")]
    extern "C" {
        fn AXUIElementCreateSystemWide() -> AXUIElementRef;
        fn AXUIElementCreateApplication(pid: i32) -> AXUIElementRef;
        fn AXUIElementCopyElementAtPosition(el: AXUIElementRef, x: f32, y: f32, out: *mut AXUIElementRef) -> i32;
        fn AXUIElementCopyAttributeValue(el: AXUIElementRef, attr: CFTypeRef, out: *mut CFTypeRef) -> i32;
        fn AXUIElementSetAttributeValue(el: AXUIElementRef, attr: CFTypeRef, value: CFTypeRef) -> i32;
        fn AXUIElementPerformAction(el: AXUIElementRef, action: CFTypeRef) -> i32;
        fn AXUIElementGetPid(el: AXUIElementRef, pid: *mut i32) -> i32;
        fn AXUIElementSetMessagingTimeout(el: AXUIElementRef, seconds: f32) -> i32;
        fn AXValueGetValue(v: CFTypeRef, kind: u32, out: *mut c_void) -> bool;
    }
    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        static kCFBooleanTrue: CFTypeRef;
        fn CFRelease(cf: CFTypeRef);
        fn CFGetTypeID(cf: CFTypeRef) -> usize;
        fn CFStringGetTypeID() -> usize;
        fn CFStringCreateWithBytes(alloc: CFTypeRef, bytes: *const u8, len: isize, enc: u32, ext: bool) -> CFTypeRef;
        fn CFStringGetCString(s: CFTypeRef, buf: *mut u8, size: isize, enc: u32) -> bool;
        fn CFStringGetLength(s: CFTypeRef) -> isize;
        fn CFStringGetMaximumSizeForEncoding(len: isize, enc: u32) -> isize;
        fn CFArrayGetTypeID() -> usize;
        fn CFArrayGetCount(a: CFTypeRef) -> isize;
        fn CFArrayGetValueAtIndex(a: CFTypeRef, i: isize) -> CFTypeRef;
        fn CFRetain(cf: CFTypeRef) -> CFTypeRef;
        fn CFStringCreateWithSubstring(alloc: CFTypeRef, s: CFTypeRef, range: CFRange) -> CFTypeRef;
    }

    #[repr(C)]
    struct CFRange {
        location: isize,
        length: isize,
    }

    /// An owned CoreFoundation reference.
    struct Cf(CFTypeRef);
    impl Drop for Cf {
        fn drop(&mut self) {
            if !self.0.is_null() {
                unsafe { CFRelease(self.0) }
            }
        }
    }

    fn cfstr(s: &str) -> Cf {
        Cf(unsafe { CFStringCreateWithBytes(std::ptr::null(), s.as_ptr(), s.len() as isize, UTF8, false) })
    }

    fn attr(el: AXUIElementRef, name: &str) -> Option<Cf> {
        let key = cfstr(name);
        let mut out: CFTypeRef = std::ptr::null();
        let err = unsafe { AXUIElementCopyAttributeValue(el, key.0, &mut out) };
        (err == AX_OK && !out.is_null()).then_some(Cf(out))
    }

    fn string(el: AXUIElementRef, name: &str) -> String {
        let Some(v) = attr(el, name) else { return String::new() };
        cf_to_string(&v, 512)
    }

    /// A CFString as UTF-8, up to `max_chars` UTF-16 units (a huge
    /// selection is cut before it is copied, not after).
    fn cf_to_string(v: &Cf, max_chars: isize) -> String {
        unsafe {
            if CFGetTypeID(v.0) != CFStringGetTypeID() {
                return String::new();
            }
            let len = CFStringGetLength(v.0);
            let cut;
            let s = if len > max_chars {
                cut = Cf(CFStringCreateWithSubstring(std::ptr::null(), v.0, CFRange { location: 0, length: max_chars }));
                if cut.0.is_null() {
                    return String::new();
                }
                cut.0
            } else {
                v.0
            };
            let size = CFStringGetMaximumSizeForEncoding(len.min(max_chars), UTF8) + 1;
            let mut buf = vec![0u8; size.max(1) as usize];
            if !CFStringGetCString(s, buf.as_mut_ptr(), buf.len() as isize, UTF8) {
                return String::new();
            }
            let end = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
            String::from_utf8_lossy(&buf[..end]).into_owned()
        }
    }

    /// Children of an element (retained).
    fn children(el: AXUIElementRef) -> Vec<Cf> {
        let Some(arr) = attr(el, "AXChildren") else { return Vec::new() };
        unsafe {
            if CFGetTypeID(arr.0) != CFArrayGetTypeID() {
                return Vec::new();
            }
            (0..CFArrayGetCount(arr.0))
                .map(|i| CFArrayGetValueAtIndex(arr.0, i))
                .filter(|v| !v.is_null())
                .map(|v| Cf(CFRetain(v)))
                .collect()
        }
    }

    fn is_secure(el: AXUIElementRef) -> bool {
        string(el, "AXSubrole") == "AXSecureTextField" || string(el, "AXRole") == "AXSecureTextField"
    }

    /// The front app, skipping LUMA itself. (The system-wide
    /// AXFocusedApplication attribute is unreliable; the front window's
    /// process id is not.)
    fn focused_app() -> Option<Cf> {
        let pid = active_win_pos_rs::get_active_window().ok()?.process_id as i32;
        if pid == std::process::id() as i32 || pid <= 0 {
            return None;
        }
        let app = Cf(unsafe { AXUIElementCreateApplication(pid) });
        unsafe { AXUIElementSetMessagingTimeout(app.0, 0.15) };
        enable_tree(&app, pid);
        Some(app)
    }

    /// Chromium and Electron apps (Chrome, Slack, VS Code, …) only build
    /// their accessibility tree once an assistive client asks for it. Ask
    /// once per process; other apps ignore the attribute.
    fn enable_tree(app: &Cf, pid: i32) {
        static ENABLED: std::sync::Mutex<Vec<i32>> = std::sync::Mutex::new(Vec::new());
        let mut done = ENABLED.lock().unwrap();
        if done.contains(&pid) {
            return;
        }
        done.push(pid);
        let key = cfstr("AXManualAccessibility");
        let err = unsafe { AXUIElementSetAttributeValue(app.0, key.0, kCFBooleanTrue) };
        log::debug!("AXManualAccessibility on pid {pid}: {err}");
    }

    pub fn focus() -> Option<Focus> {
        let app = focused_app()?;
        let el = attr(app.0, "AXFocusedUIElement")?;
        let secure = is_secure(el.0);
        let mut name = string(el.0, "AXTitle");
        if name.is_empty() {
            name = string(el.0, "AXDescription");
        }
        if name.is_empty() {
            name = string(el.0, "AXPlaceholderValue");
        }
        let selection = if secure {
            None
        } else {
            attr(el.0, "AXSelectedText").map(|v| cf_to_string(&v, super::MAX_SELECTION as isize + 1))
        };
        Some(Focus { role: string(el.0, "AXRole"), name, selection, secure, frame: frame(el.0) })
    }

    pub fn secure_fields() -> Vec<Rect> {
        let Some(app) = focused_app() else { return Vec::new() };
        let mut out = Vec::new();
        // The focused field first: it is the one most likely on screen.
        if let Some(el) = attr(app.0, "AXFocusedUIElement") {
            if is_secure(el.0) {
                out.extend(frame(el.0));
            }
        }
        let Some(win) = attr(app.0, "AXFocusedWindow") else { return out };
        let t = std::time::Instant::now();
        let mut queue = std::collections::VecDeque::from([win]);
        let mut seen = 0;
        while let Some(el) = queue.pop_front() {
            seen += 1;
            if seen > super::WALK_MAX_NODES || t.elapsed().as_millis() > super::WALK_MAX_MS {
                log::debug!("secure-field walk stopped after {seen} elements");
                break;
            }
            if is_secure(el.0) {
                if let Some(f) = frame(el.0) {
                    if !out.contains(&f) {
                        out.push(f);
                    }
                }
                continue;
            }
            queue.extend(children(el.0));
        }
        out
    }

    fn frame(el: AXUIElementRef) -> Option<Rect> {
        let pos = attr(el, "AXPosition")?;
        let size = attr(el, "AXSize")?;
        let mut p = CGPoint::default();
        let mut s = CGSize::default();
        unsafe {
            if !AXValueGetValue(pos.0, AX_VALUE_CGPOINT, &mut p as *mut _ as *mut c_void)
                || !AXValueGetValue(size.0, AX_VALUE_CGSIZE, &mut s as *mut _ as *mut c_void)
            {
                return None;
            }
        }
        Some(Rect::new(p.x, p.y, s.w, s.h))
    }

    fn hit(root: AXUIElementRef, at: Point) -> Option<Cf> {
        let mut out: AXUIElementRef = std::ptr::null();
        let err = unsafe { AXUIElementCopyElementAtPosition(root, at.x as f32, at.y as f32, &mut out) };
        (err == AX_OK && !out.is_null()).then_some(Cf(out))
    }

    fn pid_of(el: AXUIElementRef) -> i32 {
        let mut pid = 0;
        unsafe { AXUIElementGetPid(el, &mut pid) };
        pid
    }

    /// The element at `at` in the app the user is working in.
    fn element_at(at: Point) -> Option<Cf> {
        let me = std::process::id() as i32;
        let system = Cf(unsafe { AXUIElementCreateSystemWide() });
        // A hung app must not stall the answer.
        unsafe { AXUIElementSetMessagingTimeout(system.0, 0.15) };
        let el = hit(system.0, at)?;
        // LUMA's own overlay sits on top of everything; look beneath it in
        // the app the user is working in.
        if pid_of(el.0) != me {
            return Some(el);
        }
        let front = active_win_pos_rs::get_active_window().ok().map(|w| w.process_id as i32)?;
        if front == me {
            return None;
        }
        let app = Cf(unsafe { AXUIElementCreateApplication(front) });
        unsafe { AXUIElementSetMessagingTimeout(app.0, 0.15) };
        hit(app.0, at)
    }

    pub fn press_at(at: Point) -> bool {
        let Some(mut el) = element_at(at) else { return false };
        // The hit is often a label or image inside the button: try a couple of parents.
        for _ in 0..3 {
            let role = string(el.0, "AXRole");
            let inside = frame(el.0).is_some_and(|f| f.contains(at) && f.w * f.h < 400.0 * 200.0);
            if inside && super::PRESSABLE.contains(&role.as_str()) {
                let action = cfstr("AXPress");
                let err = unsafe { AXUIElementPerformAction(el.0, action.0) };
                log::debug!("AXPress on {role} {:?}: {err}", string(el.0, "AXTitle"));
                return err == AX_OK;
            }
            match attr(el.0, "AXParent") {
                Some(p) => el = p,
                None => break,
            }
        }
        false
    }

    /// Centre of the first button titled `title` in app `pid` (tests only).
    #[cfg(test)]
    pub fn find_button(pid: i32, title: &str) -> Option<Point> {
        let app = Cf(unsafe { AXUIElementCreateApplication(pid) });
        let mut queue = std::collections::VecDeque::from([app]);
        let mut seen = 0;
        while let Some(el) = queue.pop_front() {
            seen += 1;
            if seen > 500 {
                return None;
            }
            if string(el.0, "AXRole") == "AXButton" && string(el.0, "AXTitle") == title {
                return frame(el.0).map(|f| f.center());
            }
            queue.extend(children(el.0));
        }
        None
    }

    pub fn focused_value() -> Option<String> {
        let app = focused_app()?;
        let el = attr(app.0, "AXFocusedUIElement")?;
        if is_secure(el.0) {
            return None;
        }
        attr(el.0, "AXValue").map(|v| cf_to_string(&v, 4000))
    }

    pub fn elements_at(at: Point) -> Vec<Element> {
        let Some(mut el) = element_at(at) else { return Vec::new() };
        let mut out = Vec::new();
        for _ in 0..MAX_DEPTH {
            if let Some(f) = frame(el.0) {
                let role = string(el.0, "AXRole");
                let mut name = string(el.0, "AXTitle");
                if name.is_empty() {
                    name = string(el.0, "AXDescription");
                }
                if role == "AXApplication" {
                    break;
                }
                out.push(Element { frame: f, role, name });
            }
            match attr(el.0, "AXParent") {
                Some(p) => el = p,
                None => break,
            }
        }
        out
    }
}

#[cfg(target_os = "windows")]
mod native {
    use super::{Element, Focus, Point};
    use luma_core::geometry::Rect;
    use windows::core::Interface;
    use windows::Win32::Foundation::POINT;
    use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED};
    use windows::Win32::System::Variant::VARIANT;
    use windows::Win32::UI::Accessibility::{
        CUIAutomation, IUIAutomation, IUIAutomation2, IUIAutomationElement, IUIAutomationInvokePattern,
        IUIAutomationTextPattern, IUIAutomationValuePattern, TreeScope_Descendants, UIA_InvokePatternId,
        UIA_IsPasswordPropertyId, UIA_TextPatternId, UIA_ValuePatternId,
    };
    use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId};

    /// Ancestors past this are panes and windows, which never win a snap.
    const MAX_DEPTH: usize = 8;
    /// A hung app must not stall the answer (ms).
    const TIMEOUT_MS: u32 = 150;

    /// UIA control type ids 50000.. in order, named as `luma_core::snap` expects.
    const CONTROL_TYPES: &[&str] = &[
        "Button", "Calendar", "CheckBox", "ComboBox", "Edit", "Hyperlink", "Image", "ListItem", "List",
        "Menu", "MenuBar", "MenuItem", "ProgressBar", "RadioButton", "ScrollBar", "Slider", "Spinner",
        "StatusBar", "Tab", "TabItem", "Text", "ToolBar", "ToolTip", "Tree", "TreeItem", "Custom", "Group",
        "Thumb", "DataGrid", "DataItem", "Document", "SplitButton", "Window", "Pane", "Header",
        "HeaderItem", "Table", "TitleBar", "Separator", "SemanticZoom", "AppBar",
    ];

    fn role(id: i32) -> String {
        usize::try_from(id - 50_000)
            .ok()
            .and_then(|i| CONTROL_TYPES.get(i))
            .map_or_else(|| "Unknown".to_string(), |s| s.to_string())
    }

    thread_local! {
        /// One automation client per thread; creating it costs a few ms.
        static UIA: Option<IUIAutomation> = unsafe {
            // S_FALSE / RPC_E_CHANGED_MODE just mean COM is already set up here.
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            let uia: Option<IUIAutomation> = CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER).ok();
            if let Some(u2) = uia.as_ref().and_then(|u| u.cast::<IUIAutomation2>().ok()) {
                let _ = u2.SetConnectionTimeout(TIMEOUT_MS);
                let _ = u2.SetTransactionTimeout(TIMEOUT_MS);
            }
            uia
        };
    }

    fn element(el: &IUIAutomationElement) -> Option<Element> {
        Some(Element {
            frame: frame(el)?,
            role: unsafe { el.CurrentControlType() }.map(|t| role(t.0)).unwrap_or_default(),
            name: unsafe { el.CurrentName() }.map(|n| n.to_string()).unwrap_or_default(),
        })
    }

    fn frame(el: &IUIAutomationElement) -> Option<Rect> {
        let r = unsafe { el.CurrentBoundingRectangle() }.ok()?;
        (r.right > r.left && r.bottom > r.top)
            .then(|| Rect::new(r.left as f64, r.top as f64, (r.right - r.left) as f64, (r.bottom - r.top) as f64))
    }

    fn is_own(el: &IUIAutomationElement) -> bool {
        unsafe { el.CurrentProcessId() }.ok() == Some(std::process::id() as i32)
    }

    pub fn focus() -> Option<Focus> {
        UIA.with(|uia| {
            let uia = uia.as_ref()?;
            let el = unsafe { uia.GetFocusedElement() }.ok()?;
            if is_own(&el) {
                return None;
            }
            let secure = unsafe { el.CurrentIsPassword() }.map(|b| b.as_bool()).unwrap_or(false);
            let selection = if secure {
                None
            } else {
                unsafe {
                    el.GetCurrentPatternAs::<IUIAutomationTextPattern>(UIA_TextPatternId)
                        .and_then(|tp| tp.GetSelection())
                        .ok()
                        .and_then(|ranges| {
                            let mut text = String::new();
                            for i in 0..ranges.Length().unwrap_or(0) {
                                if let Ok(t) = ranges.GetElement(i).and_then(|r| r.GetText(super::MAX_SELECTION as i32 + 1)) {
                                    text.push_str(&t.to_string());
                                }
                            }
                            (!text.is_empty()).then_some(text)
                        })
                }
            };
            Some(Focus {
                role: unsafe { el.CurrentControlType() }.map(|t| role(t.0)).unwrap_or_default(),
                name: unsafe { el.CurrentName() }.map(|n| n.to_string()).unwrap_or_default(),
                selection,
                secure,
                frame: frame(&el),
            })
        })
    }

    pub fn press_at(at: Point) -> bool {
        UIA.with(|uia| {
            let Some(uia) = uia.as_ref() else { return false };
            let pt = POINT { x: at.x.round() as i32, y: at.y.round() as i32 };
            let Ok(mut el) = (unsafe { uia.ElementFromPoint(pt) }) else { return false };
            if is_own(&el) {
                return false;
            }
            let Ok(walker) = (unsafe { uia.ControlViewWalker() }) else { return false };
            for _ in 0..3 {
                let role = unsafe { el.CurrentControlType() }.map(|t| role(t.0)).unwrap_or_default();
                let inside = frame(&el).is_some_and(|f| f.contains(at) && f.w * f.h < 800.0 * 400.0);
                if inside && super::PRESSABLE.contains(&role.as_str()) {
                    return unsafe { el.GetCurrentPatternAs::<IUIAutomationInvokePattern>(UIA_InvokePatternId).and_then(|p| p.Invoke()) }.is_ok();
                }
                match unsafe { walker.GetParentElement(&el) } {
                    Ok(p) => el = p,
                    Err(_) => break,
                }
            }
            false
        })
    }

    pub fn focused_value() -> Option<String> {
        UIA.with(|uia| unsafe {
            let el = uia.as_ref()?.GetFocusedElement().ok()?;
            if is_own(&el) || el.CurrentIsPassword().map(|b| b.as_bool()).unwrap_or(true) {
                return None;
            }
            el.GetCurrentPatternAs::<IUIAutomationValuePattern>(UIA_ValuePatternId).and_then(|p| p.CurrentValue()).ok().map(|v| v.to_string())
        })
    }

    pub fn secure_fields() -> Vec<Rect> {
        UIA.with(|uia| {
            let Some(uia) = uia.as_ref() else { return Vec::new() };
            unsafe {
                let hwnd = GetForegroundWindow();
                let mut pid = 0u32;
                GetWindowThreadProcessId(hwnd, Some(&mut pid));
                if hwnd.is_invalid() || pid == std::process::id() {
                    return Vec::new();
                }
                let Ok(root) = uia.ElementFromHandle(hwnd) else { return Vec::new() };
                let Ok(cond) = uia.CreatePropertyCondition(UIA_IsPasswordPropertyId, &VARIANT::from(true)) else {
                    return Vec::new();
                };
                // The transaction timeout bounds this search in a hung app.
                let Ok(found) = root.FindAll(TreeScope_Descendants, &cond) else { return Vec::new() };
                (0..found.Length().unwrap_or(0).min(super::WALK_MAX_NODES as i32))
                    .filter_map(|i| found.GetElement(i).ok())
                    .filter_map(|e| frame(&e))
                    .collect()
            }
        })
    }

    pub fn elements_at(at: Point) -> Vec<Element> {
        UIA.with(|uia| {
            let Some(uia) = uia else { return Vec::new() };
            // Input space is physical pixels and LUMA is per-monitor DPI
            // aware, which is what UIA expects and returns.
            let pt = POINT { x: at.x.round() as i32, y: at.y.round() as i32 };
            let Ok(mut el) = (unsafe { uia.ElementFromPoint(pt) }) else { return Vec::new() };
            // The overlay is click-through, so hit testing normally passes
            // beneath it; if it does not, there is nothing honest to snap to.
            if is_own(&el) {
                return Vec::new();
            }
            let Ok(walker) = (unsafe { uia.ControlViewWalker() }) else { return Vec::new() };
            let mut out = Vec::new();
            for _ in 0..MAX_DEPTH {
                if let Some(e) = element(&el) {
                    let top = e.role == "Window";
                    out.push(e);
                    if top {
                        break;
                    }
                }
                match unsafe { walker.GetParentElement(&el) } {
                    Ok(p) => el = p,
                    Err(_) => break,
                }
            }
            out
        })
    }
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
mod native {
    use super::{Element, Focus, Point};
    pub fn elements_at(_at: Point) -> Vec<Element> {
        Vec::new()
    }
    pub fn focus() -> Option<Focus> {
        None
    }
    pub fn secure_fields() -> Vec<luma_core::geometry::Rect> {
        Vec::new()
    }
    pub fn press_at(_at: Point) -> bool {
        false
    }
    pub fn focused_value() -> Option<String> {
        None
    }
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    /// Needs Accessibility permission for the test runner; run with
    /// `cargo test -p luma ax -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn menu_bar_apple_menu_has_a_frame() {
        if !crate::input::has_control_permission() {
            eprintln!("skipped: no Accessibility permission");
            return;
        }
        let t = std::time::Instant::now();
        let els = super::elements_at(luma_core::geometry::Point::new(18.0, 10.0));
        println!("{} ms: {els:#?}", t.elapsed().as_millis());
        assert!(!els.is_empty());
    }

    /// Presses a button in a throwaway dialog through accessibility and
    /// checks the dialog reports it. `cargo test -p luma ax::tests::press -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn press_a_dialog_button() {
        if !crate::input::has_control_permission() {
            eprintln!("skipped: no Accessibility permission");
            return;
        }
        let child = std::process::Command::new("osascript")
            .args(["-e", r#"display dialog "LUMA accessibility press test" buttons {"Cancel", "Press me"} default button "Cancel" giving up after 15"#])
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
        std::thread::sleep(std::time::Duration::from_millis(1500));
        let at = super::native::find_button(child.id() as i32, "Press me").expect("dialog button");
        println!("button centre {at:?}");
        let pressed = super::press_at(at);
        let out = child.wait_with_output().unwrap();
        let said = String::from_utf8_lossy(&out.stdout);
        println!("pressed={pressed} dialog said {said:?}");
        assert!(pressed && said.contains("Press me"));
    }

    /// `cargo test -p luma ax::tests::focus -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn focus_and_secure_fields_probe() {
        if !crate::input::has_control_permission() {
            eprintln!("skipped: no Accessibility permission");
            return;
        }
        let t = std::time::Instant::now();
        let f = super::focus();
        let ms_focus = t.elapsed().as_millis();
        let t = std::time::Instant::now();
        let s = super::secure_fields();
        println!("focus {ms_focus} ms: {f:?}\nsecure fields {} ms: {s:?}", t.elapsed().as_millis());
        assert!(t.elapsed().as_millis() < 400);
    }
}
