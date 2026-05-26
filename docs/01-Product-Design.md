# 01 · LumoRPA 产品功能设计

> 总报告：[00-LumoRPA-Master-Report.md](./00-LumoRPA-Master-Report.md)

本文回答"**产品长什么样、有哪些功能、用户怎么用**"。

---

## 1. 产品形态

| 组件 | 部署形态 | 受众 | 价格 |
|---|---|---|---|
| **LumoStudio** | Tauri 2 桌面 App（Win/macOS/Linux/UOS/麒麟） | 流程设计者 | 免费开源 |
| **LumoRunner** | Rust 二进制（可单机内嵌 / 可独立 daemon） | 执行机器人 | 免费开源 |
| **LumoCloud** | Self-host Web 控制台（Docker Compose / K8s Helm） | 团队/企业 | 免费开源，托管 SaaS 单收费 |
| **LumoCLI** | 跨平台 CLI（`lumo init/run/test/deploy/pack/sign`） | 开发者/CI | 免费开源 |
| **LumoMarket** | 公共应用市场（OCI registry + Web 浏览） | 全部用户 | 免费 + 付费应用 |
| **LumoMCP** | 内置 MCP Server / Client | LLM/Agent 联动 | 免费 |
| **LumoMobile**（可选） | Android 助手 APK + scrcpy/Appium | 手机自动化用户 | 免费 |

> 类比：影刀=Studio+Robot+控制台+商店+Copilot+AI Power+GO 八件套，LumoRPA 用 7 个开源组件等价覆盖且更模块化。

---

## 2. 功能矩阵（Master Checklist）

> **标签**：✅=MVP（M1）  🟦=AI Native（M2）  🟧=Enterprise（M3）  ✨=Frontier（M4）  ★=差异化/创新

### 2.1 流程设计（Design）

| ID | 功能 | 阶段 | 备注 |
|---|---|---|---|
| D-01 | 节点视图（拖拽 + 表单参数） | ✅ |  |
| D-02 | 流程图视图（DAG + 分支并行） | ✅ | ★ 影刀缺失 |
| D-03 | 代码视图（YAML/JSON + Code Lens） | ✅ | ★ 影刀缺失 |
| D-04 | 三向同构实时同步 | ✅ | ★ |
| D-05 | 变量面板（Watch / 修改） | ✅ |  |
| D-06 | 子流程 / 模块化 / 参数化 | ✅ |  |
| D-07 | Try/Catch/Finally 异常块 | ✅ |  |
| D-08 | 重试策略（次数/退避/异常过滤） | ✅ |  |
| D-09 | 条件分支 / 循环 / break / continue / return | ✅ |  |
| D-10 | 并行块（fork-join, race, all-settled） | ✅ |  |
| D-11 | 注释 / 折叠 / 标签 / 颜色 | ✅ |  |
| D-12 | **任意节点级单步 Run / 子流程级 Run** | ✅ | ★ 影刀只能从某行起跑 |
| D-13 | 断点 / 条件断点 / Hit Count | ✅ |  |
| D-14 | 变量类型推断 + LSP 自动补全 | 🟦 | TS 化 schema |
| D-15 | 自然语言生成节点 / 整段流程 | 🟦 | ★ Copilot |
| D-16 | Runbook 模式（Markdown SOP → Agent） | ✨ | ★ Sema4.ai 启发 |
| D-17 | 流程模板 + 参数化向导 | ✅ |  |
| D-18 | 流程评估集（Eval Set）+ 回归测试 | 🟧 | ★ |
| D-19 | 流程 Lint（最佳实践检查） | 🟦 |  |
| D-20 | Git 集成（commit/diff/blame 内嵌） | 🟧 | ★ |

### 2.2 录制器（Recorder）

| ID | 功能 | 阶段 |
|---|---|---|
| R-01 | Web 录制（CDP 事件捕获） | ✅ |
| R-02 | 桌面录制（Windows UIA） | ✅ |
| R-03 | macOS 录制（AX API） | 🟦 |
| R-04 | Linux 录制（AT-SPI） | 🟦 |
| R-05 | 智能录制（自动判别 Web/桌面/像素） | ✅ |
| R-06 | 录制时双轨：动作流 + 元素快照 | ✅ |
| R-07 | 录制后可视化校正/微调 selector | ✅ |
| R-08 | 自动归并冗余事件（去抖 / 同元素连点合并） | ✅ |
| R-09 | 相似元素一键抓取（影刀同款） | ✅ |
| R-10 | 锚点元素（用附近静态文本辅助定位） | ✅ |
| R-11 | 关联元素（父/子/兄弟） | ✅ |
| R-12 | 图像元素（OpenCV 模板匹配） | ✅ |
| R-13 | 录制 → 自动生成测试 case | 🟦 | ★ |
| R-14 | 录制 → 自动生成 Markdown SOP | 🟧 | ★ |

### 2.3 元素拾取与选择器

| ID | 功能 | 阶段 |
|---|---|---|
| S-01 | CSS 选择器 | ✅ |
| S-02 | XPath | ✅ |
| S-03 | Accessibility 树路径（UIA/AX/AT-SPI 统一） | ✅ |
| S-04 | Shadow DOM 穿透 | ✅ |
| S-05 | iframe 切换 | ✅ |
| S-06 | 智能选择器（多策略评分 + 自动选最稳） | ✅ |
| S-07 | 相似元素集合 / 关联元素 | ✅ |
| S-08 | 锚点定位 | ✅ |
| S-09 | OCR 文本匹配定位 | 🟦 |
| S-10 | 图像模板匹配 | ✅ |
| S-11 | **Vision-LLM 自愈选择器**（OmniParser + UI-TARS） | 🟦 | ★ |
| S-12 | Set-of-Mark Prompt 兜底 | 🟦 | ★ |
| S-13 | 选择器命中可视化（Inspector 高亮） | ✅ |
| S-14 | 选择器健康分（运行时打分，长期偏移预警） | 🟧 | ★ |

### 2.4 浏览器自动化

| ID | 功能 | 阶段 |
|---|---|---|
| B-01 | 启动/连接 Chromium/Edge（CDP） | ✅ |
| B-02 | 启动/连接 Firefox（Playwright 后端） | 🟦 |
| B-03 | 启动/连接 WebKit（Playwright 后端） | 🟦 |
| B-04 | 多 Tab / 多 Window / 多 Context | ✅ |
| B-05 | 元素操作（click/type/hover/scroll/upload/download） | ✅ |
| B-06 | 表单填充（auto-detect + Form-fill AI） | 🟦 | ★ |
| B-07 | 表格抓取（DataTable + 分页跟随） | ✅ |
| B-08 | JS 注入 / 执行 | ✅ |
| B-09 | Cookie / LocalStorage / IndexedDB 操控 | ✅ |
| B-10 | 网络拦截（route / mock / 录回放） | ✅ |
| B-11 | Headless / Headed 一键切换 | ✅ |
| B-12 | Stealth 反指纹（Patchright 思路） | 🟦 | ★ |
| B-13 | CAPTCHA 处理（图形/滑块/2FA） | 🟧 | 与第三方对接 |
| B-14 | 已登录 Session 接管（如 Browser-Use） | 🟦 |  |
| B-15 | 文件上传 / 下载（影刀 Win 端缺） | ✅ |
| B-16 | 视频录屏 + Trace + HAR 导出 | 🟦 |  |
| B-17 | 浏览器扩展模式（可选，作为 fallback） | 🟦 |  |
| B-18 | 云浏览器适配（Browserbase / Steel / Anchor） | 🟧 |  |

### 2.5 桌面自动化

| ID | 功能 | 阶段 |
|---|---|---|
| W-01 | Windows UIA 拾取/操作（via AccessKit） | ✅ |
| W-02 | macOS AX API（沙箱 entitlement 自动引导） | 🟦 |
| W-03 | Linux AT-SPI | 🟦 |
| W-04 | 模拟键鼠（绝对坐标 / 相对元素） | ✅ |
| W-05 | 热键 / 全局快捷键 | ✅ |
| W-06 | 窗口操作（查找/激活/置顶/最小化/关闭） | ✅ |
| W-07 | 剪贴板 | ✅ |
| W-08 | 图像匹配（OpenCV） | ✅ |
| W-09 | SAP GUI Scripting | 🟧 |
| W-10 | Citrix（H2C / vChannel） | ✨ |
| W-11 | Java Swing/JavaFX（JAB） | 🟧 |
| W-12 | 终端自动化（PTY 录制/回放） | 🟧 |
| W-13 | pywinauto 兼容 API（Python 侧） | ✅ | 迁移友好 |

### 2.6 Office / 文档

| ID | 功能 | 阶段 |
|---|---|---|
| O-01 | Excel 读写（openpyxl 引擎） | ✅ |
| O-02 | Excel 读写（COM/WPS 引擎） | ✅ |
| O-03 | Polars DataFrame Action 集 | ✅ | ★ 高性能 |
| O-04 | 批量写入（不走单元格循环） | ✅ |
| O-05 | 数据透视 / 筛选 / 排序 / 公式 | ✅ |
| O-06 | 跨工作簿/跨 sheet 合并 | ✅ |
| O-07 | 宏调用 | ✅ |
| O-08 | **Excel 行驱动循环（一行=一次任务）** | ✅ | ★ 用户核心场景 |
| O-09 | Word 读写（python-docx / COM） | ✅ |
| O-10 | PPT 读写（python-pptx） | ✅ |
| O-11 | Outlook / IMAP-SMTP / Exchange | ✅ |
| O-12 | PDF 读写（pdfium-rs + Marker） | ✅ |
| O-13 | OCR（PaddleOCR 3.0 PP-OCRv5） | 🟦 |
| O-14 | 文档结构化抽取（PP-StructureV3） | 🟦 |
| O-15 | 大模型文档抽取（合同/发票字段） | 🟦 | ★ |
| O-16 | WPS 兼容 | ✅ |

### 2.7 数据 / 网络 / 集成

| ID | 功能 | 阶段 |
|---|---|---|
| I-01 | HTTP（GET/POST/PUT/DELETE，文件上下载） | ✅ |
| I-02 | GraphQL | 🟦 |
| I-03 | Webhook 接收 | ✅ |
| I-04 | gRPC / Protobuf | 🟧 |
| I-05 | 数据库（MySQL/Postgres/SQLite/SQL Server/Oracle/达梦） | ✅ |
| I-06 | NoSQL（MongoDB/Redis） | 🟦 |
| I-07 | 消息队列（Kafka/RabbitMQ/NATS） | 🟧 |
| I-08 | FTP / SFTP / S3 / OSS / COS / OBS | ✅ |
| I-09 | 邮件（POP3/IMAP/SMTP/Exchange） | ✅ |
| I-10 | IM（钉钉/飞书/企微/Slack/Discord/Telegram） | 🟦 |
| I-11 | 主流连接器：Salesforce / SAP / Jira / Notion / GitHub / Gitee / Linear 等 | 🟧 | 借鉴 n8n 节点协议 |
| I-12 | 通用连接器（OpenAPI → 自动生成 Action） | 🟧 | ★ Tray 风格 |
| I-13 | 字段映射器（GUI 拖拽 JSON ↔ JSON） | 🟦 |

### 2.8 AI 节点

| ID | 功能 | 阶段 |
|---|---|---|
| A-01 | LLM 节点（多 provider，BYOK） | 🟦 |
| A-02 | Embedding / 向量检索（libSQL vector） | 🟦 |
| A-03 | RAG 知识库（文档→分块→嵌入→检索→生成） | 🟦 |
| A-04 | OCR 节点 | 🟦 |
| A-05 | 屏幕理解节点（OmniParser v2） | 🟦 |
| A-06 | Vision 定位 Click（UI-TARS 1.5） | 🟦 |
| A-07 | **Computer Use 节点**（Claude/Gemini CU/Fara1.5 切换） | 🟦 | ★ |
| A-08 | **Planner/Actor/Validator 三相 Agent** | 🟧 | ★ Skyvern 启发 |
| A-09 | 函数调用 / Tool Use | 🟦 |
| A-10 | 多 Agent Orchestrator | 🟧 | ★ |
| A-11 | Form-fill 整体语义填充 | 🟧 | ★ UiPath |
| A-12 | Clipboard AI 跨应用智能粘贴 | ✨ | ★ UiPath 独家 |
| A-13 | 流程自然语言生成 | 🟦 | ★ |
| A-14 | Self-Healing Router（零 LLM 重试） | 🟦 | ★ |
| A-15 | Voice Agent（语音输入输出 / 真实电话，可选） | ✨ |
| A-16 | 本地模型托管（OmniParser/UI-TARS/PaddleOCR/Qwen-VL） | 🟦 |

### 2.9 触发器 / 调度 / 队列

| ID | 功能 | 阶段 |
|---|---|---|
| T-01 | 定时触发（cron） | ✅ |
| T-02 | 文件触发（创建/修改，glob 通配） | ✅ |
| T-03 | 邮件触发 | ✅ |
| T-04 | Webhook 触发 | ✅ |
| T-05 | 热键触发 | ✅ |
| T-06 | 链式触发（流程完成→触发下一个） | ✅ |
| T-07 | MCP 工具调用触发 | 🟦 | ★ |
| T-08 | 数据库变化触发（CDC） | 🟧 |
| T-09 | 工作队列（多 worker 抢占） | 🟧 |
| T-10 | 优先级 / 速率限制 / SLA | 🟧 |
| T-11 | 任务依赖 DAG（含跨流程） | 🟧 |
| T-12 | 限流 + 退避 + 死信队列 | 🟧 |

### 2.10 调试 / 可观测

| ID | 功能 | 阶段 |
|---|---|---|
| X-01 | 单步 / 断点 / 变量面板 | ✅ |
| X-02 | 截图日志（每步 before/after） | ✅ |
| X-03 | DOM 快照（每步） | ✅ |
| X-04 | 错误堆栈 + 重试链路 | ✅ |
| X-05 | OTel GenAI semconv span 输出 | 🟦 | ★ |
| X-06 | 内置 Trace Viewer（火焰图 + 截图缩略） | 🟦 | ★ |
| X-07 | **Time-Travel Debugger 回放** | 🟧 | ★ |
| X-08 | 接入 Grafana / Langfuse / Jaeger | 🟧 |
| X-09 | 实时 stdout/stderr / 日志检索 | ✅ |
| X-10 | 模型 token / 费用统计 | 🟦 |

### 2.11 控制台 / 编排 / 治理（LumoCloud）

| ID | 功能 | 阶段 |
|---|---|---|
| C-01 | 项目 / 文件夹 / 标签 / 权限 | 🟧 |
| C-02 | 机器人列表 / 心跳 / 在线状态 | 🟧 |
| C-03 | 任务编排（DAG 可视化） | 🟧 |
| C-04 | 队列管理 + 限流 + 优先级 | 🟧 |
| C-05 | 凭据 Vault（age 加密 + 平台 KeyStore） | 🟧 | ★ |
| C-06 | 凭据 just-in-time 注入 | 🟧 | ★ |
| C-07 | RBAC + SSO（OIDC / SAML） + LDAP | 🟧 |
| C-08 | 审计日志 / Session Replay | 🟧 | ★ |
| C-09 | 流程版本 + 灰度 / A-B 发布 | 🟧 |
| C-10 | 流程评估集回归 | 🟧 | ★ |
| C-11 | ROI Dashboard（人时节省、运行次数、失败率） | 🟧 |
| C-12 | 多租户 | ✨ |
| C-13 | 信创合规模板（等保三级 checklist） | 🟧 |

### 2.12 安全 / 沙箱

| ID | 功能 | 阶段 |
|---|---|---|
| Se-01 | 流程 capability 声明（network/fs/mic/cam） | ✅ |
| Se-02 | 默认 deny 网络出站 | ✅ |
| Se-03 | 独立 OS user 运行 | 🟧 |
| Se-04 | 平台沙箱（Firejail/sandbox-exec/Job Object） | 🟧 | ★ |
| Se-05 | 凭据隔离（LLM 看不到原文） | 🟧 | ★ |
| Se-06 | Prompt Injection 防护（消毒 + 白名单 + 高风险动作人工确认） | 🟧 | ★ |
| Se-07 | 全链路 TLS + mTLS | 🟧 |
| Se-08 | 签名插件（cosign） | 🟧 |
| Se-09 | 流程数据加密落盘（age） | 🟧 |

### 2.13 移动 / 跨设备

| ID | 功能 | 阶段 |
|---|---|---|
| M-01 | Android 自动化（Appium 2.x + UiAutomator2） | ✨ |
| M-02 | scrcpy 实时镜像 + 操作 | ✨ |
| M-03 | 设备群控（多手机并发） | ✨ |
| M-04 | 云手机适配（华为云/阿里云） | ✨ |
| M-05 | iOS 自动化（WebDriverAgent，仅企业证书） | — |
| M-06 | 鸿蒙（HarmonyOS）适配（hdc） | ✨ |

### 2.14 市场 / 共享 / 协作

| ID | 功能 | 阶段 |
|---|---|---|
| Mk-01 | 流程包格式 `.lumoflow`（YAML in zip + 签名） | ✅ |
| Mk-02 | 插件包格式（OCI artifact） | 🟧 |
| Mk-03 | 应用市场（公共 + 私有） | 🟧 |
| Mk-04 | 加密/密码分享 | 🟧 |
| Mk-05 | 流程二维码分享（移动 → 桌面） | 🟧 |
| Mk-06 | 团队子流程库（含权限） | 🟧 |

### 2.15 MCP 双向打通

| ID | 功能 | 阶段 |
|---|---|---|
| MCP-01 | LumoRPA 作为 MCP Server，暴露 Actions / Flows | 🟦 | ★ |
| MCP-02 | LumoRPA 作为 MCP Client，调用外部 MCP 工具 | 🟦 | ★ |
| MCP-03 | MCP Tool Discovery + 权限审批 | 🟧 |
| MCP-04 | MCP Gateway / Allowlist | 🟧 |

---

## 3. 节点目录（Action Library 首发清单）

按"指令族"组织，MVP 首发 400 条左右（影刀近千条，先做 80/20）。

| 类别 | 数量 | 代表节点 |
|---|---|---|
| 流程控制 | 20 | 开始/结束、IF/ELSE、循环、并行、Try/Catch、Return |
| 变量 / 数据 | 30 | 设变量、JSON 解析、列表过滤、字典合并、日期格式化、UUID |
| Excel / DataTable | 40 | 打开/保存、读单元格/区域、写区域、按行循环、筛选、透视、合并 sheet、写入 CSV |
| 浏览器 | 60 | 启动浏览器、打开网址、点击元素、输入文本、获取属性、抓取表格、上传文件、切换 iframe、JS 执行、Cookie、Stealth、Headless 切换、抓截图、抓 PDF |
| 桌面 | 40 | 拾取元素、点击、双击、键入、滚动、热键、窗口操控、剪贴板、屏幕截图、像素匹配 |
| Office | 20 | Word 读写、PPT 读写、PDF 抽取、Outlook 收发 |
| HTTP / API | 15 | GET、POST、表单、文件上传、Auth（Basic/Bearer/OAuth2/JWT） |
| 数据库 | 15 | 连接、Query、Exec、批量 Insert、事务、迁移 |
| 邮件 / IM | 15 | SMTP 发送、IMAP 拉取、附件、钉钉/飞书/企微/Slack |
| 文件 | 15 | 读、写、追加、复制、移动、删除、压缩/解压、文件夹遍历、Hash |
| 系统 | 15 | 执行 Shell、环境变量、进程、服务、注册表（Win）、计划任务 |
| 影音 / 多媒体 | 10 | 截图、录屏、生成 GIF、ffmpeg 转码 |
| OCR / Vision | 15 | OCR 文本识别、表格识别、签名检测、二维码、人脸 |
| AI / LLM | 30 | Chat、Embed、Vector Search、RAG、Function Call、Computer Use、Form-fill、Planner、Validator |
| 触发器 | 10 | Cron、文件、邮件、Webhook、热键、MCP、队列 |
| Mobile | 10 | 连接设备、点击、滑动、输入、安装 APK、截图、Toast 监听 |
| 集成（首发） | 40 | GitHub、GitLab、Gitee、Linear、Jira、Notion、Lark、DingTalk、WeChat Work、Salesforce、HubSpot、Stripe、Shopify、Taobao、PDD、Douyin、小红书 |

> 插件协议保证：第三方 30 行 TypeScript 即可贡献一个新 Action（参考 n8n 节点协议但极简）。

---

## 4. 关键 UI 草案（ASCII）

### 4.1 LumoStudio 主界面

```
┌─ LumoStudio ───────────────────────────────────────────────────────────────┐
│ File  Edit  View  Run  Debug  Plugins  Help                       [☁ │ ⚙] │
├────────────┬────────────────────────────────────────────────┬──────────────┤
│ Sidebar    │  Editor (3-way isomorphic, switch by tab)      │ Inspector    │
│            │  ┌─[Graph]─[Nodes]─[Code]──────────────────┐   │              │
│ Flows      │  │                                          │   │ Selected:    │
│  ├ orders  │  │   ●━━[打开网址]                          │   │   ClickBtn   │
│  ├ pdf-ext │  │       │                                   │   │ Locator:     │
│  └ test-1  │  │   ●━━[Excel 循环开始]                    │   │  css> button │
│            │  │       │                                   │   │  ai-fallback │
│ Recorder ▸ │  │   ●━━[输入文本] ⚠ AI heal                │   │ Selector     │
│ AI ▸       │  │       │                                   │   │ Health: 87%  │
│ Vault ▸    │  │   ●━━[点击按钮]                          │   │              │
│ Triggers ▸ │  │       ▼                                   │   │ Last Run:    │
│            │  │   ●━━[Excel 循环结束]                    │   │  ✅ 2.3s     │
│            │  └──────────────────────────────────────────┘   │  📷 截图     │
├────────────┴────────────────────────────────────────────────┴──────────────┤
│ Terminal | Variables | OTel Trace | Replay                                 │
└────────────────────────────────────────────────────────────────────────────┘
```

### 4.2 三向同构编辑器（核心创新）

```
─[Graph]──────────────────       ─[Nodes]──────────────       ─[Code]───────────────
   ●━[Open URL]                   1. Open URL                   - id: n1
       │                              url: https://x.com         action: browser.open
   ●━[Click Login]                2. Click Login                 with: { url: ... }
       │                              selector: css>             needs: []
   ●━[Type User]                  3. Type User                 - id: n2
       │                              text: $vault.user           action: browser.click
   ●━[For Each Row]               4. For Each Row                with: { selector: ... }
                                       in: excel.rows         ...
```

任意一边的编辑实时映射到另外两边（基于同一份内存 AST）。

### 4.3 Time-Travel Debugger

```
┌─ Replay ─────────────────────────────────────────────────────────────────┐
│ ◀◀  ◀   ⏸   ▶   ▶▶      [══════●═══════════]   step 8/22                │
│                                                                           │
│  ┌─ Step 7 ─────────────┐  ┌─ Step 8 ⏸ ───────────┐  ┌─ Step 9 ────────┐│
│  │ click "登录"          │  │ type "u@x.com"        │  │ click "提交"     ││
│  │ ✅ 220ms              │  │ ⏸ 350ms              │  │ ❌ 1.2s          ││
│  │ 📷 [screenshot]      │  │ 📷 [screenshot]       │  │ 📷 [screenshot] ││
│  └──────────────────────┘  └──────────────────────┘  └──────────────────┘│
│  Variables:  user=u@x.com   url=...   page.url=login                      │
│  Trace span: lumorpa.action.browser.type  (otel.gen_ai.system=anthropic) │
└───────────────────────────────────────────────────────────────────────────┘
```

### 4.4 自然语言生成流程

```
┌─ Magic Prompt ──────────────────────────────────────────┐
│ 描述你要做的事情：                                       │
│ ┌─────────────────────────────────────────────────────┐ │
│ │ 从我的 Outlook 收件箱里找出所有附带发票 PDF 的邮件,  │ │
│ │ 抽取金额、日期、买方名, 写入 D:/invoices.xlsx       │ │
│ └─────────────────────────────────────────────────────┘ │
│ 模型：[ Claude Sonnet 4.6 ▼ ]  上下文：[ 选择子流程库 ▼ ] │
│                                              [生成流程] │
│ ─────────────────────────────────────────────────────── │
│ 生成结果（11 节点，预估成本 $0.12）：[预览] [插入]      │
└─────────────────────────────────────────────────────────┘
```

---

## 5. 用户角色与权限

| 角色 | 典型权限 |
|---|---|
| Viewer | 查看流程、查看日志 |
| Developer | 编辑流程、本地试跑、提交版本 |
| Reviewer | 审核流程、批准发布 |
| Operator | 启停机器人、查看运行 |
| Admin | 凭据、用户、配额、审计 |
| Service Account | API/MCP 调用，无 UI |

---

## 6. 计费 / 资源模型（开源 + 可选托管）

- **开源版**：无限制；用户自带 LLM Key。
- **托管 SaaS**（可选）：按 **execution-minute + AI token** 计费（避免影刀"按机器人"和 Zapier"按 task"两个极端）。
- **企业**：按 worker 节点数 + SLA。

---

## 7. 与影刀直接对比（业务用户视角）

| 场景 | 影刀做法 | LumoRPA 做法 | 体验差 |
|---|---|---|---|
| 录到一半发现要改第 5 行 | 必须从头跑 | 直接对第 5 行 Right-Click → Run This Step | +++ |
| 一个流程 200 行，找一个 bug | 没有流程图视图，靠搜索 | 切到 Graph 视图，DAG 一目了然 | +++ |
| 提交给同事评审 | 发 `.flow` 文件 | 提交 PR，行级 diff | +++ |
| Selector 跑挂了 | 手动重拾 | Vision-LLM 自愈或弹窗"建议替换" | +++ |
| 跑的时候不能动鼠标 | 是 | Headless + 独立 user session，无感后台 | +++ |
| 多机器人同机并行 | 抢焦点冲突 | 每 worker 独立 session | +++ |
| 想让 Claude 调度 | 仅 MCP server（影刀已有） | MCP server + Client，双向 | + |
| 凭据安全 | 控制台保管 | Vault + JIT inject + LLM 不可见 | ++ |
| 离线开发 | 不行 | 完全可以 | +++ |
| 二次开发 / 私有指令 | 闭源 | 30 行 TS 写一个 Action | +++ |

---

## 8. 体验设计原则

1. **小白 10 分钟跑通第一个流程**：默认模板 + 自然语言 + 录制三入口。
2. **专业用户用 VS Code 也能写**：纯文本 YAML + LSP + CLI。
3. **失败永远要有"为什么 + 怎么修"**：截图 + selector 偏移图 + AI 建议。
4. **永远不会偷偷上传你的数据**：默认离线，云端连接显示徽章。
5. **比影刀更"中国友好"**：内置钉钉/飞书/企微/淘宝/拼多多/小红书/抖音连接器；中文 Locale + 全中文文档。

---

下一篇 → [02 · 系统架构设计](./02-Architecture-Design.md)
