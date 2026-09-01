# Codex 저장 형식 호환성

> 현재 기본 경로는 SQLite가 아니라 `~/.codex/sessions/YYYY/MM/DD/rollout-*.jsonl`이다. Codex 0.151에서 `state_5.sqlite`의 `threads` 테이블이 사라진 것을 확인했으며, 앱 v1.7.2부터 날짜별 rollout을 직접 탐색한다. 아래 SQLite 내용은 2026-05-28 당시 구버전 호환과 회귀 분석을 위한 기록이다.

## 개요

Claude Code의 Codex 백엔드는 두 개의 SQLite 데이터베이스를 관리합니다:
- **logs_2.sqlite**: 시스템 로그 (구조화되지 않은 텍스트 로그)
- **state_5.sqlite**: 상태 정보 (스레드, 작업, 메모리 등)

토큰 사용량 추적은 **state_5.sqlite**의 `threads` 테이블에 집중되어 있습니다.

---

## 파일별 테이블

### ~/.codex/logs_2.sqlite

#### _sqlx_migrations
SQLx ORM 마이그레이션 메타데이터 (시스템 테이블)
```sql
CREATE TABLE _sqlx_migrations (
    version BIGINT PRIMARY KEY,
    description TEXT NOT NULL,
    installed_on TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
    success BOOLEAN NOT NULL,
    checksum BLOB NOT NULL,
    execution_time BIGINT NOT NULL
);
```

#### logs
시스템 레벨 디버그/정보 로그 (구조화 텍스트)
```sql
CREATE TABLE logs (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    ts INTEGER NOT NULL,                      -- Unix timestamp (seconds)
    ts_nanos INTEGER NOT NULL,                -- Nanosecond precision
    level TEXT NOT NULL,                      -- TRACE, DEBUG, INFO, WARN, ERROR
    target TEXT NOT NULL,                     -- Log module/target name
    feedback_log_body TEXT,                   -- Unstructured log message
    module_path TEXT,                         -- Rust module path (if applicable)
    file TEXT,                                -- Source file
    line INTEGER,                             -- Source line number
    thread_id TEXT,                           -- Claude Code thread ID (optional)
    process_uuid TEXT,                        -- Process UUID
    estimated_bytes INTEGER NOT NULL DEFAULT 0
);
```

**용도**: 토큰 사용량 추적에는 직접 사용 불가. 이 테이블은 시스템 로그이며, 토큰 정보는 `state_5.sqlite`에 있습니다.

**인덱스**:
- `idx_logs_ts`: 시간순 조회용
- `idx_logs_thread_id`: 스레드별 조회용
- `idx_logs_process_uuid_threadless_ts`: 프로세스별 조회용

---

### ~/.codex/state_5.sqlite

#### _sqlx_migrations
SQLx ORM 마이그레이션 메타데이터 (시스템 테이블, logs_2.sqlite와 동일)

#### threads
**토큰 추적의 핵심 테이블** — 모든 Claude Code 세션/스레드의 메타데이터
```sql
CREATE TABLE threads (
    id TEXT PRIMARY KEY,                      -- 고유 스레드 ID (ULID 형식)
    rollout_path TEXT NOT NULL,               -- Feature rollout path
    created_at INTEGER NOT NULL,              -- Unix timestamp (seconds)
    updated_at INTEGER NOT NULL,              -- Unix timestamp (seconds)
    created_at_ms INTEGER,                    -- Unix timestamp (milliseconds, trigger로 자동 설정)
    updated_at_ms INTEGER,                    -- Unix timestamp (milliseconds, trigger로 자동 설정)
    source TEXT NOT NULL,                     -- 'vscode', 'cli', etc.
    model_provider TEXT NOT NULL,             -- 'anthropic'
    model TEXT,                               -- 'gpt-5.4', 'gpt-5.5', etc.
    cwd TEXT NOT NULL,                        -- 작업 디렉토리 (프로젝트 경로)
    title TEXT NOT NULL,                      -- 세션 제목
    sandbox_policy TEXT NOT NULL,
    approval_mode TEXT NOT NULL,
    tokens_used INTEGER NOT NULL DEFAULT 0,   -- ★ 누적 토큰 사용량 (정수)
    has_user_event INTEGER NOT NULL DEFAULT 0,
    archived INTEGER NOT NULL DEFAULT 0,      -- Boolean (0/1)
    archived_at INTEGER,                      -- Unix timestamp
    git_sha TEXT,                             -- Git commit SHA
    git_branch TEXT,                          -- Git branch name
    git_origin_url TEXT,                      -- Git origin URL
    cli_version TEXT NOT NULL DEFAULT '',     -- CLI 버전
    first_user_message TEXT NOT NULL DEFAULT '',
    agent_nickname TEXT,                      -- 에이전트 별칭 (있는 경우)
    agent_role TEXT,                          -- 에이전트 역할
    memory_mode TEXT NOT NULL DEFAULT 'enabled',
    reasoning_effort TEXT,                    -- 'low', 'medium', 'high' (등)
    agent_path TEXT,                          -- 에이전트 경로
    thread_source TEXT,                       -- 추가 스레드 소스 정보
    preview TEXT NOT NULL DEFAULT ''
);
```

**중요 필드**:
- `tokens_used`: 스레드가 소비한 **총 토큰 수** (입력 + 출력 + 캐시)
- `created_at_ms` / `updated_at_ms`: 밀리초 단위 타임스탐프 (트리거로 자동 유지)
- `model`: 사용된 모델명
- `cwd`: 프로젝트 경로 (필터링에 유용)

**인덱스**:
- `idx_threads_created_at_ms`: 시간순 조회
- `idx_threads_updated_at_ms`: 업데이트 시간순 조회
- `idx_threads_archived`: 보관 상태 필터링
- `idx_threads_source`: 소스별 필터링

**샘플 데이터** (상위 5개):
```
id: 019cff32-f134-7c50-ae19-0eeca4316e3c | model: gpt-5.4 | tokens_used: 181925662 | cwd: /Users/wannypark/Desktop/@Projects/3_Seqnex_tuist | created_at_ms: 1773808054581
id: 019d4baf-33fc-76c3-9f22-beced78e071c | model: gpt-5.4 | tokens_used: 94735304  | cwd: /Users/wannypark/Desktop/@Projects/2_App/3_Seqnex | created_at_ms: 1775091266561
id: 019d6114-a43d-7111-a656-17e142e2eed4 | model: gpt-5.4 | tokens_used: 80572316  | cwd: /Users/wannypark/Desktop/@Projects/2_App/3_Seqnex | created_at_ms: 1775450235971
id: 019d4178-68af-7d53-83ac-610a5565026d | model: gpt-5.4 | tokens_used: 58887006  | cwd: /Users/wannypark/Desktop/@Projects/2_App/3_Seqnex | created_at_ms: 1774919903414
id: 019d2401-9c41-7d71-b211-ac92a046e848 | model: gpt-5.4 | tokens_used: 58220606  | cwd: /Users/wannypark/Desktop/@Projects/5_nViewer1_origin | created_at_ms: 1774425578568
```

#### thread_dynamic_tools
각 스레드에 할당된 동적 도구 목록
```sql
CREATE TABLE thread_dynamic_tools (
    thread_id TEXT NOT NULL,
    position INTEGER NOT NULL,
    name TEXT NOT NULL,
    description TEXT NOT NULL,
    input_schema TEXT NOT NULL,
    defer_loading INTEGER NOT NULL DEFAULT 0,
    namespace TEXT,
    PRIMARY KEY(thread_id, position),
    FOREIGN KEY(thread_id) REFERENCES threads(id) ON DELETE CASCADE
);
```

#### stage1_outputs
메모리/롤아웃 정보 (토큰 추적에는 불필요)
```sql
CREATE TABLE stage1_outputs (
    thread_id TEXT PRIMARY KEY,
    source_updated_at INTEGER NOT NULL,
    raw_memory TEXT NOT NULL,
    rollout_summary TEXT NOT NULL,
    generated_at INTEGER NOT NULL,
    rollout_slug TEXT,
    usage_count INTEGER,
    last_usage INTEGER,
    selected_for_phase2 INTEGER NOT NULL DEFAULT 0,
    selected_for_phase2_source_updated_at INTEGER,
    FOREIGN KEY(thread_id) REFERENCES threads(id) ON DELETE CASCADE
);
```

#### jobs, backfill_state, agent_jobs, agent_job_items, thread_spawn_edges
Codex 내부 작업 관리/에이전트 작업 관련 테이블 (이 프로젝트에는 불필요)

#### remote_control_enrollments
원격 제어 등록 정보 (이 프로젝트에는 불필요)

---

## 토큰 사용량 위치

### 핵심 쿼리
```sql
SELECT 
    id,
    model,
    tokens_used,
    cwd,
    created_at_ms,
    updated_at_ms
FROM threads
WHERE archived = 0
ORDER BY created_at_ms DESC;
```

### 컬럼 매핑

| 의미 | 컬럼명 | 타입 | 설명 |
|------|--------|------|------|
| 스레드 ID | `id` | TEXT | ULID 형식의 고유 식별자 |
| 토큰 (총계) | `tokens_used` | INTEGER | 입력+출력+캐시 포함 누적값 |
| 모델 | `model` | TEXT | 'gpt-5.4', 'gpt-5.5' 등 |
| 프로젝트 경로 | `cwd` | TEXT | 작업 디렉토리 (필터링용) |
| 생성 시각 | `created_at_ms` | INTEGER | Unix timestamp (밀리초) |
| 수정 시각 | `updated_at_ms` | INTEGER | Unix timestamp (밀리초) |
| 소스 | `source` | TEXT | 'vscode', 'cli' 등 |

### 토큰 세부 정보 부재
**주의**: `tokens_used`는 **합계만 제공**합니다. 다음 정보는 **state_5.sqlite에 없습니다**:
- 입력 토큰 개수 (`prompt_tokens`)
- 출력 토큰 개수 (`completion_tokens`)
- 캐시 생성 토큰 (`cache_creation_tokens`)
- 캐시 읽기 토큰 (`cache_read_tokens`)

**결론**: 실시간 모니터링을 위해서는 Codex의 API 또는 로그 스트림을 사용해야 합니다. SQLite는 "현재 세션의 누적 토큰"만 제공합니다.

---

## Quota 정보

**발견 못함**. state_5.sqlite 또는 logs_2.sqlite에 `quota_limit`, `quota_used`, 또는 유사한 필드가 없습니다.

**처리 방법**: AI Agent Monitor는 quota를 추적하지 않습니다. 필요시:
- Codex API 문서 참고
- 또는 런타임에 Codex 프로세스의 config 파일 읽기

---

## WAL 모드 및 동시 접근

### Journal Mode
```
logs_2.sqlite:  wal
state_5.sqlite: wal
```

### 동시 접근 가능
✓ WAL 모드로 설정되어 있으므로, read-only 연결로 Codex가 쓰는 중에도 안전하게 읽을 수 있습니다.

**rusqlite 사용 시**:
```rust
let db = rusqlite::Connection::open_with_flags(
    "~/.codex/state_5.sqlite",
    rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY
)?;
```

---

## 타임스탐프 형식

### created_at / updated_at
- **단위**: Unix timestamp (초 단위)
- **타입**: INTEGER
- **범위**: 1773808054 ~ (현재)

### created_at_ms / updated_at_ms
- **단위**: Unix timestamp (밀리초)
- **타입**: INTEGER
- **범위**: 1773808054581 ~ (현재)
- **자동 유지**: 트리거로 자동으로 `created_at * 1000`으로 설정됨

### 변환 예시
```rust
use std::time::{SystemTime, UNIX_EPOCH};

// 밀리초 → 시간대
let ms = 1773808054581;
let secs = ms / 1000;
let nanos = (ms % 1000) * 1_000_000;
let duration = Duration::new(secs as u64, nanos as u32);
let datetime = SystemTime::UNIX_EPOCH + duration;
```

---

## 데이터 통계 (2026-05-28 기준)

| DB | 테이블 | 레코드 수 | 설명 |
|----|--------|----------|------|
| logs_2.sqlite | logs | 28,276 | 시스템 로그 |
| state_5.sqlite | threads | 102 | 활성/보관 세션 |

---

## 추후 검증 필요

- [ ] Codex CLI 실행 중에 새 row가 logs_2.sqlite에 추가되는지 확인
- [ ] `tokens_used`가 실제로 증가하는지 모니터링 (장시간 실행)
- [ ] 프로젝트별 필터링 성능 (cwd 인덱스)

---

## 참고

**파일 경로**:
- logs_2.sqlite: `~/.codex/logs_2.sqlite`
- state_5.sqlite: `~/.codex/state_5.sqlite`

**WAL 부수 파일** (자동 관리):
- `~/.codex/logs_2.sqlite-wal`
- `~/.codex/logs_2.sqlite-shm`
- `~/.codex/state_5.sqlite-wal`
- `~/.codex/state_5.sqlite-shm`

SQLite는 WAL 모드에서 이 파일들을 자동으로 관리하므로, read-only 접근 시 명시적으로 삭제할 필요는 없습니다.
