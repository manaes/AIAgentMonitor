#include "transportlogic.h"

uint32_t transportBackoffMs(uint32_t attempt) {
    constexpr uint32_t BASE_MS = 2000;
    constexpr uint32_t CAP_MS = 30000;
    constexpr uint32_t CAP_AT_ATTEMPT = 4;  // 2000 << 4 = 32000 > CAP_MS

    if (attempt >= CAP_AT_ATTEMPT) {
        return CAP_MS;
    }
    return BASE_MS << attempt;
}

String transportPickHost(bool mdnsFound, const String &mdnsHost, const String &storedHost) {
    if (mdnsFound && mdnsHost.length() > 0) {
        return mdnsHost;
    }
    return storedHost;  // 비어 있을 수 있다 — 호출자가 "이번 판은 없음"으로 읽는다.
}

ReconnectDecision transportReconnectDecision(AuthStep authStep) {
    if (authStep == AuthStep::NeedsPairing || authStep == AuthStep::Failed) {
        return ReconnectDecision::Hold;
    }
    return ReconnectDecision::Retry;
}
