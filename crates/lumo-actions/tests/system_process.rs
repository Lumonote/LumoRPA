//! `system.process_kill` / `system.app_start` 集成测试。两动作共用
//! `LUMO_ALLOW_PROCESS` 环境闸,而环境变量是进程全局的 —— 同一测试二进制里的
//! 并行用例会互踩,故全部场景串进**一个**测试函数按序跑:先验默认拒绝,再开
//! 闸验行为,最后撤闸。(其它测试文件是独立进程,不受影响。)

mod common;
use common::run;
use serde_json::json;

/// 立即退出的子进程(拿一个「确定不存在」的 pid 用)。
fn quick_exit_cmd() -> std::process::Command {
    if cfg!(windows) {
        let mut c = std::process::Command::new("cmd");
        c.args(["/C", "exit", "0"]);
        c
    } else {
        std::process::Command::new("true")
    }
}

/// 长跑子进程(被 kill 的靶子)。Windows 没有 sleep,用 ping 凑时长。
fn long_sleep_cmd() -> std::process::Command {
    if cfg!(windows) {
        let mut c = std::process::Command::new("ping");
        c.args(["-n", "30", "127.0.0.1"]);
        c
    } else {
        let mut c = std::process::Command::new("sleep");
        c.arg("30");
        c
    }
}

#[tokio::test]
async fn process_actions_gate_and_behavior() {
    // ── 1. 默认拒绝(未设 LUMO_ALLOW_PROCESS):门禁先于一切副作用 ─────────
    std::env::remove_var("LUMO_ALLOW_PROCESS");
    let err = run("system.process_kill", json!({"pid": 1}))
        .await
        .unwrap_err();
    assert!(err.contains("disabled"), "got: {err}");
    let err = run("system.app_start", json!({"program": "true"}))
        .await
        .unwrap_err();
    assert!(err.contains("disabled"), "got: {err}");

    std::env::set_var("LUMO_ALLOW_PROCESS", "1");

    // ── 2. 防呆:拒杀自身 pid / pid 0 ─────────────────────────────────────
    let me = std::process::id();
    let err = run("system.process_kill", json!({"pid": me}))
        .await
        .unwrap_err();
    assert!(err.contains("own process"), "got: {err}");
    let err = run("system.process_kill", json!({"pid": 0}))
        .await
        .unwrap_err();
    assert!(err.contains("pid 0"), "got: {err}");

    // ── 3. 杀不存在的 pid:显式报错,不沉默 ───────────────────────────────
    // 用「刚退出并收尸的子进程」的 pid:短窗口内基本不会被系统复用。
    let mut gone = quick_exit_cmd().spawn().expect("spawn quick-exit child");
    let gone_pid = gone.id();
    gone.wait().expect("reap quick-exit child");
    let err = run("system.process_kill", json!({"pid": gone_pid}))
        .await
        .unwrap_err();
    assert!(err.contains("no such process"), "got: {err}");

    // ── 4. 杀自己 spawn 的子进程:force 强杀成功 ──────────────────────────
    let mut child = long_sleep_cmd().spawn().expect("spawn long-sleep child");
    let pid = child.id();
    let out = run("system.process_kill", json!({"pid": pid, "force": true}))
        .await
        .expect("force kill own child");
    assert_eq!(out["pid"], json!(pid));
    assert_eq!(
        out["signal"],
        json!("kill"),
        "force ⇒ SIGKILL/TerminateProcess"
    );
    let status = child.wait().expect("reap killed child");
    assert!(!status.success(), "SIGKILL 终止的子进程不应正常退出");

    // ── 5. 非 force:Unix 走 SIGTERM(可优雅处理),Windows 回落强杀 ───────
    let mut child = long_sleep_cmd().spawn().expect("spawn long-sleep child #2");
    let pid = child.id();
    let out = run("system.process_kill", json!({"pid": pid}))
        .await
        .expect("term kill own child");
    let expect_signal = if cfg!(windows) { "kill" } else { "term" };
    assert_eq!(out["signal"], json!(expect_signal));
    child.wait().expect("reap term-killed child");

    // ── 6. dry_run:返回预览但不终止进程 ────────────────────────────────
    let mut child = long_sleep_cmd().spawn().expect("spawn dry-run child");
    let pid = child.id();
    let out = run(
        "system.process_kill",
        json!({"pid": pid, "force": true, "dry_run": true}),
    )
    .await
    .expect("preview process kill");
    assert_eq!(out["dry_run"], json!(true));
    assert_eq!(out["would_kill"], json!(true));
    assert!(
        child.try_wait().unwrap().is_none(),
        "dry_run must leave child alive"
    );
    child.kill().unwrap();
    child.wait().unwrap();

    // ── 7. app_start:输入校验 + 启动成功返回 pid(detached,不等退出)─────
    let err = run("system.app_start", json!({"program": "  "}))
        .await
        .unwrap_err();
    assert!(err.contains("must not be empty"), "got: {err}");
    let err = run(
        "system.app_start",
        json!({"program": "lumo-no-such-binary-zzz9"}),
    )
    .await
    .unwrap_err();
    assert!(err.contains("spawn"), "got: {err}");

    let (program, args): (&str, Vec<&str>) = if cfg!(windows) {
        ("ping", vec!["-n", "30", "127.0.0.1"])
    } else {
        ("sleep", vec!["30"])
    };
    let out = run(
        "system.app_start",
        json!({"program": program, "args": args}),
    )
    .await
    .expect("app_start a long-running child");
    let started_pid = out["pid"].as_u64().expect("pid in output") as u32;
    assert!(started_pid > 0, "got: {out}");
    assert_eq!(out["launcher"], json!("direct"));
    // 返回的 pid 是真实活进程:用 process_kill 收掉,顺带交叉验证两动作。
    let out = run(
        "system.process_kill",
        json!({"pid": started_pid, "force": true}),
    )
    .await
    .expect("kill the app_start child");
    assert_eq!(out["pid"], json!(started_pid));

    std::env::remove_var("LUMO_ALLOW_PROCESS");
}
