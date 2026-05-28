use tauri::{
    menu::{Menu, MenuItem, PredefinedMenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    AppHandle, Manager, Runtime,
};

pub fn install<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<()> {
    let detail = MenuItem::with_id(app, "detail", "Open Detail Window…", true, None::<&str>)?;
    let logs = MenuItem::with_id(app, "logs", "Open Log Folder", true, None::<&str>)?;
    let sep = PredefinedMenuItem::separator(app)?;
    let quit = MenuItem::with_id(app, "quit", "Quit AI Monitor", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&detail, &logs, &sep, &quit])?;

    let _tray = TrayIconBuilder::with_id("main")
        .icon(app.default_window_icon().cloned().expect("icon"))
        .icon_as_template(true)
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_tray_icon_event(|tray, event| {
            tracing::info!(?event, "tray event received");
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                position,
                rect,
                ..
            } = event
            {
                let app = tray.app_handle();
                if let Some(w) = app.get_webview_window("popover") {
                    let visible = w.is_visible().unwrap_or(false);
                    tracing::info!(visible, "popover toggle");
                    if visible {
                        let _ = w.hide();
                    } else {
                        // 팝오버를 트레이 아이콘 바로 아래에 위치시킨다.
                        // rect.position 은 dpi::Position(논리 좌표)이므로
                        // PhysicalPosition 인 position(커서 위치)과 rect.size를 조합해 계산한다.
                        let win_w = 360.0_f64;
                        let icon_right_edge = position.x; // 클릭 지점은 아이콘 안에 있음
                        // rect.size 를 물리 픽셀로 추정(스케일 1 가정, 보수적으로 22px)
                        let icon_h = 22.0_f64;
                        let target_x = (icon_right_edge - win_w / 2.0).max(8.0);
                        let target_y = position.y - icon_h / 2.0 + icon_h + 4.0;
                        let _ = w.set_position(tauri::PhysicalPosition::new(target_x, target_y));
                        let _ = w.show();
                        let _ = w.set_focus();
                        // rect 바인딩 사용 — unused 경고 억제
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
                if let Some(dir) = dirs_next::home_dir() {
                    let log_dir = dir.join("Library/Logs/AIMonitor");
                    let _ = std::process::Command::new("open").arg(log_dir).spawn();
                }
            }
            "quit" => app.exit(0),
            _ => {}
        })
        .build(app)?;
    Ok(())
}
