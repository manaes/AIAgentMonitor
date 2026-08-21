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
