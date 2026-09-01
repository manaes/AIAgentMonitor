// AI Agent Monitor — CYD 펌웨어: 에이전트별 사용량 카드 화면 (Task 15b).
//
// Mac 및 iOS 미러와 정확히 동일한 시각 규칙을 따른다:
//   - 쿼터 바 색상 경계: 90% 이상 #ff453a(빨강), 70~89% #ff9f0a(주황), 70% 미만 #30d158(초록)
//   - 5시간 리셋 카운트다운 / 주간 사용률 바(주간 값이 있을 때만 표시)
//   - tok/s 속도 표시
#pragma once

#include <lvgl.h>
#include "transport.h"

/// 카드 화면 위젯을 생성한다.
/// @param parent 화면 컨테이너 (nullptr인 경우 활성 화면 기본값)
/// @return 생성된 루트 오브젝트
lv_obj_t *uiCardsCreate(lv_obj_t *parent);

/// 매 loop() 마다 호출되어 카드 화면 데이터를 갱신한다.
/// 위젯을 재생성하지 않고 텍스트/바 값/색상만 변경하여 깜빡임을 방지한다.
void uiCardsUpdate(const Transport &transport);

/// tok/s 속도 라벨만 갱신한다(이름/쿼터%/바/리셋시간은 건드리지 않음).
/// 라벨 3개(폭 좁고 텍스트만)만 dirty 해지므로 uiCardsUpdate() 보다 훨씬
/// 가볍다 — 매 loop() 마다 스로틀 없이 불러도 안전하다.
void uiCardsUpdateRates(const Transport &transport);

/// 카드 화면의 표시 여부를 설정한다.
void uiCardsSetVisible(bool visible);
