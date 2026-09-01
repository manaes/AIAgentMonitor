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
    const bool isSubscribed = (transport.authStep() == AuthStep::Subscribed && transport.isConnected());

    if (isSubscribed) {
        // 인가 완료: 카드 대시보드 활성화 및 갱신
        bool justShown = false;
        if (g_cardsVisibleCache != 1) {
            g_cardsVisibleCache = 1;
            uiCardsSetVisible(true);
            justShown = true;
        }

        // uiNavUpdate() 는 매 loop() 마다(초당 수백 회) 불린다. 카드 3개가
        // 화면 세로 전체에 걸쳐 있어서, 이름/쿼터%/바/리셋시간까지 전부
        // 갱신하면 LVGL 이 3개 카드의 dirty 영역을 화면 전체 높이로 합쳐
        // 풀스크린 SPI 플러시(~130ms대, 정상 대비 200배)를 유발한다. 그
        // 사이 loop() 이 막혀 BLE 무선 이벤트 처리가 밀려 연결 타임아웃
        // (reason=520)까지 이어지는 정황이 있었다.
        //
        // tok/s 는 사람이 체감하는 속도감이라 자주 바뀌는 게 자연스럽고,
        // 라벨 3개(텍스트만, 폭도 좁음)만 dirty 해지므로 훨씬 가볍다 —
        // 이건 매 loop() 마다 스로틀 없이 갱신한다. 나머지(쿼터%/바/리셋
        // 시간)는 1초 단위로 안 바뀌어도 체감상 문제없으므로 2초 간격으로
        // 늦춰서, 무거운 풀카드 갱신이 걸리는 빈도 자체를 줄인다.
        uiCardsUpdateRates(transport);

        static uint32_t lastCardsUpdateMs = 0;
        const uint32_t now = millis();
        if (justShown || now - lastCardsUpdateMs >= 2000) {
            uiCardsUpdate(transport);
            lastCardsUpdateMs = now;
        }
    } else {
        // 연결 끊김 / 재연결 중 / 페어링 대기: 카드 대시보드 숨김
        if (g_cardsVisibleCache != 0) {
            g_cardsVisibleCache = 0;
            uiCardsSetVisible(false);
        }
    }

    uiPairingUpdate(transport);
}
