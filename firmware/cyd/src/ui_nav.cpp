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
        // (reason=520)까지 이어지는 정황이 있었다. 2026-09-01 프로파일링
        // (ui_cards.cpp의 PROFILE_SKIP_* 매크로)으로 위젯 종류를 하나씩
        // 빼봐도 결국 "3카드 동시 갱신 → dirty 영역 합쳐짐" 자체가 문제의
        // 핵심임을 확인했다 — 그래서 카드를 매번 다 갱신하는 대신 **한
        // 번에 한 장만** 갱신하도록 `uiCardsUpdate()` 시그니처를 바꿨다
        // (ui_cards.h 문서 참고).
        //
        // tok/s 는 사람이 체감하는 속도감이라 자주 바뀌는 게 자연스럽고,
        // 라벨 3개(텍스트만, 폭도 좁음)만 dirty 해지므로 훨씬 가볍다 —
        // 이건 매 loop() 마다 스로틀 없이 전부 갱신한다(uiCardsUpdateRates).
        // 나머지(이름/쿼터%/바/리셋시간)는 카드 1장씩 2초 간격으로 순환
        // 갱신한다(전체 한 바퀴 6초) — 매초 갱신 대비 체감 차이는 거의
        // 없으면서, 한 번에 그려야 하는 영역은 카드 1장 높이로 줄어든다.
        uiCardsUpdateRates(transport);

        static uint32_t lastCardsUpdateMs = 0;
        static size_t nextCardToUpdate = 0;
        const uint32_t now = millis();
        if (justShown) {
            // 처음 보여질 때는 3장 다 비어 있으니 예외적으로 한 번에 채운다.
            for (size_t i = 0; i < SNAPSHOT_MAX_AGENTS; i++) {
                uiCardsUpdate(transport, i);
            }
            lastCardsUpdateMs = now;
            nextCardToUpdate = 0;
        } else if (now - lastCardsUpdateMs >= 2000) {
            uiCardsUpdate(transport, nextCardToUpdate);
            lastCardsUpdateMs = now;
            nextCardToUpdate = (nextCardToUpdate + 1) % SNAPSHOT_MAX_AGENTS;
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
