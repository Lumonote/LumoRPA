# LumoRPA · 设计文档

> **一句话**：以 YAML 流程即代码（flow-as-code）为契约、Rust 执行内核为底座、确定性动作为能力面、BYOK AI 路由为增值项的**本地优先 RPA 运行时**。

> ## 第一原则
> **执行路径必须可解释、可复现、可离线。** 每条指令要么确定性执行，要么显式声明它依赖外部（模型 / 网络 / 宿主 UI）并因此在无该依赖时**明确失败**。
> **验证判据**：同一份流程 + 同一份输入，在断网环境下要么产出相同结果，要么给出可读的失败原因 —— 不允许静默产出空值或近似值。

---

## 一、文档地图

### 核心文档

| 文档 | 内容 | 何时读 |
| --- | --- | --- |
| [`00-LumoRPA-Master-Report.md`](./00-LumoRPA-Master-Report.md) | 总报告：项目定位、能力全景、里程碑与验收口径 | 需要全局俯瞰时 |
| [`01-Product-Design.md`](./01-Product-Design.md) | 产品设计：用户角色、使用场景、界面与交互、录制—编辑—运行闭环 | 讨论「给谁用、怎么用」时 |
| [`02-Architecture-Design.md`](./02-Architecture-Design.md) | **架构权威规范**：Cargo 工作区分层、VM 执行模型、capability 门禁、存储与调度 | 需要任何架构结论时 —— **以本篇为准** |
| [`03-Subsystems-Deep-Dive.md`](./03-Subsystems-Deep-Dive.md) | 子系统深潜：动作族、浏览器 / 桌面录制、技能加载、AI 路由、语音 | 实现具体子系统时 |
| [`04-优化与补充开发-路线图.md`](./04-优化与补充开发-路线图.md) | 路线图与补强清单：分期目标、验收标准、排期依据 | 规划与排期时 |
| [`05-LumoFlow-Instruction-Set.md`](./05-LumoFlow-Instruction-Set.md) | **LumoFlow 指令集规范**：YAML 结构、步骤字段（含 `on_error` / `retry`）、模板引用、动作清单 | 写流程或加动作时 |
| [`release.md`](./release.md) | **安装包从哪来**：main / tag 两条发布通道、各平台产物清单、未签名包如何处理 | 需要下发或排查安装包时 |
| [`06-Capability-Coverage-Matrix.md`](./06-Capability-Coverage-Matrix.md) | **能力覆盖矩阵**：成熟商业 RPA 指令的对表进度、A/B/C 缺口分级、测试与回归实测数字 | 判断「还差什么」时 |

### 专题设计（实现相关能力时并读）

| 文档 | 内容 |
| --- | --- |
| [`superpowers/specs/2026-07-11-desktop-voice-agent-skill-mcp-design.md`](./superpowers/specs/2026-07-11-desktop-voice-agent-skill-mcp-design.md) | 桌面语音智能体 × 技能 × MCP 的装配设计 |
| [`superpowers/specs/2026-05-30-s-class-actions-design.md`](./superpowers/specs/2026-05-30-s-class-actions-design.md) | S 级动作族的设计契约与验收 |
| [`superpowers/specs/2026-05-30-encrypted-vault-design.md`](./superpowers/specs/2026-05-30-encrypted-vault-design.md) | 本地加密凭据库：密钥派生、解锁生命周期、与动作层的边界 |
| [`superpowers/specs/2026-07-25-voice-quick-commands-voicepack-design.md`](./superpowers/specs/2026-07-25-voice-quick-commands-voicepack-design.md) | 语音快捷指令与语音包格式 |
| [`superpowers/plans/`](./superpowers/plans/) | 阶段性实施计划（按日期归档），含每期的验收清单 |

---

## 二、阅读路径

| 目的 | 路径 |
| --- | --- |
| **快速对齐全局**（30 分钟） | `00` 总报告 → `02` 架构的「分层与执行模型」一节 → `05` 指令集 |
| **写第一条流程** | `05` 指令集（YAML 结构 + 步骤字段） → `cargo run -p lumo-cli -- actions` |
| **加一个新动作** | `05` 指令集 → `02` 架构的 capability 门禁一节 → `crates/lumo-actions/src/` 下的同族实现 |
| **判断功能缺口** | `06` 覆盖矩阵 §2 现状 → §4 缺口分级（A 可做 / B 依赖宿主 / C 明确不做） |
| **实现具体子系统** | `03` 深潜对应章节 + `02` 架构约束 |
| **规划排期** | `04` 路线图 + `06` 覆盖矩阵 §4 |

---

## 三、技术栈一览

```text
语言:        Rust（edition 2021，Cargo workspace）
流程契约:    LumoFlow YAML（apiVersion: lumorpa.io/v1）
执行内核:    lumo-core VM —— 步骤机 + 重试与失败终局处置 + 取消传播 + 分支合流
指令面:      lumo-actions（262 条已注册动作，按 capability 门禁归类）
校验:        lumo-dsl —— 结构校验 + 模板引用检查 + linter（含 on_error.silent_bind）
AI 路由:     lumo-ai —— BYOK，多 provider，流式与非流式
技能:        lumo-skills —— SKILL.md 加载 + flow.call 子流程（含递归深度护栏）
存储:        lumo-storage —— SQLite 运行历史与资源
录制:        lumo-recorder —— 浏览器（CDP）与桌面双通道
接口:        lumo-cli（headless，CI 可用） + apps/desktop（Tauri 工作台）
前端:        原生 JS + node --test + ESLint 9（无框架构建链）
```

---

## 四、硬规矩

1. **动作契约统一**：任何动作实现 `Action`（`id` / `summary` / `schema` / `execute`），输入结构体一律 `deny_unknown_fields`。
2. **capability 零漂移**：每个注册动作必须落在 `ACTION_CAPS` 或 `NO_CAP_ACTIONS` **二者之一**，由测试守着；`ACTION_CAPS` 必须严格字典序。
3. **指令集清单同步**：`05` 的 `ACTIONS_START` / `ACTIONS_END` 区块必须与注册表逐条一致，由 `crates/lumo-cli/tests/instruction_set_docs.rs` 守着。
4. **缺省行为不漂移**：新增字段一律 `#[serde(default)]`，未配置的既有流程执行路径不变。
5. **失败优先于静默**：无宿主 / 无依赖 / 不可达时**返回可读错误**，绝不 panic，绝不静默返回空值 —— 静默空值让流程带着错误数据继续跑，比直接失败难排查。
6. **诚实标注**：语义与外部产品不等价时，把差异写进返回值或文档，不用近似实现冒充。
7. **文档随代码走**：改了动作表 / 步骤字段 / 能力分级，必须同步更新 `05` / `06`，并重跑文档一致性测试。

---

## 五、验证入口

```bash
# 环境（沙箱内 cargo registry 不可写，必须重定向 CARGO_HOME）
export PATH="$HOME/.cargo/bin:$PATH"
export CARGO_HOME=$PWD/.cargo-home

# 全量测试与静态检查
cargo test --workspace --all-targets --no-fail-fast
cargo clippy --workspace --all-targets -- -D warnings

# 实测注册表（对照 05 的指令清单）
cargo run -p lumo-cli -- actions

# 前端（node 25 有兼容问题，用 nvm 的 v22）
cd apps/desktop/frontend
export PATH="$HOME/.nvm/versions/node/v22.23.2/bin:$PATH"
node --test && npm run lint
```

---

*文档与代码不一致时，以**代码与测试**为准，并立即修正文档 —— 不一致本身就是缺陷。*
