// AI Agent Monitor — CYD 펌웨어: 봉인 스냅샷 평문(JSON) → 데이터 모델 (Task 15a)
//
// (파일 위치 참고) 브리프는 `firmware/cyd/src/` 를 지정했지만, Task 10/11/12
// 가 세운 것과 같은 이유(`lib/cryptov2/cryptov2.cpp` 상단 주석의 실측 — 테스트
// 빌드가 `src/` 의 `setup()`/`loop()` 를 테스트 파일 자신의 것과 함께 링크해
// 이중 정의로 실패한다)로 `lib/snapshot/` 에 둔다. `snapshotParse()` 는 소켓도
// 복호화도 모르는 순수 함수라 이 위치에서 `pio test -e cyd` 로 하드웨어에서
// 곧장 검증할 수 있다. `Transport::handleSnapshotFrame()`(src/transport.cpp)
// 는 이 함수를 부르기만 한다 — 파싱 로직 자체는 여기 없다.
//
// 와이어 스키마는 `src-tauri/src/ble/wire.rs` 의 `MirrorSnapshot`/`MirrorAgent`/
// `MirrorProject` 를 그대로 옮긴 것이다(2026-08-27, 직접 확인).
//
// **주의 — 이 태스크의 브리프 자체가 인용한 필드 표에 오차가 있었다.** 브리프는
// `k` 를 "0=claude, 1=codex" 두 값으로만 적었지만, 실제 `wire.rs` 의
// `impl From<&AgentState> for MirrorAgent` 는 `AgentKind::Antigravity => 2` 도
// 만든다(`types.rs:7` 의 `enum AgentKind { Claude, Codex, Antigravity }` 도
// 세 값이다). 이 저장소가 반복해 겪은 "인용 오차" 를 이번에도 만난 것이다 —
// 그래서 이 파일은 브리프의 표가 아니라 `wire.rs`/`types.rs` 실물을 기준으로
// 세 값을 전부 다룬다(아래 `SnapshotAgentKind`).

#pragma once

#include <Arduino.h>

/// 한 스냅샷에 실릴 수 있는 에이전트 개수의 상한. 맥 쪽에 이 값을 강제하는
/// 상수는 없다(`grep` 으로 확인, `MAX_AGENTS` 류가 없다) — 다만 `AgentKind`
/// (`types.rs:7`)가 이론상 세 값(Claude/Codex/Antigravity)뿐이고, 맥이 같은
/// 종류를 두 번 보고할 이유가 없어 4면 여유롭게 넘친다.
///
/// **처음에는 8로 잡았다가 4로 줄였다** — `Transport::latestSnapshot_`
/// (`transport.h`)이 이 구조체를 **값으로**(포인터가 아니라) 들고 있는데,
/// `Transport` 인스턴스 자체가 `main.cpp` 의 전역 `static Transport
/// transport;` 라 이 배열들이 힙이 아니라 **DRAM(.bss) 에 고정으로
/// 잡힌다**. 8×32(구 상한)일 때 `pio run -e cyd` 가 실제로
/// `region 'dram0_0_seg' overflowed by 6192 bytes` 로 링크에 실패하는 것을
/// 실측했다 — 이 캡을 크게 잡는 것이 공짜가 아니라는 것을 그 실패가
/// 증명한다. 이 상한을 넘는 초과분은 잘라내고 `Snapshot::agentsTruncated`
/// 를 세운다 — 조용히 버리지 않는다.
constexpr size_t SNAPSHOT_MAX_AGENTS = 4;

/// 한 에이전트에 실릴 수 있는 프로젝트 개수의 상한. 맥 쪽에도 대응하는 상한이
/// 없어(같은 방식으로 확인) 이론상 64KiB 프레임(`MAX_FRAME_BYTES`, `server.rs`)
/// 안에서 얼마든지 늘 수 있다 — 이 상한은 실측값이 아니라 이 보드의 한정된
/// DRAM 을 보호하기 위한 방어값이다(`SNAPSHOT_MAX_AGENTS` 문서의 DRAM
/// overflow 실측 참고 — 이 값도 그 실측을 보고 8→32 대신 4→12 로 줄였다).
constexpr size_t SNAPSHOT_MAX_PROJECTS_PER_AGENT = 12;

/// `MirrorAgent.k`(wire.rs). 범위 밖 값(향후 맥이 새 에이전트 종류를 추가하는
/// 경우)은 파싱 자체를 실패시키지 않고 `Unknown` 으로 fail-safe 한다 — 이
/// 펌웨어가 모르는 에이전트 하나 때문에 스냅샷 전체(다른 에이전트들의 정상
/// 데이터까지)를 버릴 이유가 없다. 이 값을 화면에 어떻게 그릴지는 Task 15b
/// (카드 화면)의 몫이다.
enum class SnapshotAgentKind : uint8_t {
    Claude = 0,
    Codex = 1,
    Antigravity = 2,
    Unknown = 255,
};

/// `MirrorProject.s`(wire.rs). 범위 밖 값은 `Dormant` 로 fail-safe 한다 —
/// "이 프로젝트가 지금 활동 중이다" 라고 잘못 우기는 것보다, 모르는 상태를
/// 가장 낮은 활동성으로 깎아 보여주는 쪽이 안전하다: 실제로 조용한 프로젝트를
/// active 로 잘못 표시하면 사람이 신경 쓸 필요 없는 곳에 주의를 뺏기지만,
/// 거꾸로(active 인데 dormant 로 보임)는 최악의 경우 화면을 한 번 더 눌러
/// 확인하는 정도로 끝난다.
enum class SnapshotProjectStatus : uint8_t {
    Active = 0,
    Idle = 1,
    Dormant = 2,
};

/// `MirrorProject`(wire.rs) 하나.
struct SnapshotProject {
    uint32_t id = 0;                    // wire.rs `id` — FNV-1a(경로). 전체 경로는 오지 않는다(프라이버시).
    String name;                        // wire.rs `n`
    String model;                       // wire.rs `m`
    float rateTokPerSec = 0.0f;         // wire.rs `r` — tok/s
    uint64_t lastActivityEpochSec = 0;  // wire.rs `t` — epoch 초
    SnapshotProjectStatus status = SnapshotProjectStatus::Dormant;  // wire.rs `s`
};

/// `MirrorAgent`(wire.rs) 하나.
///
/// **퍼센트 단위는 맥과 동일하게 0~100 float 다(0~1 소수가 아니다).**
/// `src-tauri/src/ble/wire.rs` 의 `omits_null_quota_fields_from_json` 테스트가
/// `quota_used_pct: Some(62.0)` 를 `"p5":62` 로 그대로 직렬화하는 것을 직접
/// 확인해 준다 — 이 파서는 그 숫자를 나누거나 곱하지 않고 그대로 옮겨 담는다.
/// Task 15b 가 화면에 `%` 를 붙일 때 이 값을 다시 100을 곱하면 틀린다.
struct SnapshotAgent {
    SnapshotAgentKind kind = SnapshotAgentKind::Unknown;  // wire.rs `k`
    float rateTokPerSec = 0.0f;         // wire.rs `r` — tok/s
    uint32_t tokens5hCumulative = 0;    // wire.rs `t5` — "동기화 전" 캐시 표시에만 쓰이는 값

    /// wire.rs `p5`(`Option<f32>`). 맥이 아직 5h 사용률을 동기화하지 못한
    /// 상태("동기화 전")면 이 키 자체가 JSON 에 없다 — 그때는 false 로
    /// 남는다. **0.0f 를 "0%" 로 착각하면 안 된다.**
    bool has5hUsagePct = false;
    float usage5hPct = 0.0f;            // 0~100

    bool has5hResetAt = false;          // wire.rs `r5`(`Option<u64>`)
    uint64_t reset5hEpochSec = 0;       // epoch 초

    bool hasWeeklyUsagePct = false;     // wire.rs `pw`(`Option<f32>`)
    float usageWeeklyPct = 0.0f;        // 0~100

    bool hasWeeklyResetAt = false;      // wire.rs `rw`(`Option<u64>`)
    uint64_t resetWeeklyEpochSec = 0;   // epoch 초

    SnapshotProject projects[SNAPSHOT_MAX_PROJECTS_PER_AGENT];  // wire.rs `pj`
    size_t projectCount = 0;
    /// `pj` 의 실제 원소 수가 `SNAPSHOT_MAX_PROJECTS_PER_AGENT` 를 넘어
    /// 잘려 나갔는가.
    bool projectsTruncated = false;
};

/// `MirrorSnapshot`(wire.rs) 전체.
struct Snapshot {
    uint8_t protocolVersion = 0;        // wire.rs `v`
    uint64_t emittedAtEpochSec = 0;     // wire.rs `t` — epoch 초
    SnapshotAgent agents[SNAPSHOT_MAX_AGENTS];  // wire.rs `a`
    size_t agentCount = 0;
    /// `a` 의 실제 원소 수가 `SNAPSHOT_MAX_AGENTS` 를 넘어 잘려 나갔는가.
    bool agentsTruncated = false;
};

/// 평문 JSON 바이트 → `Snapshot`. 소켓도 복호화도 전혀 모르는 순수 함수 —
/// `SealedChannel::open()`(`lib/cryptov2/`)이 이미 연 평문을 그대로 받는다.
/// `json` 은 널 종단을 요구하지 않는다(`len` 을 그대로 믿는다).
///
/// **실패 조건(필수 필드 기준으로 스냅샷 전체를 버린다):**
///   - `json`이 애초에 유효한 JSON이 아니다(잘림 등)
///   - 최상위 `v`/`t`/`a` 중 하나라도 없거나 선언된 타입이 아니다
///   - 에이전트 객체의 `k`/`r`/`t5`/`pj` 중 하나라도 없거나 타입이 다르다
///   - 프로젝트 객체의 `id`/`n`/`m`/`r`/`t`/`s` 중 하나라도 없거나 타입이 다르다
/// 실패하면 false 를 돌려주고 **`out` 은 손대지 않는다** — 호출자가 "복호화는
/// 됐지만 내용이 이상하다" 와 "이전 스냅샷을 그대로 유지한다" 를 스스로 정할
/// 수 있게 한다.
///
/// **실패가 아닌 것:**
///   - optional 필드(`p5`/`r5`/`pw`/`rw`)의 부재 — `has*` 플래그가 false 로
///     남을 뿐이다(맥이 `#[serde(skip_serializing_if = "Option::is_none")]`
///     로 아예 키를 생략하기 때문에 정상적으로 자주 일어난다, `wire.rs:34-39`).
///   - `k`/`s` 의 범위 밖 값 — 위 두 enum 문서에 적은 fail-safe 값으로
///     대체될 뿐이다.
///   - `SNAPSHOT_MAX_AGENTS`/`SNAPSHOT_MAX_PROJECTS_PER_AGENT` 초과 — 잘라내고
///     해당 `*Truncated` 플래그를 세울 뿐이다.
bool snapshotParse(const uint8_t *json, size_t len, Snapshot &out);
