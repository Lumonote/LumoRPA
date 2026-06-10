//! P1-3 模板上下文代数缓存的语义钉子:
//!   1. 同代数内快照复用(Arc 共享 + minijinja scope 只 serialize 一次);
//!   2. vars/steps/bindings 变更后渲染必须看到新值(失效正确性);
//!   3. parallel fork 的写时复制隔离不被 Arc 共享破坏。

use lumo_core::ctx::StepCtx;
use lumo_core::ActionRegistry;
use lumo_dsl::Capabilities;
use serde_json::json;
use std::sync::Arc;

fn ctx() -> StepCtx {
    StepCtx::new(
        "run-cache".into(),
        "flow-cache".into(),
        ActionRegistry::new(),
        None,
        json!({ "who": "cache" }),
        Capabilities::default(),
        Vec::new(),
    )
}

#[test]
fn same_generation_snapshots_share_arcs_and_scope() {
    let ctx = ctx();
    ctx.record_step_output("s1", &json!("v1"));

    let tc1 = ctx.template_ctx();
    let tc2 = ctx.template_ctx();
    // 同代数:两次快照共享同一份 steps/vars 分配(零深拷贝)。
    assert!(Arc::ptr_eq(&tc1.steps, &tc2.steps), "steps 必须 Arc 共享");
    assert!(Arc::ptr_eq(&tc1.vars, &tc2.vars), "vars 必须 Arc 共享");
    // 共享同一个 scope 缓存槽:一次渲染构建后,后续渲染复用(serialize 3N→N)。
    assert!(Arc::ptr_eq(&tc1.scope, &tc2.scope), "scope 缓存必须共享");
    assert!(tc1.scope.get().is_none(), "渲染前 scope 尚未构建");
    // 带前缀文本的插值,强制走 minijinja 全量 scope(纯路径快路径不触发)。
    let out = lumo_dsl::render(&json!("v={{ steps.s1.result }}"), &tc1).unwrap();
    assert_eq!(out, json!("v=v1"));
    assert!(
        tc2.scope.get().is_some(),
        "tc1 渲染构建的 scope 必须对 tc2 可见(同代数内不再重复 serialize)"
    );
}

#[test]
fn set_var_between_renders_is_visible() {
    let ctx = ctx();
    ctx.set_var("flag", json!("old"));
    let tc = ctx.template_ctx();
    assert_eq!(
        lumo_dsl::render(&json!("f={{ vars.flag }}"), &tc).unwrap(),
        json!("f=old")
    );
    // 渲染期间修改 vars,再渲染必须看到新值(代数缓存失效正确性)。
    ctx.set_var("flag", json!("new"));
    let tc2 = ctx.template_ctx();
    assert!(
        !Arc::ptr_eq(&tc.vars, &tc2.vars),
        "set_var 后必须换代,不能复用旧快照"
    );
    assert_eq!(
        lumo_dsl::render(&json!("f={{ vars.flag }}"), &tc2).unwrap(),
        json!("f=new")
    );
    // 旧快照保持当时的视图(快照语义不变)。
    assert_eq!(
        lumo_dsl::render(&json!("f={{ vars.flag }}"), &tc).unwrap(),
        json!("f=old")
    );
}

#[test]
fn step_output_and_bindings_invalidate_cache() {
    let ctx = ctx();
    let tc = ctx.template_ctx();
    ctx.record_step_output("s1", &json!("out1"));
    let tc2 = ctx.template_ctx();
    assert!(!Arc::ptr_eq(&tc.steps, &tc2.steps));
    assert_eq!(
        lumo_dsl::render(&json!("{{ steps.s1.result }}"), &tc2).unwrap(),
        json!("out1")
    );

    ctx.push_binding("item", json!(7));
    let tc3 = ctx.template_ctx();
    assert_eq!(
        lumo_dsl::render(&json!("i={{ item }}"), &tc3).unwrap(),
        json!("i=7")
    );
    ctx.clear_binding("item");
    let tc4 = ctx.template_ctx();
    assert!(
        lumo_dsl::render(&json!("i={{ item }}"), &tc4).is_err(),
        "清除绑定后 `item` 必须回到未定义(SemiStrict 报错)"
    );
}

#[test]
fn fork_isolation_survives_arc_sharing() {
    let parent = ctx();
    parent.set_var("v", json!("p"));
    let _warm = parent.template_ctx(); // 父级先建好缓存

    let a = parent.fork();
    let b = parent.fork();
    // 分支内写入走写时复制,不污染兄弟分支与父级。
    a.set_var("v", json!("a"));
    a.record_step_output("sa", &json!("ra"));

    let tb = b.template_ctx();
    assert_eq!(
        lumo_dsl::render(&json!("{{ vars.v }}"), &tb).unwrap(),
        json!("p"),
        "兄弟分支不得看到 a 的写入"
    );
    assert!(
        lumo_dsl::render(&json!("x={{ steps.sa.result }}"), &tb).is_err(),
        "兄弟分支不得看到 a 的步骤输出"
    );
    let tp = parent.template_ctx();
    assert_eq!(
        lumo_dsl::render(&json!("{{ vars.v }}"), &tp).unwrap(),
        json!("p"),
        "父级在 merge 前不得看到分支写入"
    );

    // join 后按分支顺序 merge,父级才看到分支结果。
    parent.merge_branch(&a);
    let tp2 = parent.template_ctx();
    assert_eq!(
        lumo_dsl::render(&json!("{{ vars.v }}"), &tp2).unwrap(),
        json!("a")
    );
    assert_eq!(
        lumo_dsl::render(&json!("{{ steps.sa.result }}"), &tp2).unwrap(),
        json!("ra")
    );
}
