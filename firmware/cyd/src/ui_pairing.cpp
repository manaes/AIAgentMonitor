#include "ui_pairing.h"

#include <lvgl.h>
#include <string.h>

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

const char *KEYPAD_MAP[] = {
    "1", "2", "3", "\n",
    "4", "5", "6", "\n",
    "7", "8", "9", "\n",
    "<", "0", "OK", "",
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
    lv_obj_set_style_text_font(label, &lv_font_montserrat_16, 0);
    lv_obj_set_style_text_color(label, lv_color_hex(0xffffff), 0);
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
    } else if (strcmp(txt, "OK") == 0 || strcmp(txt, "확인") == 0) {
        // "OK" 버튼 클릭 시 6자리가 입력되어 있으면 submitCode 호출
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
    lv_obj_set_style_text_color(g_statusTitle, lv_color_hex(0xffffff), 0);

    g_digitsLabel = makeLabel(g_root, 8, 36);
    lv_obj_set_style_text_font(g_digitsLabel, &lv_font_montserrat_20, 0);
    lv_obj_set_style_text_color(g_digitsLabel, lv_color_hex(0x0a84ff), 0);

    g_attemptsLabel = makeLabel(g_root, 8, 64);
    lv_obj_set_style_text_font(g_attemptsLabel, &lv_font_montserrat_14, 0);
    lv_obj_set_style_text_color(g_attemptsLabel, lv_color_hex(0x8e8e93), 0);

    g_timeLabel = makeLabel(g_root, 8, 86);
    lv_obj_set_style_text_font(g_timeLabel, &lv_font_montserrat_14, 0);
    lv_obj_set_style_text_color(g_timeLabel, lv_color_hex(0x8e8e93), 0);

    g_btnMatrix = lv_buttonmatrix_create(g_root);
    lv_buttonmatrix_set_map(g_btnMatrix, KEYPAD_MAP);
    lv_obj_set_style_text_font(g_btnMatrix, &lv_font_montserrat_16, LV_PART_ITEMS);

    lv_obj_set_pos(g_btnMatrix, 10, 130);
    lv_obj_set_size(g_btnMatrix, LV_HOR_RES - 20, LV_VER_RES - 130 - 12);
    lv_obj_add_event_cb(g_btnMatrix, onKeypadEvent, LV_EVENT_VALUE_CHANGED, &transport);

    g_otherLabel = lv_label_create(screen);
    lv_obj_set_style_text_font(g_otherLabel, &lv_font_montserrat_16, 0);
    lv_obj_set_style_text_color(g_otherLabel, lv_color_hex(0xffffff), 0);
    lv_obj_center(g_otherLabel);

    refreshDigitsLabel();
    refreshConfirmEnabled();
    uiPairingUpdate(transport);
}

void uiPairingUpdate(Transport &transport) {
    const AuthStep step = transport.authStep();

    const bool wasNeedsPairingOrFailed =
        (g_prevStep == AuthStep::NeedsPairing || g_prevStep == AuthStep::Failed);
    const bool isNeedsPairingOrFailed =
        (step == AuthStep::NeedsPairing || step == AuthStep::Failed);
    if (isNeedsPairingOrFailed && !wasNeedsPairingOrFailed) {
        g_codeWindowStarted = false;
    }
    g_prevStep = step;

    if (!g_codeWindowStarted && step == AuthStep::SendCode2) {
        g_codeWindowStarted = true;
        g_codeWindowStartedAtMs = millis();
    }

    if (!isPairingRelevant(step)) {
        lv_obj_add_flag(g_root, LV_OBJ_FLAG_HIDDEN);
        if (step == AuthStep::Subscribed && transport.isConnected() && transport.hasSnapshot()) {
            lv_obj_add_flag(g_otherLabel, LV_OBJ_FLAG_HIDDEN);
            g_otherLabelSubscribedCache = 1;
        } else {
            lv_obj_remove_flag(g_otherLabel, LV_OBJ_FLAG_HIDDEN);
            if (g_otherLabelSubscribedCache != 0) {
                g_otherLabelSubscribedCache = 0;
                lv_label_set_text(g_otherLabel, "Connecting...");
            }
        }
        return;
    }

    lv_obj_remove_flag(g_root, LV_OBJ_FLAG_HIDDEN);
    lv_obj_add_flag(g_otherLabel, LV_OBJ_FLAG_HIDDEN);
    g_otherLabelSubscribedCache = -1;

    const uint8_t attemptsLeft = transport.attemptsLeft();
    const bool exhausted = attemptsLeft == 0;
    const bool midFlight = (step == AuthStep::SendHello2 || step == AuthStep::SendCode2);
    const bool blockInput = exhausted || midFlight;

    const int desiredStatus = exhausted ? 2 : (!transport.isConnected() ? 1 : 0);
    if (desiredStatus != g_statusIsExhaustedCache) {
        g_statusIsExhaustedCache = desiredStatus;
        if (desiredStatus == 2) {
            lv_label_set_text(g_statusTitle, "Please re-pair in App");
        } else if (desiredStatus == 1) {
            lv_label_set_text(g_statusTitle, "Connecting...");
        } else {
            lv_label_set_text(g_statusTitle, "Enter Code");
        }
    }

    const int desiredBlocked = blockInput ? 1 : 0;
    if (desiredBlocked != g_buttonsBlockedCache) {
        g_buttonsBlockedCache = desiredBlocked;
        if (blockInput) {
            lv_buttonmatrix_set_button_ctrl_all(g_btnMatrix, LV_BUTTONMATRIX_CTRL_DISABLED);
            g_confirmEnabledCache = 0;
        } else {
            lv_buttonmatrix_clear_button_ctrl_all(g_btnMatrix, LV_BUTTONMATRIX_CTRL_DISABLED);
            g_confirmEnabledCache = 1;
            refreshConfirmEnabled();
        }
    }

    if (exhausted && g_typedDigits.length() > 0) {
        g_typedDigits = "";
        refreshDigitsLabel();
    }

    const int attemptsInt = (int)attemptsLeft;
    if (attemptsInt != g_attemptsShownCache) {
        g_attemptsShownCache = attemptsInt;
        lv_label_set_text_fmt(g_attemptsLabel, "Attempts left: %u", (unsigned)attemptsLeft);
    }

    if (g_codeWindowStarted) {
        const uint32_t elapsedMs = millis() - g_codeWindowStartedAtMs;
        const uint32_t elapsedSec = elapsedMs / 1000;
        const uint32_t remaining =
            elapsedSec >= CODE_WINDOW_SECONDS ? 0 : CODE_WINDOW_SECONDS - elapsedSec;
        if ((int32_t)remaining != g_remainingShownCache) {
            g_remainingShownCache = (int32_t)remaining;
            lv_label_set_text_fmt(g_timeLabel, "Time: %us", (unsigned)remaining);
        }
    } else if (g_remainingShownCache != -1) {
        g_remainingShownCache = -1;
        lv_label_set_text(g_timeLabel, "");
    }
}
