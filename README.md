# LumoRPA

> **开源、AI 原生的 RPA 平台 —— 流程即代码,确定优先,AI 兜底。**
> Rust 执行核心 · TypeScript/Tauri 设计器 (M2) · Python 扩展 · SQLite/libSQL 存储。

[![License](https://img.shields.io/badge/License-Apache_2.0-blue.svg)](LICENSE)
[![Status](https://img.shields.io/badge/Status-M1%20MVP-orange.svg)]()

---

## 什么是 LumoRPA?

LumoRPA 是一款新一代的流程机器人自动化 (RPA) 平台,灵感来自
**影刀 RPA**、UiPath、Power Automate、n8n、Browser-Use 与 Claude
Computer Use,从底层开始就为以下目标而设计:

- **Flow-as-code (流程即代码)** —— 每一条流程都是纯 YAML,可以 `git diff`、`git blame`、走 PR 审核
- **AI 原生 · 确定优先** —— 先用 CSS/XPath/A11y 选择器,失败时再回落到视觉 LLM
- **本地优先** —— 完全离线运行,需要 AI 节点就 BYOK (自带 API Key)
- **多机器人友好** —— 每个 worker 在独立 OS 用户会话中执行,不会抢焦点
- **MCP-first** —— 每个 Action 都自动暴露为一个 MCP 工具

完整设计报告见 [`docs/`](docs/) —— 共四份文档约 1800 行,覆盖产品、架构与子系统深挖。

## 当前状态 —— M1 MVP

本分支对应 **M1 里程碑**(详见 [docs/00-LumoRPA-Master-Report.md §6](docs/00-LumoRPA-Master-Report.md)):

- ✅ Cargo workspace 共 **八个 crate**
- ✅ **LumoFlow DSL**(YAML 解析 + AST + Jinja 模板 + 校验)
- ✅ **Flow VM** 支持步骤级持久化、重试、完整控制流(if / for / for_each / try / catch / finally)
- ✅ **存储层**(SQLite + WAL,libSQL trait 已预留)
- ✅ **22+ 内置 Action**:control(log/set_var/sleep/if/for/for_each/try/fail)· data(json)· file · http · excel(读写)· browser(launch/open/click/type/extract/close)· ai.chat · skill.invoke
- ✅ **AI Router** 接入真实的 OpenAI(Chat + Responses 两套 wire)、Anthropic(Bearer 与 x-api-key 双方案)、DeepSeek、Ollama —— 通过 `~/.lumorpa/providers.toml` 配置(cc-switch 风格),网络访问由 `LUMO_ALLOW_LLM_NETWORK=1` 显式开关控制
- ✅ **Skill 子系统** —— 兼容 Claude Code `SKILL.md` 风格,自动从 `~/.lumorpa/skills/` 加载,可通过 `skill.invoke` 当作子流程调用
- ✅ **CLI**:`lumo init`、`lumo run`、`lumo validate`、`lumo runs list/show`、`lumo actions`、`lumo providers`、`lumo skills`

## 快速开始

```bash
# 构建所有 crate
cargo build --workspace --release

# 跑示例 hello
./target/release/lumo run examples/hello-world.lumoflow.yaml

# 列出全部内置 action
./target/release/lumo actions

# 校验任意流程
./target/release/lumo validate examples/browser-scrape.lumoflow.yaml

# 查看历史运行
./target/release/lumo runs list
./target/release/lumo runs show <run-id>
```

## 一眼看懂 LumoFlow

```yaml
apiVersion: lumorpa.io/v1
kind: Flow
metadata:
  id: hello
  version: 0.1.0
spec:
  inputs:
    - { name: who, type: string, default: world }
  steps:
    - id: greet
      action: control.log
      with: { message: "你好, {{ inputs.who }}" }
```

## LLM Provider 配置(cc-switch 风格)

LumoRPA 内置一个多 profile 路由,兼容:
OpenAI Chat Completions、OpenAI **Responses API**
(`wire_api = "responses"`)、Anthropic Messages API(`x-api-key` 与
`Bearer` 双方案)、DeepSeek 以及本地的 Ollama。

```bash
lumo providers init         # 初始化 ~/.lumorpa/providers.toml,种 4 个默认 profile
lumo providers list         # 列出所有 profile
lumo providers use openai   # 切换激活的 profile
lumo providers test         # 真实拨测一次(需要 LUMO_ALLOW_LLM_NETWORK=1)
```

每个 profile 可以单独覆盖 `wire_api`、`reasoning_effort`、
`disable_response_storage`、自定义请求头、`base_url` 与 `default_model`。
**模型名永远不写死在代码或示例 YAML 里** —— 流程未指定 `model`
时,自动取激活 profile 的 `default_model`。

## Skills —— 可复用的命名流程

Skill 兼容 Claude Code 的 `SKILL.md` 风格,由 frontmatter、Markdown 正文与
一个 fenced yaml 代码块组成,默认放在 `~/.lumorpa/skills/<name>/SKILL.md`
(可通过环境变量 `LUMO_SKILLS_PATH` 覆盖)。

````markdown
---
name: greet
description: 可复用的问候 skill
inputs:
  - { name: who, type: string, required: true, default: world }
triggers: [greet, hello]
---

```yaml
inputs:
  - { name: who, type: string, required: true, default: world }
steps:
  - id: say
    action: control.log
    with: { message: "你好, {{ inputs.who }}!" }
```
````

```bash
lumo skills install ./greet/SKILL.md      # 复制到 skills 根目录
lumo skills list                           # 列出已安装的 skill
lumo skills show greet                     # 查看 frontmatter + flow
lumo skills run  greet -i who=palmer       # 端到端执行一次
```

任何流程都可以通过 `skill.invoke` 调用一个 skill:

```yaml
- id: hi
  action: skill.invoke
  with:
    name: greet
    inputs: { who: lumorpa }
```

## 仓库结构

```
LumoRPA/
├── Cargo.toml                # workspace 与共享依赖
├── crates/
│   ├── lumo-dsl/             # AST · 解析 · 模板 · 校验
│   ├── lumo-storage/         # SQLite 仓储 + schema
│   ├── lumo-core/            # Action trait · 注册表 · FlowVm · StepCtx
│   ├── lumo-actions/         # control · data · file · http · excel · browser
│   ├── lumo-ai/              # AiRouter · OpenAI/Anthropic/DeepSeek/Ollama · ai.chat
│   ├── lumo-skills/          # SKILL.md 加载器 · 注册表 · skill.invoke action
│   ├── lumo-recorder/        # M2 占位(CDP / AccessKit 录制器)
│   └── lumo-cli/             # `lumo` 可执行
├── examples/                 # hello-world / excel-loop / browser-scrape / skill-driver
├── docs/                     # 完整设计报告(00..03)
└── .github/workflows/        # CI 矩阵 (mac / linux / windows)
```

## 路线图

| 里程碑 | 主题 | 预计 |
|---|---|---|
| **M1**(当前分支) | Rust 核心 + DSL + CLI + 22 个 action + AI 路由 + Skill | 0-3 个月 |
| **M2** | AI 路由(Claude/Gemini/GLM)· OmniParser/UI-TARS · CDP 录制器 · MCP server | 3-6 个月 |
| **M3** | LumoCloud 控制面 · 多 worker · vault · 审计 · 信创 | 6-9 个月 |
| **M4** | 移动端 · 云桌面 · Runbook 模式 · PAV agent · RL 自演化 | 9-12 个月 |

完整规划与设计取舍见 [`docs/00-LumoRPA-Master-Report.md`](docs/00-LumoRPA-Master-Report.md)。

## License

Apache-2.0。详见 [LICENSE](LICENSE)。
