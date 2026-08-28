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
    TEST_ASSERT_EQUAL_size_t(2, a0.projectCount);
    TEST_ASSERT_FALSE(a0.projectsTruncated);

    const SnapshotProject &p0 = a0.projects[0];
    TEST_ASSERT_EQUAL_UINT32(3826002220, p0.id);
    TEST_ASSERT_EQUAL_STRING("foo", p0.name.c_str());
    TEST_ASSERT_EQUAL_STRING("claude-opus-5", p0.model.c_str());
    TEST_ASSERT_EQUAL_FLOAT(98.25f, p0.rateTokPerSec);
    TEST_ASSERT_EQUAL_UINT64(1755499987, p0.lastActivityEpochSec);
    TEST_ASSERT_TRUE(p0.status == SnapshotProjectStatus::Active);

    const SnapshotProject &p1 = a0.projects[1];
    TEST_ASSERT_TRUE(p1.status == SnapshotProjectStatus::Idle);

    const SnapshotAgent &a1 = s.agents[1];
    TEST_ASSERT_TRUE(a1.kind == SnapshotAgentKind::Codex);
    TEST_ASSERT_EQUAL_size_t(0, a1.projectCount);
    TEST_ASSERT_FALSE(a1.projectsTruncated);
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

// ── s(프로젝트 상태) 범위 밖 값 — Dormant 로 fail-safe ─────────────────────

void test_out_of_range_project_status_falls_back_to_dormant() {
    const char *json =
        "{\"v\":1,\"t\":1,\"a\":[{\"k\":0,\"r\":1.0,\"t5\":1,\"pj\":["
        "{\"id\":1,\"n\":\"x\",\"m\":\"y\",\"r\":1.0,\"t\":1,\"s\":9}"
        "]}]}";
    Snapshot s;
    TEST_ASSERT_TRUE_MESSAGE(parse(json, s), "범위 밖 s 는 파싱 실패가 아니다");
    TEST_ASSERT_TRUE(s.agents[0].projects[0].status == SnapshotProjectStatus::Dormant);
}

// ── k(에이전트 종류) — Antigravity(2)와 그 밖의 값(Unknown) ────────────────
//
// 브리프 표에는 k 가 0/1 두 값뿐이라고 적혀 있었지만, 실제 wire.rs 는
// Antigravity=2 도 만든다(snapshot.h 상단 주석) — 그 실제 값과, 향후 더
// 늘어날 수 있는 값(Unknown fail-safe)을 둘 다 확인한다.
void test_antigravity_and_unknown_agent_kind() {
    const char *json =
        "{\"v\":1,\"t\":1,\"a\":["
        "{\"k\":2,\"r\":1.0,\"t5\":1,\"pj\":[]},"
        "{\"k\":99,\"r\":1.0,\"t5\":1,\"pj\":[]}"
        "]}";
    Snapshot s;
    TEST_ASSERT_TRUE(parse(json, s));
    TEST_ASSERT_TRUE(s.agents[0].kind == SnapshotAgentKind::Antigravity);
    TEST_ASSERT_TRUE(s.agents[1].kind == SnapshotAgentKind::Unknown);
}

// ── 프로젝트 개수가 상한을 넘으면 잘라내고 truncated 를 세운다(실패 아님) ──

void test_projects_beyond_cap_are_truncated_not_failed() {
    String json = "{\"v\":1,\"t\":1,\"a\":[{\"k\":0,\"r\":1.0,\"t5\":1,\"pj\":[";
    const size_t overflowCount = SNAPSHOT_MAX_PROJECTS_PER_AGENT + 3;
    for (size_t i = 0; i < overflowCount; i++) {
        if (i > 0) json += ",";
        json += "{\"id\":" + String((unsigned)i) +
                ",\"n\":\"p\",\"m\":\"m\",\"r\":1.0,\"t\":1,\"s\":0}";
    }
    json += "]}]}";

    Snapshot s;
    TEST_ASSERT_TRUE(parse(json.c_str(), s));
    TEST_ASSERT_EQUAL_size_t(SNAPSHOT_MAX_PROJECTS_PER_AGENT, s.agents[0].projectCount);
    TEST_ASSERT_TRUE(s.agents[0].projectsTruncated);
}

void setup() {
    delay(2000);  // 시리얼 모니터가 붙을 시간을 준다.
    UNITY_BEGIN();
    RUN_TEST(test_parses_normal_snapshot_all_fields);
    RUN_TEST(test_all_optional_fields_absent_does_not_crash);
    RUN_TEST(test_truncated_json_fails_cleanly);
    RUN_TEST(test_wrong_type_top_level_fails_cleanly);
    RUN_TEST(test_wrong_type_nested_field_fails_cleanly);
    RUN_TEST(test_out_of_range_project_status_falls_back_to_dormant);
    RUN_TEST(test_antigravity_and_unknown_agent_kind);
    RUN_TEST(test_projects_beyond_cap_are_truncated_not_failed);
    UNITY_END();
}

void loop() {}
