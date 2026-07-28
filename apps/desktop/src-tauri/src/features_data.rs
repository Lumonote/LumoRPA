//! Feature-map data: hard-coded implementation-status snapshot of the
//! design-doc feature matrix (rendered by the Studio "feature map" panel).
//! Pure move out of `lib.rs`; semantics unchanged.

use serde::Serialize;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct FeatureStatus {
    pub(crate) id: String,
    pub(crate) title: String,
    pub(crate) stage: String,
    pub(crate) status: String, // "ready" | "partial" | "planned"
    pub(crate) note: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct FeatureSection {
    pub(crate) id: String,
    pub(crate) title: String,
    pub(crate) items: Vec<FeatureStatus>,
}

/// Hard-coded snapshot of the implementation status of the design-doc feature
/// matrix. This is what the Studio "feature map" panel renders so the user can
/// see exactly which docs/01-Product-Design items are wired up vs. planned.
pub(crate) fn feature_map_data() -> Vec<FeatureSection> {
    fn item(id: &str, title: &str, stage: &str, status: &str, note: &str) -> FeatureStatus {
        FeatureStatus {
            id: id.into(),
            title: title.into(),
            stage: stage.into(),
            status: status.into(),
            note: note.into(),
        }
    }

    vec![
        FeatureSection {
            id: "design".into(),
            title: "流程设计 (D)".into(),
            items: vec![
                item(
                    "D-01",
                    "节点视图 + 表单参数",
                    "M1",
                    "ready",
                    "动作库可拖入画布，schema 自动生成属性表单。",
                ),
                item(
                    "D-02",
                    "流程图视图 (DAG)",
                    "M1",
                    "ready",
                    "Graph 视图基于 SVG 节点 + 折线连接。",
                ),
                item(
                    "D-03",
                    "代码视图 (YAML)",
                    "M1",
                    "ready",
                    "Code 视图行号 + 简易高亮。",
                ),
                item(
                    "D-04",
                    "三向同构实时同步",
                    "M1",
                    "ready",
                    "Graph / Tree / Code 共享同一 AST。",
                ),
                item(
                    "D-05",
                    "变量面板",
                    "M1",
                    "partial",
                    "inputs JSON 编辑 + outputs 展示。",
                ),
                item(
                    "D-06",
                    "子流程 / 参数化",
                    "M1",
                    "ready",
                    "`flow.call` 已注册，子流程文件按当前 flow 目录解析并继承 capability 裁剪。",
                ),
                item(
                    "D-07",
                    "Try / Catch / Finally",
                    "M1",
                    "ready",
                    "DSL + VM 已支持 try.catch.finally.",
                ),
                item(
                    "D-08",
                    "重试策略",
                    "M1",
                    "ready",
                    "retry.times/backoff/on 已在 VM 实现。",
                ),
                item(
                    "D-09",
                    "条件分支 / 循环",
                    "M1",
                    "ready",
                    "control.if / control.for / control.for_each / control.break.",
                ),
                item(
                    "D-10",
                    "并行块",
                    "M2",
                    "ready",
                    "★ control.parallel branches: [[steps], ...] 真并发执行（futures::join_all），StepCtx Arc<Mutex> 共享变量绑定；back-compat：do: [step,...] 每项作为单步分支。examples/parallel-demo.lumoflow.yaml.",
                ),
                item(
                    "D-11",
                    "注释 / 折叠 / 标签",
                    "M1",
                    "partial",
                    "Tree 视图可折叠；标签来自 metadata.tags.",
                ),
                item(
                    "D-12",
                    "任意节点级单步运行",
                    "M1",
                    "ready",
                    "run_step 命令已落地，可直接运行当前选中节点。",
                ),
                item(
                    "D-13",
                    "断点 / 条件断点",
                    "M1",
                    "ready",
                    "debug_flow 通过 VM 断点 / 单步模式 + resume_from 持久化运行实现断点调试。",
                ),
                item(
                    "D-15",
                    "自然语言生成节点 / 整段流程",
                    "M2",
                    "ready",
                    "Magic Prompt / `lumo copilot` 共用 lumo_ai::copilot，可生成并校验 LumoFlow YAML 草稿。",
                ),
            ],
        },
        FeatureSection {
            id: "recorder".into(),
            title: "录制器 (R)".into(),
            items: vec![
                item(
                    "R-01",
                    "Web 录制 (CDP)",
                    "M2",
                    "ready",
                    "★ BrowserRecorder + CDP Runtime.addBinding 注入 JS 钩子，捕获 click/input/change/keydown，附 CSS+XPath+a11y 标签。导航/心跳并存。",
                ),
                item(
                    "R-02",
                    "桌面录制 (Windows UIA)",
                    "M1",
                    "ready",
                    "DesktopRecorder 已接入 Studio 的 desktop/mixed 目标，轮询 OS 前台窗口/焦点控件并把 desktop 事件写入元素库；mixed 当前等价于桌面 lane。",
                ),
                item(
                    "R-05",
                    "智能录制 (自动判别)",
                    "M1",
                    "partial",
                    "browser 与 desktop lane 都可录制；真正同时捕获并自动归并两条 lane 的 CompositeRecorder 仍待补。",
                ),
                item(
                    "R-08",
                    "事件去抖 / 合并",
                    "M2",
                    "ready",
                    "★ ActionBuffer 200ms 同 selector 输入合并 + 三档跨事件抑制:click→input 焦点丢弃(<250ms)、input→change blur 回声丢弃(<500ms)、近距 dblclick 折叠(<60ms);7 个新测试覆盖正负路径.",
                ),
                item(
                    "R-09",
                    "相似元素一键抓取",
                    "M2",
                    "ready",
                    "★ Alt+点击触发同款泛化：注入 JS 比对父节点同 tag + 80% 共有 class，生成 `parent > tag.class` 选择器，YAML patch 直出 browser.extract { all: true } + 兄弟数注释。",
                ),
                item(
                    "R-10",
                    "录制→YAML patch",
                    "M2",
                    "ready",
                    "★ events_to_yaml_patch 把录制流转成可粘贴的 browser.open/click/type 步骤；desktop 焦点事件进入元素库，recorder_stop 直接返回。",
                ),
            ],
        },
        FeatureSection {
            id: "selectors".into(),
            title: "选择器 / Self-Healing (S)".into(),
            items: vec![
                item(
                    "S-01",
                    "CSS 选择器",
                    "M1",
                    "ready",
                    "browser.click / type 接受 selector (CSS) 或 selectors 多策略对象，二者择一。",
                ),
                item(
                    "S-02",
                    "XPath",
                    "M2",
                    "ready",
                    "★ selectors.xpath 走 document.evaluate；与 CSS / aria-label / text 共用 Self-Healing 回退。",
                ),
                item(
                    "S-06",
                    "智能多策略选择器",
                    "M2",
                    "ready",
                    "★ Self-Healing Router 完整落地：6 策略 (id/data-testid/css/aria-label/text/xpath)，按 base_cost × history_penalty 动态排序，每次解析记录 resolved_by 与 tried 列表，下一轮自动收益。Vision-LLM 后续 plug-in。",
                ),
                item(
                    "S-11",
                    "Vision-LLM 自愈",
                    "M2",
                    "partial",
                    "★ AI 层传输完成:`ChatMessage.attachments: Vec<ImageAttachment>` + base64/URL 双源 + Anthropic/OpenAI 双 wire 编码(`image_url` / `image` block)+ 7 个 vision 测试.OmniParser/UI-TARS 端到端注入选择器路由仍排期 M3.",
                ),
                item(
                    "S-12",
                    "Set-of-Mark 兜底",
                    "M2",
                    "partial",
                    "传输层就绪(可向 vision 模型发送截图);Set-of-Mark 标注 / 视觉坐标 → DOM 元素的反查机制排期 M3.",
                ),
            ],
        },
        FeatureSection {
            id: "browser".into(),
            title: "浏览器 (B)".into(),
            items: vec![
                item(
                    "B-01",
                    "Chromium CDP",
                    "M1",
                    "ready",
                    "lumo-actions::browser 已经接 chromiumoxide.",
                ),
                item(
                    "B-04",
                    "多 Tab / Context",
                    "M1",
                    "partial",
                    "browser.open / close 已就绪.",
                ),
                item(
                    "B-05",
                    "click / type / hover / scroll / upload / download",
                    "M1",
                    "ready",
                    "首发动作集已覆盖核心交互.",
                ),
                item(
                    "B-07",
                    "表格抓取",
                    "M1",
                    "partial",
                    "browser.extract 支持 map 字段.",
                ),
                item(
                    "B-11",
                    "Headless / Headed 切换",
                    "M1",
                    "ready",
                    "browser.launch 支持 headless 标志.",
                ),
                item(
                    "B-12",
                    "Stealth 反指纹",
                    "M2",
                    "planned",
                    "Patchright 思路排期 M2.",
                ),
            ],
        },
        FeatureSection {
            id: "office".into(),
            title: "Office / 文档 (O)".into(),
            items: vec![
                item(
                    "O-01",
                    "Excel 读写",
                    "M1",
                    "ready",
                    "excel.read_rows / write_row 已实现.",
                ),
                item(
                    "O-03",
                    "Polars DataFrame Action",
                    "M1",
                    "partial",
                    "data.* 系列动作初版.",
                ),
                item(
                    "O-08",
                    "Excel 行驱动循环",
                    "M1",
                    "ready",
                    "典型批处理场景；examples/excel-loop.lumoflow.yaml.",
                ),
                item(
                    "O-13",
                    "OCR (PaddleOCR 3.0)",
                    "M2",
                    "ready",
                    "image.ocr 支持云端 vision/OCR provider 与本地 ModelScope OCR 预设；桌面 Models 页可下载支持模型。",
                ),
            ],
        },
        FeatureSection {
            id: "ai".into(),
            title: "AI 节点 (A)".into(),
            items: vec![
                item(
                    "A-01",
                    "LLM 节点 (多 provider)",
                    "M1",
                    "ready",
                    "ai.chat + ProvidersConfig + Anthropic/OpenAI 适配.",
                ),
                item(
                    "A-02",
                    "Embedding / 向量检索",
                    "M2",
                    "planned",
                    "libSQL F32_BLOB 待启用.",
                ),
                item(
                    "A-05",
                    "屏幕理解 (OmniParser v2)",
                    "M2",
                    "planned",
                    "本地视觉路线.",
                ),
                item(
                    "A-07",
                    "Computer Use 节点",
                    "M2",
                    "planned",
                    "Claude / Gemini CU 适配.",
                ),
                item(
                    "A-13",
                    "自然语言生成流程",
                    "M2",
                    "ready",
                    "★ `lumo copilot \"...\"` 子命令通过 AiRouter 生成 lumo/v1 YAML 草稿,内置 system prompt 含 schema/合法 action id 列表;parse+validate 失败自动重试一次并把错误带回提示;支持 --out / --dry-run / --model 覆盖.",
                ),
                item(
                    "A-14",
                    "Self-Healing Router",
                    "M2",
                    "ready",
                    "★ 双层学习:per-strategy 成功率(`history_penalty` 1-3 倍成本)+ per-(prev→next) 转移概率(`transition_score` 0-1);贪心选择 `cost(s)/(1+5×score(prev→s))` 把验证过的恢复策略提到第二位即使基础成本更高;`resolve_element` 自动记录 last_failed→winner 转移;选择器统计已 JSON 持久化.Vision-LLM 端点排期 M3.",
                ),
            ],
        },
        FeatureSection {
            id: "triggers".into(),
            title: "触发 / 调度 (T)".into(),
            items: vec![
                item(
                    "T-01",
                    "Cron",
                    "M2",
                    "ready",
                    "★ `lumo serve` 启动时扫 --flows 目录，spec.triggers.[kind: cron, with: { schedule: \"0 */5 * * * *\" }] 每个触发器起独立 tokio 任务，按 schedule 睡到下一次 fire，run 走 lumo.db 持久化（trigger_kind=cron）。每次 fire 重新 parse flow，编辑后无需重启。",
                ),
                item("T-02", "文件触发", "M2", "ready", "★ `lumo serve` 同进程内 spawn `notify` watcher;`triggers.[kind:file, with:{path, events:[create,modify,remove], pattern:\"*.csv\"}]` 触发 → 输入 `{trigger:{path,kind}}` 自动注入,run 走 lumo.db 持久化(trigger_kind=file)."),
                item(
                    "T-04",
                    "Webhook",
                    "M2",
                    "ready",
                    "★ `lumo serve` 启 axum HTTP server (默认 127.0.0.1:8787)，POST /webhook/<flow-name> 触发流；流必须声明 spec.triggers.[kind: webhook] 才能被外网驱动；X-Lumo-Token 共享密钥可选；run 走 lumo.db 持久化。",
                ),
                item(
                    "T-05",
                    "热键",
                    "M1",
                    "ready",
                    "`lumo serve` 扫描 spec.triggers.[kind: hotkey] 并通过 rdev 后端监听组合键；无权限/不支持平台会显式降级。",
                ),
                item(
                    "T-07",
                    "MCP 工具调用触发",
                    "M2",
                    "ready",
                    "`lumo mcp` 通过 stdio 暴露 run_flow 等 flow 级工具；完整 action 级自动暴露和审批网关仍在 M3。",
                ),
            ],
        },
        FeatureSection {
            id: "observe".into(),
            title: "调试 / 可观测 (X)".into(),
            items: vec![
                item(
                    "X-01",
                    "单步 / 变量面板",
                    "M1",
                    "ready",
                    "右栏属性 + 单步运行入口.",
                ),
                item(
                    "X-04",
                    "错误堆栈 + 重试链路",
                    "M1",
                    "ready",
                    "step_runs.error_json 已写库.",
                ),
                item(
                    "X-05",
                    "OTel GenAI semconv",
                    "M2",
                    "planned",
                    "opentelemetry crate 待集成.",
                ),
                item(
                    "X-07",
                    "Time-Travel Debugger",
                    "M1",
                    "partial",
                    "时间线滑块基于已有 step_runs.",
                ),
                item(
                    "X-09",
                    "实时 stdout/stderr",
                    "M1",
                    "partial",
                    "Studio 底栏聚合日志.",
                ),
            ],
        },
        FeatureSection {
            id: "mcp".into(),
            title: "MCP 双向 (MCP)".into(),
            items: vec![
                item(
                    "MCP-01",
                    "LumoRPA as MCP Server",
                    "M2",
                    "ready",
                    "`lumo mcp --flows ./flows` 通过 JSON-RPC 2.0 / stdio 暴露 5 个工具 (list_flows, validate_flow, run_flow, list_runs, get_run) 以及 resources/list + resources/read(把流文件以 `file://` URI 暴露,Claude/Cursor 可直接读取 YAML;路径越界拒绝).",
                ),
                item(
                    "MCP-02",
                    "LumoRPA as MCP Client",
                    "M2",
                    "ready",
                    "`mcp.call` action 已注册;通过 stdio + JSON-RPC 2.0 调用任意 MCP server,执行 initialize → tools/call 握手,受 `capabilities.mcp` 白名单门禁保护.",
                ),
                item(
                    "MCP-03",
                    "Tool Discovery + 审批",
                    "M3",
                    "ready",
                    "`mcp.discover` action 通过 `tools/list` 返回工具描述符 + `proposed_grant` + `already_allowed`;`capabilities.mcp` 支持 `server`、`server:tool`、`server:tool_*` 三档粒度,`mcp.call` 强制按 `(server,tool)` 对放行.",
                ),
            ],
        },
        FeatureSection {
            id: "security".into(),
            title: "安全 / 沙箱 (Se)".into(),
            items: vec![
                item(
                    "Se-01",
                    "Capability 声明",
                    "M1",
                    "ready",
                    "spec.capabilities 在执行前强校验;Studio 右侧 `权限` Tab 渲染当前 network/fs.read/fs.write/llm/mcp 五档 chip 列表;每档自带 `+加白名单` 表单,通过 `add_capability_grant` Tauri 命令把 grant 追加回 YAML 自动去重并热刷新编辑器(配合 MCP-03 的 `proposed_grant`).",
                ),
                item(
                    "Se-02",
                    "默认 deny 网络出站",
                    "M1",
                    "ready",
                    "ai.chat 需要 LUMO_ALLOW_LLM_NETWORK=1;`add_capability_grant` 把 network/fs.read/fs.write/llm/mcp 五档 grant 写回 YAML,自动去重.",
                ),
                item(
                    "Se-05",
                    "凭据 LLM 不可见",
                    "M3",
                    "planned",
                    "Vault JIT 注入排期 M3.",
                ),
            ],
        },
    ]
}
