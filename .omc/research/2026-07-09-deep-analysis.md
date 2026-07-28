# LumoRPA 深度分析（2026-07-09，HEAD 9e56de9 + 未提交 diff）

三路并行：架构（architect）、指令集缺口复盘（analyst）、未提交 diff 审查（code-reviewer）。所有结论经 file:line 一手核验。

## 总览

- 06-10 报告的 8 项 P0/P1 已确认修复（desktop cancel/timeout/artifacts 接线、resource kind/retry.on 值域校验、template_ctx Arc 化、pg/mysql limit+timeout、validate_steps 合一、artifacts 生产者、try/finally 根因保留）。
- 指令集 06-10 清单：24 新增中 15 已做、9 未做；9 增强中 4 已做、1 部分、4 未做。
- **新发现 2 个 P0（架构）+ 3 个 HIGH（未提交 diff）**；根因与上轮同源：引擎能力与宿主/再入口之间缺乏强制接线契约。

## A. 未提交 diff 审查 —— REQUEST CHANGES

前端测试基建（npm test 10/10 过、ESLint 零告警、cargo check 过），但 3 个 HIGH 必修：

1. **HIGH-1 ai-drawer 保存丢 branches（静默数据丢失）**：`frontend/src/js/editor/ai-drawer.js:71-75` patch 不含 branches → control.parallel 步骤经 AI 抽屉保存后 branches 子树从 YAML 消失。修：patch 补 `branches: step.branches`。
2. **HIGH-2 分支内断点契约不匹配**：`yaml.js:271-275` stepIdChain 遇 branches 不产出路径段（`parallel/b_wait`），VM 是 `parallel/branch[1]/b_wait`（vm.rs:1523，ctx.rs:151 精确匹配）→ 分支内断点永不命中、debugPausedAt 标记对不上；yaml.test.js:65 把错误行为固化。修：push `branch[i]` 段并改断言。
3. **HIGH-3 图节点点击选中大概率失效**：`graph.js:273-280` document 捕获层 pointerdown 无阈值立即 beginGraphNodeDrag（setPointerCapture+preventDefault）→ click 派发到 svg，closest 找不到节点。修：<4px 位移不算拖动 + handlePointerEnd 显式 selectStep。
- MEDIUM：resize 后 viewBox 过期坐标错位（建议干脆不设 viewBox）；拖拽每帧全量 innerHTML 重建掉帧。
- LOW：节点下方 40px 隐形条带可拖/挡 pan；手工布局按索引键控结构编辑后错位；npm test glob 不兼容 Windows（改 `node --test test/`）。

## B. 架构新问题

### P0
1. **flow.call / skill.invoke 子 VM 漏接全套宿主能力**【M】：`flow_call.rs:119-121`、`action.rs:89-91` 只 `FlowVm::new(.., None)`（vm.rs:116-124 默认全 None）。后果：① 父取消只翻父 ctx 中断位（vm.rs:707-743），子流程 spawn_blocking 的 excel/sqlite/pdf 副作用照落地——851e117 修的「判死后事务仍 commit」经子流程复活；② human.* 在子流程/skill 内必炸（prompter=None）；③ vault 断；④ 子 run 不落库但返回 run_id → 时光机盲区；⑤ **增补：父取消/超时 drop 子 future 时子 VM 的 teardown（vm.rs:359-368，按 run_id 隔离）永远走不到 → 子流程的 headless Chrome / DB 连接进程级泄漏**。修：`FlowVm::child_of(ctx)` 一处继承全部能力 + 保证子 VM teardown 必执行 + 「取消穿透」与「取消后子资源已回收」两条回归测试。
   （附：架构收口确认——单层 run 的 teardown 三路径覆盖健康；上轮 P2「unwrap 集中于 serve/lib/mcp」撤销：非测试区 unwrap 全为 0；recorder_start 竞态已修 lib.rs:1765-1766。）
2. **桌面前端未接 cancel_run 与 human-prompt**【S-M 纯前端】：后端 lib.rs:1155/1266-1283 就绪，`grep frontend/src` 零命中 → 桌面用户没有取消按钮；human.* 提示无人应答一律挂到超时（approve=拒绝）。修：运行面板取消按钮 + listen("human-prompt") 三形态模态 + human_respond。

### P1
3. serve/mcp/hotkey 漏接 cancel/step_timeout/artifacts/prompter；CLI 漏 step_timeout（LUMO_STEP_TIMEOUT_MS 只在 desktop 解析）；serve 无 cancel 路由、无并发上限。修：host_vm helper + POST /runs/:id/cancel + Semaphore。【M】
4. 能力静态校验三份硬编码漂移（ctx.rs:839-889 真源 vs lumo-core/validate.rs:48-64 vs lint.rs:141+）→ validate 全绿运行期 CapabilityDenied。修：Action trait `required_caps()` 元数据，registry 派生。【M】
5. vars_json 每步全量快照落库（vm.rs:1698）+ ctx.rs:618-620 持锁深拷贝 + repo 零 retention → O(steps×vars) 放大。修：超阈值截断 + 代数去重 + `lumo runs prune`。【M】
6. human.* 默认 1h 超时被桌面步级默认 10min 截杀（human.rs:38 vs lib.rs:2660-2667），GUI 无设置项 → >10min 审批必 timeout。修：human.* 豁免步级超时或取 max。【S】
7. desktop 未接 with_vault（lib.rs:2604-2618 链中唯一缺项；CLI/serve/mcp/hotkey 都有）→ 同 flow CLI 能跑桌面报错。【S】

### P2
log_buffer 只写不读（control.log GUI 不可见+内存线性涨）；lib.rs:1520/1530 运行期 set_var 数据竞争；build_action_registry 每命令全量重建+吞 skills 加载错误；excel 样式族整簿读写税（T3 `kind: xlsx` 资源仍未做）；desktop lib.rs 3859 行继续膨胀（feature_map 外置 JSON）；network grant 带 scheme 永不匹配无校验；无 run 进度事件流（前端只能轮询）。

### 能力×宿主接线矩阵
| 能力 | CLI | desktop | serve | mcp | hotkey | 子VM |
|---|---|---|---|---|---|---|
| cancel | ✅ | ✅(前端❌) | ❌ | ❌ | ❌ | ❌ |
| step_timeout | ❌ | ✅ | ❌ | ❌ | ❌ | ❌ |
| artifacts | ✅ | ✅ | ❌ | ❌ | ❌ | ❌ |
| prompter | ✅ | 后端✅/UI❌ | ❌ | ❌ | ❌ | ❌ |
| vault | ✅ | ❌ | ✅ | ✅ | ✅ | ❌ |
| repo 落库 | ✅ | ✅ | ✅ | ✅ | ✅ | ❌ |

## C. 指令集缺口复盘

### 06-10 清单落地状态
已做（15+4）：email.fetch 增强版（include_body/save_attachments_to）、email.mark/move、xml.parse/build/xpath、desktop.screenshot、window.list/activate/bounds、browser.drag_and_drop/print_pdf/wait_response、control.while/break/continue、human.input/confirm/approve、desktop.click_text、excel.lookup、file.wait、util.url_encode/decode；增强：browser proxy/UA、http mTLS/proxy、system.process_kill/app_start、notify 门禁复用。db 查询超时与 pg/mysql limit 也已修（06-10 可销账）。

未做重排：
- **P0**：pg/mysql 显式事务（`db.postgres_batch/mysql_batch`，sqlx begin，财务底线）；excel.set_formula「写而不算」文档注记（一行防误解）。
- **P1**：iframe click/type 入帧（登录/支付页高频；帧内 JS 合成事件 vs CDP DOM.resolveNode 待选型）；邮件触发器（降级：cron+fetch(unseen)+mark 已可轮询，IDLE 是增强）；db.sqlserver（tiberius，先要用户信号）。
- **P2 按需**：redis/mongo、mq、queue.*、pptx、captcha、invoice.ocr、soap（建议先出 xml.build+http+xpath 组合 example）。

### 新发现一致性/设计问题（14 项）
1. typed 错误分类覆盖极窄：db/http/email/file/excel 全走 msg→kind=other，`retry.on:[timeout]` 抓不到动作内超时（db_ops.rs:1035-1037）。
2. truncated/limit 约定只在 db/http：browser.extract_table、excel.read_rows、file.list 无上限，与 vars_json 落库复合放大。
3. timeout_ms 覆盖不均：email 全族无超时（connect 无包装 email.rs:449）；excel/pdf/docx spawn_blocking 无内部超时。
4. IMAP 无资源绑定：fetch→mark→move 3 次 TLS 登录，易触发频控 → 建议新增 imap 资源 kind。
5. ftp/s3/http 传输三套动词、data.json_* 与 json.* 并存 → alias/文档交叉引用。
6. dry_run 仅 desktop.click_text 一处 → 破坏性动作（file.delete/db exec/process_kill/email.send）应立横切约定。
7. excel 缺工作表级/行列级结构操作（add/delete/rename sheet、insert/delete row/col 全缺）。
8. hash 族仅字符串输入，缺文件校验 `path` 参数。
9. archive.zip 无密码支持（zip crate 支持 AES）。
10. window 族无 close/minimize/maximize。
11. browser 无 back/forward/reload 一等动作（recorder 无法映射）。
12. control.parallel 无并发上限、for_each 无 parallel 选项。
13. desktop 族无 drag（滑块/文件拖放）。
14. 门禁双体系不可见：capability YAML 与 LUMO_ALLOW_* 环境开关并行，validate 无法预知目标机开关 → validate/doctor 应提示。

## Top 建议（合并三路）

1. 先修未提交 diff 的 3 个 HIGH（丢 branches / 断点路径 / 点击选中）再提交。
2. `FlowVm::child_of(ctx)` 统一子 VM 构造（消灭 P0-1 全簇）。
3. 桌面前端补取消按钮 + human-prompt 对话框（最后一公里）。
4. 宿主接线契约测试（能力×宿主参数化表）+ host_vm helper 补齐 serve/mcp/hotkey。
5. pg/mysql 显式事务 + excel.set_formula 文档注记（指令集 P0）。
6. Action required_caps 派生校验 + 三个横切约定立 checklist（truncated/limit、dry_run、typed error）。
7. 存储治理：vars_json 截断/去重 + runs retention。

## Open Questions
- db.sqlserver 是否有真实用户信号？
- iframe 入帧走 JS 合成事件（isTrusted=false 风险）还是升级 CDP？
- 三个横切约定是专项批次还是随新动作渐进？
- human.approve 的 serve/MCP webhook 回执（docs/05:327 planned）优先级？
