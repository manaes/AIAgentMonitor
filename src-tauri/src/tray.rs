use tauri::{
    menu::{Menu, MenuItem, PredefinedMenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    AppHandle, Manager, PhysicalPosition, Runtime,
};

pub fn install<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<()> {
    let detail = MenuItem::with_id(app, "detail", "Open Detail Window…", true, None::<&str>)?;
    let logs   = MenuItem::with_id(app, "logs",   "Open Log Folder",     true, None::<&str>)?;
    let sep    = PredefinedMenuItem::separator(app)?;
    let quit   = MenuItem::with_id(app, "quit",   "Quit AI Monitor",     true, None::<&str>)?;
    let menu   = Menu::with_items(app, &[&detail, &logs, &sep, &quit])?;

    // 플랫폼별 아이콘 선택:
    //   macOS → PNG + iconAsTemplate(true)  : 다크/라이트 메뉴바 자동 대응
    //   Windows/Linux → ICO/PNG + template(false) : 컬러 아이콘 그대로 표시
    #[cfg(target_os = "macos")]
    let icon = tauri::include_image!("icons/32x32.png");
    #[cfg(not(target_os = "macos"))]
    let icon = tauri::include_image!("icons/icon.ico");

    let tray = TrayIconBuilder::new()
        .icon(icon)
        .icon_as_template(cfg!(target_os = "macos"))
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                rect,
                ..
            } = event
            {
                let app = tray.app_handle();
                if let Some(w) = app.get_webview_window("detail") {
                    if w.is_visible().unwrap_or(false) {
                        let _ = w.hide();
                    } else {
                        // macOS: 아이콘 아래에 위치시킴
                        #[cfg(target_os = "macos")]
                        if let Ok(win_size) = w.outer_size() {
                            let scale = w.scale_factor().unwrap_or(1.0);
                            let icon_pos  = rect.position.to_physical::<f64>(scale);
                            let icon_size = rect.size.to_physical::<f64>(scale);
                            let mut x = icon_pos.x;
                            let y = icon_pos.y + icon_size.height;
                            if let Ok(Some(mon)) = w.current_monitor() {
                                let left  = mon.position().x as f64 + 4.0;
                                let right = mon.position().x as f64 + mon.size().width as f64
                                    - win_size.width as f64 - 4.0;
                                if right >= left { x = x.clamp(left, right); }
                            }
                            let _ = w.set_position(PhysicalPosition::new(x, y));
                        }
                        let _ = w.show();
                        let _ = w.set_focus();
                        // rect을 사용하지 않는 플랫폼에서 경고 억제
                        let _ = rect;
                    }
                }
            }
        })
        .on_menu_event(|app, event| match event.id().as_ref() {
            "detail" => {
                if let Some(w) = app.get_webview_window("detail") {
                    let _ = w.show();
                    let _ = w.set_focus();
                }
            }
            "logs" => {
                #[cfg(target_os = "macos")]
                if let Some(dir) = dirs_next::home_dir() {
                    let log_dir = dir.join("Library/Logs/AIMonitor");
                    let _ = std::process::Command::new("open").arg(log_dir).spawn();
                }
                #[cfg(target_os = "windows")]
                if let Some(dir) = dirs_next::home_dir() {
                    let log_dir = dir.join("AppData\\Local\\AIMonitor\\logs");
                    let _ = std::process::Command::new("explorer").arg(log_dir).spawn();
                }
            }
            "quit" => app.exit(0),
            _ => {}
        })
        .build(app)?;

    // TrayIcon을 앱 종료까지 유지 (drop되면 아이콘 사라짐)
    Box::leak(Box::new(tray));
    Ok(())
}
