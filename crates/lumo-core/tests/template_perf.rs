//! P1-3 全局性能税:`template_ctx()` 深拷贝放大的基准式回归测试。
//!
//! 模拟 vm.rs 的真实节奏:每步调用 `template_ctx()` 3 次(when 判断、输入渲染、
//! 控制流 inline 渲染),每步执行完把一块大输出记进 steps map。改动前,每次
//! `template_ctx()` 都对累积的 steps/vars/bindings 做全量深拷贝,且每次
//! `render` 内部再克隆 + serialize 一遍 —— O(步数 × 累积输出体积) 的放大;
//! 改动后应为 Arc 克隆 + 每个状态代数至多一次 serialize。
//!
//! 手动运行(耗时,默认 ignored):
//!   cargo test -p lumo-core --test template_perf -- --ignored --nocapture

use lumo_core::ctx::StepCtx;
use lumo_core::ActionRegistry;
use lumo_dsl::Capabilities;
use serde_json::json;
use std::time::Instant;

fn bench_ctx() -> StepCtx {
    StepCtx::new(
        "run-bench".into(),
        "flow-bench".into(),
        ActionRegistry::new(),
        None,
        json!({ "who": "bench" }),
        Capabilities::default(),
        Vec::new(),
    )
}

/// 跑 `steps_n` 步,每步绑 `payload_kib` KiB 输出、渲染 3 次,返回总耗时。
fn run_bench(steps_n: usize, payload_kib: usize) -> std::time::Duration {
    let ctx = bench_ctx();
    let payload = "x".repeat(payload_kib * 1024);
    let started = Instant::now();
    for i in 0..steps_n {
        // 1) when 判断(表达式模式,走 lookup_path)。
        let tc = ctx.template_ctx();
        assert!(lumo_dsl::eval_predicate("inputs.who == 'bench'", &tc).unwrap());

        // 2) 输入渲染:带前缀文本的字符串插值,强制走 minijinja 全量 scope。
        let tc = ctx.template_ctx();
        let tpl = if i == 0 {
            "p={{ inputs.who }}".to_string()
        } else {
            format!("p={{{{ steps.s{}.result.size }}}}", i - 1)
        };
        lumo_dsl::render(&json!({ "v": tpl }), &tc).unwrap();

        // 3) 控制流 inline 渲染:纯路径查找快路径。
        let tc = ctx.template_ctx();
        lumo_dsl::render(&json!("{{ inputs.who }}"), &tc).unwrap();

        // 步骤完成:大输出进 steps map,外加一次 set_var(bind 语义)。
        ctx.record_step_output(
            &format!("s{i}"),
            &json!({ "size": i, "body": payload.clone() }),
        );
        ctx.set_var("last", json!(i));
    }
    started.elapsed()
}

#[test]
#[ignore = "基准测试,手动运行采集耗时"]
fn template_ctx_amplification_bench() {
    // 预热一轮小的,摊掉一次性开销。
    let _ = run_bench(10, 16);
    for (n, kib) in [(100usize, 256usize), (200, 1024)] {
        let took = run_bench(n, kib);
        println!(
            "bench steps={n} payload={kib}KiB total={:.3}s per-step={:.3}ms",
            took.as_secs_f64(),
            took.as_secs_f64() * 1000.0 / n as f64
        );
    }
}
