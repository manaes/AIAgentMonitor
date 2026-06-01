# AI Agent Monitor

> Claude Code와 Codex의 토큰 사용량을 macOS 상태 바에서 실시간으로 모니터링하는 네이티브 앱

[![Release](https://img.shields.io/github/v/release/manaes/AIAgentMonitor)](https://github.com/manaes/AIAgentMonitor/releases)
[![Platform](https://img.shields.io/badge/platform-macOS%20%7C%20Windows-lightgrey)](#설치)
[![License](https://img.shields.io/badge/license-MIT-blue)](#)

---

## 스크린샷

| 상단바 팝업 | 세부 대시보드 |
|---|---|
| ![Floating](docs/screenshots/floating.png) | ![Detail](docs/screenshots/detail.png) |

> 📸 *실제 사용 화면. OTEL 연결 시 정확한 실시간 수치를 표시합니다.*

---

## 주요 기능

### 📊 실시간 토큰 모니터링
- **tok/s** — 현재 AI 응답 속도 (10초 EMA)
- **5h 사용량 바** — 요금제 한도 대비 현재 사용량 (%)
- **리셋 카운트다운** — "약 X시간 Y분 Z초 남음" 실시간 표시
- **활성 세션 목록** — 프로젝트별 / 모델별 분리 (Active/Idle/Dormant)

### 🎯 OTEL 정밀 모드
Claude Code의 OpenTelemetry 텔레메트리를 직접 수신해서 **Anthropic 서버 계산 기준**의 정확한 토큰 수를 표시합니다.

```json
// ~/.claude/settings.json 에 추가하면 모든 세션에 자동 적용
{
  "env": {
    "CLAUDE_CODE_ENABLE_TELEMETRY": "1",
    "OTEL_METRICS_EXPORTER": "otlp",
    "OTEL_LOGS_EXPORTER": "otlp",
    "OTEL_EXPORTER_OTLP_PROTOCOL": "http/json",
    "OTEL_EXPORTER_OTLP_ENDPOINT": "http://localhost:4318",
    "OTEL_METRIC_EXPORT_INTERVAL": "10000"
  }
}
```

### ⏰ Anchor Trigger (v1.1)
정해진 시각에 자동으로 가벼운 프롬프트를 발사해 5h quota window의 reset 타이밍을 생활 패턴(점심/퇴근)에 고정합니다.

예) 08:00에 ping → 13:00/18:00에 reset → 점심 직후·퇴근 직후 새 quota

---

## 설치

### 다운로드 (권장)

[**최신 릴리즈 다운로드 →**](https://github.com/manaes/AIAgentMonitor/releases/latest)

| 플랫폼 | 파일 | 비고 |
|---|---|---|
| macOS Apple Silicon | `AI.Agent.Monitor_*_aarch64.dmg` | M1/M2/M3/M4 |
| macOS Intel | `AI.Agent.Monitor_*_x64.dmg` | Intel Mac |
| Windows | `AI.Agent.Monitor_*_x64_en-US.msi` | Windows 10/11 |

**macOS 첫 실행 시** Gatekeeper 경고가 뜨면:
```bash
xattr -cr "/Applications/AI Agent Monitor.app"
```
또는 시스템 설정 → 개인 정보 보호 및 보안 → 앱 허용

---

## 사용 방법

### 기본 사용

1. 앱 실행 → 상단 메뉴바에 아이콘 표시
2. **아이콘 클릭** → Detail 창 열기 (Sessions / Triggers 탭)
3. **우클릭** → Open Log Folder / Quit

### 요금제 한도 설정

Detail 창 → 에이전트 카드 → 플랜 드롭다운에서 선택:

| 플랜 | input+output 한도 |
|---|---|
| Free | 30,000 tok |
| Pro | 300,000 tok |
| Max (5×) | 1,500,000 tok |
| Max (20×) | 6,000,000 tok |
| 직접 입력 | 사용자 지정 |

### OTEL 정밀 모드 설정

1. `~/.claude/settings.json`에 위 env 블록 추가
2. 새 Claude 세션 시작
3. Detail 창 상단 **"◎ OTEL 대기 중"** → 10초 내 **"● OTEL 수신 중"** 으로 변경

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
| Claude Code (OTEL) | `localhost:4318` HTTP JSON 수신 | 정확한 tok/s, 비용($) |
| Codex | `~/.codex/state_5.sqlite` 2s polling | 누적 토큰 delta |

모든 접근은 **read-only** 또는 로컬 HTTP 수신이며 외부 서버로 데이터가 전송되지 않습니다.

---

## 아키텍처

```
DATA SOURCES (read-only)
Claude jsonl ──FSEvents──┐
Claude OTEL ──HTTP 4318──┤──▶ aggregator ──▶ emit_gate ──▶ Tauri "snapshot" ──▶ Svelte 5 UI
Codex sqlite ──2s poll───┘    (ring + 5h)    (500ms)
```

**Tech Stack**: Tauri 2 · Rust (tokio, notify, rusqlite, axum) · Svelte 5 · TypeScript

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

- Claude quota 한도: Anthropic 공식 API 미제공 → 요금제 선택 또는 직접 입력 필요
- Codex: `tokens_used` 누적값만 제공 → in/out 분리 불가
- OTEL 미설정 시: jsonl 파싱 기반 근사치 사용
- 앱 미실행 중 Trigger 동작 안 함 (LaunchAgent 미지원)
