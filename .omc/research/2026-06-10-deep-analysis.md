# LumoRPA 深度分析（2026-06-10）

三路并行分析：架构问题（architect）、指令集缺口（analyst）、未提交 diff 审查（code-reviewer）。
工作区基线：`cargo clippy --workspace --all-targets -- -D warnings` 通过。

## A. 未提交改动审查（db_ops/http，REQUEST CHANGES）

阻塞提交的 2 个 HIGH（均为「静默错数据」）：
1. `crates/lumo-actions/src/db_ops.rs:661-736` — `pg_row_to_json`/`mysql_row_to_json` 对 TIMESTAMP/DATE/NUMERIC/DECIMAL/UUID 列 `try_get::<String>` 类型不兼容被 `unwrap_or(Null)` 吞掉 → 静默变 null；该转换路径零测试。修：chrono/bigdecimal/uuid feature 显式分支 + 回退改报错/warn。
2. `crates/lumo-actions/src/http.rs:881-1010` — `http.paginate` 不检查响应状态码，4xx/5xx 被当「分页自然结束」，返回部分数据且 `truncated: false`。修：读 body 前 `is_success()` 检查。

MEDIUM×4：db 动作无查询超时（PgIn/MysqlIn 加 `timeout_ms`）；`ctx.rs:735` 畸形 DSN 凭据回显进错误；unbound 路径每次新建整池裸 drop（改单连接 + `close().await`）；paginate next_url 模式重复附加初始 query。
LOW×4：oauth2 输出 `raw`（可能含 refresh_token）持久化；`link_header_next` 解析不严谨；`extract_host` 边缘 DSN；oauth2 `expires_in` 字符串型。
优点：DSN 凭据脱敏意识到位、每页 URL 重过 SSRF 门禁、文档三表已同步、sqlx 0.8.6 rustls-only 选型合理。

## B. 架构与代码问题（P0×2 / P1×7 / P2×8）

### P0
- **桌面端无法取消运行、无步级超时**：`apps/desktop/src-tauri/src/lib.rs:2365-2403` execute_flow 从不调 `with_cancel`/`with_step_timeout`；引擎侧机制完整（vm.rs:632-694、cancel_timeout.rs 测试齐全）。修：加 `cancel_run(run_id)` 命令 + CancelToken 入 DesktopState。
- **超时对 spawn_blocking 动作无效**：`vm.rs:638-647` select! 丢 future，excel(21处)/db(8处)/pdf/docx 的 spawn_blocking 照跑 → 超时后事务仍 commit 而状态记 timeout（重跑重复写入）、阻塞任务持连接锁卡后续步骤。范式参考 `archive.rs:249`（协作取消已有先例）。

### P1
- artifacts 全链路无生产者（`ctx.rs:635 with_artifacts_dir` 零调用方）→ Time-Travel/回放是死功能。
- 资源 kind 拼错静默降级（validate.rs:151 只查名字不查 kind；db/http/browser 运行期 `_ => None` 回退 default slot）；`registry.rs:55 resource_factory()` 无调用方。
- `template_ctx()` 每步全量深拷贝（ctx.rs:471-481）+ `vars_json` 每行全量落库（vm.rs:1438-1443）→ O(n²) 内存放大。
- pg/mysql 无行数 limit（sqlite 有 limit:1000+truncated）+ 4 段重复池解析（db_ops.rs:832-1048）。
- `control.try` finally 失败覆盖根因（vm.rs:1177-1184）；`error` var 永久污染命名空间（应学 for_each 的 push_binding）。
- `retry: {on: [timeout]}` 永不生效（ErrorKind 无 timeout，超时绕过 retry 循环）；retry.on/backoff 拼错静默。
- desktop lib.rs 3579 行上帝文件；`validate_steps` 与 lumo-cli 双实现；registry 每命令全量重建。

### P2（摘要）
control.log 三路输出两路死；http.paginate 聚合无总量上限；响应头 HashMap 丢重复 Set-Cookie；运行时 `std::env::set_var` 数据竞争；recorder_start 竞态泄漏；unwrap 集中在 serve.rs(39)/lib.rs(36)/mcp.rs(35)；desktop 0 集成测试（覆盖偏科）；excel 每个样式步骤全量 read→write 整簿（T3 加 `kind: xlsx` 资源是自然解）。

### 根因
1. 引擎层与宿主层无强制「能力接线清单」——CancelToken/timeout/artifacts/ResourceFactory 引擎做完、宿主漏接。
2. DSL 校验只覆盖结构不覆盖值域（resource kind、retry.on、backoff）。

### Top 5 优化
1. desktop 接线 CancelToken + step_timeout + artifacts_dir（一处改动消灭 P0-1+P1-1）
2. template_ctx 去深拷贝（全局性能税；内部已是 Arc<Map>）
3. 校验层补值域检查（kind/retry.on/backoff + validate_steps 上移合一）
4. db_ops pg/mysql 加 limit + 抽 4 段重复池解析（~250 行→80 行）
5. 拆分 desktop lib.rs（feature_map ~700 行硬编码数据外置 JSON）

## C. 指令集缺口（24 新增 + 9 增强）

经源码核实的断点：email.fetch 只取头无正文/附件；browser click/type 不能入 iframe、无拖拽/代理；desktop 无原生截屏（图像闭环断）；零 XML 动作；control 无 while/break/continue。

### 新增（按批次）
- **P0 第一批**：`email.fetch_full`/`email.mark`/`email.move`（async-imap+mail-parser）、`xml.parse/build/xpath`（quick-xml）、`desktop.screenshot`（xcap，desktop feature 内）、`window.list/activate/bounds`、邮件到达触发器（IMAP IDLE）。
- **P1 第二批（CDP 现成）**：`browser.drag_and_drop`、`browser.print_pdf`、`browser.wait_response`、`control.while`（max_iterations 守卫）、`control.break/continue`。
- **P1 第三批**：`human.input/confirm`（Tauri 弹窗+F-20 暂停机制）、`human.approve`（notify+webhook 回执）、`db.sqlserver_*`（tiberius 纯 Rust）、`desktop.click_text`（OCR 定位点击）、`excel.lookup`（vlookup 语义）、`file.wait`。
- **P2 按需**：redis/mongo、mq(MQTT/AMQP)、`soap.call`、`queue.*`（UiPath Queue 模式，注意与 M3 多 worker schema 兼容）、`pptx.replace_placeholders`、`captcha.solve`、`invoice.ocr`（电子发票结构化）、`util.url_encode/decode`。

### 现有增强
iframe 级选择器（click/type 入帧）；browser.launch 加 proxy/UA；pg/mysql 显式事务（`*_batch` 或 transaction 参数，财务底线）；http.request 加 mTLS/代理；excel.set_formula 文档明示「写而不算」；system 补 `process_kill`/`app_start`；notify 收消息走既有 webhook 触发器（补 example）；selectors 相对定位（低优）。

### 明确不做
Oracle（OCI C 库破坏信创红线，引导走中间 API）；Outlook COM；Excel 公式计算引擎；完整 UIA/AX 控件树（F-1 用户已定调坐标+图像，先用 OCR 点击验证需求）。

### Open Questions
窗口管理/截屏是否接受 desktop feature 的非信创 C-API 定位（建议接受）；桌面自动化用户占比 → 是否重启 UIA 评估；queue.* 单机先做 vs 并入 M3；电子发票是否要 OFD 版式支持。
