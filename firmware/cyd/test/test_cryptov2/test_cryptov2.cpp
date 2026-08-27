// AI Agent Monitor — CYD 펌웨어: E2EE v2 골든 벡터 대조.
//
// `docs/ble-protocol/golden/e2ee-v2-sample.json` 의 값을 그대로 옮긴다.
// **세 언어가 같은 값을 내야 한다 — 이 테스트가 실패하면 프로토콜이 갈라진 것이다.**
// 갱신하면 여기도 갱신한다.
//
// (T10-D) 골든 파일 자체의 `note` 가 못박은 규약: 아래 SHARED_SECRET/CPK/SPK/
// NONCE/TOKEN 은 **원시 바이트**다 — 이를테면 SHARED_SECRET 은 0x11 이 32번
// 반복된 바이트 배열이지, `"1111...1"` 이라는 64글자 hex 문자열의 UTF-8 바이트가
// 아니다. 거꾸로 하면 골든이 통째로 어긋나는데, 세 구현 전체에서 가장 흔한
// 실수라 여기서도 다시 적는다.
//
// (변수명 참고) 골든 파일과 스펙은 이 값을 `ss` 라 부르지만, 여기서는
// `SHARED_SECRET` 으로 풀어 썼다 — `Arduino.h`(`pins_arduino.h`) 가 SPI Slave
// Select 핀 매크로로 전역 `SS` 를 이미 잡고 있어 이름이 충돌한다.

#include <unity.h>
#include <Arduino.h>
#include "cryptov2.h"

static const uint8_t SHARED_SECRET[32] = {
    0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11,
    0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11,
    0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11,
};
static const uint8_t CPK[32] = {
    0x22, 0x22, 0x22, 0x22, 0x22, 0x22, 0x22, 0x22, 0x22, 0x22, 0x22, 0x22,
    0x22, 0x22, 0x22, 0x22, 0x22, 0x22, 0x22, 0x22, 0x22, 0x22, 0x22, 0x22,
    0x22, 0x22, 0x22, 0x22, 0x22, 0x22, 0x22, 0x22,
};
static const uint8_t SPK[32] = {
    0x33, 0x33, 0x33, 0x33, 0x33, 0x33, 0x33, 0x33, 0x33, 0x33, 0x33, 0x33,
    0x33, 0x33, 0x33, 0x33, 0x33, 0x33, 0x33, 0x33, 0x33, 0x33, 0x33, 0x33,
    0x33, 0x33, 0x33, 0x33, 0x33, 0x33, 0x33, 0x33,
};
static const uint8_t NONCE[16] = {
    0x44, 0x44, 0x44, 0x44, 0x44, 0x44, 0x44, 0x44,
    0x44, 0x44, 0x44, 0x44, 0x44, 0x44, 0x44, 0x44,
};
static const uint8_t TOKEN[16] = {
    0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55,
    0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55,
};
static const char *CODE = "123456";

// docs/ble-protocol/golden/e2ee-v2-sample.json 의 여덟 값.
static const char *EXPECT_TRANSCRIPT =
    "22222222222222222222222222222222"
    "22222222222222222222222222222222"
    "33333333333333333333333333333333"
    "33333333333333333333333333333333";
static const char *EXPECT_CODE_BINDING =
    "5ebe7b92a1fa298d7a6f6bc92dcfc01be6b52f2e8db46ac4c0dbf19fe3e4be37";
static const char *EXPECT_K_S2C =
    "428bb14b6c0957581892b459f4c6578d9bda35d8b687f34fd8cadb5988b7ddb8";
static const char *EXPECT_K_C2S =
    "84ff86a1f55ad2ca05a1a186559656f8efa19c88a5559cc8fa58a44da068cc5d";
static const char *EXPECT_PAIR_KEY =
    "e7488866e51e8a77f109547dbc408774c7676f18254c39e6058c29f2eb375536";
static const char *EXPECT_SESSION_PROOF =
    "5040a8456eea36d340020216d2b5bc18742cefe94c04c72f9bd97d65452486ef";
static const char *EXPECT_SEALED_FRAME_0 =
    "000000000000000063f4db4526c90e3b6e9117390e0aa3cf656af45204a5ad";
static const char *EXPECT_SEALED_FRAME_1 =
    "00000000000000018ccc887c7e6a27b313fa21a42e102fcc42ac70623e25af";

void test_transcript_matches_golden() {
    uint8_t tr[64];
    v2Transcript(CPK, SPK, tr);
    TEST_ASSERT_EQUAL_STRING(EXPECT_TRANSCRIPT, toHex(tr, 64).c_str());
}

void test_code_binding_matches_golden() {
    uint8_t tr[64], out[32];
    v2Transcript(CPK, SPK, tr);
    v2CodeBinding(CODE, tr, out);
    TEST_ASSERT_EQUAL_STRING(EXPECT_CODE_BINDING, toHex(out, 32).c_str());
}

void test_session_keys_match_golden() {
    uint8_t s2c[32], c2s[32];
    v2DeriveSessionKeys(SHARED_SECRET, TOKEN, sizeof TOKEN, NONCE, sizeof NONCE, s2c, c2s);
    TEST_ASSERT_EQUAL_STRING(EXPECT_K_S2C, toHex(s2c, 32).c_str());
    TEST_ASSERT_EQUAL_STRING(EXPECT_K_C2S, toHex(c2s, 32).c_str());
}

void test_pair_key_matches_golden() {
    uint8_t out[32];
    v2DerivePairKey(SHARED_SECRET, NONCE, sizeof NONCE, out);
    TEST_ASSERT_EQUAL_STRING(EXPECT_PAIR_KEY, toHex(out, 32).c_str());
}

void test_session_proof_matches_golden() {
    uint8_t tr[64], out[32];
    v2Transcript(CPK, SPK, tr);
    v2SessionProof(TOKEN, sizeof TOKEN, NONCE, sizeof NONCE, tr, out);
    TEST_ASSERT_EQUAL_STRING(EXPECT_SESSION_PROOF, toHex(out, 32).c_str());
}

// sealed_frame_0/1 이 이 스위트에서 가장 중요하다 — nonce/counter 조립
// ([0,0,0,0] || BE(counter)) 을 고정하는 유일한 자리다. 골든 파일의 `input` 에는
// 봉인 프레임의 평문이 없으므로, 스펙 §10 에 적힌 대로 `{"v":2}` 를 쓴다
// (`src-tauri/src/crypto/mod.rs` 의 골든 생성 테스트가 그 값으로 봉인했다).
void test_sealed_frames_match_golden_and_reopen_to_plaintext() {
    uint8_t s2c[32], c2s[32];
    v2DeriveSessionKeys(SHARED_SECRET, TOKEN, sizeof TOKEN, NONCE, sizeof NONCE, s2c, c2s);

    // 맥(서버) 쪽: 보낼 때는 s2c, 받을 때는 c2s.
    SealedChannel server(s2c, c2s);

    const uint8_t plaintext[] = "{\"v\":2}";
    const size_t plen = sizeof(plaintext) - 1; // NUL 제외 7바이트

    uint8_t frame0[8 + 7 + 16];
    size_t frame0Len = server.seal(plaintext, plen, frame0);
    TEST_ASSERT_EQUAL_UINT32(sizeof frame0, frame0Len);
    TEST_ASSERT_EQUAL_STRING(EXPECT_SEALED_FRAME_0, toHex(frame0, frame0Len).c_str());

    uint8_t frame1[8 + 7 + 16];
    size_t frame1Len = server.seal(plaintext, plen, frame1);
    TEST_ASSERT_EQUAL_UINT32(sizeof frame1, frame1Len);
    TEST_ASSERT_EQUAL_STRING(EXPECT_SEALED_FRAME_1, toHex(frame1, frame1Len).c_str());

    // 클라이언트 쪽: 보낼 때는 c2s, 받을 때는 s2c — server 와 반대.
    SealedChannel client(c2s, s2c);
    uint8_t opened[16];
    size_t openedLen = 0;
    bool ok = client.open(frame1, frame1Len, opened, &openedLen);
    TEST_ASSERT_TRUE_MESSAGE(ok, "sealed_frame_1 은 client 의 세션 키로 열려야 한다");
    TEST_ASSERT_EQUAL_UINT32(plen, openedLen);
    TEST_ASSERT_EQUAL_UINT8_ARRAY(plaintext, opened, plen);
}

// (T10-C) 카운터가 순서대로가 아니어도(0을 건너뛰고 1부터) 열려야 한다 — 위
// 테스트가 frame1 을 먼저 여는 것 자체가 이 성질의 최소 증거이지만, 여기서는
// 뮤테이션 관점의 반증도 같이 둔다: 같은 프레임을 두 번 열면 두 번째는 재전송으로
// 거절돼야 한다.
void test_reopening_same_frame_is_rejected_as_replay() {
    uint8_t s2c[32], c2s[32];
    v2DeriveSessionKeys(SHARED_SECRET, TOKEN, sizeof TOKEN, NONCE, sizeof NONCE, s2c, c2s);
    SealedChannel server(s2c, c2s);
    SealedChannel client(c2s, s2c);

    const uint8_t plaintext[] = "hi";
    uint8_t frame[8 + 2 + 16];
    size_t frameLen = server.seal(plaintext, sizeof plaintext, frame);

    uint8_t opened[8];
    size_t openedLen = 0;
    TEST_ASSERT_TRUE(client.open(frame, frameLen, opened, &openedLen));
    TEST_ASSERT_FALSE_MESSAGE(client.open(frame, frameLen, opened, &openedLen),
                               "같은 카운터를 두 번 받으면 거부해야 한다");
}

// (T10-E) 저차 점(전부 0인 공개키)은 명시적으로 거절해야 한다. monocypher
// 2.0.6 은 내부적으로도 이미 걸러내지만(`monocypher.c:1357`), 그 사실에만
// 기대지 않는다는 것을 이 테스트로 고정한다 — v2X25519 안의 명시적
// crypto_verify32 검사를 지우면 이 테스트가 실제로 빨개져야 한다는 뜻이다
// (단, 이 버전은 라이브러리 자체도 막으므로 오늘은 두 겹 모두가 이 테스트를
// 통과시킨다).
void test_x25519_rejects_low_order_point() {
    static const uint8_t ZERO32[32] = {0};
    uint8_t mySecret[32];
    memset(mySecret, 0x07, sizeof mySecret); // 아무 임의의 비밀키
    uint8_t outSS[32];
    memset(outSS, 0xAA, sizeof outSS); // 실패 시 0으로 덮였는지 확인하기 위한 오염값

    bool ok = v2X25519(mySecret, ZERO32, outSS);
    TEST_ASSERT_FALSE_MESSAGE(ok, "전부 0인 공개키(저차 점)는 거절해야 한다");
    TEST_ASSERT_EQUAL_UINT8_ARRAY(ZERO32, outSS, 32);
}

void setup() {
    delay(2000); // 시리얼 모니터가 붙을 시간을 준다.
    UNITY_BEGIN();
    RUN_TEST(test_transcript_matches_golden);
    RUN_TEST(test_code_binding_matches_golden);
    RUN_TEST(test_session_keys_match_golden);
    RUN_TEST(test_pair_key_matches_golden);
    RUN_TEST(test_session_proof_matches_golden);
    RUN_TEST(test_sealed_frames_match_golden_and_reopen_to_plaintext);
    RUN_TEST(test_reopening_same_frame_is_rejected_as_replay);
    RUN_TEST(test_x25519_rejects_low_order_point);
    UNITY_END();
}

void loop() {}
