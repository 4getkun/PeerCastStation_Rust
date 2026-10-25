use crate::preferences::WindowSettings;
use std::sync::atomic::{AtomicBool, Ordering};
use tauri::{
    menu::{Menu, MenuItem},
    tray::{MouseButton, TrayIconBuilder, TrayIconEvent},
    AppHandle, LogicalSize, Manager, PhysicalPosition, Runtime, WebviewWindow, Window, WindowEvent,
};

const MAIN_WINDOW: &str = "main";
const SHOW_ITEM: &str = "native-window-show";
const EXIT_ITEM: &str = "native-window-exit";
const MIN_WIDTH: u32 = 1280;
const MIN_HEIGHT: u32 = 720;

static MINIMIZE_TO_TRAY: AtomicBool = AtomicBool::new(false);
static HIDDEN_BY_TRAY: AtomicBool = AtomicBool::new(false);

/// Creates one persistent tray icon and routes its exit command through the main window close event.
pub fn setup(app: &AppHandle) -> Result<(), String> {
    let show = MenuItem::with_id(app, SHOW_ITEM, "表示", true, None::<&str>)
        .map_err(|error| error.to_string())?;
    let exit = MenuItem::with_id(app, EXIT_ITEM, "終了", true, None::<&str>)
        .map_err(|error| error.to_string())?;
    let menu = Menu::with_items(app, &[&show, &exit]).map_err(|error| error.to_string())?;
    let icon = app
        .default_window_icon()
        .cloned()
        .ok_or_else(|| "アプリのトレイアイコンが設定されていません".to_owned())?;
    let tray = TrayIconBuilder::with_id("peercast-rt-tray")
        .icon(icon)
        .tooltip("Peercast RT")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id().as_ref() {
            SHOW_ITEM => restore_main(app),
            EXIT_ITEM => {
                if let Some(window) = app.get_webview_window(MAIN_WINDOW) {
                    let _ = window.close();
                }
            }
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if matches!(
                event,
                TrayIconEvent::DoubleClick {
                    button: MouseButton::Left,
                    ..
                }
            ) {
                restore_main(tray.app_handle());
            }
        })
        .build(app)
        .map_err(|error| error.to_string())?;

    if !app.manage(tray) {
        return Err("システムトレイの状態を登録できません".into());
    }
    Ok(())
}

/// Applies the saved minimize behavior and optionally updates the initial/main window size.
pub fn apply(app: &AppHandle, settings: &WindowSettings, size_changed: bool) -> Result<(), String> {
    let was_minimize_to_tray = MINIMIZE_TO_TRAY.swap(settings.minimize_to_tray, Ordering::AcqRel);
    if was_minimize_to_tray
        && !settings.minimize_to_tray
        && HIDDEN_BY_TRAY.swap(false, Ordering::AcqRel)
    {
        restore_main(app);
    }

    if size_changed {
        let window = app
            .get_webview_window(MAIN_WINDOW)
            .ok_or_else(|| "メインウィンドウが見つかりません".to_owned())?;
        apply_size(&window, settings.width, settings.height)?;
    }
    Ok(())
}

/// Hides a minimized main window only when the persisted setting is enabled.
pub fn handle_window_event(window: &Window, event: &WindowEvent) {
    if window.label() != MAIN_WINDOW || !matches!(event, WindowEvent::Resized(_)) {
        return;
    }
    if MINIMIZE_TO_TRAY.load(Ordering::Acquire)
        && window.is_minimized().unwrap_or(false)
        && window.hide().is_ok()
    {
        HIDDEN_BY_TRAY.store(true, Ordering::Release);
    }
}

pub fn restore_main<R: Runtime>(app: &AppHandle<R>) {
    if let Some(window) = app.get_webview_window(MAIN_WINDOW) {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
        HIDDEN_BY_TRAY.store(false, Ordering::Release);
    }
}

fn apply_size(
    window: &WebviewWindow,
    requested_width: u32,
    requested_height: u32,
) -> Result<(), String> {
    if window.is_maximized().unwrap_or(false) {
        window.unmaximize().map_err(|error| error.to_string())?;
    }
    let outer_size = window.outer_size().map_err(|error| error.to_string())?;
    let inner_size = window.inner_size().map_err(|error| error.to_string())?;
    let frame_width = outer_size.width.saturating_sub(inner_size.width);
    let frame_height = outer_size.height.saturating_sub(inner_size.height);
    let monitor = window.current_monitor().ok().flatten().or_else(|| {
        window
            .available_monitors()
            .ok()
            .and_then(|monitors| monitors.into_iter().next())
    });

    let (width, height, min_width, min_height, position) = if let Some(monitor) = monitor {
        let work = monitor.work_area();
        let scale = monitor.scale_factor().max(0.25);
        let max_width =
            ((work.size.width.saturating_sub(frame_width) as f64 / scale).floor() as u32).max(1);
        let max_height =
            ((work.size.height.saturating_sub(frame_height) as f64 / scale).floor() as u32).max(1);
        let min_width = MIN_WIDTH.min(max_width);
        let min_height = MIN_HEIGHT.min(max_height);
        let width = requested_width.clamp(min_width, max_width);
        let height = requested_height.clamp(min_height, max_height);
        let old_position = window.outer_position().unwrap_or(*monitor.position());
        let physical_width = (width as f64 * scale).round() as u32 + frame_width;
        let physical_height = (height as f64 * scale).round() as u32 + frame_height;
        let max_x = work
            .position
            .x
            .saturating_add(work.size.width.saturating_sub(physical_width) as i32);
        let max_y = work
            .position
            .y
            .saturating_add(work.size.height.saturating_sub(physical_height) as i32);
        let position = PhysicalPosition::new(
            old_position.x.clamp(work.position.x, max_x),
            old_position.y.clamp(work.position.y, max_y),
        );
        (width, height, min_width, min_height, Some(position))
    } else {
        (
            requested_width.max(MIN_WIDTH),
            requested_height.max(MIN_HEIGHT),
            MIN_WIDTH,
            MIN_HEIGHT,
            None,
        )
    };

    window
        .set_min_size(Some(LogicalSize::new(min_width as f64, min_height as f64)))
        .map_err(|error| error.to_string())?;
    window
        .set_size(LogicalSize::new(width as f64, height as f64))
        .map_err(|error| error.to_string())?;
    if let Some(position) = position {
        window
            .set_position(position)
            .map_err(|error| error.to_string())?;
    }
    Ok(())
}
