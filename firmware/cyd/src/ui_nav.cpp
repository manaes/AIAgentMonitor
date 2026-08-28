// AI Agent Monitor — CYD 펌웨어: 좌우 스와이프 제스처 화면 전환 컨트롤러 (Tileview).
#include "ui_nav.h"

#include "ui_cards.h"
#include "ui_sessions.h"
#include "ui_pairing.h"

namespace {

lv_obj_t *g_tileview = nullptr;
lv_obj_t *g_tileCards = nullptr;
lv_obj_t *g_tileSessions = nullptr;

int g_navVisibleCache = -1;  // -1=미적용, 0=숨김, 1=표시

}  // namespace

void uiNavCreate(Transport &transport) {
    lv_obj_t *screen = lv_screen_active();

    // 1. 페어링 화면 초기화
    uiPairingCreate(transport);

    // 2. 풀스크린 좌우 스와이프 Tileview 생성 (상단 탭 바 제거)
    g_tileview = lv_tileview_create(screen);
    lv_obj_set_size(g_tileview, LV_HOR_RES, LV_VER_RES);
    lv_obj_set_pos(g_tileview, 0, 0);
    lv_obj_set_style_bg_opa(g_tileview, LV_OPA_TRANSP, 0);
    lv_obj_set_style_border_width(g_tileview, 0, 0);
    lv_obj_set_style_pad_all(g_tileview, 0, 0);
    lv_obj_set_scrollbar_mode(g_tileview, LV_SCROLLBAR_MODE_OFF);

    // 타일 0,0: 카드 뷰 (오른쪽 스와이프 허용)
    g_tileCards = lv_tileview_add_tile(g_tileview, 0, 0, LV_DIR_RIGHT);
    lv_obj_set_style_pad_all(g_tileCards, 0, 0);
    uiCardsCreate(g_tileCards);

    // 타일 1,0: 세션 목록 뷰 (왼쪽 스와이프 허용)
    g_tileSessions = lv_tileview_add_tile(g_tileview, 1, 0, LV_DIR_LEFT);
    lv_obj_set_style_pad_all(g_tileSessions, 0, 0);
    uiSessionsCreate(g_tileSessions);

    // 초기 상태: Tileview 숨김 (페어링 완료 시 표시)
    lv_obj_add_flag(g_tileview, LV_OBJ_FLAG_HIDDEN);
}

void uiNavSetView(UiView view) {
    if (g_tileview != nullptr) {
        if (view == UiView::Cards && g_tileCards != nullptr) {
            lv_obj_set_tile(g_tileview, g_tileCards, LV_ANIM_ON);
        } else if (view == UiView::Sessions && g_tileSessions != nullptr) {
            lv_obj_set_tile(g_tileview, g_tileSessions, LV_ANIM_ON);
        }
    }
}

void uiNavUpdate(Transport &transport) {
    const bool isSubscribed = (transport.authStep() == AuthStep::Subscribed);

    if (isSubscribed) {
        // 인가 완료된 상태: 좌우 스와이프 Tileview 활성화
        if (g_navVisibleCache != 1) {
            g_navVisibleCache = 1;
            lv_obj_clear_flag(g_tileview, LV_OBJ_FLAG_HIDDEN);
        }
        uiCardsUpdate(transport);
        uiSessionsUpdate(transport);
    } else {
        // 페어링 또는 재인증 중: Tileview 숨김, 페어링 UI 표시
        if (g_navVisibleCache != 0) {
            g_navVisibleCache = 0;
            lv_obj_add_flag(g_tileview, LV_OBJ_FLAG_HIDDEN);
        }
    }

    uiPairingUpdate(transport);
}
