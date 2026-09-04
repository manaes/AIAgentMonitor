// AI Agent Monitor — CYD 펌웨어: snapshot.cpp(JSON 바이트 → 데이터 모델)
// 순수 파싱 로직 테스트.
//
// `snapshotParse()` 는 소켓도 복호화도 모르는 순수 함수라 여기서 실제
// JSON 바이트를 손으로 만들어 넣고 결과를 고정한다 — "카드에 맥과 같은
// 숫자가 뜬다" 는 육안 대조는 이 테스트를 대신하지 못한다(브리프 원문:
// 필드가 잘못 매핑돼도 우연히 비슷한 값이 나오면 육안으로는 못 잡는다).
//
// 아래 JSON 문자열은 `src-tauri/src/ble/wire.rs` 의 `MirrorSnapshot`/
// `MirrorAgent`/`MirrorProject` 를 손으로 직렬화한 것이다(serde_json 의
// 압축 출력과 같은 모양 — 콜론/콤마 뒤에 공백 없음). 필드 이름은 그
// 파일에서 그대로 옮겼다(brief 의 표가 아니라 실제 소스 기준 — 이유는
// `snapshot.h` 상단 "인용 오차" 주석 참고).

#include <unity.h>
#include <Arduino.h>
#include "snapshot.h"

static bool parse(const char *json, Snapshot &out) {
    return snapshotParse((const uint8_t *)json, strlen(json), out);
}

// ── 정상 스냅샷: 에이전트 2개, 프로젝트 여럿, optional 필드 섞여 있음 ──────

void test_parses_normal_snapshot_all_fields() {
    // 첫 에이전트(claude) — 5h/주간 사용률 전부 실려 있다.
    // 두 번째 에이전트(codex) — pj 가 빈 배열(프로젝트 없음)이어도 죽지
    // 않아야 한다.
    const char *json =
        "{\"v\":1,\"t\":1755500000,\"a\":["
        "{\"k\":0,\"r\":123.5,\"t5\":3000,\"p5\":62.0,\"r5\":1755512400,"
        "\"pw\":41.5,\"rw\":1755900000,\"pj\":["
        "{\"id\":3826002220,\"n\":\"foo\",\"m\":\"claude-opus-5\",\"r\":98.25,"
        "\"t\":1755499987,\"s\":0},"
        "{\"id\":123456,\"n\":\"bar\",\"m\":\"claude-sonnet-5\",\"r\":10.0,"
        "\"t\":1755498000,\"s\":1}"
        "]},"
        "{\"k\":1,\"r\":50.0,\"t5\":1000,\"pj\":[]}"
        "]}";

    Snapshot s;
    TEST_ASSERT_TRUE(parse(json, s));

    TEST_ASSERT_EQUAL_UINT8(1, s.protocolVersion);
    TEST_ASSERT_EQUAL_UINT64(1755500000, s.emittedAtEpochSec);
    TEST_ASSERT_EQUAL_size_t(2, s.agentCount);
    TEST_ASSERT_FALSE(s.agentsTruncated);

    const SnapshotAgent &a0 = s.agents[0];
    TEST_ASSERT_TRUE(a0.kind == SnapshotAgentKind::Claude);
    TEST_ASSERT_EQUAL_FLOAT(123.5f, a0.rateTokPerSec);
    TEST_ASSERT_EQUAL_UINT32(3000, a0.tokens5hCumulative);
    TEST_ASSERT_TRUE(a0.has5hUsagePct);
    TEST_ASSERT_EQUAL_FLOAT(62.0f, a0.usage5hPct);  // 0~100, 100을 곱하거나 나누지 않는다.
    TEST_ASSERT_TRUE(a0.has5hResetAt);
    TEST_ASSERT_EQUAL_UINT64(1755512400, a0.reset5hEpochSec);
    TEST_ASSERT_TRUE(a0.hasWeeklyUsagePct);
    TEST_ASSERT_EQUAL_FLOAT(41.5f, a0.usageWeeklyPct);
    TEST_ASSERT_TRUE(a0.hasWeeklyResetAt);
    TEST_ASSERT_EQUAL_UINT64(1755900000, a0.resetWeeklyEpochSec);

    const SnapshotAgent &a1 = s.agents[1];
    TEST_ASSERT_TRUE(a1.kind == SnapshotAgentKind::Codex);
}

// ── "동기화 전" 상태: p5/r5/pw/rw 가 키 자체로 안 온다 ────────────────────

void test_all_optional_fields_absent_does_not_crash() {
    const char *json =
        "{\"v\":1,\"t\":1755500000,\"a\":["
        "{\"k\":0,\"r\":0.0,\"t5\":0,\"pj\":[]}"
        "]}";

    Snapshot s;
    TEST_ASSERT_TRUE(parse(json, s));
    TEST_ASSERT_EQUAL_size_t(1, s.agentCount);

    const SnapshotAgent &a0 = s.agents[0];
    // 키가 아예 없으므로 has* 는 전부 false 여야 한다 — "값이 0" 과 혼동하면
    // 안 된다(snapshot.h 의 has5hUsagePct 문서).
    TEST_ASSERT_FALSE(a0.has5hUsagePct);
    TEST_ASSERT_FALSE(a0.has5hResetAt);
    TEST_ASSERT_FALSE(a0.hasWeeklyUsagePct);
    TEST_ASSERT_FALSE(a0.hasWeeklyResetAt);
}

// ── 깨진 JSON: 잘림 ────────────────────────────────────────────────────────

void test_truncated_json_fails_cleanly() {
    // 닫는 중괄호가 없다 — deserializeJson 이 IncompleteInput 을 내야 한다.
    const char *json = "{\"v\":1,\"t\":1755500000,\"a\":[{\"k\":0,\"r\":1.0";

    Snapshot before;
    before.agentCount = 42;  // out 이 안 바뀌었는지 확인할 표식.
    Snapshot s = before;
    TEST_ASSERT_FALSE(parse(json, s));
    TEST_ASSERT_EQUAL_size_t(42, s.agentCount);  // 실패 시 out 을 손대지 않는다(snapshot.h 문서).
}

// ── 깨진 JSON: 타입 불일치(최상위) ─────────────────────────────────────────

void test_wrong_type_top_level_fails_cleanly() {
    // a 가 배열이 아니라 문자열이다.
    const char *json = "{\"v\":1,\"t\":1755500000,\"a\":\"nope\"}";
    Snapshot s;
    TEST_ASSERT_FALSE(parse(json, s));
}

// ── 깨진 JSON: 타입 불일치(에이전트 내부 필수 필드) ────────────────────────

void test_wrong_type_nested_field_fails_cleanly() {
    // r(tok/s) 이 숫자가 아니라 문자열이다 — 개별 에이전트를 스킵하지 않고
    // 스냅샷 전체를 실패시킨다(snapshotParse 문서).
    const char *json =
        "{\"v\":1,\"t\":1,\"a\":[{\"k\":0,\"r\":\"abc\",\"t5\":1,\"pj\":[]}]}";
    Snapshot s;
    TEST_ASSERT_FALSE(parse(json, s));
}

// ── 에이전트 개수가 상한을 넘으면 잘라내고 truncated 를 세운다 ────────────

/// 조회 실패 코드는 문장이 아니라 숫자로 온다 — 맥의
/// `quota_error_travels_as_a_code_not_a_message`(wire.rs)와 짝이다.
/// 맥은 실패 중에도 %를 함께 보내므로 그 값도 그대로 파싱돼야 한다
/// (가리는 것은 파서가 아니라 화면의 몫이다 — ui_cards.cpp).
void test_parses_quota_error_code() {
    const char *json =
        "{\"v\":1,\"t\":1,\"a\":[{\"k\":1,\"r\":0,\"t5\":0,\"p5\":8.0,\"pw\":35.0,\"e\":1,\"pj\":[]}]}";
    Snapshot snap;
    TEST_ASSERT_TRUE(snapshotParse((const uint8_t *)json, strlen(json), snap));
    TEST_ASSERT_TRUE(snap.agents[0].hasQuotaError);
    TEST_ASSERT_EQUAL_UINT8(1, snap.agents[0].quotaErrorCode);
    TEST_ASSERT_EQUAL_FLOAT(8.0f, snap.agents[0].usage5hPct);
}

/// 정상이면 맥이 키를 생략한다 — 여기가 깨지면 멀쩡한 카드에 경고가 뜬다.
void test_healthy_snapshot_has_no_quota_error() {
    const char *json = "{\"v\":1,\"t\":1,\"a\":[{\"k\":0,\"r\":0,\"t5\":0,\"p5\":10.0,\"pj\":[]}]}";
    Snapshot snap;
    TEST_ASSERT_TRUE(snapshotParse((const uint8_t *)json, strlen(json), snap));
    TEST_ASSERT_FALSE(snap.agents[0].hasQuotaError);
}

/// 코드↔문구 대응은 맥(`error_kind_codes_are_frozen`)·iOS 와 같은 계약이다.
/// 모르는 코드도 "정상" 으로 되돌아가지 않는다.
void test_quota_error_text_mapping() {
    TEST_ASSERT_EQUAL_STRING("LOGIN REQUIRED", snapshotQuotaErrorText(1));
    TEST_ASSERT_EQUAL_STRING("CLI ERROR", snapshotQuotaErrorText(2));
    TEST_ASSERT_EQUAL_STRING("TIMEOUT", snapshotQuotaErrorText(3));
    TEST_ASSERT_EQUAL_STRING("QUOTA ERROR", snapshotQuotaErrorText(4));
    TEST_ASSERT_EQUAL_STRING("QUOTA ERROR", snapshotQuotaErrorText(99));
}

void test_agents_beyond_cap_are_truncated_not_failed() {
    String json = "{\"v\":1,\"t\":1,\"a\":[";
    for (size_t i = 0; i < SNAPSHOT_MAX_AGENTS + 2; i++) {
        if (i > 0) json += ",";
        json += "{\"k\":0,\"r\":1.0,\"t5\":1}";
    }
    json += "]}";

    Snapshot s;
    TEST_ASSERT_TRUE(parse(json.c_str(), s));
    TEST_ASSERT_EQUAL_size_t(SNAPSHOT_MAX_AGENTS, s.agentCount);
    TEST_ASSERT_TRUE(s.agentsTruncated);
}

void setup() {
    delay(2000);  // 시리얼 모니터가 붙을 시간을 준다.
    UNITY_BEGIN();
    RUN_TEST(test_parses_normal_snapshot_all_fields);
    RUN_TEST(test_all_optional_fields_absent_does_not_crash);
    RUN_TEST(test_truncated_json_fails_cleanly);
    RUN_TEST(test_wrong_type_top_level_fails_cleanly);
    RUN_TEST(test_wrong_type_nested_field_fails_cleanly);
    RUN_TEST(test_antigravity_and_unknown_agent_kind);
    RUN_TEST(test_agents_beyond_cap_are_truncated_not_failed);
    RUN_TEST(test_parses_quota_error_code);
    RUN_TEST(test_healthy_snapshot_has_no_quota_error);
    RUN_TEST(test_quota_error_text_mapping);
    UNITY_END();
}

void loop() {}
