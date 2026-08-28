#include "ui_pairing.h"

#include <string.h>

#include "font_ko.h"

namespace {

constexpr size_t CODE_DIGITS = 6;

/// 로컬 추정 페어링 창 길이. 맥의 실제 상수(`CODE_TTL`,
/// `src-tauri/src/ble/pairing.rs:46`, 2026-08-27 그 커밋 기준 확인)와 같은
/// 값이다 — 하지만 이 값 자체가 와이어에 실리지 않으므로(`AwaitingCode2`
/// 는 epk/nonce 만 나른다) 이 화면이 아는 것은 "맥의 상수가 120초" 라는
/// 사실뿐이지, "맥의 창이 지금 몇 초 남았는지" 가 아니다. 아래
/// `codeWindowStarted_` 카운트다운은 그 차이를 메우는 최선의 추정치일
/// 뿐이다 — uiPairingUpdate() 의 문서 참고.
constexpr uint32_t CODE_WINDOW_SECONDS = 120;

/// KEYPAD_MAP 에서 "확인" 버튼의 0-기반 인덱스(줄바꿈 "\n" 은 세지 않는다).
constexpr uint32_t CONFIRM_BTN_ID = 11;

/// 브리프 스케치는 백스페이스에 "←"(U+2190)를 쓰지만, font_ko 는 ASCII
/// (0x20-0x7F) + 한글 음절 51자만 담고 있어(`tools/build-font.sh` 상단
/// 주석) 화살표 글리프가 없다 — `LV_USE_FONT_PLACEHOLDER=1`(끄면 안 되는
/// 전역 제약) 이라 보더만 있는 네모로 나온다. 이 한 글자를 위해 폰트
/// 서브셋을 U+2190 까지 넓히는 대신, ASCII 안에서 뜻이 통하는 "<" 로
/// 대체한다.
const char *KEYPAD_MAP[] = {
    "1", "2", "3", "\n",
    "4", "5", "6", "\n",
    "7", "8", "9", "\n",
    "<", "0", "확인", "",
};

lv_obj_t *g_root = nullptr;         // 페어링 키패드 전체(라벨+버튼 매트릭스)를 담는 컨테이너.
lv_obj_t *g_statusTitle = nullptr;  // "코드 입력" 또는 시도 소진 안내문.
lv_obj_t *g_digitsLabel = nullptr;
lv_obj_t *g_attemptsLabel = nullptr;
lv_obj_t *g_timeLabel = nullptr;
lv_obj_t *g_btnMatrix = nullptr;
lv_obj_t *g_otherLabel = nullptr;  // 페어링과 무관한 상태(재연결 중/인가됨)의 대체 문구.

String g_typedDigits;

bool g_codeWindowStarted = false;
uint32_t g_codeWindowStartedAtMs = 0;

/// fix round 1 (I-2) — `NeedsPairing`/`Failed` 로 돌아온 적이 있는지
/// 추적해 카운트다운을 다시 잰다. 센티널로 `Subscribed` 를 쓴다 — 부팅
/// 직후 첫 `uiPairingUpdate()` 호출에서 아래 리셋이 공짜로 한 번 더
/// 일어나도(`g_codeWindowStarted` 가 이미 `false` 라) 무해하다.
AuthStep g_prevStep = AuthStep::Subscribed;

/// **엣지 트리거 캐시 — 실기 실측으로 필요성이 드러났다.** `lv_buttonmatrix_
/// set/clear_button_ctrl(_all)` 은 실제 소스 확인 결과(`lv_buttonmatrix.c`)
/// 값이 바뀌었는지 보지 않고 매번 무조건 `invalidate_button_area()` 를
/// 부른다 — 매 `loop()` 마다(상태 변화가 없어도) 이 함수들을 그대로
/// 불렀더니 실기에서 `lv_timer_handler()` 한 번이 최대 118,840us 까지
/// 찍혔다(Task 14a 정상 상태 기준 400~600us 의 약 200배 — 보고서의
/// 재측정 절 참고). 아래 캐시들은 "이미 그 상태다" 를 걸러 LVGL 무효화
/// 호출 자체를 상태가 실제로 바뀔 때만 하도록 만든다. 센티널 값(-1 등)은
/// 첫 호출을 무조건 통과시켜 위젯을 만든 직후의 초기 상태를 확실히
/// 적용하기 위한 것이다.
int g_confirmEnabledCache = -1;   // -1=미적용, 0=disabled, 1=enabled.
int g_buttonsBlockedCache = -1;   // -1=미적용, 0=전체 활성, 1=전체 비활성(ctrl_all DISABLED).
int g_statusIsExhaustedCache = -1;  // -1=미적용, 0="코드 입력", 1=소진 안내문.
int g_attemptsShownCache = -1;    // -1=미적용, 그 외 마지막으로 라벨에 찍은 값.
int32_t g_remainingShownCache = -2;  // -2=미적용, -1="아직 시작 안 함"(빈 칸), 그 외 마지막 값.
int g_otherLabelSubscribedCache = -1;  // -1=미적용, 0="연결 중", 1="연결됨".

/// 이 화면이 실제로 그려야 하는 상태인가 — 사람이 코드를 입력해야
/// 하거나(NeedsPairing), 방금 입력한 코드를 처리 중이거나(SendHello2/
/// SendCode2), 이 펌웨어가 응답을 이해하지 못해 사람이 봐야 하는
/// (Failed) 경우다. SendAuth2/SendProof2(기존 토큰으로 조용히 재연결
/// 중)와 Subscribed(인가됨)는 사람이 할 일이 없으므로 이 화면이 아니라
/// `g_otherLabel` 이 대신 그린다.
bool isPairingRelevant(AuthStep step) {
    switch (step) {
        case AuthStep::NeedsPairing:
        case AuthStep::SendHello2:
        case AuthStep::SendCode2:
        case AuthStep::Failed:
            return true;
        default:
            return false;
    }
}

lv_obj_t *makeLabel(lv_obj_t *parent, int32_t x, int32_t y) {
    lv_obj_t *label = lv_label_create(parent);
    lv_obj_set_style_text_font(label, &font_ko, 0);
    lv_obj_set_pos(label, x, y);
    return label;
}

void refreshDigitsLabel() {
    String display;
    for (size_t i = 0; i < CODE_DIGITS; i++) {
        if (i > 0) {
            display += ' ';
        }
        display += (i < g_typedDigits.length()) ? String(g_typedDigits[i]) : String('_');
    }
    lv_label_set_text(g_digitsLabel, display.c_str());
}

/// "확인" 버튼 하나만 6자리가 다 찼을 때만 켠다. 나머지 버튼(숫자/백스페이스)
/// 의 활성/비활성은 uiPairingUpdate() 가 상태별로 따로 관리한다 — 그
/// 함수가 `LV_BUTTONMATRIX_CTRL_DISABLED` 를 전체에 걸었다 풀었다 할 때마다
/// 이 함수를 다시 불러야 "확인" 의 자체 조건이 덮어써지지 않는다.
///
/// `g_confirmEnabledCache` 로 실제 값이 바뀔 때만 LVGL 을 부른다 — 위
/// 캐시 블록 주석의 실측(무조건 호출 시 118ms) 이 이유다.
void refreshConfirmEnabled() {
    const int desired = (g_typedDigits.length() == CODE_DIGITS) ? 1 : 0;
    if (desired == g_confirmEnabledCache) {
        return;
    }
    g_confirmEnabledCache = desired;
    if (desired == 1) {
        lv_buttonmatrix_clear_button_ctrl(g_btnMatrix, CONFIRM_BTN_ID, LV_BUTTONMATRIX_CTRL_DISABLED);
    } else {
        lv_buttonmatrix_set_button_ctrl(g_btnMatrix, CONFIRM_BTN_ID, LV_BUTTONMATRIX_CTRL_DISABLED);
    }
}

void onKeypadEvent(lv_event_t *e) {
    lv_obj_t *btnm = (lv_obj_t *)lv_event_get_target(e);
    Transport *transport = (Transport *)lv_event_get_user_data(e);
    const uint32_t id = lv_buttonmatrix_get_selected_button(btnm);
    if (id == LV_BUTTONMATRIX_BUTTON_NONE) {
        return;
    }
    const char *txt = lv_buttonmatrix_get_button_text(btnm, id);
    if (txt == nullptr) {
        return;
    }

    if (strcmp(txt, "<") == 0) {
        if (g_typedDigits.length() > 0) {
            g_typedDigits.remove(g_typedDigits.length() - 1);
        }
    } else if (strcmp(txt, "확인") == 0) {
        // "확인" 은 refreshConfirmEnabled() 가 6자리 미만일 때 이미
        // disabled 로 걸어 두지만, 여기서 길이를 한 번 더 본다 — disabled
        // 버튼이 이벤트를 안 낸다는 것을 이 콜백이 직접 확인하지 않았고,
        // 잘못된 길이의 코드를 submitCode() 로 흘려보내는 것보다는 방어
        // 검사 하나가 싸다.
        if (g_typedDigits.length() == CODE_DIGITS) {
            transport->submitCode(g_typedDigits);
            g_typedDigits = "";
        }
    } else if (g_typedDigits.length() < CODE_DIGITS) {
        g_typedDigits += txt;  // "0".."9" 한 글자.
    }

    refreshDigitsLabel();
    refreshConfirmEnabled();
}

}  // namespace

void uiPairingCreate(Transport &transport) {
    lv_obj_t *screen = lv_screen_active();

    // 순수 위치 지정용 컨테이너라 기본 패널 스타일(배경·테두리)을 지운다 —
    // 그러지 않으면 화면 전체를 덮는 사각형이 그려진다.
    g_root = lv_obj_create(screen);
    lv_obj_remove_style_all(g_root);
    lv_obj_set_pos(g_root, 0, 0);
    lv_obj_set_size(g_root, LV_HOR_RES, LV_VER_RES);

    g_statusTitle = makeLabel(g_root, 8, 8);
    g_digitsLabel = makeLabel(g_root, 8, 36);
    g_attemptsLabel = makeLabel(g_root, 8, 64);
    g_timeLabel = makeLabel(g_root, 8, 86);

    g_btnMatrix = lv_buttonmatrix_create(g_root);
    lv_buttonmatrix_set_map(g_btnMatrix, KEYPAD_MAP);
    // btnmatrix 는 버튼 글자를 LV_PART_ITEMS 스타일로 그린다(실제 소스
    // 확인: lv_buttonmatrix.c 의 lv_obj_init_draw_label_dsc(obj,
    // LV_PART_ITEMS, ...)) — LV_PART_MAIN 에만 폰트를 걸면 "확인" 이 LVGL
    // 기본 폰트(한글 없음)로 그려져 상자만 보인다.
    lv_obj_set_style_text_font(g_btnMatrix, &font_ko, LV_PART_ITEMS);

    // T14a 인수인계 — XPT2046 소프트웨어 미러 X 좌표 off-by-one
    // (`esp32_smartdisplay@2.1.1` 의 `esp_lcd_touch.c:92`,
    // task-14a-report.md "발견한 버그 2"). 대응 옵션 (ii) 채택 — 버튼
    // 매트릭스를 화면 진짜 가장자리에서 안쪽으로 여백을 두고 배치해
    // 우회한다. 근거: 그 버그는 미러 변환에서 `-1` 이 빠진 **상수** +1
    // 오차라, 화면의 정확히 마지막 1 raw 픽셀 열(터치 IC 분해능 기준으로도
    // 아주 좁은 띠)에서만 무효 좌표(x=240, 유효 범위 밖)를 낸다 — 아래
    // 여백(가로 10px, 세로 12px)은 그 폭보다 훨씬 넓다. 그 결과 문제가
    // 되는 물리적 가장자리는 항상 "버튼도 아무것도 없는 여백" 에 떨어져,
    // 오터치가 나도 반응이 없을 뿐 옆 버튼이 대신 눌리지는 않는다.
    // 근본 수정(옵션 i, `esp_lcd_touch.c` 자동 패치)은 하지 않았다 —
    // `.pio/libdeps/` 아래 서드파티 소스라 `rm -rf .pio` 클린 빌드마다
    // 사라지고, 지속시키려면 `extra_scripts` 인프라가 새로 필요해 이
    // 태스크(키패드 화면) 범위를 넘는다.
    lv_obj_set_pos(g_btnMatrix, 10, 130);
    lv_obj_set_size(g_btnMatrix, LV_HOR_RES - 20, LV_VER_RES - 130 - 12);
    lv_obj_add_event_cb(g_btnMatrix, onKeypadEvent, LV_EVENT_VALUE_CHANGED, &transport);

    g_otherLabel = lv_label_create(screen);
    lv_obj_set_style_text_font(g_otherLabel, &font_ko, 0);
    lv_obj_center(g_otherLabel);

    refreshDigitsLabel();
    refreshConfirmEnabled();
    uiPairingUpdate(transport);
}

void uiPairingUpdate(Transport &transport) {
    const AuthStep step = transport.authStep();

    // fix round 1 (I-2) — `NeedsPairing`/`Failed` 로 "막" 돌아왔다면(직전
    // 프레임은 그 상태가 아니었다면) 카운트다운을 다시 처음부터 잰다.
    // 원래 "부팅 후 1회만 시작"이었는데, 그 근거("맥의 open_window() 는
    // begin_pairing 시점에 한 번만 창을 연다")는 **같은 창 안의 재시도**
    // 에만 성립했다 — 리뷰가 실제로 확인한 `pairing.rs:356-364` 의
    // `begin_pairing()` 은 사용자가 맥에서 페어링을 다시 시작할 때마다
    // `attempts_left`/TTL 을 처음부터 다시 잰다. CYD 는 "같은 창의 재시도"
    // 와 "완전히 새 창"을 와이어로 구별할 방법이 없으므로(둘 다 그냥
    // HELLO2→AwaitingCode2 왕복 하나로만 보인다), 리셋하지 않으면 만료된
    // 창 이후 화면이 "남은 시간 0초"에 영구히 멈춰 실제로는 120초 남은
    // 새 창을 만료된 것처럼 보여준다 — 매번 다시 재는 쪽이 그보다 덜
    // 틀린다.
    const bool wasNeedsPairingOrFailed =
        (g_prevStep == AuthStep::NeedsPairing || g_prevStep == AuthStep::Failed);
    const bool isNeedsPairingOrFailed =
        (step == AuthStep::NeedsPairing || step == AuthStep::Failed);
    if (isNeedsPairingOrFailed && !wasNeedsPairingOrFailed) {
        g_codeWindowStarted = false;
    }
    g_prevStep = step;

    // T14b-A — 로컬 추정 카운트다운. 이 화면이 (다시) SendCode2(=
    // AwaitingCode2 를 받아 CODE2 를 보낼 차례) 에 들어오는 순간 시작한다
    // — 위 리셋 덕분에 새 페어링 시도마다 다시 잰다. 맥이 실제로 창을
    // 연 시각과 이 시작 시각의 오차는 사람이 맥 화면의 6자리를 보고 이
    // 기기로 옮겨 오는 데 걸린 시간과, HELLO2 왕복 지연뿐이다 — 대체로
    // 짧지만 **보장되지는 않는다.** 맥이 앱 재시작·수동 취소 등으로
    // 창을 일찍 닫으면 이 값은 그냥 틀린다. 그래서 절대 시각이 아니라
    // "남은 초" 만 보여주고, 어디에도 "정확하다"는 주장을 남기지 않는다.
    if (!g_codeWindowStarted && step == AuthStep::SendCode2) {
        g_codeWindowStarted = true;
        g_codeWindowStartedAtMs = millis();
    }

    if (!isPairingRelevant(step)) {
        lv_obj_add_flag(g_root, LV_OBJ_FLAG_HIDDEN);       // 이미 숨어 있으면 lv_obj_add_flag 자체가 즉시 반환한다(실제 소스 확인).
        if (step == AuthStep::Subscribed) {
            // Task 15b: 인가 완료 시에는 카드/세션 뷰가 전체 화면을 차지하므로 안내 라벨을 숨긴다.
            lv_obj_add_flag(g_otherLabel, LV_OBJ_FLAG_HIDDEN);
            g_otherLabelSubscribedCache = 1;
        } else {
            lv_obj_remove_flag(g_otherLabel, LV_OBJ_FLAG_HIDDEN);
            if (g_otherLabelSubscribedCache != 0) {
                g_otherLabelSubscribedCache = 0;
                lv_label_set_text(g_otherLabel, "연결 중");
            }
        }
        return;
    }

    lv_obj_remove_flag(g_root, LV_OBJ_FLAG_HIDDEN);
    lv_obj_add_flag(g_otherLabel, LV_OBJ_FLAG_HIDDEN);
    g_otherLabelSubscribedCache = -1;  // 이 화면을 다시 벗어날 때 문구를 강제로 다시 정하게 한다.

    const uint8_t attemptsLeft = transport.attemptsLeft();
    // attemptsLeft()==0 은 실제 Denied 응답으로만 도달한다(Transport 의
    // 기본값은 5) — 즉 이 화면이 부팅 직후 보여 줄 수 있는 값이 아니라,
    // 적어도 한 번의 실제 거절을 거쳐야만 참이 된다. `NeedsPairing`/
    // `Failed` 둘 다 authfsm.cpp 판정 표에서 서로 다른 세 원인(코드 오답·
    // 핸드셰이크 만료·창 만료)을 구별하지 않고 하나로 묶으므로, 여기서도
    // 그 둘을 다시 가르지 않고 attemptsLeft 하나로만 "다 썼다" 를 판정한다
    // (브리프 리뷰 T14b 지시 — 새 상태를 만들지 말고 있는 구분을 쓴다).
    const bool exhausted = attemptsLeft == 0;
    const bool midFlight = (step == AuthStep::SendHello2 || step == AuthStep::SendCode2);
    const bool blockInput = exhausted || midFlight;

    const int desiredStatus = exhausted ? 1 : 0;
    if (desiredStatus != g_statusIsExhaustedCache) {
        g_statusIsExhaustedCache = desiredStatus;
        lv_label_set_text(g_statusTitle,
                           exhausted ? "맥에서 페어링을 다시 시작하세요" : "코드 입력");
    }

    const int desiredBlocked = blockInput ? 1 : 0;
    if (desiredBlocked != g_buttonsBlockedCache) {
        g_buttonsBlockedCache = desiredBlocked;
        if (blockInput) {
            lv_buttonmatrix_set_button_ctrl_all(g_btnMatrix, LV_BUTTONMATRIX_CTRL_DISABLED);
            g_confirmEnabledCache = 0;  // ctrl_all 이 "확인" 도 같이 껐다 — 캐시를 맞춘다.
        } else {
            lv_buttonmatrix_clear_button_ctrl_all(g_btnMatrix, LV_BUTTONMATRIX_CTRL_DISABLED);
            g_confirmEnabledCache = 1;  // ctrl_all 이 "확인" 도 같이 켰다 — 캐시를 맞춘 뒤 실제 조건으로 되돌린다.
            refreshConfirmEnabled();    // 6자리 미만이면 여기서 다시 끈다(캐시가 방금 바뀌었으니 실제로 호출된다).
        }
    }

    // exhausted 인 동안은 입력이 막혀 있으므로(위 blockInput) 실제로는
    // exhausted 에 막 들어온 그 순간에만 g_typedDigits 가 비어 있지 않을
    // 수 있다 — 길이 검사 자체가 사실상 엣지 트리거라 별도 캐시가 필요
    // 없다.
    if (exhausted && g_typedDigits.length() > 0) {
        g_typedDigits = "";
        refreshDigitsLabel();
    }

    const int attemptsInt = (int)attemptsLeft;
    if (attemptsInt != g_attemptsShownCache) {
        g_attemptsShownCache = attemptsInt;
        lv_label_set_text_fmt(g_attemptsLabel, "남은 시도 %u회", (unsigned)attemptsLeft);
    }

    if (g_codeWindowStarted) {
        const uint32_t elapsedMs = millis() - g_codeWindowStartedAtMs;
        const uint32_t elapsedSec = elapsedMs / 1000;
        const uint32_t remaining =
            elapsedSec >= CODE_WINDOW_SECONDS ? 0 : CODE_WINDOW_SECONDS - elapsedSec;
        // 초 단위라 사실상 초당 한 번만 실제로 바뀐다 — 캐시가 그 이하
        // 빈도로 자연히 걸러 준다(매 loop() 마다 같은 초를 다시 찍지
        // 않는다).
        if ((int32_t)remaining != g_remainingShownCache) {
            g_remainingShownCache = (int32_t)remaining;
            lv_label_set_text_fmt(g_timeLabel, "남은 시간 %u초", (unsigned)remaining);
        }
    } else if (g_remainingShownCache != -1) {
        // 아직 맥에게서 코드를 요구받은 적이 없다(HELLO2 조차 못 보냈다 —
        // 오프라인이거나 아직 연결 중) — 추정할 시작점이 없으므로 빈
        // 칸으로 둔다. "0초" 를 보여주면 창이 이미 닫혔다는 거짓 신호가
        // 된다.
        g_remainingShownCache = -1;
        lv_label_set_text(g_timeLabel, "");
    }
}
