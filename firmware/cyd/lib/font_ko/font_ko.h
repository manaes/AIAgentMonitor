// AI Agent Monitor — CYD 펌웨어: font_ko 공개 선언 (Task 14a)
//
// Task 13 은 `font_ko.c`(lv_font_conv 생성물, 헤더 없음)만 만들었다 —
// 그때는 아무도 이 폰트를 참조하지 않아서 문제가 없었다. Task 14a 가 처음
// 참조하면서 실제로 겪은 문제: PlatformIO 의 chain LDF 는 소스 파일의
// `#include` 문을 보고 어떤 `lib/<name>/` 를 빌드에 끌어올지 정하는데,
// `main.cpp` 에 `extern "C" const lv_font_t font_ko;` 선언만 적어서는
// `#include` 가 없으니 `font_ko.c` 자체가 컴파일 대상에서 빠지고
// (`undefined reference to font_ko` 로 실제 링크 에러가 났다), 이 헤더가
// 그 `#include` 대상이 되어 준다.
//
// 선언 내용은 `font_ko.c` 맨 끝의 정의와 정확히 같아야 한다(타입이 다르면
// ODR 위반). `font_ko.c` 는 순수 C 파일이라 C 링키지로 컴파일되므로 여기서도
// `extern "C"` 로 감싼다.
#pragma once

// font_ko.c 상단과 같은 가드 — LV_LVGL_H_INCLUDE_SIMPLE 을 정의하지 않는 한
// (이 프로젝트의 lv_conf.h 는 정의하지 않는다) "lvgl/lvgl.h" 경로를 쓴다.
#ifdef LV_LVGL_H_INCLUDE_SIMPLE
#include "lvgl.h"
#else
#include "lvgl/lvgl.h"
#endif

#ifdef __cplusplus
extern "C" {
#endif

extern const lv_font_t font_ko;

#ifdef __cplusplus
}
#endif
