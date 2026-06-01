# AI Agent Monitor

> Claude Code와 Codex의 토큰 사용량을 상태 바에서 실시간으로 모니터링하는 네이티브 앱

[![Release](https://img.shields.io/github/v/release/manaes/AIAgentMonitor)](https://github.com/manaes/AIAgentMonitor/releases)
[![Platform](https://img.shields.io/badge/platform-macOS%20%7C%20Windows-lightgrey)](#설치)
[![License](https://img.shields.io/badge/license-MIT-blue)](#)

---

## 스크린샷

| 상태 바 팝업 | 세부 대시보드 |
|---|---|
| ![Floating](docs/screenshots/floating.png) | ![Detail](docs/screenshots/detail.png) |

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

### ⏰ Anchor Trigger
정해진 시각에 자동으로 가벼운 프롬프트를 발사해 5h quota window의 reset 타이밍을 생활 패턴(점심/퇴근)에 고정합니다.

예) 08:00에 ping → 13:00/18:00에 reset

---

## 설치

### 다운로드 (권장)

[**최신 릴리즈 다운로드 →**](https://github.com/manaes/AIAgentMonitor/releases/latest)

| 플랫폼 | 파일 |
|---|---|
| macOS Apple Silicon | `AI.Agent.Monitor_*_aarch64.dmg` |
| macOS Intel | `AI.Agent.Monitor_*_x64.dmg` |
| Windows | `AI.Agent.Monitor_*_x64_en-US.msi` |

**macOS 첫 실행 시** Gatekeeper 경고가 뜨면:
```bash
xattr -cr "/Applications/AI Agent Monitor.app"
```
또는 시스템 설정 → 개인 정보 보호 및 보안 → 앱 허용

---

## 사용 방법

### 기본 사용

1. 앱 실행 → 상단 상태 바에 아이콘 표시
2. **아이콘 클릭** → Detail 창 열기 (Sessions / Triggers 탭)
3. **우클릭** → Open Log Folder / Quit

### 사용량 자동 동기화

앱이 시작되면 5초 후 자동으로 사용량을 동기화합니다. 이후 Claude를 사용하는 동안 10분마다 자동 갱신됩니다.

즉시 갱신하려면 Detail 창 에이전트 카드의 **🔄 동기화** 버튼을 클릭하세요.

### Anchor Trigger 사용

1. Detail 창 → **Triggers** 탭
2. **새 트리거 추가**: 에이전트 / 실행 시각(HH:MM) / 작업 디렉토리 / 프롬프트 입력
3. 📁 버튼으로 폴더 선택 가능
4. **▶ 지금 실행** 버튼으로 즉시 테스트

---

## 데이터 소스

| 소스 | 접근 방식 | 데이터 |
|---|---|---|
| Claude Code | `~/.claude/projects/*/*.jsonl` FSEvents tail | 토큰 (in/out/cache), 세션, 모델 |
| Claude Code | 내부 프록시(포트 4319) 경유 자동 핑 | 실측 5h/주간 사용률, 리셋 시각 |
| Codex | `~/.codex/state_5.sqlite` → rollout JSONL | 토큰 delta, 실측 5h/주간 사용률 |

모든 접근은 **read-only** 또는 로컬 프록시이며 외부 서버로 데이터가 전송되지 않습니다.

---

## 아키텍처

```
DATA SOURCES (read-only)
Claude jsonl ──FSEvents──┐
Claude proxy ──포트 4319──┤──▶ aggregator ──▶ emit_gate ──▶ Tauri "snapshot" ──▶ UI
Codex rollout ──2s poll──┘    (ring + 5h)    (500ms)
```

**Tech Stack**: Tauri 2 · Rust (tokio, notify, rusqlite, axum, reqwest) · Svelte 5 · TypeScript

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

릴리즈 배포·코드 서명 상세: [docs/RELEASE.md](docs/RELEASE.md)

---

## 알려진 한계

- 앱이 실행 중일 때만 동기화 및 Trigger가 동작합니다.
- Codex: rollout JSONL이 기록되는 활성 세션에서만 사용률이 갱신됩니다.
