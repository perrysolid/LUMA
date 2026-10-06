mod audio;
mod companion;
mod screen;
mod settings;

use companion::{Companion, Phase};
use luma_core::geometry::Display;
use luma_core::session::Session;
use serde::Serialize;
use settings::{get_key, key_source, set_key, KeySource, Prefs, Provider};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager, WebviewUrl, WebviewWindowBuilder};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, ShortcutState};

pub struct AppState {
    pub prefs: Mutex<Prefs>,
    pub prefs_path: PathBuf,
    pub session: Mutex<Session>,
    pub companion: Arc<Companion>,
}

#[derive(Serialize)]
struct SettingsView {
    prefs: Prefs,
    /// Where each configured key comes from. The keys themselves never reach the UI.
    keys: std::collections::BTreeMap<String, Option<KeySource>>,
    platform: &'static str,
}

#[tauri::command]
fn get_settings(state: tauri::State<AppState>) -> SettingsView {
    SettingsView {
        prefs: state.prefs.lock().unwrap().clone(),
        keys: Provider::ALL
            .iter()
            .map(|p| (format!("{p:?}").to_lowercase(), key_source(*p)))
            .collect(),
        platform: std::env::consts::OS,
    }
}

#[tauri::command]
fn save_key(provider: Provider, key: String) -> Result<(), String> {
    set_key(provider, &key).map_err(|e| e.to_string())
}

#[tauri::command]
fn save_prefs(app: AppHandle, state: tauri::State<AppState>, prefs: Prefs) -> Result<(), String> {
    let old_hotkey = state.prefs.lock().unwrap().hotkey.clone();
    if prefs.hotkey != old_hotkey {
        register_hotkey(&app, &prefs.hotkey).map_err(|e| format!("couldn't use that shortcut: {e}"))?;
        let _ = app.global_shortcut().unregister(old_hotkey.as_str());
    }
    prefs.save(&state.prefs_path).map_err(|e| e.to_string())?;
    *state.prefs.lock().unwrap() = prefs;
    refresh_tray(&app);
    Ok(())
}

#[tauri::command]
fn ask(app: AppHandle, state: tauri::State<AppState>, question: String) {
    if !question.trim().is_empty() {
        state.companion.ask_text(&app, question.trim().to_string());
    }
}

#[tauri::command]
fn stop(app: AppHandle, state: tauri::State<AppState>) {
    state.companion.interrupt(&app);
    state.companion.status(&app, Phase::Idle, None);
}

#[tauri::command]
fn clear_session(app: AppHandle, state: tauri::State<AppState>) {
    state.companion.interrupt(&app);
    state.session.lock().unwrap().clear();
    state.companion.status(&app, Phase::Idle, Some("Session cleared.".into()));
}

#[tauri::command]
fn set_paused(app: AppHandle, state: tauri::State<AppState>, paused: bool) {
    set_paused_inner(&app, &state, paused);
}

fn set_paused_inner(app: &AppHandle, state: &AppState, paused: bool) {
    if paused {
        state.companion.interrupt(app);
    }
    {
        let mut p = state.prefs.lock().unwrap();
        p.paused = paused;
        let _ = p.save(&state.prefs_path);
    }
    state.companion.status(app, if paused { Phase::Paused } else { Phase::Idle }, None);
    refresh_tray(app);
}

pub fn show_panel(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
    }
}

fn register_hotkey(app: &AppHandle, accel: &str) -> Result<(), String> {
    app.global_shortcut().register(accel).map_err(|e| e.to_string())
}

#[derive(Clone, Serialize, PartialEq)]
struct CursorEvent {
    display: usize,
    x: f64,
    y: f64,
}

/// Streams the mouse position (display-local view points) to the overlays so
/// the companion cursor can follow it. Polls at ~60 Hz but only emits when
/// the pointer actually moves; idle cost is negligible.
fn spawn_cursor_tracker(app: AppHandle) {
    std::thread::Builder::new()
        .name("luma-cursor".into())
        .spawn(move || {
            let mut displays = screen::displays().unwrap_or_default();
            let mut refreshed = std::time::Instant::now();
            let mut last: Option<CursorEvent> = None;
            let mut hidden = false;
            loop {
                std::thread::sleep(std::time::Duration::from_millis(16));
                let paused = app.state::<AppState>().prefs.lock().unwrap().paused;
                if paused {
                    if !hidden {
                        let _ = app.emit("luma://cursor", Option::<CursorEvent>::None);
                        hidden = true;
                        last = None;
                    }
                    continue;
                }
                hidden = false;
                if refreshed.elapsed().as_secs() >= 3 || displays.is_empty() {
                    if let Ok(d) = screen::displays() {
                        displays = d;
                    }
                    refreshed = std::time::Instant::now();
                }
                let Some(p) = screen::pointer(&app, &displays) else { continue };
                let Some(d) = luma_core::geometry::display_at(&displays, p) else { continue };
                let v = d.input_to_view(p);
                let ev = CursorEvent { display: d.index, x: v.x.round(), y: v.y.round() };
                if last.as_ref() != Some(&ev) {
                    let _ = app.emit("luma://cursor", Some(&ev));
                    last = Some(ev);
                }
            }
        })
        .expect("cursor thread");
}

/// One transparent, click-through overlay window per display. Called at
/// startup and at every turn, so plugging in or rearranging monitors is
/// picked up without a restart.
pub fn sync_overlays(app: &AppHandle, displays: &[Display]) {
    let displays = displays.to_vec();
    let app2 = app.clone();
    let _ = app.run_on_main_thread(move || {
        for d in &displays {
            let label = format!("overlay-{}", d.index);
            let win = match app2.get_webview_window(&label) {
                Some(w) => w,
                None => {
                    let built = WebviewWindowBuilder::new(
                        &app2,
                        &label,
                        WebviewUrl::App(format!("overlay.html?display={}", d.index).into()),
                    )
                    .title("LUMA overlay")
                    .transparent(true)
                    .decorations(false)
                    .always_on_top(true)
                    .skip_taskbar(true)
                    .resizable(false)
                    .focused(false)
                    .shadow(false)
                    .visible_on_all_workspaces(true)
                    // Keep our own drawings out of the screenshots we send.
                    .content_protected(true)
                    .build();
                    match built {
                        Ok(w) => {
                            let _ = w.set_ignore_cursor_events(true);
                            elevate_overlay(&w);
                            w
                        }
                        Err(e) => {
                            log::error!("overlay window: {e}");
                            continue;
                        }
                    }
                }
            };
            let f = d.input_frame;
            if cfg!(target_os = "macos") {
                let _ = win.set_position(tauri::LogicalPosition::new(f.x, f.y));
                let _ = win.set_size(tauri::LogicalSize::new(f.w, f.h));
            } else {
                let _ = win.set_position(tauri::PhysicalPosition::new(f.x as i32, f.y as i32));
                let _ = win.set_size(tauri::PhysicalSize::new(f.w as u32, f.h as u32));
            }
            let _ = win.show();
        }
        // displays that went away
        for (label, w) in app2.webview_windows() {
            if let Some(i) = label.strip_prefix("overlay-").and_then(|s| s.parse::<usize>().ok()) {
                if i >= displays.len() {
                    let _ = w.close();
                }
            }
        }
    });
}

/// Float above the menu bar and full-screen apps, on every Space.
#[cfg(target_os = "macos")]
fn elevate_overlay(w: &tauri::WebviewWindow) {
    use objc2::runtime::AnyObject;
    if let Ok(ns) = w.ns_window() {
        let ns = ns as *mut AnyObject;
        // NSPopUpMenuWindowLevel (101): above the menu bar, below alerts.
        // Behaviour: canJoinAllSpaces | stationary | ignoresCycle | fullScreenAuxiliary
        let behaviour: usize = (1 << 0) | (1 << 4) | (1 << 6) | (1 << 8);
        unsafe {
            let _: () = objc2::msg_send![ns, setLevel: 101isize];
            let _: () = objc2::msg_send![ns, setCollectionBehavior: behaviour];
        }
    }
}

#[cfg(not(target_os = "macos"))]
fn elevate_overlay(_w: &tauri::WebviewWindow) {}

fn refresh_tray(app: &AppHandle) {
    let Some(tray) = app.tray_by_id("luma") else { return };
    if let Ok(menu) = build_tray_menu(app) {
        let _ = tray.set_menu(Some(menu));
    }
}

fn build_tray_menu(app: &AppHandle) -> tauri::Result<Menu<tauri::Wry>> {
    let st = app.state::<AppState>();
    let prefs = st.prefs.lock().unwrap().clone();
    let hint = MenuItem::with_id(app, "hint", format!("Hold {} and talk", prefs.hotkey), false, None::<&str>)?;
    let open = MenuItem::with_id(app, "open", "Open LUMA…", true, None::<&str>)?;
    let pause = MenuItem::with_id(
        app,
        "pause",
        if prefs.paused { "Resume watching" } else { "Pause (stop seeing & listening)" },
        true,
        None::<&str>,
    )?;
    let clear = MenuItem::with_id(app, "clear", "Forget this session", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit LUMA", true, None::<&str>)?;
    Menu::with_items(
        app,
        &[&hint, &PredefinedMenuItem::separator(app)?, &open, &pause, &clear, &PredefinedMenuItem::separator(app)?, &quit],
    )
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("luma=info,luma_lib=info")).init();

    tauri::Builder::default()
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(|app, _shortcut, event| {
                    let st = app.state::<AppState>();
                    match event.state() {
                        ShortcutState::Pressed => st.companion.on_press(app),
                        ShortcutState::Released => st.companion.on_release(app),
                    }
                })
                .build(),
        )
        .setup(|app| {
            #[cfg(target_os = "macos")]
            app.set_activation_policy(tauri::ActivationPolicy::Accessory);

            let config_dir = app.path().app_config_dir()?;
            settings::load_env_files(Some(&config_dir));
            let prefs_path = config_dir.join("prefs.json");
            let prefs = Prefs::load(&prefs_path);
            let hotkey = prefs.hotkey.clone();
            app.manage(AppState {
                prefs: Mutex::new(prefs),
                prefs_path,
                session: Mutex::new(Session::new()),
                companion: Arc::new(Companion::new()),
            });
            let handle = app.handle().clone();
            if let Err(e) = register_hotkey(&handle, &hotkey) {
                log::error!("hotkey {hotkey} unavailable: {e}");
            }

            let menu = build_tray_menu(&handle)?;
            TrayIconBuilder::with_id("luma")
                .icon(app.default_window_icon().cloned().expect("app icon"))
                .icon_as_template(true)
                .tooltip("LUMA")
                .menu(&menu)
                .show_menu_on_left_click(true)
                .on_menu_event(|app, ev| {
                    let st = app.state::<AppState>();
                    match ev.id().as_ref() {
                        "open" => show_panel(app),
                        "pause" => {
                            let paused = !st.prefs.lock().unwrap().paused;
                            set_paused_inner(app, &st, paused);
                        }
                        "clear" => {
                            st.companion.interrupt(app);
                            st.session.lock().unwrap().clear();
                        }
                        "quit" => app.exit(0),
                        _ => {}
                    }
                })
                .on_tray_icon_event(|tray, ev| {
                    if let TrayIconEvent::Click { button: MouseButton::Right, button_state: MouseButtonState::Up, .. } = ev {
                        show_panel(tray.app_handle());
                    }
                })
                .build(app)?;

            match screen::displays() {
                Ok(d) => sync_overlays(&handle, &d),
                Err(e) => log::error!("{e}"),
            }
            spawn_cursor_tracker(handle.clone());
            if !screen::has_screen_permission() {
                log::warn!("Screen Recording permission missing; LUMA will ask on first use");
            }
            if Provider::ALL.iter().take(2).any(|p| get_key(*p).is_none()) {
                show_panel(&handle);
            }
            let _ = handle.emit("luma://status", companion::StatusEvent { phase: Phase::Idle, message: None, display: None });
            Ok(())
        })
        .on_window_event(|window, event| {
            // Closing the panel hides it; LUMA keeps running in the menu bar.
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                if window.label() == "main" {
                    api.prevent_close();
                    let _ = window.hide();
                }
            }
        })
        .invoke_handler(tauri::generate_handler![get_settings, save_key, save_prefs, ask, stop, clear_session, set_paused])
        .run(tauri::generate_context!())
        .expect("error while running LUMA");
}
