# LumoRPA

<div align="center">
  <p><strong>流程即代码的本地优先 RPA 运行时</strong></p>
  <p>LumoFlow YAML · Rust 执行内核 · 确定性动作 · BYOK AI 路由</p>

  <p>
    <a href="./LICENSE"><img src="https://img.shields.io/badge/License-Apache--2.0-blue?logo=apache&logoColor=white" alt="License: Apache-2.0"></a>
    <a href="https://www.rust-lang.org/"><img src="https://img.shields.io/badge/Rust-1.88%2B-000000?logo=rust&logoColor=white" alt="Rust 1.88+"></a>
    <a href="https://v2.tauri.app/"><img src="https://img.shields.io/badge/Tauri-2-FFC131?logo=tauri&logoColor=black" alt="Tauri 2"></a>
    <a href="https://nodejs.org/"><img src="https://img.shields.io/badge/Node-22-339933?logo=nodedotjs&logoColor=white" alt="Node 22"></a>
    <a href="./.github/workflows/ci.yml"><img src="https://github.com/Lumonote/LumoRPA/actions/workflows/ci.yml/badge.svg" alt="CI"></a>
    <a href="./.github/workflows/release.yml"><img src="https://github.com/Lumonote/LumoRPA/actions/workflows/release.yml/badge.svg" alt="Release"></a>
  </p>
</div>

> **可解释、可复现、可离线。** 每条指令要么确定性执行，要么显式声明它依赖外部（模型 / 网络 / 宿主 UI），并在缺少该依赖时**明确失败**——绝不静默产出空值或近似值。

**验证判据**：同一份流程 + 同一份输入，在断网环境下要么产出相同结果，要么给出可读的失败原因。

LumoRPA 把自动化流程当成**可版本控制的源码**而不是二进制画布：流程是 LumoFlow YAML（`apiVersion: lumorpa.io/v1`），由 Rust 编写的步骤机执行，动作走统一的 capability 门禁。同一份流程既能在 CLI 里 headless 跑（CI 可用），也能在 Tauri 桌面工作台里录制、编辑、运行、检视。

## 为什么使用 LumoRPA

- **流程即代码**：YAML 是唯一契约，diff 可读、可 review、可回滚。不绑定某个 GUI 的二进制工程文件。
- **本地优先**：默认不发起任何网络调用；状态只落本机 SQLite。缺能力就报错，不用别的东西静默模拟。
- **确定性动作面**：262 条已注册动作按 capability 归类，浏览器（CDP）、桌面 UI、Excel、PDF/DOCX、HTTP、数据库、邮件、MCP 全覆盖。
- **失败优先于静默**：无宿主 / 无依赖 / 不可达时返回可读错误，绝不 panic、绝不静默返回空值——静默空值会让流程带着错误数据继续跑，比直接失败难排查得多。
- **断网可交付**：纯 Rust 依赖，无 C 依赖包袱，交叉编译与信创（麒麟 / 龙芯）目标友好。
- **能力边界写在文档里**：`docs/06` 用实测数字回答「成熟商业 RPA 的指令，我覆盖到哪一步了」，缺口分 A/B/C 三级，不靠推测冒充已覆盖。

## 核心能力

| 模块 | 能力 |
| --- | --- |
| LumoFlow 契约 | YAML 解析、模板渲染、结构校验、capability 门禁与 linter（含 `on_error.silent_bind`） |
| 执行内核 | `lumo-core` 步骤机：重试、失败终局处置（`on_error`）、取消传播、分支合流、`for_each` 限流 |
| 动作族 | 控制流、本地数据整形、文件、HTTP、Excel、浏览器、PDF/DOCX、邮件、数据库、MCP、AI chat、human-in-the-loop、技能调用 |
| 文档解析 | `lumo-doc` 纯 Rust 解析引擎（DocIR）：DOCX / XLSX / PDF / OCR |
| 录制 | `lumo-recorder` 浏览器（Chromium CDP）与桌面（a11y）双通道，选择器打分 |
| 技能 | Claude 风格 `SKILL.md` 加载 + `skill.invoke` 子流程（含递归深度护栏） |
| 运行历史 | SQLite 持久化，含控制流与循环迭代的嵌套路径 |
| AI 路由 | BYOK 多 provider，流式 / 非流式，视觉与 OCR 模型分列默认值 |
| 调度 | cron / 文件监听 / webhook / hotkey，可安装为登录常驻服务 |
| 接口 | `lumo` CLI（headless、CI 可用） + Tauri 桌面工作台 |
| 环境体检 | `lumo doctor` 一条命令回答「这台机器上哪些流程能力能跑」 |

## 快速开始

### 1. 从源码构建

环境要求：Rust stable（MSRV 1.88，见 `rust-toolchain.toml`），以及浏览器动作所需的 Chromium 兼容浏览器。

```bash
cargo build --workspace
cargo test --workspace --all-targets

# 校验并运行第一条流程
cargo run -p lumo-cli -- validate examples/hello-world.lumoflow.yaml
cargo run -p lumo-cli -- run --no-store examples/hello-world.lumoflow.yaml
```

运行带本地示例技能的流程：

```bash
cargo run -p lumo-cli -- run --no-store examples/skill-driver.lumoflow.yaml
```

CLI 从 `$LUMO_HOME/skills` 加载已装技能，也会加载流程文件同级的 `skills/` 目录。

> 沙箱或受限环境下 `cargo` 可能无法写入默认 registry 缓存，把 `CARGO_HOME` 重定向到工作区即可：`export CARGO_HOME=$PWD/.cargo-home`（该目录已在 `.gitignore` 中）。

### 2. 安装包与发布流程

**每次推送到 `main` 都会自动构建可安装的安装包**，不需要打 tag：Windows（NSIS `.exe` 安装器 **和** `.msi`）、macOS（universal `.dmg`，Intel + Apple Silicon 同一个文件）、Linux 包，外加各平台的 CLI 归档。推 `v*` tag 则发布带签名的正式 GitHub Release。完整流程见 [`docs/release.md`](./docs/release.md)。

| 平台 | 桌面 | CLI 归档 |
| --- | --- | --- |
| Windows x86_64 | `.exe`（NSIS）+ `.msi` | `…-windows-x86_64.zip` |
| macOS universal（Intel + ARM） | `.dmg`（单文件通用） | `…-darwin-universal.tar.gz` |
| Linux x86_64 | `.deb` / `.rpm` / `.AppImage` | `…-linux-x86_64.tar.gz` |
| Linux aarch64（麒麟 ARM） | `.deb` / `.rpm` | `…-linux-aarch64.tar.gz` |
| loongarch64（龙芯） | — | `…-linux-loongarch64.tar.gz` |

**去哪里下载**（按方便程度排序）：

1. **滚动预览版（固定链接，推荐）**：[github.com/Lumonote/LumoRPA/releases/tag/dev-latest](https://github.com/Lumonote/LumoRPA/releases/tag/dev-latest) —— Windows / macOS 安装包可以直接点。这个 tag **不是**正式发布，它每次 main 推送都会被重建覆盖，所以链接永远有效、内容永远是最新的。
2. **Workflow 运行页 → Artifacts**：[`release` workflow](https://github.com/Lumonote/LumoRPA/actions/workflows/release.yml) 最近一次 main 运行，在页面底部 `Artifacts` 区下载。**仅保留 14 天**，过期就没了。
3. **CLI 用户**：`gh run download` 取最近一次 main 构建的产物：

   ```bash
   gh run download --repo Lumonote/LumoRPA \
     --branch main --workflow release.yml \
     --pattern 'lumorpa-*' --dir ./lumorpa-dev
   ```

> ⚠️ **`dev-latest` 是非官方、未签名的构建快照。** 它由 main 的最新提交自动生成，**没有代码签名、没有 Apple 公证**，macOS 会弹 Gatekeeper 警告、Windows 会弹 SmartScreen 警告——这是预期行为，不是包坏了。
>
> - **macOS**：在 Finder 里**右键（或按住 Control 点击）应用 → 打开**，再确认一次；直接双击会被拦下。也可以 `xattr -dr com.apple.quarantine /Applications/LumoRPA.app`（只在你信任该构建时用）。
> - **Windows**：SmartScreen 提示里点 **更多信息 → 仍要运行**。
>
> **需要受支持的构建，请用版本化的 `v*` tag 发布**（见下方「构建与发布」）；需要签名安装包就不要用 `dev-latest`。

> **打包装不了跨平台。** 每个 Tauri 目标都需要该 OS 原生的 WebKit / GTK，桌面安装包无法从 macOS 交叉构建；Linux 与 Windows 产物由 CI 在各自 runner 上构建。仓库内的 `dist/` 是历史的本地打包输出，不作为可复现来源。

### 3. 桌面工作台（开发模式）

```bash
cd apps/desktop
cargo tauri dev
```

打包需由构建脚本注入 updater 公钥：

```bash
scripts/build-desktop.sh                                                # macOS
scripts/build-desktop.sh x86_64-pc-windows-msvc nsis,msi                # Windows（在 Windows 构建机上）
scripts/build-desktop.sh x86_64-unknown-linux-gnu deb,rpm,appimage      # Linux / 麒麟 x86_64（在目标 Linux 构建机上）
scripts/build-desktop.sh aarch64-unknown-linux-gnu deb,rpm,appimage     # 麒麟 ARM64（在 aarch64 Linux 构建机上）
scripts/build-desktop.sh loongarch64-unknown-linux-gnu deb,rpm          # 麒麟 LoongArch（在 loongarch64 Linux 构建机上）
```

**Updater 产物。** `tauri.conf.json` 启用了 `createUpdaterArtifacts`，但不含 `plugins.updater` 块——公钥在运行时由 `LUMO_UPDATER_PUBKEY` 注入（`apps/desktop/src-tauri/src/lib.rs`）。bundler 需要同一把钥匙，因此 `scripts/build-desktop.sh` 会把它透传下去。与 `TAURI_SIGNING_PRIVATE_KEY` 一起导出才能得到带签名的 `.tar.gz` / `.sig`；不设置则产出无 updater 产物的普通安装包。裸跑 `cargo tauri build` 会在 bundling 阶段失败。

桌面端的打包边界与细节见 [`apps/desktop/README.md`](./apps/desktop/README.md)。

## 信创与交叉编译

Ubuntu runner 的 glibc 可能比目标麒麟 / 统信发行版更新，Ubuntu 构建出的二进制在较旧的 信创 目标上可能加载失败。生产用 信创 包必须在匹配的麒麟 / 龙芯镜像（或自建 runner）上原生构建。GitHub 没有 loongarch64 runner，其 CLI 为 best-effort 交叉编译，桌面包必须原生构建——把 Ubuntu 产物当作便利基线，而非认证过的 信创 交付物。

## 构建与发布

### 一次产出桌面包 + CLI 归档

发布需要**桌面安装包和 CLI 归档两份产物**。用统一入口一次做完：

```bash
scripts/package-release.sh                                      # 宿主平台
scripts/package-release.sh "" deb,rpm,appimage                  # Linux / 麒麟 x86_64
scripts/package-release.sh x86_64-pc-windows-msvc nsis,msi      # Windows（在 Windows 构建机上）
scripts/package-release.sh universal-apple-darwin app,dmg       # macOS universal
```

它按顺序调用下面两个脚本，共用一次依赖解析与同一个 target 目录，**任一半失败即整体失败**（不产出"只有安装包没有归档"的半成品）。只要有其中一半时用环境变量跳过另一半：

```bash
LUMO_SKIP_DESKTOP=1 scripts/package-release.sh   # 只要 CLI 归档
LUMO_SKIP_CLI=1     scripts/package-release.sh   # 只要桌面安装包
```

CI 的 macOS / Windows / Linux 三个平台走的都是这个入口。

### 单独打包 CLI

```bash
scripts/package-cli.sh                          # 宿主平台
scripts/package-cli.sh x86_64-unknown-linux-gnu # 交叉编译（先 rustup target add）
scripts/package-cli.sh universal-apple-darwin   # macOS fat binary（lipo 合并）
```

产出 `dist/lumorpa-<version>-<os>-<arch>.tar.gz`（Windows 为 `.zip`），内含 `bin/lumo`、`examples/`、`README.md` 与 `LICENSE`。loongarch64（龙芯）只出 CLI 归档，走这个脚本。

### 发布：`.github/workflows/release.yml`（两条通道）

[`release.yml`](./.github/workflows/release.yml) 是一个 workflow、两条发布通道：

| 触发 | 产出 | 签名 |
| --- | --- | --- |
| push `main` | 全平台**桌面安装包 + CLI 归档**，合并发布为滚动预发布 `dev-latest` | ❌ 未签名、未公证 |
| push `v*` tag | 真正的 GitHub Release，各平台各自附上产物 | ✅ 需要 Apple 签名 / 公证与 Tauri updater secrets，缺任一项**直接失败**（绝不放行未签名的「正式版」） |

```bash
# 通道二：正式发布
git tag v0.1.0
git push origin v0.1.0
```

main 通道在完整门禁（`production-gate`）通过后才会发布 `dev-latest`，且发布前断言合并产物里确有 Windows 安装包与 macOS `.dmg`——空产物宁可让流水线红，也不发一个装不了东西的包。也可在 Actions 页面选择 `release` → *Run workflow* 手动构建（手动触发走 main 通道语义，产物留在 Actions Artifacts）。

> tag 构建与 main 构建共用同一份打包脚本（`scripts/package-release.sh`），因此**能发布出来的东西，两条通道是一样的**；区别只在签名与保留期（正式 90 天 / dev 14 天）。

## 测试与质量检查

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --all-targets --no-fail-fast
```

前端（原生 ESM，无打包器）：

```bash
cd apps/desktop/frontend
node --test && npm run lint
```

CI（[`.github/workflows/ci.yml`](./.github/workflows/ci.yml)）分层把守：

| 作业 | 内容 |
| --- | --- |
| `build-test` | ubuntu / macOS / windows 三平台编译 + 全量测试 + CLI smoke |
| `fmt-clippy` | `cargo fmt --check` 与 `clippy -D warnings` |
| `coverage` | `cargo llvm-cov` 行覆盖门槛（浏览器 / 视觉 / 录制器不计入分母） |
| `cargo-deny` | 供应链：advisories（含 yank）、licenses、sources、bans |
| `msrv` | 在声明的 MSRV 1.88 上 `cargo check` |
| `cross-check` | aarch64 硬门禁；loongarch64 best-effort |
| `bundle-smoke` | 只打 CLI 归档，验证打包脚本没坏（不含桌面打包，那由 `release.yml` 在 main 上覆盖） |

> 先在本地把受影响模块的测试与静态检查跑绿再提 PR。CI 单次全量约需数分钟，本地能复现的失败不要留给 CI。

## 文档

| 文档 | 内容 |
| --- | --- |
| [`docs/README.md`](./docs/README.md) | 文档地图与阅读路径（**从这里开始**） |
| [`docs/00-LumoRPA-Master-Report.md`](./docs/00-LumoRPA-Master-Report.md) | 总报告：项目定位、能力全景、里程碑与验收口径 |
| [`docs/01-Product-Design.md`](./docs/01-Product-Design.md) | 产品设计：角色、场景、界面交互、录制—编辑—运行闭环 |
| [`docs/02-Architecture-Design.md`](./docs/02-Architecture-Design.md) | **架构权威规范**：分层、VM 执行模型、capability 门禁、存储与调度 |
| [`docs/03-Subsystems-Deep-Dive.md`](./docs/03-Subsystems-Deep-Dive.md) | 子系统深潜：动作族、录制、技能、AI 路由、语音 |
| [`docs/04-优化与补充开发-路线图.md`](./docs/04-优化与补充开发-路线图.md) | 路线图与补强清单 |
| [`docs/05-LumoFlow-Instruction-Set.md`](./docs/05-LumoFlow-Instruction-Set.md) | **LumoFlow 指令集规范**：YAML 结构、步骤字段、模板引用、动作清单 |
| [`docs/06-Capability-Coverage-Matrix.md`](./docs/06-Capability-Coverage-Matrix.md) | **能力覆盖矩阵**：对表进度、A/B/C 缺口分级、实测数字 |
| [`docs/release.md`](./docs/release.md) | **安装包从哪来**：main / tag 两条通道、产物清单、未签名包的处理 |
| [`apps/desktop/README.md`](./apps/desktop/README.md) | 桌面打包边界与构建细节 |

## 仓库结构

| 路径 | 内容 |
| --- | --- |
| `crates/lumo-dsl/` | LumoFlow AST、解析器、模板渲染、校验与 linter |
| `crates/lumo-core/` | Action trait、注册表、StepCtx、Flow 步骤机 |
| `crates/lumo-actions/` | 内置确定性动作族（含 capability 门禁） |
| `crates/lumo-doc/` | 纯 Rust 文档解析引擎（DocIR：DOCX / XLSX / PDF / OCR） |
| `crates/lumo-ai/` | Provider 配置、AI 路由、`ai.chat` |
| `crates/lumo-skills/` | `SKILL.md` 加载与 `skill.invoke` |
| `crates/lumo-storage/` | SQLite schema 与仓储层 |
| `crates/lumo-recorder/` | 浏览器 CDP 录制（事件采集 + 选择器打分）与桌面 a11y 后端 |
| `crates/lumo-agent/` | 受监督的改进循环：评估、遥测、追踪挖掘、提案与信任边界 |
| `crates/lumo-voice/` | 语音能力：provider、模型安装、人格与快捷指令 |
| `crates/lumo-cli/` | `lumo` 命令行接口（另有 `doctor` / `service`） |
| `apps/desktop/` | Tauri 桌面工作台：`src-tauri/` 后端与 `frontend/` 原生 ESM 前端 |
| `examples/` | 可直接运行的流程示例与示例技能 |
| `docs/` | 中文设计文档：架构、指令集、覆盖矩阵与专题规格 |
| `scripts/` | 打包与构建脚本（`package-release.sh` 统一入口，及 `package-cli.sh` / `build-desktop.sh`） |
| `dist/` | 打包输出目录（**非可复现来源**，发布以 CI 产物为准） |

## 运行时安全

流程在运行时做 capability 检查。触碰文件、HTTP / 浏览器网络或 LLM 的动作需要 `spec.capabilities` 中对应的声明：

```yaml
spec:
  capabilities:
    fs.read: ["./examples/data/**"]
    fs.write: ["/tmp/**"]
    network: ["example.com"]
    llm: ["*"]
```

**双体系门禁**：静态的 capability YAML 与运行时的 `LUMO_ALLOW_*` 环境开关相互独立，前者管「这份流程声明要什么」，后者管「这台机器允许什么」。静态校验看不到环境开关状态——这正是 `lumo doctor` 存在的理由。

AI 动作默认关闭。使用 `ai.chat`、云端 `image.ocr` 或 AI hook 模式的流程需先设 `LUMO_ALLOW_LLM_NETWORK=1`。

Vault 占位符在模板渲染期间保留，在动作执行前从环境变量解析。声明 `vault: [smtp]` 后可用 `{{ vault.smtp.password }}`，由环境变量 `LUMO_VAULT_SMTP_PASSWORD` 提供。

## AI Provider

```bash
cargo run -p lumo-cli -- providers init
cargo run -p lumo-cli -- providers list
```

每个 provider 可设 `default_model`，并为截图定位与 OCR 分别设 `vision_model` 与 `ocr_model`。

本地 OCR 使用内置 `local-ocr` profile，把 `ocr_model` 指向 ModelScope 预设（如 `modelscope/ZhipuAI/GLM-OCR`、`modelscope/PaddlePaddle/PaddleOCR-VL-1.6`）。桌面的 Models 页可用 ModelScope CLI 下载受支持的 OCR 预设（`pip install modelscope`），本地推理随后走配置的 Python 运行时（`LUMO_OCR_PYTHON`，默认 `python3`）。

## 常用命令

```bash
cargo run -p lumo-cli -- actions                  # 列出全部已注册动作
cargo run -p lumo-cli -- actions --show ai.chat   # 查看某动作的 schema
cargo run -p lumo-cli -- doctor                   # 环境体检：这台机器能跑哪些能力
cargo run -p lumo-cli -- skills list
cargo run -p lumo-cli -- runs list
cargo run -p lumo-cli -- runs show <run_id> --outputs
cargo run -p lumo-cli -- service install          # 把调度守护装成登录常驻服务
```

## 贡献

欢迎提交 Issue、改进文档和 Pull Request。动手前请先读 [`docs/README.md`](./docs/README.md)（文档地图）与 [`docs/02-Architecture-Design.md`](./docs/02-Architecture-Design.md)（架构权威规范），并注意以下硬约束：

1. **动作契约统一。** 任何动作实现 `Action`（`id` / `summary` / `schema` / `execute`），输入结构体一律 `deny_unknown_fields`。
2. **capability 零漂移。** 每个注册动作必须落在 `ACTION_CAPS` 或 `NO_CAP_ACTIONS` 二者之一，由测试守着；`ACTION_CAPS` 必须严格字典序。
3. **失败优先于静默。** 无宿主 / 无依赖 / 不可达时返回可读错误，绝不 panic，绝不静默返回空值。
4. **缺省行为不漂移。** 新增字段一律 `#[serde(default)]`，未配置的既有流程执行路径不变。
5. **诚实标注。** 语义与外部产品不等价时，把差异写进返回值或文档，不用近似实现冒充。
6. **文档随代码走。** 改了动作表 / 步骤字段 / 能力分级，必须同步更新 `docs/05` / `docs/06`，并重跑文档一致性测试（`crates/lumo-cli/tests/instruction_set_docs.rs` 会守 `05` 的动作清单区块）。

请保持改动聚焦（一个 PR 只做一件事），在描述里给出复现步骤与验证结果。

## 许可

本项目以 [Apache License 2.0](./LICENSE) 发布。

## 免责声明

LumoRPA 是开源软件，本身**不提供**任何模型服务、数据源或云资源。使用 AI 动作需自行准备 LLM Provider 凭据并遵守其服务条款；模型输出可能不准确，请人工核验后再用于生产。涉及浏览器 / 桌面 UI 自动化的流程可能受目标软件的服务条款约束，部署方需自行负责凭据保管、数据合规与访问审计。本项目不构成任何形式的商业承诺或专业建议。
