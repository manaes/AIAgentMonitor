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
lv_obj_t *g_btnClose = nullptr;     // 키패드 닫기(X) 버튼 -> 모드 선택 화면 복귀

// Connecting... 화면용 위젯 (세로형 Wi-Fi / Bluetooth 버튼)
lv_obj_t *g_connectingContainer = nullptr;
lv_obj_t *g_connectingTitle = nullptr;
lv_obj_t *g_btnWifi = nullptr;
lv_obj_t *g_labelWifi = nullptr;
lv_obj_t *g_btnBle = nullptr;
lv_obj_t *g_labelBle = nullptr;

String g_typedDigits;

bool g_manualModeSelection = false;
bool g_codeWindowStarted = false;
uint32_t g_codeWindowStartedAtMs = 0;

AuthStep g_prevStep = AuthStep::Subscribed;

int g_confirmEnabledCache = -1;   // -1=미적용, 0=disabled, 1=enabled.
int g_buttonsBlockedCache = -1;   // -1=미적용, 0=전체 활성, 1=전체 비활성(ctrl_all DISABLED).
int g_statusIsExhaustedCache = -1;  // -1=미적용, 0="코드 입력", 1=소진 안내문.
int g_attemptsShownCache = -1;    // -1=미적용, 그 외 마지막으로 라벨에 찍은 값.
int32_t g_remainingShownCache = -2;  // -2=미적용, -1="아직 시작 안 함"(빈 칸), 그 외 마지막 값.
int g_otherLabelSubscribedCache = -1;  // -1=미적용, 0="연결 중", 1="연결됨".
int g_modeShownCache = -1;        // -1=미적용, 0=WiFi, 1=Ble.

/// 이 화면이 실제로 그려야 하는 상태인가 — 사람이 코드를 입력해야
/// 하거나(NeedsPairing), 방금 입력한 코드를 처리 중이거나(SendHello2/
/// SendCode2), 이 펌웨어가 응답을 이해하지 못해 사람이 봐야 하는
/// (Failed) 경우다. SendAuth2/SendProof2(기존 토큰으로 조용히 재연결
/// 중)와 Subscribed(인가됨)는 사람이 할 일이 없으므로 이 화면이 아니라
/// `g_otherLabel` 이 대신 그린다.
bool isPairingRelevant(AuthStep step, TransportMode mode, bool isConnected) {
    // NeedsPairing + WiFi 는 예외다. `Transport::loop()`(transport.cpp:230
    // 근처)는 사람이 코드를 입력하기(`pendingCode_`) 전에는 맥의 제한된
    // 연결 슬롯을 아끼려고 소켓 자체를 안 연다 — 그런데 이 화면이
    // `isConnected()` 를 요구하면 코드를 입력할 방법이 아예 없어 영영
    // "Connecting via Wi-Fi..." 에 멈춘다(실기 재현 확인, 2026-09-01). BLE
    // 는 코드 유무와 상관없이 항상 먼저 스캔·연결을 시도하는 별도 경로라
    // 이 예외가 필요 없고, 오히려 부팅 직후(아직 미연결) 키패드를 조기
    // 노출하면 안 되므로 그대로 isConnected() 를 요구한다.
    if (step == AuthStep::NeedsPairing && mode == TransportMode::WiFi) {
        return true;
    }
    if (!isConnected) {
        return false; // Mac에 연결되기 전에는 Connecting... 화면을 유지한다.
    }
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

void onWifiBtnClick(lv_event_t *e) {
    Transport *transport = (Transport *)lv_event_get_user_data(e);
    g_manualModeSelection = false;
    if (transport != nullptr) {
        transport->setMode(TransportMode::WiFi);
        uiPairingUpdate(*transport);
        lv_refr_now(nullptr);
    }
}

void onBleBtnClick(lv_event_t *e) {
    Transport *transport = (Transport *)lv_event_get_user_data(e);
    g_manualModeSelection = false;
    if (transport != nullptr) {
        transport->setMode(TransportMode::Ble);
        uiPairingUpdate(*transport);
        lv_refr_now(nullptr);
    }
}

void onCloseKeypad(lv_event_t *e) {
    Transport *transport = (Transport *)lv_event_get_user_data(e);
    g_manualModeSelection = true;
    g_typedDigits = "";
    refreshDigitsLabel();
    if (transport != nullptr) {
        uiPairingUpdate(*transport);
    }
    lv_refr_now(nullptr);
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

    // ── 키패드 닫기 (X) 버튼 ──
    g_btnClose = lv_button_create(g_root);
    lv_obj_set_pos(g_btnClose, LV_HOR_RES - 48, 6);
    lv_obj_set_size(g_btnClose, 40, 32);
    lv_obj_set_style_radius(g_btnClose, 6, 0);
    lv_obj_set_style_bg_color(g_btnClose, lv_color_hex(0x2c2c2e), 0);
    lv_obj_add_event_cb(g_btnClose, onCloseKeypad, LV_EVENT_PRESSED, &transport);

    lv_obj_t *lblClose = lv_label_create(g_btnClose);
    lv_obj_set_style_text_font(lblClose, &lv_font_montserrat_16, 0);
    lv_obj_set_style_text_color(lblClose, lv_color_hex(0xff453a), 0);
    lv_label_set_text(lblClose, "X");
    lv_obj_center(lblClose);

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

    // ── Connecting... 화면 컨테이너 (Wi-Fi / Bluetooth 모드 선택) ──
    g_connectingContainer = lv_obj_create(screen);
    lv_obj_remove_style_all(g_connectingContainer);
    lv_obj_set_pos(g_connectingContainer, 0, 0);
    lv_obj_set_size(g_connectingContainer, LV_HOR_RES, LV_VER_RES);

    g_connectingTitle = lv_label_create(g_connectingContainer);
    lv_obj_set_style_text_font(g_connectingTitle, &lv_font_montserrat_16, 0);
    lv_obj_set_style_text_color(g_connectingTitle, lv_color_hex(0xffffff), 0);
    lv_obj_set_pos(g_connectingTitle, 0, 32);
    lv_obj_set_size(g_connectingTitle, LV_HOR_RES, 24);
    lv_obj_set_style_text_align(g_connectingTitle, LV_TEXT_ALIGN_CENTER, 0);
    lv_label_set_text(g_connectingTitle, "Connecting...");

    // Wi-Fi 버튼
    g_btnWifi = lv_button_create(g_connectingContainer);
    lv_obj_set_pos(g_btnWifi, 40, 85);
    lv_obj_set_size(g_btnWifi, LV_HOR_RES - 80, 48);
    lv_obj_set_style_radius(g_btnWifi, 8, 0);
    lv_obj_add_event_cb(g_btnWifi, onWifiBtnClick, LV_EVENT_PRESSED, &transport);

    g_labelWifi = lv_label_create(g_btnWifi);
    lv_obj_set_style_text_font(g_labelWifi, &lv_font_montserrat_16, 0);
    lv_label_set_text(g_labelWifi, "Wi-Fi (LAN)");
    lv_obj_center(g_labelWifi);

    // Bluetooth 버튼
    g_btnBle = lv_button_create(g_connectingContainer);
    lv_obj_set_pos(g_btnBle, 40, 150);
    lv_obj_set_size(g_btnBle, LV_HOR_RES - 80, 48);
    lv_obj_set_style_radius(g_btnBle, 8, 0);
    lv_obj_add_event_cb(g_btnBle, onBleBtnClick, LV_EVENT_PRESSED, &transport);

    g_labelBle = lv_label_create(g_btnBle);
    lv_obj_set_style_text_font(g_labelBle, &lv_font_montserrat_16, 0);
    lv_label_set_text(g_labelBle, "Bluetooth (BLE)");
    lv_obj_center(g_labelBle);

    refreshDigitsLabel();
    refreshConfirmEnabled();
    uiPairingUpdate(transport);
}

void uiPairingUpdate(Transport &transport) {
    const AuthStep step = transport.authStep();
    const TransportMode mode = transport.mode();

    const int modeInt = (mode == TransportMode::Ble) ? 1 : 0;
    if (modeInt != g_modeShownCache) {
        g_modeShownCache = modeInt;
        if (mode == TransportMode::WiFi) {
            lv_label_set_text(g_connectingTitle, "Connecting via Wi-Fi...");
            lv_obj_set_style_bg_color(g_btnWifi, lv_color_hex(0x0a84ff), 0);
            lv_obj_set_style_text_color(g_labelWifi, lv_color_hex(0xffffff), 0);

            lv_obj_set_style_bg_color(g_btnBle, lv_color_hex(0x2c2c2e), 0);
            lv_obj_set_style_text_color(g_labelBle, lv_color_hex(0x8e8e93), 0);
        } else {
            lv_label_set_text(g_connectingTitle, "Connecting via BLE...");
            lv_obj_set_style_bg_color(g_btnWifi, lv_color_hex(0x2c2c2e), 0);
            lv_obj_set_style_text_color(g_labelWifi, lv_color_hex(0x8e8e93), 0);

            lv_obj_set_style_bg_color(g_btnBle, lv_color_hex(0x0a84ff), 0);
            lv_obj_set_style_text_color(g_labelBle, lv_color_hex(0xffffff), 0);
        }
    }

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

    const bool showPairing = !g_manualModeSelection && isPairingRelevant(step, mode, transport.isConnected());
    if (!showPairing) {
        lv_obj_add_flag(g_root, LV_OBJ_FLAG_HIDDEN);
        if (!g_manualModeSelection && step == AuthStep::Subscribed && transport.isConnected()) {
            lv_obj_add_flag(g_connectingContainer, LV_OBJ_FLAG_HIDDEN);
            g_otherLabelSubscribedCache = 1;
        } else {
            lv_obj_remove_flag(g_connectingContainer, LV_OBJ_FLAG_HIDDEN);
            g_otherLabelSubscribedCache = 0;
        }
        return;
    }

    lv_obj_remove_flag(g_root, LV_OBJ_FLAG_HIDDEN);
    lv_obj_add_flag(g_connectingContainer, LV_OBJ_FLAG_HIDDEN);
    g_otherLabelSubscribedCache = -1;

    const uint8_t attemptsLeft = transport.attemptsLeft();
    const bool exhausted = (attemptsLeft == 0);
    const bool blockInput = exhausted;

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
