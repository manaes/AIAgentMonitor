# AI Agent Monitor

Claude Code와 Codex의 토큰 사용을 macOS menubar에서 실시간으로 보여주는 Tauri 앱 (v1).

## Build

```bash
pnpm install
pnpm tauri dev    # 개발용 (GUI 실행)
pnpm tauri build  # 릴리즈 빌드 → target/release/bundle/dmg/
```

## 데이터 소스 (read-only)

- **Claude Code**: `~/.claude/projects/<인코딩된 프로젝트 경로>/<session-uuid>.jsonl`
  - 메시지 단위 `usage` 필드에서 토큰 추출 (input_tokens, output_tokens, cache_creation_input_tokens, cache_read_input_tokens)
  - FSEvents (`notify` crate) 구독 + offset 캐시로 새 줄만 tail
- **Codex**: `~/.codex/state_5.sqlite` → `threads` 테이블
  - read-only WAL connection, 2초 폴링
  - `tokens_used`는 세션별 **누적값** → watcher가 delta 추출

## 로그

- 위치: `~/Library/Logs/AIMonitor/app.log.*` (일 단위 회전)
- 트레이 우클릭 → "Open Log Folder" 로 바로 열기

## UI

- **Menubar 아이콘 (좌클릭)**: 320px popover. 에이전트별 Big-Number 카드 (속도 + quota bar + 모델·프로젝트).
- **Menubar 아이콘 (우클릭)**: Open Detail Window / Open Log Folder / Quit AI Monitor
- **Detail 윈도우**: AgentCard 2-column grid + Active sessions 리스트 (프로젝트별, 최근 활동 순)

## 알려진 한계 (v1)

- **Quota 한도값**: Claude API는 공식 quota endpoint가 없어 현재는 한도 미표시 (사용량만). 사용자가 plan limit를 알면 v1.x에서 설정창으로 입력하는 방향 검토.
- **Codex 토큰 분리 없음**: `state_5.sqlite.threads.tokens_used`는 누적 합계만 있어 input/output/cache 구분 불가. UI에서는 `tokens_in`에 합쳐 표시.
- **Antigravity 미지원** (v2 후보).
- **Anchor Trigger 미구현** (v1.1 ─ 아래 로드맵 참고).
- **알림 없음** (조용한 관찰용).
- **자체 데이터베이스 없음**: 메모리 ring buffer + 5h rotating bucket. 앱 재시작 시 jsonl/sqlite 다시 읽어 5h 복원.

## v1.1 로드맵 — Anchor Trigger

5h quota window의 reset 타이밍이 사용자의 생활 패턴(점심·퇴근)과 어긋날 때, 특정 시각에 자동으로 가벼운 prompt를 발사해서 window 시작점을 이동시키는 기능.

예: 9-14-19 reset이 자연스럽지 않다면 08:00에 ping 한 번 → 13-18로 anchor가 이동.

구현 예정:
- `scheduler` 모듈 (`tokio_cron_scheduler` 또는 `cron` crate)
- 룰 CRUD (agent / cron 시각 / working_dir / prompt template)
- Claude/Codex CLI 자동 호출 — 트리거 결과 토큰은 기존 watcher가 자동 캡처
- Detail window에 "Triggers" 탭 추가

스펙 자리: `docs/superpowers/specs/2026-05-28-ai-agent-monitor-design.md` §10
데이터 모델 자리: `AgentState.triggered_by: Option<String>`은 v1부터 이미 비워둠

## 24h 스모크 체크리스트 (v1 출시 전)

배포 전 본인이 직접 24h 실사용해서 다음을 확인:

- [ ] menubar 아이콘이 표시되고, 좌클릭 popover toggle이 정상
- [ ] popover에 두 agent 카드가 표시되고 tok/s 값이 사용에 따라 변동
- [ ] Detail 창 "More details →" 진입, 세션 리스트가 최근 활동 순으로 정렬
- [ ] 모델·프로젝트 명이 정확히 표시
- [ ] 활성 → idle → dormant 상태 전이 (60s, 5min 경계 확인)
- [ ] Claude 새 세션 시작 시 자동 detection (jsonl 새 파일 생성)
- [ ] Codex 새 사용 시 2초 이내 delta 반영
- [ ] 앱 비정상 종료 후 재시작 — 5h 데이터 복원
- [ ] 24h 동안 메모리 사용량 안정 (목표 50MB 이하)
- [ ] 우클릭 메뉴: Open Log Folder, Open Detail Window, Quit 정상
- [ ] 로그 파일에 watcher start / poll error 정도만 보이고 panic 없음

## 아키텍처 요약

```
DATA SOURCES (read-only)     RUST BACKEND (Tauri)                 FRONTEND (Svelte 5)
Claude jsonl  ──FSEvents──▶  claude_watcher                       Menubar popover
                              │
Codex sqlite  ──2s poll───▶  codex_watcher                        
                              │
                              ▼
                             aggregator (5분 ring + 5h rotating)   Detail window
                              │
                              ▼
                             emit_gate (500ms throttle + hash)
                              │
                              ▼
                             Tauri "snapshot" event ─────────────▶ store (Svelte 5 $state)
```

자세한 디자인: `docs/superpowers/specs/2026-05-28-ai-agent-monitor-design.md`
구현 계획: `docs/superpowers/plans/2026-05-28-ai-agent-monitor.md`
