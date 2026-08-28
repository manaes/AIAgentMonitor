// AI Agent Monitor — CYD 펌웨어: 상단 탭 네비게이션 및 화면 전환 컨트롤러 (Task 15b).
#include "ui_nav.h"

#include "font_ko.h"
#include "ui_cards.h"
#include "ui_sessions.h"
#include "ui_pairing.h"

namespace {

lv_obj_t *g_navBar = nullptr;
lv_obj_t *g_btnCards = nullptr;
lv_obj_t *g_btnSessions = nullptr;
lv_obj_t *g_labelCards = nullptr;
lv_obj_t *g_labelSessions = nullptr;

UiView g_currentView = UiView::Cards;
int g_navVisibleCache = -1;  // -1=미적용, 0=숨김, 1=표시

void updateTabStyles() {
    if (g_currentView == UiView::Cards) {
        lv_obj_set_style_bg_color(g_btnCards, lv_color_hex(0x0a84ff), 0);
        lv_obj_set_style_bg_color(g_btnSessions, lv_color_hex(0x2c2c2e), 0);
    } else {
        lv_obj_set_style_bg_color(g_btnCards, lv_color_hex(0x2c2c2e), 0);
        lv_obj_set_style_bg_color(g_btnSessions, lv_color_hex(0x0a84ff), 0);
    }
}

void onTabClicked(lv_event_t *e) {
    lv_obj_t *target = (lv_obj_t *)lv_event_get_target(e);
    if (target == g_btnCards) {
        uiNavSetView(UiView::Cards);
    } else if (target == g_btnSessions) {
        uiNavSetView(UiView::Sessions);
    }
}

}  // namespace

void uiNavCreate(Transport &transport) {
    lv_obj_t *screen = lv_screen_active();

    // 1. 페어링 화면 초기화
    uiPairingCreate(transport);

    // 2. 카드 및 세션 뷰 생성
    uiCardsCreate(screen);
    uiSessionsCreate(screen);

    // 3. 상단 탭 네비게이션 바 생성
    g_navBar = lv_obj_create(screen);
    lv_obj_set_size(g_navBar, LV_HOR_RES, 34);
    lv_obj_set_pos(g_navBar, 0, 0);
    lv_obj_set_style_bg_color(g_navBar, lv_color_hex(0x1c1c1e), 0);
    lv_obj_set_style_border_width(g_navBar, 0, 0);
    lv_obj_set_style_pad_all(g_navBar, 3, 0);
    lv_obj_set_flex_flow(g_navBar, LV_FLEX_FLOW_ROW);
    lv_obj_set_flex_align(g_navBar, LV_FLEX_ALIGN_SPACE_EVENLY, LV_FLEX_ALIGN_CENTER, LV_FLEX_ALIGN_CENTER);

    // 카드 탭 버튼
    g_btnCards = lv_button_create(g_navBar);
    lv_obj_set_size(g_btnCards, 110, 28);
    lv_obj_set_style_radius(g_btnCards, 6, 0);
    lv_obj_set_style_border_width(g_btnCards, 0, 0);
    lv_obj_add_event_cb(g_btnCards, onTabClicked, LV_EVENT_CLICKED, nullptr);

    g_labelCards = lv_label_create(g_btnCards);
    lv_obj_set_style_text_font(g_labelCards, &font_ko, 0);
    lv_label_set_text(g_labelCards, "카드");
    lv_obj_center(g_labelCards);

    // 세션 탭 버튼
    g_btnSessions = lv_button_create(g_navBar);
    lv_obj_set_size(g_btnSessions, 110, 28);
    lv_obj_set_style_radius(g_btnSessions, 6, 0);
    lv_obj_set_style_border_width(g_btnSessions, 0, 0);
    lv_obj_add_event_cb(g_btnSessions, onTabClicked, LV_EVENT_CLICKED, nullptr);

    g_labelSessions = lv_label_create(g_btnSessions);
    lv_obj_set_style_text_font(g_labelSessions, &font_ko, 0);
    lv_label_set_text(g_labelSessions, "세션");
    lv_obj_center(g_labelSessions);

    // 초기 상태: 카드 뷰 선택 및 탭 바 숨김
    updateTabStyles();
    uiNavSetView(UiView::Cards);
    lv_obj_add_flag(g_navBar, LV_OBJ_FLAG_HIDDEN);
}

void uiNavSetView(UiView view) {
    g_currentView = view;
    updateTabStyles();
    if (view == UiView::Cards) {
        uiCardsSetVisible(true);
        uiSessionsSetVisible(false);
    } else {
        uiCardsSetVisible(false);
        uiSessionsSetVisible(true);
    }
}

void uiNavUpdate(Transport &transport) {
    const bool isSubscribed = (transport.authStep() == AuthStep::Subscribed);

    if (isSubscribed) {
        // 인가 완료된 상태: 탭 바 및 데이터 뷰 표시
        if (g_navVisibleCache != 1) {
            g_navVisibleCache = 1;
            lv_obj_clear_flag(g_navBar, LV_OBJ_FLAG_HIDDEN);
            uiNavSetView(g_currentView);
        }
        uiCardsUpdate(transport);
        uiSessionsUpdate(transport);
    } else {
        // 페어링 또는 재인증 중: 탭 바 및 데이터 뷰 숨김, 페어링 UI 표시
        if (g_navVisibleCache != 0) {
            g_navVisibleCache = 0;
            lv_obj_add_flag(g_navBar, LV_OBJ_FLAG_HIDDEN);
            uiCardsSetVisible(false);
            uiSessionsSetVisible(false);
        }
    }

    uiPairingUpdate(transport);
}
