// AI Agent Monitor — CYD 펌웨어: dashlogic(Dash 화면의 행 배치·시간 포맷) 순수 로직 테스트.
#include <unity.h>
#include <Arduino.h>
#include "dashlogic.h"

static SnapshotJob job(SnapshotJobState s) {
    SnapshotJob j;
    j.state = s;
    return j;
}

void test_layout_with_one_failure() {
    SnapshotJob js[4] = {job(SnapshotJobState::Running), job(SnapshotJobState::Success),
                         job(SnapshotJobState::Failed), job(SnapshotJobState::Success)};
    DashRow r[4];
    TEST_ASSERT_EQUAL_size_t(4, dashLayoutRows(js, 4, r));
    const int16_t y[4] = {80, 113, 146, 203};
    const uint8_t h[4] = {30, 30, 54, 30};
    for (int i = 0; i < 4; i++) {
        TEST_ASSERT_EQUAL_INT16(y[i], r[i].y);
        TEST_ASSERT_EQUAL_UINT8(h[i], r[i].h);
        TEST_ASSERT_EQUAL_UINT8(i, r[i].jobIndex);
        TEST_ASSERT_EQUAL(i == 2, r[i].failEmphasis);
    }
    TEST_ASSERT_EQUAL_INT(233, r[3].y + r[3].h);
}

void test_layout_no_failure_uniform_36() {
    SnapshotJob js[4] = {job(SnapshotJobState::Running), job(SnapshotJobState::Success),
                         job(SnapshotJobState::Success), job(SnapshotJobState::Success)};
    DashRow r[4];
    TEST_ASSERT_EQUAL_size_t(4, dashLayoutRows(js, 4, r));
    const int16_t y[4] = {80, 119, 158, 197};
    for (int i = 0; i < 4; i++) {
        TEST_ASSERT_EQUAL_INT16(y[i], r[i].y);
        TEST_ASSERT_EQUAL_UINT8(36, r[i].h);
        TEST_ASSERT_FALSE(r[i].failEmphasis);
    }
}

void test_layout_two_failures_only_first_emphasized_and_fits() {
    SnapshotJob js[4] = {job(SnapshotJobState::Failed), job(SnapshotJobState::Failed),
                         job(SnapshotJobState::Success), job(SnapshotJobState::Failed)};
    DashRow r[4];
    TEST_ASSERT_EQUAL_size_t(4, dashLayoutRows(js, 4, r));
    TEST_ASSERT_TRUE(r[0].failEmphasis);
    for (int i = 1; i < 4; i++) {
        TEST_ASSERT_FALSE(r[i].failEmphasis);
        TEST_ASSERT_TRUE(r[i].y >= r[i - 1].y + r[i - 1].h);  // 겹침 없음
    }
    TEST_ASSERT_TRUE(r[3].y + r[3].h <= 236);
}

void test_layout_zero_and_five_jobs() {
    DashRow r[4];
    SnapshotJob js[6];
    TEST_ASSERT_EQUAL_size_t(0, dashLayoutRows(js, 0, r));
    TEST_ASSERT_EQUAL_size_t(4, dashLayoutRows(js, 6, r));
}

void test_format_elapsed() {
    char b[16];
    dashFormatElapsed(222, b, sizeof(b));
    TEST_ASSERT_EQUAL_STRING("3:42", b);
    dashFormatElapsed(3723, b, sizeof(b));
    TEST_ASSERT_EQUAL_STRING("1:02:03", b);
    dashFormatElapsed(0, b, sizeof(b));
    TEST_ASSERT_EQUAL_STRING("0:00", b);
}

void test_format_ago() {
    char b[16];
    dashFormatAgo(45, b, sizeof(b));
    TEST_ASSERT_EQUAL_STRING("45s ago", b);
    dashFormatAgo(480, b, sizeof(b));
    TEST_ASSERT_EQUAL_STRING("8m ago", b);
    dashFormatAgo(7300, b, sizeof(b));
    TEST_ASSERT_EQUAL_STRING("2h ago", b);
    dashFormatAgo(3 * 86400 + 5, b, sizeof(b));
    TEST_ASSERT_EQUAL_STRING("3d ago", b);
}

void test_format_reset() {
    char b[16];
    dashFormatReset(1000 + 2 * 86400 + 4 * 3600 + 59, 1000, b, sizeof(b));
    TEST_ASSERT_EQUAL_STRING("2d 4h", b);
    dashFormatReset(1000 + 5 * 3600 + 12 * 60, 1000, b, sizeof(b));
    TEST_ASSERT_EQUAL_STRING("5h 12m", b);
    dashFormatReset(1000 + 42 * 60, 1000, b, sizeof(b));
    TEST_ASSERT_EQUAL_STRING("42m", b);
    dashFormatReset(900, 1000, b, sizeof(b));
    TEST_ASSERT_EQUAL_STRING("now", b);
}

void test_format_truncates_to_buffer() {
    char b[4];
    dashFormatAgo(480, b, sizeof(b));  // "8m ago" 는 4바이트에 안 들어간다 → 잘려도 NUL 종료
    TEST_ASSERT_EQUAL_size_t(3, strlen(b));
}

void test_now_epoch_wraps_and_elapsed_clamps() {
    // millis() 가 0xFFFFFC18(랩 1초 전)에서 0x3E8 로 넘어간 경우: 경과 2000ms → +2초
    TEST_ASSERT_EQUAL_UINT64(102, dashNowEpoch(100, 0xFFFFFC18u, 0x3E8u));
    TEST_ASSERT_EQUAL_UINT64(100, dashNowEpoch(100, 5000u, 5999u));
    // 시작 시각이 지금보다 미래(시계 불일치)여도 음수가 되지 않는다
    TEST_ASSERT_EQUAL_UINT32(0, dashElapsedSec(100, 105));
    TEST_ASSERT_EQUAL_UINT32(60, dashElapsedSec(160, 100));
}

void test_format_rate() {
    char b[16];
    struct { float v; const char *want; } c[] = {
        {0.0f, "0.0"}, {18.4f, "18.4"}, {99.94f, "99.9"}, {99.96f, "100"}, {123.4f, "123"},
        {999.4f, "999"}, {999.6f, "1.0k"}, {1234.0f, "1.2k"}, {12345.0f, "12k"},
        {123456.0f, "123k"}, {1500000.0f, "1.5M"}, {-5.0f, "0.0"},
    };
    for (auto &t : c) {
        dashFormatRate(t.v, b, sizeof(b));
        TEST_ASSERT_EQUAL_STRING(t.want, b);
    }
}

void test_quota_wait_five_hour_with_weekly_remaining() {
    SnapshotAgent a;
    a.has5hUsagePct = a.has5hResetAt = a.hasWeeklyUsagePct = true;
    a.usage5hPct = 100;
    a.usageWeeklyPct = 54;
    a.reset5hEpochSec = 6000;
    auto w = dashQuotaWait(a);
    TEST_ASSERT_TRUE(w.fiveHour);
    TEST_ASSERT_FALSE(w.weekly);
    TEST_ASSERT_TRUE(w.hasReset);
    TEST_ASSERT_EQUAL_UINT64(6000, w.resetEpochSec);
    // 리셋 시각이 지나도 새 사용률을 받기 전에는 소진 상태를 유지한다.
    TEST_ASSERT_TRUE(dashQuotaWait(a).fiveHour);
    a.usage5hPct = 0;
    TEST_ASSERT_FALSE(dashQuotaWait(a).fiveHour);
}

void test_quota_wait_both_limits_use_later_reset() {
    SnapshotAgent a;
    a.has5hUsagePct = a.hasWeeklyUsagePct = a.has5hResetAt = a.hasWeeklyResetAt = true;
    a.usage5hPct = a.usageWeeklyPct = 100;
    a.reset5hEpochSec = 6000;
    a.resetWeeklyEpochSec = 9000;
    auto w = dashQuotaWait(a);
    TEST_ASSERT_TRUE(w.fiveHour && w.weekly && w.hasReset);
    TEST_ASSERT_EQUAL_UINT64(9000, w.resetEpochSec);
    a.reset5hEpochSec = 12000;
    TEST_ASSERT_EQUAL_UINT64(12000, dashQuotaWait(a).resetEpochSec);
    a.usage5hPct = 54;
    w = dashQuotaWait(a);
    TEST_ASSERT_FALSE(w.fiveHour);
    TEST_ASSERT_TRUE(w.weekly);
    TEST_ASSERT_EQUAL_UINT64(9000, w.resetEpochSec);
}

void test_quota_wait_missing_reset_error_and_below_limit() {
    SnapshotAgent a;
    TEST_ASSERT_FALSE(dashQuotaWait(a).fiveHour);
    a.has5hUsagePct = true;
    a.usage5hPct = 99.9f;  // 반올림해 100으로 보이더라도 소진으로 판단하지 않는다.
    TEST_ASSERT_FALSE(dashQuotaWait(a).fiveHour);
    a.usage5hPct = 100;
    TEST_ASSERT_TRUE(dashQuotaWait(a).fiveHour);
    TEST_ASSERT_FALSE(dashQuotaWait(a).hasReset);
    a.has5hResetAt = true;
    TEST_ASSERT_FALSE(dashQuotaWait(a).hasReset);  // epoch 0도 알 수 없는 시각이다.
    a.reset5hEpochSec = 6000;
    TEST_ASSERT_TRUE(dashQuotaWait(a).hasReset);
    a.hasWeeklyUsagePct = true;
    a.usageWeeklyPct = 100;
    TEST_ASSERT_FALSE(dashQuotaWait(a).hasReset);  // 두 한도 중 하나만 알아도 추측하지 않는다.
    a.hasQuotaError = true;
    auto w = dashQuotaWait(a);
    TEST_ASSERT_FALSE(w.fiveHour || w.weekly);
}

void setup() {
    delay(2000);
    UNITY_BEGIN();
    RUN_TEST(test_layout_with_one_failure);
    RUN_TEST(test_layout_no_failure_uniform_36);
    RUN_TEST(test_layout_two_failures_only_first_emphasized_and_fits);
    RUN_TEST(test_layout_zero_and_five_jobs);
    RUN_TEST(test_format_elapsed);
    RUN_TEST(test_format_ago);
    RUN_TEST(test_format_reset);
    RUN_TEST(test_format_truncates_to_buffer);
    RUN_TEST(test_format_rate);
    RUN_TEST(test_now_epoch_wraps_and_elapsed_clamps);
    RUN_TEST(test_quota_wait_five_hour_with_weekly_remaining);
    RUN_TEST(test_quota_wait_both_limits_use_later_reset);
    RUN_TEST(test_quota_wait_missing_reset_error_and_below_limit);
    UNITY_END();
}

void loop() {}
