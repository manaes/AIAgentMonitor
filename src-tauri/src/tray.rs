use tauri::{
    menu::{Menu, MenuItem, PredefinedMenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconEvent},
    AppHandle, Manager, PhysicalPosition, Runtime,
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

    // 좌클릭: popover를 트레이 아이콘 바로 아래(가로 중앙)에 띄운다. 다시 누르면 숨김.
    tray.on_tray_icon_event(|tray, event| {
        if let TrayIconEvent::Click {
            button: MouseButton::Left,
            button_state: MouseButtonState::Up,
            rect,
            ..
        } = event
        {
            let app = tray.app_handle();
            if let Some(w) = app.get_webview_window("popover") {
                if w.is_visible().unwrap_or(false) {
                    let _ = w.hide();
                } else if let Ok(win_size) = w.outer_size() {
                    // rect.position/size는 tauri::Position/Size enum → 물리 좌표로 변환.
                    // 아이콘 가로 중앙에 창 중앙을 맞추고, 메뉴바 바로 아래에 놓는다.
                    let scale = w.scale_factor().unwrap_or(1.0);
                    let icon_pos = rect.position.to_physical::<f64>(scale);
                    let icon_size = rect.size.to_physical::<f64>(scale);
                    // popover 오른쪽 끝을 아이콘 오른쪽 끝에 맞춘다(우측 메뉴바 아이콘에서 자연스러운 방향).
                    let icon_right = icon_pos.x + icon_size.width;
                    let mut x = icon_right - win_size.width as f64;
                    let y = icon_pos.y + icon_size.height;
                    // 모니터 경계를 벗어나지 않도록 보정 (아이콘이 화면 오른쪽 끝일 때 대비)
                    if let Ok(Some(mon)) = w.current_monitor() {
                        let left = mon.position().x as f64 + 4.0;
                        let right =
                            mon.position().x as f64 + mon.size().width as f64 - win_size.width as f64 - 4.0;
                        if right >= left {
                            x = x.clamp(left, right);
                        }
                    }
                    let _ = w.set_position(PhysicalPosition::new(x, y));
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
