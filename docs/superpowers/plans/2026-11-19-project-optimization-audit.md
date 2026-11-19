# 项目优化复核与开发优先级

日期：2026-11-19。基线：`dev` / `367b91945dd47e139fba6e52f11eb07cb25f1958`。

本轮分析代码、发布配置和既有验收记录，并执行小型逻辑复现；未修改业务代码，未操作真实业务任务，未验证远端最新 CI 或已发布安装包。2026-11-10 的 323/323 前端测试结果是历史基线，不等于本轮重新完成的原生端到端验收。

## 建议决策

优先把编辑正确性、结果归属和安装升级做完整。当前有 6 类可复现的编辑/结果缺陷；此外有发布配置断点、可测量的规模性能热点，以及复杂画布的能力缺口。单纯继续增加动作数量或调整颜色，不能解决这些操作中断。

| 顺序 | 工作包 | 影响 | 工作量估计 |
| --- | --- | --- | --- |
| 1 | 历史结果竞态、标量输出、嵌套单步回显 | 结果可能属于另一任务、缺失或显示错误状态 | 中 |
| 2 | 节点重命名、整组选区复制、行内 YAML 粘贴 | 编辑会破坏数据引用，或成功提示与实际不符 | 中 |
| 3 | 正式发布门禁、macOS 签名公证、安装后更新 | 安装交付与后续升级缺少完整保证 | 中至大 |
| 4 | 结果面板双向联动、分组折叠、节点搜索与缩略图 | 减少查找、切换和复杂图的视觉负担 | 中至大 |
| 5 | 类型化输出、字段级连线、布局持久化 | 让复杂数据流可视化编辑并可跨机器恢复 | 大 |
| 6 | 状态增量刷新、结果分页、规模与原生 E2E | 保证大流程和真实桌面环境中的可用性 | 中至大 |

工作量是相对范围，不是交付时间承诺。发布修复可与编辑器修复并行；关键回归测试应与对应修复一起完成。

## A. 已复现的正确性问题

### A1. 历史运行快速切换会混合两个任务的结果（P1）

**触发：** 点击历史运行 A，再点击 B；A 的费用请求晚返回。

真实 `runs.js` 配合最小 DOM/IPC 模拟得到：`state.activeRun=B`、`activeRunSteps=step-B`，但 `runDetail` 展示 `flow-A/step-A`；点击表格节点后展示 B 的输出。用户无法可靠判断看到的是哪一次运行。

**原因：** [runs.js:488](../../../apps/desktop/frontend/src/js/runs.js#L488) 的 `showHistoricalRun` 在多次 await 之间没有请求版本检查，最终表格的节点检视又从共享状态读取步骤（[runs.js:560](../../../apps/desktop/frontend/src/js/runs.js#L560)）。

**修复与验收：** 为整次加载建立请求 token；每次 await 后检查；表格闭包绑定该次 `detail.steps`。测试 A/B 任意回包顺序，断言选中行、标题、步骤、费用、截图与输出始终来自同一 runId。

### A2. 节点重命名未迁移结果引用（P2）

**触发：** `read → save` 已绑定 `{{ steps.read.result }}`；在属性中把 `read` 改成 `read_new`。

按属性面板使用的变更函数复现：`save` 仍引用 `steps.read.result`，画布数据边从 1 条变为 0 条。

**原因：** [inspector.js:145](../../../apps/desktop/frontend/src/js/editor/inspector.js#L145) 仅替换当前节点映射，没有重写引用。稳定布局键、断点也依赖 ID，重命名应一起处理。

**修复与验收：** 原子化重命名：解析引用并迁移参数、条件、输出、布局与断点；保留普通文本，不做全局字符串替换；动态或歧义引用明确提示。覆盖点号/下标表达式、带连字符 ID、嵌套节点，以及一次撤销恢复全部关联。

### A3. 复制粘贴没有保留选中子图的内部依赖（P2）

**触发一：** 同时复制 `read` 和 `save`。

实际得到 `read_c1`、`save_c1`，但 `save_c1` 仍引用原 `read`。每个 block 分别生成 ID 映射，不能把整个选区的依赖一起迁移，见 [yaml.js:563](../../../apps/desktop/frontend/src/js/yaml.js#L563)。

**触发二：** 复制 `- {id: item, action: control.log, with: {message: hello}}`。

行内 YAML 可提取到剪贴板，但 [yaml.js:546](../../../apps/desktop/frontend/src/js/yaml.js#L546) 的顶部 ID 正则不支持此格式，粘贴后源码完全不变。界面仍按剪贴板条数提示成功，见 [mutations.js:169](../../../apps/desktop/frontend/src/js/editor/mutations.js#L169)。

**修复与验收：** 先解析整个选区，统一分配 ID，再重写内部引用；外部引用保留原目标；按实际插入数量报告结果。覆盖跨流程粘贴、嵌套选区去重、行内 YAML、带连字符 ID、撤销/重做。

### A4. 嵌套节点单独运行成功仍显示“待执行”（P2）

后端 [lib.rs:623](../../../apps/desktop/src-tauri/src/lib.rs#L623) 把选中节点提取成顶层流程；运行路径变为 `child`，画布节点路径仍为 `gate/do/child`。

将该响应形态输入真实 [getGraphRunState](../../../apps/desktop/frontend/src/js/editor/graph-run-state.js#L99) 后得到 `idle/待执行`。此为路径映射复现，本轮未启动原生单步运行。

**修复与验收：** 单步上下文保存原节点路径和本次执行根路径，按此次运行映射子路径；仍保持 flow/source/runId 隔离。覆盖 if/try/parallel/loop 子节点、单跑控制子树及源码变更后的状态失效。

### A5. 合法的 false、0、空字符串输出被隐藏（P2）

[runs.js:438](../../../apps/desktop/frontend/src/js/runs.js#L438) 与 [runs.js:587](../../../apps/desktop/frontend/src/js/runs.js#L587) 使用输出值的真值判断是否渲染。真实 `file.exists` 的 [file.rs:150](../../../crates/lumo-actions/src/file.rs#L150) 在文件不存在时返回 `false`。

真实 `applyRunResponse` 配合最小 DOM 模拟，输入 `outputJson:false` 后没有输出区。

**修复与验收：** 以存在性判断结果，区分“无记录”“JSON null”和标量值；字符串用明确的字符串显示方式。时间线与历史详情同时覆盖 false、0、空字符串、null、对象和数组。

### A6. 长流程的“适应画布”仍会裁掉两端（P2）

使用真实模型与 [fitGraphBounds](../../../apps/desktop/frontend/src/js/editor/graph-viewport.js#L48)：200 个顺序节点、1000×900 视口得到最小缩放 2%，屏幕范围为 `x=-211.04…1211.04`，无法完整进入视口。

**修复与验收：** 适应范围计算与手动缩放下限分开；极小比例只展示概览形状；通过搜索/缩略图恢复到可读区域。验证 80/200/1000 节点和负坐标节点的所有边界。单纯继续缩小会让文字不可读，应与分组和导航一起完成。

## B. 性能与数据规模

### B1. 每次运行事件都扫描所有节点和所有执行状态

[runs.js:51](../../../apps/desktop/frontend/src/js/runs.js#L51) 每条步骤事件调用全图刷新；[graph.js:87](../../../apps/desktop/frontend/src/js/editor/graph.js#L87) 遍历每个节点；每个节点又在 [graph-run-state.js:106](../../../apps/desktop/frontend/src/js/editor/graph-run-state.js#L106) 扫描全部步骤状态。复杂度为每次刷新 O(节点数 × 执行状态数)，循环次数会进一步放大后者。

本机 Node 纯函数基准：每个节点对应一个完成状态，先预热一次，再取 5 次中位数；不包含 DOM、SVG 或原生 WebView 开销。

| 节点/状态数 | 一次全图状态计算中位数 |
| --- | ---: |
| 80 / 80 | 0.59 ms |
| 300 / 300 | 6.99 ms |
| 1000 / 1000 | 81.46 ms |

这是已测量的逻辑热点，不能直接当作浏览器帧率。建议按节点路径建立索引、循环状态增量聚合、同一帧合并事件，且只更新受影响节点。拖动几何更新也应改为受影响节点及相邻边；当前 [graph.js:59](../../../apps/desktop/frontend/src/js/editor/graph.js#L59) 每帧遍历全部节点和边。

### B2. 大结果应分页并按需加载

[repo.rs:196](../../../crates/lumo-storage/src/repo.rs#L196) 一次读取全部步骤，并为缺失变量快照逐行克隆前一快照；[runs.js:536](../../../apps/desktop/frontend/src/js/runs.js#L536) 一次构造所有历史步骤行。结果区还会完整格式化 JSON。

这是一项由代码路径确认的规模风险，本轮没有执行真实大表/长循环内存压测。建议步骤游标分页、结果摘要与完整 payload 分离、大结果按需展开、时间线虚拟化。历史列表目前固定取 60 条（[runs.js:464](../../../apps/desktop/frontend/src/js/runs.js#L464)），应增加分页/检索；桌面设置也可接入 CLI 已具备的保留策略。

### B3. 布局应成为可迁移的项目数据

[graph.js:29](../../../apps/desktop/frontend/src/js/editor/graph.js#L29) 仅写 localStorage，失败被静默忽略；[history.js:20](../../../apps/desktop/frontend/src/js/editor/history.js#L20) 每次编辑复制所有流程的布局。流程数量增加后会扩大快照成本，换机器也不能恢复布局。

建议版本化项目布局 sidecar，按流程保存/迁移，明确保存失败提示；撤销仅存当前流程布局或操作差异。不要把显示坐标混入执行顺序语义。

## C. 画布能力与审美的下一阶段

### C1. 结果检视与画布节点双向联动

[selectStep](../../../apps/desktop/frontend/src/js/editor/inspector.js#L18) 更新属性，结果由独立的 [showStepDetail](../../../apps/desktop/frontend/src/js/runs.js#L411) 驱动。应让点击节点直接展示对应运行的输入、输出、错误、耗时和产物；点击时间线反向聚焦节点。循环实例提供选择器，旧结果显示所属运行与源码版本。

实现顺序：统一节点/执行实例身份 → 结果抽屉 → 文本/JSON/表格/图像预览 → 重跑入口。图片与文件通过现有 artifact 访问路径显示；每次预览要有大小限制。

### C2. 复杂图的视觉组织

建议按以下顺序推进：

1. **分组与控制子树折叠：** 标题、颜色、运行汇总；折叠时保留边界连接和错误提示。
2. **节点搜索与缩略图：** 搜索 ID/动作/参数并聚焦，支持返回原视口；概览显示当前视野和失败节点。
3. **连线整理：** 中继点、吸附对齐、分支避让；选中节点时强化相关连接，降低无关信息干扰。
4. **缩放分层显示：** 大比例展示参数和结果，小比例仅保留标题/状态/端口概要；避免缩小后所有文本挤成噪声。

审美验收应使用真实复杂分支和长流程，检查信息层级、连线可追踪性、文本可读性、键盘焦点以及深浅主题，而不只验收三节点截图。

### C3. 类型化端口与字段级绑定

当前 [Action trait](../../../crates/lumo-core/src/action.rs#L38) 提供 `with` 输入 Schema，没有统一的动作输出 Schema；画布输出统一是 `result`（[graph-model.js:146](../../../apps/desktop/frontend/src/js/editor/graph-model.js#L146)）。[graph-connections.js:29](../../../apps/desktop/frontend/src/js/editor/graph-connections.js#L29) 检查顺序和作用域，没有输入/输出类型匹配。

建议先为动作目录增加输出结构契约，兼容未声明输出类型的旧动作；再提供 `result.rows` 等字段选择、兼容端口高亮与冲突解释。跨分支结果绑定需定义运行时可见范围和汇合语义，不能仅在界面上放开连线。

## D. 交付配置与验收缺口

### D1. macOS 签名凭据没有进入实际打包任务（P1）

[release.yml:104](../../../.github/workflows/release.yml#L104) 只在前置 job 检查 Apple secrets 非空；[实际打包步骤:149](../../../.github/workflows/release.yml#L149) 仅传 Tauri updater 变量。仓库发布脚本也未找到证书导入步骤。

应在 macOS 打包 job 导入证书并提供签名/公证配置，完成后校验最终产物。当前配置无法建立“前置检查通过即产物已签名公证”的保证；本轮未检查已发布安装包，不能据此断言已有成品状态。

### D2. 安装后的自动更新配置不完整（P1）

[desktop_update_commands.rs:37](../../../apps/desktop/src-tauri/src/desktop_update_commands.rs#L37) 从进程环境读取 endpoint，[第54行](../../../apps/desktop/src-tauri/src/desktop_update_commands.rs#L54) 读取公钥；[lib.rs:1289](../../../apps/desktop/src-tauri/src/lib.rs#L1289) 同样使用运行期环境。普通安装包在未设置这些变量时判定未配置。构建时注入公钥不等于启动时有环境变量。

建议将公开的更新地址、公钥和渠道配置固化进安装包，环境变量作为覆盖；发布流程生成版本清单，验收安装旧版→检查→下载→验证→重启→新版本。仓库内未找到清单生成发布步骤，是否有仓库外部署服务本轮未确认。

### D3. 正式发布与完整 CI 不是同一条门禁链（P1）

`v*` 独立触发 [release.yml:42](../../../.github/workflows/release.yml#L42)，主 [ci.yml:3](../../../.github/workflows/ci.yml#L3) 不响应 tag。release 前置 job 运行部分包测试，各平台只依赖此 job，随后直接上传正式发布附件，没有依赖同一 SHA 的完整核心/actions/原生验收成功。

建议使用可复用验证 workflow，确保同一 SHA 通过需要的检查，再统一发布齐全的平台产物。是否配置了仓库外审批或分支保护，本轮未检查，不能把它们假定为现有兜底。

### D4. 原生画布与桌宠 E2E 未常态覆盖

[desktop-e2e.py:228](../../../scripts/desktop-e2e.py#L228) 进入代码视图编辑执行，后续覆盖 Mission Control，但未覆盖画布连线/拖动或桌宠跨窗口任务。现有画布截图[明确使用模拟接口](../../testing/reports/2026-11-10-canvas/README.md#L5)。

下一批原生用例至少包括：图上添加→连线→改名→复制→撤销→保存→执行→选择节点查看结果；桌宠提交→主窗审批→进度→结果→取消/重试。同时验收异步乱序、切换流程与退出恢复。

主 CI 的 PR 目标也应把 `dev` 加入（[ci.yml:6](../../../.github/workflows/ci.yml#L6)）；当前 dev PR 不触发前端单测/lint和主 Rust 检查，合并后的 push 才执行。原生验收 workflow 已包含 dev PR。

2026-11-10 文档记录过 Linux Tauri/录制失败，本轮没有读取最新远端运行结果，因此仅将其作为需补充当前验收证据的历史记录。

## 开发完成标准

- 编辑不会静默改变引用目标，复制后的子图读自己的内部数据，撤销可整体恢复。
- 每个显示的状态、表格、截图和输出都有一致的 runId 与节点身份。
- 80/300/1000 节点基准包含状态更新、拖动、搜索和内存；分别记录逻辑与真实 WebView 指标。
- 正式安装包具备可验证的签名、公证、版本及更新路径，发布依赖同一 SHA 的验收结果。
- 分组、导航、结果预览和类型连线都通过实际复杂流程验收；每项报告区分模拟、原生和未覆盖场景。
