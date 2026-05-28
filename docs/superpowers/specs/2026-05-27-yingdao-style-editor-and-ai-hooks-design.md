# 影刀式步骤编辑器 + 节点级 AI 增强钩子 设计文档

| 字段 | 值 |
|---|---|
| Spec ID | `2026-05-27-yingdao-style-editor-and-ai-hooks` |
| 日期 | 2026-05-27 |
| 状态 | Draft（待用户复核） |
| 范围 | P0 子项目（影刀对标 + AI 切入）首批 |
| 依赖文档 | `docs/00-LumoRPA-Master-Report.md` · `docs/01-Product-Design.md` |
| 后续动作 | 复核通过后 → `writing-plans` 出实现计划 |

---

## 1. 背景与动机

LumoRPA 当前的 Tauri Studio 编辑器以 SVG 画布（graph view）为主，外加树视图、代码视图，但缺少影刀 RPA 的"线性编号步骤列表 + 就地配置 + 拖动重排"主交互。用户运行 `examples/browser-scrape.lumoflow.yaml` 时遇到的两个真实问题：

1. **配置入口不直观** — 不知道点节点后右栏 inspector 能编辑（视觉权重低，影刀同款是行内展开）
2. **`capability denied` 硬失败** — 把 URL 改成 `wwww.baidu.com` 跑时报 `capability denied: network 'wwww.baidu.com' is not declared`，没有一键授权出口

更深层：用户的目标是"对标影刀且超越"，希望在保留 LumoRPA 差异化（YAML 即代码、三向同构、Vision-LLM 自愈）的同时，让主编辑面与影刀视觉/交互对齐，并把 AI 增强**动态地切入每个节点**，而不是堆一个单独的"AI 节点"族。

本 spec 设计 P0 子项目：**影刀式线性步骤编辑器 + 节点级 AI 增强开关**，覆盖 4 个最高价值的 AI 切入点。

## 2. 目标 / 非目标

### 2.1 目标（in scope）

- T1：在现有 `[图][树][代码]` 三视图组上新增 `[步骤]` 视图并置为默认；不删除任何现有视图
- T2：步骤行支持折叠 / 展开 / 拖动重排 / + 插入 / 删除 / 复制；展开时复用现有 schema-driven 表单
- T3：YAML 新增 `metadata.ai` 和 `step.ai` 两层字段；向后兼容
- T4：VM 在 `lumo-core::vm` 中加 pre / post AI hook；调用 `lumo-ai` 三个 helper（`heal_selector` / `extract_visual` / `decide`）
- T5：四个 AI 切入点端到端可跑：
  - ① 选择器失效 → 视觉重定位（`browser.click/type/wait_element`）
  - ② 抓取失败 → LLM 看截图抽取（`browser.extract/data.extract`）
  - ③ `control.if` → LLM 语义决策
  - ④ 失败节点 → LLM 诊断报告（流程级开关）
- T6：能力一键授权 toast：`capability denied` 报错时按钮一键扩白名单
- T7：现有 8 个示例 flow 在 `mode: off` 下行为零回归；改造 `browser-scrape` 为示范用例
- T8：预算计数器（`metadata.ai.budget.max_calls_per_run`）超额时 AI 通路停，确定性通路继续

### 2.2 非目标（out of scope，see §10）

- 多 flow 标签页编辑（P2）
- 元素库 / 图像库 `@elements.foo` 引用机制（P1）
- 录制器接入（M2）
- Time-Travel 截图回放（P2）
- Magic Prompt 自然语言生成节点 / 流程（P1）
- 本地视觉模型 OmniParser / UI-TARS 集成（M2）
- MCP 暴露 AI helper 给外部 Agent（M2）
- AI 自动改写整段流程（明确否决）
- 画布视图删除（明确否决，保留为 ★ 差异化辅助视图）

## 3. 用户旅程

### 3.1 场景 A：业务用户跑改 URL 的 browser-scrape

1. 双击左栏 "浏览器抓取示例" → 步骤视图载入 4 个步骤
2. 右栏输入 JSON：`{"url": "https://wwww.baidu.com"}`
3. 点 ▶ 运行 → step `open` 触发 `capability denied`
4. toast 弹出 [➕ 加入 capabilities.network]，点击后 YAML 自动加 `wwww.baidu.com`
5. 重跑 → step `title` 用 selector `h1` 抓不到（百度无 h1）
6. 该节点 `ai.mode: fallback` 已开 → VM 自动调 `ai.extract_visual` 看截图，识别出标题
7. step `is_chinese_site`（`ai.mode: primary`）调 `ai.decide` 看 prompt → true → 进 do 分支
8. 全流程绿，时间线上 2/3 步带紫色 "AI heal" 徽章

### 3.2 场景 B：用户对节点切 AI 增强

1. 用户在步骤视图点 step `title` 行右上 ✨
2. 弹出抽屉：mode 三选一（off/fallback/primary）、model 输入（默认空 = 走流程级）、prompt 文本域（placeholder 显示自动构造结果）
3. 选 fallback、模型留空、prompt 留空 → 保存
4. YAML 写入 `ai: { mode: fallback }`；如果 flow 没有 `capabilities.llm` 则自动补 `["*"]` 并提示 toast
5. 步骤行的 ✨ 图标变绿色（fallback 态）

## 4. 架构

### 4.1 高层组件关系

```
┌─────────────────────────────────────────────────────────────────────┐
│  apps/desktop/frontend (Tauri WebView)                              │
│  ┌─────────────────────────────────────────────────────────────┐   │
│  │  步骤视图 (renderStepList)                                   │   │
│  │   ├─ 步骤行（折叠态）                                         │   │
│  │   ├─ 展开层（复用 renderSchemaFields）                       │   │
│  │   ├─ 拖拽重排（bindStepListDnD）                             │   │
│  │   ├─ + 插入弹出（picker）                                    │   │
│  │   └─ ✨ 抽屉（AI mode/model/prompt）                          │   │
│  └─────────────────────────────────────────────────────────────┘   │
│       ▲ ▼ AST 同步                                                  │
│  ┌─────────────────────────────────────────────────────────────┐   │
│  │  state.ast / state.source （现有 YAML 双向）                  │   │
│  └─────────────────────────────────────────────────────────────┘   │
└─────────────────────────────────────────────────────────────────────┘
                              │ Tauri IPC
                              ▼
┌─────────────────────────────────────────────────────────────────────┐
│  lumo-core / lumo-dsl / lumo-ai (Rust)                              │
│                                                                      │
│  lumo-dsl::ast::Step          + ai: Option<StepAi>                  │
│  lumo-dsl::ast::Metadata      + ai: Option<FlowAi>                  │
│  lumo-dsl::validate           ai.mode != off ⇒ require llm cap      │
│                                                                      │
│  lumo-core::vm::execute_step                                        │
│    ├─ if step.ai.mode == primary: pre-hook                          │
│    ├─ run deterministic action handler                              │
│    ├─ on failure + mode==fallback: post-hook                        │
│    └─ on final failure + diagnose_on_failure: ai.chat(诊断)         │
│                                                                      │
│  lumo-ai::action::heal_selector / extract_visual / decide           │
│  lumo-ai::router::Budget (AtomicU32, max_calls_per_run)             │
└─────────────────────────────────────────────────────────────────────┘
```

### 4.2 视图架构

| 视图 | 默认 | 渲染器 | 备注 |
|---|---|---|---|
| 步骤 (list) | ✅ 默认 | `renderStepList()` 新增 | 影刀对标主视图 |
| 图 (graph) | 否 | 现有 `renderGraph()` | 保留为 ★ 差异化辅助视图 |
| 树 (tree) | 否 | 现有 `renderTree()` | 保留 |
| 代码 (code) | 否 | 现有 codeEditor | 权威源 |

四视图共享同一份 `state.ast` / `state.source`，任一视图编辑都同步到另外三个。

## 5. 步骤视图设计

### 5.1 步骤行（折叠态）布局

```
┌─ 步骤行 ─────────────────────────────────────────────────────────────┐
│ ⠿  1   📂  open                              [✨] [⋮]                │
│        browser.open · headless                                       │
│        url: {{ inputs.url }}                                         │
└──────────────────────────────────────────────────────────────────────┘
```

- 序号：`1`、`2.1`、`2.2.3`（按嵌套自动计算）
- 图标：按 action 家族着色（browser=蓝 / excel=绿 / control=橙 / ai=紫 / file=灰 / http=青）
- 副标题第一行：`<action> · <关键 modifier>`
- 副标题第二行：取 `with` 头 1-2 个键值，截断到 50 字符
- ✨ 三态颜色：灰（off）/ 绿（fallback）/ 紫（primary）
- 选中态：左侧 4px 强调条（影刀同款）

### 5.2 步骤行（展开态）

```
┌─ 步骤行 ─ 选中 ──────────────────────────────────────────────────────┐
│ ⠿  1   📂  open                              [✨ fallback] [⋮]      │
│        browser.open                                                  │
│ ┌────────────────────────────────────────────────────────────────┐  │
│ │ Step ID    [ open                              ]               │  │
│ │ Action     [ browser.open                      ] 家族: browser │  │
│ │                                                                │  │
│ │ with: 参数（按 Action JSON Schema）                            │  │
│ │   url         [ {{ inputs.url }}                          ]    │  │
│ │   headless    [✓] true                                         │  │
│ │   user_agent  [                                           ]    │  │
│ │                                                                │  │
│ │ [▷ 单独运行]    [📥 写入 YAML]                                 │  │
│ └────────────────────────────────────────────────────────────────┘  │
└──────────────────────────────────────────────────────────────────────┘
```

- 同时只能展开一个步骤
- 表单复用 `renderSchemaFields(schema, step.with)`（现有 inspector 代码）
- 展开层底部按钮和现有 inspector 一致

### 5.3 嵌套（控制流）

```
1   📂  open                browser.open
2   🔀  branch              control.if   cond: {{ inputs.target }}
    └─ do:
       2.1  📝  branch_true     control.log
       2.2  🔀  retry           control.try
            └─ do:    2.2.1  📞  http_call
            └─ catch: 2.2.2  📝  on_error
    └─ else:
       2.3  📝  branch_false    control.log
3   📂  close               browser.close
```

- 嵌套用 CSS `border-left` 虚线 connector，每层 24px 缩进
- 控制流子块（do / else / catch / finally）有自己的 label header
- 子块本身可拖（整块挪位）、子块内步骤也能拖（**仅同层内**重排）

### 5.4 交互细则

| 操作 | UX |
|---|---|
| 拖动重排 | 整行可拖；其他行让位；蓝色 drop indicator 显示插入位置；**P0 仅支持同层级重排** |
| + 插入 | 行间空隙 hover 浮现 `+ 在此处插入`，点击弹动作选择器（搜索 + 家族分组） |
| 拖入 | 左栏动作库拖到画布继续支持，drop 位置 = 最近行间隙 |
| 删除 | `⋮ → 删除` 或选中后 `Delete` |
| 复制 | `⋮ → 复制` 或 `⌘/Ctrl+C` |
| 添加节点快捷键 | `Ctrl+Shift+P`（影刀同款） |
| 选行 | `↑↓` 上下选 / `Enter` 展开折叠 / `Cmd+/` 注释 |
| 空 flow 提示 | `+ 点击添加指令 (Ctrl+Shift+P)，或从左侧动作库拖入` |

### 5.5 YAML 同步

- 步骤视图所有编辑先改内存 AST，再 serialize 回 `state.source`
- Serialize 保序 + 保缩进；不破坏注释
- 三个视图共用同一 AST，互不污染

## 6. AI 增强 Schema

### 6.1 双层结构

```yaml
# 流程级（可选，所有节点的默认值 + 失败诊断开关）
metadata:
  id: ...
  ai:
    enabled: true             # 主开关；关掉后忽略所有节点级 ai 字段
    model: ""                 # 默认模型，留空走 router active；例 "anthropic/claude-opus-4-7"
    diagnose_on_failure: true # 切入点 ④：失败节点 LLM 诊断报告
    budget:
      max_calls_per_run: 100  # 单次运行 LLM 调用上限（含视觉模型）

spec:
  capabilities:
    llm: ["*"]                # 必须声明；UI 切 ✨ 时自动补
  steps:
    - id: title
      action: browser.extract
      with: { selector: "h1" }
      ai:                     # 节点级（可选）
        mode: fallback        # off | fallback | primary
        model: ""             # 覆盖流程级；留空继承
        prompt: ""            # 留空 → VM 按 action+with 自动构造
```

### 6.2 三档状态语义（按切入点）

| 切入点 | `mode: off` | `mode: fallback` | `mode: primary` |
|---|---|---|---|
| ① click/type/wait_element | 仅 CSS/XPath；失败 → 报错 | CSS/XPath 优先；失败 → VLM 视觉重定位 + 写回新指纹 | 跳过确定性，直接 VLM |
| ② extract | CSS/XPath；失败 → 报错 | CSS/XPath 优先；失败 → VLM 看截图 + SoM | 直接 VLM |
| ③ control.if | 仅 `with.cond` Jinja | Jinja 优先；表达式为空 / 报错 → LLM 看 prompt 决策 | 跳过 Jinja，直接 LLM |
| ④ 诊断 | 看流程级 `metadata.ai.diagnose_on_failure`，不看节点 mode | — | — |

### 6.3 字段规范

`metadata.ai`（流程级，全部可选）：

| 字段 | 类型 | 默认 | 说明 |
|---|---|---|---|
| `enabled` | bool | 任一 step 有 `ai:` 时为 `true` | 总开关 |
| `model` | string | `""`（用 router active） | 形如 `openai/gpt-4o` |
| `diagnose_on_failure` | bool | `false` | 任一节点失败时调 LLM 诊断 |
| `budget.max_calls_per_run` | int | `100` | 超额后 AI 通路全停；确定性通路继续 |

`step.ai`（节点级，全部可选）：

| 字段 | 类型 | 默认 | 说明 |
|---|---|---|---|
| `mode` | enum | `off` | `off` / `fallback` / `primary` |
| `model` | string | 继承流程级 | 覆盖流程级默认 |
| `prompt` | string | `""` | 自然语言目标；留空时 VM 按 action+with 自动构造 |

### 6.4 自动构造 prompt 模板（`prompt` 留空时）

| Action | 自动构造模板 |
|---|---|
| `browser.click` | `点击 selector 为 {{ with.selector }} 的元素 + 截图` |
| `browser.type` | `在 selector 为 {{ with.selector }} 的元素中填入 {{ with.text }}` |
| `browser.extract` | `按 selector {{ with.selector }} 从页面截图中抽取文本` |
| `browser.wait_element` | `等待 selector {{ with.selector }} 对应元素出现 + 截图判断` |
| `control.if` | `评估表达式 {{ with.cond }} 的真值` |

UI 在 inspector 里把自动构造的 prompt 以 placeholder 灰色显示；用户改写即覆盖。

### 6.5 能力声明协作

- 节点 `ai.mode != off` 时，flow `spec.capabilities.llm` **必须包含** `"*"` 或具体 model 通配
- UI 切 ✨ → fallback/primary 时
  - 检查 capabilities：缺则**自动补** `llm: ["*"]`，toast 提示"已自动添加 LLM 能力声明"
  - 流程顶部 banner：可手动收紧
- ① 视觉重定位还需 `capabilities.network` 含视觉模型 API 域名

### 6.6 向后兼容

- 现有 8 个 example YAML 无 `ai:` 字段 → `mode = off`，行为完全不变
- `lumo-dsl` validator 给 `ai:` 加 schema 但不强制，缺失视为 off
- 老流程载入后保存**不引入** `ai:` 字段（除非用户主动切 ✨）

## 7. 执行期 hook

### 7.1 vm step 执行三段式

```
                    ┌────────────────────────────────────┐
                    │   lumo-core::vm::execute_step()    │
                    └─────────────────┬──────────────────┘
                                      │
        ┌─────────────────────────────┴─────────────────────────────┐
        │                                                            │
        ▼                                                            ▼
  step.ai.mode = primary                                  step.ai.mode = off/fallback
        │                                                            │
        │  ┌──────────────────┐                          ┌───────────────────────┐
        └─▶│ ai hook (pre)    │                          │ run deterministic     │
           │  - heal_selector │                          │   action handler      │
           │  - extract_vis   │                          └────────┬──────────────┘
           │  - decide        │                                   │
           └────────┬─────────┘                       success ┌───┴───┐ failure
                    │                                  │      │       │
                    ▼                                  ▼      │       ▼
              record OTel                       record &      │   mode=fallback?
              result                            return        │       │
                                                              │   ┌───┴───┐
                                                              │   yes    no
                                                              │   │      │
                                                              │   ▼      ▼
                                                              │  ai hook (post)
                                                              │  - heal_selector
                                                              │  - extract_vis
                                                              │  - decide
                                                              │   │      │
                                                              │   └──┬───┘
                                                              │      │
                                                              │ success / final fail
                                                              │      │
                                                              └──────┴── record & return
                                                                              │
                                                                              ▼
                                                              metadata.ai.diagnose_on_failure?
                                                                              │
                                                                          ┌───┴───┐
                                                                         yes      no
                                                                          │       │
                                                                          ▼       ▼
                                                                  ai.chat(诊断)   raw error
                                                                          │
                                                                          ▼
                                                                    error + 解读
```

### 7.2 helper action 接口（Rust 签名草）

```rust
// 切入点 ①
pub async fn heal_selector(
    ctx: &Ctx,
    screenshot_png: Bytes,
    failed_selector: &str,
    prompt: &str,
    model: Option<&str>,
) -> Result<HealedSelector, StepError>;

pub struct HealedSelector {
    pub css: Option<String>,
    pub xpath: Option<String>,
    pub bbox: Option<(u32, u32, u32, u32)>,
    pub confidence: f32,
}

// 切入点 ②
pub async fn extract_visual(
    ctx: &Ctx,
    screenshot_png: Bytes,
    target_description: &str,
    schema: Option<JsonValue>,    // 期望返回的 JSON shape；P0 不暴露 UI
    model: Option<&str>,
) -> Result<JsonValue, StepError>;

// 切入点 ③
pub async fn decide(
    ctx: &Ctx,
    vars_snapshot: JsonValue,
    prompt: &str,
    model: Option<&str>,
) -> Result<Decision, StepError>;

pub struct Decision {
    pub result: bool,
    pub confidence: f32,
    pub reasoning: String,
}
```

### 7.3 四个切入点链路细化

**① 选择器失效 → 视觉重定位**

```
browser.click(selector) fails with SelectorNotFound
  └─► if step.ai.mode in [fallback, primary]:
         1. browser._screenshot()                   # 内部能力，不计入用户 budget
         2. ai.heal_selector(screenshot, selector, prompt)
         3. if confidence >= 0.6:                   # 默认阈值
              browser._click_by_bbox or browser.click(new_css)
              in-memory 改 step.with.selector
              emit toast "AI 已自愈选择器，可一键写回 YAML"
            else:
              fail with original error + AI 置信度过低
```

**② 抓取失败 → LLM 看截图抽**

```
browser.extract(selector) returns empty / fails
  └─► if step.ai.mode in [fallback, primary]:
         1. browser._screenshot()
         2. ai.extract_visual(screenshot, prompt, schema=None)
         3. result.set("steps.<id>.result", json)
         4. result.set("steps.<id>.ai_source", "visual")
```

**③ IF → LLM 语义决策**

```
control.if eval(with.cond) → None / error / non-bool / cond=""
  └─► if step.ai.mode in [fallback, primary]:
         1. snapshot = { inputs, vars, steps.<recent>.result }
         2. ai.decide(snapshot, prompt)
         3. branch into do / else by decision.result
         4. record decision.reasoning to step output
```

**④ 失败诊断**

```
any step ends with state=failed
  └─► if metadata.ai.diagnose_on_failure:
         1. payload = { action, with, error, recent screenshot if any, vars }
         2. ai.chat(system="你是 RPA 调试助手", user=payload as json)
         3. attach analysis to step.error.diagnostic
         4. UI 在 Time-Travel detail 区显示
```

### 7.4 预算 / 限流

- `Ctx` 挂全局计数器 `RunBudget { calls: AtomicU32 }`
- 每个 helper 入口 `ctx.ai_budget.consume(1)?`；超额返回 `BudgetExceeded`
- 超额后**继续往下跑**（其他确定性步骤不受影响），仅 AI 兜底被禁用
- 运行结束 toast 报告"本次 AI 调用达上限 X 次"

### 7.5 可观测

- 每次 AI helper 调用生成 OTel span：`lumorpa.ai_hook.{insertion_point}`，attrs 含 `model` / `tokens` / `latency_ms` / `confidence`
- `step.outputJson` 挂 `_ai: { used: true, helper, model, latency_ms, confidence, healed_selector? }`
- Time-Travel detail 在 ✨ 用过的步骤上加紫色徽章 "AI heal"

### 7.6 安全 / 隐私

- helper 入口先过 `ctx.ensure_llm(model)`（现有 capability 校验）
- 凭据：`Vault` 占位符不传给 LLM（已有 `resolve_vault_placeholders` 后于 helper 调用）
- 截图：`metadata.ai.redact_screenshots` 字段占位（P0 不实现，留 hook）

## 8. 能力一键授权（独立小特性）

### 8.1 触发

- 任一 step 抛出 `StepError::CapabilityDenied { kind, host }`（kind ∈ network/fs.read/fs.write/llm）
- 现有 toast 改造为带按钮

### 8.2 UX

```
✗ 运行失败 — capability denied: network `wwww.baidu.com` is not declared
[➕ 加入 capabilities.network]   [查看诊断]   [取消]
```

按钮行为：
- `➕ 加入 capabilities.network`：把 `wwww.baidu.com` 追加到 `spec.capabilities.network` 数组；如已是 `["*"]` 则什么都不做
- `查看诊断`：若 `diagnose_on_failure: true` 则展示 LLM 解读，否则展示原 raw error 详情
- `取消`：关 toast

### 8.3 文件级改动

- `lumo-core::error::StepError`：加 `CapabilityDenied { kind: CapKind, target: String }` variant，替换现有 string-based error
- 前端 `app.js::toast`：识别 kind=capability_denied 时渲染按钮
- 前端 `app.js`：新增 `addCapability(kind, value)` 函数，修改内存 AST + 重 serialize source

## 9. 文件级改动清单

### 9.1 前端

| 文件 | 改动 |
|---|---|
| `apps/desktop/frontend/index.html` | 视图切换按钮加 `<button data-mode="list">步骤</button>`（默认 active）；新增 `<div id="view-list">` 容器；流程顶部 AI banner 容器 |
| `apps/desktop/frontend/app.js` | 新增 `renderStepList()` / `bindStepListDnD()` / 行点击展开 / + 插入弹出 / ✨ 抽屉；改造 toast 支持 capability_denied 按钮；自动写回 capabilities 逻辑 |
| `apps/desktop/frontend/styles.css` | 步骤行 / 展开层 / 拖拽 indicator / 嵌套 connector / ✨ 三态色 / AI banner 样式 |
| 复用 | `renderInspector` / `loadSchema` / `renderSchemaFields` / `applyInspectorEdits`（目标容器从右栏改为展开层即可） |

### 9.2 后端

| 文件 | 改动 |
|---|---|
| `crates/lumo-dsl/src/ast.rs` | `Step` 加 `ai: Option<StepAi>`；新增 `StepAi { mode, model, prompt }`；`Metadata` 加 `ai: Option<FlowAi>`；`FlowAi { enabled, model, diagnose_on_failure, budget }` |
| `crates/lumo-dsl/src/validate.rs` | `step.ai.mode != off` 时校验 flow `capabilities.llm` 存在 |
| `crates/lumo-core/src/error.rs` | `StepError` 加 `kind: ErrorKind`，枚举含 `SelectorNotFound` / `ExtractFailed` / `CondError` / `CapabilityDenied` / `Other` |
| `crates/lumo-core/src/vm.rs` | `execute_step` 三段式（pre-hook / deterministic / post-hook）；记录 OTel span `lumorpa.ai_hook`；接 `metadata.ai.diagnose_on_failure` |
| `crates/lumo-core/src/ctx.rs` | 加 `ai_budget: RunBudget`；保持现有 capability 校验签名 |
| `crates/lumo-ai/src/action.rs` | 新增 `heal_selector` / `extract_visual` / `decide`；不注册到 user-facing registry |
| `crates/lumo-ai/src/router.rs` | 加 `Budget` 计数器；超额返回 `Error::BudgetExceeded` |
| `crates/lumo-actions/src/browser.rs` | `click/type/extract/wait_element` 的 `StepError` 带 `kind` |
| `crates/lumo-actions/src/control.rs` | `control.if` cond eval 失败时返回 `kind: CondError` + 上下文变量快照 |

### 9.3 示例

| 文件 | 改动 |
|---|---|
| `examples/browser-scrape.lumoflow.yaml` | 改造为带 `ai:` 字段的演示版；保留原 selector 作主路 |
| `examples/browser-scrape.original.lumoflow.yaml` | 新增，保留改造前版本做对照 |

## 10. 不在 P0 范围（明确划线）

### 10.1 推到后续里程碑

| 功能 | 推到哪 | 原因 |
|---|---|---|
| 多 flow 标签页 | P2 | tab 改造涉及 state.flowPath / 路由 / 多 AST 并存 |
| 元素库 / 图像库（`@elements.foo` 引用） | P1 | 需先把自愈选择器写回链路跑稳 |
| 录制器接入 | M2 | 接 CDP + AccessKit 是大工程 |
| Time-Travel 截图回放 | P2 | 截图 / DOM snapshot 持久化要先建数据流 |
| Magic Prompt 生成节点 / 流程 | P1 | 依赖输入区组件 + prompt 模板库 |
| AI extract JSON schema 强约束（UI 暴露） | P0+ | helper 接口预留 schema 字段，UI 不暴露 |
| 失败诊断的节点级覆盖 | P0+ | P0 只有流程级总开关 |
| 截图打码 / PII redaction | P1 | 留 `metadata.ai.redact_screenshots` 占位 |
| 多模型 fallback chain | P1 | router 已有基础，UI 暴露另立 |
| 流式 AI 进度推送 UI | P1 | P0 用同步 IPC |
| 本地视觉模型（OmniParser/UI-TARS）落地 | M2 | P0 切入点①②用云 VLM |
| MCP 暴露 AI helper | M2 | docs 路线图 |
| 跨节点 AI 上下文记忆 | P1 | P0 只看当前 vars + 最近 step.result |
| AI 成本 dashboard | P1 | P0 仅运行结束 toast 总数 |

### 10.2 明确否决

| 想做但拒绝 | 原因 |
|---|---|
| 拖出 do: 块到外层 | 语义破坏 |
| AI 自动改写整段流程（一键"让 AI 优化"） | 黑盒重写违反"YAML 是权威源" |
| AI 完全替代 Jinja 表达式 | Jinja 快、确定、可调试；primary 模式仅在用户明确选择时启用 |
| 隐藏 ai 字段 | YAML 永远是权威源，UI 改 ✨ 就要写回 ai 字段 |
| 画布视图删除 | docs/01 D-02 列为 ★ 差异化点 |
| 节点级 capabilities 收紧 | 已有 flow 级，P0 不引入 step 级 |

## 11. 验收标准

1. 步骤视图能渲染所有 8 个示例 flow，控制流嵌套层级正确
2. 任一节点行点击展开 → 表单与现有 inspector 等价；改了写回 YAML 后代码视图能看到
3. 拖动节点改顺序、+ 插入新节点、删除节点三个操作 YAML 改动符合预期
4. ✨ 抽屉切到 fallback / primary → ai 字段写入；切回 off → 字段删除（保持 YAML 干净）
5. 改造版 browser-scrape 在 `wwww.baidu.com` URL 下能端到端跑完
6. 现有 `mode: off` 的 8 个示例 flow 跑分数与改造前完全一致（回归零）
7. budget 计数器超额时 AI 通路停、确定性通路继续，运行结束有汇总 toast
8. capability_denied toast 一键扩白名单后 YAML 修改正确且可重跑

## 12. 测试策略

### 12.1 后端单元测试

- `lumo-ai::heal_selector`：mock LLM，三种 case（高置信度成功 / 低置信度返回原错误 / budget 超额）
- `lumo-ai::extract_visual`：mock LLM，正常返回 + 异常 + budget 超额
- `lumo-ai::decide`：mock LLM，true / false / 异常
- `lumo-core::vm`：三段式 hook 的状态机覆盖（primary / fallback success / fallback fail / off）
- `lumo-dsl::validate`：`step.ai.mode != off` 但缺 `llm` capability 应报错

### 12.2 后端集成测试

- 现有 8 个 example flow 在 `mode: off` 下 `cargo test --workspace` 行为完全不变（回归套件）
- 改造版 browser-scrape 模拟 `extract` 失败 → `extract_visual` 兜底成功 → outputs 符合预期
- budget 上限设为 1，跑包含 3 个 ai 节点的 flow → 第 1 个 ai 成功、后 2 个降级

### 12.3 前端

- 步骤视图快照测试：4 个代表性 flow（browser-scrape / control-flow / excel-loop / skill-driver）渲染对比
- 拖拽重排测试：mock dnd events，验证 YAML diff
- ✨ 抽屉交互：切档 → AST.step.ai 变化 → source 重 serialize 后 diff 验证
- capability_denied toast：模拟错误 → 点按钮 → 验证 capabilities 字段修改

### 12.4 E2E（手动）

- 拿真实账号跑改造版 browser-scrape on `wwww.baidu.com`，确认走通 ② / ③ / ④ 切入点
- 拿 `examples/llm-summarize-orders` 跑确认 P0 改动对纯 ai.chat 节点零影响

## 13. 风险与对策

| 风险 | 对策 |
|---|---|
| 步骤视图与画布视图实时同步出错 | 共享 AST，所有视图都是只读 reader（编辑回 AST）；加 AST snapshot diff 测试 |
| YAML serialize 破坏用户注释 | 用 `yaml.composer` 保结构 + 降级 textual mutate；保留 `applyInspectorEdits` 已有 mutate 路径 |
| AI helper 同步等待阻塞 UI | 现有 IPC 已是异步；前端 timeline 行显示"⏳ AI 思考中"占位 |
| budget 计数跨线程同步问题 | `AtomicU32` 即可，单流程单 Run 不存在跨进程 |
| 自愈写回造成 YAML 跳动 | P0 默认 in-memory 改 + toast 让用户主动 [写回]；不静默持久化 |
| LLM 返回非 bool（切入点 ③） | helper 内部强制 schema 校验，失败时报 `Other` 错误进入诊断链路 |
| 老 flow 加 ai 字段后被旧版 LumoRPA 读取报错 | `lumo-dsl::validate` 容错（未识别字段 warn 不 error）；spec 升级 minor |

## 14. 未决问题（写完 spec 后留待 plan 阶段细化）

- ✨ 抽屉里 model 字段的输入控件：dropdown（从 providers.toml 拉列表）还是 free-text？
- 自愈成功后 "写回 YAML" 是普通按钮还是 diff 预览？
- 拖动重排时控制流子块的可视化反馈（蓝条 vs 整块灰底）？
- `metadata.ai.budget` 超额后是否阻塞 step 还是降级 off？（当前设计是降级）
- 失败诊断是否要支持流式输出到 UI？

## 15. 后续动作

- 本 spec 用户复核通过后 → 调 `superpowers:writing-plans` 出实现计划（按 §9 文件级改动清单 + §12 测试策略组织）
- 实现按子项目顺序：(a) StepAi/FlowAi schema + validator → (b) vm hook + 3 helper → (c) 步骤视图 + ✨ 抽屉 → (d) 一键授权 toast → (e) 改造 browser-scrape 示例

---

## 附录 A · 改造版 browser-scrape 完整 YAML

```yaml
apiVersion: lumorpa.io/v1
kind: Flow
metadata:
  id: browser-scrape
  version: 0.1.0
  name: 浏览器抓取示例
  description: 打开任意网站，智能提取首屏标题；选择器失效时视觉重定位。
  tags: [example, browser, ai]
  ai:
    enabled: true
    diagnose_on_failure: true
    budget: { max_calls_per_run: 20 }

spec:
  inputs:
    - { name: url, type: string, default: "https://example.com" }
  capabilities:
    network: ["*"]
    llm: ["*"]
  steps:
    - id: open
      action: browser.open
      with: { url: "{{ inputs.url }}", headless: true }

    - id: title
      action: browser.extract
      with: { selector: "h1" }
      ai:
        mode: fallback

    - id: is_chinese_site
      action: control.if
      with: { cond: "" }
      ai:
        mode: primary
        prompt: |
          抓到的标题是 "{{ steps.title.result }}"。这是不是一个中文网站？
      do:
        - id: zh_log
          action: control.log
          with: { message: "中文站点：{{ steps.title.result }}" }
      else:
        - id: en_log
          action: control.log
          with: { message: "Non-Chinese: {{ steps.title.result }}" }

    - id: close
      action: browser.close
      with: {}
```
