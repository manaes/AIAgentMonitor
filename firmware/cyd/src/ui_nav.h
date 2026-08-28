// AI Agent Monitor — CYD 펌웨어: 상단 탭 네비게이션 및 화면 전환 컨트롤러 (Task 15b).
//
// Subscribed(인가됨) 상태에서 카드 뷰와 세션 목록 뷰 간의 전환을 관리한다.
#pragma once

#include <lvgl.h>
#include "transport.h"

enum class UiView : uint8_t {
    Cards = 0,
    Sessions = 1,
};

/// 네비게이션 탭 바 및 하위 뷰들을 초기화한다.
void uiNavCreate(Transport &transport);

/// 매 loop() 마다 호출되어 현재 상태(인증 단계 및 활성 뷰)에 맞게 뷰를 갱신한다.
void uiNavUpdate(Transport &transport);

/// 활성 뷰를 변경한다.
void uiNavSetView(UiView view);
