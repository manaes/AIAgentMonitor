# AI Agent Monitor

> Claude Code와 Codex의 토큰 사용량을 상태 바에서 실시간으로 모니터링하는 네이티브 앱

[![Release](https://img.shields.io/github/v/release/manaes/AIAgentMonitor)](https://github.com/manaes/AIAgentMonitor/releases)
[![Platform](https://img.shields.io/badge/platform-macOS%20%7C%20Windows-lightgrey)](#설치)
[![License](https://img.shields.io/badge/license-MIT-blue)](#)

---

## 스크린샷

| 상태 바 팝업 | 세부 대시보드 |
|---|---|
| ![Floating](app/docs/screenshots/floating.png) | ![Detail](app/docs/screenshots/detail.png) |

---

## 주요 기능

### 📊 실시간 토큰 모니터링
- **tok/s** — 현재 AI 응답 속도 (10초 EMA)
- **5h 사용량 바** — 실측 사용률(%) 자동 표시
- **주간(7d) 사용량 바** — 주간 사용률 별도 표시
- **리셋 카운트다운** — "약 X시간 Y분 Z초 남음" 실시간 표시
- **활성 세션 목록** — 프로젝트별 / 모델별 분리 (Active/Idle/Dormant)

### 🎯 실측 사용량 자동 동기화
- **Claude Code**: 앱 시작 시 및 10분마다 자동으로 `claude -p "ping"`을 내부 프록시(포트 4319) 경유로 실행해 Anthropic 서버가 응답 헤더로 보내는 5h/주간 사용률과 리셋 시각을 캡처합니다. 수동 동기화 버튼도 제공.
- **Codex**: 세션 rollout JSONL의 `rate_limits` 필드에서 5h/주간 사용률과 리셋 시각을 자동으로 읽습니다.
- **Antigravity**: `agy -p /usage` 명령을 통해 Google AI Pro 5h/주간 쿼터를 자동으로 폴링하고 동기화합니다.

### 📟 외장 스마트 디스플레이 (ESP32 CYD) 지원
- 저렴한 2.8인치 터치스크린 디스플레이 보드(**ESP32 CYD**)를 책상 위에 거치하여 실시간 대시보드로 활용할 수 있습니다.
- **Wi-Fi (LAN) & Bluetooth LE (BLE) 듀얼 모드**: 환경에 따라 로컬 네트워크 또는 블루투스 무선 연결을 선택하여 사용 가능
- **종단간 암호화(E2EE)**: X25519 및 ChaCha20-Poly1305 기반의 안전한 6자리 코드 페어링 및 암호화 스트리밍 지원
- [**👉 ESP32 CYD 설치 & 연결 가이드 보기**](firmware/docs/CYD_SETUP_GUIDE.md)

---

## 설치

### 다운로드 (권장)

[**최신 릴리즈 다운로드 →**](https://github.com/manaes/AIAgentMonitor/releases/latest)

| 플랫폼 | 파일 |
|---|---|
| macOS Apple Silicon | `AI.Agent.Monitor_*_aarch64.dmg` |
| macOS Intel | `AI.Agent.Monitor_*_x64.dmg` |
| Windows | `AI.Agent.Monitor_*_x64_en-US.msi` |

> macOS 빌드는 Apple Developer ID로 **서명·공증(notarized)** 되어 있어 별도 Gatekeeper 우회 없이 바로 실행됩니다.

---

## 사용 방법

### 기본 사용

1. 앱 실행 → 상단 상태 바에 아이콘 표시
2. **아이콘 클릭** → Detail 창 열기 (Sessions / Devices 탭)
3. **우클릭** → Open Log Folder / Quit

### 사용량 자동 동기화

앱이 시작되면 5초 후 자동으로 사용량을 동기화합니다. 이후 Claude를 사용하는 동안 10분마다 자동 갱신됩니다.

즉시 갱신하려면 Detail 창 에이전트 카드의 **🔄 동기화** 버튼을 클릭하세요.

---

## 데이터 소스

| 소스 | 접근 방식 | 데이터 |
|---|---|---|
| Claude Code | `~/.claude/projects/*/*.jsonl` FSEvents tail | 토큰 (in/out/cache), 세션, 모델 |
| Claude Code | 내부 프록시(포트 4319) 경유 자동 핑 | 실측 5h/주간 사용률, 리셋 시각 |
| Codex | `~/.codex/sessions/**/rollout-*.jsonl` (최신) 또는 `state_5.sqlite` 인덱스(구버전) | 토큰 delta, 실측 5h/주간 사용률 |

모든 접근은 **read-only** 또는 로컬 프록시이며 외부 서버로 데이터가 전송되지 않습니다.

---

## 동작 원리 (아키텍처)

**Tech Stack**: Tauri 2 · Rust (tokio, notify, rusqlite, axum) · Svelte 5 · TypeScript

### 1. 시스템 토폴로지

하나의 Tauri 2 프로세스 안에서 **Rust 백엔드(데이터 수집·집계)** 와 **Svelte 5 프론트엔드(렌더)** 가 IPC 로 연결된다. 창은 `popover`(상태 바 팝업)·`detail`(대시보드) 둘이며, 같은 `snapshot` 이벤트를 함께 구독한다.

```mermaid
graph TD
    subgraph Sources["데이터 소스 (read-only)"]
        CJ[("~/.claude/projects/*.jsonl")]
        CH[("Anthropic 응답 헤더")]
        CX[("~/.codex/sessions/**/rollout JSONL<br/>구버전: state_5.sqlite 인덱스")]
    end
    subgraph Proc["Tauri 2 프로세스"]
        subgraph BE["Rust 백엔드 (app/src-tauri/src)"]
            Watch["수집<br/>watchers/claude.rs · codex.rs<br/>quota_proxy.rs (:4319)"]
            Agg["집계 aggregator/<br/>ring(10s EMA) + rotating(60×5분=5h)"]
            Emit["emitter.rs<br/>EmitGate (500ms · 해시 변경시만)"]
            Tray["tray.rs · clock.rs"]
        end
        subgraph FE["Svelte 프론트 (src)"]
            App["App.svelte (라우터)<br/>Popover / Detail<br/>AgentCard · QuotaBar · SessionList"]
            Lib["lib/store.svelte.ts<br/>lib/tauri.ts (invoke/listen) · format.ts"]
        end
    end

    CJ -->|"FSEvents tail"| Watch
    CH -->|"프록시 :4319"| Watch
    CX -->|"2초 폴링"| Watch
    Watch -->|"TokenEvent (mpsc)"| Agg
    Agg -->|"250ms tick"| Emit
    Emit -->|"emit snapshot"| Lib
    Lib --> App
```

### 2. 데이터 흐름 (단방향)

```mermaid
flowchart LR
    subgraph 수집
        CW["ClaudeWatcher<br/>FSEvents tail"]
        XW["CodexWatcher<br/>2초 폴링"]
        QP["QuotaProxy :4319<br/>응답 헤더 관찰 → QuotaState"]
    end
    subgraph 집계["Aggregator"]
        Ring["EventRing<br/>10초 EMA → tok/s"]
        Rot["RotatingBucket<br/>60×5분 = 5h 합산"]
    end
    Gate["EmitGate<br/>500ms · 해시 변경시만"]
    Store["store.snap<br/>(Svelte 5 룬)"]
    UI["AgentCard · QuotaBar<br/>SessionList 재렌더"]

    CW -->|"TokenEvent"| Ring
    XW -->|"TokenEvent"| Ring
    CW --> Rot
    XW --> Rot
    QP -.->|"실측 quota 주입"| Gate
    Ring -->|"250ms snapshot"| Gate
    Rot --> Gate
    Gate -->|"emit snapshot"| Store
    Store --> UI
```

1. **수집** — `ClaudeWatcher` 는 FSEvents 로 JSONL 을 tail-follow한다. `CodexWatcher` 는 최신 Codex의 `~/.codex/sessions/**/rollout-*.jsonl`을 직접 찾고, 구버전에서는 `state_5.sqlite`의 인덱스를 폴백으로 사용해 `token_count`·`rate_limits`를 읽는다. `QuotaProxy` 는 `127.0.0.1:4319` 에서 Claude 요청을 포워딩하면서 사용률 헤더만 관찰한다.
2. **집계** — `Aggregator` 가 `TokenEvent` 를 `EventRing`(10초 EMA로 tok/s)과 `RotatingBucket`(60×5분 = 5h 합산)에 누적하고, 프로젝트별 활동 상태(Active<60s / Idle<300s / Dormant)를 갱신한다.
3. **송출** — 250ms 틱마다 `snapshot()` 으로 `Snapshot{ emitted_at, agents[] }` 를 만들고 실측 quota 를 주입한 뒤, `EmitGate` 가 **내용 해시가 바뀌었고 직전 송출에서 500ms 이상 지났을 때만** `app.emit("snapshot")` 한다(불필요한 재렌더 차단).
4. **렌더** — 프론트는 `store.init()` 에서 `listen("snapshot")` 으로 구독하고, 수신 시 `store.snap` 갱신 → Svelte 5 룬 반응성으로 AgentCard/QuotaBar/SessionList 가 재렌더된다. 별도 1초 타이머로 카운트다운·stale 표시를 갱신한다.

### 3. IPC 경계 (의존 방향)

프론트는 `lib/tauri.ts` 한 곳으로만 백엔드에 의존한다. 백엔드→프론트는 **이벤트(`snapshot`) 단방향 push**, 프론트→백엔드는 **command invoke** 뿐이다.

| 방향 | 종류 | 시그니처 |
|---|---|---|
| 백→프 | event | `snapshot` (250ms 생성·500ms 게이트) |
| 프→백 | command | `open_detail_window` · `sync_quota` |

### 4. 영속화 / 진단

| 대상 | 위치 | 시점 |
|---|---|---|
| Claude quota 캐시 | `~/.config/ai-agent-monitor/claude-quota.json` | quota 헤더 관찰 시 |
| 앱 오류 | Detail의 Devices/Settings 패널 | 전송·설정 작업 실패 시 |
| CYD 진단 | USB 시리얼 115200 baud | 부팅·터치·연결·인증 상태 |

> Aggregator의 ring/rotating은 인메모리지만 시작 시 최근 Claude/Codex 원본 로그를 재생한다. Claude quota는 별도 캐시에서 복원하고 Codex quota는 최신 rollout의 `rate_limits`에서 복원한다. 현재 `tracing` subscriber는 없어 파일 로그를 만들지 않는다.

---

## 개발

```bash
# 의존성 설치
pnpm install

# 개발 서버 (hot reload)
pnpm tauri dev

# 릴리즈 빌드
pnpm tauri build
```

릴리즈 배포·코드 서명 상세: [app/docs/RELEASE.md](app/docs/RELEASE.md)

---

## 알려진 한계

- 앱이 실행 중일 때만 동기화가 동작합니다.
- Codex: rollout JSONL이 기록되는 활성 세션에서만 사용률이 갱신됩니다.

---

## 부록 (Appendix)

### Codex SQLite 스키마

`CodexWatcher`는 최신 Codex에서는 날짜별 rollout 디렉터리를 직접 탐색하고, `threads` 테이블이 존재하는 구버전에서만 `state_5.sqlite`를 읽는다. 두 형식과 호환성 메모는 [`app/docs/codex-schema.md`](app/docs/codex-schema.md)에 기록한다.

현재 아키텍처, 전송 경로와 보안 경계는 [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md), [`docs/SECURITY.md`](docs/SECURITY.md)를 참고하세요. CYD 설치는 [`firmware/docs/CYD_SETUP_GUIDE.md`](firmware/docs/CYD_SETUP_GUIDE.md)에 정리되어 있습니다.
