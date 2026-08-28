// AI Agent Monitor — CYD 펌웨어: 카드 대시보드 화면 컨트롤러.
#include "ui_nav.h"

#include "ui_cards.h"
#include "ui_pairing.h"

namespace {

int g_cardsVisibleCache = -1;  // -1=미적용, 0=숨김, 1=표시

}  // namespace

void uiNavCreate(Transport &transport) {
    lv_obj_t *screen = lv_screen_active();

    // 1. 페어링 화면 초기화
    uiPairingCreate(transport);

    // 2. 풀스크린 카드 전용 대시보드 생성
    uiCardsCreate(screen);

    // 초기 상태: 카드 화면 숨김 (페어링 완료 시 표시)
    uiCardsSetVisible(false);
}

void uiNavSetView(UiView /*view*/) {
    // 단일 카드 뷰 전용
}

void uiNavUpdate(Transport &transport) {
    const bool isSubscribed = (transport.authStep() == AuthStep::Subscribed);

    if (isSubscribed) {
        // 인가 완료된 상태: 카드 대시보드 활성화 및 갱신
        if (g_cardsVisibleCache != 1) {
            g_cardsVisibleCache = 1;
            uiCardsSetVisible(true);
        }
        uiCardsUpdate(transport);
    } else {
        // 페어링 또는 재인증 중: 카드 대시보드 숨김, 페어링 UI 표시
        if (g_cardsVisibleCache != 0) {
            g_cardsVisibleCache = 0;
            uiCardsSetVisible(false);
        }
    }

    uiPairingUpdate(transport);
}
