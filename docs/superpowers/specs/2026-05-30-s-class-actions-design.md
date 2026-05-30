# S 类动作批次 设计文档

> 生成于 2026-05-30。对应路线图 `docs/04-优化与补充开发-路线图.md` 的"🧩 补充开发 / Actions"中标记为 (S) / (S~M) 的五项:
> **F-5 剪贴板**、**F-7 ZIP/归档**、**F-8 通知**、**F-9 显式 browser.wait**、**F-11 http.download/上传 + 响应大小上限**。
> 这五项按"工作量(S)"分组而非按子系统分组,故以"标准库/实用工具动作批次"统一设计,各动作独立契约。

## 背景与现状

- **动作编写范式**(`crates/lumo-actions/src/*.rs`):每个动作 = 单元结构体 impl `Action`(`id()`/`summary()`/`schema()`/`async execute()`);输入经 `serde_json::from_value` 解析进 `#[derive(Deserialize)]` 结构;`schema()` 返回 `once_cell::sync::Lazy<Value>` 静态 JSON Schema(`additionalProperties: false`);每模块 `pub fn register(r: &mut ActionRegistry)`,由 `lib.rs::register_all` 汇总调用。
- **能力沙箱**(`crates/lumo-dsl/src/ast.rs:94` `Capabilities { network, fs_read("fs.read"), fs_write("fs.write"), llm, mcp }`,glob 模式):
  - 文件读写:`ctx.ensure_fs_read(&path)?` / `ctx.ensure_fs_write(&path)?`(`crates/lumo-core/src/ctx.rs:489/493`)。
  - 网络:`ctx.ensure_network_url(&url)?`(`ctx.rs:497`)。
  - **无** clipboard 能力变体(本批次不新增能力变体,见 F-5)。
- **现有可复用件**:
  - `http.rs::RequestAction`(`http.request`):`reqwest::Client::builder().timeout(..).build()` + `ensure_network_url`,返回 `{status, headers, text, json}`。
  - `browser.rs`:会话模型 `session_for_run(ctx.run_id())` / `current_page(&s)` / `resolve_element(page, spec, timeout_ms)`(多策略选择器轮询等待)/ `build_selector(selector, selectors)`(back-compat `selector:String` + 新 `selectors:{}`)/ `MultiSelector`(id/data_testid/css/aria_label/text_includes/xpath)。`browser.open` 已有 `wait_for: Option<String>`(单次 find,非轮询)。
  - `file.rs`:`ensure_fs_read/write` 前置 + `tokio::fs`;`WriteAction` 先 `create_dir_all(parent)`。
  - P0-2 路径规范化(lexical-clean 折叠 `..`/`.` 后做前缀匹配,不 canonicalize 软链接)—— F-7 zip-slip 防护复用此思路。
  - P1-3:`${{ vault.* }}` 在模板层字面量化、运行时解析,秘密绝不进 argv/快照/日志 —— F-8 的 `secret` 复用此通道。
- **依赖现状**(`Cargo.toml`):`reqwest 0.12` 已启用 `["rustls-tls","json","stream","multipart"]`(F-11 download 流式 + upload multipart 无需新增);`sha2`/`base64 0.22` 已在树(F-8 加签复用);**无** clipboard/zip/hmac/arboard(F-5/F-7/F-8 需新增)。
- **测试夹具**(`crates/lumo-actions/tests/common/mod.rs`):`run/ok/run_with/ok_with(id,input[,caps])`、`fs_caps(dir)`(授 `{dir}/**` 读写)、`ctx_with(caps)`;`wiremock 0.6` 起本地 HTTP、`tempfile` 起临时目录;每个 `tests/<module>.rs` 各为独立 crate(`mod common;`)。

## 范围

**纳入**(6 个新动作 + 1 处既有动作增强):
- `archive.zip` / `archive.unzip`(F-7,仅 ZIP)
- `http.download` / `http.upload`(F-11);`http.request` 增可选 `max_bytes`
- `notify.send`(F-8,统一动作 + 4 provider + 钉钉/飞书 HMAC 加签)
- `clipboard.get` / `clipboard.set`(F-5,arboard)
- `browser.wait`(F-9,present/visible/clickable/hidden/text)

**不纳入**(留待后续 M/L 类):tar.gz 等 ZIP 以外归档格式;图片剪贴板;FTP/对象存储(F-6);浏览器其余动作补全(F-10)。

## 结构方案(选定:方案 C)

- **新模块**:`clipboard.rs`、`archive.rs`、`notify.rs`(三个新域,各自成模块,与 `file.rs`/`string_ops.rs` 等"每域一模块"惯例一致)。
- **扩展既有模块**:`http.rs`(download/upload 与 `http.request` 同域)、`browser.rs`(`wait` 与 open/click/type 同域)。
- `lib.rs::register_all` 新增 `clipboard::register`/`archive::register`/`notify::register` 三处调用;http/browser 的新动作并入各自既有 `register()`。

> 备选方案 A(每功能各开新模块,含 http/browser 也新开)被否:download 与 request 同域却分模块,违背就近原则。备选方案 B(全塞 `utility.rs`)被否:单文件过大,不符现有组织。

---

## 动作契约

### F-7 归档 —— `archive.rs`

**`archive.zip`** — 打包文件/目录为 ZIP。
- 输入:`{ paths: [String](必填,≥1), dest: String(必填), base_dir?: String }`
- 输出:`{ dest, entries: u64, bytes: u64 }`
- 行为:Deflated 压缩;`paths` 中目录递归收录;归档内条目名为相对 `base_dir` 的路径(`base_dir` 缺省时取各路径的 `file_name`,即扁平收录到归档根)。
- 能力:对每个源路径 `ensure_fs_read`,对 `dest` `ensure_fs_write`。
- 实现:**两段式**——async 段持 `ctx` 枚举待打包文件、逐一过 `ensure_fs_read`、读出字节;随后 `tokio::task::spawn_blocking` 内用 `zip::ZipWriter` 同步写盘(zip crate 同步,放阻塞线程池不卡 runtime)。`dest` 父目录 `create_dir_all`。

**`archive.unzip`** — 解压 ZIP 到目录。
- 输入:`{ src: String(必填), dest: String(必填), max_total_bytes?: u64 }`
- 输出:`{ dest, entries: u64 }`
- 行为:解压全部条目到 `dest`。
- 安全:
  - **zip-slip 防护**:对每个条目名做 lexical-clean(折叠 `..`/`.`),拼到 `dest` 后校验规范化结果仍以 `dest` 为前缀;逃逸即 `StepError`(不写任何文件)。
  - **zip-bomb 防护**:`max_total_bytes` 缺省 `1 GiB`;累计已解压字节超限即中止并报错。
- 能力:对 `src` `ensure_fs_read`,对每个解出目标文件 `ensure_fs_write`。
- 实现:async 段读 `src` 字节 + 过 `ensure_fs_read` + 预扫条目名做 zip-slip/能力校验;`spawn_blocking` 内 `zip::ZipArchive` 逐条目解压,边解边累计字节判超限。

**依赖**:`zip = { version = "2", default-features = false, features = ["deflate"] }`(裁掉 bzip2/zstd/aes,缩小依赖与许可面;deflate 后端 flate2→miniz_oxide 纯 Rust,利好交叉编译。实现时取满足 MSRV 1.83 的最新 2.x)。

**测试**(CI 全可跑,tempfile):zip→unzip 往返内容一致;zip-slip 条目(`../evil`)被拒且未落盘;越权(无 fs_caps)拒绝;`max_total_bytes` 超限拒绝;空 `paths` 校验失败。

---

### F-11 http 下载/上传 —— 扩展 `http.rs`

**`http.download`** — 流式下载到文件,带大小上限。
- 输入:`{ url: String(必填), dest: String(必填), max_bytes?: u64, headers?: {String:String}, timeout_ms?: u64 }`
- 输出:`{ dest, bytes: u64, status: u16, content_type?: String }`
- 行为:`reqwest` 发 GET;`response.bytes_stream()` 流式写盘;`max_bytes` 缺省 `100 MiB`:
  - 若响应带 `Content-Length` 且 > `max_bytes` → 不下载,直接报错;
  - 流式累计字节超 `max_bytes` → 中止、删除残留文件、报错。
- 能力:`ensure_network_url(&url)` + `ensure_fs_write(&dest)`。`dest` 父目录 `create_dir_all`。

**`http.upload`** — 上传本地文件,两种形态。
- 输入:`{ url: String(必填), src: String(必填), mode: "multipart"|"body"(必填), method?: String(缺省 multipart→POST / body→PUT), field?: String(multipart 字段名,缺省 "file"), filename?: String(multipart 文件名,缺省取 src file_name), headers?: {String:String}, max_bytes?: u64, timeout_ms?: u64 }`
- 输出:`{ status: u16, headers: {String:String}, text: String, json?: Value }`(与 `http.request` 输出形状一致)
- 行为:
  - `mode="multipart"`:`reqwest::multipart::Form` 加一个文件 part(`field`/`filename`/读取的字节);
  - `mode="body"`:文件内容作为请求体,默认方法 PUT(适配 S3 预签名 URL / REST PUT)。
  - 读入文件前过 `max_bytes`(缺省 `100 MiB`)防 OOM。
- 能力:`ensure_network_url(&url)` + `ensure_fs_read(&src)`。

**`http.request` 增强**:新增可选 `max_bytes`(缺省高位如 `100 MiB`,**向后兼容**——现有流程不传则行为不变),响应体超限报错。落实路线图"响应大小上限"。

**依赖**:无新增(reqwest 已带 `stream`+`multipart`)。
**测试**(CI 全可跑,wiremock+tempfile):download 正常落盘 + 字节数正确;download 超 `max_bytes`(流式与 Content-Length 两路)均拒绝且不留残件;upload multipart 命中 + 服务端收到 part;upload body PUT 命中;upload/download 无网授权(无 network cap)拒绝;`http.request` `max_bytes` 超限拒绝。

---

### F-8 通知 —— `notify.rs`

**`notify.send`** — 统一通知动作。
- 输入:`{ provider: "dingtalk"|"feishu"|"wecom"|"webhook"(必填), url: String(必填), text?: String, payload?: Value, title?: String, msgtype?: "text"|"markdown"(缺省 text), secret?: String, timeout_ms?: u64 }`
  - `text` 与 `payload` 至少一项;`payload` 存在时按原样发送(高级用法),否则按 provider 由 `text`/`title`/`msgtype` 组装。
- 输出:`{ status: u16, ok: bool, response: Value }`
- provider 体形:
  - **dingtalk**:`{"msgtype":"text","text":{"content":text}}`;markdown → `{"msgtype":"markdown","markdown":{"title":title,"text":text}}`。
  - **feishu**:`{"msg_type":"text","content":{"text":text}}`;markdown → `interactive`/`post`(最小实现先支持 text,markdown 走 `post` 富文本最简结构)。
  - **wecom**(企业微信群机器人):`{"msgtype":"text","text":{"content":text}}`;markdown 同理。
  - **webhook**:有 `payload` 直发;否则发 `{"text":text}`。
- **加签**(`secret` 经 `${{ vault.* }}` 解析后传入,绝不进 argv/快照):
  - **dingtalk**:`sign = base64(HMAC_SHA256(key=secret, msg=f"{timestamp}\n{secret}"))`,URL 追加 `&timestamp={ms}&sign={urlencode(sign)}`。
  - **feishu**:`sign = base64(HMAC_SHA256(key=f"{timestamp}\n{secret}", msg=""))`,body 加 `{"timestamp": "{s}", "sign": sign}`。
  - **wecom/webhook**:不加签(忽略 `secret`)。
  - `timestamp` 由调用方注入(VM 无 `Date::now` 限制不适用于动作运行时;动作内用 `std::time::SystemTime` 取当前毫秒)。
- 成功判定:HTTP 状态非 2xx,或响应 JSON 的 `errcode`/`code` 字段存在且 `!= 0` → `StepError`(含 provider 返回消息),使流程显式失败;`ok` 字段反映此判定。
- 能力:`ensure_network_url(&url)`。
- **依赖**:新增 `hmac = "0.12"`(纯 Rust;`sha2`/`base64` 已在树;URL 编码用 reqwest 传递依赖 `url` 或手写百分号编码)。
- **测试**(CI 全可跑,wiremock):四 provider 各自 body 形状正确;dingtalk 有 secret 时 URL 带 `timestamp`+`sign`(可复算校验);feishu 有 secret 时 body 带 `timestamp`+`sign`;provider 返回 `errcode!=0` → 动作失败;无网授权拒绝。

---

### F-5 剪贴板 —— `clipboard.rs`

**`clipboard.get`** — 读系统剪贴板文本。
- 输入:`{}` → 输出:`{ text: String }`
**`clipboard.set`** — 写系统剪贴板文本。
- 输入:`{ text: String(必填) }` → 输出:`{ ok: true }`
- 实现:用 `arboard`;每次调用建短生命周期 `arboard::Clipboard`,整段包进 `tokio::task::spawn_blocking`(避免阻塞 runtime + 规避 `Clipboard` 的 `!Send`)。
- **无显示环境**(CI / headless Linux):`Clipboard::new()` 失败 → 映射为清晰 `StepError`("clipboard unavailable: {e}"),不 panic。
- 门禁:**不加** env/能力门禁(与 `system.env_get` 等本地纯信息动作一致)。模块 doc 注明:① 读取剪贴板可能含密码管理器等敏感内容,流程作者自负;② Linux X11 下进程退出后写入内容可能不持久的已知局限。
- **依赖**:新增 `arboard = { version = "3", default-features = false }`(裁 `image-data`,只要文本;实现时取满足 MSRV 1.83 的最新 3.x)。
  - ⚠️ 见"风险":arboard 的 Linux 传递依赖可能影响交叉编译硬门禁,实现时**先单独验证**,必要时改 `[target.'cfg(...)'.dependencies]` + `#[cfg]` 桩。
- **测试**:schema/输入校验单测 CI 可跑(`clipboard.set` 缺 `text` 失败;`clipboard.get` 接受空对象);真实剪贴板往返标 `#[ignore]`(CI 无显示,与 browser e2e 同处理)。

---

### F-9 显式等待 —— 扩展 `browser.rs`

**`browser.wait`** — 等待元素满足条件或文本出现。
- 输入:`{ selector?: String, selectors?: MultiSelector, condition?: "present"|"visible"|"clickable"|"hidden"(缺省 visible), text?: String, timeout_ms?: u64(缺省 30000) }`
  - `selector`/`selectors` 与 `text` 至少一项;纯 `text`(无 selector)对 `document.body` 判含串。
- 输出:`{ condition: String, matched: String, waited_ms: u64 }`
- 行为:复用 `build_selector` + `session_for_run(ctx.run_id())` + `current_page` + `resolve_element`;以 ~100ms 间隔轮询至条件满足或超时:
  - `present`:`resolve_element` 找到即可;
  - `visible`:找到且 `boundingBox` 有非零尺寸(JS `getBoundingClientRect`/`checkVisibility`);
  - `clickable`:`visible` 且元素未 `disabled`;
  - `hidden`:未找到或不可见(语义:等待元素消失);
  - `text`:目标元素(无 selector 时 `body`)`innerText` 含 `text` 串。
  - 超时未满足 → `StepError`(条件 + 选择器提示)。
- 能力:无额外(浏览器会话已由 `browser.open` 建立)。无新依赖。
- **测试**:schema/输入校验单测 CI 可跑;条件行为测试需真实 Chrome,标 `#[ignore]`(随 P1-8 浏览器排除项)。

---

## 横切关注点

### 新依赖与许可

| crate | 启用特性 | 许可 | deny.toml 白名单 | 备注 |
|---|---|---|---|---|
| `hmac` | 默认 | MIT OR Apache-2.0 | ✅ | 纯 Rust |
| `zip` | `["deflate"]`(裁默认) | MIT | ✅ | flate2→miniz_oxide 纯 Rust |
| `arboard` | `default-features=false` | MIT OR Apache-2.0 | ✅(本体) | **传递依赖待核**(Linux x11rb/wayland) |

> 三者本体许可均在现有 `deny.toml` 白名单(MIT/Apache-2.0 系)。实现各步加依赖后**必须**核对其传递依赖的许可仍在白名单(本地无 cargo-deny 时,以 `cargo tree` + 已知许可人工核;CI `cargo-deny` job 为最终门禁)。MSRV:三者均支持 ≤1.83。

### 安全与一致性
- 所有文件/网络访问一律走既有 `ensure_fs_read/write` + `ensure_network_url`,与 file.rs/http.rs 同沙箱;不绕过、不新增逃逸面。
- `archive.unzip` zip-slip 防护复用 P0-2 lexical-clean;zip-bomb 由 `max_total_bytes` 兜底。
- `notify.send` 的 `secret` 经 `${{ vault.* }}` 解析(P1-3 通道),绝不进 argv/快照/日志;加签在动作内完成。
- `clipboard.*` 不加门禁但 doc 标注数据敏感性与平台局限。
- 同步阻塞库(zip、arboard)一律 `spawn_blocking`,不阻塞 async runtime。

## 测试策略

- **CI 全绿可跑**:archive(tempfile 往返/zip-slip/越权/超限/空输入)、http.download/upload + request max_bytes(wiremock+tempfile)、notify(wiremock 四 provider body + 加签字段 + errcode 失败 + 越权)、clipboard/browser.wait 的 schema 与输入校验单测。各模块一个 `tests/<module>.rs`,`mod common;` 复用夹具。
- **标 `#[ignore]`**(随既有 P1-8 排除项,CI 不跑):clipboard 真实往返、browser.wait 真实 Chrome 行为。

## 落地顺序(TDD,每动作一提交)

按"纯 CI 可测 → 需真实环境" + 依赖增量隔离递增:

1. **archive.rs**(+`zip`):`archive.zip` → `archive.unzip`。
2. **http.rs 扩展**(无新依赖):`http.download` → `http.upload` → `http.request` 增 `max_bytes`。
3. **notify.rs**(+`hmac`):`notify.send`(先 text 四 provider → 再钉钉/飞书加签)。
4. **clipboard.rs**(+`arboard`,风险最高放后):**先验交叉编译/许可** → `clipboard.set` → `clipboard.get`。
5. **browser.rs 扩展**(无新依赖):`browser.wait`。

每步循环:加依赖 → 写失败测试(watch fail)→ 最小实现 → `cargo test -p lumo-actions <module>` → `cargo clippy -p lumo-actions --all-targets -- -D warnings` → `cargo fmt -p lumo-actions` → 提交。全部完成后整体 code review + 更新路线图 F-5/F-7/F-8/F-9/F-11 为 `[x]`。

## 风险与缓解

- **arboard 交叉编译(最高)**:P1-9 的 `aarch64-unknown-linux-gnu` 为 `cross-check` 硬门禁(`cargo check --workspace --exclude lumorpa-desktop`)。arboard Linux 后端拉 x11rb/wayland 系传递依赖,可能 ① 拖红 cross-check,② 引入非白名单许可。
  - **缓解(计划内独立任务,带回退分支)**:在 clipboard 步**先**加 arboard 跑 `cargo check`(本机)+ 人工核许可面;若交叉/许可有问题,改为 `[target.'cfg(not(...))'.dependencies] arboard` 并用 `#[cfg]` 在不支持目标上给 clipboard 动作一个返回 "clipboard unavailable on this target" 的运行时桩(动作**始终注册**以保 schema 一致)。
- **notify 加签格式**:钉钉/飞书签名以官方文档为准;wiremock 仅能验"sign 字段存在 + 可复算",真实推送留手动 e2e。
- **zip-bomb**:`max_total_bytes` 缺省 1 GiB 兜底。
- **download/upload OOM**:`max_bytes` 缺省 100 MiB;download 流式不全量入内存,upload body 读入受 `max_bytes` 限制。

## 验收标准

- 6 个新动作 + `http.request` `max_bytes` 增强全部实现并经真实 `ActionRegistry` 测试(与 VM 同路径)。
- 能力沙箱:越权访问(无 fs/network cap)一律被拒,有对应测试。
- 安全:zip-slip/zip-bomb 被拒;notify `secret` 不泄漏;均有测试或审计说明。
- `cargo test -p lumo-actions` 全绿;`cargo clippy -p lumo-actions --all-targets -- -D warnings` 无警告;`cargo fmt -p lumo-actions -- --check` 干净。
- 新依赖传递许可经核对仍满足 deny.toml;arboard 交叉编译风险已验证或已回退处理。
- 路线图 F-5/F-7/F-8/F-9/F-11 标记 `[x]`。
