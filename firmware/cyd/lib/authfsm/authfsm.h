// AI Agent Monitor — CYD 펌웨어: 페어링/재연결 인증 상태 기계 (순수 함수).
//
// Task 11. 다음에 어떤 동사를 보낼지 — 소켓도 타이머도 저장소도 모르는
// 순수 함수로 뽑는다. iOS 쪽에서 이 결정을 연결 코드 안에 녹여 넣었다가
// 똑같은 종류의 버그가 두 번 났다(브리프 원문) — 그래서 여기서는 연결
// 코드가 실제로 소켓에 무엇을 쓸지 결정하기 전에, 이 모듈이 이미 답을
// 내놓는다.

// (파일 위치 참고) 브리프는 `firmware/cyd/src/` 를 지정했지만 Task 10 의
// cryptov2 와 같은 이유로 `lib/authfsm/` 에 둔다: `pio test -e cyd` 는
// `src/main.cpp` 의 `setup()`/`loop()` 와 테스트 파일 자신의 `setup()`/
// `loop()` 를 함께 링크하므로, `src/` 에 두면 이중 정의로 링크가 실패한다
// (Task 10 이 실측: `firmware/cyd/lib/cryptov2/cryptov2.cpp` 상단 주석).
// 이 모듈도 cryptov2 처럼 전송·설정과 무관한 순수 모듈이라 `lib/` 라는
// 위치와 성격이 맞고, `main.cpp` 는 이 태스크에서 전혀 건드리지 않는다 —
// 이 상태 기계를 실제로 연결에 이어붙이는 것은 뒤 태스크(WebSocket 이
// 붙는 Task 12 이후) 의 몫이다.

#pragma once

#include <Arduino.h>

/// 인증 핸드셰이크에서 다음에 할 일.
enum class AuthStep {
    SendHello2,   // HELLO2:<cpk> 를 보낸다 — 코드가 있다, 새로 페어링한다.
    SendAuth2,    // AUTH2:<cpk> 를 보낸다 — 저장된 토큰으로 재연결한다.
    SendCode2,    // CODE2:<hmac> 를 보낸다 — HELLO2 응답(AwaitingCode2)을 받았다.
    SendProof2,   // PROOF2:<hmac> 를 보낸다 — AUTH2 응답(Nonce2)을 받았다.
    Subscribed,   // 인가됐다 — 세션 키가 있고 스냅샷을 구독할 수 있다.
    NeedsPairing, // 사람이 (Task 14 화면에서) 새 6자리 코드를 입력해야 한다.
    Failed,       // 이 v2-only 펌웨어가 이해할 수 없는 응답 — 사람이 봐야 한다.
};

/// 맥의 Auth 응답 JSON 에서 이 상태 기계가 실제로 쓰는 필드만 뽑은 것.
///
/// 필드 이름과 존재 조건은 오직 `src-tauri/src/ble/pairing.rs` 의
/// `AuthReply::to_json_bytes()`(147~170줄, 2026-08-27 그 커밋 기준 확인)가
/// 정한다 — 아래 필드는 그 직렬화를 손으로 옮겨 적은 것이지 새로 지어낸
/// 이름이 아니다. 실제 JSON 파싱(ArduinoJson)은 Task 12 의 몫이고, 이
/// 태스크는 이미 채워진 `ReplyView` 하나를 놓고 무엇을 할지만 결정한다.
///
/// 맥이 실제로 만드는 8가지 `AuthReply` 중 이 펌웨어(v2-only)가 받을 수
/// 있는 것은 다음 5가지뿐이다(v1 전용 응답 셋 — AwaitingCode/Granted/
/// Nonce/Authorized/Rejected 의 v1 짝 — 은 이 펌웨어가 v1 동사를 절대
/// 보내지 않으므로 도달 불가):
///   - `AwaitingCode2 { epk, nonce }` → `{"ok":false,"v":2,"await":"code","epk":..,"nonce":..}`
///   - `Nonce2 { epk, nonce }`        → `{"ok":false,"v":2,"epk":..,"nonce":..}`
///   - `Granted2 { sealed }`         → `{"ok":true,"v":2,"sealed":..}`
///   - `Authorized2`                 → `{"ok":true,"v":2}`
///   - `Denied { left }`             → `{"ok":false,"left":N}` — **v1/v2 공유**,
///     `Denied` 자체에는 `"v":2` 가 없다(같은 응답 타입을 두 프로토콜
///     버전이 공유하기 때문 — `pairing.rs:158`).
/// 그 외에 실제로 도달 가능한 것은 `Rejected` → 맨몸 `{"ok":false}` 하나뿐
/// 이다. 이건 `Denied`/`Malformed`/시도 만료 등 서로 다른 세 원인을
/// 가리는데(`pairing.rs:855` 의 `Malformed => Rejected`, 그리고 창/논스
/// 만료), authfsm.cpp 의 판정은 그 셋을 구별하지 않는다 — 아래
/// `authOnReply` 문서 참고.
struct ReplyView {
    bool ok = false;
    /// `"v":2` 키가 있었는가. **존재 자체가 v2 라는 뜻**이다(부재 = v1).
    /// 다만 `Denied` 처럼 v1/v2 가 응답 타입 자체를 공유해 이 필드가 없는
    /// 채로도 실제로 도달 가능한 경우가 있다 — "v 없음" 을 곧바로
    /// "v1 이 왔다" 로 읽으면 안 된다. `left` 를 먼저 본다.
    bool v = false;
    /// `"await":"code"` 일 때만 채워진다(AwaitingCode2). 그 외엔 빈 문자열.
    String await;
    /// `"left":N` 이 있었는가와 그 값(Denied 전용). 0 도 유효한 값이라
    /// 빈 문자열 같은 sentinel 로는 "없음" 을 표현할 수 없어 플래그를 둔다.
    bool hasLeft = false;
    uint8_t left = 0;
    /// 맥의 임시 공개키(64자 소문자 hex). AwaitingCode2/Nonce2 에 실린다.
    String epk;
    /// 논스(hex). AwaitingCode2/Nonce2 에 실린다.
    String nonce;
    /// 봉인된 새 토큰(hex). Granted2 에만 실린다 — 있으면 연결 코드가 이걸
    /// 열어 새 토큰을 NVS 에 저장해야 한다는 뜻이다. **이 모듈은 그 여는
    /// 동작을 하지 않는다** — `Subscribed` 를 돌려줄 뿐이고, sealed 를
    /// 열어 토큰을 꺼내 저장하는 것은 호출자(연결 코드)의 몫이다.
    String sealed;
};

/// 연결을 새로 시작할 때 맨 처음 보낼 동사.
///
/// 방금 입력한 코드가 저장된 토큰보다 우선한다 — 사용자가 코드를
/// 입력했다는 것은 "새로 페어링하겠다" 는 명시적 의사다. 맥이 예전
/// 토큰을 이미 폐기했을 수 있고, 그 경우 토큰 재인증(AUTH2)은 어차피
/// 거부되며 그사이 방금 받은 코드는 쓰이지도 못한다.
AuthStep authInitialStep(bool hasToken, bool hasCode);

/// 응답 하나를 보고 다음에 보낼 동사(또는 종료 상태)를 결정한다.
///
/// **"지금 어떤 동사를 보내고 기다리던 중이었는지" 를 인자로 받지
/// 않는다** — 응답의 모양 자체가 그것을 알려준다: `AwaitingCode2` 는
/// `HELLO2` 에만, `Nonce2` 는 `AUTH2` 에만 오는 응답이라 서로 헷갈릴
/// 여지가 없다.
///
/// `hasToken`/`hasCode` 는 그 흐름이 실제로 성립 가능한지 보는 방어용
/// 가드다 — 예를 들어 `AwaitingCode2` 를 받았는데 손에 코드가 없다면
/// (`HELLO2` 는 `hasCode` 일 때만 보내므로 정상 흐름에서는 일어나지 않는
/// 조합이다) `SendCode2` 를 돌려줘 봐야 호출자가 채울 수 없는 빈 코드로
/// HMAC 을 계산하게 될 뿐이다 — 그 대신 `Failed` 로 멈춘다.
///
/// 맨몸 거절(`{"ok":false}`, "v" 없음, "left" 없음)에 대한 판정은
/// authfsm.cpp 의 구현부 주석에 표로 적어 뒀다 — 요약하면: 코드/토큰
/// 거절, 핸드셰이크 만료, 페어링 창 만료라는 서로 다른 세 원인이 이
/// 한 가지 와이어 모양을 공유하고(`pairing.rs:159`, `pairing.rs:855`),
/// 이 펌웨어는 그 셋을 구별할 필요가 없다 — 셋 다 "지금 경로를 포기하고
/// 사람에게 새 코드를 요구한다"(`NeedsPairing`)로 안전하게 하나로
/// 묶인다.
AuthStep authOnReply(const ReplyView &reply, bool hasToken, bool hasCode);
