// AI Agent Monitor — CYD 펌웨어: Dash 화면(B안)의 행 배치·시간 포맷. LVGL 을 모르는 순수 함수라
// 호스트/보드 양쪽에서 Unity 로 검증한다. 좌표 값은 docs/design(CYDDevCenter)의 승인된 B안을 따른다.
#pragma once

#include <Arduino.h>
#include "snapshot.h"

/// 화면에 놓을 job 행 하나. `jobIndex` 는 입력 배열의 인덱스, `y`/`h` 는 1x 픽셀.
struct DashRow {
    uint8_t jobIndex;
    int16_t y;
    uint8_t h;
    bool failEmphasis;  // 실패 강조 행(풀폭 빨강 바). 실패가 여럿이어도 첫 번째만 true.
};

/// 최대 4행. 시작 y=80, 행 간격 3. 실패 job 이 있으면 첫 실패 행만 h=54(강조), 나머지 h=30.
/// 실패가 없으면 전부 h=36. 마지막 행 끝은 항상 233 이하(화면 240 안).
/// 반환값은 채운 행 수(min(count,4)).
size_t dashLayoutRows(const SnapshotJob *jobs, size_t count, DashRow out[SNAPSHOT_MAX_JOBS]);

/// "3:42" / "1:02:03". 버퍼가 작으면 잘라도 항상 NUL 종료. 반환은 쓴 길이.
size_t dashFormatElapsed(uint32_t sec, char *buf, size_t n);
/// "45s ago" / "8m ago" / "2h ago" / "3d ago".
size_t dashFormatAgo(uint32_t sec, char *buf, size_t n);
/// 리셋까지 남은 시간: "2d 4h" / "5h 12m" / "42m" / 이미 지났으면 "now".
size_t dashFormatReset(uint64_t resetEpoch, uint64_t nowEpoch, char *buf, size_t n);

/// tok/s 를 짧게: 99.9 미만 "18.4", 1000 미만 "123", 이후 "1.2k" "12k" "123k" "1.5M". 음수/NaN 은 0.0. 최대 4글자라 "123k tok/s" 가 10글자 안에 든다.
size_t dashFormatRate(float tokPerSec, char *buf, size_t n);

/// CYD 는 RTC 가 없다. "지금" = 스냅샷 `t` + 수신 후 경과(millis 랩어라운드 안전).
uint64_t dashNowEpoch(uint64_t snapshotT, uint32_t rxMillis, uint32_t nowMillis);
/// 경과 초. 시작 시각이 지금보다 미래(시계 불일치)여도 0 으로 clamp.
uint32_t dashElapsedSec(uint64_t nowEpoch, uint64_t startEpoch);

/// 소진된 한도가 있으면 두 한도가 모두 풀릴 때까지 대기한다.
/// 어느 소진 한도든 리셋 시각이 없으면 남은 시간을 추측하지 않는다.
struct DashQuotaWait {
    bool fiveHour = false;
    bool weekly = false;
    bool hasReset = false;
    uint64_t resetEpochSec = 0;
};
DashQuotaWait dashQuotaWait(const SnapshotAgent &agent);
