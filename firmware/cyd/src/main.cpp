// AI Agent Monitor — CYD 프로토콜 펌웨어 (Task 8: 화면 없음)
//
// 여기서 확인하는 것은 딱 하나다: 툴체인 · 보드 id · 업로드 경로가 맞아서
// 보드가 우리가 올린 코드를 실제로 돌리고 있는가. 화면도 WiFi 도 아직 없다.

#include <Arduino.h>

// ─────────────────────────────────────────────────────────────────────────────
// 제약 — `loop()` 한 바퀴는 30초를 넘겨 블로킹하지 않는다. Task 8~15 전부에 걸린다.
//
// 맥은 30초마다 WebSocket Ping 을 보내고, 이 기기에게서 무언가 받은 지 90초가
// 지나면 사라진 것으로 보고 연결을 놓는다(`src-tauri/src/lan/server.rs` 의
// `PING_INTERVAL` · `IDLE_TIMEOUT`). 그 Ping 에 Pong 을 돌려주는 것은 OS 의 TCP
// 스택이 아니라 `links2004/WebSockets` 라이브러리이고, 그것은 `webSocket.loop()`
// 가 불릴 때만 응답한다. 즉 메인 루프가 오래 블로킹하면 WiFi 도 소켓도 멀쩡한
// 보드가 맥에서 끊긴다 — 그리고 이 기기에는 이유를 설명할 화면도 키보드도 없다.
//
// 예산이 90초가 아니라 30초인 이유: Ping 한 번 놓치는 정도까지만 허용하는 값이다.
// 90초에 바짝 붙여 두면 전체 화면 갱신이 한 번 느려지는 날 곧바로 넘긴다.
//
// 그래서 이 태스크에는 아직 소켓이 없지만 주기를 `delay()` 로 만들지 않는다.
// 브리프 스케치의 `delay(5000)` 은 지금은 무해하지만 Task 12 에서 소켓이 붙는
// 순간 그대로 결함이 되고, "그때 고치기로 한 것" 은 잘 안 고쳐진다. 처음부터
// `millis()` 비교로 써 두면 나중에 고칠 것이 없다.
//
// Task 12 이후 지켜야 할 나머지:
//   - 블로킹이 불가피한 구간(WiFi 재접속, 전체 화면 갱신, NVS 쓰기)은 조각을 내고
//     사이사이에 `webSocket.loop()` 를 부른다.
//   - `loop()` 한 바퀴의 최장 시간을 시리얼로 찍어 둔다. 그러면 이 제약이
//     깨지는 날이 눈에 보인다.
// ─────────────────────────────────────────────────────────────────────────────

// 살아 있다는 신호를 찍는 주기.
static const uint32_t HEARTBEAT_INTERVAL_MS = 5000;

static uint32_t lastHeartbeatMs = 0;

void setup() {
    Serial.begin(115200);
    delay(300);                       // USB 시리얼이 붙을 시간
    Serial.println();
    Serial.printf("chip=%s rev=%d cores=%d\n",
                  ESP.getChipModel(), ESP.getChipRevision(), ESP.getChipCores());
    Serial.printf("flash=%u bytes  free heap=%u\n",
                  ESP.getFlashChipSize(), ESP.getFreeHeap());
    Serial.println("AI Agent Monitor — CYD 프로토콜 펌웨어 (화면 없음)");
}

void loop() {
    const uint32_t now = millis();

    // 부호 없는 뺄셈이라 millis() 가 약 49.7일에 한 번 넘어가도 그대로 맞다.
    if (now - lastHeartbeatMs >= HEARTBEAT_INTERVAL_MS) {
        lastHeartbeatMs = now;
        Serial.printf("alive  heap=%u\n", ESP.getFreeHeap());
    }

    // 루프를 완전히 비워 두면 같은 코어의 IDLE 태스크가 굶는다. 1ms 는 위
    // 30초 예산에 아무 영향이 없고, 이 자리는 나중에 `webSocket.loop()` 등이
    // 들어올 자리다.
    delay(1);
}
