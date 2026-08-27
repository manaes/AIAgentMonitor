// AI Agent Monitor — CYD 펌웨어: transportlogic(백오프·호스트 선택·재연결 판정)
// 순수 함수 테스트. 소켓도 mDNS 도 없이 하드웨어에서 돈다.

#include <unity.h>
#include <Arduino.h>
#include "transportlogic.h"

// ── transportBackoffMs ──────────────────────────────────────────────────────

void test_backoff_starts_at_two_seconds() {
    TEST_ASSERT_EQUAL_UINT32(2000, transportBackoffMs(0));
}

void test_backoff_doubles_each_attempt() {
    TEST_ASSERT_EQUAL_UINT32(4000, transportBackoffMs(1));
    TEST_ASSERT_EQUAL_UINT32(8000, transportBackoffMs(2));
    TEST_ASSERT_EQUAL_UINT32(16000, transportBackoffMs(3));
}

/// attempt=4 는 그대로 두면 32000이지만 30초에서 잘린다.
void test_backoff_caps_at_thirty_seconds() {
    TEST_ASSERT_EQUAL_UINT32(30000, transportBackoffMs(4));
}

/// 큰 attempt 값에서도 오버플로 없이 그대로 캡에 머문다 — 맥이 오래
/// 꺼져 있어 재시도 횟수가 계속 쌓이는 상황을 흉내낸다.
void test_backoff_stays_capped_for_large_attempts() {
    TEST_ASSERT_EQUAL_UINT32(30000, transportBackoffMs(5));
    TEST_ASSERT_EQUAL_UINT32(30000, transportBackoffMs(1000));
}

// ── transportPickHost ───────────────────────────────────────────────────────

void test_mdns_result_wins_over_stored_host() {
    String host = transportPickHost(true, "192.168.1.50", "192.168.1.99");
    TEST_ASSERT_EQUAL_STRING("192.168.1.50", host.c_str());
}

/// mDNS 가 막힌 망 — 저장된 IP로 물러난다(브리프의 확인 항목 2번째 줄).
void test_stored_host_used_when_mdns_finds_nothing() {
    String host = transportPickHost(false, "", "192.168.1.99");
    TEST_ASSERT_EQUAL_STRING("192.168.1.99", host.c_str());
}

/// mDNS 를 켰다는 응답은 왔지만(true) 빈 IP 를 들고 온 비정상 값도
/// "못 찾았다"와 같게 다룬다 — 실제로 `MDNS.queryService()` 가 이 조합을
/// 낼 수는 없지만(개수>0이면 `IP(0)`이 채워진다), 방어적으로 저장된 IP로
/// 물러나는 것이 빈 문자열로 연결을 시도하는 것보다 안전하다.
void test_empty_mdns_host_falls_back_even_if_flagged_found() {
    String host = transportPickHost(true, "", "192.168.1.99");
    TEST_ASSERT_EQUAL_STRING("192.168.1.99", host.c_str());
}

/// 둘 다 없다 — 맥 주소 칸을 비워 둔 사람이 mDNS 가 막힌 망에 있는 경우.
/// 빈 문자열은 "이번 판은 시도할 게 없다"는 신호다(Task 14 전까지 수동
/// 입력 화면이 없으므로 호출자는 다음 백오프까지 기다린다).
void test_no_candidate_returns_empty_string() {
    String host = transportPickHost(false, "", "");
    TEST_ASSERT_EQUAL_STRING("", host.c_str());
}

// ── transportReconnectDecision ──────────────────────────────────────────────

/// 사람이 새 코드를 넣어야 풀리는 두 종결 상태 — 재시도하면 같은 거절만
/// 반복해 맥의 AUTH_DEADLINE 자리를 헛되이 쓴다.
void test_needs_pairing_holds() {
    TEST_ASSERT_EQUAL(ReconnectDecision::Hold, transportReconnectDecision(AuthStep::NeedsPairing));
}

void test_failed_holds() {
    TEST_ASSERT_EQUAL(ReconnectDecision::Hold, transportReconnectDecision(AuthStep::Failed));
}

/// 그 외 값 — 핸드셰이크 도중(다음 동사를 보내야 하는 상태)이거나 이미
/// 인가됐다가 링크만 끊긴 경우 — 는 전부 재시도한다.
void test_in_progress_steps_retry() {
    TEST_ASSERT_EQUAL(ReconnectDecision::Retry, transportReconnectDecision(AuthStep::SendHello2));
    TEST_ASSERT_EQUAL(ReconnectDecision::Retry, transportReconnectDecision(AuthStep::SendAuth2));
    TEST_ASSERT_EQUAL(ReconnectDecision::Retry, transportReconnectDecision(AuthStep::SendCode2));
    TEST_ASSERT_EQUAL(ReconnectDecision::Retry, transportReconnectDecision(AuthStep::SendProof2));
}

void test_subscribed_link_drop_retries() {
    TEST_ASSERT_EQUAL(ReconnectDecision::Retry, transportReconnectDecision(AuthStep::Subscribed));
}

void setup() {
    delay(2000);  // 시리얼 모니터가 붙을 시간을 준다.
    UNITY_BEGIN();
    RUN_TEST(test_backoff_starts_at_two_seconds);
    RUN_TEST(test_backoff_doubles_each_attempt);
    RUN_TEST(test_backoff_caps_at_thirty_seconds);
    RUN_TEST(test_backoff_stays_capped_for_large_attempts);
    RUN_TEST(test_mdns_result_wins_over_stored_host);
    RUN_TEST(test_stored_host_used_when_mdns_finds_nothing);
    RUN_TEST(test_empty_mdns_host_falls_back_even_if_flagged_found);
    RUN_TEST(test_no_candidate_returns_empty_string);
    RUN_TEST(test_needs_pairing_holds);
    RUN_TEST(test_failed_holds);
    RUN_TEST(test_in_progress_steps_retry);
    RUN_TEST(test_subscribed_link_drop_retries);
    UNITY_END();
}

void loop() {}
