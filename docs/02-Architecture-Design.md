# 02 · LumoRPA 系统架构设计

> 总报告：[00-LumoRPA-Master-Report.md](./00-LumoRPA-Master-Report.md)
> 产品功能：[01-Product-Design.md](./01-Product-Design.md)

本文回答"**这套系统在工程上长什么样、各模块怎么搭、数据如何流动**"。

---

## 1. 架构总览

```
                    ┌─────────────────────────────────────┐
                    │     LumoStudio (Desktop)            │
                    │  Tauri 2 + React 19 + ReactFlow     │
                    │  ├─ 3-way isomorphic Editor         │
                    │  ├─ Recorder Panel                  │
                    │  ├─ Trace Viewer / Time-travel       │
                    │  └─ Marketplace                     │
                    └──────────┬──────────────────────────┘
                  Tauri IPC    │   gRPC (TLS)
                               ▼
┌──────────────────────────────────────────────────────────────────┐
│                       lumo-core (Rust)                            │
│  ┌──────────────┐  ┌──────────────┐  ┌──────────────────────┐   │
│  │   API Gate    │  │  Flow VM      │  │  Action Registry     │   │
│  │ tonic + axum  │  │  (DSL exec)   │  │  (built-in+plugin)   │   │
│  └──────┬────────┘  └──────┬───────┘  └──────┬───────────────┘   │
│         │                  │                  │                   │
│  ┌──────▼──────────────────▼──────────────────▼────────────────┐ │
│  │                  Capability Drivers                          │ │
│  │ Browser(CDP)  Desktop(AccessKit)  Office(polars+py)         │ │
│  │ OCR(paddle)   HTTP  DB  Files  Mobile(adb/appium)            │ │
│  └──────────────────────────────────────────────────────────────┘ │
│  ┌──────────────────────────────────────────────────────────────┐ │
│  │  AI Router  ·  Selector Engine  ·  Recorder Engine           │ │
│  │  Scheduler  ·  Queue  ·  Trigger Manager  ·  Sandbox         │ │
│  └──────────────────────────────────────────────────────────────┘ │
│  ┌──────────────────────────────────────────────────────────────┐ │
│  │  Storage:  libSQL (state + vectors)  ·  Object Store (blob)  │ │
│  │  Observability:  OTel exporter (OTLP/file/Grafana)            │ │
│  └──────────────────────────────────────────────────────────────┘ │
│  ┌──────────────────────────────────────────────────────────────┐ │
│  │  Embedded Python 3.12 (PyO3)   for code-node + py plugins   │ │
│  └──────────────────────────────────────────────────────────────┘ │
└──────────────────────────────┬───────────────────────────────────┘
                  gRPC / MCP  │
                              ▼
┌──────────────────────────────────────────────────────────────────┐
│            LumoCloud (Optional self-host)                         │
│   Next.js 15 + tRPC + libSQL/Postgres + S3-compatible             │
│   ├─ Orchestrator (DAG, queue, fan-out)                          │
│   ├─ Worker Pool (mTLS gRPC, heartbeat)                          │
│   ├─ Vault (age + KMS)                                           │
│   ├─ RBAC + SSO (OIDC/SAML/LDAP)                                  │
│   ├─ Audit & Session Replay                                      │
│   └─ Marketplace Server (OCI registry)                            │
└──────────────────────────────────────────────────────────────────┘
```

---

## 2. 进程拓扑

| 模式 | 拓扑 | 适用 |
|---|---|---|
| **单机本地** | LumoStudio.exe（Tauri main = Rust core; renderer = web） + 单进程 `lumo-runner` | 个人/小团队 |
| **本地多 Worker** | Studio + N×Runner（独立 OS user）+ 共用 SQLite-MQ | 单机多机器人 |
| **远程 Worker** | Studio → Cloud Orchestrator → 远程 Runner（gRPC mTLS） | 企业 |
| **混合云** | Studio → Cloud（公司内）→ 桌面 Runner（员工电脑） / 虚拟桌面 Runner（公司机房） | 信创/合规 |

`lumo-runner` 是 Rust 静态二进制（~30 MB），含 Python 嵌入。

---

## 3. 技术栈选型（含版本下限）

### 3.1 Rust 内核

| 选项 | 版本 | 用途 |
|---|---|---|
| `rustc` | 1.83+ | 全工程 |
| `tokio` | 1.45+ | 异步运行时 |
| `tonic` + `prost` | 0.12+ | gRPC server/client |
| `axum` | 0.7+ | HTTP server（Webhook、本地 API） |
| `chromiumoxide` | 0.7+ | CDP 客户端 |
| `accesskit` + `accesskit_windows/macos/unix` | 0.16+ | 跨平台 a11y |
| `windows-rs` / `core-foundation` / `zbus` | latest | 平台原生 |
| `libsql` Rust client | 0.7+ | 主存储 |
| `tokio-cron-scheduler` | 0.15+ | 调度 |
| `opentelemetry` + `opentelemetry-otlp` | 0.27+ | OTel |
| `pyo3` + `pyo3-async-runtimes` | 0.23+ | Python 内嵌（Rust 1.83 兼容） |
| `polars` | 0.45+ | DataFrame 引擎 |
| `image` + `opencv-rust` | latest | OpenCV 模板匹配 |
| `age` | 0.10+ | 凭据加密 |
| `cosign-rs` / `sigstore-rs` | 最新 | 插件签名 |
| `napi-rs` | 2.16+ | 可选 Node 扩展 |

### 3.2 TypeScript / 前端

| 选项 | 版本 | 用途 |
|---|---|---|
| Tauri | 2.9.x | 桌面 shell |
| React | 19.x | UI |
| TypeScript | 5.5+ | 类型 |
| ReactFlow（@xyflow/react） | 12.x | 节点编辑器 |
| Monaco Editor | 0.50+ | 代码视图 + YAML LSP |
| TanStack Query + Zustand | latest | 状态管理 |
| shadcn/ui + Radix UI + Tailwind v4 | latest | 组件库 |
| Zod | 3.x | Schema 校验 |
| tRPC | 11.x | Studio ↔ Core 内部 RPC（可选 Tauri channel 直连） |

### 3.3 Python 内嵌

| 选项 | 用途 |
|---|---|
| CPython 3.12 stable ABI (`abi3`) | 内嵌 |
| `openpyxl`, `pandas`, `polars[python]`, `pdfplumber`, `python-docx`, `python-pptx`, `pywinauto`（仅 Win）, `appium-python-client`, `paddleocr` | RPA 库 |
| `maturin` | 打包 PyO3 扩展 |

### 3.4 LumoCloud（控制台）

| 选项 | 版本 | 用途 |
|---|---|---|
| Next.js | 15.x（App Router + RSC） | Web UI + API |
| tRPC | 11.x | 内部 API |
| Drizzle ORM | latest | 类型安全 SQL |
| libSQL（自托管 sqld）/ Postgres 16 | — | 主库 |
| MinIO / S3 兼容 | — | 对象存储 |
| Lucia + Oslo | latest | Auth |
| Casbin-rs / OPA | latest | RBAC/ABAC |

### 3.5 AI 视觉与 OCR

| 模型 | 部署 | 备注 |
|---|---|---|
| OmniParser v2 | ONNX/torch + 本地推理 | 屏幕元素解析 |
| UI-TARS-1.5-7B | vLLM/ollama | 本地决策视觉 grounding |
| UI-TARS-2 | 云 API（字节） | 进阶 |
| PaddleOCR 3.0（PP-OCRv5 + PP-StructureV3） | Paddle Inference | OCR/表格 |
| Claude Computer Use `computer-use-2025-11-24` | Anthropic API | 云端默认 |
| Gemini 2.5 Computer Use | Google API | 浏览器路由 |
| Fara1.5 | 可选本地 | 4B/9B/27B |
| Qwen2.5-VL-7B/32B、GLM-4.5V、CogAgent v2 | 可选本地 | 备选 |

---

## 4. LumoFlow DSL（流程即代码）

### 4.1 设计目标
- **可 Git diff**（纯文本 YAML）
- **可 LSP 校验**（每个 Action 自带 JSON Schema）
- **可 deterministic replay**（Inngest step durability）
- **三视图同源 AST**（Graph/Node/Code 同 AST 不同呈现）

### 4.2 顶层 schema

```yaml
apiVersion: lumorpa.io/v1
kind: Flow
metadata:
  id: order-export
  name: 订单导出与感谢邮件
  version: 0.3.2
  description: |
    遍历淘宝今日订单，写 Excel，并对买家发送感谢邮件
  authors: [zhangsan@example.com]
  tags: [ecommerce, daily]
spec:
  inputs:
    - { name: date, type: date, default: today() }
  outputs:
    - { name: exported_path, type: file }
  vault:
    - taobao_session
    - smtp_pass

  triggers:
    - kind: cron
      with: { expr: "0 9 * * *" }

  capabilities:        # 显式声明（默认 deny）
    network: [taobao.com, smtp.example.com]
    fs.write: ["${HOME}/Documents/lumo/**"]
    llm: [anthropic]

  resources:           # 资源声明（启动一次，全流程复用）
    browser:
      kind: chromium.cdp
      profile: stealth-default

  steps:
    - id: open-orders
      action: browser.open
      with:
        url: "https://www.taobao.com/orders?date={{ inputs.date }}"
      retry: { times: 3, backoff: exponential }

    - id: each-order
      action: browser.for_each
      with:
        selector: { css: "tr.order-row", strategy: similar-elements }
      do:
        - id: extract
          action: browser.extract
          with:
            map:
              order_id: { css: ".order-id" }
              buyer:    { css: ".buyer" }
              email:    { css: ".email" }
              amount:   { css: ".amount", as: number }

        - id: write
          action: excel.append_row
          with:
            file: "{{ outputs.exported_path }}"
            row:  "{{ extract.result }}"

        - id: thanks
          action: smtp.send
          with:
            to:      "{{ extract.result.email }}"
            subject: "感谢您的订单 #{{ extract.result.order_id }}"
            body_file: templates/thanks.md
            secrets:
              user: "${{ vault.smtp_pass.user }}"
              pass: "${{ vault.smtp_pass.pass }}"
```

### 4.3 关键设计点

1. **Action 是 URI 形如 `family.verb`**，注册在 Action Registry，可被插件扩展。
2. **`steps[*].id`** 全流程唯一，是回放/审计/截图 key（参考 Inngest step durability）。
3. **`with`** 是 schema 化参数（每 Action 有 JSON Schema）。
4. **`retry`** 是声明式重试。
5. **`vault.*`** 模板表达式由 Vault 在执行前 just-in-time 注入；模板渲染**永远不出现在日志/截图/OTel attr 中**。
6. **`capabilities`** 是 capability-based security 声明，缺一即默认 deny。
7. **三原语映射**：底层执行时把 high-level Action 翻译为 Stagehand 风格 `act/extract/observe` 三类原子，AI fallback 在原子层介入。

### 4.4 DSL → AST → 三视图

```
        ┌─→ Graph View   (ReactFlow nodes + edges)
flow.yaml ─→ parse ─→ AST ─┼─→ Node Form View (auto-generated form per Action schema)
        └─→ Code View    (Monaco editor, two-way)
```

任意视图修改都直接更新 AST，再 serialize 回另外两个视图。

---

## 5. 执行引擎（Flow VM）

### 5.1 模型
- 类似 **Inngest** 的 **durable function** 模型：每个 step 完成即 commit 到 SQLite，失败重启时从最近 checkpoint 恢复。
- 借鉴 **Temporal** 确定性 replay：input/output 全量持久化，回放可复现。
- 单流程默认串行，可显式 `parallel:` 块。

### 5.2 关键数据结构（Rust）

```rust
pub struct Flow {
    pub meta: FlowMeta,
    pub spec: FlowSpec,
}

pub struct StepRun {
    pub step_id: String,
    pub flow_run_id: Uuid,
    pub idx: u32,
    pub state: StepState,       // Pending | Running | Ok | Failed | Retrying
    pub input_hash: Sha256,
    pub output: serde_json::Value,
    pub started_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
    pub artifacts: Vec<ArtifactRef>, // screenshot, dom, har
    pub span_id: TraceId,
    pub error: Option<StepError>,
}

#[async_trait]
pub trait Action: Send + Sync {
    fn id(&self) -> &'static str;             // "browser.click"
    fn schema(&self) -> &JsonSchema;
    async fn execute(&self, ctx: &mut StepCtx, input: Value) -> Result<Value, ActionError>;
    fn capabilities(&self) -> CapabilitySet;
}
```

### 5.3 Step 执行流水线

```
┌─ Step ─────────────────────────────────────────────────────────────┐
│ 1. Resolve inputs (template render w/ vault JIT inject)            │
│ 2. Check capability allow                                          │
│ 3. Open OTel span (gen_ai.* attributes for AI nodes)               │
│ 4. Pre-snapshot: screenshot + DOM + variables                      │
│ 5. Dispatch to Action.execute()                                    │
│      ├─ deterministic path (selector hit ok)                       │
│      └─ AI fallback (Stagehand `act/extract/observe`)              │
│         ├─ Self-Healing Router (graph dijkstra)                    │
│         └─ Vision-LLM (OmniParser/UI-TARS or Computer Use)         │
│ 6. Post-snapshot                                                   │
│ 7. Persist StepRun to libSQL + artifacts to ObjectStore            │
│ 8. Close span                                                      │
└────────────────────────────────────────────────────────────────────┘
```

### 5.4 Self-Healing Router（Δ核心创新）

- 把选择器组织成 **多策略图**：CSS → XPath → A11y → Vision-LLM 各为节点，边权 = 平均时延 + 失败概率。
- 每次执行后更新边权（指数滑动平均）。
- 选择器失败：边权 → ∞，**Dijkstra** 立即重路由到次优路径。
- **80% 失败可在不调 LLM 情况下自愈**（基于 PALADIN 数据）。
- 仅当所有静态路径都失败才走 Vision-LLM 重锚定。

---

## 6. AI Router

### 6.1 职责
- 统一接口给上层（LLM/Embed/Vision/OCR/ComputerUse）。
- BYOK 多 provider，按节点策略路由。
- 成本/速率/超时统一治理。
- 缓存（prompt → 响应）+ 流式输出。

### 6.2 Provider 矩阵

| 用途 | 默认 provider | 备选 |
|---|---|---|
| 文本 LLM | Anthropic Claude Opus 4.7 / Sonnet 4.6 | OpenAI GPT-5、Gemini 2.5 Pro、DeepSeek V3、Qwen2.5、GLM-4.6 |
| Embedding | Voyage / OpenAI text-embedding-3-large | bge-m3、Qwen-embedding |
| Computer Use | Claude `computer-use-2025-11-24` | Gemini 2.5 CU、Fara1.5、AutoGLM-Web、UI-TARS-2 |
| 屏幕解析 | OmniParser v2（本地） | UI-TARS-1.5、Aria-UI |
| OCR | PaddleOCR 3.0（本地） | Surya、Marker、Tesseract、阿里 OCR API |
| Voice（可选） | ElevenLabs / OpenAI Realtime | Tencent ASR |

### 6.3 安全：Prompt Injection 防御

- 所有从外部 web/邮件/文档读入的文本，进入 LLM 前打 `<external_content>` 包装。
- 高风险动作（转账、删除、发送 IM、写注册表）需 `human_in_the_loop: true` 或 `policy: ask` 才能执行。
- LLM 看到的凭据全部以 `${{ token://xxx }}` 占位符代替，注入只发生在最终 Action 执行那一步。

---

## 7. 选择器引擎（详见 03）

四路 fallback：

```
       ┌──────────────┐
       │  CSS         │ ─┐
       └──────────────┘  │
       ┌──────────────┐  ├──→ Score & pick
       │  XPath       │ ─┤      best at runtime
       └──────────────┘  │
       ┌──────────────┐  │
       │  A11y path   │ ─┤
       └──────────────┘  │
       ┌──────────────┐  │
       │ Vision LLM   │ ─┘     (only when score < threshold)
       └──────────────┘
```

录制时同一元素**保存全部 4 套指纹**，运行时根据 Self-Healing Router 选最稳。

---

## 8. 数据模型（libSQL Schema）

> 单库 `lumo.db`（WAL 模式，可与 `lumo.db-wal` / `lumo.db-shm` 同盘）。

```sql
-- 流程定义（最终源是 YAML 文件；DB 仅缓存）
CREATE TABLE flows (
  id              TEXT PRIMARY KEY,             -- "order-export"
  version         TEXT NOT NULL,                -- "0.3.2"
  yaml            TEXT NOT NULL,                -- 原始 YAML
  hash            BLOB NOT NULL,                -- sha256
  created_at      INTEGER NOT NULL,
  updated_at      INTEGER NOT NULL,
  tags            TEXT NOT NULL DEFAULT '[]',   -- JSON array
  UNIQUE(id, version)
);

-- 一次运行
CREATE TABLE flow_runs (
  id              TEXT PRIMARY KEY,             -- ulid
  flow_id         TEXT NOT NULL,
  flow_version    TEXT NOT NULL,
  trigger_kind    TEXT NOT NULL,                -- cron|webhook|manual|mcp|chain
  inputs          TEXT NOT NULL,                -- JSON
  outputs         TEXT,                         -- JSON
  state           TEXT NOT NULL,                -- queued|running|ok|failed|cancelled
  worker_id       TEXT,
  started_at      INTEGER,
  finished_at     INTEGER,
  cost_token      INTEGER DEFAULT 0,
  cost_usd_micro  INTEGER DEFAULT 0,
  trace_id        TEXT,                         -- W3C trace-id
  FOREIGN KEY (flow_id, flow_version) REFERENCES flows(id, version)
);

-- step 级别 durability
CREATE TABLE step_runs (
  flow_run_id     TEXT NOT NULL,
  step_id         TEXT NOT NULL,
  idx             INTEGER NOT NULL,
  state           TEXT NOT NULL,
  attempt         INTEGER NOT NULL DEFAULT 1,
  input_hash      BLOB NOT NULL,
  output_json     TEXT,
  error_json      TEXT,
  started_at      INTEGER,
  finished_at     INTEGER,
  span_id         TEXT,
  PRIMARY KEY (flow_run_id, step_id, attempt)
);

-- 截图/DOM/HAR 等工件
CREATE TABLE artifacts (
  id              TEXT PRIMARY KEY,             -- ulid
  flow_run_id     TEXT NOT NULL,
  step_id         TEXT,
  kind            TEXT NOT NULL,                -- screenshot|dom|har|video|file
  mime            TEXT NOT NULL,
  size            INTEGER NOT NULL,
  blob_path       TEXT NOT NULL,                -- 本地相对路径或 s3:// URI
  sha256          BLOB NOT NULL,
  created_at      INTEGER NOT NULL
);

-- 元素拾取库（同一元素的 4 套指纹）
CREATE TABLE elements (
  id              TEXT PRIMARY KEY,
  flow_id         TEXT NOT NULL,
  alias           TEXT NOT NULL,                -- "登录按钮"
  css             TEXT,
  xpath           TEXT,
  a11y_path       TEXT,
  visual_anchor   TEXT,                         -- 截图区域 hash + bbox
  ocr_text        TEXT,
  embeddings      F32_BLOB(768),                -- libSQL vector
  health_score    REAL NOT NULL DEFAULT 1.0,
  last_seen_at    INTEGER,
  UNIQUE(flow_id, alias)
);

-- 凭据 Vault（age 加密）
CREATE TABLE vault_items (
  name            TEXT PRIMARY KEY,
  age_ciphertext  BLOB NOT NULL,
  metadata        TEXT NOT NULL,                -- JSON: rotation, owner, scopes
  updated_at      INTEGER NOT NULL
);

-- Triggers / Queue / Schedules
CREATE TABLE triggers (
  id              TEXT PRIMARY KEY,
  flow_id         TEXT NOT NULL,
  kind            TEXT NOT NULL,
  spec_json       TEXT NOT NULL,
  enabled         INTEGER NOT NULL DEFAULT 1,
  last_fired_at   INTEGER
);

CREATE TABLE queue (
  id              INTEGER PRIMARY KEY AUTOINCREMENT,
  topic           TEXT NOT NULL,
  payload         TEXT NOT NULL,
  priority        INTEGER NOT NULL DEFAULT 5,
  available_at    INTEGER NOT NULL,
  attempts        INTEGER NOT NULL DEFAULT 0,
  visible_until   INTEGER,
  done_at         INTEGER
);

CREATE INDEX idx_queue_topic_avail ON queue(topic, available_at) WHERE done_at IS NULL;

-- AI Router 缓存
CREATE TABLE ai_cache (
  prompt_hash     BLOB PRIMARY KEY,
  model           TEXT NOT NULL,
  response        TEXT NOT NULL,
  expires_at      INTEGER
);

-- RAG / 知识库（向量列）
CREATE TABLE kb_chunks (
  id              TEXT PRIMARY KEY,
  kb_id           TEXT NOT NULL,
  text            TEXT NOT NULL,
  embedding       F32_BLOB(1024),               -- libSQL native vector
  metadata        TEXT
);
CREATE INDEX idx_kb_emb ON kb_chunks(libsql_vector_idx(embedding));
```

---

## 9. 录制器架构（详见 03）

```
                       ┌──────────────────┐
                       │  Recorder UI     │
                       │ (Studio panel)   │
                       └──────────┬───────┘
                                  │ subscribe
       Browser tab               ▼
       ┌──────────┐         ┌──────────────────┐         Desktop OS
       │ Chromium │         │  Recorder Engine  │         ┌──────────┐
       │  CDP     │◀────────┤  (Rust)            ├────────▶│ Hook svc │
       └──────────┘  events │                    │         └──────────┘
                            │  ActionBuffer      │
                            │   debounce/merge   │
                            │  AccessKit snap    │
                            │  ImgFP snap        │
                            └──────────┬─────────┘
                                       │ canonicalize
                                       ▼
                            ┌─────────────────────┐
                            │  Step DSL (LumoFlow)│
                            └─────────────────────┘
```

- 浏览器源 = CDP `Input.*` + `DOM.*` + `Page.*` 事件
- 桌面源 = Win hook（low-level mouse/keyboard）/ macOS CGEventTap / Linux libinput
- 每个事件实时打元素指纹（CSS / XPath / A11y / 截图 bbox）
- ActionBuffer 做去抖（200ms 滑动窗口）合并连续 type、连续 click

---

## 10. 沙箱与凭据

### 10.1 沙箱
| 平台 | 实现 | 关键 |
|---|---|---|
| Windows | `CreateProcessAsUser` → 独立低权限 user + Job Object 限制 CPU/内存/Handle | 多 worker 互不抢焦点 |
| macOS | `sandbox-exec` SBPL profile，按 capability 生成 | Apple notarization |
| Linux | Firejail profile + Landlock + seccomp-bpf | 适配 UOS/麒麟 |
| 信创/容器 | Podman rootless + capability 投影 | 数据中心 |

### 10.2 凭据 Vault
- 落盘前用 `age` 加密；密钥保存于平台 KeyStore（Win DPAPI / macOS Keychain / libsecret / TPM）。
- 控制台版可选 HSM / KMS。
- **JIT 注入**：模板渲染发生在 Action.execute() 内部，绝不落入 step 输入快照、OTel attr、LLM prompt。
- 高风险 Action（转账/SQL DELETE/邮件群发）必须 `policy: ask` 走 Action Center 审批。

---

## 11. 可观测性（OpenTelemetry GenAI）

- 一个 `flow_run` = 一个 OTel **trace**。
- 每个 `step_run` = 一个 **span**。
- AI 节点 span attributes 遵循 OTel GenAI semconv：

```
gen_ai.system = "anthropic"
gen_ai.request.model = "claude-opus-4-7"
gen_ai.usage.input_tokens = 1024
gen_ai.usage.output_tokens = 256
gen_ai.operation.name = "chat" | "computer_use" | "embed"
lumorpa.step.id = "open-orders"
lumorpa.selector.strategy = "css" | "xpath" | "a11y" | "vision-llm"
lumorpa.selector.healed = true|false
```

- 默认 exporter：本地 SQLite file + 内置 Trace Viewer。
- 可选 exporter：OTLP gRPC → Grafana Tempo / Jaeger / Langfuse / OpenLLMetry。
- 每 step 附 artifact（截图/DOM）→ Trace Viewer 时间线上显示缩略图。

---

## 12. 跨进程协议

| 通道 | 协议 | 用途 |
|---|---|---|
| Studio ↔ Core（同进程） | Tauri Channel + 共享内存 | UI ↔ Rust |
| Studio ↔ Core（远程模式） | gRPC + mTLS | UI 远程连接 |
| Worker ↔ Cloud | gRPC + mTLS + bidi stream（heartbeat、event push） | 集群 |
| Webhook 入 | HTTPS + HMAC | 外部触发 |
| MCP Server | JSON-RPC 2.0 over stdio / SSE / HTTP | LLM 调度 |
| MCP Client | 同上 | 调外部工具 |

`.proto` 关键片段（节选）：

```proto
service LumoOrchestrator {
  rpc Register(WorkerInfo) returns (RegisterAck);
  rpc Heartbeat(stream HB) returns (stream OrchEvent);
  rpc Dispatch(stream WorkerEvent) returns (stream Job);
  rpc ReportArtifact(stream ArtifactChunk) returns (ArtifactAck);
}
```

---

## 13. 部署形态

### 13.1 本地

```bash
# 一行启动
brew install lumorpa            # 或 winget / apt / dnf / pacman
lumo serve                       # 启动本地后台 + 控制台 (127.0.0.1:7891)
open lumo://studio
```

### 13.2 自托管控制台

```yaml
# docker-compose.yml
services:
  lumo-cloud:
    image: ghcr.io/lumorpa/cloud:0.1
    ports: ["7890:7890"]
    environment:
      LUMO_DB: "libsql://db:8080"
      LUMO_S3: "http://minio:9000"
  db:
    image: ghcr.io/tursodatabase/libsql:latest
  minio:
    image: minio/minio
    command: server /data
  # Worker 可在任意机器，gRPC 注册到 lumo-cloud
```

### 13.3 信创

- Rust + Tauri 跨平台天然支持 UOS/麒麟（aarch64/loongarch64/mips64）。
- 构建矩阵：linux/{amd64,arm64,loong64} + windows/amd64 + macos/{amd64,arm64}。
- 数据库可切达梦/人大金仓（适配层走 sqlx）。

---

## 14. 安全模型（STRIDE 摘要）

| 威胁 | 缓解 |
|---|---|
| Spoofing：仿冒 Worker | mTLS 双向证书 |
| Tampering：流程包被改 | Cosign 签名校验，签名失败拒绝加载 |
| Repudiation：操作抵赖 | Append-only 审计日志（hash chain） |
| Info Disclosure：凭据泄漏 | Vault + JIT + LLM 不可见 + 截图脱敏 |
| DoS：插件死循环 | Job Object/cgroups 限制 CPU/RAM + 超时 |
| Elevation：插件越权 | Capability 显式声明 + 沙箱拒绝 |

---

## 15. 性能预算

| 指标 | 目标 |
|---|---|
| Studio 冷启动 | < 1.5s |
| 内核常驻 RAM（空载） | < 80 MB |
| 单 step 调度开销 | < 1 ms |
| 浏览器 click + screenshot | < 80 ms（headless），< 200 ms（headed） |
| Excel 10w 行 Polars 读 | < 1.2 s |
| OmniParser v2 单帧（本地 GPU） | < 0.6 s |
| Self-Healing Router 决策 | < 0.5 ms |
| OTel span 落盘 | < 0.2 ms |

---

## 16. 开放性

- 协议：MCP、OpenAPI、OTel、CDP、libSQL 全标准化。
- 包格式：`.lumoflow` = zip(YAML+assets+lock)，可手工解包；插件 = OCI artifact。
- 协议：主仓 Apache-2.0；插件可选 Commercial Source-Available。
- 文档：中英双语；命令行/UI/LSP 全 i18n。

---

下一篇 → [03 · 关键子系统详设](./03-Subsystems-Deep-Dive.md)
