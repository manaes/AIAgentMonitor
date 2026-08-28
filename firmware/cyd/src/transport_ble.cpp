// AI Agent Monitor — CYD 펌웨어: BLE Central 전송 계층 구현
#include "transport_ble.h"

#include <ArduinoJson.h>
#include <string.h>

namespace {

static const NimBLEUUID SERVICE_UUID("07A98A35-16C7-4BBA-A296-E28B78B7E683");
static const NimBLEUUID AUTH_UUID("1403603A-4C78-4899-A2B8-FDA198101900");
static const NimBLEUUID SNAPSHOT_UUID("0AE789AA-EF38-4A35-9E72-A7CD7AD995D5");

constexpr uint32_t SCAN_INTERVAL_MS = 10000;
constexpr uint32_t BLE_SNAPSHOT_TIMEOUT_MS = 45000;
constexpr size_t MAX_SNAPSHOT_FRAME_BYTES = 64 * 1024;

TransportBle *g_activeBleInstance = nullptr;

int hexNibble(char c) {
    if (c >= '0' && c <= '9') return c - '0';
    if (c >= 'a' && c <= 'f') return c - 'a' + 10;
    return -1;
}

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

class BleAdvertisedDeviceCallbacks : public NimBLEAdvertisedDeviceCallbacks {
    void onResult(NimBLEAdvertisedDevice *advertisedDevice) override {
        if (g_activeBleInstance == nullptr) {
            return;
        }

        std::string name = advertisedDevice->haveName() ? advertisedDevice->getName() : "";
        std::string mfg = advertisedDevice->haveManufacturerData() ? advertisedDevice->getManufacturerData() : "";
        String mfgHex = "";
        for (size_t i = 0; i < mfg.length() && i < 8; i++) {
            char buf[4];
            sprintf(buf, "%02x", (uint8_t)mfg[i]);
            mfgHex += buf;
        }

        Serial.printf("BLE [스캔]: addr=%s rssi=%d name='%s' srvCount=%d mfg=%s\n",
                      advertisedDevice->getAddress().toString().c_str(),
                      advertisedDevice->getRSSI(),
                      name.c_str(),
                      advertisedDevice->getServiceUUIDCount(),
                      mfgHex.c_str());

        bool match = false;
        if (advertisedDevice->isAdvertisingService(SERVICE_UUID)) {
            match = true;
        } else if (!name.empty() && (name.rfind("AIM-", 0) == 0 || name.rfind("AIM", 0) == 0)) {
            match = true;
        } else {
            // 서비스 UUID 목록 직접 비교
            for (size_t i = 0; i < advertisedDevice->getServiceUUIDCount(); i++) {
                if (advertisedDevice->getServiceUUID(i).equals(SERVICE_UUID)) {
                    match = true;
                    break;
                }
            }
        }

        if (match) {
            Serial.printf("BLE: ★ Mac 미러 기기 발견! (이름: %s, 주소: %s, RSSI: %d) -> 연결 시도\n",
                          name.c_str(),
                          advertisedDevice->getAddress().toString().c_str(),
                          advertisedDevice->getRSSI());
            g_activeBleInstance->onDiscovered(advertisedDevice);
        }
    }
};

class BleClientCallbacks : public NimBLEClientCallbacks {
    void onConnect(NimBLEClient *pClient) override {
        Serial.println("BLE: GATT 서버에 연결됨");
        if (g_activeBleInstance != nullptr) {
            g_activeBleInstance->handleConnected();
        }
    }

    void onDisconnect(NimBLEClient *pClient) override {
        Serial.println("BLE: GATT 연결 끊김");
        if (g_activeBleInstance != nullptr) {
            g_activeBleInstance->handleDisconnected();
        }
    }
};

static BleAdvertisedDeviceCallbacks g_scanCallbacks;
static BleClientCallbacks g_clientCallbacks;

static void onScanComplete(NimBLEScanResults results) {
    if (g_activeBleInstance != nullptr) {
        g_activeBleInstance->onScanEnded();
    }
}

}  // namespace

TransportBle::TransportBle() {
    g_activeBleInstance = this;
}

TransportBle::~TransportBle() {
    stop();
    delete channel_;
    if (g_activeBleInstance == this) {
        g_activeBleInstance = nullptr;
    }
}

void TransportBle::begin(Config &config) {
    config_ = &config;
    wantsRunning_ = true;

    // NimBLE 초기화 (최초 1회만 실행)
    static bool s_bleInit = false;
    if (!s_bleInit) {
        NimBLEDevice::init("CYD-Monitor");
        NimBLEDevice::setPower(ESP_PWR_LVL_P9);
        NimBLEDevice::setMTU(517);
        s_bleInit = true;
    }

    resetAuth();
    startScan();
}

void TransportBle::resetAuth() {
    delete channel_;
    channel_ = nullptr;
    v2Wipe(pendingS2c_, sizeof(pendingS2c_));
    v2Wipe(pendingC2s_, sizeof(pendingC2s_));
    v2Wipe(pendingSs_, sizeof(pendingSs_));
    v2Wipe(pendingNonceBytes_, sizeof(pendingNonceBytes_));
    v2Wipe(pendingSpk_, sizeof(pendingSpk_));
    hasAwaitingParams_ = false;
    v2Wipe(mySecret_, sizeof(mySecret_));
    v2Wipe(myPublic_, sizeof(myPublic_));

    v2GenerateKeypair(mySecret_, myPublic_);

    const bool hasStoredToken = (config_ != nullptr && configIsPaired(*config_));
    const bool hasCode = (pendingCode_.length() == 6);
    authStep_ = authInitialStep(hasStoredToken, hasCode);
    hasSnapshot_ = false;
}

void TransportBle::startScan() {
    if (!wantsRunning_ || isScanning_ || (client_ != nullptr && client_->isConnected())) {
        return;
    }

    NimBLEScan *pScan = NimBLEDevice::getScan();
    pScan->setAdvertisedDeviceCallbacks(&g_scanCallbacks, false);
    pScan->setActiveScan(true);
    pScan->setDuplicateFilter(false);
    pScan->setInterval(80);
    pScan->setWindow(50);
    pScan->setMaxResults(50);

    Serial.println("BLE: Mac 스캔 시작 (Service UUID: 07A98A35-...)");
    isScanning_ = true;
    lastScanStartedAtMs_ = millis();
    pScan->start(5, onScanComplete, false);
}

void TransportBle::onScanEnded() {
    isScanning_ = false;
}

void TransportBle::onDiscovered(NimBLEAdvertisedDevice *advertisedDevice) {
    NimBLEDevice::getScan()->stop();
    isScanning_ = false;
    targetDevice_ = advertisedDevice;
    doConnect_ = true;
}

void TransportBle::handleConnected() {
    NimBLERemoteService *pService = client_->getService(SERVICE_UUID);
    if (pService == nullptr) {
        Serial.println("BLE: 미러 서비스를 찾을 수 없음");
        client_->disconnect();
        return;
    }

    authCh_ = pService->getCharacteristic(AUTH_UUID);
    snapshotCh_ = pService->getCharacteristic(SNAPSHOT_UUID);

    if (authCh_ == nullptr || snapshotCh_ == nullptr) {
        Serial.println("BLE: 필수 특성을 찾을 수 없음");
        client_->disconnect();
        return;
    }

    // Auth 및 Snapshot 특성 Notify 사전 구독 (GATT 등록)
    const bool authSub = authCh_->subscribe(true, [](NimBLERemoteCharacteristic *pCh, uint8_t *pData, size_t length, bool isNotify) {
        if (g_activeBleInstance != nullptr) {
            g_activeBleInstance->handleAuthNotify(pData, length);
        }
    });
    Serial.printf("BLE: Auth 특성 구독 %s\n", authSub ? "성공" : "실패");

    const bool snapSub = snapshotCh_->subscribe(true, [](NimBLERemoteCharacteristic *pCh, uint8_t *pData, size_t length, bool isNotify) {
        if (g_activeBleInstance != nullptr) {
            g_activeBleInstance->handleSnapshotChunk(pData, length);
        }
    });
    Serial.printf("BLE: Snapshot 특성 구독 %s\n", snapSub ? "성공" : "실패");
    Serial.printf("BLE: client_->getMTU() = %u, NimBLEDevice::getMTU() = %u\n",
                  client_ != nullptr ? client_->getMTU() : 0, NimBLEDevice::getMTU());

    Serial.println("BLE: Auth & Snapshot 특성 구독 완료 — 핸드셰이크 시작");

    // 핸드셰이크 시작
    resetAuth();
    if (config_ != nullptr && configIsPaired(*config_)) {
        authStep_ = AuthStep::SendAuth2;
        sendVerb("AUTH2:", myPublic_, 32);
    } else {
        // 페어링되지 않은 경우: 즉시 HELLO2를 보내 Mac의 AwaitingCode2를 미리 받아둔다!
        authStep_ = AuthStep::NeedsPairing;
        sendVerb("HELLO2:", myPublic_, 32);
    }
}

void TransportBle::handleDisconnected() {
    authCh_ = nullptr;
    snapshotCh_ = nullptr;
    hasSnapshot_ = false;
    hasAwaitingParams_ = false;
    doConnect_ = false;
    targetDevice_ = nullptr;
    delete channel_;
    channel_ = nullptr;

    const bool hasStoredToken = (config_ != nullptr && configIsPaired(*config_));
    const bool hasCode = (pendingCode_.length() == 6);
    authStep_ = authInitialStep(hasStoredToken, hasCode);
}

void TransportBle::loop() {
    if (!wantsRunning_) {
        return;
    }

    // 스캔에서 장치 발견 시 메인 루프에서 안전하게 GATT 연결 시도
    if (doConnect_ && targetDevice_ != nullptr) {
        doConnect_ = false;
        if (client_ == nullptr) {
            client_ = NimBLEDevice::createClient();
            client_->setClientCallbacks(&g_clientCallbacks, false);
            client_->setConnectionParams(12, 12, 0, 400); // 빠른 통신 파라미터
        }

        Serial.printf("BLE: %s 에 연결 시도 중...\n", targetDevice_->getAddress().toString().c_str());
        connectStartedAtMs_ = millis();
        if (!client_->connect(targetDevice_)) {
            Serial.println("BLE: 연결 실패, 재스캔 예정");
            handleDisconnected();
        }
        targetDevice_ = nullptr;
        return;
    }

    // 메인 루프에서 Notify 처리 (NVS/Flash 충돌 방지)
    if (hasPendingNotify_) {
        hasPendingNotify_ = false;
        String text = pendingNotifyText_;
        pendingNotifyText_ = "";

        ReplyView reply;
        if (!parseReplyJson(text, reply)) {
            authStep_ = AuthStep::Failed;
        } else {
            const bool hasStoredToken = (config_ != nullptr && configIsPaired(*config_));
            const bool hasCode = (pendingCode_.length() == 6);
            AuthStep next = authOnReply(reply, hasStoredToken, hasCode);

            if (reply.hasLeft) {
                attemptsLeft_ = reply.left;
            }

            if (next == AuthStep::SendCode2 || (reply.v && reply.await == "code")) {
                uint8_t spk[32];
                uint8_t nonce[16];
                if (hexDecode(reply.epk, spk, sizeof(spk)) && hexDecode(reply.nonce, nonce, sizeof(nonce))) {
                    if (v2X25519(mySecret_, spk, pendingSs_)) {
                        memcpy(pendingNonceBytes_, nonce, sizeof(nonce));
                        memcpy(pendingSpk_, spk, sizeof(spk));
                        hasAwaitingParams_ = true;
                        Serial.println("BLE: AwaitingCode2 수신 — 페어링 키패드 입력 대기");
                    }
                }
                if (hasCode) {
                    authStep_ = AuthStep::SendCode2;
                    sendCode2(reply);
                } else {
                    authStep_ = AuthStep::NeedsPairing;
                }
            } else if (next == AuthStep::SendProof2) {
                authStep_ = next;
                sendProof2(reply);
            } else if (next == AuthStep::Subscribed) {
                authStep_ = next;
                finishHandshake(reply);
            } else if (next == AuthStep::NeedsPairing) {
                authStep_ = next;
                pendingCode_ = "";
                hasAwaitingParams_ = false;
                if (config_ != nullptr && (reply.hasLeft || !reply.ok)) {
                    configClearToken(*config_);
                }
                // 사용자가 코드를 제출했으나 틀린 경우(Denied)에만 다음 입력을 위해 HELLO2 재전송
                if (reply.hasLeft && !reply.ok && attemptsLeft_ > 0 && isConnected()) {
                    Serial.printf("BLE: 코드 불일치(남은 횟수: %d) -> 다음 시도를 위해 HELLO2 재요청\n", attemptsLeft_);
                    resetAuth();
                    sendVerb("HELLO2:", myPublic_, 32);
                }
            } else {
                authStep_ = next;
            }
        }
    }

    // 메인 루프에서 Snapshot Chunk 큐 순차 처리
    std::vector<std::vector<uint8_t>> chunksToProcess;
    {
        std::lock_guard<std::mutex> lock(chunkMutex_);
        if (!pendingChunkQueue_.empty()) {
            chunksToProcess = std::move(pendingChunkQueue_);
            pendingChunkQueue_.clear();
        }
    }

    for (const auto &chunk : chunksToProcess) {
        processSnapshotChunk(chunk.data(), chunk.size());
    }

    // 45초 스냅샷 타임아웃 감지
    if (authStep_ == AuthStep::Subscribed && isConnected()) {
        if (hasSnapshot_ && (millis() - lastSnapshotAtMs_ > BLE_SNAPSHOT_TIMEOUT_MS)) {
            Serial.println("BLE: 45초 동안 스냅샷 수신 없음 -> 재연결 유도");
            hasSnapshot_ = false;
            if (client_ != nullptr) {
                client_->disconnect();
            }
            return;
        }
    }

    // 미연결 시 주기적 스캔 재시도
    if (!isConnected() && !isScanning_) {
        if (millis() - lastScanStartedAtMs_ > SCAN_INTERVAL_MS) {
            startScan();
        }
    }
}

bool TransportBle::isConnected() const {
    return (client_ != nullptr && client_->isConnected());
}

void TransportBle::submitCode(const String &code) {
    pendingCode_ = code;
    if (hasAwaitingParams_ && isConnected() && authCh_ != nullptr) {
        // 공유 비밀 pendingSs_ 계산 보장
        if (!v2X25519(mySecret_, pendingSpk_, pendingSs_)) {
            authStep_ = AuthStep::Failed;
            v2Wipe(pendingSs_, sizeof(pendingSs_));
            return;
        }

        // Transcript 및 CodeBinding 계산 및 전송
        uint8_t tr[64];
        v2Transcript(myPublic_, pendingSpk_, tr);

        uint8_t cbind[32];
        v2CodeBinding(pendingCode_.c_str(), tr, cbind);

        authStep_ = AuthStep::SendCode2;
        sendVerb("CODE2:", cbind, sizeof(cbind));
        v2Wipe(cbind, sizeof(cbind));
        Serial.println("BLE: CODE2 전송 완료");
    } else if (isConnected() && authCh_ != nullptr) {
        resetAuth();
        sendVerb("HELLO2:", myPublic_, 32);
    } else if (!isConnected()) {
        startScan();
    }
}

void TransportBle::sendVerb(const char *prefix, const uint8_t *data, size_t len) {
    if (authCh_ == nullptr || !isConnected()) {
        return;
    }
    String hex = toHex(data, len);
    String frame = String(prefix) + hex;
    authCh_->writeValue((const uint8_t *)frame.c_str(), frame.length(), true);
}

void TransportBle::handleAuthNotify(const uint8_t *data, size_t length) {
    String text;
    text.concat((const char *)data, length);
    text.trim();
    pendingNotifyText_ = text;
    hasPendingNotify_ = true;
}

void TransportBle::sendCode2(const ReplyView &reply) {
    uint8_t spk[32];
    uint8_t nonce[16];
    if (!hexDecode(reply.epk, spk, sizeof(spk)) || !hexDecode(reply.nonce, nonce, sizeof(nonce))) {
        authStep_ = AuthStep::Failed;
        return;
    }

    if (!v2X25519(mySecret_, spk, pendingSs_)) {
        authStep_ = AuthStep::Failed;
        v2Wipe(pendingSs_, sizeof(pendingSs_));
        return;
    }
    memcpy(pendingNonceBytes_, nonce, sizeof(nonce));

    uint8_t tr[64];
    v2Transcript(myPublic_, spk, tr);

    uint8_t cbind[32];
    v2CodeBinding(pendingCode_.c_str(), tr, cbind);

    sendVerb("CODE2:", cbind, sizeof(cbind));
    v2Wipe(cbind, sizeof(cbind));
}

void TransportBle::sendProof2(const ReplyView &reply) {
    uint8_t spk[32];
    uint8_t nonce[16];
    if (!hexDecode(reply.epk, spk, sizeof(spk)) || !hexDecode(reply.nonce, nonce, sizeof(nonce))) {
        authStep_ = AuthStep::Failed;
        return;
    }

    uint8_t ss[32];
    if (!v2X25519(mySecret_, spk, ss)) {
        authStep_ = AuthStep::Failed;
        v2Wipe(ss, sizeof(ss));
        return;
    }

    uint8_t tr[64];
    v2Transcript(myPublic_, spk, tr);

    uint8_t tokenBytes[16];
    if (!hexDecode(config_->token, tokenBytes, sizeof(tokenBytes))) {
        authStep_ = AuthStep::Failed;
        v2Wipe(ss, sizeof(ss));
        return;
    }

    uint8_t proof[32];
    v2SessionProof(tokenBytes, sizeof(tokenBytes), nonce, sizeof(nonce), tr, proof);
    v2DeriveSessionKeys(ss, tokenBytes, sizeof(tokenBytes), nonce, sizeof(nonce), pendingS2c_, pendingC2s_);

    sendVerb("PROOF2:", proof, sizeof(proof));

    v2Wipe(ss, sizeof(ss));
    v2Wipe(tokenBytes, sizeof(tokenBytes));
    v2Wipe(proof, sizeof(proof));
}

void TransportBle::finishHandshake(const ReplyView &reply) {
    if (reply.sealed.length() > 0) {
        uint8_t pairKey[32];
        v2DerivePairKey(pendingSs_, pendingNonceBytes_, sizeof(pendingNonceBytes_), pairKey);

        const size_t sealedLen = reply.sealed.length() / 2;
        uint8_t sealedBuf[128];
        uint8_t tokenJson[128];
        if (sealedLen < 24 || sealedLen > sizeof(sealedBuf) || !hexDecode(reply.sealed, sealedBuf, sealedLen)) {
            authStep_ = AuthStep::Failed;
            v2Wipe(pairKey, sizeof(pairKey));
            v2Wipe(pendingSs_, sizeof(pendingSs_));
            return;
        }

        SealedChannel pairChannel(pairKey, pairKey);
        size_t tokenJsonLen = 0;
        if (!pairChannel.open(sealedBuf, sealedLen, tokenJson, &tokenJsonLen) || tokenJsonLen >= sizeof(tokenJson)) {
            authStep_ = AuthStep::Failed;
            v2Wipe(pairKey, sizeof(pairKey));
            v2Wipe(pendingSs_, sizeof(pendingSs_));
            v2Wipe(sealedBuf, sizeof(sealedBuf));
            v2Wipe(tokenJson, sizeof(tokenJson));
            return;
        }
        tokenJson[tokenJsonLen] = '\0';

        JsonDocument doc;
        if (deserializeJson(doc, (const char *)tokenJson) != DeserializationError::Ok) {
            authStep_ = AuthStep::Failed;
            v2Wipe(pairKey, sizeof(pairKey));
            v2Wipe(pendingSs_, sizeof(pendingSs_));
            v2Wipe(tokenJson, sizeof(tokenJson));
            return;
        }
        String newToken = String((const char *)(doc["token"] | ""));

        if (!configSaveToken(*config_, newToken)) {
            authStep_ = AuthStep::Failed;
            v2Wipe(pairKey, sizeof(pairKey));
            v2Wipe(pendingSs_, sizeof(pendingSs_));
            v2Wipe(tokenJson, sizeof(tokenJson));
            return;
        }

        uint8_t tokenBytes[16];
        hexDecode(config_->token, tokenBytes, sizeof(tokenBytes));

        uint8_t newS2c[32];
        uint8_t newC2s[32];
        v2DeriveSessionKeys(pendingSs_, tokenBytes, sizeof(tokenBytes), pendingNonceBytes_,
                            sizeof(pendingNonceBytes_), newS2c, newC2s);

        delete channel_;
        channel_ = new SealedChannel(newC2s, newS2c);

        v2Wipe(pairKey, sizeof(pairKey));
        v2Wipe(pendingSs_, sizeof(pendingSs_));
        v2Wipe(tokenBytes, sizeof(tokenBytes));
        v2Wipe(newS2c, sizeof(newS2c));
        v2Wipe(newC2s, sizeof(newC2s));
        v2Wipe(tokenJson, sizeof(tokenJson));

        pendingCode_ = "";
        Serial.println("BLE: 새로 페어링 완료 — 토큰 저장됨");
    } else {
        delete channel_;
        channel_ = new SealedChannel(pendingC2s_, pendingS2c_);
        v2Wipe(pendingS2c_, sizeof(pendingS2c_));
        v2Wipe(pendingC2s_, sizeof(pendingC2s_));
        Serial.println("BLE: 기존 토큰으로 인가됨");
    }

    authStep_ = AuthStep::Subscribed;
    Serial.println("BLE: E2EE 인가 완료! (Subscribed)");
}

void TransportBle::handleSnapshotChunk(const uint8_t *data, size_t length) {
    if (length < 3) return;
    std::vector<uint8_t> item(data, data + length);
    std::lock_guard<std::mutex> lock(chunkMutex_);
    pendingChunkQueue_.push_back(std::move(item));
}

void TransportBle::processSnapshotChunk(const uint8_t *data, size_t length) {
    if (length < 3) return;
    const uint8_t frameId = data[0];
    const uint8_t chunkIdx = data[1];
    const uint8_t chunkCount = data[2];
    if (chunkCount == 0) return;

    if (chunkIdx == 0) {
        currentFrameId_ = frameId;
        totalChunks_ = chunkCount;
        expectedChunkIdx_ = 1;
        reassemblyBuf_.clear();
        reassemblyBuf_.insert(reassemblyBuf_.end(), data + 3, data + length);
        reassemblyAborted_ = false;
    } else {
        if (reassemblyAborted_ || frameId != currentFrameId_ || chunkIdx != expectedChunkIdx_ || chunkCount != totalChunks_) {
            Serial.printf("BLE: 청크 순서 불일치/이탈로 프레임 폐기 (frame=%u, idx=%u, expected=%u, total=%u)\n",
                          frameId, chunkIdx, expectedChunkIdx_, totalChunks_);
            reassemblyAborted_ = true;
            return;
        }
        reassemblyBuf_.insert(reassemblyBuf_.end(), data + 3, data + length);
        expectedChunkIdx_++;
    }

    if (!reassemblyAborted_ && expectedChunkIdx_ == totalChunks_) {
        // 프레임 조립 완료 -> 복호화
        Serial.printf("BLE: 프레임 조립 완료 (%u 바이트, frameId=%u) -> 복호화 시도\n",
                      (unsigned)reassemblyBuf_.size(), frameId);
        if (channel_ == nullptr) {
            Serial.println("BLE: 복호화 불가 — SealedChannel 미설정(인증 미완료)");
        } else if (reassemblyBuf_.size() < 24) {
            Serial.printf("BLE: 복호화 실패 — 봉인 프레임 길이 부족(%u 바이트)\n", (unsigned)reassemblyBuf_.size());
        } else if (reassemblyBuf_.size() > MAX_SNAPSHOT_FRAME_BYTES) {
            Serial.printf("BLE: 복호화 실패 — 봉인 프레임 크기 초과(%u 바이트)\n", (unsigned)reassemblyBuf_.size());
        } else {
            std::vector<uint8_t> plain(reassemblyBuf_.size() + 1, 0);
            size_t plainLen = 0;
            if (!channel_->open(reassemblyBuf_.data(), reassemblyBuf_.size(), plain.data(), &plainLen)) {
                Serial.println("BLE: 복호화 실패 — AEAD Poly1305 인증 태그 불일치 (세션 키 또는 카운터 어긋남)");
            } else {
                plain[plainLen] = '\0';
                Snapshot snap;
                if (!snapshotParse(plain.data(), plainLen, snap)) {
                    Serial.printf("BLE: 스냅샷 JSON 파싱 실패 (평문 %u 바이트)\n", (unsigned)plainLen);
                } else {
                    latestSnapshot_ = snap;
                    hasSnapshot_ = true;
                    lastSnapshotAtMs_ = millis();
                    Serial.printf("BLE: ★ 스냅샷 수신 및 복호화 성공! (agentCount=%u, emittedAt=%llu)\n",
                                  (unsigned)snap.agentCount, (unsigned long long)snap.emittedAtEpochSec);
                }
            }
        }
        reassemblyBuf_.clear();
    }
}

void TransportBle::stop() {
    wantsRunning_ = false;
    if (isScanning_) {
        NimBLEDevice::getScan()->stop();
        isScanning_ = false;
    }
    if (client_ != nullptr) {
        if (client_->isConnected()) {
            client_->disconnect();
        }
    }
}
