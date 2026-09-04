# AI Agent Monitor 아키텍처

이 문서는 현재 릴리즈의 구현 구조를 설명한다. `docs/superpowers/`는 구현 당시 설계·계획 기록이며 현재 동작의 기준 문서가 아니다.

## 데이터 수집

| 에이전트 | 입력 | 수집 방식 |
|---|---|---|
| Claude Code | `~/.claude/projects/**/*.jsonl` | FSEvents 감시와 시작 시 기존 이벤트 재생 |
| Claude quota | Anthropic 응답 헤더 | 로컬 프록시 `127.0.0.1:4319`를 경유한 자동/수동 ping |
| Codex | `~/.codex/sessions/**/rollout-*.jsonl` | 최신 rollout을 2초마다 탐색·tail |
| Codex quota | `codex app-server` 의 `account/rateLimits/read` | 유휴 시에도 10분마다 조회(토큰 소모 없음) |
| 한도 조회 실패 | 각 조회 경로의 실패 사유 | 데스크톱은 문장, 미러(iOS·CYD)는 1바이트 코드(`MirrorAgent.e`) |
| Codex 구버전 | `~/.codex/state_5.sqlite` | `threads.rollout_path`를 읽는 호환 폴백 |
| Antigravity | 로컬 대화 DB와 `agy -p /usage` | 파일 감시 및 설정 가능한 주기 polling |

입력은 `TokenEvent`로 정규화된다. 집계기는 10초 EMA tok/s, 5시간 회전 버킷, 프로젝트 활동 상태(Active 60초 / Idle 5분 / Dormant)를 만든다. 서버가 보고한 5시간·주간 quota가 있으면 로컬 추정보다 우선한다.

## 출력 경로

| 경로 | 대상 | 주기/형식 |
|---|---|---|
| Tauri event | 메뉴바 popover와 Detail 창 | 변경 시 최대 500ms 단위 JSON |
| BLE | iPhone/iPad, CYD | 최대 초당 1회, 암호화 프레임을 MTU에 맞춰 청크 전송 |
| 원격 네트워크 | iPhone/iPad | iroh 기반 인터넷/P2P, QR 부트스트랩 |
| 로컬 LAN | CYD | TCP 4320 WebSocket, 암호화된 바이너리 프레임 |

세 전송 토글은 서로 독립이다. 페어링 창, 시도 예산과 peer 저장소는 공유한다. 표시 에이전트 설정은 UI와 외부 미러에 함께 적용되지만 수집기는 계속 실행된다.

## 영속성과 포트

- Mac은 앱 설정과 통합 peer 저장소를 보관한다. 토큰 집계는 메모리 기반이며 시작 시 최근 원본 로그에서 복원한다.
- CYD는 Wi-Fi 자격증명, Mac 호스트, 선택 전송과 페어링 토큰을 ESP32 NVS에 저장한다.
- `127.0.0.1:4319`: Claude quota 프록시. 외부 인터페이스에 바인딩하지 않는다.
- `0.0.0.0:4320`: 사용자가 LAN 공유를 켰을 때만 열리는 CYD WebSocket.
- LAN 자동 검색은 mDNS를 사용한다. 멀티캐스트가 막히면 Devices 탭의 `IP:4320`을 직접 입력한다.

## 기준 문서

- [iOS·CYD 데이터 송수신](DATA_TRANSPORT.md)
- [보안과 E2EE](SECURITY.md)
- [CYD 설치 및 문제 해결](../firmware/docs/CYD_SETUP_GUIDE.md)
- [Codex 저장 형식 호환성](../app/docs/codex-schema.md)
- [릴리즈 및 배포](../app/docs/RELEASE.md)
