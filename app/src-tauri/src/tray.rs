use std::sync::{
    atomic::{AtomicU32, Ordering},
    Arc,
};
use tauri::{
    menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    AppHandle, Manager, PhysicalPosition, Runtime,
};
use tauri_plugin_autostart::ManagerExt;

/// 모든 에이전트의 tok/s 합계를 centi-tokens/s(=tok/s * 100) 정수로 담아 둔다.
/// f32 원자값이 없어서 정수로 스케일링해 저장한다.
#[derive(Clone)]
pub struct TrayActivityRate(pub Arc<AtomicU32>);

impl TrayActivityRate {
    pub fn new() -> Self {
        Self(Arc::new(AtomicU32::new(0)))
    }

    pub fn set_tok_per_sec(&self, rate: f32) {
        let centi = (rate.max(0.0) * 100.0).round() as u32;
        self.0.store(centi, Ordering::Relaxed);
    }

    pub fn tok_per_sec(&self) -> f32 {
        self.0.load(Ordering::Relaxed) as f32 / 100.0
    }
}

/// 활동량(tok/s)이 클수록 프레임 전환을 빠르게 — 유휴 상태에서도 완전히
/// 멈추지 않고 천천히 순환시켜 "정적으로 보인다"는 문제를 해결한다.
fn frame_interval_ms(rate_tok_per_sec: f32) -> u64 {
    const IDLE_MS: f32 = 900.0;
    const FAST_MS: f32 = 90.0;
    const SATURATE_AT: f32 = 150.0; // 이 tok/s 이상이면 가장 빠른 속도로 고정
    let t = (rate_tok_per_sec / SATURATE_AT).clamp(0.0, 1.0);
    (IDLE_MS - (IDLE_MS - FAST_MS) * t) as u64
}

pub fn install<R: Runtime>(app: &AppHandle<R>, activity: TrayActivityRate) -> tauri::Result<()> {
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

    // macOS 메뉴바 아이콘을 RunCat 스타일로 애니메이션: tok/s 활동량에 따라
    // 이퀄라이저 바 프레임을 순환시킨다. Windows/Linux는 트레이 좌클릭이 바로
    // Detail 창을 띄우는 용도라 이번 스코프에서는 macOS 전용으로 둔다.
    #[cfg(target_os = "macos")]
    {
        let frames = [
            tauri::include_image!("icons/tray_anim/frame_0.png"),
            tauri::include_image!("icons/tray_anim/frame_1.png"),
            tauri::include_image!("icons/tray_anim/frame_2.png"),
            tauri::include_image!("icons/tray_anim/frame_3.png"),
            tauri::include_image!("icons/tray_anim/frame_4.png"),
            tauri::include_image!("icons/tray_anim/frame_5.png"),
            tauri::include_image!("icons/tray_anim/frame_6.png"),
            tauri::include_image!("icons/tray_anim/frame_7.png"),
        ];
        let tray_for_anim = tray.clone();
        tauri::async_runtime::spawn(async move {
            let mut idx = 0usize;
            loop {
                let interval_ms = frame_interval_ms(activity.tok_per_sec());
                idx = (idx + 1) % frames.len();
                // set_icon 뒤에 set_icon_as_template을 따로 호출하면 macOS에서
                // 아이콘이 두 번 그려지며 깜빡인다 — 원자적으로 같이 설정한다.
                let _ = tray_for_anim.set_icon_with_as_template(Some(frames[idx].clone()), true);
                tokio::time::sleep(std::time::Duration::from_millis(interval_ms)).await;
            }
        });
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = activity;
    }

    // TrayIcon을 앱 종료까지 유지 (drop되면 아이콘 사라짐)
    Box::leak(Box::new(tray));
    Ok(())
}
