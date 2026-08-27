// AI Agent Monitor — CYD 프로토콜 펌웨어 (Task 12: WebSocket 연결과 mDNS 발견)
//
// Task 8 이 확인한 것: 툴체인 · 보드 id · 업로드 경로. Task 9 가 더한 것: 설정
// 저장과 WiFi 포털. Task 10~11 이 만든 것: E2EE v2 암호 계층과 인증 상태
// 기계 — 둘 다 순수 모듈이라 여기 연결되지 않은 채였다. Task 12 가 그 둘을
// 실제 소켓(`Transport`)에 이어붙인다 — 이 프로젝트가 처음으로 `loop()` 안에서
// 블로킹 호출을 실행하는 지점이다. 화면은 여전히 없다(Task 13~14).

#include <Arduino.h>
#include <WiFi.h>

#include "config.h"
#include "transport.h"

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
// **이 예산은 `loop()` 에 걸리는 제약이다.** `setup()` 은 WebSocket 이 붙기
// 전에 한 번 돌므로 여기 해당하지 않는다 — 아래 T9-D 가 그 예외를 어디까지만
// 쓰는지 적어 뒀고, 그 범위를 넓히지 마라.
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

// 이 기기의 설정. Task 13(페어링)이 토큰을 채우고 Task 9(포털)가 맥 주소를 쓴다.
static Config config;

// 맥과의 WebSocket 연결 — mDNS 발견, 재연결 백오프, v2 인증 핸드셰이크.
static Transport transport;

void setup() {
    Serial.begin(115200);
    delay(300);                       // USB 시리얼이 붙을 시간
    Serial.println();
    Serial.printf("chip=%s rev=%d cores=%d\n",
                  ESP.getChipModel(), ESP.getChipRevision(), ESP.getChipCores());
    Serial.printf("flash=%u bytes  free heap=%u\n",
                  ESP.getFlashChipSize(), ESP.getFreeHeap());
    Serial.println("AI Agent Monitor — CYD 프로토콜 펌웨어 (화면 없음)");

    // `stored` 와 `paired` 를 따로 찍는다. 둘은 다른 질문이고(config.h 참고),
    // Task 14 는 이 조합으로 초기 설정 안내와 페어링 키패드를 갈라 띄운다.
    // **토큰 값 자체는 찍지 않는다** — config.cpp 의 금지 목록.
    const bool stored = configLoad(config);
    Serial.printf("config: stored=%s paired=%s machost=\"%s\"\n",
                  stored ? "yes" : "no",
                  configIsPaired(config) ? "yes" : "no",
                  config.macHost.c_str());

    // ─────────────────────────────────────────────────────────────────────────
    // T9-D — `wifiConnectOrPortal()` 이 false 를 돌려주면 무엇을 하는가:
    // **부팅 중 이 자리에서, 성공할 때까지 다시 부른다.** 근거 셋.
    //
    // 1. 연결 없이 할 수 있는 일이 없다. 이 펌웨어의 존재 이유가 맥에 붙는
    //    것이고, 화면이 붙는 Task 14 이전에는 "WiFi 없음" 을 보여 줄 곳조차 없다.
    //    그러니 여기서 포기하고 `loop()` 로 내려가 봐야 아무 일도 하지 않는
    //    루프만 돈다.
    //
    // 2. `ESP.restart()` 로 되돌리지 않는다. 재부팅해도 결국 같은 코드가 같은
    //    자리에서 다시 돌 뿐인데, 대가로 시리얼 로그가 날아간다 — 화면 없는 이
    //    기기에서 무슨 일이 있었는지 알 수 있는 유일한 통로다. 실패 횟수를 세어
    //    찍는 편이 재부팅보다 남는 정보가 많다.
    //
    // 3. **여기서 블로킹해도 되는 이유는 아직 WebSocket 이 없다는 것 하나뿐이다.**
    //    위 30초 예산은 `loop()` 에 걸리는 제약이고 `setup()` 은 소켓이 붙기 전에
    //    한 번 돈다. Task 12 이후 런타임에 포털을 다시 열게 된다면 이 루프를 그대로
    //    옮겨 쓰면 안 된다 — 소켓을 명시적으로 끊고 열었다가, 돌아온 뒤 다시 붙여야
    //    한다. 끊지 않고 열면 맥은 90초 뒤 그 연결을 죽은 것으로 처리한다.
    //
    // 한 바퀴는 무한정이 아니다: 연결 시도와 포털에 각각 시간 제한이 걸려 있어서
    // (config.cpp 의 T9-C) 실패해도 반드시 돌아오고, 돌아오면 다시 시도한다.
    // ─────────────────────────────────────────────────────────────────────────
    //
    // 힙을 같이 찍는 이유: 이 루프는 한 바퀴마다 `WiFiManager`(+DNS 서버 +
    // 웹서버)를 새로 만들고 부수는데 상한이 없다. 그리고 하트비트의 `heap=` 은
    // `setup()` 이 끝나야 시작되므로 **이 루프 안에서는 힙이 전혀 안 보인다** —
    // 화면 없는 기기가 유일하게 오래 머무를 수 있는 자리인데 계측이 0 이 된다.
    // 누수가 입증된 것은 아니다. 누수가 생기면 보이게 해 두는 것뿐이다.
    uint32_t attempts = 0;
    while (!wifiConnectOrPortal(config)) {
        ++attempts;
        Serial.printf("wifi: 연결도 설정도 못 했다 (%u번째 실패, heap=%u) — 다시 시도한다\n",
                      attempts, ESP.getFreeHeap());
    }

    Serial.printf("wifi: 연결됨 ssid=\"%s\" ip=%s rssi=%d\n",
                  WiFi.SSID().c_str(), WiFi.localIP().toString().c_str(),
                  WiFi.RSSI());

    // WebSocket 이 붙기 전, 부팅 중 한 번만. `MDNS.begin()` 도 여기서 딱 한
    // 번만 불린다(T12-B, `transport.h` 상단 주석).
    transport.begin(config);
}

void loop() {
    const uint32_t now = millis();

    // 부호 없는 뺄셈이라 millis() 가 약 49.7일에 한 번 넘어가도 그대로 맞다.
    if (now - lastHeartbeatMs >= HEARTBEAT_INTERVAL_MS) {
        lastHeartbeatMs = now;

        // WiFi 상태를 같이 찍는다. 화면이 없는 동안 이 줄이 유일한 진단 통로다.
        //
        // **여기에 재접속 로직은 없다.** 링크가 끊기면 arduino-esp32 의 WiFi
        // 라이브러리가 스스로 다시 붙으려 한다(`_autoReconnect` 기본값이 true 이고
        // `_isReconnectableReason(reason)` 인 끊김에 한해 재시도한다,
        // `WiFiGeneric.cpp:1084`). **Task 12(이 태스크)도 그 재시도로 돌아오지
        // 못하는 경우(공유기가 아예 사라졌거나 비밀번호가 바뀐 경우)를 다루지
        // 않는다** — Task 9 의 이전 버전은 이 자리에서 그것을 Task 12 의 몫으로
        // 적어 뒀지만, 여기서 여는 것은 WebSocket 소켓이지 WiFi 포털이 아니다.
        // 포털을 런타임에 다시 여는 길은 `config.h` 의 `wifiConnectOrPortal`
        // 문서가 이미 "만든다면"으로 조건부로 남겨 둔 미착수 작업이고, 그 상태는
        // 그대로다 — WiFi 가 영영 끊긴 기기는 `Transport` 가 맥에 붙으려 계속
        // 재시도하는 동안(백오프가 캡인 30초로 수렴한다) 시리얼에 `wifi=down` 으로
        // 보이는 것이 오늘의 전부다.
        if (WiFi.isConnected()) {
            Serial.printf("alive  heap=%u wifi=up rssi=%d authStep=%d authorized=%s\n",
                          ESP.getFreeHeap(), WiFi.RSSI(), (int)transport.authStep(),
                          transport.authorized() ? "yes" : "no");
        } else {
            Serial.printf("alive  heap=%u wifi=down\n", ESP.getFreeHeap());
        }
    }

    // Task 12 부터 이 자리가 실제로 블로킹할 수 있다 — 최악의 경우와 그것이
    // 왜 30초 예산 안에 드는지는 `transport.h` 상단 주석. `main.cpp` 는 그
    // 예산에 하트비트(위)와 아래 `delay(1)` 만 더 얹으므로, 이 파일이 그
    // 예산 전체를 진 유일한 곳이다.
    transport.loop();

    // 루프를 완전히 비워 두면 같은 코어의 IDLE 태스크가 굶는다. 1ms 는 위
    // 30초 예산에 아무 영향이 없다.
    delay(1);
}
