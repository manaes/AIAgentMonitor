// AI Agent Monitor — CYD 프로토콜 펌웨어 (Task 14b: 페어링 키패드 화면)
//
// Task 8 이 확인한 것: 툴체인 · 보드 id · 업로드 경로. Task 9 가 더한 것: 설정
// 저장과 WiFi 포털. Task 10~11 이 만든 것: E2EE v2 암호 계층과 인증 상태
// 기계 — 둘 다 순수 모듈이라 여기 연결되지 않은 채였다. Task 12 가 그 둘을
// 실제 소켓(`Transport`)에 이어붙였다 — 이 프로젝트가 처음으로 `loop()` 안에서
// 블로킹 호출을 실행하는 지점. Task 14a 가 디스플레이·터치를 처음 켰다(LVGL,
// 정적인 "연결 중" 한 줄, 터치 좌표 디버그 콜백) — 위젯도 페어링 로직도 없는
// 순수 브링업이었다.
//
// **Task 14b 가 그 위에 처음으로 진짜 UI 로직을 얹는다.** `ui_pairing.h/.cpp`
// (신규, 브리프의 Files 목록)가 6자리 코드 키패드 화면을 만들고,
// `transport.begin(config)` 을 부르기도 전에 `main.cpp` 가 그 화면을 켠다 —
// `Transport::authStep_` 의 멤버 초기값이 이미 `NeedsPairing` 이라 안전하다
// (`ui_pairing.cpp`, `transport.h` 참고). 이 파일 자체가 브리프의 Files
// 목록에는 없었지만, Task 14a 의 정적 화면을 실제 키패드로 바꿔 끼우려면
// (그리고 `uiPairingUpdate()` 를 매 loop() 마다 불러야) 반드시 필요했다 —
// Task 14a 가 `font_ko.h` 를 브리프 밖에서 만들어야 했던 것과 같은 처지다.
#include <Arduino.h>
#include <WiFi.h>
#include <esp32_smartdisplay.h>

#include "config.h"
#include "transport.h"
#include "font_ko.h"
#include "ui_pairing.h"

// **위 `#include "font_ko.h"` 자체가 실질적인 코드다, 장식이 아니다.**
// PlatformIO 의 chain LDF 는 소스의 `#include` 문을 보고 어떤 `lib/<name>/`
// 를 빌드에 끌어올지 정한다 — Task 13 은 헤더 없이 `font_ko.c` 만 만들어
// 뒀는데(그 시점엔 아무도 참조하지 않아 문제가 없었다), 여기서
// `extern "C" const lv_font_t font_ko;` 선언만 적어 봤더니(헤더 없이)
// `#include` 가 없으니 LDF 가 `font_ko.c` 를 아예 컴파일 대상에서 빼 버려서
// `undefined reference to font_ko` 링크 에러가 실제로 났다(T14a 실측). 그래서
// `lib/font_ko/font_ko.h` 를 새로 만들어 그 선언을 옮기고 여기서 include
// 한다 — 브리프의 Files 목록엔 없던 파일이지만, `main.cpp` 를 브리프대로
// 고치려면 반드시 필요했다.

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

// ─────────────────────────────────────────────────────────────────────────────
// T14a-A — lv_tick_inc/lv_timer_handler 계측.
//
// esp32_smartdisplay README(Step 7)의 예시 그대로 `lv_last_tick` 을 들고
// 있다가 매 loop() 마다 경과 ms 를 넘긴다. `lvglMaxHandlerUs` 는 하트비트
// 주기(5초)마다 `lv_timer_handler()` 호출 하나하나의 소요 시간 중 최댓값을
// 들고 있다가 하트비트 줄에 같이 찍고 0 으로 되돌린다 — 매 호출(대략
// 1ms 주기, delay(1) 때문에 초당 수백 번)을 전부 찍으면 시리얼이 그 자체로
// 30초 예산을 위협하는 부담이 되므로, Task 12 가 세운 "loop 한 바퀴 최장
// 시간을 시리얼로 남긴다" 관례를 그대로 따른다.
static uint32_t lastLvglTickMs = 0;
static uint32_t lvglMaxHandlerUs = 0;

// T14a-B — 터치 확인용 최소 콜백. 키패드/위젯 없이 좌표만 시리얼에 찍는다.
//
// **다음 화면(Task 14b 키패드)을 만들기 전에 볼 것 — X 좌표 off-by-one.**
// `esp32_smartdisplay@2.1.1` 의 `src/esp_lcd_touch.c:92`(설치된 패키지
// 소스로 확인)가 소프트웨어 미러-X 좌표를 `x_max - x[i]` 로 계산한다
// (`-1` 없음) — 이 보드가 `TOUCH_MIRROR_X=true` 라서 화면의 물리적 왼쪽
// 가장자리를 누르면 `x=240`(유효 범위 `[0,239]` 밖)이 나온다(실기 재현
// 확인됨). 오른쪽 가장자리에 버튼을 배치하기 전에
// `.superpowers/sdd/2026-08-25-cyd-client/task-14a-report.md` "발견한
// 버그 2" 절과 `task-14b-brief.md` 상단 캐비어트를 먼저 읽어라.
//
// `esp32_smartdisplay` 는 등록한 `lv_indev_t*` 를 공개 헤더(`esp32_smartdisplay.h`)
// 로 노출하지 않는다(소스 확인: `esp32_smartdisplay.c` 의 `lv_indev_t *indev;`
// 는 파일 스코프 전역이지 extern 선언이 아니다) — 그래서 이 파일에서 직접
// 그 포인터를 얻을 수 없고, LVGL 의 표준 API로 등록된 입력장치 목록을 순회해
// 찾아야 한다(`setup()` 참고). 콜백 안에서는 `lv_indev_active()`(LVGL v9
// 공개 API, "Can be used in action functions too" — `lvgl/src/indev/lv_indev.h`)
// 로 지금 이 이벤트를 일으킨 입력장치를 되찾는다.
static void onTouchEvent(lv_event_t *e) {
    (void)e;
    lv_indev_t *indev = lv_indev_active();
    if (indev == nullptr) {
        return;
    }
    lv_point_t point;
    lv_indev_get_point(indev, &point);
    Serial.printf("touch: x=%d y=%d\n", point.x, point.y);
}

// T14a 가 만든 디버그 로거를 그대로 남긴다 — 좌표를 시리얼에 찍는 것은
// 페어링 키패드(Task 14b, `ui_pairing.cpp`)가 실제로 위젯을 눌렀을 때도
// 여전히 유용하다(특히 오른쪽 열 버튼의 가장자리를 눌렀을 때 XPT2046
// 소프트웨어 미러 X 좌표 off-by-one 이 재현되는지 확인할 때 —
// `task-14b-brief.md` 상단 캐비어트, `ui_pairing.cpp` 의 대응 옵션 주석
// 참고). LV_EVENT_PRESSED 는 눌리는 "순간"에 한 번만 온다(누르고 있는
// 동안 계속 오는 LV_EVENT_PRESSING 이 아니다).
static void registerTouchDebugLogger() {
    // 등록된 포인터형(터치) 입력장치를 전부 찾아 콜백을 건다 — 이 보드는
    // XPT2046 저항막 터치 하나뿐이라 보통 한 번만 걸린다.
    lv_indev_t *indev = nullptr;
    while ((indev = lv_indev_get_next(indev)) != nullptr) {
        if (lv_indev_get_type(indev) == LV_INDEV_TYPE_POINTER) {
            lv_indev_add_event_cb(indev, onTouchEvent, LV_EVENT_PRESSED, nullptr);
        }
    }
}

void setup() {
    Serial.begin(115200);
    delay(300);                       // USB 시리얼이 붙을 시간
    Serial.println();
    Serial.printf("chip=%s rev=%d cores=%d\n",
                  ESP.getChipModel(), ESP.getChipRevision(), ESP.getChipCores());
    Serial.printf("flash=%u bytes  free heap=%u\n",
                  ESP.getFlashChipSize(), ESP.getFreeHeap());
    Serial.println("AI Agent Monitor — CYD 프로토콜 펌웨어 (Task 14b: 페어링 키패드 화면)");

    // ─────────────────────────────────────────────────────────────────────
    // T14a — 디스플레이·터치 브링업. WiFi 연결/포털보다 먼저 켠다: 포털이
    // 열리는 동안(사람이 휴대폰으로 공유기를 골라야 하는 몇 분)에도 화면에
    // 뭔가 떠 있는 편이, "화면이 꺼진 채 멈춘 것처럼 보이는 기기" 보다 낫다.
    //
    // Task 14b 부터는 그 "뭔가"가 정적인 한 줄이 아니라 페어링 키패드
    // 자체다(`uiPairingCreate`) — `transport.begin(config)` 가 아직 안
    // 불렸어도 안전하다: `Transport::authStep_` 의 멤버 초기값이 이미
    // `NeedsPairing`(transport.h)이라, 이 시점의 `authStep()` 은 "아직 뭘
    // 모른다" 가 아니라 "코드가 필요하다" 는 이 화면이 그대로 그릴 수 있는
    // 값이다. WiFi 포털이 열려 있는 동안은(그 함수가 블로킹이라) 이 화면도
    // 같이 멈춰 있다 — Task 14a 때부터 있던 한계이고 이 태스크가 고치는
    // 범위가 아니다.
    smartdisplay_init();
    lastLvglTickMs = millis();
    registerTouchDebugLogger();
    uiPairingCreate(transport);

    // T14a-A — "부팅 직후 최초 전체 리프레시" 시간. 위젯을 막 만든 직후라
    // 화면 전체가 dirty 상태이므로, 이 첫 lv_timer_handler() 호출이 곧 첫
    // 전체 리프레시다. loop() 의 계측(아래)과 같은 micros() 방식을 쓴다.
    // Task 14b 가 위젯 수를 늘렸으므로(라벨 4개 + 버튼매트릭스 12버튼)
    // 이 값도 다시 재야 한다 — 보고서의 재측정 절 참고.
    const uint32_t firstRefreshStartUs = micros();
    lv_timer_handler();
    const uint32_t firstRefreshUs = micros() - firstRefreshStartUs;
    Serial.printf(
        "lvgl: 최초 전체 리프레시 %u us (30초 예산 대비 %.4f%%)\n",
        firstRefreshUs, firstRefreshUs / 300000.0);

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

    // T14a-A — LVGL 티커 갱신과 화면 그리기. esp32_smartdisplay README(Step 7)
    // 가 보인 그대로: 경과 ms 를 lv_tick_inc() 에 넘기고 매 루프 lv_timer_handler()
    // 를 부른다.
    //
    // **정정(fix round 1) — "transport.loop() 와 아직 안 엮여 있다"는 이전
    // 버전의 이 주석은 틀렸다.** `transport.loop()`(아래, Task 12 부터 있던
    // 코드)는 **이미 이 같은 `loop()` 함수 안에서 순차 실행 중**이다 — 배선이
    // 안 된 게 아니다. 아직 안 된 것은 **양쪽이 동시에 무거운 순간의 최악
    // 케이스를 실측하는 일**이다(예: WebSocket 재연결 백오프가 걸린 순간에
    // 화면도 무거운 리드로우를 하는 조합). 지금까지의 계측(아래 최댓값)은
    // 화면 그리기 하나만 격리해서 잰 값이다. `setup()` 의 최초 전체
    // 리프레시(~52ms)는 `transport.loop()` 가 한 번도 불리기 전(부팅 중)에
    // 끝나므로 애초에 경쟁하지 않는다 — 매 `loop()` 반복에서 실제로
    // `transport.loop()` 와 나란히 도는 건 이 정상 상태 값(400~600us 대,
    // 태스크 보고서 T14a-A)이다. `transport.loop()` 의 최악 시간(Task 12
    // 문서화 기준 8초)과 이 값을 단순 합산하면 30초 예산 안에 들지만,
    // 둘이 **같은 순간에** 최악을 찍는 조합은 아직 실측된 적이 없다 —
    // Task 14b/15 가 위젯을 늘릴 때 다시 재야 한다.
    // Task 14b — 화면 내용을 이번 프레임에 그리기 전에 최신 상태로 맞춘다.
    // 위젯을 다시 만들지 않고 텍스트/버튼 상태만 갱신하므로(T12-D, 깜빡임
    // 방지) 매 loop() 마다 불러도 싸다.
    uiPairingUpdate(transport);

    lv_tick_inc(now - lastLvglTickMs);
    lastLvglTickMs = now;

    const uint32_t lvglHandlerStartUs = micros();
    lv_timer_handler();
    const uint32_t lvglHandlerUs = micros() - lvglHandlerStartUs;
    if (lvglHandlerUs > lvglMaxHandlerUs) {
        lvglMaxHandlerUs = lvglHandlerUs;
    }

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
            Serial.printf(
                "alive  heap=%u wifi=up rssi=%d authStep=%d authorized=%s lvglMaxUs=%u\n",
                ESP.getFreeHeap(), WiFi.RSSI(), (int)transport.authStep(),
                transport.authorized() ? "yes" : "no", lvglMaxHandlerUs);
        } else {
            Serial.printf("alive  heap=%u wifi=down lvglMaxUs=%u\n",
                          ESP.getFreeHeap(), lvglMaxHandlerUs);
        }
        // 다음 5초 구간의 최댓값을 새로 잰다 — 하트비트 간격마다 리셋.
        lvglMaxHandlerUs = 0;
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
