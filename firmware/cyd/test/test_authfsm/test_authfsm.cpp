// AI Agent Monitor — CYD 펌웨어: 인증 상태 기계(authfsm) 테스트.
//
// `authInitialStep`/`authOnReply` 가 순수 함수라 소켓 없이 여기서 전부
// 고정한다. 맥의 실제 응답 모양은 `src-tauri/src/ble/pairing.rs:147-170`
// (`AuthReply::to_json_bytes()`) 를 그대로 옮겨 적은 것이다 — 골든 벡터
// 처럼 세 언어를 대조하는 파일은 아니지만, 문자열 자체가 그 함수의 출력과
// 바이트 단위로 같아야 이 테스트가 실제 와이어를 검증하는 게 된다.

#include <unity.h>
#include <Arduino.h>
#include "authfsm.h"

// ─────────────────────────────────────────────────────────────────────────────
// parseReply — 테스트 전용 손짜기 스캐너.
//
// **프로덕션에 링크되지 않는다** — `lib/authfsm/` 어디에도 이 함수가 없고
// `test/` 안에만 있다. ArduinoJson 은 Task 12 의 몫으로 미뤄 둔 의존성이라
// (`platformio.ini` 의 lib_deps 주석 참고) 여기서 끌어오지 않는다. 이 아래
// 테스트들이 쓰는 고정된 몇 가지 모양(따옴표 뒤에 공백 없이 압축된 JSON —
// `to_json_bytes()` 의 `format!` 이 실제로 그렇게 찍는다)만 읽으면 되므로,
// 일반 JSON 파서를 흉내 낼 필요가 없다.
static String extractString(const String &json, const char *key) {
    String needle = String("\"") + key + "\":\"";
    int idx = json.indexOf(needle);
    if (idx < 0) return String();
    int start = idx + needle.length();
    int end = json.indexOf('"', start);
    if (end < 0) return String();
    return json.substring(start, end);
}

static ReplyView parseReply(const char *jsonCStr) {
    String json(jsonCStr);
    ReplyView r;
    r.ok = json.indexOf("\"ok\":true") >= 0;
    r.v = json.indexOf("\"v\":2") >= 0;
    r.await = extractString(json, "await");
    r.epk = extractString(json, "epk");
    r.nonce = extractString(json, "nonce");
    r.sealed = extractString(json, "sealed");

    const char *leftKey = "\"left\":";
    int leftIdx = json.indexOf(leftKey);
    if (leftIdx >= 0) {
        int start = leftIdx + (int)strlen(leftKey);
        int end = start;
        while (end < (int)json.length() && isDigit(json[end])) end++;
        if (end > start) {
            r.hasLeft = true;
            r.left = (uint8_t)json.substring(start, end).toInt();
        }
    }
    return r;
}

// ── authInitialStep ─────────────────────────────────────────────────────────

/// 방금 입력한 코드가 저장된 토큰보다 우선한다. 사용자가 키패드로 코드를
/// 넣었다는 것은 "새로 페어링하겠다"는 명시적 의사다. 맥이 토큰을 이미
/// 폐기했다면 토큰 재인증은 반드시 거부되고, 그 사이 코드는 쓰이지도 못한다.
void test_fresh_code_wins_over_stored_token() {
    TEST_ASSERT_EQUAL(AuthStep::SendHello2, authInitialStep(true, true));
}

void test_token_without_code_reconnects() {
    TEST_ASSERT_EQUAL(AuthStep::SendAuth2, authInitialStep(true, false));
}

void test_no_token_no_code_needs_pairing() {
    TEST_ASSERT_EQUAL(AuthStep::NeedsPairing, authInitialStep(false, false));
}

// ── authOnReply: 성공 경로 ──────────────────────────────────────────────────

/// CODE2 성공(Granted2) — 새 토큰이 sealed 안에 있다. 그걸 열어 저장하는
/// 것은 연결 코드의 몫이고, 이 함수는 "이제 인가됐다" 만 알려준다.
void test_granted2_reaches_subscribed() {
    ReplyView r = parseReply(
        "{\"ok\":true,\"v\":2,\"sealed\":\"aabbcc\"}");
    TEST_ASSERT_EQUAL(AuthStep::Subscribed, authOnReply(r, true, false));
}

/// PROOF2 성공(Authorized2) — 이미 아는 토큰을 확인했을 뿐이라 필드가 없다.
void test_authorized2_reaches_subscribed() {
    ReplyView r = parseReply("{\"ok\":true,\"v\":2}");
    TEST_ASSERT_EQUAL(AuthStep::Subscribed, authOnReply(r, true, false));
}

/// `"ok":true` 인데 `"v":2` 가 없는 모양은 v1 의 `Granted`/`Authorized` 다.
/// 이 펌웨어는 v1 동사를 절대 보내지 않으므로 정상 흐름에서는 도달
/// 불가능하지만, 혹시 온다면 이 펌웨어는 v2 세션 키를 하나도 유도하지
/// 않은 채로 "인가됐다" 고 믿게 된다 — 그러느니 멈춘다.
void test_v1_shaped_success_fails_safe() {
    ReplyView r = parseReply("{\"ok\":true}");
    TEST_ASSERT_EQUAL(AuthStep::Failed, authOnReply(r, true, false));
}

// ── authOnReply: 핸드셰이크 진행 ────────────────────────────────────────────

/// HELLO2 에 대한 AwaitingCode2 — CODE2 로 이어간다.
void test_awaiting_code2_advances_to_send_code2() {
    ReplyView r = parseReply(
        "{\"ok\":false,\"v\":2,\"await\":\"code\",\"epk\":\"ee\",\"nonce\":\"44\"}");
    TEST_ASSERT_EQUAL(AuthStep::SendCode2, authOnReply(r, false, true));
}

/// AUTH2 에 대한 Nonce2(await 필드 없음) — PROOF2 로 이어간다.
void test_nonce2_advances_to_send_proof2() {
    ReplyView r = parseReply(
        "{\"ok\":false,\"v\":2,\"epk\":\"ee\",\"nonce\":\"44\"}");
    TEST_ASSERT_EQUAL(AuthStep::SendProof2, authOnReply(r, true, false));
}

/// AwaitingCode2 를 받았는데 손에 코드가 없다 — HELLO2 는 hasCode 일 때만
/// 보내므로 정상 흐름에서는 안 생기는 조합이다. 빈 코드로 HMAC 을 계산하게
/// 두지 않고 멈춘다.
void test_awaiting_code2_without_a_code_in_hand_fails_safe() {
    ReplyView r = parseReply(
        "{\"ok\":false,\"v\":2,\"await\":\"code\",\"epk\":\"ee\",\"nonce\":\"44\"}");
    TEST_ASSERT_EQUAL(AuthStep::Failed, authOnReply(r, false, false));
}

/// Nonce2 를 받았는데 손에 토큰이 없다 — AUTH2 는 hasToken 일 때만 보내므로
/// 마찬가지로 멈춘다.
void test_nonce2_without_a_token_in_hand_fails_safe() {
    ReplyView r = parseReply(
        "{\"ok\":false,\"v\":2,\"epk\":\"ee\",\"nonce\":\"44\"}");
    TEST_ASSERT_EQUAL(AuthStep::Failed, authOnReply(r, false, false));
}

/// "v":2 는 있는데 AwaitingCode2 도 Nonce2 도 아닌 모양 — 오늘의 프로토콜엔
/// 없는 조합이다. 알지 못하는 것을 관대하게 넘기지 않는다.
void test_unrecognized_v2_tagged_rejection_fails_safe() {
    ReplyView r = parseReply("{\"ok\":false,\"v\":2}");
    TEST_ASSERT_EQUAL(AuthStep::Failed, authOnReply(r, true, true));
}

// ── authOnReply: "v" 없는 거절 — 전부 NeedsPairing 으로 묶인다 ─────────────

/// 토큰이 거부되면 그것을 버리고 코드를 요구한다. **재시도하지 않는다** —
/// 코드 없이는 통과할 수 없는 재시도가 화면을 깜빡이게 한다.
void test_rejected_token_asks_for_a_code_without_retrying() {
    ReplyView r = parseReply("{\"ok\":false}");
    TEST_ASSERT_EQUAL(AuthStep::NeedsPairing, authOnReply(r, true, false));
}

/// (T11-C) 맨몸 거절(`{"ok":false}`)은 세 가지 실제 원인 — 코드/토큰 거절,
/// 핸드셰이크 만료, 페어링 창 만료 — 를 가리지만(`pairing.rs:159`,
/// `pairing.rs:855` 의 `Malformed => Rejected`), 이 펌웨어는 그 셋을
/// 구별할 필요가 없다: 셋 다 안전한 결론은 하나, 지금 경로를 포기하고
/// 사람에게 새 코드를 요구하는 것이다.
///
/// 브리프 원안의 이름은 "v1 로 다운그레이드하지 않는다" 였고 테스트
/// 대상도 v1 의 진짜 `AwaitingCode` 응답(`{"ok":false,"await":"code"}`)
/// 이었는데, 그 응답은 맥이 v1 리터럴 `HELLO` 를 받았을 때만 보낸다
/// (`pairing.rs:626-627`) — 이 펌웨어는 `HELLO` 를 절대 보내지 않고
/// `HELLO2:...` 만 보내므로, 그 응답 자체를 받을 길이 없다(리뷰 T11-C).
/// 게다가 "다운그레이드" 라는 표현 자체가 오해를 부른다: `AuthStep` 에는
/// 애초에 v1 동사(SendHello/SendAuth 같은 값)가 없으므로 "v1 로 물러선다"
/// 는 이 타입 시스템에서 표현조차 불가능하다. 이 테스트가 실제로 고정하는
/// 것은 그게 아니라, 진짜로 도달 가능한 맨몸 거절을 받았을 때 항상
/// `NeedsPairing`(코드 요구)으로 후퇴한다는 것이다 — 그래서 실제 와이어
/// 모양(`{"ok":false}`)으로 바꾸고 이름도 그에 맞게 고쳤다.
void test_bare_rejection_always_retreats_to_needing_a_fresh_code() {
    ReplyView r = parseReply("{\"ok\":false}");
    TEST_ASSERT_EQUAL(AuthStep::NeedsPairing, authOnReply(r, false, true));
}

/// Denied(left 있음) — CODE2 에 잘못된 코드를 보낸 경우. 핸드셰이크는
/// 성공/실패와 무관하게 서버에서 소비되므로(`pairing.rs` `Code2` 처리부
/// 주석 — "재시도하려면 HELLO2 부터다") 같은 코드로 자동 재시도가 불가능
/// 하고, 다른 코드를 쓰려면 사람이 개입해야 한다 — `left` 값이 몇이든
/// 결론은 같다.
void test_denied_with_attempts_left_still_retreats_to_needing_a_fresh_code() {
    ReplyView r = parseReply("{\"ok\":false,\"left\":3}");
    TEST_ASSERT_EQUAL(AuthStep::NeedsPairing, authOnReply(r, false, true));
}

void setup() {
    delay(2000); // 시리얼 모니터가 붙을 시간을 준다.
    UNITY_BEGIN();
    RUN_TEST(test_fresh_code_wins_over_stored_token);
    RUN_TEST(test_token_without_code_reconnects);
    RUN_TEST(test_no_token_no_code_needs_pairing);
    RUN_TEST(test_granted2_reaches_subscribed);
    RUN_TEST(test_authorized2_reaches_subscribed);
    RUN_TEST(test_v1_shaped_success_fails_safe);
    RUN_TEST(test_awaiting_code2_advances_to_send_code2);
    RUN_TEST(test_nonce2_advances_to_send_proof2);
    RUN_TEST(test_awaiting_code2_without_a_code_in_hand_fails_safe);
    RUN_TEST(test_nonce2_without_a_token_in_hand_fails_safe);
    RUN_TEST(test_unrecognized_v2_tagged_rejection_fails_safe);
    RUN_TEST(test_rejected_token_asks_for_a_code_without_retrying);
    RUN_TEST(test_bare_rejection_always_retreats_to_needing_a_fresh_code);
    RUN_TEST(test_denied_with_attempts_left_still_retreats_to_needing_a_fresh_code);
    UNITY_END();
}

void loop() {}
