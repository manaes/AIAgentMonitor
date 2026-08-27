#include "cryptov2.h"

#include <string.h>

// monocypher.h 는 `extern "C"` 가드가 없다(davylandman/Monocypher@2.0.6 확인).
// monocypher.c 는 C 컴파일러가 컴파일해 C 링키지 기호를 내놓는데, 이 파일은
// C++ 이라 가드 없이 include 하면 같은 함수를 C++ 로 다시 선언해 버려서
// 심벌 이름이 맹글링된다 — 컴파일은 통과하지만 **링크 단계에서 undefined
// reference 로 실패한다(실측: `pio test -e cyd` 에서 처음 이 형태로 걸렸다).
// 여기서 직접 감싸서 링키지를 맞춘다.
extern "C" {
#include <monocypher.h>
}
#include <mbedtls/md.h>

// ─────────────────────────────────────────────────────────────────────────────
// T10-A: HKDF-SHA256 은 monocypher 에 없다.
//
// 브리프는 `crypto_hmac_sha256` 을 가정했지만 monocypher 의 유일한 내장 해시는
// BLAKE2b 다. `optional/monocypher-ed25519.h` 확장이 HMAC 을 더하긴 하지만
// **SHA-512** 용(`crypto_sha512_hmac_*`)이지 SHA-256 이 아니다. monocypher
// core·optional 어디에도 SHA-256 은 없다.
//
// Arduino-ESP32 는 mbedtls 를 이미 정적으로 링크하고 있고, SHA-256 은 (X25519 나
// 이색 커브와 달리) sdkconfig 로 꺼져 있을 수 없는 mbedtls 기본 기능이다 —
// `MBEDTLS_SHA256_C` 가 이 프로젝트가 받는 프레임워크 헤더에 실제로 정의돼
// 있음을 확인했다. HMAC-SHA256/HKDF 는 mbedtls 로, X25519 와
// ChaCha20-Poly1305 는 monocypher 로 — 각자 실제로 있는 것만 쓴다.
// ─────────────────────────────────────────────────────────────────────────────

/// HMAC-SHA256. 메시지를 최대 두 조각(`msg1||msg2`)으로 나눠 받는다 — 호출자가
/// 매번 이어붙일 버퍼를 새로 잡을 필요가 없게 하기 위해서다(예: session_proof 의
/// `nonce || transcript`).
static void hmacSha256(const uint8_t *key, size_t keyLen,
                        const uint8_t *msg1, size_t msg1Len,
                        const uint8_t *msg2, size_t msg2Len,
                        uint8_t out[32]) {
    mbedtls_md_context_t ctx;
    mbedtls_md_init(&ctx);
    mbedtls_md_setup(&ctx, mbedtls_md_info_from_type(MBEDTLS_MD_SHA256), 1 /* hmac */);
    mbedtls_md_hmac_starts(&ctx, key, keyLen);
    mbedtls_md_hmac_update(&ctx, msg1, msg1Len);
    if (msg2 != nullptr && msg2Len > 0) {
        mbedtls_md_hmac_update(&ctx, msg2, msg2Len);
    }
    mbedtls_md_hmac_finish(&ctx, out);
    mbedtls_md_free(&ctx);
}

/// HKDF-SHA256(RFC 5869), 32바이트 고정 출력. 출력이 해시 길이와 같으므로
/// expand 는 `T(1) = HMAC(PRK, info || 0x01)` 한 번이면 끝난다.
///
/// `ikm` 도 최대 두 조각(`ikm1||ikm2`)으로 받는다 — `derive_session_keys` 의
/// `ss || token` 을 이어붙일 스택 버퍼가 따로 필요 없다. HMAC 의 incremental
/// 업데이트가 그 이어붙이기를 대신한다.
///
/// **딱 두 조각까지만 지원한다** — 시그니처에 드러나지 않는 제약이다. 지금
/// 호출부 넷(`v2DeriveSessionKeys` 둘, `v2DerivePairKey`)은 전부 한두 조각이라
/// 문제없지만, 세 조각짜리 ikm 이 필요해지면 이 함수의 시그니처부터 늘려야
/// 한다 — 조용히 세 번째 조각을 누락시키는 호출을 만들면 안 된다.
static void hkdf32(const uint8_t *ikm1, size_t ikm1Len,
                    const uint8_t *ikm2, size_t ikm2Len,
                    const uint8_t *salt, size_t saltLen,
                    const char *info, size_t infoLen,
                    uint8_t out[32]) {
    uint8_t prk[32];
    // extract: PRK = HMAC-SHA256(salt, ikm)
    hmacSha256(salt, saltLen, ikm1, ikm1Len, ikm2, ikm2Len, prk);
    // expand: T(1) = HMAC-SHA256(PRK, info || 0x01)
    const uint8_t counter = 0x01;
    hmacSha256(prk, sizeof prk, (const uint8_t *)info, infoLen, &counter, 1, out);
    crypto_wipe(prk, sizeof prk);
}

void v2Transcript(const uint8_t cpk[32], const uint8_t spk[32], uint8_t out[64]) {
    memcpy(out, cpk, 32);
    memcpy(out + 32, spk, 32);
}

void v2DeriveSessionKeys(const uint8_t ss[32],
                          const uint8_t *token, size_t tlen,
                          const uint8_t *nonce, size_t nlen,
                          uint8_t s2c[32], uint8_t c2s[32]) {
    hkdf32(ss, 32, token, tlen, nonce, nlen, AIM_INFO_S2C, strlen(AIM_INFO_S2C), s2c);
    hkdf32(ss, 32, token, tlen, nonce, nlen, AIM_INFO_C2S, strlen(AIM_INFO_C2S), c2s);
}

void v2DerivePairKey(const uint8_t ss[32],
                      const uint8_t *nonce, size_t nlen,
                      uint8_t out[32]) {
    hkdf32(ss, 32, nullptr, 0, nonce, nlen, AIM_INFO_PAIR, strlen(AIM_INFO_PAIR), out);
}

void v2CodeBinding(const char *code, const uint8_t tr[64], uint8_t out[32]) {
    hmacSha256((const uint8_t *)code, strlen(code), tr, 64, nullptr, 0, out);
}

void v2SessionProof(const uint8_t *token, size_t tlen,
                     const uint8_t *nonce, size_t nlen,
                     const uint8_t tr[64], uint8_t out[32]) {
    hmacSha256(token, tlen, nonce, nlen, tr, 64, out);
}

bool v2X25519(const uint8_t mySecret[32], const uint8_t theirPublic[32], uint8_t outSS[32]) {
    static const uint8_t ZERO32[32] = {0};
    int rc = crypto_x25519(outSS, mySecret, theirPublic);
    // 라이브러리의 내부 판정(위 주석 참고)에 더해, 명시적으로 한 번 더 검사한다.
    bool lowOrder = (rc != 0) || (crypto_verify32(outSS, ZERO32) == 0);
    if (lowOrder) {
        crypto_wipe(outSS, 32);
        memset(outSS, 0, 32);
        return false;
    }
    return true;
}

// ─────────────────────────────────────────────────────────────────────────────
// T10-B: IETF ChaCha20-Poly1305(RFC 8439, 12바이트 논스)를 손으로 짠다.
//
// `pio pkg search monocypher` 가 내놓는 유일한 레지스트리 패키지는
// `davylandman/Monocypher@2.0.6`(2019년 발행)이고, 이 프로젝트가 실제로 받는
// 버전도 그것이다(`.pio/libdeps/cyd/Monocypher/src/monocypher.h` 로 직접 확인).
// 이 버전은 편의용 원샷 API 로 `crypto_lock`/`crypto_unlock`,
// `crypto_lock_aead`/`crypto_unlock_aead` 만 제공하는데 전부 **24바이트 논스의
// XChaCha20-Poly1305** 다(내부적으로 HChacha20 으로 부속키를 만든 뒤 원본
// ChaCha20 을 쓴다) — 다른 원시함수라 우리 프로토콜(맥의 Rust
// `chacha20poly1305` 크레이트, RFC 8439 IETF 변형, 12바이트 논스)과 절대 맞물리지
// 않는다. `crypto_aead_init_ietf`, `crypto_chacha20_ietf` 류의 12바이트 논스
// 진입점도 이 헤더에는 없다(`grep -i ietf` 무결과로 확인).
//
// 그래서 저수준 원시함수(`crypto_chacha20_*`, `crypto_poly1305_*`)로 RFC 8439 를
// 직접 조립한다. monocypher 의 공개 `crypto_chacha20_init` 은 **원본 ChaCha20**
// 배치(64비트 카운터 = input[12,13], 64비트 논스 = input[14,15])만 만들어 준다 —
// IETF 배치(32비트 카운터 = input[12], 96비트 논스 = input[13,14,15])와 워드
// 위치 자체가 다르다. 그런데 `crypto_chacha_ctx` 구조체는 헤더에 그대로 공개돼
// 있다("For experts only. You have been warned." 주석과 함께) — 그래서
// `crypto_chacha20_init` 으로 키(input[0..12])만 세팅한 뒤 `input[12..16]` 을
// IETF 규칙대로 직접 덮어쓴다. ChaCha20 코어 라운드 자체는 손대지 않고
// monocypher 구현 그대로 쓰므로, "저수준 원시함수로 조립" 이지 재구현이 아니다.
// ─────────────────────────────────────────────────────────────────────────────

static const uint8_t AIM_AAD[] = "aim-v2";
static const size_t AIM_AAD_LEN = sizeof(AIM_AAD) - 1; // NUL 제외 6바이트

static uint32_t load32LE(const uint8_t s[4]) {
    return (uint32_t)s[0] | ((uint32_t)s[1] << 8) | ((uint32_t)s[2] << 16) |
           ((uint32_t)s[3] << 24);
}

/// ChaCha20 컨텍스트를 IETF 배치(RFC 8439)로 초기화하고, 블록 카운터 0의
/// 키스트림에서 Poly1305 원타임 키를 뽑는다. 호출 뒤 `ctx->input[12]` 는 1이 되어
/// 있다 — RFC 8439 는 카운터 0의 키스트림 앞 32바이트만 Poly1305 키로 쓰고
/// **나머지 32바이트는 버린 뒤 메시지 키스트림을 카운터 1부터** 시작하라고
/// 규정하는데, monocypher 의 64바이트 풀이 정확히 그 경계에서 소진되므로
/// (`chacha20_encrypt` 가 풀을 64바이트씩 채우고 `input[12]` 를 그때 전진시킨다)
/// 64바이트를 통째로 뽑아내는 것만으로 그 규정이 자연히 지켜진다.
static void ietfInitAndPolyKey(const uint8_t key[32], const uint8_t nonce12[12],
                                crypto_chacha_ctx *ctx, uint8_t polyKey[32]) {
    uint8_t dummyNonce8[8] = {0};
    crypto_chacha20_init(ctx, key, dummyNonce8);

    ctx->input[13] = load32LE(nonce12 + 0);
    ctx->input[14] = load32LE(nonce12 + 4);
    ctx->input[15] = load32LE(nonce12 + 8);
    ctx->input[12] = 0; // 블록 카운터 0 — Poly1305 키 생성용
    ctx->pool_idx = 64; // 다음 바이트 요청 때 새로 채우도록 강제

    uint8_t block0[64];
    crypto_chacha20_encrypt(ctx, block0, nullptr, 64);
    memcpy(polyKey, block0, 32);
    crypto_wipe(block0, sizeof block0);
    // ctx->input[12] 는 이제 1 — 메시지 키스트림은 여기서부터.
}

static void pad16Update(crypto_poly1305_ctx *pctx, size_t len) {
    static const uint8_t ZERO16[16] = {0};
    size_t rem = len % 16;
    if (rem != 0) {
        crypto_poly1305_update(pctx, ZERO16, 16 - rem);
    }
}

/// RFC 8439 §2.8 의 태그 계산: `AAD || pad16(AAD) || CT || pad16(CT) ||
/// LE64(len(AAD)) || LE64(len(CT))` 를 Poly1305 원타임 키로 MAC 한다.
static void aeadTag(const uint8_t polyKey[32],
                     const uint8_t *aad, size_t aadLen,
                     const uint8_t *ct, size_t ctLen,
                     uint8_t tag[16]) {
    crypto_poly1305_ctx pctx;
    crypto_poly1305_init(&pctx, polyKey);
    crypto_poly1305_update(&pctx, aad, aadLen);
    pad16Update(&pctx, aadLen);
    crypto_poly1305_update(&pctx, ct, ctLen);
    pad16Update(&pctx, ctLen);

    uint8_t lens[16];
    uint64_t a = (uint64_t)aadLen, c = (uint64_t)ctLen;
    for (int i = 0; i < 8; i++) {
        lens[i] = (uint8_t)(a >> (8 * i));
        lens[8 + i] = (uint8_t)(c >> (8 * i));
    }
    crypto_poly1305_update(&pctx, lens, sizeof lens);
    crypto_poly1305_final(&pctx, tag);
}

SealedChannel::SealedChannel(const uint8_t sendKey[32], const uint8_t recvKey[32])
    : sendCounter_(0), haveLastRecv_(false), lastRecv_(0) {
    memcpy(sendKey_, sendKey, 32);
    memcpy(recvKey_, recvKey, 32);
}

SealedChannel::~SealedChannel() {
    crypto_wipe(sendKey_, sizeof sendKey_);
    crypto_wipe(recvKey_, sizeof recvKey_);
}

size_t SealedChannel::seal(const uint8_t *plaintext, size_t len, uint8_t *out) {
    uint64_t counter = sendCounter_++;

    uint8_t nonce12[12] = {0, 0, 0, 0};
    for (int i = 0; i < 8; i++) {
        nonce12[4 + i] = (uint8_t)(counter >> (8 * (7 - i)));
        out[i] = nonce12[4 + i]; // 프레임 접두 = 같은 빅엔디언 카운터
    }

    crypto_chacha_ctx ctx;
    uint8_t polyKey[32];
    ietfInitAndPolyKey(sendKey_, nonce12, &ctx, polyKey);

    uint8_t *ct = out + 8;
    crypto_chacha20_encrypt(&ctx, ct, plaintext, len);
    crypto_wipe(&ctx, sizeof ctx);

    uint8_t tag[16];
    aeadTag(polyKey, AIM_AAD, AIM_AAD_LEN, ct, len, tag);
    crypto_wipe(polyKey, sizeof polyKey);

    memcpy(out + 8 + len, tag, sizeof tag);
    return 8 + len + 16;
}

bool SealedChannel::open(const uint8_t *frame, size_t len, uint8_t *out, size_t *outLen) {
    if (len < 8 + 16) {
        return false;
    }

    uint64_t counter = 0;
    for (int i = 0; i < 8; i++) {
        counter = (counter << 8) | frame[i];
    }
    if (haveLastRecv_ && counter <= lastRecv_) {
        return false; // 재전송이거나 순서 역행
    }

    size_t ctLen = len - 8 - 16;
    const uint8_t *ct = frame + 8;
    const uint8_t *tagIn = frame + 8 + ctLen;

    uint8_t nonce12[12] = {0, 0, 0, 0};
    for (int i = 0; i < 8; i++) {
        nonce12[4 + i] = frame[i];
    }

    crypto_chacha_ctx ctx;
    uint8_t polyKey[32];
    ietfInitAndPolyKey(recvKey_, nonce12, &ctx, polyKey);

    uint8_t expectedTag[16];
    aeadTag(polyKey, AIM_AAD, AIM_AAD_LEN, ct, ctLen, expectedTag);

    if (crypto_verify16(expectedTag, tagIn) != 0) {
        // 인증 실패 — 여기서 반환하고 lastRecv_ 는 절대 건드리지 않는다.
        // 그래야 변조된 프레임 하나가 이후 정상 프레임을 영구히 막지 못한다.
        crypto_wipe(polyKey, sizeof polyKey);
        crypto_wipe(&ctx, sizeof ctx);
        return false;
    }

    crypto_chacha20_encrypt(&ctx, out, ct, ctLen); // 복호 = 같은 XOR
    crypto_wipe(&ctx, sizeof ctx);
    crypto_wipe(polyKey, sizeof polyKey);

    lastRecv_ = counter; // 인증에 성공한 뒤에만 전진
    haveLastRecv_ = true;
    *outLen = ctLen;
    return true;
}

String toHex(const uint8_t *data, size_t len) {
    static const char *digits = "0123456789abcdef";
    String out;
    out.reserve(len * 2);
    for (size_t i = 0; i < len; i++) {
        out += digits[data[i] >> 4];
        out += digits[data[i] & 0x0f];
    }
    return out;
}
