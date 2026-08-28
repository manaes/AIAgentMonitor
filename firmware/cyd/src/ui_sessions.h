// AI Agent Monitor — CYD 펌웨어: 프로젝트 및 세션 목록 화면 (Task 15b).
//
// Mac 및 iOS 미러와 정확히 동일한 시각 규칙을 따른다:
//   - 전체 에이전트의 프로젝트를 최근 활동순(lastActivityEpochSec 내림차순)으로 정렬
//   - 상태 점(dotColor): dormant=#636366, idle=#ff9f0a, active는 에이전트별(Claude=#30d158, Antigravity=#388bfd, Codex=#ff9f0a)
//   - 상태 단어: idle->"유휴", dormant->"휴면", active->tok/s 속도 표시
#pragma once

#include <lvgl.h>
#include "transport.h"

/// 세션 목록 화면 위젯을 생성한다.
/// @param parent 화면 컨테이너 (nullptr인 경우 활성 화면 기본값)
/// @return 생성된 루트 오브젝트
lv_obj_t *uiSessionsCreate(lv_obj_t *parent);

/// 매 loop() 마다 호출되어 세션 목록 데이터를 갱신한다.
/// 위젯을 재생성하지 않고 텍스트/상태 점/라벨 값만 변경한다.
void uiSessionsUpdate(const Transport &transport);

/// 세션 목록 화면의 표시 여부를 설정한다.
void uiSessionsSetVisible(bool visible);
