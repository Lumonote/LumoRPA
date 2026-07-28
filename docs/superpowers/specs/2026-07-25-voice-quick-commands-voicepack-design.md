# 语音快捷指令 + 本地 Lumo AI 语音包 + 结果反馈增强 — 设计

日期：2026-07-25 ｜ 状态：已实施

> 命名说明：原始需求提及「小智AI语音包」，因「小智」是另一个项目的名称，
> 按用户指示（2026-07-25）改名为 **Lumo AI 语音包**（persona id `lumo`）。
> 全部代码、话术、测试均不出现旧称。

## 目标

1. **多种指令控制的快速执行**：常用口令（打开视图、运行流程、停止、静音、状态查询等）在本地即时匹配执行，不经过 LLM agent 规划，毫秒级响应；未命中口令的话语仍走现有 agent 路由。支持用户自定义「短语 → 动作」。
2. **本地 Lumo AI 语音包**：内置 `lumo` 人格语音包（完全本地：中文系统 TTS 音色 zh-CN + 亲切话术模板），语音设置中一键切换；保留 `default` 默认语音包。
3. **结果反馈增强**：指令/任务执行结果通过 ①胶囊 UI（success/error 态 + 结果文案）②TTS 播报（人格化、尊重安静模式）③主窗口 toast ④视图联动（`lumo://open-view`）四路反馈。

## 现状（勘察结论）

- 管线：capture → wake(sherpa) → STT(本地/云路由) → `publish_transcript`（voice_commands.rs）→ final 时 emit `lumo://agent-start-request` → agent_commands.rs 起完整 agent 会话。
- 反馈：`announce_agent_status` = macOS TTS + `lumo://voice-state`(reporting/error) + follow-up 续听。agent_service.rs 在任务完成/失败时调用。
- 配置：`VoiceConfigDto`（serde camelCase，磁盘 voice-config.json，向后兼容靠 `#[serde(default)]`）。
- 前端：voice-capsule.js（事件投影渲染）、voice-settings.js（设置表单+模型卡）、`lumo://open-view`/`lumo://toast` 已有监听。

## 方案（选定：后端权威快速通道）

备选比较：A) 前端匹配（弱：胶囊窗口隐藏时失效、无法直接调内部命令）；B) 交给 agent 的 LLM 快速意图（弱：仍有延迟与不确定性）；**C) 后端 `publish_transcript` 处前置本地匹配（选定）**：零网络、零 LLM、对所有入口（唤醒词/快捷键/续听）生效，且能直接调用内部命令（stop_host、run_flow、daemon set_muted）。

### 新模块（apps/desktop/src-tauri/src/）

**voice_persona.rs** — 语音包（人格）定义：
- `VoicePersona { id, display_name, tts_voice, 模板集 }`；`PersonaMoment { Ack, Success, Failure, Status, FollowUp }`
- 内置：`default`（现行朴素文案，系统默认音色）、`lumo`（话术：「好嘞，Lumo 这就去办…」「搞定啦！…」「哎呀…」，音色 `zh-CN` 走 AVSpeech voiceWithLanguage，完全本地）
- `persona(id)` 未知 id 回落 default；`render(moment, detail)` 纯函数可单测。

**voice_intents.rs** — 快速指令引擎（纯逻辑）：
- `QuickIntent`：`OpenView{view,label}` / `StopAll` / `SetMuted(bool)` / `StartListening` / `Status` / `RunFlow{query}`
- `match_quick_intent(text, custom)`：归一化（去空白/标点、小写、循环剥离「lumo/请/帮我」等前导词）→ 自定义短语（完全匹配，优先）→ 内置别名表（视图别名 × 动词前缀；停止/静音/取消静音/听写/状态；「运行|执行|启动 X (流程)」→ RunFlow{X}）。全部完全匹配避免误触发；None 则回落 agent。
- `resolve_flow(query, flows)`：跳过 invalid，先精确后双向包含；`Unique/Ambiguous/NotFound`。
- `validate_quick_commands`：短语非空、归一化去重、action ∈ {open_view, run_flow, stop, mute, unmute, status, listen}、open_view 参数 ∈ 视图白名单（mission-control/capability-hub/settings/runs/design/recorder）、run_flow 参数非空。

### voice_commands.rs 修改

- `VoiceConfigDto` += `voicePack`（默认 "default"）、`quickCommands`（`QuickCommandDto { id, phrase, action, argument, enabled }`）；旧配置文件自动兼容（serde default）。
- configure 校验：quick commands + voice pack 合法性。
- `publish_transcript`：final 先 `match_quick_intent`；命中 → `handle_quick_intent`（spawn dispatch，**不发** agent-start-request）；未命中 → 原路径。
- `announce_feedback(moment, detail, speak, allow_follow_up)` 统一反馈：voice-state + `lumo://voice-feedback`{ok,message} + `lumo://toast`（标题=语音包名）+ 人格化 TTS（persona 音色）+ 可选续听。`announce_agent_status` 改为其薄封装（agent_service 调用点零改动）。
- 执行语义：StopAll = stop_host + 取消 agent 会话，不开续听；SetMuted 走 daemon 内部通道并 emit `lumo://voice-muted`；StartListening 成功时静默（避免 TTS 打断收音）；RunFlow 先 Ack 播报 → `crate::run_flow`（inputs `{}`）→ 按 `report.success` 播报步数结果；Status 汇总 daemon 状态 + 活跃 agent 会话数（agent_commands::active_agent_session_count）。
- `schedule_follow_up` 事件携带 persona 续听文案（前端优先展示）。

### 前端修改

- voice-capsule.js：`voice.feedback` → status `success|error`（DONE/ERROR 标签、清空转写、进度置满）；监听 `lumo://voice-feedback`、`lumo://voice-muted`；续听消息优先用后端 persona 文案。
- voice-settings.js：「语音包」选择（默认助手 / Lumo AI · 本地）；「语音快捷指令」编辑器（短语/动作/参数/启用/删除/添加行，`collectQuickCommands` 收集）；serialize/normalize 携带 voicePack、quickCommands。
- styles：voice-capsule.css `.is-success`（#45e6d0 系）；voice-settings.css `.voice-quick-commands` 编辑器样式。

## 测试与验证

- Rust（`cargo test -p lumorpa-desktop`）：意图匹配全表、流程解析（唯一/多义/无/invalid 过滤）、快捷指令校验、config 新字段 round-trip + 旧配置兼容、persona 渲染与回落。
- 前端（`node --test`）：capsule feedback 投影/渲染/续听文案、settings 新字段 normalize/serialize、编辑器渲染与 DOM 收集。
- `cargo clippy -p lumorpa-desktop`；日志读全文确认（不信管道 exit code）。

## 非目标（YAGNI）

- 不引入新 ASR/TTS 模型下载（语音包用系统本地中文音色；模型 manifest 体系不动）。
- 不做拼音/模糊音纠错；固定别名表 + 自定义短语已覆盖诉求。
- 不改 agent 规划路径与现有快捷键体系；RunFlow 暂以空 inputs 运行（需输入的流程会得到明确失败播报）。

---

# 第二波（2026-07-25 同日「全部完整补充」）

> 用户随后要求把建议清单全部落地。上面「非目标」中的拼音容错与参数追问在本波转正。

## A 线（语音）

- **A1 拼音谐音容错**：`pinyin` 纯 Rust 依赖；`pinyin_key`（无声调全拼）作第二层等价：别名/自定义短语/流程名（`resolve_flow` 精确与包含两档均比对拼音键）。「人物中心」→任务中心、「陈间日报」→晨间日报。
- **A2 连续多指令**：`match_quick_commands` 按 紧接着/然后/接着/随后 切分（长词优先），**全段命中才执行**（否则整句回落 agent）；顺序 await 执行，逐项静默播报（视觉/toast/历史保留），结束统一播报「已依次执行 N 项」。多指令模式不开启对话（确认按“任一项要求确认→整批确认一次”）。
- **A3 语音对话机**：`VoiceRuntime.dialog: Option<VoiceDialog>`（`Confirm{intents}` / `CollectInputs{path,pending,collected}`），publish_transcript 优先级：**对话 → 快捷指令 → agent**；过期（确认 15s / 参数 25s）自动失效。确认词/取消词含拼音层；参数按 `IoDeclDto`（required 且无 default）逐项 TTS 追问，number/boolean 粗转换、错误答复带提示重问、「取消」可退出；胶囊 confirming 态 + 免唤醒续听收音。自定义指令 `confirm` 标志 + 全局 `confirmFlowRun` 配置。
- **A4 跨平台 TTS**：lumo-voice `system_tts`（macOS AVSpeech / Windows PowerShell System.Speech / Linux spd-say→espeak 回退），命令构造纯函数 + 单测，进程 kill 级取消；`announce_feedback`/对话提问/试听统一走它。
- **A5 音色语速**：`ttsVoice`（覆盖语音包音色）+ `ttsRatePercent`（10..=100，整数保 Eq）；设置页音色输入、语速下拉、「试听」按钮（`voice_tts_preview` 命令，不受安静模式限制）。
- **A6 指令历史**：`DesktopState.voice_history`（VecDeque 上限 50：atMs/utterance/intent/ok/message），每次快捷指令/对话结束 push + `lumo://voice-history` 事件；`voice_command_history` 命令；任务中心新增「最近语音指令」区块（mission-control `renderVoiceHistory`/`refreshVoiceHistory`，main.js 事件驱动刷新）。
- **A7 导入导出**：设置页导出 JSON（Blob 下载）/导入合并（`mergeQuickCommands` 按去空格短语去重、导入优先，提示后需手动保存生效）。

## B 线（项目级缺口）

- **B1 lint UI**：核验发现主 lintBtn（错误/警告/提示计数 + 前 8 条详情 toast）已在此前未提交改动中实现——无需新代码。
- **B2 嵌套断点**：`DebugController` 三层命中——运行期路径全等 / 叶子 step id（Studio 存裸 id）/ 双侧 `[N]` 序号剥离后的静态链相等（`loop[3]/click` ↔ `loop/click`）。宁可多停不可不停（同名步骤跨迭代/分支都会停）。回归：breakpoint.rs `breakpoint_on_bare_id_and_static_chain_pauses_inside_loop`。
- **B3 桌面录制器**：核验发现 lib.rs 已对 desktop/mixed 实例化真实 `DesktopRecorder`（R-02 注释，Noop 仅未知目标兜底）——无需新代码。

## 已知边界

- 多指令批中的 RunFlow 若缺参数 → 明确失败播报（对话只在单指令开启）。
- 参数追问依赖收音可用（守护静音时提问后不自动收音，可用快捷键作答窗口内补收）。
- `tauri dev` 交互冒烟仍需人工执行一轮（自动验证覆盖编译/单测/静态）。
