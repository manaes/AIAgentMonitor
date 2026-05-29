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
            rect,   // 아이콘의 실제 바운딩 박스 (물리 픽셀)
            ..
        } = event
        {
            let app = tray.app_handle();
            if let Some(w) = app.get_webview_window("popover") {
                let visible = w.is_visible().unwrap_or(false);
                if visible {
                    let _ = w.hide();
                } else {
                    // 디스플레이 스케일 팩터 — Retina(2.0), 일반(1.0)
                    let scale = w
                        .primary_monitor()
                        .ok()
                        .flatten()
                        .map(|m| m.scale_factor())
                        .unwrap_or(2.0);

                    // rect.position / rect.size 는 Position/Size enum — 물리 픽셀(f64)로 통일
                    let (ix, iy) = match rect.position {
                        tauri::Position::Physical(p) => (p.x as f64, p.y as f64),
                        tauri::Position::Logical(p) => (p.x * scale, p.y * scale),
                    };
                    let (iw, ih) = match rect.size {
                        tauri::Size::Physical(s) => (s.width as f64, s.height as f64),
                        tauri::Size::Logical(s) => (s.width * scale, s.height * scale),
                    };

                    // 아이콘 중심 X, 하단 Y
                    let icon_cx = ix + iw / 2.0;
                    let icon_bottom = iy + ih;

                    // 창 논리 너비(360px)를 물리 픽셀로 변환해서 중심 정렬
                    let win_w_phys = 360.0 * scale;
                    let target_x = (icon_cx - win_w_phys / 2.0).max(0.0);
                    let target_y = icon_bottom + 4.0;

                    let _ = w.set_position(tauri::PhysicalPosition::new(target_x, target_y));
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
