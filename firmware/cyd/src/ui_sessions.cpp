// AI Agent Monitor — CYD 펌웨어: 프로젝트 및 세션 목록 화면 (Task 15b).
//
// Mac SessionList.svelte 와 동일한 정렬 및 색상 규칙을 적용한다.
#include "ui_sessions.h"

#include <Arduino.h>
#include <stdio.h>
#include <algorithm>
#include "font_ko.h"
#include "snapshot.h"

namespace {

constexpr size_t MAX_SESSION_ROWS = 16;

struct SessionItemWidgets {
    lv_obj_t *row = nullptr;
    lv_obj_t *dot = nullptr;
    lv_obj_t *titleLabel = nullptr;
    lv_obj_t *modelLabel = nullptr;
    lv_obj_t *statusLabel = nullptr;
    lv_obj_t *timeLabel = nullptr;
};

struct FlatSession {
    SnapshotAgentKind agentKind;
    SnapshotProject project;
};

lv_obj_t *g_sessionsRoot = nullptr;
lv_obj_t *g_sessionsContainer = nullptr;
lv_obj_t *g_noSessionsLabel = nullptr;
SessionItemWidgets g_rows[MAX_SESSION_ROWS];

// 상태 점(dot) 색상 계산
lv_color_t getDotColor(SnapshotAgentKind kind, SnapshotProjectStatus status) {
    if (status == SnapshotProjectStatus::Dormant) {
        return lv_color_hex(0x636366);  // 휴면
    }
    if (status == SnapshotProjectStatus::Idle) {
        return lv_color_hex(0xff9f0a);  // 유휴
    }
    // Active
    switch (kind) {
        case SnapshotAgentKind::Claude: return lv_color_hex(0x30d158);       // 초록
        case SnapshotAgentKind::Antigravity: return lv_color_hex(0x388bfd);  // 파랑
        case SnapshotAgentKind::Codex: return lv_color_hex(0xff9f0a);        // 주황
        default: return lv_color_hex(0xff9f0a);
    }
}

// 상대 시각 포맷팅
void formatRelativeTime(uint64_t lastEventSec, uint64_t currentSec, char *buf, size_t bufSize) {
    if (lastEventSec == 0 || currentSec <= lastEventSec) {
        snprintf(buf, bufSize, "방금");
        return;
    }
    uint64_t diff = currentSec - lastEventSec;
    if (diff < 60) {
        snprintf(buf, bufSize, "방금");
    } else if (diff < 3600) {
        snprintf(buf, bufSize, "%u분 전", (unsigned)(diff / 60));
    } else if (diff < 86400) {
        snprintf(buf, bufSize, "%u시간 전", (unsigned)(diff / 3600));
    } else {
        snprintf(buf, bufSize, "%u일 전", (unsigned)(diff / 86400));
    }
}

}  // namespace

lv_obj_t *uiSessionsCreate(lv_obj_t *parent) {
    if (parent == nullptr) {
        parent = lv_screen_active();
    }

    g_sessionsRoot = lv_obj_create(parent);
    lv_obj_set_size(g_sessionsRoot, lv_pct(100), lv_pct(100));
    lv_obj_set_pos(g_sessionsRoot, 0, 0);
    lv_obj_set_style_bg_opa(g_sessionsRoot, LV_OPA_TRANSP, 0);
    lv_obj_set_style_border_width(g_sessionsRoot, 0, 0);
    lv_obj_set_style_pad_all(g_sessionsRoot, 4, 0);

    g_sessionsContainer = lv_obj_create(g_sessionsRoot);
    lv_obj_set_size(g_sessionsContainer, lv_pct(100), lv_pct(100));
    lv_obj_set_flex_flow(g_sessionsContainer, LV_FLEX_FLOW_COLUMN);
    lv_obj_set_style_bg_color(g_sessionsContainer, lv_color_hex(0x2c2c2e), 0);
    lv_obj_set_style_bg_opa(g_sessionsContainer, LV_OPA_COVER, 0);
    lv_obj_set_style_radius(g_sessionsContainer, 6, 0);
    lv_obj_set_style_border_width(g_sessionsContainer, 0, 0);
    lv_obj_set_style_pad_all(g_sessionsContainer, 6, 0);
    lv_obj_set_style_pad_row(g_sessionsContainer, 4, 0);

    g_noSessionsLabel = lv_label_create(g_sessionsContainer);
    lv_obj_set_style_text_font(g_noSessionsLabel, &font_ko, 0);
    lv_label_set_text(g_noSessionsLabel, "세션 없음");
    lv_obj_center(g_noSessionsLabel);

    // 세션 행 위젯 사전 생성
    for (size_t i = 0; i < MAX_SESSION_ROWS; i++) {
        SessionItemWidgets &rw = g_rows[i];

        rw.row = lv_obj_create(g_sessionsContainer);
        lv_obj_set_size(rw.row, lv_pct(100), LV_SIZE_CONTENT);
        lv_obj_set_style_bg_opa(rw.row, LV_OPA_TRANSP, 0);
        lv_obj_set_style_border_width(rw.row, 0, 0);
        lv_obj_set_style_pad_all(rw.row, 2, 0);
        lv_obj_set_flex_flow(rw.row, LV_FLEX_FLOW_ROW);
        lv_obj_set_flex_align(rw.row, LV_FLEX_ALIGN_SPACE_BETWEEN, LV_FLEX_ALIGN_CENTER, LV_FLEX_ALIGN_CENTER);

        // 좌측 컨테이너 (점 + 제목 + 모델)
        lv_obj_t *left = lv_obj_create(rw.row);
        lv_obj_set_size(left, LV_SIZE_CONTENT, LV_SIZE_CONTENT);
        lv_obj_set_style_bg_opa(left, LV_OPA_TRANSP, 0);
        lv_obj_set_style_border_width(left, 0, 0);
        lv_obj_set_style_pad_all(left, 0, 0);
        lv_obj_set_flex_flow(left, LV_FLEX_FLOW_ROW);
        lv_obj_set_flex_align(left, LV_FLEX_ALIGN_START, LV_FLEX_ALIGN_CENTER, LV_FLEX_ALIGN_CENTER);
        lv_obj_set_style_pad_column(left, 4, 0);

        rw.dot = lv_obj_create(left);
        lv_obj_set_size(rw.dot, 6, 6);
        lv_obj_set_style_radius(rw.dot, LV_RADIUS_CIRCLE, 0);
        lv_obj_set_style_border_width(rw.dot, 0, 0);

        rw.titleLabel = lv_label_create(left);
        lv_obj_set_style_text_font(rw.titleLabel, &font_ko, 0);
        lv_obj_set_style_text_color(rw.titleLabel, lv_color_hex(0xffffff), 0);
        lv_label_set_text(rw.titleLabel, "");

        rw.modelLabel = lv_label_create(left);
        lv_obj_set_style_text_font(rw.modelLabel, &font_ko, 0);
        lv_obj_set_style_text_color(rw.modelLabel, lv_color_hex(0x8e8e93), 0);
        lv_label_set_text(rw.modelLabel, "");

        // 우측 컨테이너 (상태/속도 + 상대시각)
        lv_obj_t *right = lv_obj_create(rw.row);
        lv_obj_set_size(right, LV_SIZE_CONTENT, LV_SIZE_CONTENT);
        lv_obj_set_style_bg_opa(right, LV_OPA_TRANSP, 0);
        lv_obj_set_style_border_width(right, 0, 0);
        lv_obj_set_style_pad_all(right, 0, 0);
        lv_obj_set_flex_flow(right, LV_FLEX_FLOW_ROW);
        lv_obj_set_flex_align(right, LV_FLEX_ALIGN_START, LV_FLEX_ALIGN_CENTER, LV_FLEX_ALIGN_CENTER);
        lv_obj_set_style_pad_column(right, 8, 0);

        rw.statusLabel = lv_label_create(right);
        lv_obj_set_style_text_font(rw.statusLabel, &font_ko, 0);
        lv_label_set_text(rw.statusLabel, "");

        rw.timeLabel = lv_label_create(right);
        lv_obj_set_style_text_font(rw.timeLabel, &font_ko, 0);
        lv_obj_set_style_text_color(rw.timeLabel, lv_color_hex(0x8e8e93), 0);
        lv_label_set_text(rw.timeLabel, "");

        lv_obj_add_flag(rw.row, LV_OBJ_FLAG_HIDDEN);
    }

    return g_sessionsRoot;
}

void uiSessionsUpdate(const Transport &transport) {
    if (!transport.hasSnapshot()) {
        lv_obj_clear_flag(g_noSessionsLabel, LV_OBJ_FLAG_HIDDEN);
        for (size_t i = 0; i < MAX_SESSION_ROWS; i++) {
            lv_obj_add_flag(g_rows[i].row, LV_OBJ_FLAG_HIDDEN);
        }
        return;
    }

    const Snapshot &snap = transport.latestSnapshot();
    FlatSession sessions[MAX_SESSION_ROWS];
    size_t count = 0;

    for (size_t a = 0; a < snap.agentCount; a++) {
        const SnapshotAgent &ag = snap.agents[a];
        for (size_t p = 0; p < ag.projectCount; p++) {
            if (count >= MAX_SESSION_ROWS) break;
            sessions[count].agentKind = ag.kind;
            sessions[count].project = ag.projects[p];
            count++;
        }
    }

    if (count == 0) {
        lv_obj_clear_flag(g_noSessionsLabel, LV_OBJ_FLAG_HIDDEN);
        for (size_t i = 0; i < MAX_SESSION_ROWS; i++) {
            lv_obj_add_flag(g_rows[i].row, LV_OBJ_FLAG_HIDDEN);
        }
        return;
    }

    lv_obj_add_flag(g_noSessionsLabel, LV_OBJ_FLAG_HIDDEN);

    // 최근 활동 순 정렬 (lastActivityEpochSec 내림차순)
    std::sort(sessions, sessions + count, [](const FlatSession &a, const FlatSession &b) {
        return a.project.lastActivityEpochSec > b.project.lastActivityEpochSec;
    });

    uint64_t currentSec = snap.emittedAtEpochSec;

    for (size_t i = 0; i < MAX_SESSION_ROWS; i++) {
        SessionItemWidgets &rw = g_rows[i];
        if (i >= count) {
            lv_obj_add_flag(rw.row, LV_OBJ_FLAG_HIDDEN);
            continue;
        }

        lv_obj_clear_flag(rw.row, LV_OBJ_FLAG_HIDDEN);
        const FlatSession &s = sessions[i];

        // 점 색상
        lv_obj_set_style_bg_color(rw.dot, getDotColor(s.agentKind, s.project.status), 0);

        // 에이전트명 + 프로젝트명
        const char *agName = (s.agentKind == SnapshotAgentKind::Claude) ? "Claude" :
                             (s.agentKind == SnapshotAgentKind::Antigravity) ? "Antigravity" : "Codex";
        char titleBuf[64];
        snprintf(titleBuf, sizeof(titleBuf), "%s · %s", agName, s.project.name.c_str());
        lv_label_set_text(rw.titleLabel, titleBuf);

        // 모델명
        lv_label_set_text(rw.modelLabel, s.project.model.c_str());

        // 상태 / 속도
        if (s.project.status == SnapshotProjectStatus::Active) {
            char rateBuf[32];
            snprintf(rateBuf, sizeof(rateBuf), "%.1f tok/s", s.project.rateTokPerSec);
            lv_label_set_text(rw.statusLabel, rateBuf);
            lv_obj_set_style_text_color(rw.statusLabel, lv_color_hex(0x0a84ff), 0);
        } else if (s.project.status == SnapshotProjectStatus::Idle) {
            lv_label_set_text(rw.statusLabel, "유휴");
            lv_obj_set_style_text_color(rw.statusLabel, lv_color_hex(0x8e8e93), 0);
        } else {
            lv_label_set_text(rw.statusLabel, "휴면");
            lv_obj_set_style_text_color(rw.statusLabel, lv_color_hex(0x636366), 0);
        }

        // 상대 시각
        char timeBuf[32];
        formatRelativeTime(s.project.lastActivityEpochSec, currentSec, timeBuf, sizeof(timeBuf));
        lv_label_set_text(rw.timeLabel, timeBuf);
    }
}

void uiSessionsSetVisible(bool visible) {
    if (g_sessionsRoot != nullptr) {
        if (visible) {
            lv_obj_clear_flag(g_sessionsRoot, LV_OBJ_FLAG_HIDDEN);
        } else {
            lv_obj_add_flag(g_sessionsRoot, LV_OBJ_FLAG_HIDDEN);
        }
    }
}
