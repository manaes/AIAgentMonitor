use tauri::{
    menu::{Menu, MenuItem, PredefinedMenuItem},
    tray::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent},
    AppHandle, Manager, Runtime,
};

pub fn install<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<()> {
    let detail = MenuItem::with_id(app, "detail", "Open Detail Window…", true, None::<&str>)?;
    let logs = MenuItem::with_id(app, "logs", "Open Log Folder", true, None::<&str>)?;
    let sep = PredefinedMenuItem::separator(app)?;
    let quit = MenuItem::with_id(app, "quit", "Quit AI Monitor", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&detail, &logs, &sep, &quit])?;

    // include_image! 은 컴파일 타임에 PNG를 embed해서 dev/release 모두 경로 문제 없음
    let tray: TrayIcon<R> = TrayIconBuilder::new()
        .icon(tauri::include_image!("icons/32x32.png"))
        .icon_as_template(true)
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_tray_icon_event(|tray, event| {
            tracing::info!(?event, "tray event received");
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                position,
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
                        let win_w = 360.0_f64;
                        let target_x = (position.x - win_w / 2.0).max(8.0);
                        let target_y = position.y + 4.0;
                        let _ = w.set_position(tauri::PhysicalPosition::new(target_x, target_y));
                        let _ = w.show();
                        let _ = w.set_focus();
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

    // TrayIcon을 app state에 보관해서 install() 반환 후에도 drop 방지
    app.manage(tray);
    Ok(())
}
