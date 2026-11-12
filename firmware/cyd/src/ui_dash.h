// AI Agent Monitor — CYD 펌웨어: Dash 화면(승인된 B안, docs/design/b-final.en.html 기준).
//
// 상단: Claude/Codex 주간(7d) 게이지 2개 + tok/s + 리셋까지 남은 시간.
// 한도 소진 시: 주간 게이지 대신 재사용까지 남은 시간(5h/7d 중 늦은 리셋)과 주간 %를 주황색으로 표시.
// 중단: 최근 CI job 최대 4행(진행중/성공/실패, 첫 실패 행만 풀폭 빨강 강조).
// 라벨은 전부 ASCII, 폰트는 LVGL 내장 Montserrat 10/12/16/20 Regular 뿐이다(펌웨어에 한글 폰트 없음).
// 행 높이·시간 포맷은 lib/dashlogic(순수 함수, 호스트 테스트 가능)이 정한다.
#pragma once

#include <lvgl.h>
#include "transport.h"

/// 위젯을 한 번만 만든다(이후 갱신은 텍스트/값/위치만 바꾼다). 화면은 320x240 가로 기준이다.
lv_obj_t *uiDashCreate(lv_obj_t *parent);

/// 매 loop() 마다 불러도 된다. 새 스냅샷이 왔을 때와 1초마다만 실제로 일하고, 바뀐 위젯만 건드린다
/// (ui_cards.h 의 "여러 영역 동시 갱신 → 풀스크린 플러시 → BLE 타임아웃" 교훈).
void uiDashUpdate(const Transport &transport);

void uiDashSetVisible(bool visible);
