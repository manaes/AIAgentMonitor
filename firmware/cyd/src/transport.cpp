#include "transport.h"

#include <ArduinoJson.h>
#include <ESPmDNS.h>
#include <string.h>

#include "transportlogic.h"

namespace {

/// 맥의 LAN 미러 포트. `src-tauri/src/lan/server.rs:118` 의 `pub const PORT:
/// u16 = 4320` 과 같은 값이어야 하는, 언어를 건너뛰는 계약이다 — 그 파일이
/// `discovery.rs` 의 `SERVICE_TYPE` 에 대해 남긴 것과 같은 종류의 주석.
constexpr uint16_t MAC_PORT = 4320;
constexpr char MAC_PATH[] = "/mirror";

/// `MDNS.begin()` 에 넘기는 이 보드의 mDNS 이름. `foo.local` 의 `foo` 다 —
/// 브리프 스케치가 쓴 이름 그대로.
constexpr char MDNS_HOSTNAME[] = "aim-cyd";

/// 이번 호스트로 연결을 시도하다 포기하는 시각까지의 여유. 단일 `connect()`
/// 상한(`WEBSOCKETS_TCP_TIMEOUT` = 5000ms, `transport.h` 상단 주석 1번)보다
/// 넉넉히 잡아, 그 호출이 실제로 5000ms 를 다 쓰고 실패했을 때도 최소 한 번은
/// 더 볼 기회를 준다.
constexpr uint32_t CONNECT_GIVEUP_MS = 6000;

int hexNibble(char c) {
    if (c >= '0' && c <= '9') return c - '0';
    if (c >= 'a' && c <= 'f') return c - 'a' + 10;
    // 대문자는 거절한다 — `config.cpp` 의 `configTokenIsValid` 와 같은 기준:
    // 맥이 내는 값은 언제나 소문자다.
    return -1;
}

/// 정확히 `outLen` 바이트를 기대하는 hex 디코드. 길이나 문자가 안 맞으면
/// `out` 을 건드리지 않고 false — 잘못된 길이의 값을 부분적으로 채워 넣고
/// 계속 진행하는 것보다, 호출자가 그 자리에서 Failed 로 멈추는 편이 안전하다.
bool hexDecode(const String &hex, uint8_t *out, size_t outLen) {
    if ((size_t)hex.length() != outLen * 2) {
        return false;
    }
    for (size_t i = 0; i < outLen; i++) {
        const int hi = hexNibble(hex[2 * i]);
        const int lo = hexNibble(hex[2 * i + 1]);
        if (hi < 0 || lo < 0) {
            return false;
        }
        out[i] = (uint8_t)((hi << 4) | lo);
    }
    return true;
}

/// 맥의 인증 응답 JSON 텍스트 프레임을 `ReplyView` 로 옮긴다. 필드 이름과
/// 존재 조건은 `authfsm.h` 의 `ReplyView` 문서가 `src-tauri/src/ble/
/// pairing.rs:147-170` 의 `to_json_bytes()` 에서 그대로 옮겨 적은 것이다 —
/// 여기서는 그 다섯 필드만 읽는다. `test/test_authfsm` 의 `parseReply` 는
/// 이 태스크 전에 ArduinoJson 이 없어서 손으로 짠 스캐너였다(그 파일 주석
/// 참고) — 이제 진짜 파서로 교체한다.
///
/// 파싱 자체가 실패하면(JSON 이 아니다) false 를 돌려준다. 이 경우
/// `ReplyView` 의 기본값(`ok=false, v=false`)으로 `authOnReply` 를 부르면
/// "v 없는 거절"(`NeedsPairing`) 로 떨어지는데, 그건 틀린 신호다 — 코드가
/// 나쁜 게 아니라 **이 펌웨어가 응답을 이해하지 못한 것**이므로 호출자는
/// 이 반환값을 보고 `authOnReply` 를 부르지 않고 곧장 `Failed` 로 가야 한다.
bool parseReplyJson(const String &text, ReplyView &out) {
    JsonDocument doc;
    if (deserializeJson(doc, text) != DeserializationError::Ok) {
        return false;
    }
    out.ok = doc["ok"] | false;
    out.v = doc["v"].is<int>() && doc["v"].as<int>() == 2;
    out.await = String((const char *)(doc["await"] | ""));
    out.hasLeft = doc["left"].is<int>();
    out.left = out.hasLeft ? (uint8_t)doc["left"].as<int>() : 0;
    out.epk = String((const char *)(doc["epk"] | ""));
    out.nonce = String((const char *)(doc["nonce"] | ""));
    out.sealed = String((const char *)(doc["sealed"] | ""));
    return true;
}

}  // namespace

Transport::Transport() {}

Transport::~Transport() {
    delete channel_;
}

void Transport::begin(Config &config) {
    config_ = &config;

    // T12-B — 딱 한 번. 이유는 transport.h 상단 주석.
    mdnsStarted_ = MDNS.begin(MDNS_HOSTNAME);
    if (!mdnsStarted_) {
        Serial.println("transport: mDNS 시작 실패 — 저장된 IP로만 맥을 찾는다");
    }

    webSocket_.onEvent([this](WStype_t type, uint8_t *payload, size_t length) {
        onWsEvent(type, payload, length);
    });

    setAuthStep(configIsPaired(*config_) ? AuthStep::SendAuth2 : AuthStep::NeedsPairing);
}

void Transport::loop() {
    if (config_ == nullptr) {
        return;  // begin() 이 안 불렸다.
    }

    // 재시도를 완전히 멈춘 상태 — brief 규칙(NeedsPairing/Failed 는 사람이
    // 새 코드를 넣어야 풀린다). handleReply() 가 이 값을 세운다.
    if (holding_) {
        return;
    }

    // "아직 한 번도 페어링하지 않았다"는 위와 다른 상태다 — 나중에 토큰이
    // 채워지면 바로 다음 loop() 에서 다시 시도해야 하므로 여기서는
    // 래치하지 않고 매번 다시 묻는다. 붙어 봐야 아무것도 보낼 수 없는
    // 연결이라 소켓조차 열지 않는다 — 열면 맥의 8자리 `AUTH_DEADLINE`
    // 자리 하나를 헛되이 쓴다(`server.rs` 의 `MAX_CONNECTIONS`/
    // `AUTH_DEADLINE` 문서).
    //
    // **Task 14b 가 더한 예외**: `pendingCode_` 가 있으면(사람이 방금
    // 키패드에서 확인을 눌렀다) 토큰이 없어도 연결을 시도한다 — HELLO2
    // 자체가 처음 페어링하는 흐름이라 애초에 토큰을 요구하지 않는다.
    if (!configIsPaired(*config_) && pendingCode_.length() == 0) {
        setAuthStep(AuthStep::NeedsPairing);
        return;
    }

    if (webSocket_.isConnected()) {
        webSocket_.loop();
        return;
    }

    const uint32_t now = millis();
    if (now < nextAttemptAtMs_) {
        return;  // 백오프 중 — mDNS 도 connect() 도 시도하지 않는다.
    }

    if (targetHost_.length() == 0) {
        targetHost_ = chooseTarget();  // mDNS 조회 최대 1회(≤3000ms, transport.h 1번).
        if (targetHost_.length() == 0) {
            // mDNS 도 저장된 IP 도 없다 — 이번 판은 포기한다. 수동 입력
            // 화면은 아직 없다(Task 14).
            scheduleNextAttempt();
            return;
        }
        webSocket_.begin(targetHost_.c_str(), MAC_PORT, MAC_PATH);  // 논블로킹 — 상태만 세팅.
        connectStartedAtMs_ = now;
    }

    webSocket_.loop();  // 여기서 최대 1회 connect() 를 시도한다(≤5000ms, transport.h 2번).

    // **실기로 잡은 버그**: raw TCP `connect()` 는 성공했지만 HTTP 업그레이드
    // 응답이 없는 경우, 라이브러리 자신의 헤더 응답 타임아웃(`WebSocketsClient.
    // cpp:646`, `WSC_HEADER`/`WSC_BODY` 상태에서만 적용, 5000ms)이 바로 위
    // `webSocket_.loop()` 호출 **안에서** 이미 `clientDisconnect()` 를 불러
    // `WStype_DISCONNECTED` 를 낸다 — 그 콜백은 같은 스레드에서 `webSocket_.
    // loop()` 가 반환하기 전에 동기적으로 실행되므로, **동시성 경쟁이 아니라
    // 이 한 번의 `Transport::loop()` 호출 안에서 일어나는 결정론적 이중
    // 처리**다: `handleSocketDisconnected()` 가 그 안에서 먼저 돌아
    // `targetHost_` 를 비우고 백오프를 예약해 버린 뒤, 아래 조건이
    // `targetHost_` 를 보지 않고 그대로 쓰면 같은 실패 하나에 백오프가 두 번
    // (예: 2000ms 뒤가 아니라 곧장 4000ms 뒤로) 걸린다 — 처음 이 코드를 그대로
    // 실기에 올렸을 때 시리얼에서 그렇게 찍히는 것을 봤다. `targetHost_.
    // length() > 0` 가드가 "이 판이 아직 마무리되지 않았다"는 것을 보장한다 —
    // 이미 비었다면 handleSocketDisconnected() 가 방금 이 판을 끝낸 것이므로
    // 여기서 또 끝낼 필요가 없다.
    //
    // **이 가드가 죽은 코드가 아닌 이유** — 두 실패 모드가 서로 다른 회수
    // 수단으로 걸린다. raw TCP `connect()` 자체가 실패하는 경로(호스트가
    // 아예 응답하지 않거나 RST 를 준 경우)는 `connectFailedCb()`
    // (`WebSocketsClient.cpp:992-994`)로 가는데, 이 함수는 로그만 찍고
    // `WStype_DISCONNECTED` 를 **내지 않는다** — 그러면
    // `handleSocketDisconnected()` 도 안 불리고 `targetHost_` 도 안 비워진다.
    // 이 경로에서는 위 헤더 타임아웃이 아예 적용되지 않으므로(연결 자체가
    // 안 됐으니 `WSC_HEADER` 에도 못 들어간다), **`CONNECT_GIVEUP_MS` 가
    // 이 판을 회수하는 유일한 수단**이다. 즉 아래 조건은 "연결은 됐는데 응답이
    // 없다"(라이브러리의 헤더 타임아웃이 먼저 처리, 이 가드는 중복 방지용)와
    // "연결 자체가 안 된다"(라이브러리가 회수하지 않음, 이 가드가 유일한
    // 회수 수단) 두 실패 모드를 하나의 판정으로 같이 받는다.
    if (targetHost_.length() > 0 && !webSocket_.isConnected() &&
        (now - connectStartedAtMs_) > CONNECT_GIVEUP_MS) {
        // 이 판은 안 됐다 — 다음 판에서 대상을 다시 고른다(mDNS 결과가
        // 바뀌었을 수 있다. 맥이 DHCP 로 주소를 새로 받은 경우 등).
        webSocket_.disconnect();
        targetHost_ = "";
        scheduleNextAttempt();
    }
}

bool Transport::authorized() const {
    return authStep_ == AuthStep::Subscribed;
}

void Transport::setAuthStep(AuthStep step) {
    if (authStep_ == step) {
        return;
    }
    authStep_ = step;
    Serial.printf("transport: authStep=%d\n", (int)step);
}

String Transport::chooseTarget() {
    bool mdnsFound = false;
    String mdnsHost;

    if (mdnsStarted_) {
        // ESPmDNS.cpp:216 — 최대 3000ms 블로킹(transport.h 1번).
        const int n = MDNS.queryService("aim", "tcp");
        if (n > 0) {
            mdnsHost = MDNS.IP(0).toString();
            mdnsFound = true;
            Serial.printf("transport: mDNS 로 맥을 찾았다 — %s\n", mdnsHost.c_str());
        }
    }

    const String host = transportPickHost(mdnsFound, mdnsHost, config_->macHost);
    if (!mdnsFound && host.length() > 0) {
        Serial.printf("transport: mDNS 실패 — 저장된 IP로 시도한다: %s\n", host.c_str());
    }
    return host;
}

void Transport::scheduleNextAttempt() {
    const uint32_t delayMs = transportBackoffMs(backoffAttempt_);
    nextAttemptAtMs_ = millis() + delayMs;
    Serial.printf("transport: %u번째 재시도 — %ums 뒤\n", backoffAttempt_ + 1, delayMs);
    if (backoffAttempt_ < 4) {
        ++backoffAttempt_;  // transportBackoffMs 가 이미 4 이상을 캡으로 다룬다.
    }
}

void Transport::onWsEvent(WStype_t type, uint8_t *payload, size_t length) {
    switch (type) {
        case WStype_CONNECTED:
            handleSocketConnected();
            break;
        case WStype_DISCONNECTED:
            handleSocketDisconnected();
            break;
        case WStype_TEXT:
            handleReply(String((char *)payload, length));
            break;
        default:
            // ERROR/BIN/PING/PONG/FRAGMENT* — 인증 단계에서는 텍스트 프레임만
            // 의미가 있다. BIN(봉인 스냅샷)은 이 태스크 이후의 몫이다.
            break;
    }
}

void Transport::handleSocketConnected() {
    v2GenerateKeypair(mySecret_, myPublic_);

    // Task 14b 이전에는 `loop()` 가 `configIsPaired()==false` 면 애초에
    // 연결을 시도하지 않아서 hasToken 이 언제나 true, hasCode 는 언제나
    // false 였다. 이제 `pendingCode_`(submitCode()) 가 있으면 토큰 없이도
    // 여기 도달한다 — 그래서 두 값 다 실제 상태를 그대로 묻는다. 판단
    // 자체는 여전히 `authInitialStep` 하나로만 한다.
    const bool hasToken = configIsPaired(*config_);
    const bool hasCode = pendingCode_.length() > 0;
    const AuthStep step = authInitialStep(hasToken, hasCode);
    setAuthStep(step);

    switch (step) {
        case AuthStep::SendHello2:
            sendVerb("HELLO2:", myPublic_, sizeof myPublic_);
            break;
        case AuthStep::SendAuth2:
            sendVerb("AUTH2:", myPublic_, sizeof myPublic_);
            break;
        default:
            // NeedsPairing — hasToken=false, hasCode=false 조합인데,
            // `loop()` 의 위 가드가 이 조합으로는 애초에 소켓을 열지
            // 않는다(둘 다 없으면 시도할 게 없다). 그래도 여기서 강제로
            // 무엇을 보내지는 않는다: 모르는 값으로 소켓에 아무거나 쓰는
            // 것보다, 아무것도 안 보내고 응답 없이 idle 로 남는 편이
            // 안전하다.
            break;
    }
}

void Transport::handleSocketDisconnected() {
    delete channel_;
    channel_ = nullptr;
    targetHost_ = "";  // 다음 판에서 대상을 다시 고른다.

    // authStep_ 이 아직 NeedsPairing/Failed 로 확정되지 않았다면(= 링크가
    // 그냥 끊겼다) 재시도한다. 이미 확정됐다면 handleReply() 가 이미
    // holding_ 을 세웠고 disconnect() 도 그쪽에서 불렀으므로 여기서는 그
    // 결정을 다시 만들지 않는다 — `transportReconnectDecision` 은 그
    // 확정 시점에 한 번만 묻는다.
    if (transportReconnectDecision(authStep_) == ReconnectDecision::Retry) {
        scheduleNextAttempt();
    }
}

void Transport::handleReply(const String &text) {
    ReplyView reply;
    if (!parseReplyJson(text, reply)) {
        Serial.println("transport: 맥 응답을 JSON 으로 읽을 수 없다 — Failed");
        setAuthStep(AuthStep::Failed);
        holding_ = true;
        webSocket_.disconnect();
        return;
    }

    // Denied 응답에만 실제로 실린다(ReplyView 문서) — 그 외 응답은 이
    // 필드가 없으므로 마지막으로 본 값을 그대로 유지한다. `attemptsLeft()`
    // 문서 참고.
    if (reply.hasLeft) {
        attemptsLeft_ = reply.left;
    }

    const bool hasToken = configIsPaired(*config_);
    const bool hasCode = pendingCode_.length() > 0;
    const AuthStep step = authOnReply(reply, hasToken, hasCode);
    setAuthStep(step);

    switch (step) {
        case AuthStep::SendCode2:
            sendCode2(reply);
            break;
        case AuthStep::SendProof2:
            sendProof2(reply);
            break;
        case AuthStep::Subscribed:
            finishHandshake(reply);
            break;
        case AuthStep::NeedsPairing:
        case AuthStep::Failed:
            // 브리프 규칙 — 재시도하지 않는다. 사람이 (Task 14b 키패드
            // 화면에서) 새 코드를 넣어야 풀린다. 이번 시도에 쓴 코드는
            // 더 이상 유효하지 않다(맥의 CODE2 는 성공·실패와 무관하게
            // handshake 를 소비한다, `pairing.rs` 주석) — 다음
            // submitCode() 호출까지 지운다.
            holding_ = true;
            pendingCode_ = "";
            webSocket_.disconnect();
            break;
        default:
            // SendHello2 — authOnReply 는 "응답을 보고 다음 동사를 정하는"
            // 함수라 이 값을 돌려주지 않는다(그건 연결 시작 시점의
            // authInitialStep 의 몫이다, authfsm.cpp 판정 표에 SendHello2
            // 행이 없다).
            break;
    }
}

void Transport::sendProof2(const ReplyView &reply) {
    uint8_t spk[32];
    uint8_t nonce[16];  // 32 hex 문자 — pairing.rs 의 `random_hex128()` 실측.
    if (!hexDecode(reply.epk, spk, sizeof spk) || !hexDecode(reply.nonce, nonce, sizeof nonce)) {
        Serial.println("transport: Nonce2 의 epk/nonce 길이가 이상하다 — Failed");
        setAuthStep(AuthStep::Failed);
        holding_ = true;
        webSocket_.disconnect();
        return;
    }

    uint8_t ss[32];
    if (!v2X25519(mySecret_, spk, ss)) {
        Serial.println("transport: 맥의 임시 공개키가 저차 점이다 — Failed");
        setAuthStep(AuthStep::Failed);
        holding_ = true;
        webSocket_.disconnect();
        v2Wipe(ss, sizeof ss);
        return;
    }

    uint8_t tr[64];
    v2Transcript(myPublic_, spk, tr);

    uint8_t tokenBytes[16];
    if (!hexDecode(config_->token, tokenBytes, sizeof tokenBytes)) {
        // configTokenIsValid 가 이미 이 형식(32자 소문자 hex)을 보장하므로
        // 정상 흐름에서는 도달하지 않는다 — 그래도 조용히 넘기지 않는다.
        Serial.println("transport: 저장된 토큰이 16바이트 hex 로 디코드되지 않는다 — Failed");
        setAuthStep(AuthStep::Failed);
        holding_ = true;
        webSocket_.disconnect();
        v2Wipe(ss, sizeof ss);
        v2Wipe(tokenBytes, sizeof tokenBytes);  // 실패해도 일부가 채워졌을 수 있다.
        return;
    }

    uint8_t proof[32];
    v2SessionProof(tokenBytes, sizeof tokenBytes, nonce, sizeof nonce, tr, proof);
    v2DeriveSessionKeys(ss, tokenBytes, sizeof tokenBytes, nonce, sizeof nonce, pendingS2c_,
                         pendingC2s_);

    sendVerb("PROOF2:", proof, sizeof proof);

    v2Wipe(ss, sizeof ss);
    v2Wipe(tokenBytes, sizeof tokenBytes);
    v2Wipe(proof, sizeof proof);
}

void Transport::sendCode2(const ReplyView &reply) {
    uint8_t spk[32];
    uint8_t nonce[16];  // 32 hex 문자 — pairing.rs 의 `random_hex128()` 실측.
    if (!hexDecode(reply.epk, spk, sizeof spk) || !hexDecode(reply.nonce, nonce, sizeof nonce)) {
        Serial.println("transport: AwaitingCode2 의 epk/nonce 길이가 이상하다 — Failed");
        setAuthStep(AuthStep::Failed);
        holding_ = true;
        pendingCode_ = "";
        webSocket_.disconnect();
        return;
    }

    if (!v2X25519(mySecret_, spk, pendingSs_)) {
        Serial.println("transport: 맥의 임시 공개키가 저차 점이다 — Failed");
        setAuthStep(AuthStep::Failed);
        holding_ = true;
        pendingCode_ = "";
        webSocket_.disconnect();
        v2Wipe(pendingSs_, sizeof pendingSs_);
        return;
    }
    memcpy(pendingNonceBytes_, nonce, sizeof nonce);

    uint8_t tr[64];
    v2Transcript(myPublic_, spk, tr);

    uint8_t cbind[32];
    v2CodeBinding(pendingCode_.c_str(), tr, cbind);

    sendVerb("CODE2:", cbind, sizeof cbind);
    v2Wipe(cbind, sizeof cbind);

    // pendingCode_/pendingSs_/pendingNonceBytes_ 는 여기서 지우지 않는다 —
    // 응답(Granted2 성공, Denied/Rejected 실패)이 올 때까지 필요하다.
    // 성공하면 finishHandshake() 가, 실패하면 handleReply() 의
    // NeedsPairing/Failed 분기가 정리한다.
}

void Transport::finishHandshake(const ReplyView &reply) {
    if (reply.sealed.length() > 0) {
        // Granted2 — HELLO2/CODE2 로 새로 페어링에 성공했다. AUTH2/PROOF2
        // 흐름(아래)과 달리 세션 키를 미리 만들어 둘 수 없었다 — 토큰
        // 자체가 이 응답에서야 처음 생기기 때문이다(맥 쪽 `Code2` 핸들러,
        // `pairing.rs`: `derive_pair_key(&hs.ss, &nonce_bytes)` 로 새 토큰
        // 하나만 봉인해 보내고, 그 새 토큰으로 다시
        // `derive_session_keys(&hs.ss, &token_bytes, &nonce_bytes)` 를
        // 부른다 — 이 함수가 지금부터 그대로 재현한다).
        uint8_t pairKey[32];
        v2DerivePairKey(pendingSs_, pendingNonceBytes_, sizeof pendingNonceBytes_, pairKey);

        if (reply.sealed.length() % 2 != 0) {
            Serial.println("transport: Granted2 의 sealed 필드 길이가 홀수다 — Failed");
            setAuthStep(AuthStep::Failed);
            holding_ = true;
            pendingCode_ = "";
            webSocket_.disconnect();
            v2Wipe(pairKey, sizeof pairKey);
            v2Wipe(pendingSs_, sizeof pendingSs_);
            return;
        }

        const size_t sealedLen = reply.sealed.length() / 2;
        uint8_t sealedBuf[128];
        uint8_t tokenJson[128];
        if (sealedLen < 24 || sealedLen > sizeof sealedBuf ||
            !hexDecode(reply.sealed, sealedBuf, sealedLen)) {
            Serial.println("transport: Granted2 의 sealed 필드가 hex 로 디코드되지 않는다 — Failed");
            setAuthStep(AuthStep::Failed);
            holding_ = true;
            pendingCode_ = "";
            webSocket_.disconnect();
            v2Wipe(pairKey, sizeof pairKey);
            v2Wipe(pendingSs_, sizeof pendingSs_);
            return;
        }

        // 이 sealed 프레임 한 건만 열 일회용 채널 — 카운터는 항상 0부터라
        // 새 인스턴스로 여는 것이 맞다(맥 쪽도 `SealedChannel::new` 를
        // 새로 만들어 딱 한 번 `seal()` 한다).
        SealedChannel pairChannel(pairKey, pairKey);
        size_t tokenJsonLen = 0;
        if (!pairChannel.open(sealedBuf, sealedLen, tokenJson, &tokenJsonLen)) {
            Serial.println("transport: Granted2 의 sealed 토큰을 열 수 없다(위조/변조?) — Failed");
            setAuthStep(AuthStep::Failed);
            holding_ = true;
            pendingCode_ = "";
            webSocket_.disconnect();
            v2Wipe(pairKey, sizeof pairKey);
            v2Wipe(pendingSs_, sizeof pendingSs_);
            v2Wipe(sealedBuf, sizeof sealedBuf);
            return;
        }

        JsonDocument doc;
        if (deserializeJson(doc, tokenJson, tokenJsonLen) != DeserializationError::Ok) {
            Serial.println("transport: 열린 페어링 페이로드가 JSON 이 아니다 — Failed");
            setAuthStep(AuthStep::Failed);
            holding_ = true;
            pendingCode_ = "";
            webSocket_.disconnect();
            v2Wipe(pairKey, sizeof pairKey);
            v2Wipe(pendingSs_, sizeof pendingSs_);
            v2Wipe(tokenJson, sizeof tokenJson);
            return;
        }
        const String newToken = String((const char *)(doc["token"] | ""));

        if (!configSaveToken(*config_, newToken)) {
            Serial.println("transport: 새 토큰이 맥의 발급 형식에 맞지 않는다 — Failed");
            setAuthStep(AuthStep::Failed);
            holding_ = true;
            pendingCode_ = "";
            webSocket_.disconnect();
            v2Wipe(pairKey, sizeof pairKey);
            v2Wipe(pendingSs_, sizeof pendingSs_);
            v2Wipe(tokenJson, sizeof tokenJson);
            return;
        }

        uint8_t tokenBytes[16];
        hexDecode(config_->token, tokenBytes, sizeof tokenBytes);  // configSaveToken 이 이미 검증했다.

        uint8_t newS2c[32];
        uint8_t newC2s[32];
        v2DeriveSessionKeys(pendingSs_, tokenBytes, sizeof tokenBytes, pendingNonceBytes_,
                             sizeof pendingNonceBytes_, newS2c, newC2s);

        delete channel_;
        channel_ = new SealedChannel(newC2s, newS2c);

        v2Wipe(pairKey, sizeof pairKey);
        v2Wipe(pendingSs_, sizeof pendingSs_);
        v2Wipe(tokenBytes, sizeof tokenBytes);
        v2Wipe(newS2c, sizeof newS2c);
        v2Wipe(newC2s, sizeof newC2s);
        v2Wipe(tokenJson, sizeof tokenJson);

        pendingCode_ = "";
        backoffAttempt_ = 0;
        Serial.println("transport: 새로 페어링됨 — 토큰 저장, 세션 키 준비됨");
        return;
    }

    // sealed 가 없다 — AUTH2/PROOF2 흐름(기존 토큰 재연결)은 언제나
    // `Authorized2`(필드 없음)만 돌려준다. 실측: `src-tauri/src/ble/
    // pairing.rs` 의 `v2_proof2_replay_after_success_is_rejected` 테스트가
    // Proof2 성공을 `Authorized2` 하나로만 단정한다.
    delete channel_;
    channel_ = new SealedChannel(pendingC2s_, pendingS2c_);
    v2Wipe(pendingS2c_, sizeof pendingS2c_);
    v2Wipe(pendingC2s_, sizeof pendingC2s_);

    backoffAttempt_ = 0;  // 성공했다 — 다음 끊김은 처음부터 다시 센다.
    Serial.println("transport: 인가됨 — 세션 키 준비됨");
}

void Transport::submitCode(const String &code) {
    pendingCode_ = code;
    holding_ = false;  // 사람이 새 코드를 넣었다 — 재시도를 막던 래치를 푼다.

    // 지금 붙어 있는 연결(이전 실패의 잔재일 수 있다)이 있다면 정리하고
    // 다음 loop() 에서 HELLO2 부터 새로 시작한다 — CODE2 는 handshake 를
    // 소비하므로(`pairing.rs` 주석, sendCode2() 위 참고) 기존 소켓을 그대로
    // 재사용해 봐야 통하지 않는다.
    if (webSocket_.isConnected()) {
        webSocket_.disconnect();
    }
    targetHost_ = "";
    nextAttemptAtMs_ = 0;  // 백오프를 기다리지 않고 바로 다음 loop() 에서 시도한다.
}

void Transport::sendVerb(const char *prefix, const uint8_t *data, size_t len) {
    String msg = prefix;
    msg += toHex(data, len);
    webSocket_.sendTXT(msg);
}
