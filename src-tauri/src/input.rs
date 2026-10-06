//! Synthetic input on the user's real desktop: mouse, keyboard, scrolling.
//!
//! macOS requires Accessibility permission for this; we check and guide the
//! user instead of failing silently. The real cursor glides to each target
//! so the user always sees what LUMA is about to do.

use anyhow::{anyhow, Result};
use enigo::{Axis, Button, Coordinate, Direction, Enigo, Key, Keyboard, Mouse, Settings};
use luma_core::action::MouseButton;
use luma_core::geometry::Point;
use std::thread::sleep;
use std::time::Duration;

#[cfg(target_os = "macos")]
#[link(name = "ApplicationServices", kind = "framework")]
extern "C" {
    fn AXIsProcessTrusted() -> bool;
}

pub fn has_control_permission() -> bool {
    #[cfg(target_os = "macos")]
    unsafe {
        AXIsProcessTrusted()
    }
    #[cfg(not(target_os = "macos"))]
    true
}

#[derive(Debug)]
pub struct NoControlPermission;
impl std::fmt::Display for NoControlPermission {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "To click and type for you I need Accessibility access. In System Settings → Privacy & Security → Accessibility, turn on the app LUMA runs in, then quit and reopen it."
        )
    }
}
impl std::error::Error for NoControlPermission {}

pub fn request_control_permission() -> NoControlPermission {
    #[cfg(target_os = "macos")]
    {
        let _ = std::process::Command::new("open")
            .arg("x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility")
            .spawn();
    }
    NoControlPermission
}

fn enigo() -> Result<Enigo> {
    Enigo::new(&Settings { open_prompt_to_get_permissions: false, ..Settings::default() })
        .map_err(|e| anyhow!("input unavailable: {e}"))
}

/// Glide the real cursor to `to` (OS input space) over ~250 ms.
pub fn glide_to(to: Point) -> Result<()> {
    let mut e = enigo()?;
    let (fx, fy) = e.location().unwrap_or((to.x as i32, to.y as i32));
    let steps = 14;
    for i in 1..=steps {
        let t = i as f64 / steps as f64;
        let ease = 1.0 - (1.0 - t).powi(3);
        let x = fx as f64 + (to.x - fx as f64) * ease;
        let y = fy as f64 + (to.y - fy as f64) * ease;
        e.move_mouse(x.round() as i32, y.round() as i32, Coordinate::Abs).map_err(|e| anyhow!("{e}"))?;
        sleep(Duration::from_millis(18));
    }
    Ok(())
}

pub fn click(at: Point, button: MouseButton, double: bool) -> Result<()> {
    glide_to(at)?;
    sleep(Duration::from_millis(60));
    let mut e = enigo()?;
    let b = match button {
        MouseButton::Left => Button::Left,
        MouseButton::Right => Button::Right,
    };
    e.button(b, Direction::Click).map_err(|e| anyhow!("{e}"))?;
    if double {
        sleep(Duration::from_millis(70));
        e.button(b, Direction::Click).map_err(|e| anyhow!("{e}"))?;
    }
    Ok(())
}

pub fn type_text(text: &str, clear_first: bool) -> Result<()> {
    let mut e = enigo()?;
    if clear_first {
        press_keys(&["cmd_or_ctrl".into(), "a".into()])?;
        sleep(Duration::from_millis(60));
    }
    e.text(text).map_err(|e| anyhow!("{e}"))
}

fn key_of(name: &str) -> Result<Key> {
    Ok(match name {
        "cmd" => {
            if cfg!(target_os = "macos") {
                Key::Meta
            } else {
                Key::Control
            }
        }
        "cmd_or_ctrl" => {
            if cfg!(target_os = "macos") {
                Key::Meta
            } else {
                Key::Control
            }
        }
        "ctrl" => Key::Control,
        "alt" => Key::Alt,
        "shift" => Key::Shift,
        "enter" => Key::Return,
        "tab" => Key::Tab,
        "escape" => Key::Escape,
        "space" => Key::Space,
        "backspace" => Key::Backspace,
        "delete" => Key::Delete,
        "up" => Key::UpArrow,
        "down" => Key::DownArrow,
        "left" => Key::LeftArrow,
        "right" => Key::RightArrow,
        "home" => Key::Home,
        "end" => Key::End,
        "pageup" => Key::PageUp,
        "pagedown" => Key::PageDown,
        f if f.starts_with('f') && f.len() > 1 => match f[1..].parse::<u8>() {
            Ok(1) => Key::F1,
            Ok(2) => Key::F2,
            Ok(3) => Key::F3,
            Ok(4) => Key::F4,
            Ok(5) => Key::F5,
            Ok(6) => Key::F6,
            Ok(7) => Key::F7,
            Ok(8) => Key::F8,
            Ok(9) => Key::F9,
            Ok(10) => Key::F10,
            Ok(11) => Key::F11,
            Ok(12) => Key::F12,
            _ => return Err(anyhow!("unknown key {f}")),
        },
        c if c.chars().count() == 1 => Key::Unicode(c.chars().next().unwrap()),
        other => return Err(anyhow!("unknown key {other}")),
    })
}

/// Hold modifiers, tap the final key, release in reverse order.
pub fn press_keys(keys: &[String]) -> Result<()> {
    let mut e = enigo()?;
    let parsed: Vec<Key> = keys.iter().map(|k| key_of(k)).collect::<Result<_>>()?;
    let (last, mods) = parsed.split_last().ok_or_else(|| anyhow!("no keys"))?;
    for m in mods {
        e.key(*m, Direction::Press).map_err(|e| anyhow!("{e}"))?;
    }
    let r = e.key(*last, Direction::Click).map_err(|e| anyhow!("{e}"));
    for m in mods.iter().rev() {
        let _ = e.key(*m, Direction::Release);
    }
    r
}

pub fn scroll(at: Option<Point>, down: bool, amount: i32) -> Result<()> {
    if let Some(p) = at {
        glide_to(p)?;
    }
    let mut e = enigo()?;
    e.scroll(if down { amount } else { -amount }, Axis::Vertical).map_err(|e| anyhow!("{e}"))
}

pub fn open_url(url: &str) -> Result<()> {
    #[cfg(target_os = "macos")]
    let r = std::process::Command::new("open").arg(url).spawn();
    #[cfg(target_os = "windows")]
    let r = std::process::Command::new("rundll32").args(["url.dll,FileProtocolHandler", url]).spawn();
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let r = std::process::Command::new("xdg-open").arg(url).spawn();
    r.map(|_| ()).map_err(|e| anyhow!("couldn't open {url}: {e}"))
}
