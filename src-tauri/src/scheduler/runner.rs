use tokio::process::Command;

#[cfg(target_os = "windows")]
fn hide_console_window(cmd: &mut Command) -> &mut Command {
    use std::os::windows::process::CommandExt;

    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    cmd.creation_flags(CREATE_NO_WINDOW)
}

#[cfg(not(target_os = "windows"))]
fn hide_console_window(cmd: &mut Command) -> &mut Command {
    cmd
}

// 지정된 agent 바이너리로 prompt를 1회성으로 실행
pub async fn run_trigger(agent: &str, prompt: &str, working_dir: &str) {
    let mut cmd = if agent == "codex" {
        let mut c = Command::new("codex");
        c.args(["exec", "--skip-git-repo-check", "-C", working_dir, prompt]);
        c
    } else {
        let mut c = Command::new("claude");
        c.args(["-p", prompt]).current_dir(working_dir);
        c
    };

    let result = hide_console_window(&mut cmd).spawn();
    match result {
        Ok(_) => tracing::info!(%agent, %prompt, "trigger 실행됨"),
        // 바이너리 없거나 spawn 실패 — 앱 크래시 없이 warn 로그만 남김
        Err(e) => tracing::warn!(%agent, %e, "trigger spawn 실패 (바이너리 없거나 경로 오류)"),
    }
}
