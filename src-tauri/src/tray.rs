use tauri::{
    menu::{Menu, MenuItem, PredefinedMenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconEvent},
    AppHandle, Manager, Runtime,
};

pub fn install<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<()> {
    // conf에서 만든 tray icon "main"을 가져와서 여기서 이벤트 핸들러와 메뉴를 붙인다.
    // 이 방식은 double-init 충돌 없이 conf 단에서 보장된 tray를 재사용한다.
    let tray = app
        .tray_by_id("main")
        .ok_or_else(|| tauri::Error::AssetNotFound("tray id 'main' not found".into()))?;

    let detail = MenuItem::with_id(app, "detail", "Open Detail Window…", true, None::<&str>)?;
    let logs = MenuItem::with_id(app, "logs", "Open Log Folder", true, None::<&str>)?;
    let sep = PredefinedMenuItem::separator(app)?;
    let quit = MenuItem::with_id(app, "quit", "Quit AI Monitor", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&detail, &logs, &sep, &quit])?;

    tray.set_menu(Some(menu))?;
    tray.set_show_menu_on_left_click(false)?;

    tray.on_tray_icon_event(|tray, event| {
        if let TrayIconEvent::Click {
            button: MouseButton::Left,
            button_state: MouseButtonState::Up,
            ..
        } = event
        {
            // 팝오버 대신 Detail 창을 직접 토글
            let app = tray.app_handle();
            if let Some(w) = app.get_webview_window("detail") {
                if w.is_visible().unwrap_or(false) {
                    let _ = w.hide();
                } else {
                    let _ = w.show();
                    let _ = w.set_focus();
                }
            }
        }
    });

    tray.on_menu_event(|app, event| match event.id().as_ref() {
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
    });

    Ok(())
}
