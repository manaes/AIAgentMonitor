// AI Agent Monitor — CYD 펌웨어: WebSocket 연결과 mDNS 발견 (Task 12).
//
// 이 파일이 여는 것: 이 프로젝트에서 처음으로 실제 소켓을 잡고, 처음으로
// `loop()` 한 바퀴 안에서 블로킹 호출을 실행하는 코드다. `main.cpp` 상단의
// 30초 예산(WebSocket Ping/Pong, `src-tauri/src/lan/server.rs` 의
// `PING_INTERVAL`·`IDLE_TIMEOUT`)이 실제로 깨질 수 있는 첫 자리이기도 하다.
//
// ── 예산 안에서 실제로 블로킹하는 자리들(실측) ──────────────────────────────
//
// 1. `MDNS.queryService("aim", "tcp")` — 최대 3000ms.
//    `.pio/libdeps/cyd/../framework-arduinoespressif32/libraries/ESPmDNS/
//    src/ESPmDNS.cpp:216` 이 `mdns_query_ptr(srv, prt, 3000, 20, &results)`
//    를 하드코딩한다. `queryService()` 시그니처 자체에 타임아웃 인자가 없다
//    (`ESPmDNS.h:101`) — 이 3000ms 는 우리가 조절할 수 없는, 라이브러리가
//    정한 상수다.
//
// 2. `WebSocketsClient::loop()` 의 최초 `tcp->connect()` — 최대 5000ms.
//    `.pio/libdeps/cyd/WebSockets/src/WebSocketsClient.cpp:331` 이
//    `_client.tcp->connect(host, port, WEBSOCKETS_TCP_TIMEOUT)` 를 부르고,
//    `WEBSOCKETS_TCP_TIMEOUT` 은 `WebSockets.h:125` 에서 `5000` 으로
//    고정된다(ESP32 매크로 분기, 다른 값으로 정의된 곳이 코드베이스에 없다
//    — `grep -rn WEBSOCKETS_TCP_TIMEOUT` 로 확인).
//
// 3. 연결된 뒤 프레임을 읽는 `readCb()`(`WebSockets.cpp:596-661`) — **한
//    번에 열려 있는 상한이 아니라 무응답(idle) 상한**이다: 바이트가 조금이라도
//    들어올 때마다 `t = millis()` 로 시계를 다시 감는다(`:644-645`). 즉 상대가
//    5000ms 안에 한 바이트라도 계속 보내는 한 이 호출은 끝나지 않는다 — 이
//    자체가 "블로킹 호출에 상한이 없는" 경우다. **다만 이 태스크가 실제로
//    주고받는 프레임(HELLO2/AUTH2/CODE2/PROOF2 요청, `AwaitingCode2`/
//    `Nonce2`/`Authorized2`/`Denied`/`Rejected` 응답)은 전부 1KB 를 크게
//    밑도는 JSON/hex 한 줄이다(`docs`/`pairing.rs:147-170` 의 실제 포맷).**
//    그 크기의 프레임은 한 TCP 세그먼트로 통째로 오거나 전혀 안 오므로,
//    실질적인 단일 호출 상한은 다시 idle 상한인 5000ms 로 접힌다. 이 여유는
//    **인증 단계에만** 해당한다 — 나중 태스크가 봉인된 스냅샷(최대 64KiB,
//    `server.rs` 의 `MAX_FRAME_BYTES`)을 받기 시작하면 이 문서의 결론을
//    다시 확인해야 한다. `handleClientData()`(`WebSocketsClient.cpp:653`)가
//    `tcp->available() > 0` 일 때만 위 경로에 들어가므로, 아무것도 안 왔을
//    때는 즉시 반환한다 — 대기 없이 통과한다.
//
// `Transport::loop()` 한 바퀴가 실제로 겹칠 수 있는 조합은 (1)+(2) 뿐이다
// (mDNS 조회는 대상을 새로 고를 때만, connect 는 그 직후 같은 판에서) —
// 최악 3000+5000 = 8000ms, 30000ms 예산 안에 22000ms 여유가 남는다. (3)은
// 이미 연결된 이후에만 일어나고 (1)/(2) 와 같은 판에 겹치지 않는다.
//
// `main.cpp` 는 이 위에 하트비트(5000ms 주기 출력)와 `delay(1)` 만 얹으므로
// 이 예산 계산이 그대로 `loop()` 전체의 예산이다.
//
// ── mDNS 는 한 번만 시작한다(T12-B) ──────────────────────────────────────
// `MDNS.begin()` 은 `mdns_init()`(`tools/sdk/esp32/include/mdns/include/
// mdns.h:107`)을 부른다. 이 esp-idf mDNS 컴포넌트는 이 프레임워크 패키지에
// **미리 컴파일된 정적 라이브러리**(`libmdns.a`)로만 들어 있어 소스가 없다
// — 두 번 부르는 것이 안전한지(재초기화인지, 리소스가 새는지) 헤더 주석도
// 밝히지 않고, 바이너리에서 뽑은 문자열로도 확인되지 않는다("확인했다"고
// 적을 근거가 없다는 뜻이다). 그래서 안전 여부를 추정하는 대신 **아예 두 번
// 부르지 않는다** — `begin()` 에서 딱 한 번만 호출하고, 재발견이 필요하면
// `MDNS.queryService()` 만 다시 부른다(이 함수는 몇 번을 불러도 되게
// 설계돼 있다 — 매번 이전 결과를 free 하고 새로 채운다, `ESPmDNS.cpp:198`).
//
// ── AuthStep 을 그대로 드러낸다(T12-C) ────────────────────────────────────
// 브리프의 `bool authorized()` 만으로는 `NeedsPairing`(정상 거절 — 코드
// 다시 입력)과 `Failed`(이 v2-only 펌웨어가 이해 못 하는 응답 — 사람이
// 봐야 한다)를 가릴 수 없다. Task 14(페어링 키패드)는 "연결 중" · "코드
// 필요" · "실패" · "인가됨" 을 다르게 그려야 하므로 `authStep()` 을 더한다.
// `authorized()` 는 그 위에 얹은 얇은 편의 함수로 남긴다(브리프 시그니처와
// 호환).
//
// ── T12-D: "깜빡이지 않는다"는 이 태스크에서 확인할 수 없다 ─────────────
// 브리프 Step 2 의 마지막 항목("맥에서 기기를 해제하면 키패드 화면으로
// 가고 깜빡이지 않는다")은 화면이 있어야 관찰 가능한 성질이다. 디스플레이는
// Task 13~14 이전에는 존재하지 않는다 — 이 파일은 그 항목을 "된다"고
// 주장하지 않는다. 시리얼로 확인 가능한 나머지 셋(mDNS 발견·저장된 IP
// 대체·백오프 재연결)만 이 태스크의 검증 대상이다.
//
// ── Task 14b 가 더한 것: submitCode() 진입점과 HELLO2/CODE2 흐름 ─────────
// Task 12 은 `hasCode` 를 항상 false 로 두고 AUTH2/PROOF2(재연결) 흐름만
// 배선했다 — 그때는 키패드 UI 가 없어서 "사람이 방금 6자리를 입력했다"를
// 전달할 방법 자체가 없었다. `submitCode()` 가 그 진입점이다: 페어링
// 키패드(Task 14b, `ui_pairing.cpp`)가 확인 버튼을 누르면 이 함수를 부르고,
// 그 뒤로는 이 파일이 `authInitialStep`/`authOnReply`(둘 다 `lib/authfsm`
// 의 순수 함수, 이 파일을 수정할 필요가 없었다 — 두 함수 모두 애초에
// hasCode 를 매개변수로 받게 설계돼 있었다)의 결정을 그대로 따라 HELLO2 →
// CODE2 를 보낸다. "코드가 입력됐을 때 다음에 무엇을 보낼지" 를 결정하는
// 로직은 여전히 `lib/authfsm` 안에만 있다 — 이 파일은 그 결정(SendHello2/
// SendCode2)을 받아 실제로 소켓에 무엇을 쓸지만 안다.

#pragma once

#include <Arduino.h>
#include <WebSocketsClient.h>

#include "authfsm.h"
#include "config.h"
#include "cryptov2.h"
#include "snapshot.h"

class Transport {
  public:
    Transport();
    ~Transport();

    /// 한 번만 부른다 — 보통 `setup()` 에서, WiFi 연결 뒤. `MDNS.begin()`
    /// 도 여기서 딱 한 번 불린다(위 T12-B).
    void begin(Config &config);

    /// 매 `loop()` 마다 부른다. 한 바퀴가 실제로 블로킹할 수 있는 최댓값과
    /// 근거는 이 파일 상단의 예산 분석을 보라.
    void loop();

    /// 세션 키가 서고 스냅샷을 구독할 수 있는 상태인가. `authStep() ==
    /// AuthStep::Subscribed` 의 얇은 편의 함수 — 브리프가 정한 시그니처를
    /// 유지하면서, Task 14 가 실제로 필요로 하는 정밀도는 `authStep()` 이
    /// 준다.
    bool authorized() const;

    /// 지금 인증 핸드셰이크의 상태. 아직 소켓조차 없을 때는 "다음에 연결되면
    /// 보낼 동사"를 미리 담아 둔다(`SendAuth2` 또는 `NeedsPairing`) — 아직
    /// 아무것도 보내지 않았다는 뜻이지 이미 보냈다는 뜻이 아니다. 소켓이 붙어
    /// 실제로 그 동사를 보내고 나면 응답에 따라 다음 값으로 넘어간다.
    AuthStep authStep() const { return authStep_; }

    /// 페어링 키패드(Task 14b, `ui_pairing.cpp`)의 확인 버튼이 부른다 —
    /// "사람이 방금 6자리를 입력했다" 를 전달하는 유일한 통로다. Task 12
    /// 는 이 진입점을 몰랐다: 그때는 키패드 UI 가 없어서 `hasCode` 가
    /// 코드 전체에서 항상 false 였다.
    ///
    /// 코드를 들고 있다가 다음(또는 지금 붙어 있는 연결을 끊고 새로)
    /// `loop()` 에서 HELLO2 부터 새로 시작한다 — 저장된 토큰이 있어도
    /// 코드가 우선한다(`authfsm.h` 의 `authInitialStep` 문서와 같은 규칙,
    /// 이 함수는 그 규칙을 다시 적지 않고 그대로 위임한다). 코드 자체는
    /// `Denied`/`Rejected`(→ `NeedsPairing`) 또는 성공(→ `Subscribed`) 으로
    /// 결론이 날 때까지 들고 있는다 — 그 전에 연결이 그냥 끊기기만 했다면
    /// (예: WiFi 순단) 같은 코드로 다음 재시도 때 자동으로 다시 HELLO2 를
    /// 보낸다. 사람이 다시 타이핑하게 만들 이유가 없다.
    void submitCode(const String &code);

    /// 마지막으로 실제 와이어에서 받은 `Denied.left` 값. 첫 시도 전에는
    /// 맥의 실제 상수(`MAX_ATTEMPTS`, `src-tauri/src/ble/pairing.rs:47`,
    /// 2026-08-27 그 커밋 기준 확인)와 같은 값 5 로 초기화해 둔다 —
    /// 와이어에 없는 값을 추측하는 게 아니라, 첫 시도 전에는 실제로 5회가
    /// 맞기 때문이다(브리프의 "remaining time and remaining attempts"
    /// 요구를 놓고 T14b-A 가 내린 판단 — `AwaitingCode2` 는 이 값을 전혀
    /// 싣지 않으므로 첫 화면에서는 이 초기값 말고 얻을 방법이 없다).
    uint8_t attemptsLeft() const { return attemptsLeft_; }

    /// 최근에 성공적으로 복호화·파싱된 스냅샷을 받은 적이 있는가. `false`
    /// 면 `latestSnapshot()` 은 아직 기본값(전부 0/빈 값)이다 — Task 15b
    /// 는 이 값을 "카드에 무엇을 그릴지" 를 정하기 전에 먼저 물어야 한다.
    bool hasSnapshot() const { return hasSnapshot_; }

    /// 가장 최근에 받은 스냅샷. `hasSnapshot()` 이 `false` 인 동안은
    /// 유효하지 않다(기본 생성값일 뿐이다).
    const Snapshot &latestSnapshot() const { return latestSnapshot_; }

  private:
    Config *config_ = nullptr;
    WebSocketsClient webSocket_;

    AuthStep authStep_ = AuthStep::NeedsPairing;

    /// `NeedsPairing`/`Failed` 로 확정된 뒤 재연결을 완전히 멈춘 상태
    /// (`transportlogic.h` 의 `ReconnectDecision::Hold`). "아직 페어링을
    /// 한 번도 안 한 상태"(토큰이 아예 없음)와는 다르다 — 그건 매 `loop()`
    /// 마다 다시 확인하지, 여기서 래치하지 않는다(토큰이 나중에 채워지면
    /// 다음 `loop()` 에서 바로 다시 시도해야 하므로). 자세한 구분은
    /// `transport.cpp` 의 `loop()` 주석.
    bool holding_ = false;

    bool mdnsStarted_ = false;

    /// 이번 판에서 시도 중인 호스트. 비어 있으면 "새로 골라야 한다".
    String targetHost_;
    uint32_t connectStartedAtMs_ = 0;

    uint32_t backoffAttempt_ = 0;
    uint32_t nextAttemptAtMs_ = 0;

    /// 이번 연결의 임시 X25519 키쌍. 연결마다 새로 만든다 — transcript(스펙
    /// §4)가 이 키쌍에 묶이므로 재사용하면 재연결마다 같은 transcript 가
    /// 나와 재생 저항이 약해진다.
    uint8_t mySecret_[32] = {0};
    uint8_t myPublic_[32] = {0};

    /// `SendProof2` 단계에서 미리 유도해 둔 세션 키. `Authorized2` 가
    /// 오면 이 값으로 `SealedChannel` 을 만든다 — `authOnReply` 는 그
    /// 시점에 이미 사라진 `ss`/논스를 다시 볼 수 없으므로, 여기서
    /// 들고 있어야 한다.
    ///
    /// **Task 15a 조사 결과, 이름의 `pending` 이 실제로 정확하다 — 오해를
    /// 살 이름이 아니다.** Task 15a 브리프는 "`pending` 이라는 이름이
    /// `Subscribed` 상태 내내 살아 있는 세션 키를 페어링 전용 값처럼
    /// 보이게 한다" 고 적었지만, `transport.cpp` 의 `finishHandshake()`
    /// 를 직접 읽어 보면 이 두 배열은 `channel_` 을 만드는 즉시(성공한
    /// AUTH2/PROOF2 흐름이든 HELLO2/CODE2 흐름이든 둘 다) `v2Wipe()` 로
    /// 지워진다 — 정말로 "다음 단계로 넘어갈 때까지만" 사는 값이다.
    /// **`Subscribed` 상태 내내 살아 있는 실제 세션 키 보관소는 아래
    /// `channel_`(`SealedChannel*`) 하나뿐이다** — 봉인 스냅샷을 열 때
    /// (`handleSnapshotFrame()`, Task 15a)도 이 배열이 아니라 `channel_`
    /// 을 그대로 쓴다. 이름을 바꾸지 않고 이 주석으로 남기는 이유:
    /// 실제 문제(session key 가 어디 사는지 헷갈림)는 이름이 아니라
    /// "wipe 되는 시점" 이 코드 흐름 안에 흩어져 있다는 것이었고, 그건
    /// 이 doc 주석 하나로 충분히 설명된다 — 이름을 바꾸면 이미 검증된
    /// 핸드셰이크 코드(`sendProof2`/`finishHandshake`)의 여러 줄을
    /// 건드리게 되는데, 그 변경이 주는 이득이 없다.
    uint8_t pendingS2c_[32] = {0};
    uint8_t pendingC2s_[32] = {0};

    /// 사람이 `submitCode()` 로 넣은 6자리. 비어 있으면 "쓸 코드가 없다".
    /// `Denied`/`Rejected`/`Failed`(= `NeedsPairing`/`Failed` 로 확정)
    /// 또는 성공(`Subscribed`)이 오면 지운다 — 그 전까지는(단순 연결
    /// 끊김) 다음 재시도가 같은 코드로 다시 HELLO2 를 보낼 수 있게 남겨
    /// 둔다.
    String pendingCode_;

    /// `sendCode2()` 에서 유도해 `finishHandshake()` 까지 들고 가는
    /// 공유 비밀·논스. AUTH2/PROOF2 흐름(`pendingS2c_`/`pendingC2s_`)과
    /// 달리 여기서는 세션 키를 미리 만들 수 없다 — 새 토큰 자체가
    /// `Granted2` 응답에서야(sealed 프레임 안에) 처음 생기기 때문에,
    /// 그 응답이 올 때까지 `ss`/논스를 원본 그대로 들고 있어야 한다.
    uint8_t pendingSs_[32] = {0};
    uint8_t pendingNonceBytes_[16] = {0};

    /// `handleReply()` 가 실제 `Denied.left` 를 받을 때만 갱신한다 —
    /// `attemptsLeft()` 문서 참고.
    uint8_t attemptsLeft_ = 5;

    /// `Subscribed` 상태 내내 살아 있는 실제 세션 키 보관소(위 `pendingS2c_`
    /// 문서 참고). `finishHandshake()` 가 성공할 때 `(c2s, s2c)` 순서로
    /// 만든다(`SealedChannel(sendKey, recvKey)` — 이 기기 기준 송신은 c2s,
    /// 수신은 s2c). `handleSnapshotFrame()`(Task 15a)이 맥→CYD 방향
    /// 봉인 프레임을 열 때 이 객체의 `open()` 을 그대로 쓴다 — s2c 키를
    /// 따로 들고 있다가 두 번째 `SealedChannel` 을 새로 만들 필요가 없다
    /// (애초에 `pendingS2c_` 는 이 시점에 이미 지워진 값이라 그럴 수도
    /// 없다).
    SealedChannel *channel_ = nullptr;

    /// `handleSnapshotFrame()` 에서 연속으로 복호화에 실패한 횟수.
    /// 성공하면 0으로 되돌린다. `SNAPSHOT_DECRYPT_FAIL_LIMIT`
    /// (`transport.cpp`)에 닿으면 "이 한 프레임이 아니라 세션 키 자체가
    /// 어긋났다" 는 신호로 보고 소켓을 끊어 재연결(→ AUTH2/PROOF2 재핸드셰이크
    /// → 새 `channel_`)을 강제한다 — 근거는 `handleSnapshotFrame()` 주석.
    uint8_t snapshotDecryptFailStreak_ = 0;

    /// 가장 최근에 성공적으로 복호화·파싱된 스냅샷. `hasSnapshot_` 이
    /// `false` 인 동안은 기본 생성값일 뿐 유효하지 않다.
    Snapshot latestSnapshot_;
    bool hasSnapshot_ = false;

    void onWsEvent(WStype_t type, uint8_t *payload, size_t length);
    void handleSocketConnected();
    void handleSocketDisconnected();
    void handleReply(const String &text);
    void sendCode2(const ReplyView &reply);
    void sendProof2(const ReplyView &reply);
    void finishHandshake(const ReplyView &reply);
    void handleSnapshotFrame(const uint8_t *payload, size_t length);
    void sendVerb(const char *prefix, const uint8_t *data, size_t len);
    void setAuthStep(AuthStep step);
    void scheduleNextAttempt();
    String chooseTarget();
};
