# 03 · LumoRPA 关键子系统详设

> 总报告：[00](./00-LumoRPA-Master-Report.md) · 产品：[01](./01-Product-Design.md) · 架构：[02](./02-Architecture-Design.md)

本文展开六个最具差异化或最易做错的子系统：
1. 录制器（Recorder）
2. 选择器引擎与 Self-Healing Router
3. AI 视觉与 Computer Use 子系统
4. Excel 驱动循环（影刀招牌场景）
5. 调度器、触发器与队列
6. MCP 双向打通

---

## 1. 录制器（Recorder）

### 1.1 目标
- 一次录制，回放在任何环境都稳。
- 浏览器/桌面统一 step DSL，输出即可 git diff 的 YAML。
- 录制时同步产出 4 套元素指纹 + 截图/DOM 快照，为 Self-Healing 提供基础。

### 1.2 架构

```
Browser source                Desktop source                Mobile source
┌────────────┐                ┌────────────┐                ┌────────────┐
│ Chromium    │   CDP events  │ OS hook     │  raw events    │ scrcpy/adb │
│ + content   ├───────────────► svc (Rust)  ├───────────────► (later)     │
│   shim      │   DOM mutation│ + AccessKit │  AX snapshot   │            │
└──────┬─────┘   selectors    └──────┬─────┘  bbox + OCR     └────────────┘
       │                              │
       └──────────────┬───────────────┘
                      ▼
            ┌────────────────────────┐
            │  Normalizer            │  → 统一事件 schema
            └──────────┬─────────────┘
                       ▼
            ┌────────────────────────┐
            │  ActionBuffer (200ms)  │  → debounce/merge
            │  rules:                │      • 连续 type → 单 type
            │   • drop hover noise   │      • 同元素 ≤300ms 双击合并
            │   • merge type/click   │      • 滚动合并
            └──────────┬─────────────┘
                       ▼
            ┌────────────────────────┐
            │  Element Fingerprinter │  → CSS/XPath/A11y/Visual/OCR
            └──────────┬─────────────┘
                       ▼
            ┌────────────────────────┐
            │  DSL Emitter           │  → flow.yaml step
            └──────────┬─────────────┘
                       ▼
                LumoStudio Recorder Panel
```

### 1.3 关键算法

**最稳 selector 评分**（受 Playwright codegen 启发）：

```
score(sel) = w1*specificity + w2*uniqueness + w3*locality + w4*history
            – penalty(generated|index-based|dynamic-id)
```

- **specificity**: ARIA `role+name` > `data-testid` > 显式 `id` > 唯一 text > class 组合 > nth-child。
- **uniqueness**: 同页 hit 数 = 1 为最高。
- **locality**: 锚点元素附近相对路径优于全局路径（影刀同款思路）。
- **history**: 多次成功的 selector 加权。
- **penalty**: 含 `nth-child`、含 `:contains`、含运行时生成 id（`__r_123`）。

**录制后处理**：
- 自动识别"循环模式"：连续 3 次同形态 + 同 selector 模板，提示用户合并为 `for_each`。
- 自动识别"表单填写"：连续 type+tab 序列合并为 `form.fill_by_map`。

### 1.4 智能录制（混合模式）
- Web 与桌面源同时启用。
- 拾取冲突解决：当一次点击同时在 Chromium 和 OS hook 中收到，优先 Web（因 DOM 选择器更稳）。
- 跨应用切换（如：从 Excel 切到 Chrome 复制粘贴）自动产生 `app.switch_to` 节点。

### 1.5 录制即文档
- 每个 Step 自动生成中文一句话注释（调本地小 LLM 或离线模板：`点击「{role+name}」 按钮`）。
- 录制完一键导出 SOP Markdown（Sema4.ai 风格），可作为知识库种子。

---

## 2. 选择器引擎与 Self-Healing Router

### 2.1 4 策略 fallback

| 策略 | 适用 | 速度 | 健壮性 |
|---|---|---|---|
| CSS / XPath | DOM 可靠场景 | 极快 | 中（DOM 变化即坏） |
| A11y path（UIA/AX/AT-SPI） | 桌面 + 现代 Web | 快 | 高 |
| Visual anchor + OCR | 几何稳定的 UI | 慢 | 中（分辨率敏感） |
| Vision-LLM grounding（OmniParser/UI-TARS/CU） | 任何 | 慢 + 有 token 成本 | 极高 |

### 2.2 Self-Healing Router 数据结构

```rust
struct StrategyNode { id: StrategyId, last_latency: f32, fail_prob: f32 }
struct StrategyGraph {
    nodes: Vec<StrategyNode>,           // CSS, XPath, A11y, Visual, OCR, VLM
    edges: HashMap<(StrategyId, StrategyId), f32>, // 切换代价
}

fn best_path(graph: &StrategyGraph, target: &ElementSpec) -> Vec<StrategyId> {
    // Dijkstra
    // weight = expected_latency * (1 + fail_prob * recovery_cost)
}
```

### 2.3 Healing 流程

```
attempt(strategy=CSS) → ok? → done
    └─ fail ─→ mark css.fail_prob += 0.1
              ↓
        next = router.next_best()           // 不是固定顺序，按运行时打分
        attempt(strategy=A11y) → ok? → done + heal-suggest("A11y > CSS, switch?")
              ↓
        attempt(strategy=Visual) ...
              ↓
        attempt(strategy=Vision-LLM)
              ↓
        all fail → raise SelectorError(reason+screenshot+candidate-suggestions)
```

### 2.4 健康分（Selector Health Score）

- 每个元素维护 `success_count / total_attempts` 与漂移率（CSS hit 数变化趋势）。
- 当 health < 0.6 时 Studio 红色徽章 + Code Lens 建议替换。
- 当 Vision-LLM 重锚定成功，可一键把新的 CSS/A11y 指纹**写回元素库**，下次免 LLM。

---

## 3. AI 视觉与 Computer Use 子系统

### 3.1 节点种类

| 节点 | 入参 | 出参 | 默认 backend |
|---|---|---|---|
| `vision.describe_screen` | screenshot | text | UI-TARS-1.5 (local) |
| `vision.locate` | screenshot + query | bbox + confidence | OmniParser v2 |
| `vision.click_by_query` | query（自然语言） | side-effect | OmniParser + 鼠标 |
| `vision.ocr` | screenshot/PDF/image | text + bbox | PaddleOCR 3.0 |
| `vision.structure_extract` | document image | JSON | PP-StructureV3 |
| `agent.computer_use` | goal + tools | trace + result | Claude `computer-use-2025-11-24` |
| `agent.browser_use` | goal + start_url | trace + result | Browser-Use 风格内嵌实现 |
| `agent.planner_actor_validator` | goal | result + score | 三相 Agent |

### 3.2 Computer Use 节点工作流

```
                       ┌─────────────────────┐
   user goal           │ Planner             │  (Claude / GPT-5 / GLM-4.6)
   + capabilities  ──▶ │  break into steps    │ outputs sub-goal list
                       └──────────┬──────────┘
                                  ▼
                       ┌─────────────────────┐
                       │ Actor (loop)         │  (Computer Use model)
                       │  see → think → act   │
                       │  via OS canvas tool  │
                       └──────────┬──────────┘
                                  ▼
                       ┌─────────────────────┐
                       │ Validator            │  (any vision LLM)
                       │  verify goal reached │ confidence + reason
                       └──────────┬──────────┘
                                  ▼
                  ok? → done    no? → return to Planner with feedback
```

每相全部走 OTel span，prompt/screenshot/decision 全留痕。

### 3.3 本地模型部署

- **OmniParser v2** 走 ONNX Runtime（CPU 1–2s/帧，GPU < 0.6s/帧），Rust 直集成 `ort` crate，无 Python 依赖。
- **UI-TARS-1.5-7B** 走 vLLM 或 Ollama，HTTP 端口本地。
- **PaddleOCR 3.0** 走 Paddle Inference C++（Rust FFI）或 paddleocr-python（PyO3 内嵌）。
- 用户首次启用时弹窗：选"云端默认"或"下载本地模型"（含磁盘/显存自检）。

### 3.4 视觉 Prompt 兜底：Set-of-Mark

当 OmniParser/UI-TARS 都失败时，自动启用 SoM：
1. 用 SAM/SEEM 分割当前截图。
2. 每个区域打数字标号。
3. 把"标注后截图 + 数字"喂给任意 VLM（Claude/GPT/Gemini）：「请说出包含『提交订单』按钮的编号」。
4. 返回编号 → 区域中心 → 模拟点击。

这样即使没有专门 grounding 模型，任何 VLM 都能定位 UI。

---

## 4. Excel 驱动循环（核心场景，影刀招牌）

### 4.1 用户语义

「我有 1000 行 Excel，每行触发一次完整子流程：登录、查询、抓取、写回。」

### 4.2 DSL 范式

```yaml
- id: foreach-row
  action: excel.for_each_row
  with:
    file: "${{ inputs.excel_path }}"
    sheet: "Sheet1"
    header: true
    filter:
      where: "status == 'pending'"        # 类 SQL where, polars expr 编译
    batch: 50                              # 每批 50 行并行
    parallelism: 4                         # 同时跑 4 个 worker
    on_error: continue                     # skip/continue/halt
  do:
    - { action: browser.open, with: { url: "{{ row.url }}" } }
    - { action: browser.type, with: { selector: "#kw", text: "{{ row.keyword }}" } }
    - { action: browser.extract, with: { map: { price: ".price", ts: ".time" } } }
    - { action: excel.write_back, with: { row_index: "{{ row._index }}", values: { result: "{{ extract.result }}" } }}
```

### 4.3 实现要点

- **后端引擎切换**：
  - `polars`（Rust 原生）做查询/过滤/聚合，零拷贝。
  - `openpyxl`（PyO3）做"按单元格回写"场景（保留公式/样式/格式）。
  - `xlsxwriter`/`fastexcel` 用于只写场景。
  - 自动选：若不需要保留样式 → Polars；需要 → openpyxl。

- **进度回写**：每行处理后立即 commit 到 step_run；中途崩溃可断点续跑（每行 `where status='pending'` 自然幂等）。

- **结果再循环**：`extract.result` 自动成为下一节点的 row context（影刀同款"循环 Excel 内容"语义）。

- **类型安全**：Schema 推断时区分 int/float/date/string，避免影刀"整数变浮点"坑。

- **大文件**：>50 万行自动切换流式（Polars LazyFrame + chunked write）。

### 4.4 反例：避免影刀踩过的坑

| 影刀坑 | LumoRPA 对策 |
|---|---|
| "写入行/列/区域"语义混乱 | 三件套 API 分离：`excel.write_row` / `excel.write_column` / `excel.write_range`，参数明确 |
| 日期被自动识别成字符串 | 显式 `as: date`/`as: string` 字段声明 |
| 大批量循环写慢 | 默认 batch 提交（每 200 行一次 save） |
| 公式被覆盖 | 默认 `preserve_formulas: true` |

---

## 5. 调度器、触发器与队列

### 5.1 单机模式

- **scheduler**：`tokio-cron-scheduler` + libSQL store。
- **queue**：自研 `sqlite-mq`（参考 Litequeue），WAL + busy_timeout，提供 `enqueue/lease/ack/nack` 四原语。
- **trigger manager**：
  - cron：标准 cron 表达式（含 6 位秒级）。
  - 文件：notify-rs 监听 + glob。
  - 邮件：IMAP IDLE 长连接。
  - webhook：axum HTTP server，HMAC + IP allowlist。
  - 热键：rdev/全局热键 hook。
  - 链式：上游 `flow_run.state=ok` 事件订阅。
  - MCP：MCP server 收到 tool 调用，转 dispatch。

### 5.2 集群模式（LumoCloud）

- queue：NATS JetStream，subject 按 `flows.{flow_id}.tasks`。
- worker 注册：gRPC bidi stream，长心跳；orchestrator 按"剩余 capacity + 标签匹配"分发。
- 死信队列：连续 N 次 nack 后落入 `dlq.{flow_id}`，Studio 可重放。
- 优先级：JetStream 多 stream + consumer priority 组合。
- SLA：每流程可声明 `max_running_seconds`，超时强杀。

### 5.3 多机器人并发不抢焦点

- 每个 worker 绑定一个独立 OS user session：
  - Windows：`CreateProcessAsUser` + 独立桌面会话（`WTS_CURRENT_SESSION` 隔离）+ Job Object。
  - Linux：systemd-run --user → 独立 xpra/Xvfb display；headless 浏览器默认。
  - macOS：单机推荐 headless 模式（多 user session 受系统限制）。
- 这是 LumoRPA 相对影刀**最大的工程胜场**：影刀至今仍受 "鼠标焦点抢占" 投诉。

### 5.4 工作队列前端

Studio 内提供"队列面板"：
- 实时可视化每个 worker 的负载、当前任务。
- 拖拽改优先级 / 重排。
- 重放 / 取消 / 暂停队列。

---

## 6. MCP 双向打通

### 6.1 LumoRPA as MCP Server

- 所有 Action（含用户自定义）自动暴露为 MCP tool。
- 暴露粒度：
  - **粗粒度**：`lumorpa.run_flow`（参数 = inputs，返回 = outputs）。
  - **细粒度**：`lumorpa.action.browser.click` 等（仅在 capability 允许的情况下）。
- 暴露范围 + 权限：MCP allowlist（哪些 tool / 哪些 flow / 哪些 capability 对哪个 client）。
- 协议：JSON-RPC 2.0 over stdio（Claude Desktop / Cursor）+ SSE/HTTP（远程）。
- 凭据：MCP client 看不到 vault 项，仅看到占位符；执行时 LumoRPA 内部 JIT 注入。

例：Claude Desktop 配置：

```jsonc
{
  "mcpServers": {
    "lumorpa": {
      "command": "lumo",
      "args": ["mcp", "serve", "--allow=lumorpa.run_flow,lumorpa.action.browser.*"]
    }
  }
}
```

### 6.2 LumoRPA as MCP Client

- 内置 MCP client；Flow DSL 可声明依赖的外部 MCP server。
- 外部 MCP tool 通过 `mcp.call` Action 调用：

```yaml
- action: mcp.call
  with:
    server: "@modelcontextprotocol/servers/filesystem"
    tool: "read_file"
    args: { path: "${HOME}/notes.md" }
```

- 网关化：所有外部 MCP 调用统一走 Studio 的 MCP Gateway（默认 deny + 用户审批 + 审计），保护用户避免恶意 MCP server。

### 6.3 MCP + Agent 联动样例

```
用户对 Claude Desktop 说：
  "把昨天淘宝订单的发货状态整理到我桌面的 daily.xlsx"
↓ Claude 看到 LumoRPA 暴露的 tools，挑选并调用
   lumorpa.run_flow(name="order-export",
                    inputs={ date: "yesterday", out: "${HOME}/Desktop/daily.xlsx" })
↓ LumoRPA 启动 Worker，跑完返回 outputs.exported_path
↓ Claude 拿到结果继续与用户对话
```

---

## 7. 子系统交互全景图（一图流）

```
┌────────────────────────────────────────────────────────────────────────────┐
│  Trigger        Flow VM           Action            Cap Driver              │
│   ↓              ↓                 ↓                 ↓                       │
│ cron/file ──▶ schedule run ──▶ resolve step ──▶ browser/desktop/excel ──▶   │
│                                                                           │ │
│                          ↑                ↑                                │ │
│                          │                │                                │ │
│                       Selector ◀── Self-Healing Router ◀── AI/Vision/CU ◀─┘ │
│                                                                              │
│  Vault ◀── JIT inject ──┐                                                    │
│                          ▼                                                    │
│                    ┌──────────┐                                              │
│                    │ Sandbox  │  ──▶ logs + screenshots + DOM ──▶ libSQL    │
│                    └──────────┘                                  + S3       │
│                                                                              │
│                                                            ↓                 │
│                                                       OTel spans            │
│                                                            ↓                 │
│                                                  Trace Viewer / Time-travel │
└────────────────────────────────────────────────────────────────────────────┘
```

---

## 8. 设计开放问题（待 PoC 验证）

1. **OmniParser v2 + UI-TARS-1.5 双轨**在中端 GPU（如 RTX 3060 / 4060 Laptop）下的延时是否满足生产；不行则降级 4B/3B 量化版。
2. **libSQL 嵌入式 + 向量列**在跨 platform Tauri 包内分发的体积与启动时延。
3. **Tauri 2 Channel** 大量截图/二进制传输是否需要切 `tauri::ipc::Channel` 流模式或独立 IPC pipe。
4. **CDP 直连 vs Playwright 后端**：放弃 Playwright 是否过早？建议第一版 Chromium 走 chromiumoxide，Firefox/WebKit 临时桥 Playwright。
5. **Self-Healing Router 边权** 的冷启动数据从哪来：可先用一组开源基准（WebVoyager / OSWorld 失败案例）做先验权重。
6. **凭据 JIT 注入** 与 Time-travel 回放的矛盾：回放时不能重放真实凭据，需保留占位符 + 模拟值。
7. **MCP allowlist UX**：当 Claude 调用风险 tool（删除/转账）时，如何兼顾顺畅与安全。

---

## 9. 总结：为什么 LumoRPA 能"对标且超越"影刀

| 影刀的硬伤 | LumoRPA 设计上的根本对策 |
|---|---|
| 二进制工程，不可 Git diff | 流程即 YAML，原生 Git |
| 没有流程图视图、不能片段测试 | 三向同构编辑器 + step-level Run |
| 鼠标焦点抢占、多机器人冲突 | 独立 OS user session + headless 默认 |
| Web 强依赖 Chrome 扩展 | CDP 直连 chromiumoxide |
| 控制台 SaaS 锁定 | Self-host 控制台开箱即用 |
| AI 充值起步 100 元、单一云端 | BYOK + 本地视觉模型 |
| 选择器一旦坏就靠人 | Self-Healing Router + Vision-LLM 自愈 |
| 调试只能截图 + 重跑 | Time-Travel Debugger + OTel |
| 30 行分享限制 | Apache-2.0 完全开源 |
| 仅作为 MCP server | MCP 双向，agent 时代一等公民 |

> 一句话：**影刀把 RPA 做成了"易用的桌面录制软件"；LumoRPA 把 RPA 做成"一个可被 Git/CI/LLM/MCP 全栈接入的开放执行平台"。**

---

> 文档完。
>
> 报告四件套：
> - [00 · 总报告](./00-LumoRPA-Master-Report.md)
> - [01 · 产品功能](./01-Product-Design.md)
> - [02 · 系统架构](./02-Architecture-Design.md)
> - [03 · 子系统详设](./03-Subsystems-Deep-Dive.md)
