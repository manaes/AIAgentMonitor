// AI Agent Monitor — CYD 펌웨어: Transport 가 쓰는 순수 결정 함수들.
//
// Task 12. `authfsm`/`cryptov2` 와 같은 이유로 `lib/` 에 둔다: 소켓도 mDNS 도
// 모르는 순수 함수라 `pio test -e cyd` 가 `src/main.cpp` 를 함께 링크하지
// 않는 이 위치에서 하드웨어 없이 확인할 수 있다(근거는
// `lib/cryptov2/cryptov2.cpp` 상단 주석의 실측 — 테스트 빌드가 `src/` 의
// `setup()`/`loop()` 를 테스트 파일 자신의 것과 함께 링크해 이중 정의로
// 실패한다). `Transport` 본체(소켓·MDNS·WebSocketsClient)는 그 반대로 순수할
// 수 없는 통합 코드라 브리프가 지정한 `src/transport.*` 에 남는다.

#pragma once

#include <Arduino.h>
#include "authfsm.h"

/// 재연결 백오프 간격(ms). `attempt` 는 0부터 — "몇 번째로 실패한 뒤 기다리는
/// 중인가"다. `millis()` 로만 비교해서 쓴다 — `main.cpp` 상단의 30초 예산
/// 주석이 못박은 규칙대로 `delay()` 로 이 간격을 만들지 않는다.
///
/// 2초에서 시작해 두 배씩 늘어나 30초에서 멈춘다(attempt 0..4 → 2·4·8·16·30초,
/// 그 뒤로는 계속 30초). 처음 몇 번은 빨리 재시도해 순간적인 WiFi 끊김이나
/// 맥 재시작을 놓치지 않고, 그래도 안 되면 30초 간격으로 물러나 mDNS 와
/// 재연결 시도가 망에 계속 부담을 주지 않게 한다.
uint32_t transportBackoffMs(uint32_t attempt);

/// mDNS 조회 결과와 저장된 IP 로 이번 판에 시도할 호스트를 고른다.
///
/// 우선순위는 브리프의 `discoverMac()` 스케치와 같다 — **mDNS → 저장된 IP**.
/// 순수 함수라 소켓도 `MDNS.queryService()` 도 모른다 — 그 조회를 실제로
/// 실행하고 결과를 `mdnsFound`/`mdnsHost` 로 넘기는 것은 `Transport` 의 몫이다.
///
/// 반환값이 빈 문자열이면 이번 판은 시도할 대상이 없다는 뜻이다 — mDNS 도
/// 못 찾았고 저장된 IP 도 비어 있다(맥 주소 칸을 비워 둔 사람이 mDNS 가
/// 막힌 망에 있는 경우). 수동 입력 화면은 아직 없으므로(Task 14) 호출자는
/// 그냥 다음 백오프까지 기다린다.
String transportPickHost(bool mdnsFound, const String &mdnsHost, const String &storedHost);

/// 인증 시도 하나가 끝난 뒤 재연결을 계속할지 멈출지.
enum class ReconnectDecision {
    /// 백오프 뒤 다시 시도한다 — 아직 핸드셰이크 중이거나 링크가 그냥 끊긴
    /// 경우다.
    Retry,
    /// 재시도하지 않는다 — 브리프 규칙: `NeedsPairing`/`Failed` 는 사람이
    /// (Task 14 화면에서) 새 코드를 넣어야 풀리므로, 지금 있는 토큰으로
    /// 계속 다시 붙어 봐야 같은 거절만 반복해 맥의 `AUTH_DEADLINE` 자리를
    /// 헛되이 쓴다.
    Hold,
};

/// `authStep` 하나만 보고 정한다 — 소켓도 타이머도 모른다. `authfsm` 이
/// 이미 `NeedsPairing`/`Failed` 를 "핸드셰이크를 포기하고 사람을 기다린다"
/// 는 뜻으로 정의해 뒀으므로(그 두 값의 doc 참고), 여기서 다시 판단할 것은
/// 없다 — 그 결론을 재연결 루프가 쓸 수 있는 모양으로 옮기는 것뿐이다.
ReconnectDecision transportReconnectDecision(AuthStep authStep);
