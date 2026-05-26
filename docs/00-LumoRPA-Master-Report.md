# LumoRPA 产品与架构设计总报告

> 版本：v0.1（首版设计稿）
> 日期：2026-05-25
> 代号：**LumoRPA** — “一束能照进所有屏幕的光”
> 调研基线：影刀 RPA、UiPath、Power Automate、Blue Prism、来也、实在、Manus、AutoGLM、Skyvern、Browser-Use、n8n、Activepieces、Windmill、Anthropic Claude Computer Use、OpenAI Operator、Microsoft Fara1.5、UI-TARS、OmniParser v2、PaddleOCR 3.0 等 60+ 产品与开源项目。

本报告由四个文档组成，本文是总览：

| 编号 | 文档 | 作用 |
|---|---|---|
| 00 | 本文 | 愿景 · 竞品结论 · 总体设计哲学 · 模块地图 · 路线图 |
| 01 | [产品功能设计](./01-Product-Design.md) | 完整功能矩阵 · 用户旅程 · 关键体验设计 · UI 草案 |
| 02 | [系统架构设计](./02-Architecture-Design.md) | 4 层架构 · 技术栈选型 · 数据模型 · 跨进程协议 |
| 03 | [关键子系统详设](./03-Subsystems-Deep-Dive.md) | 录制器 · 选择器 · AI 视觉 · Excel 循环 · 调度 · MCP |

---

## 1. 一句话定位

> **LumoRPA = “开源 + AI 原生 + 流程即代码”的下一代 RPA 平台**
> 用 **Rust 执行引擎 + TypeScript/Tauri 设计器 + Python 脚本扩展 + SQLite/libSQL 存储**，提供从录制、可视化编排、AI 自愈、Excel 循环、桌面/Web/手机自动化，到本地/云端调度、MCP 双向打通的全栈能力。
>
> 对标影刀 RPA 的易用性与"近千条指令"工程沉淀；
> 在**流程即代码、Vision-LLM 自愈、Computer Use、多机器人并行、可观测性、跨平台、开源生态**七个维度做出超越。

---

## 2. 设计哲学（七条）

1. **流程即代码（Flow as Code）** — 任何流程都序列化为可 `git diff` 的 YAML/JSON，弥补影刀"二进制工程不可代码评审"的硬伤。
2. **三向同构编辑器** — 流程图视图 ⇄ 节点表单视图 ⇄ 纯代码视图，三者实时双向同步，开发者/业务人员各取所需。
3. **确定性优先、AI 兜底（Deterministic-first, AI-fallback）** — 录到的脚本第一次跑确定性 selector；只有 DOM/UI 漂移、selector 失效时才回退到 Vision-LLM 重锚定。借鉴 Stagehand 三原语 `act / extract / observe`。
4. **零信任沙箱** — RPA worker 在独立 OS user + 平台原生沙箱中运行；凭据 just-in-time 注入，LLM 永远看不到原始密码。
5. **MCP 一等公民** — 每条 Action 自动暴露为 MCP tool，让 Claude/Cursor/Cline/外部 Agent 双向调度 LumoRPA 流程，同时 LumoRPA 也可以反向调用 MCP server。
6. **本地优先、云端可选** — 单机零依赖跑起来；需要 7×24 多机器人、企业控制台时无痛升级到 self-hosted 集群。
7. **可观测全开放** — OpenTelemetry GenAI Semantic Conventions 落地到每一个 step，自带 time-travel debugger 与截图回放。

---

## 3. 竞品对标结论：影刀有什么、LumoRPA 要超越什么

### 3.1 必须对齐的"基础设施"（影刀已有）

| 能力 | 影刀做法 | LumoRPA 对齐方式 |
|---|---|---|
| 可视化设计器 + 指令拖拽 | 单一节点表单视图 | 三向同构编辑器（流程图/节点/代码） |
| 元素拾取（F2/F6/F9/F10）+ 智能拾取 + 相似元素 + 锚点 + 关联元素 | UIA + Chrome 扩展 + 图像 | CDP + AccessKit + OmniParser/UI-TARS 多策略 fallback |
| 网页 / 桌面 / Office / 文件 / 数据库 / Email / FTP / OCR 命令族 | 近千条官方指令 | 内置 ~400 Action（首发）+ 插件市场扩张到千 |
| Excel 循环、数据表筛选、批量填充 | openpyxl + 自有 DataTable | Polars + openpyxl 双引擎，DataTable 等价 LazyFrame |
| 录制器（Web/桌面/智能录制） | 智能切换拾取方式 | CDP 事件录制 + AccessKit 桌面录制，同一 step DSL |
| 异常处理 + 重试 + 调试 + 变量面板 + 截图日志 | Try/Catch/Finally | 同 + OpenTelemetry span 化 + Time-travel 回放 |
| 触发器（时间/文件/邮件/Webhook/热键/链式） | 控制台高级任务 | 单机内嵌调度（tokio-cron-scheduler）+ 集群版 NATS JetStream |
| 工作队列、多机器人并发抢占 | 控制台队列 | SQLite-as-MQ（本地）+ NATS（集群） |
| 控制台编排、版本、审计 | 影刀控制台 SaaS | 自托管 Web 控制台 + Git 化版本 |
| 移动端（Android）自动化 | 影刀手机助手 | Appium + UiAutomator2 + scrcpy；可选云手机 |
| AI 节点（魔法指令 / 大模型节点 / 文档抽取） | 影刀 Copilot 3.0 | 多模型路由（Claude/GPT/Gemini/Qwen/GLM/DeepSeek）+ 本地 OmniParser v2 + UI-TARS-1.5 + PaddleOCR 3.0 |
| 应用市场 + 加密分享 | 影刀 App Store | 插件/子流程/应用三层市场，OCI 制品化 |
| 信创适配（UOS / 麒麟 / 达梦） | 已支持 | Rust + Tauri 跨平台天然友好；构建矩阵覆盖 |

### 3.2 LumoRPA 的差异化（影刀短板/未做/做不好的）

| # | 维度 | 影刀短板 | LumoRPA 突破 |
|---|---|---|---|
| Δ1 | **流程图视图缺失 + 不能片段测试** | 只能从某行开始跑 | 三向同构编辑器 + 任意子流程/节点级单步 Run + Inngest 风格 step durability |
| Δ2 | **二进制工程，不可 Git diff** | 流程是私有格式 | 100% 文本 YAML/JSON，原生 Git 工作流 + Lock 文件锁定依赖 |
| Δ3 | **运行抢占用户焦点** | 鼠标键盘独占 | Headless Chromium + Xvfb/虚拟显示 + 独立 OS user 沙箱，**真正后台运行** |
| Δ4 | **多机器人焦点抢占** | 同机并行冲突 | 会话隔离 + UI 锁 + 每 robot 独立 user session（Windows 多会话 / Linux ttysrv） |
| Δ5 | **Web 强依赖 Chrome 扩展** | 扩展掉线即崩 | CDP 直采（chromiumoxide），扩展可选；Patchright/CloakBrowser 级 stealth |
| Δ6 | **AI 充值起点 100 元 + 单一云端** | 锁定影刀云 | 多模型路由 + 自带本地视觉模型 + BYOK；离线可跑 |
| Δ7 | **不支持 MCP 客户端** | 仅作为 MCP server | **MCP 双向**：既被外部 Agent 调度，也调度外部 MCP 工具 |
| Δ8 | **桌面深度仅 Windows 强** | Mac/Linux 弱 | AccessKit 抽象，三平台同优先级 |
| Δ9 | **没有原生 Computer Use 节点** | 仅"AI 操作屏幕节点" | 一等公民 Computer Use Node：Claude/Gemini CU、UI-TARS-2、Fara1.5、AutoGLM 可切换 |
| Δ10 | **可观测性闭源** | 日志在影刀云 | OpenTelemetry GenAI 标准 span + Trace 时间线 + 截图回放，可自接 Grafana/Langfuse |
| Δ11 | **30 行分享限制** | 社区版限流 | 开源 Apache-2.0，无功能阉割 |
| Δ12 | **控制台 SaaS 锁定** | 私有化要额外收费 | 控制台默认自托管，Docker Compose 一行起 |

### 3.3 创新拓展（影刀没有，主流 RPA 也少见）

| # | 创新点 | 灵感来源 |
|---|---|---|
| ✦1 | **Planner / Actor / Validator 三相 Agent** | Skyvern；每个 AI 节点都跑三相闭环，输出可信度评分 |
| ✦2 | **自然语言 Runbook 模式** | Sema4.ai；写一段 Markdown SOP 即生成 Agent |
| ✦3 | **Self-Healing Router** | 2025 学术论文；selector 失败时按图路由切策略，零 LLM 成本 |
| ✦4 | **Time-Travel Debugger** | OTel + 截图快照；任意步骤回放，重现 Bug |
| ✦5 | **流程发布为 API + MCP Server + 定时 Trigger** | n8n / Activepieces；一次定义、多入口暴露 |
| ✦6 | **Excel-Driven Dataflow** | 影刀 + Pandas/Polars；Excel 一行 = 一次任务循环，Polars 流式背压 |
| ✦7 | **云端虚拟桌面异步执行** | AutoGLM 2.0 / Copilot Studio Windows 365；可选 Worker 跑在虚拟桌面，浏览器有"AI 抢屏"解决方案 |
| ✦8 | **流程演化 RL（可选高级）** | AutoGLM ComputerRL；失败回放 → 训练样本 → 模型微调，闭环自进化 |
| ✦9 | **凭据 Just-In-Time 注入** | Browserbase + 1Password；LLM 永远看不到原始凭据 |
| ✦10 | **Set-of-Mark 视觉 Prompt 兜底** | Microsoft SoM；当 grounding 模型失败时，截图打数字标号给任意 VLM |

---

## 4. 总体模块地图

```
┌─────────────────────────────────────────────────────────────────────────┐
│                         LumoRPA Studio (Tauri 2 + React + TS)           │
│  · 三向同构编辑器  · 节点面板  · 录制器面板  · Trace 回放  · 调试器       │
└──────────────┬──────────────────────────────────────────────────────────┘
               │ Tauri IPC / gRPC
┌──────────────▼──────────────────────────────────────────────────────────┐
│                    lumo-core (Rust)  — 单进程多线程内核                  │
│ ┌──────────┐ ┌──────────┐ ┌──────────┐ ┌──────────┐ ┌──────────┐         │
│ │ Flow VM  │ │ Action   │ │ Selector │ │ Recorder │ │ AI Router│         │
│ │  (DSL)   │ │ Registry │ │  Engine  │ │  Engine  │ │  (LLM/CV)│         │
│ └────┬─────┘ └────┬─────┘ └────┬─────┘ └────┬─────┘ └────┬─────┘         │
│      │            │            │             │            │              │
│ ┌────▼────────────▼────────────▼─────────────▼────────────▼──────┐       │
│ │  Capability Layer:  Browser(CDP) · Desktop(AccessKit) ·         │       │
│ │  Office(Polars+openpyxl) · OCR(PaddleOCR) · HTTP · DB · Files · │       │
│ │  Mobile(Appium/U2)                                              │       │
│ └─────────────────────────────────────────────────────────────────┘       │
│ ┌─────────────────────────────────────────────────────────────────┐       │
│ │  Storage:  libSQL (WAL+Vector)  ·  Object Store (artifacts)      │      │
│ │  Scheduler: tokio-cron-scheduler  · Queue: sqlite-mq / NATS      │      │
│ │  Observability: OpenTelemetry GenAI semconv                     │      │
│ └─────────────────────────────────────────────────────────────────┘       │
└──────────────┬──────────────────────────────────────────────────────────┘
               │ PyO3
┌──────────────▼──────────────────────────────────────────────────────────┐
│           lumo-py (内嵌 Python 3.12) — 代码节点 / 扩展插件               │
│   · openpyxl/pandas/polars  · pywinauto 兼容层  · 用户脚本               │
└──────────────┬──────────────────────────────────────────────────────────┘
               │ MCP / gRPC
┌──────────────▼──────────────────────────────────────────────────────────┐
│      lumo-cloud (Optional, self-host)  控制台 + 多机器人编排              │
│   · Web UI (Next.js)  · Orchestrator  · Worker Pool  · Audit  · Vault   │
└─────────────────────────────────────────────────────────────────────────┘
```

---

## 5. 技术栈选型一览（详见架构设计文档）

| 层 | 选型 | 关键理由 |
|---|---|---|
| 桌面 Shell | **Tauri 2.9** | 3 MB 安装包，Rust + WebView，原生 IPC channel |
| 设计器 UI | **React 19 + TypeScript + ReactFlow + Zustand + shadcn/ui** | 节点编辑器生态成熟 |
| 执行内核 | **Rust 1.83+, tokio, chromiumoxide, accesskit** | 性能 + 安全；CDP 直连 |
| 选择器 | CSS / XPath / Accessibility / Vision-LLM 四路 fallback + Set-of-Mark | Stagehand 三原语 + 影刀相似元素 |
| AI 视觉 | **OmniParser v2 + UI-TARS-1.5/7B**（本地）+ Claude Opus 4.7 CU / Gemini 2.5 CU / Fara1.5（云） | 离线可用 + 云端最强 |
| OCR | **PaddleOCR 3.0**（PP-OCRv5 + PP-StructureV3） | 中文/表格/手写最强，自带 MCP |
| DSL/编排 | 自研 **LumoFlow DSL（YAML/JSON）**，借鉴 Inngest step durability + Stagehand 三原语 | 文本化、可 diff、可重放 |
| 脚本扩展 | **PyO3 + maturin** 内嵌 Python 3.12 | 复用 RPA 庞大 Python 生态 |
| 存储 | **libSQL**（SQLite 兼容 + 原生 `F32_BLOB` 向量列 + Embedded Replica） | 单文件 + 向量 + 同步 |
| 队列 | 单机 **SQLite-as-MQ**；集群 **NATS JetStream** | 零依赖起步，平滑升级 |
| 调度 | **tokio-cron-scheduler 0.15** | Rust 原生，多后端 |
| 沙箱 | Linux: **Firejail** / macOS: **sandbox-exec** / Windows: **独立 OS user + Job Object** | 平台原生，default-deny |
| 可观测 | **OpenTelemetry GenAI Semantic Conventions + OpenLLMetry** | 行业新标准 |
| MCP | 自研 MCP Server + 客户端 | 双向打通 Claude/Cursor/Cline 等 |
| 控制台 | **Next.js 15 + tRPC + libSQL/PostgreSQL** | 全栈 TS，自托管 |
| 包格式 | 流程：**`.lumoflow`（YAML in zip）**；插件：**OCI artifact** | 工业标准制品 |

---

## 6. 路线图（12 个月，4 个里程碑）

### M1 · MVP（0-3 月）：Hello LumoRPA
- Rust 内核（Flow VM + Action Registry 50 个 Action）
- Tauri Studio 三向编辑器最小版（节点视图 + 代码视图）
- CDP 浏览器自动化（点击/输入/抓取/上传/iframe）
- Excel 读写循环 + Polars DataTable
- 桌面自动化（Windows UIA via AccessKit）
- 简易录制器（CDP 事件 + DOM 快照）
- libSQL 持久化 + 单机调度 + cron Trigger
- 单元/集成测试 + GitHub Actions 构建矩阵（Win/Mac/Linux x64 + Win/Linux arm64）
- 文档站 + 5 个示例流程（电商比价/Excel 批量发邮件/PDF 抽取/网页填表/桌面登录）

### M2 · AI Native（3-6 月）
- AI Router：Claude/GPT/Gemini/Qwen/GLM/DeepSeek 多 provider，BYOK + Token 计费
- 本地 AI：OmniParser v2 + UI-TARS-1.5/7B + PaddleOCR 3.0 三件套
- Vision-LLM 自愈选择器（Stagehand `act/extract/observe`）
- Computer Use Node：Claude `computer-use-2025-11-24` + Gemini 2.5 CU 适配器
- 魔法指令 v1：自然语言生成节点 / 子流程
- Self-Healing Router（图路由零 LLM 重试）
- MCP Server：流程/Action 暴露为 MCP 工具

### M3 · Enterprise（6-9 月）
- LumoCloud 控制台（Next.js）：项目/机器人/任务/审计
- Worker Pool：远程 Worker 通过 gRPC 注册到控制台
- 多机器人并发 + 工作队列（SQLite-MQ → NATS JetStream）
- 凭据 Vault（age + macOS Keychain / Win DPAPI / libsecret）+ Just-in-time 注入
- RBAC + SSO（OIDC / SAML）+ 审计
- Time-Travel Debugger：每 step 截图/DOM/变量快照可回放
- 信创构建（UOS/麒麟 + 龙芯/飞腾）
- 应用/插件市场（OCI artifact + 签名）

### M4 · Frontier（9-12 月）
- 移动端自动化（Appium 2.x + UiAutomator2 + scrcpy；可选云手机）
- 云端虚拟桌面 Worker（自托管 KasmVNC + Chromium，解决 AI 抢屏）
- Runbook 模式：Markdown SOP → Agent（Sema4.ai 范式）
- Planner/Actor/Validator 三相 Agent
- 流程评估回归（Eval Set + 自动化测试）
- 失败回放训练数据导出 → 可选 RL 微调本地模型
- 中英双语 + 钉钉/飞书/企微深度集成
- Beta：浏览器云 Service（Steel/Browserbase 模式可选）

---

## 7. 高阶用户旅程（验证产品定位）

### 旅程 A — 电商运营小张（业务用户）
1. 打开 LumoStudio，点击「新建流程」。
2. 选「自然语言生成」，输入：「打开淘宝订单页，把今天的订单导出到 Excel，给每个订单的买家发感谢邮件」。
3. LumoRPA 用 Claude Sonnet 4.6 生成 12 个节点，自动注释参数。
4. 点击「录制补充」，录到的"点击发货按钮"动作自动并入流程。
5. 点击「试跑」，弹出截图回放面板，逐步看每个节点的截图和变量值。
6. 选「按 Excel 循环」，绑定 D 盘的 `订单.xlsx`，每行 = 一次循环。
7. 一键发布到「我的机器人」，设定每天 09:00 跑。

### 旅程 B — 开发者老王（专业用户）
1. `lumo init my-flow` 在终端创建文件夹（含 `flow.yaml`、`actions/`、`tests/`）。
2. 用 VS Code 写 YAML 流程，Code Lens 实时校验。
3. `lumo test tests/integration.yaml` 在本地用 headless 浏览器跑。
4. `git commit && git push`，CI 自动跑 evaluation set。
5. `lumo deploy --target prod` 推送到企业控制台。
6. Grafana 面板看 latency p95、失败率、AI token 用量。

### 旅程 C — AI Agent（Claude/Cursor 等外部 Agent）
1. Claude 通过 MCP 接口看到 LumoRPA 暴露的 200 个 tool。
2. 用户对 Claude 说"帮我从某网站抓数据存到我的 Notion"。
3. Claude 调用 `lumorpa.browser.scrape` 工具，参数为 URL + CSS selectors。
4. LumoRPA 调度本地 Worker 执行，结果以 JSON 返回。
5. Claude 继续调用 `lumorpa.notion.append_row`。
6. 全程 LumoRPA 留 OTel trace，可在控制台审计。

---

## 8. 风险与对策

| 风险 | 对策 |
|---|---|
| Rust + 多平台 + 桌面自动化技术深度大，团队学习曲线陡 | 第一阶段聚焦 Browser + Excel 两条腿，桌面用 AccessKit 抽象，先 Windows UIA 兜住 |
| AI 视觉模型本地推理对显存要求高（UI-TARS-7B 需 16GB+） | 默认调云端（Claude/Gemini CU），本地是 Power User 可选项；4B/3B 量化版可降到 8GB |
| 影刀有数年指令库沉淀（近千条） | 头 6 个月聚焦 80/20 规则，先做高频 400 条；同时开放插件协议吸引社区贡献 |
| CDP-only 浏览器自动化对 Firefox/WebKit 用户不友好 | 第二阶段补 Playwright 后端作为 Firefox/WebKit 适配器 |
| 凭据/沙箱安全做错代价大 | 借鉴 Anthropic + Browserbase 公开实践；安全外审；零信任默认 |
| 开源协议博弈 | 主仓库 Apache-2.0；企业插件可选 Commercial Source-Available；不学 Skyvern AGPL（抑制企业采用） |
| 监管/合规（金融/政企） | 信创构建矩阵 + ISO 27001/等保三级路径预留；审计 OTel 落地 |

---

## 9. 度量与"成功是什么样"

- **入门速度**：新用户从下载到跑通第一个"网页抓取 + Excel 写入" ≤ **10 分钟**。
- **稳定性**：在 100 个真实业务流程评估集上，**首次成功率 ≥ 85%**，加上自愈后 ≥ 95%。
- **性能**：1000 行 Excel 循环 + 简单网页填写，**端到端 ≤ 影刀同等流程 ×0.6**。
- **开发效率**：从录制到可发布 ≤ **影刀 ×0.7**（流程图视图 + 片段测试加成）。
- **社区**：12 个月内 GitHub star ≥ **20k**，应用市场 ≥ **300 个公开应用**。
- **企业**：能跑 ≥ **50 个机器人并行**，p95 抢占冲突率 **= 0**（每 worker 独立 session）。

---

## 10. 下一步

请继续阅读：
- 👉 [产品功能设计 (01)](./01-Product-Design.md) — 完整功能 checklist、节点目录、UI 草案
- 👉 [系统架构设计 (02)](./02-Architecture-Design.md) — 详细组件、数据模型、协议
- 👉 [关键子系统详设 (03)](./03-Subsystems-Deep-Dive.md) — 录制器/选择器/AI/Excel/调度/MCP 逐项展开
