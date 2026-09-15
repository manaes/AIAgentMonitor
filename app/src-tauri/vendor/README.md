# vendor/

## wmi-0.18.4

`iroh` → `netwatch`(Windows 네트워크 인터페이스 변경 감지)가 의존하는 `wmi` 크레이트를
로컬에 벤더링해 `windows`/`windows-core` 버전을 `=0.61.3`/`=0.61.2`로 고정했다.

**왜 필요한가**: crates.io의 원본 `wmi 0.18.4`는 `windows`/`windows-core` 요구 범위를
`>=0.59, <0.63`로 넓게 열어두는데, 이 프로젝트의 Windows 빌드 그래프에는 서로 호환되지
않는 두 메이저 라인이 이미 공존한다 —

- `tauri`/`wry`/`webview2-com` 계열은 `windows`/`windows-core` `^0.61`을 요구
- `netwatch`(iroh 의존성)는 자체적으로 `windows = "0.62.2"`를 직접 요구

Cargo 리졸버가 `wmi`의 두 개별 의존성 엣지(`windows`, `windows-core`)를 서로 다른
인스턴스(`windows 0.61.3` + `windows-core 0.62.2`)에 독립적으로 배정해버려서, `wmi`
자신의 컴파일 단위 안에서 `IWbemObjectSink`(0.61 쪽에서 옴)가 `Interface` 트레이트
(0.62 쪽에서 옴)를 구현하지 못한다는 에러로 Windows 빌드가 실패한다
(`windows_core::Interface is not implemented for IWbemObjectSink`).

두 메이저 라인 모두 다른 크레이트가 엄격하게 고정해뒀기 때문에(양방향으로
`cargo update --precise` 시도 전부 실패 확인됨), `wmi` 쪽 요구 범위를 이미 그래프에
존재하는 `0.61.x` 짝으로 좁혀 고정하는 것 외에는 방법이 없었다. `wmi`의 원래 범위가
애초에 0.61.x를 포함하고 있었으므로(`>=0.59,<0.63`), 이건 새 조합을 억지로 강요하는
게 아니라 이미 지원 범위 안의 자기일관적인 짝을 강제하는 것뿐이다.

`src-tauri/Cargo.toml`의 `[patch.crates-io]`에서 이 디렉토리를 가리킨다. `wmi`가
crates.io에서 자체적으로 windows 0.62 라인을 지원하도록 릴리즈되면 이 패치와 이
디렉토리를 통째로 지워도 된다.

## tray-icon-0.23.1

메뉴바(트레이) 아이콘을 **좌클릭해도 팝오버가 안 뜨고 메뉴만 뜨는** macOS 27
(Golden Gate) 회귀를 막기 위해 로컬에 벤더링했다.

**왜 필요한가**: `NSStatusItem` 에 `setMenu` 로 메뉴가 붙어 있으면, macOS 27 부터
좌클릭이 tray-icon 이 깔아 둔 서브뷰(`TaoTrayTarget`)까지 내려오지 않는다. 시스템이
먼저 메뉴를 띄워 버려서 `TrayIconEvent::Click` 자체가 발생하지 않고, 따라서
`show_menu_on_left_click(false)` 도 무력해진다 — 그 옵션은 "이벤트를 받은 다음
메뉴를 띄울지"를 고르는 것이라 이벤트가 안 오면 할 수 있는 게 없다. 그 결과
`src/tray.rs` 의 좌클릭 핸들러(팝오버 토글)가 영영 실행되지 않는다.

업스트림 이슈: [tauri-apps/tray-icon#355]. 수정 PR [#341]·[#365] 은 2026-09-15
기준 **둘 다 미머지**이고, 최신 릴리즈 0.25.0(2026-09-11)에도 들어가 있지 않다.

**무엇을 고쳤나**(`src/platform_impl/macos/mod.rs`, 전부 `PATCH` 주석 표시):

1. `create()` / `set_menu()` 에서 `setMenu` 를 **하지 않는다**. 메뉴는 ivars 에만
   보관한다(`setDelegate` 는 하이라이트 복구에 필요하므로 유지).
2. `on_tray_click()` / `show_menu()` 가 클릭을 받은 **뒤에** `setMenu` → `performClick`
   → `setMenu(None)` 순으로 잠깐 붙였다 뗀다(`pop_up_menu` 헬퍼). PR #341 과 같은 접근.
3. `performClick` 은 메뉴가 닫힐 때까지 반환하지 않는 **중첩 런루프**라, 그 너머까지
   `RefCell` 을 빌려두면 메뉴가 떠 있는 동안 `set_menu` 가 불릴 때 `BorrowMutError`
   로 죽는다(PR #365 가 지적한 문제). `Retained` 를 먼저 복제해 빌림을 즉시 끝낸다.

좌클릭 → 팝오버, 우클릭 → 네이티브 메뉴가 둘 다 살아 있는 게 정상 동작이다.

업스트림에 #341/#365 가 머지된 버전이 나오면 `[patch.crates-io]` 의 `tray-icon`
줄과 이 디렉토리를 통째로 지우고 그 버전으로 올리면 된다.

[tauri-apps/tray-icon#355]: https://github.com/tauri-apps/tray-icon/issues/355
[#341]: https://github.com/tauri-apps/tray-icon/pull/341
[#365]: https://github.com/tauri-apps/tray-icon/pull/365
