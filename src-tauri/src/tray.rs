use tauri::{
    menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    AppHandle, Manager, PhysicalPosition, Runtime,
};
use tauri_plugin_autostart::ManagerExt;

pub fn install<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<()> {
    let detail = MenuItem::with_id(app, "detail", "Open Detail Window…", true, None::<&str>)?;
    let sep    = PredefinedMenuItem::separator(app)?;
    // 로그인 시 자동 실행 토글 — 현재 등록 상태를 체크 표시에 반영
    let autostart_on = app.autolaunch().is_enabled().unwrap_or(false);
    let autostart = CheckMenuItem::with_id(
        app,
        "autostart",
        "로그인 시 자동 실행",
        true,
        autostart_on,
        None::<&str>,
    )?;
    let quit   = MenuItem::with_id(app, "quit",   "Quit AI Monitor",     true, None::<&str>)?;
    let menu   = Menu::with_items(app, &[&detail, &autostart, &sep, &quit])?;

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
                // 좌클릭: macOS는 popover(플로팅)를 아이콘 아래에 띄우고,
                //         Windows/Linux는 Detail 창을 바로 보여준다. 다시 누르면 숨김.
                #[cfg(target_os = "macos")]
                let label = "popover";
                #[cfg(not(target_os = "macos"))]
                let label = "detail";
                if let Some(w) = app.get_webview_window(label) {
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
        .on_menu_event({
            let autostart = autostart.clone();
            move |app, event| match event.id().as_ref() {
            "autostart" => {
                let mgr = app.autolaunch();
                // 현재 등록 상태를 뒤집고, 결과를 체크 표시에 반영
                let now_on = mgr.is_enabled().unwrap_or(false);
                let result = if now_on { mgr.disable() } else { mgr.enable() };
                match result {
                    Ok(()) => {
                        let _ = autostart.set_checked(!now_on);
                        tracing::info!("로그인 시 자동 실행 {}", if now_on { "해제" } else { "설정" });
                    }
                    Err(e) => {
                        // 실패 시 실제 상태로 체크 표시를 되돌림
                        let _ = autostart.set_checked(mgr.is_enabled().unwrap_or(false));
                        tracing::warn!("자동 실행 토글 실패: {e}");
                    }
                }
            }
            "detail" => {
                if let Some(w) = app.get_webview_window("detail") {
                    let _ = w.show();
                    let _ = w.set_focus();
                }
            }
            "quit" => app.exit(0),
            _ => {}
            }
        })
        .build(app)?;

    // TrayIcon을 앱 종료까지 유지 (drop되면 아이콘 사라짐)
    Box::leak(Box::new(tray));
    Ok(())
}
