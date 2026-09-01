// AI Agent Monitor — CYD 펌웨어: BLE Central 전송 계층 (NimBLE-Arduino)
//
// Mac 의 BleBridge(CBPeripheralManager)에 접속해:
// 1. Service UUID (07A98A35-16C7-4BBA-A296-E28B78B7E683) 스캔 및 연결
// 2. Auth 특성(1403603A-...)으로 HELLO2/CODE2/AUTH2/PROOF2 E2EE 핸드셰이크
// 3. Snapshot 특성(0AE789AA-...)의 Notify 청크를 수신·재조립하여 복호화 및 스냅샷 파싱
#pragma once

#include <Arduino.h>
#include <NimBLEDevice.h>
#include <mutex>

#include "authfsm.h"
#include "config.h"
#include "cryptov2.h"
#include "snapshot.h"

class TransportBle {
  public:
    TransportBle();
    ~TransportBle();

    /// BLE 스캔 및 초기화를 시작한다.
    void begin(Config &config);

    /// 메인 루프에서 주기적으로 호출된다. 연결 유지, 스캔 타이머, 타임아웃을 관리한다.
    void loop();

    /// BLE GATT 연결이 유지되고 있는가.
    bool isConnected() const;

    /// 유효한 스냅샷을 수신한 적이 있는가.
    bool hasSnapshot() const { return hasSnapshot_; }

    /// 현재 E2EE 인증 단계.
    AuthStep authStep() const { return authStep_; }

    /// 페어링 실패 잔여 횟수.
    uint8_t attemptsLeft() const { return attemptsLeft_; }

    /// 키패드 6자리 코드 제출.
    void submitCode(const String &code);

    /// 가장 최근 수신된 스냅샷.
    const Snapshot &latestSnapshot() const { return latestSnapshot_; }

    /// BLE 동작을 멈추고 연결/스캔을 해제한다.
    void stop();

    /// 인증 상태를 초기화하고 재연결을 준비한다.
    void resetAuth();

    void onDiscovered(NimBLEAdvertisedDevice *advertisedDevice);
    void onScanEnded();
    void handleConnected();
    void handleDisconnected();
    void handleAuthNotify(const uint8_t *data, size_t length);
    void handleSnapshotChunk(const uint8_t *data, size_t length);

  private:
    Config *config_ = nullptr;
    NimBLEClient *client_ = nullptr;
    NimBLERemoteCharacteristic *authCh_ = nullptr;
    NimBLERemoteCharacteristic *snapshotCh_ = nullptr;

    AuthStep authStep_ = AuthStep::NeedsPairing;
    uint8_t attemptsLeft_ = 5;

    bool wantsRunning_ = false;
    bool isScanning_ = false;
    bool doConnect_ = false;
    NimBLEAdvertisedDevice *targetDevice_ = nullptr;
    uint32_t lastScanStartedAtMs_ = 0;
    uint32_t connectStartedAtMs_ = 0;

    // E2EE v2 키 및 세션
    uint8_t mySecret_[32] = {0};
    uint8_t myPublic_[32] = {0};
    uint8_t pendingS2c_[32] = {0};
    uint8_t pendingC2s_[32] = {0};
    uint8_t pendingSs_[32] = {0};
    uint8_t pendingNonceBytes_[16] = {0};
    uint8_t pendingSpk_[32] = {0};
    bool hasAwaitingParams_ = false;
    String pendingCode_;
    SealedChannel *channel_ = nullptr;

    // NimBLE Notify 콜백은 별도 호스트 태스크에서 올 수 있다. Arduino String을
    // 콜백과 loop()가 동시에 만지면 힙이 손상될 수 있으므로 바이트 큐로 넘긴다.
    std::vector<std::vector<uint8_t>> pendingNotifyQueue_;
    // 아래 두 값은 loop()만 접근한다. Notify 콜백은 pendingNotifyQueue_에만 쓴다.
    String pendingNotifyText_;
    bool hasPendingNotify_ = false;
    std::vector<std::vector<uint8_t>> pendingChunkQueue_;
    std::mutex notifyMutex_;
    std::mutex chunkMutex_;

    // Auth notify는 기존 한 패킷 JSON 또는 [0xFF, idx, count, payload…] 청크다.
    uint8_t authFrameId_ = 0;
    uint8_t authExpectedChunkIdx_ = 0;
    uint8_t authTotalChunks_ = 0;
    std::vector<uint8_t> authReassemblyBuf_;
    bool authReassemblyAborted_ = false;

    Snapshot latestSnapshot_;
    bool hasSnapshot_ = false;
    uint32_t lastSnapshotAtMs_ = 0;

    // BLE 스냅샷 청크 재조립 버퍼
    uint8_t currentFrameId_ = 0;
    uint8_t expectedChunkIdx_ = 0;
    uint8_t totalChunks_ = 0;
    std::vector<uint8_t> reassemblyBuf_;
    bool reassemblyAborted_ = false;

    void startScan();
    void sendVerb(const char *prefix, const uint8_t *data, size_t len);
    void sendCode2(const ReplyView &reply);
    void sendProof2(const ReplyView &reply);
    void finishHandshake(const ReplyView &reply);
    void processSnapshotChunk(const uint8_t *data, size_t length);

    friend class BleAdvertisedDeviceCallbacks;
    friend class BleClientCallbacks;
};
