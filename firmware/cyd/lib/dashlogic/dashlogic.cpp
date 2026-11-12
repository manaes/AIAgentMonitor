#include "dashlogic.h"

#include <stdio.h>

size_t dashLayoutRows(const SnapshotJob *jobs, size_t count, DashRow out[SNAPSHOT_MAX_JOBS]) {
    size_t n = count < SNAPSHOT_MAX_JOBS ? count : SNAPSHOT_MAX_JOBS;
    size_t firstFail = n;  // n == 없음
    for (size_t i = 0; i < n; i++) {
        if (jobs[i].state == SnapshotJobState::Failed) { firstFail = i; break; }
    }
    int16_t y = 80;
    for (size_t i = 0; i < n; i++) {
        uint8_t h = (firstFail == n) ? 36 : (i == firstFail ? 54 : 30);
        out[i] = DashRow{(uint8_t)i, y, h, i == firstFail};
        y += h + 3;
    }
    return n;
}

static size_t put(char *buf, size_t n, const char *fmt, unsigned long a, unsigned long b = 0, unsigned long c = 0) {
    if (n == 0) return 0;
    int w = snprintf(buf, n, fmt, a, b, c);
    if (w < 0) { buf[0] = '\0'; return 0; }
    return (size_t)w < n ? (size_t)w : n - 1;
}

size_t dashFormatElapsed(uint32_t sec, char *buf, size_t n) {
    if (sec >= 3600) return put(buf, n, "%lu:%02lu:%02lu", sec / 3600, (sec / 60) % 60, sec % 60);
    return put(buf, n, "%lu:%02lu", sec / 60, sec % 60);
}

size_t dashFormatAgo(uint32_t sec, char *buf, size_t n) {
    if (sec < 60) return put(buf, n, "%lus ago", sec);
    if (sec < 3600) return put(buf, n, "%lum ago", sec / 60);
    if (sec < 86400) return put(buf, n, "%luh ago", sec / 3600);
    return put(buf, n, "%lud ago", sec / 86400);
}

size_t dashFormatReset(uint64_t resetEpoch, uint64_t nowEpoch, char *buf, size_t n) {
    if (resetEpoch <= nowEpoch) return put(buf, n, "now", 0);
    uint64_t d = resetEpoch - nowEpoch;
    if (d >= 86400) return put(buf, n, "%lud %luh", (unsigned long)(d / 86400), (unsigned long)((d % 86400) / 3600));
    if (d >= 3600) return put(buf, n, "%luh %lum", (unsigned long)(d / 3600), (unsigned long)((d % 3600) / 60));
    return put(buf, n, "%lum", (unsigned long)(d / 60));
}

uint64_t dashNowEpoch(uint64_t snapshotT, uint32_t rxMillis, uint32_t nowMillis) {
    return snapshotT + (uint32_t)(nowMillis - rxMillis) / 1000;  // 부호 없는 뺄셈이라 랩어라운드 안전
}

uint32_t dashElapsedSec(uint64_t nowEpoch, uint64_t startEpoch) {
    return nowEpoch > startEpoch ? (uint32_t)(nowEpoch - startEpoch) : 0;
}

size_t dashFormatRate(float v, char *buf, size_t n) {
    if (n == 0) return 0;
    if (!(v > 0.0f)) v = 0.0f;  // 음수와 NaN
    int w;
    if (v < 99.95f) w = snprintf(buf, n, "%.1f", (double)v);
    else if (v < 999.5f) w = snprintf(buf, n, "%.0f", (double)v);
    else if (v < 9950.0f) w = snprintf(buf, n, "%.1fk", (double)(v / 1000.0f));
    else if (v < 999500.0f) w = snprintf(buf, n, "%.0fk", (double)(v / 1000.0f));
    else w = snprintf(buf, n, "%.1fM", (double)(v / 1000000.0f));
    if (w < 0) { buf[0] = '\0'; return 0; }
    return (size_t)w < n ? (size_t)w : n - 1;
}

DashQuotaWait dashQuotaWait(const SnapshotAgent &a) {
    DashQuotaWait w;
    if (a.hasQuotaError) return w;
    w.fiveHour = a.has5hUsagePct && a.usage5hPct >= 100.0f;
    w.weekly = a.hasWeeklyUsagePct && a.usageWeeklyPct >= 100.0f;
    if (!w.fiveHour && !w.weekly) return w;
    w.hasReset = (!w.fiveHour || (a.has5hResetAt && a.reset5hEpochSec > 0)) &&
                 (!w.weekly || (a.hasWeeklyResetAt && a.resetWeeklyEpochSec > 0));
    if (w.fiveHour) w.resetEpochSec = a.reset5hEpochSec;
    if (w.weekly && a.resetWeeklyEpochSec > w.resetEpochSec) w.resetEpochSec = a.resetWeeklyEpochSec;
    return w;
}
