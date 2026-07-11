# LumoRPA 桌面语音智能体、Skill 与 MCP 设计

日期：2026-07-11  
状态：已完成产品与架构确认

## 1. 目标

为 LumoRPA 桌面端增加一套可正式交付的语音智能体入口。用户可以通过本地唤醒词或全局快捷键唤醒 Lumo，以自然语言调用 Flow、Skill 和 MCP Tool，并在统一的 Mission Control 中观察串行、并行、当前步骤、输入摘要、权限、进度、结果、失败恢复和重新规划。

系统同时提供 Agent Harness、可控 Agent Loop 和受监督 Self-Improvement。智能体可以反思、重试、切换工具、重新规划并提出持久改进，但不能自行批准或启用对 Flow、Skill、Prompt、路由及权限策略的修改。

## 2. 已确认的产品决策

- 唤醒方式：本地唤醒词与全局快捷键同时支持。
- 语音识别：本地唤醒词；STT 支持本地与云端 Provider 切换。
- 安全策略：风险分级。只读低风险调用可直接执行，高风险副作用必须确认。
- 命令路由：快捷口令和别名优先，AI 语义路由补充。
- 首发平台：macOS，架构保留 Windows/Linux 平台适配层。
- 管理范围：完整 Skill/MCP 管理，不包含在线市场。
- 反馈方式：短确认和关键结果可语音播报，详细信息保留在界面，可一键静音。
- 桌面入口：悬浮胶囊，必要时展开确认面板或 Mission Control。
- 执行视图：动态拓扑图与当前步骤实时详情并列。
- Self 等级：受监督自我进化，所有持久变更必须经过评估和人工批准。
- MCP 导入：支持通用配置导入、常见客户端迁移、批量预览、去重、Secret 迁移和连接验证。

## 3. 当前项目基础

现有工程是 Rust workspace、Tauri 2 桌面 Shell 和原生 ESM 前端。可直接复用的能力包括：

- Tauri command 与事件通道；
- Flow VM、取消、进度、人机回执和持久化运行记录；
- `lumo-skills` 的 Skill 加载、编译和 Registry；
- `McpCallAction` 与 CLI MCP Server 基础；
- CLI 全局热键与桌面设置页；
- Vault、Capability 校验、Provider 配置和 SQLite Repo。

本设计不创建平行的第二套执行引擎。语音、Agent 和管理中心通过适配器接入现有 Flow、Skill、MCP、存储和安全能力。

## 4. 范围

### 4.1 首版包含

- macOS 麦克风权限、音频采集、本地关键词检测和全局快捷键；
- 本地/云端 STT Provider 路由与系统 TTS；
- Flow、Skill、MCP Tool 的统一能力目录；
- Agent Harness、Plan–Act–Observe–Validate–Reflect 循环；
- 风险判断、人工确认、预算、取消、暂停、继续和重规划；
- 悬浮胶囊、确认卡、Mission Control 和执行日志；
- Skill 完整管理；
- MCP 通用导入、连接管理、工具发现、手动调用与诊断；
- 执行轨迹、评估和受监督改进提案；
- SQLite 迁移、恢复、审计和测试。

### 4.2 首版不包含

- Skill/MCP 在线市场；
- 未经批准的自动代码、Flow、Skill、Prompt 或策略修改；
- 后台静默执行 L2/L3 风险操作；
- 默认保存原始音频；
- Windows/Linux 的产品级交付，首版只保留平台接口和可测试适配边界。

## 5. 总体架构

```text
Wake Word / Global Hotkey
          │
          ▼
Audio Capture → STT Router → Transcript
          │
          ▼
┌──────────────── Agent Harness ────────────────┐
│ Session Context · Capability Catalog          │
│ Policy/Budget · Memory/Trace · Model Router   │
└──────────────────────┬────────────────────────┘
                       ▼
       Plan → Risk Gate → Act → Observe
          ▲                         │
          └── Reflect ← Validate ←──┘
                       │
                       ▼
┌────────── Unified Invocation Runtime ─────────┐
│ Flow Adapter │ Skill Adapter │ MCP Adapter    │
└──────────────────────┬────────────────────────┘
                       ▼
             Append-only Agent Events
                       │
          ┌────────────┴────────────┐
          ▼                         ▼
 Floating Capsule            Mission Control

Completed Traces → Proposal → Sandbox Evaluation
                 → Human Approval → Versioned Apply/Rollback
```

### 5.1 模块边界

建议增加两个独立 Rust crate，并在桌面宿主中增加薄适配层：

- `lumo-voice`：音频采集、唤醒、STT/TTS Provider、语音状态机，不理解 Flow、Skill 或 MCP。
- `lumo-agent`：统一能力模型、Harness、Planner、Loop、Policy、预算、事件和改进提案，不依赖 Tauri UI。
- `apps/desktop/src-tauri`：平台权限、全局快捷键、窗口控制、事件桥接和生命周期管理。
- `apps/desktop/frontend`：胶囊、Mission Control、Capability Hub 和配置 UI。

执行适配器依赖现有 `lumo-core`、`lumo-skills` 和 `lumo-actions`，不会让这些底层 crate 依赖桌面或语音模块。

桌面首版延续当前原生 ESM、HTML 和 CSS 技术栈，不在本功能中引入 React 或前端构建链迁移。全局快捷键通过 Tauri 2 的 global-shortcut 插件接入。

## 6. Voice Edge

### 6.1 Provider 接口

`lumo-voice` 提供以下可替换接口：

- `WakeWordProvider`：持续接收 PCM 帧，输出关键词、置信度和时间戳；
- `SttProvider`：支持流式 partial/final transcript、语言和取消；
- `TtsProvider`：播放短反馈，支持停止和语速配置；
- `AudioCapture`：平台音频输入和设备切换。

macOS 首版默认使用 sherpa-onnx 的关键词检测和流式离线 ASR；模型作为可下载资源管理，不打入主二进制。云端 STT 复用 Provider 配置和 Vault。模型仍被视为可替换资源，不进入上层状态机或数据库业务 Schema。TTS 首版使用 macOS `AVSpeechSynthesizer` 平台适配器。

### 6.2 隐私规则

- 唤醒前只运行本地关键词检测；
- 环形音频缓冲只用于补齐关键词之后的首句，不持久化；
- 云端 STT 只有在用户选用云端 Profile 时上传唤醒后的语音；
- 原始音频默认不保存；
- 菜单栏和胶囊持续显示监听、静音和录音状态；
- 用户可以分别禁用唤醒词、快捷键或云端 STT。

### 6.3 状态机

```text
Disabled → Idle → WakeDetected → Listening → Transcribing
                                      │             │
                                      └── Cancel ───┘
Transcribing → Routing → Planning → Confirming? → Executing → Reporting → Idle
```

任何状态都响应取消。执行期间，“暂停、继续、取消、跳过这步、展开详情”作为系统级控制意图处理，不进入普通工具路由。

## 7. 统一能力目录

Flow、Skill 和 MCP Tool 统一映射为 `CapabilityDescriptor`：

```rust
pub struct CapabilityDescriptor {
    pub id: String,
    pub source: CapabilitySource,
    pub name: String,
    pub description: String,
    pub input_schema: serde_json::Value,
    pub output_schema: Option<serde_json::Value>,
    pub aliases: Vec<String>,
    pub examples: Vec<String>,
    pub risk: RiskLevel,
    pub enabled: bool,
    pub version_hash: String,
}
```

其中 `CapabilitySource` 明确区分 Flow、Skill 和 MCP Server/Tool。路由器只看到经过权限过滤、当前 Agent Profile 可见且健康的能力。

混合路由顺序为：

1. 系统控制意图；
2. 精确别名和快捷口令；
3. 高置信度本地匹配；
4. AI 在过滤后的候选能力中选择；
5. 候选置信度接近时向用户追问一个澄清问题。

## 8. Agent Harness

Harness 为每次语音任务创建隔离 Session，持有：

- 对话与桌面上下文；
- 当前能力快照；
- Agent Profile、模型和 Provider；
- 权限策略与审批记录；
- 最大步骤、运行时间、Token、成本和并发预算；
- 短期 Memory 和脱敏 Trace；
- 取消、暂停和事件发布器。

默认预算为最大 20 个 Loop 步骤、4 路并发；运行时间、Token 和成本由 Profile 进一步限制。触达任一硬限制即停止并报告，不允许无限循环。

## 9. Agent Loop

Loop 使用明确状态，不依赖自由文本隐式驱动：

```text
Plan → Validate Plan → Risk Gate → Execute Ready Nodes
     → Observe → Validate Result → Reflect
     → Complete | Retry | Replace Tool | Replan | Ask User | Fail
```

Planner 输出可序列化 DAG。每个节点包含依赖、能力 ID、结构化参数、风险、超时、重试、预期输出和验证规则。Executor 只执行已验证且已授权的节点。

并行只发生在依赖满足、权限通过且预算允许的节点之间。任何重规划如果扩大权限、数据范围或外部影响，都必须重新确认。

Validator 优先使用确定性 Schema、断言和工具结果检查；只有无法确定时才调用模型判断。Reflector 只能从有限动作集合中选择下一步，不能直接调用工具。

## 10. 受监督 Self-Improvement

Self-Improvement 不在活动运行中修改系统。它在运行结束后处理脱敏轨迹：

1. Trace Miner 统计成功率、耗时、重试、人工纠正和工具替换；
2. Proposal Builder 生成别名、路由、Prompt、Flow 或 Skill 的结构化差异；
3. Evaluation Sandbox 使用历史样本和安全用例回放；
4. 评估输出质量、成功率、延迟、成本、权限变化和回归；
5. 用户查看差异和评估结果后批准或拒绝；
6. 批准项生成新版本，可一键回滚。

改进系统不能修改 Vault、审批记录、安全硬限制或自身批准逻辑。

## 11. 桌面交互

### 11.1 悬浮胶囊

- 待机时隐藏或保持极简菜单栏图标；
- 唤醒后显示音量波形和流式转写；
- 路由后显示将调用的能力来源；
- 低风险任务直接进入执行状态；
- 高风险任务展开确认卡，列出关键参数、影响范围和权限；
- 支持键盘、鼠标和语音取消；
- 完成后短暂显示结果摘要，详细内容留在 Mission Control。

### 11.2 Mission Control

左侧显示实时执行拓扑，右侧显示当前节点详情：

- 串行依赖和并行分支；
- 当前节点呼吸光、数据流连线和进度；
- Flow/Skill/MCP/AI 来源；
- 输入与输出的脱敏摘要；
- 风险、审批、耗时、Token、成本和重试次数；
- 错误、替代工具、重规划和回退路径；
- 底部可切换完整事件日志。

动效只表达运行状态、数据方向、并发、等待和失败，不使用与信息无关的持续装饰动画。系统遵循 `prefers-reduced-motion`。

### 11.3 自适应语音反馈

默认播报短确认、需要用户回应的确认问题和关键结果。长文本、结构化数据和详细错误只显示在界面。用户可以全局静音或对当前 Session 静音。

## 12. Capability Hub

Capability Hub 包含：总览、Skills、MCP Servers、能力目录、Agent Profiles、权限策略、执行与审计、改进提案。

### 12.1 Skill 管理

- 从本地文件、目录或 Git 地址导入；
- 解析 frontmatter、Flow、依赖和能力声明；
- 校验、启停、版本、更新差异和回滚；
- 浏览编译后的 Flow、输入 Schema 和风险；
- 配置语音别名和示例说法；
- 手动试运行并查看事件。

### 12.2 MCP 管理

- 支持 stdio、Streamable HTTP 和兼容旧服务的 SSE；
- 配置连接、Vault Secret 引用、超时和启用状态；
- 连接测试、工具发现、Schema 浏览和手动调用；
- Server 和 Tool 级启停、风险和语音别名；
- 健康率、延迟、错误率、调用日志和脱敏诊断包。

### 12.3 通用 MCP 导入

导入入口支持：

- 自动发现 Claude Desktop、Cursor、Codex 等常见客户端配置；
- JSON、JSONC、YAML、TOML 文件；
- 通用 `mcpServers` 对象或单 Server 配置；
- 粘贴配置、远程 URL、启动命令和环境变量片段；
- 手动表单。

导入流水线为：解析识别 → 标准化 → 预览 → 去重/冲突处理 → Secret 映射 → 连接测试 → 工具发现 → 风险标注 → 按条目启用。

内部使用稳定、版本化的 MCP Profile Schema。外部格式通过 Import Adapter 转换；未知字段保留在扩展区。失败条目保存为未启用草稿，不影响其他条目。

### 12.4 Agent 控制面

- 选择 Planner、Validator 和 Reflector 模型；
- 配置最大循环、并发、运行时间、Token 和成本；
- 配置可见能力、默认风险策略和 Memory 保留；
- 查看 Trace、评估和改进差异；
- 批准、拒绝、启用和回滚改进版本。

## 13. 风险与权限

风险等级：

- L0：纯查询和读取状态，可直接执行；
- L1：受限本地读取，按 Profile 策略执行；
- L2：写文件、联网提交和发送消息，必须确认；
- L3：删除、凭据操作、桌面控制和批量外部影响，需要强化确认。

权限可以限制到 Server、Skill、Tool、动作和参数范围。计划固化能力版本、Schema 哈希、模型、权限和输入摘要。Agent 不能通过替换工具降低原任务风险。

MCP 返回、网页、文档、邮件和模型输出均视为不可信数据，不能直接修改 System Prompt、授权、工具可见性或审批状态。

## 14. 数据模型

在现有 SQLite Repo 中增加版本化迁移：

- `voice_profiles`；
- `mcp_servers`；
- `mcp_tools`；
- `capability_aliases`；
- `agent_profiles`；
- `agent_runs`；
- `agent_events`；
- `improvement_proposals`；
- `improvement_approvals`。

`agent_events` 使用追加写事件流，支持 UI 重建和崩溃恢复。原始音频默认不保存；Transcript、Trace 和事件使用可配置保留周期并在写入前脱敏。

## 15. 统一事件协议

核心事件至少包含：

```text
session.started
voice.wake_detected
transcript.delta / transcript.final
route.selected
plan.created / plan.revised
permission.requested / permission.resolved
node.queued / node.started / node.progress
tool.called / tool.result
node.completed / node.failed / node.cancelled
run.paused / run.resumed / run.completed / run.failed
improvement.proposed / improvement.approved / improvement.rejected
```

每个事件带 `session_id`、`run_id`、单调递增序号、时间戳、节点 ID、父节点 ID 和脱敏 payload。前端只根据事件投影状态，不直接推测执行进度。

## 16. 错误与恢复

- STT Provider 失败时按隐私策略切换本地或云端；
- MCP 使用超时、指数退避、健康熔断和显式重连；
- MCP Schema 哈希变化后暂停相关工具，重新发现并确认；
- 节点可以重试、切换等价能力或重新规划；
- 权限扩大必须重新确认；
- 取消信号贯穿录音、STT、Planner、Flow VM、Skill 和 MCP 子进程；
- 应用重启后根据事件恢复 UI；
- 无法确认是否完成的外部副作用标记为“状态未知”，禁止自动重复执行；
- 达到预算、循环或时间上限时停止并提供已完成结果和继续选项。

## 17. 测试策略

### 17.1 单元测试

- Voice 状态机和取消；
- 混合路由、别名冲突和澄清；
- 风险分级和参数范围；
- DAG 校验、并行就绪节点和预算；
- MCP JSON/JSONC/YAML/TOML 导入适配器；
- 改进提案不能绕过审批。

### 17.2 契约与集成测试

- Flow、Skill、stdio MCP、Streamable HTTP MCP 适配器；
- Tool Schema 变化和失效；
- Tauri command 与事件顺序；
- SQLite 迁移、恢复和保留策略；
- Vault 引用与日志脱敏；
- 暂停、取消、超时和应用重启恢复。

### 17.3 前端与端到端测试

- 串并行 DAG 投影；
- 节点状态、重规划和回退路径；
- 风险确认和语音打断；
- Capability Hub 导入、连接测试和手动调用；
- macOS 麦克风权限、快捷键和模拟音频；
- Mission Control 视觉回归与 reduced-motion。

### 17.4 安全评估

- Prompt Injection；
- 恶意 MCP 返回和 Tool 描述；
- 参数越权和工具替换降级；
- Secret、Transcript 和 Trace 泄漏；
- 重复外部副作用；
- Self Proposal 修改审批、安全硬限制或 Vault。

## 18. 验收指标

- macOS 全局快捷键触发 UI 的目标延迟小于 150ms；
- 执行事件到 UI 的目标延迟小于 100ms；
- 本地唤醒空闲 CPU 目标低于 5%；
- 低风险精确别名可在不调用 Planner 模型时直接路由；
- 所有 L2/L3 调用在执行前产生可审计审批记录；
- 所有 Agent 运行受步骤、时间、Token、成本和并发硬限制；
- 崩溃恢复不会自动重复状态未知的外部副作用；
- MCP 批量导入中单条失败不阻断其他条目；
- 改进提案未经批准不能影响后续运行；
- Mission Control 能准确表示串行、并行、等待、失败、重试、替代工具和重新规划。

## 19. 实施拆分原则

实施计划按以下依赖顺序拆分，每个阶段保持可测试和可运行：

1. 统一能力模型、事件协议和存储迁移；
2. MCP Profile、通用导入和 Capability Hub；
3. Invocation Runtime 与 Flow/Skill/MCP Adapter；
4. Agent Harness、风险门、预算和确定性 Loop；
5. Mission Control 与执行拓扑；
6. Voice Edge、快捷键、胶囊、STT/TTS；
7. AI Planner/Validator/Reflector；
8. Self-Improvement、评估、审批和回滚；
9. macOS 性能、安全和端到端验收。

该顺序先建立可审计的执行和管理底座，再开放语音与模型自治，避免语音入口先于权限、事件和恢复能力上线。
