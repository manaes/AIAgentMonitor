// AI Agent Monitor — CYD 펌웨어: E2EE v2 암호 계층.
//
// Task 10 는 이 파일이 스펙(`docs/superpowers/specs/2026-08-25-e2ee-protocol-v2-design.md`)
// 을 **혼자 읽고** 구현한 뒤, `docs/ble-protocol/golden/e2ee-v2-sample.json` 하나로
// 맥(Rust)·아이폰(Swift)과 값을 맞추는 태스크다. 세 구현이 같은 파일을 읽고 같은
// 값을 내야 프로토콜이 하나라고 말할 수 있다 — 이 검사가 실패하면 어느 한쪽이
// 아니라 **프로토콜 자체가 갈라진 것**이다.
//
// 이 모듈은 전송을 모른다(BLE·LAN·iroh 어느 쪽도 알지 못한다). 순수 함수와
// `SealedChannel` 하나로만 구성된다.

// (파일 위치 참고) 브리프는 이 파일을 `firmware/cyd/src/` 에 두라고 했지만
// `lib/cryptov2/` 로 옮겼다.
//
// `src/` 에 두면 `pio test -e cyd` 가 `src/main.cpp` 의 `setup()`/`loop()` 와
// `test/test_cryptov2.cpp` 의 `setup()`/`loop()` 를 함께 링크해 이중 정의로
// 실패한다(실측). 이 PlatformIO(6.1.19) 는 테스트 빌드의 모든 컴파일에
// `-DUNIT_TEST -DPIO_UNIT_TESTING` 를 실제로 정의해 준다(`-vvv` 컴파일
// 커맨드라인으로 확인) — 그러니 `main.cpp` 를 `#ifndef UNIT_TEST` 로 감싸
// 브리프대로 `src/` 에 두는 방법도 실제로 가능했다. 그런데도 `lib/` 를 골랐다:
// 이 모듈은 원래 전송·설정과 무관한 순수 모듈(스펙 §9)이라 `lib/` 라는 위치와
// 성격이 맞고, `main.cpp` 를 매크로 가드로라도 건드리지 않는 편이 Task 8~9
// 산출물에 대한 변경을 0으로 유지한다. `main.cpp` 는 Task 11(페어링)이 이
// 모듈을 실제로 연결할 때 처음 `#include` 하게 된다.

#pragma once

#include <Arduino.h>

// HKDF info 문자열. 세 언어가 바이트 단위로 같아야 한다(스펙 §5, §6).
#define AIM_INFO_PAIR "aim-pair-v2"
#define AIM_INFO_S2C  "aim-sess-v2-s2c"
#define AIM_INFO_C2S  "aim-sess-v2-c2s"

/// 두 임시 공개키를 이어붙인 64바이트. **항상 클라이언트 키가 먼저다** — 역할과
/// 무관하게 양쪽이 같은 순서로 만들어야 cbind 와 proof 가 일치한다(스펙 §4).
void v2Transcript(const uint8_t cpk[32], const uint8_t spk[32], uint8_t out[64]);

/// 재연결(6장) 세션 키 두 개. `ikm = ss || token` 이라 **둘 다 있어야** 키가
/// 나온다 — X25519 가 깨져도 토큰이 필요하고, 토큰이 새도 임시 개인키가 필요하다.
void v2DeriveSessionKeys(const uint8_t ss[32],
                          const uint8_t *token, size_t tlen,
                          const uint8_t *nonce, size_t nlen,
                          uint8_t s2c[32], uint8_t c2s[32]);

/// 페어링(5장) 단계에서 토큰 전달 한 건만 봉인하는 키. 이 시점에는 토큰이 없으므로
/// ikm 이 공유 비밀뿐이다. 브리프의 Produces 목록에는 없었지만 골든 벡터에 `pair_key`
/// 가 있고, Task 11(페어링)이 CODE2 응답의 토큰을 열 때 그대로 필요하다.
void v2DerivePairKey(const uint8_t ss[32],
                      const uint8_t *nonce, size_t nlen,
                      uint8_t out[32]);

/// 6자리 코드를 **키로** 써서 두 임시 공개키를 MAC 한다(스펙 §5.1). 코드 자체는
/// 링크에 나타나지 않는다 — v1 은 `CODE:123456` 을 그대로 보냈지만 v2 는 이 값만
/// 보낸다.
void v2CodeBinding(const char *code, const uint8_t tr[64], uint8_t out[32]);

/// 재연결 증명(스펙 §6). v1 의 `HMAC(token, nonce)` 에 transcript 를 붙여 키
/// 합의를 토큰에 묶는다 — 중간자가 임시 키를 바꿔치기하면 proof 가 맞지 않는다.
void v2SessionProof(const uint8_t *token, size_t tlen,
                     const uint8_t *nonce, size_t nlen,
                     const uint8_t tr[64], uint8_t out[32]);

/// X25519 키 합의. **저차 점을 명시적으로 거절한다**(스펙 §5.2, T10-E).
/// monocypher 2.0.6(이 프로젝트가 실제로 받는 버전, `davylandman/Monocypher`)의
/// `crypto_x25519` 는 소스를 확인한 결과 이미 내부에서 결과가 전부 0이면 실패를
/// 돌려준다(`monocypher.c:1357`, `return -1 - zerocmp32(raw_shared_secret);`).
/// 그래도 이 버전에만 기대지 않도록 여기서 `crypto_verify32` 로 한 번 더 명시적으로
/// 검사한다 — 라이브러리를 바꾸거나 버전을 올렸을 때 이 성질이 조용히 사라지는
/// 사고를 막기 위해서다.
///
/// 반환값 false 는 상대가 저차 점(대표적으로 전부 0인 공개키)을 보냈다는 뜻이다.
/// 이 경우 `outSS` 는 정의되지 않은 값이 아니라 항상 0으로 채워진다.
bool v2X25519(const uint8_t mySecret[32], const uint8_t theirPublic[32], uint8_t outSS[32]);

/// 방향별 키와 카운터를 갖는 봉인 채널(스펙 §7). **(키, 논스) 쌍은 절대 재사용하지
/// 않는다** — 세션마다 키가 다르므로 카운터를 0에서 시작해도 안전하다.
///
/// AEAD 논스(12바이트) = `[0,0,0,0] || counter.to_be_bytes()`(u64, 빅엔디언).
/// AAD = `"aim-v2"`. 봉인 프레임 = `counter(8바이트 BE) || ciphertext || tag(16바이트)`.
class SealedChannel {
public:
    SealedChannel(const uint8_t sendKey[32], const uint8_t recvKey[32]);
    ~SealedChannel();

    /// 봉인 프레임 길이 = 8 + len + 16. `out` 은 그만큼 여유가 있어야 한다.
    /// 반환값은 실제로 쓴 바이트 수.
    size_t seal(const uint8_t *plaintext, size_t len, uint8_t *out);

    /// 인증에 성공한 뒤에만 `lastRecv_` 를 전진시킨다 — 그래야 변조된 프레임
    /// 하나가 이후 정상 프레임을 영구히 막지 못한다. 카운터가 이전에 받아들인
    /// 값 이하이면(재전송·순서 역행) 즉시 거절한다.
    ///
    /// 성공하면 true 를 돌려주고 `*outLen` 에 평문 길이를 채운다. `out` 은
    /// 최소 `len - 24` 바이트 여유가 있어야 한다.
    bool open(const uint8_t *frame, size_t len, uint8_t *out, size_t *outLen);

private:
    uint8_t sendKey_[32];
    uint8_t recvKey_[32];
    uint64_t sendCounter_;
    bool haveLastRecv_;
    uint64_t lastRecv_;
};

/// 소문자 hex 인코딩. 테스트에서 골든 벡터와 문자열로 대조할 때 쓴다.
String toHex(const uint8_t *data, size_t len);

// ─────────────────────────────────────────────────────────────────────────────
// Task 12 이 더한 것: 이 연결의 임시 X25519 키쌍을 만드는 것과, 다 쓴 비밀값을
// 지우는 것. 둘 다 여전히 "전송을 모르는" 함수다 — 소켓도 mDNS 도 이 파일에
// 들어오지 않는다. 여기 두는 이유는 순전히 monocypher 링키지 때문이다:
// `crypto_x25519_public_key`/`crypto_wipe` 는 이 프로젝트에서 `extern "C"` 로
// 감싸 부르는 자리가 `cryptov2.cpp` 한 곳뿐이어야 한다(그 이유는 파일 상단
// 주석 — 가드 없이 두 곳에서 include 하면 심벌이 갈라질 여지가 생긴다).
// `Transport` 가 직접 monocypher 를 include 하는 대신 이 두 함수를 통해서만
// 만나는 것이 그 경계를 지킨다.
// ─────────────────────────────────────────────────────────────────────────────

/// 이 연결의 임시 X25519 키쌍을 만든다. 비밀키는 하드웨어 TRNG
/// (`esp_fill_random`)로 채운다 — ESP32 의 진짜 엔트로피는 WiFi/BT RF 가 켜져
/// 있을 때 나오는데, 이 함수를 부르는 `Transport` 는 WiFi 가 이미 연결된
/// 뒤에만 동작하므로 그 조건은 항상 만족된다.
void v2GenerateKeypair(uint8_t secret[32], uint8_t pub[32]);

/// 다 쓴 비밀값을 지운다. `crypto_wipe` 그대로다 — 위 경계 주석 참고.
void v2Wipe(uint8_t *buf, size_t len);
