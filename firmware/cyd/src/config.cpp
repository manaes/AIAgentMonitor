#include "config.h"

#include <Preferences.h>
#include <WiFiManager.h>

namespace {

// NVS 네임스페이스와 키. NVS 키 이름은 15자를 넘길 수 없다.
constexpr char NVS_NAMESPACE[] = "aim";
constexpr char KEY_MACHOST[] = "machost";
constexpr char KEY_TOKEN[] = "token";

/// 설정 포털 AP 이름. 스펙과 브리프가 정한 이름이고, 사람이 폰의 WiFi 목록에서
/// 찾아야 하므로 바꾸면 문서와 어긋난다.
constexpr char PORTAL_AP_SSID[] = "AIM-Setup";

/// 저장된 자격증명으로 붙어 보는 시간(초).
///
/// 걸지 않으면 WiFiManager 의 `_connectTimeout` 이 0 이라 arduino-esp32 의
/// `WiFi.waitForConnectResult()` 기본값 60초를 그대로 쓴다. 20초면 가정용
/// 공유기의 결합 + DHCP 에 넉넉하고, 혹시 모자란 날이 있어도 치명적이지 않다 —
/// 실패는 종착점이 아니라 재시도 한 바퀴일 뿐이다(main.cpp 의 T9-D).
constexpr unsigned long WIFI_CONNECT_TIMEOUT_S = 20;

/// 설정 포털을 열어 두는 시간(초). 근거는 아래 T9-C 블록.
constexpr unsigned long PORTAL_TIMEOUT_S = 300;

/// 포털의 맥 주소 입력 칸 길이. WiFiManagerParameter 는 이 길이로 값을
/// `strncpy` 하므로 **넘치는 부분은 조용히 잘린다.** 호스트명(`mac-mini.local`)
/// 이나 IPv4 에는 40자로도 충분하지만, 잘림은 화면 없는 기기에서 원인을 알 수
/// 없는 종류의 고장이라 여유를 둔다.
constexpr int MACHOST_FIELD_LEN = 64;

/// NVS 를 쓰기용으로 연다. 실패하면 시리얼에 남기고 false.
///
/// `Preferences` 를 파일 정적으로 두지 않고 호출마다 지역 변수로 만든다.
/// 정적으로 두면 어딘가에서 `end()` 를 빠뜨린 순간 다음 `begin()` 이
/// `_started` 때문에 false 를 돌려주고, 그 다음부터 저장이 조용히 실패한다.
/// 지역 변수는 소멸자가 `end()` 를 부르므로 그 상태가 생기지 않는다.
bool openForWrite(Preferences &prefs, const char *what) {
    if (prefs.begin(NVS_NAMESPACE, /*readOnly=*/false)) {
        return true;
    }
    Serial.printf("config: NVS 를 쓰기용으로 열지 못했다 — %s 저장 실패\n", what);
    return false;
}

}  // namespace

bool configTokenIsValid(const String &token) {
    if (token.length() != 32) {
        return false;
    }
    for (unsigned int i = 0; i < 32; ++i) {
        const char ch = token[i];
        const bool isDigit = (ch >= '0' && ch <= '9');
        const bool isLowerHex = (ch >= 'a' && ch <= 'f');
        if (!isDigit && !isLowerHex) {
            return false;
        }
    }
    return true;
}

bool configIsPaired(const Config &c) {
    // `c.token.length() > 0` 이 아니라 형식 검사를 그대로 쓴다. 손으로 조립한
    // `Config` 가 들어와도 같은 답이 나오게 하려는 것이다.
    return configTokenIsValid(c.token);
}

bool configLoad(Config &c) {
    c.macHost = "";
    c.token = "";

    Preferences prefs;
    if (!prefs.begin(NVS_NAMESPACE, /*readOnly=*/true)) {
        // 네임스페이스가 아직 없다 = 우리가 한 번도 저장한 적이 없다.
        // (다른 원인일 수도 있다는 단서는 config.h 의 선언 주석에 적어 뒀다.)
        return false;
    }

    c.macHost = prefs.getString(KEY_MACHOST, "");
    const String storedToken = prefs.getString(KEY_TOKEN, "");
    prefs.end();

    if (storedToken.length() == 0) {
        // 설정은 있는데 아직 페어링 전. 정상 상태다.
    } else if (configTokenIsValid(storedToken)) {
        c.token = storedToken;
    } else {
        // 형식이 깨진 토큰은 여기서 버린다 — 미페어링과 같게 취급한다.
        //
        // NVS 에서 지우지는 **않는다.** `configLoad` 는 읽는 함수이고, 다음
        // 페어링이 어차피 같은 키를 덮어쓴다. 남겨 두면 부팅할 때마다 이 줄이
        // 다시 찍히는데, 화면이 없는 기기에서는 그 편이 낫다 — "한 번 있었던
        // 일" 이 아니라 "지금도 계속 그런 상태" 로 보이기 때문이다.
        //
        // 값 자체는 찍지 않는다(아래 T9-E). 길이만으로도 "쓰레기가 들어 있다"
        // 와 "잘렸다" 를 구분하기에 충분하다.
        Serial.printf("config: 저장된 토큰이 32자 소문자 hex 가 아니다 (길이 %u) — 미페어링으로 취급한다\n",
                      storedToken.length());
    }

    return true;
}

void configSaveHost(const Config &c) {
    Preferences prefs;
    if (!openForWrite(prefs, "맥 주소")) {
        return;
    }
    prefs.putString(KEY_MACHOST, c.macHost);
    prefs.end();
}

bool configSaveToken(Config &c, const String &token) {
    if (!configTokenIsValid(token)) {
        // 값은 찍지 않는다. 길이만 남긴다.
        Serial.printf("config: 32자 소문자 hex 가 아닌 토큰은 저장하지 않는다 (길이 %u)\n",
                      token.length());
        return false;
    }

    Preferences prefs;
    if (!openForWrite(prefs, "토큰")) {
        return false;
    }
    // `putString` 은 성공하면 쓴 길이를, 실패하면 0 을 돌려준다.
    const bool ok = prefs.putString(KEY_TOKEN, token) == token.length();
    prefs.end();

    if (!ok) {
        Serial.println("config: 토큰을 NVS 에 쓰지 못했다");
        return false;
    }
    c.token = token;
    return true;
}

void configClearToken(Config &c) {
    // 메모리 쪽을 먼저 비운다. NVS 쓰기가 실패하더라도 이번 부팅 동안은
    // 미페어링으로 동작하는 편이, 지웠다고 생각한 토큰으로 계속 붙는 것보다 낫다.
    c.token = "";

    Preferences prefs;
    if (!openForWrite(prefs, "토큰 삭제")) {
        return;
    }
    // 반환값을 보지 않는다: 키가 원래 없었을 때도 `remove` 는 false 를
    // 돌려주는데, "없는 것을 지워 달라" 는 요청은 이미 이루어진 상태다.
    prefs.remove(KEY_TOKEN);
    prefs.end();
}

// ─────────────────────────────────────────────────────────────────────────────
// 금지: **토큰은 설정 포털에 올리지 않는다.**
//
// 지금 이 함수가 포털에 만드는 칸은 `machost` 하나뿐이고 토큰은 근처에도 없다.
// 그런데 언젠가 누군가 "현재 설정 보기" 페이지를 붙이고 싶어질 것이고, 그때
// `Config` 를 통째로 찍는 것이 가장 짧은 길이다. **하지 마라.**
//
// 설정 포털은 **암호 없는 AP** 위의 평문 HTTP 다(`autoConnect(SSID)` 를 비밀번호
// 없이 부르므로 열린 AP 다). 거기에 토큰을 실으면 전파가 닿는 아무나 읽을 수
// 있고, 그건 스펙 7.2 가 받아들인 위험 — "기기를 물리적으로 주운 사람" — 과
// 전혀 다른 위험이다. 받아들인 적 없는 쪽이다.
//
// 같은 이유로 토큰을 시리얼에도 찍지 않는다. 이 파일이 토큰에 대해 남기는 것은
// 있다/없다와 형식이 틀렸다는 사실과 길이뿐이다.
//
// 디버그 레벨을 `WM_DEBUG_DEV` 이상으로 올리지 마라. 그 레벨에서 WiFiManager 는
// AP 비밀번호(`debugSoftAPConfig()`, WiFiManager.cpp:527·3408)와 STA 비밀번호
// (같은 파일 1109·1131)를 시리얼에 찍는다. 아래에서 레벨을 NOTIFY 로 못 박아
// 두는 이유가 이것이다.
// ─────────────────────────────────────────────────────────────────────────────
bool wifiConnectOrPortal(Config &c) {
    // `host` 를 `wm` 보다 먼저 선언한다 — 소멸은 역순이므로 `wm` 이 먼저 죽고,
    // 자기가 가리키던 파라미터가 살아 있는 채로 정리된다.
    //
    // 라벨과 id 는 문자열 리터럴이어야 한다: WiFiManagerParameter 는 이 둘을
    // **복사하지 않고 포인터로만 들고 있다**(기본값만 복사한다).
    WiFiManagerParameter host("machost", "맥 주소 (비우면 자동 탐색)",
                              c.macHost.c_str(), MACHOST_FIELD_LEN);

    WiFiManager wm;

    // 기본값과 같은 값이지만 명시한다. 위 금지 목록이 걸려 있는 자리이고,
    // 라이브러리 기본값이 바뀌어도 여기서 막힌다.
    wm.setDebugOutput(true, WM_DEBUG_NOTIFY);

    wm.setConnectTimeout(WIFI_CONNECT_TIMEOUT_S);

    // ── 포털은 영원히 기다리지 않는다 ──
    //
    // 이 기기는 상시 전원이고 화면도 버튼도 없다. 타임아웃을 걸지 않으면
    // (`_configPortalTimeout` 기본값이 0 이다) `autoConnect()` 는 아무도 포털에
    // 붙지 않는 한 **영원히 돌아오지 않는다.** 그러면 공유기가 재부팅된 날
    // 이렇게 된다: 저장된 자격증명으로 붙기 실패 → AP 가 뜬다 → 공유기는 몇 초
    // 뒤 돌아왔는데 기기는 포털 안에 갇혀 있다 → 사람이 알아채고 전원을 뽑을
    // 때까지 그대로다. 화면 없는 기기는 스스로 빠져나올 수 있어야 한다.
    //
    // 그래서 시간 제한을 걸고, 시간이 다 되면 false 로 돌아와 **호출자가 연결을
    // 다시 시도하게 한다**(main.cpp 의 T9-D). 공유기가 돌아온 뒤 최대 이 시간
    // 만큼 늦게 붙는다는 뜻이기도 하다.
    //
    // 5분이 사람에게 촉박하지 않은 이유: WiFiManager 의 `_webClientCheck` 가
    // 기본으로 켜져 있어서, 포털 페이지에 접근할 때마다 타임아웃 시작 시각이 그
    // 접근 시각으로 밀린다(`configPortalHasTimeout()`). 즉 5분은 "설정을 끝낼
    // 시간" 이 아니라 "아무도 손대지 않은 시간" 이다.
    //
    // `setAPClientCheck(true)`(스테이션이 붙어 있기만 해도 타임아웃 보류)는
    // 켜지 않았다 — 지나가던 폰이 자동으로 결합만 해도 포털이 무기한 열린 채로
    // 남는데, 그게 바로 이 제한이 막으려던 상태다.
    wm.setConfigPortalTimeout(PORTAL_TIMEOUT_S);

    wm.addParameter(&host);

    if (!wm.autoConnect(PORTAL_AP_SSID)) {
        return false;
    }

    // 포털이 아예 열리지 않았다면(저장된 자격증명으로 바로 붙은 경우)
    // `getValue()` 는 우리가 넣어 준 기본값을 그대로 돌려준다 — 즉 아래 비교는
    // 그때 자연스럽게 거짓이 되고 플래시를 건드리지 않는다.
    String entered = host.getValue();
    entered.trim();  // 폰에서 붙여넣다 딸려 온 공백이 mDNS 조회를 조용히 깨뜨린다
    if (entered != c.macHost) {
        c.macHost = entered;
        configSaveHost(c);
        Serial.printf("config: 맥 주소 저장 — \"%s\"\n", c.macHost.c_str());
    }
    return true;
}
