#include "authfsm.h"

AuthStep authInitialStep(bool hasToken, bool hasCode) {
    // 코드가 있으면 토큰이 있어도 코드를 쓴다 — authfsm.h 문서 참고.
    if (hasCode) return AuthStep::SendHello2;
    if (hasToken) return AuthStep::SendAuth2;
    return AuthStep::NeedsPairing;
}

// ─────────────────────────────────────────────────────────────────────────────
// authOnReply 판정 표.
//
// 맥이 실제로 만드는 `AuthReply` 8종 중 이 v2-only 펌웨어가 받을 수 있는
// 5종 + 도달 가능한 v1/v2 공유 응답 `Denied` + `Rejected` 를 모두 아래
// 표로 분류한다(근거는 authfsm.h 의 `ReplyView` 문서, 원본은
// `src-tauri/src/ble/pairing.rs:147-170` 의 `to_json_bytes()`).
//
//   ok    v      await   nonce   left    → 다음 동작
//   true  true   -       -       -       Subscribed        (Granted2/Authorized2)
//   true  false  -       -       -       Failed            (v1 성공 모양 — 도달 불가, fail-safe)
//   false true   "code"  -       -       SendCode2 (hasCode 없으면 Failed)  (AwaitingCode2)
//   false true   ""      있음    -       SendProof2 (hasToken 없으면 Failed) (Nonce2)
//   false true   ""      없음    -       Failed            (v2 태그는 있는데 알려진 모양이 아님)
//   false false  -       -       있음    NeedsPairing      (Denied — 코드 오답, 예산 소모)
//   false false  -       -       없음    NeedsPairing      (Rejected — 세 원인 공유, 구별 불필요)
//
// **맨 마지막 두 줄이 같은 결론(NeedsPairing)인 것이 요점이다.** `Denied`
// 와 `Rejected` 는 원인이 다르지만(하나는 코드 추측 실패, 하나는 거절/
// 만료가 뒤섞인 것) 이 펌웨어가 할 수 있는 유일하게 안전한 다음 동작은
// 둘 다 같다 — 지금 핸드셰이크를 버리고 사람이 새 코드를 넣을 때까지
// 기다린다. `CODE2` 는 성공이든 실패든 핸드셰이크를 소비하므로
// (`pairing.rs:739` 부근 주석 — "재시도하려면 HELLO2 부터다") 같은 코드로
// 자동 재시도하는 것 자체가 불가능하고, 다른 코드를 쓰려면 애초에 사람이
// 개입해야 한다 — 그러니 `left` 값(몇 번 남았는지)이 몇이든 이 함수의
// 결론에는 영향이 없다. `left` 는 여전히 `ReplyView` 에 남겨 둔다 —
// Task 14 화면이 "3번 남음" 같은 걸 보여주려면 필요하지만, 그건 UI 의
// 몫이지 이 상태 기계가 분기할 이유는 아니다.
// ─────────────────────────────────────────────────────────────────────────────
AuthStep authOnReply(const ReplyView &reply, bool hasToken, bool hasCode) {
    if (reply.ok) {
        // v1 의 `Granted{token}`/`Authorized` 도 `"ok":true` 지만 `"v":2` 가
        // 없다 — 이 펌웨어는 v1 동사를 절대 보내지 않으므로 정상 흐름에서는
        // 도달 불가능하다. 그런데도 이 모양이 온다면 이 펌웨어는 v2 세션
        // 키를 하나도 유도하지 않았다는 뜻이라, "성공" 을 그대로 믿고
        // Subscribed 로 넘어가면 세션 키 없이 인가됐다고 착각하게 된다 —
        // 그러느니 멈춘다.
        return reply.v ? AuthStep::Subscribed : AuthStep::Failed;
    }

    if (reply.v) {
        // v2 로 태그된 거절 — AwaitingCode2 또는 Nonce2 둘 중 하나여야 한다.
        if (reply.await == "code") {
            // HELLO2 는 hasCode 일 때만 보내므로(authInitialStep), 이 응답을
            // 받았는데 hasCode 가 거짓이면 호출자가 상태 기계의 계약을 어긴
            // 것이다 — 빈 코드로 CODE2 의 HMAC 을 계산하게 두지 않는다.
            return hasCode ? AuthStep::SendCode2 : AuthStep::Failed;
        }
        if (reply.nonce.length() > 0) {
            // 같은 논리로, Nonce2 는 AUTH2(= hasToken 일 때만 보냄)의
            // 응답이다. hasToken 이 거짓인데 이게 왔다면 마찬가지로 멈춘다.
            return hasToken ? AuthStep::SendProof2 : AuthStep::Failed;
        }
        // "v":2 는 있는데 AwaitingCode2 도 Nonce2 도 아닌 모양 — 오늘의
        // 프로토콜에는 없는 조합이다. 알지 못하는 것을 관대하게 넘기지
        // 않는다(맥 쪽 parse_auth_request 가 같은 원칙으로 Malformed 를
        // 쓰는 것과 대칭).
        return AuthStep::Failed;
    }

    // "v" 없는 거절 — Denied(left 있음, 코드 오답)와 Rejected(맨몸, 세
    // 원인 공유) 모두 여기로 들어오고 결론은 같다. 표 주석 참고.
    return AuthStep::NeedsPairing;
}
