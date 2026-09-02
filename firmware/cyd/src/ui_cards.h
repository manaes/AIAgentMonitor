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

/// 카드 화면 데이터를 갱신한다. 위젯을 재생성하지 않고 텍스트/바 값/색상만
/// 변경하여 깜빡임을 방지한다.
///
/// `agentIndexToUpdate` 로 지정한 카드 **하나만** 실제로 갱신한다(표시/숨김
/// 처리는 전체를 훑되 그건 싸다). 3카드를 한 호출에서 동시에 건드리면
/// LVGL 이 dirty 영역을 화면 세로 전체로 합쳐 풀스크린급 플러시를
/// 유발하기 때문이다(2026-09-01 프로파일링으로 확인) — 호출하는 쪽이
/// 카드 인덱스를 순환시켜 가며 여러 차례 나눠 부르는 것을 전제로 한다.
void uiCardsUpdate(const Transport &transport, size_t agentIndexToUpdate);

/// tok/s 속도 라벨만 갱신한다(이름/쿼터%/바/리셋시간은 건드리지 않음).
/// 라벨 3개(폭 좁고 텍스트만)만 dirty 해지므로 uiCardsUpdate() 보다 훨씬
/// 가볍다 — 매 loop() 마다 스로틀 없이 불러도 안전하다.
void uiCardsUpdateRates(const Transport &transport);

/// 카드 화면의 표시 여부를 설정한다.
void uiCardsSetVisible(bool visible);
