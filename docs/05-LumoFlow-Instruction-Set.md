# LumoFlow Instruction Set

This document is the runtime-facing reference for the current LumoRPA flow
instruction set. It describes the YAML shape, step semantics, capability gates,
and action ids exposed by the default CLI registry.

## Flow Shape

```yaml
apiVersion: lumorpa.io/v1
kind: Flow
metadata:
  id: my-flow
  version: 0.1.0
  name: My Flow
  description: Optional description
  tags: [example]
spec:
  inputs:
    - { name: who, type: string, default: world }
  outputs: []
  vault: []
  capabilities: {}
  steps:
    - id: greet
      action: control.log
      with:
        message: "hello {{ inputs.who }}"
```

Required top-level fields are `apiVersion`, `kind`, `metadata`, and `spec`.
`apiVersion` must be `lumorpa.io/v1`; `kind` must be `Flow`.

Each step supports:

| Field | Purpose |
| --- | --- |
| `id` | Unique step id across the whole flow, including nested blocks. |
| `action` | Registered action id, such as `file.read` or `browser.click`. |
| `with` | Action input object validated against the action JSON schema. |
| `bind` | Optional variable name that receives this step's output. |
| `when` | Optional predicate. Falsy values skip the step. |
| `retry` | Optional retry policy: `times`, `backoff` (`fixed` \| `exponential`), `initial_ms`, `on` (error kinds: `selector_not_found`, `extract_failed`, `cond_error`, `capability_denied`, `budget_exceeded`, `timeout`, `other`). 注意:步级超时**仅当 `on` 显式列出 `timeout`** 时才会重试——空 `on` 的"任意错误都重试"不含超时,此时超时仍是硬中断(整个运行立即终止);重试预算耗尽后,最后一次超时仍按 `timeout` 状态落库并终止运行。 |
| `ai` | Optional AI hook policy: `mode: off`, `fallback`, or `primary`. |
| `resource` | Optional name of a declared `spec.resources` entry to bind this step to (see [Resources](#resources)). |
| `do`, `else`, `catch`, `finally`, `branches` | Nested control blocks for `control.*` actions. |

## Templates

String values in `with` are rendered with the flow context before action
execution. A whole-field lookup preserves the original JSON type:

```yaml
with:
  items: "{{ inputs.rows }}"
  message: "processed {{ steps.count.result }} rows"
```

Available namespaces are:

| Namespace | Meaning |
| --- | --- |
| `inputs.*` | Flow inputs after defaults are merged. |
| `steps.<id>.result` | Prior step output. |
| `vars.*` | Values set by `control.set_var` or `bind`. |
| `env.*` | Environment variables. |
| loop bindings | `index`, `row`, `item`, or a custom `bind` from loop input. |
| `vault.*` | Secret placeholders resolved just before action dispatch. |

Vault expressions render as placeholders such as `${{ vault.smtp.password }}` so
secrets do not appear in rendered step snapshots.

## Capabilities

Flows are deny-by-default for side effects. Declare only the permissions a flow
needs:

```yaml
spec:
  capabilities:
    fs.read: ["./input/**"]
    fs.write: ["./output/**"]
    network: ["example.com"]
    llm: ["*"]
    mcp: ["local-tools"]
    desktop: ["mouse", "keyboard", "screen", "window"]
```

Common gates:

| Capability | Used by |
| --- | --- |
| `fs.read` | `file.read`, `file.exists`, `file.list`, `file.metadata`, `file.copy`, `file.move`, `file.rename`, `csv.read`, `excel.read_rows`, `excel.read_cell`, `excel.sheet_names`, `pdf.*`, uploads, archive inputs, `image.locate`, `image.compare`, `image.ocr`, `excel.read_range`, `docx.read_text`, `excel.set_style`, `excel.merge_cells`, `excel.set_column_width`, `excel.set_row_height`, `excel.freeze_panes`, `excel.add_chart`, `excel.set_conditional_format`, `excel.autofit_columns`, `excel.set_comment`, `excel.set_data_validation`. |
| `fs.write` | `file.write`, `file.mkdir`, `file.copy`, `file.move`, `file.rename`, `file.delete`, `csv.write`, `excel.write_row`, `excel.write_cell`, downloads, archive outputs, screenshots, `pdf.write`, `excel.write_range`, `file.append`, `docx.replace_placeholders`, `excel.set_style`, `excel.merge_cells`, `excel.set_column_width`, `excel.set_row_height`, `excel.freeze_panes`, `excel.add_chart`, `excel.set_conditional_format`, `excel.autofit_columns`, `excel.set_comment`, `excel.set_data_validation`, `email.fetch` attachment saving (`save_attachments_to`). |
| `network` | HTTP, browser, email, notification, FTP/S3, PostgreSQL/MySQL, MCP network targets. |
| `llm` | `ai.chat`, `image.ocr`, `desktop.click_text` (OCR goes through the LLM provider), and AI hook modes. |
| `mcp` | MCP tool calls. |
| `desktop` | Optional `desktop.*` / `window.*` actions when compiled with the `desktop` feature. Categories: `mouse` (move/click/scroll), `keyboard` (key/type), `screen` (`desktop.screenshot`), `window` (`window.list` / `window.activate` / `window.bounds`). `desktop.click_text` needs both `screen` and `mouse` (only `screen` with `dry_run: true`), plus `llm` for the OCR call. The screenshot destination path is additionally gated by `fs.write`. |

## Resources

A flow can declare long-lived **resources** under `spec.resources` — a browser, a
database connection, an HTTP client, an SMTP/FTP session — that are **opened once
and reused** by every step that binds them, then torn down together at run end (on
success, failure, or cancel). This is the "启动一次 / start once" model: a login or
connection is paid for once per run instead of once per step.

```yaml
spec:
  resources:
    browser:                  # an arbitrary name you reference from steps
      kind: chromium.cdp
      profile: stealth-default
      headless: true
    orders:
      kind: sqlite
      path: "./output/orders.db"
  steps:
    - id: open
      action: browser.open
      resource: browser        # bind this step to the `browser` resource
      with: { url: "https://example.com/orders" }
    - id: save
      action: db.sqlite_exec
      resource: orders          # bind to `orders`; `db:` comes from the decl
      with: { sql: "INSERT INTO orders(id) VALUES ('A-1')" }
```

**Binding.** A step's optional `resource:` field names one declared resource. An
undeclared name is a hard validation error. A step with no `resource:` keeps its
prior per-step behavior exactly — resources are opt-in and fully back-compatible.

**Lifecycle.** A resource opens **lazily** on the first step that binds it (an
unused resource never opens), is reused by later bound steps, and is closed at run
end. Each run gets its own instances (keyed by run id), so concurrent runs never
share a handle.

### Resource kinds

| `kind` | Used by | Reused handle | Config (in the decl) |
| --- | --- | --- | --- |
| `chromium.cdp` | `browser.*` | One Chrome session (tabs, cookies, pages) | `headless` (bool); `proxy` (string, passed to Chrome as `--proxy-server`); `user_agent` (string, overrides the browser UA via `--user-agent`); `profile` (see below) |
| `sqlite` | `db.sqlite_*` | One SQLite connection | `path` (the db file) |
| `postgres` | `db.postgres_*` | One pooled PostgreSQL connection | — DSN/credentials come from the step (see below) |
| `mysql` | `db.mysql_*` | One pooled MySQL connection | — DSN/credentials come from the step (see below) |
| `http` | `http.*` | One pooled HTTP client (keep-alive) | `timeout_ms` (optional); `proxy` (optional http/https/socks5 URL — the proxy URL itself is gated by the `network` capability, like any target URL) |
| `smtp` | `email.send` | One pooled SMTP transport | — host/credentials come from the step (see below) |
| `imap` | `email.fetch`, `email.mark`, `email.move` | One authenticated IMAP session | — host/credentials come from the first bound step |
| `ftp` | `ftp.*` | One authenticated FTP session | — host/credentials come from the step (see below) |
| `xlsx` | `excel.*` | One shared workbook path for the run | `path` (the workbook file); bound actions may omit `file` |

A bound step omits the per-call field it would otherwise pass: a `sqlite`-bound
`db.sqlite_*` step omits `db:` (the decl `path` wins); a `chromium.cdp`-bound
`browser.open` omits launch options (`headless`/`profile` come from the decl);
an `xlsx`-bound `excel.*` step omits `file`. IMAP credentials are supplied by
the first bound step and the authenticated session is then reused by later
`email.fetch` / `email.mark` / `email.move` steps. `s3.*` remains per-call.

### Profiles

`profile: <name>` is a resource sub-field. Today it is meaningful only for
`chromium.cdp`, where it makes the browser **persistent**: the named profile maps
to a stable Chrome user-data directory under `$LUMO_HOME/browser-profiles/<name>`,
so cookies, logins, and localStorage survive across runs. A persistent profile
also gets a minimal **stealth baseline** — it drops the `navigator.webdriver`
automation signal (`--disable-blink-features=AutomationControlled`) and the
default-browser prompt. Because Chrome locks the directory, **one profile can be
used by only one run at a time**. Deeper anti-detection (UA / fingerprint patches)
is out of scope for the baseline. For the other kinds, `profile` is reserved (no
effect today).

### Security and capabilities

- **Per-step capability gating is unchanged.** Binding a resource relaxes no gate —
  every step still checks `network` / `fs.read` / `fs.write` as before.
- **Resource config is static YAML and is never template-rendered.** `{{ … }}` and
  `${{ vault.… }}` do **not** resolve inside a decl — never put secrets there.
  Credential-bearing kinds (`smtp`, `imap`, `ftp`, `postgres`, `mysql`) take their host and
  credentials from the **first bound step's** inputs (which *are* vault-resolved),
  not from the decl. (`postgres`/`mysql` take a `dsn` such as
  `postgres://user:pass@host:5432/db`; its host is gated by `network`.)
- **The browser profile directory is app-managed infrastructure** (under
  `$LUMO_HOME`, like `lumo.db` / `selector-stats.json`), so it is not a
  user-supplied path and is not subject to `fs.write` gating — exactly as Chrome's
  default temporary profile isn't.

See `examples/order-export.lumoflow.yaml` for a flow that reuses both a persistent
browser and a shared SQLite connection.

## Control Blocks

`control.if` uses `do` and optional `else`.

```yaml
- id: gate
  action: control.if
  with: { cond: "inputs.count > 0" }
  do:
    - { id: positive, action: control.log, with: { message: ok } }
  else:
    - { id: empty, action: control.log, with: { message: empty } }
```

`control.for` and `control.for_each` require `do`. `control.try` supports
`do`, `catch`, and `finally`. `control.parallel` accepts either `branches` or
`do`, where `branches` is a list of step sequences.

`control.while` repeats its `do` block while `cond` (evaluated each round with
the same F-14 evaluator as `control.if`) stays truthy. The binding `index`
exposes the round number (from 0). `max_iterations` (default 1000) is a
runaway-loop guard: hitting it with `cond` still true fails the step.

```yaml
- id: poll
  action: control.while
  with: { cond: "!vars.ready", max_iterations: 60 }
  do:
    - { id: probe, action: http.request, with: { url: "https://api.example.com/status" }, bind: st }
    - id: gate
      action: control.if
      with: { cond: "st.status == 200" }
      do:
        - { id: ok, action: control.set_var, with: { name: ready, value: true } }
    - { id: wait, action: control.sleep, with: { ms: 1000 } }
```

`control.break` exits the nearest enclosing loop (`while`/`for`/`for_each`)
and `control.continue` skips to its next iteration; both pass through
`control.try` uncaught (`finally` still runs). They must appear inside a loop
ancestor's `do:` chain — `control.parallel` branches are separate scopes, so a
break/continue inside a branch needs a loop within that same branch (validated
statically, with a runtime error as backstop).

## Action Families

Use `cargo run -p lumo-cli -- actions` to print the registry and
`cargo run -p lumo-cli -- actions --show <id>` to inspect the input schema.

<!-- ACTIONS_START -->
| Family | Actions |
| --- | --- |
| AI | `ai.chat` |
| Archive | `archive.zip`, `archive.unzip` |
| Browser | `browser.launch`, `browser.close`, `browser.open`, `browser.back`, `browser.forward`, `browser.reload`, `browser.click`, `browser.type`, `browser.extract`, `browser.wait`, `browser.info`, `browser.eval`, `browser.screenshot`, `browser.scroll`, `browser.hover`, `browser.select`, `browser.cookies`, `browser.set_cookie`, `browser.tabs`, `browser.tab`, `browser.upload`, `browser.download_wait`, `browser.dialog`, `browser.frame`, `browser.extract_table`, `browser.drag_and_drop`, `browser.print_pdf`, `browser.wait_response` |
| Clipboard | `clipboard.get`, `clipboard.set` |
| Control | `control.log`, `control.set_var`, `control.sleep`, `control.if`, `control.for`, `control.for_each`, `control.while`, `control.break`, `control.continue`, `control.try`, `control.parallel`, `control.fail` |
| CSV | `csv.parse`, `csv.stringify`, `csv.read`, `csv.write` |
| Data | `data.json_parse`, `data.json_format`, `data.filter`, `data.group_by`, `data.join`, `data.dedup`, `data.sort_multi` |
| Date | `date.now`, `date.parse`, `date.format`, `date.add`, `date.diff`, `date.weekday`, `date.workday_add` |
| Database | `db.sqlite_query`, `db.sqlite_exec`, `db.sqlite_batch`, `db.postgres_query`, `db.postgres_exec`, `db.postgres_batch`, `db.mysql_query`, `db.mysql_exec`, `db.mysql_batch` |
| DOCX | `docx.read_text`, `docx.replace_placeholders` |
| Email | `email.send`, `email.fetch`, `email.mark`, `email.move` |
| Excel | `excel.read_rows`, `excel.write_row`, `excel.sheet_names`, `excel.read_cell`, `excel.write_cell`, `excel.read_range`, `excel.write_range`, `excel.find_replace`, `excel.set_formula`, `excel.set_style`, `excel.merge_cells`, `excel.set_column_width`, `excel.set_row_height`, `excel.freeze_panes`, `excel.add_chart`, `excel.set_conditional_format`, `excel.autofit_columns`, `excel.set_comment`, `excel.set_data_validation`, `excel.lookup`, `excel.add_sheet`, `excel.delete_sheet`, `excel.rename_sheet`, `excel.insert_rows`, `excel.delete_rows`, `excel.insert_columns`, `excel.delete_columns` |
| File | `file.read`, `file.write`, `file.exists`, `file.list`, `file.mkdir`, `file.copy`, `file.move`, `file.rename`, `file.delete`, `file.metadata`, `file.append`, `file.wait` |
| Flow | `flow.call` |
| FTP/S3 | `ftp.upload`, `ftp.download`, `s3.put`, `s3.get` (verb aliases: `ftp.put`, `ftp.get`, `s3.upload`, `s3.download`) |
| Hash/Utility | `hash.sha256`, `hash.sha512`, `hash.sha1`, `hash.md5`, `util.base64_encode`, `util.base64_decode`, `util.url_encode`, `util.url_decode`, `util.uuid` |
| HTTP | `http.request`, `http.download`, `http.upload`, `http.oauth2_token`, `http.paginate` |
| Human | `human.input`, `human.confirm`, `human.approve` |
| Image | `image.locate`, `image.compare`, `image.ocr` |
| JSON | `json.get`, `json.set`, `json.merge`, `json.keys`, `json.values`, `json.delete` |
| List | `list.length`, `list.append`, `list.sort`, `list.unique`, `list.range`, `list.contains`, `list.get`, `list.slice`, `list.reverse`, `list.pluck` |
| Math | `math.round`, `math.random`, `math.min`, `math.max`, `math.sum`, `math.avg`, `math.abs` |
| MCP | `mcp.call`, `mcp.discover` |
| Notification | `notify.send`, `notify.dingtalk`, `notify.feishu`, `notify.wecom` |
| PDF | `pdf.extract_text`, `pdf.info`, `pdf.write` |
| Regex | `regex.match`, `regex.find_all`, `regex.replace`, `regex.captures` |
| Skill | `skill.invoke` |
| String | `string.upper`, `string.lower`, `string.trim`, `string.length`, `string.split`, `string.join`, `string.replace`, `string.contains`, `string.starts_with`, `string.ends_with`, `string.substring`, `string.repeat`, `string.pad_left`, `string.pad_right`, `string.format`, `string.encode_convert` |
| System | `system.shell`, `system.env_get`, `system.sleep`, `system.platform`, `system.process_list`, `system.process_kill`, `system.app_start` |
| XML | `xml.parse`, `xml.build`, `xml.xpath` |
<!-- ACTIONS_END -->

### Cross-action contracts

- Stable retry kinds include `timeout`, `io`, and `network` in addition to the
  selector/condition/capability kinds. Action-internal deadlines return
  `timeout`; local filesystem/process failures return `io`; remote transport or
  protocol failures return `network`. These names are accepted by `retry.on`.
- Collection actions are bounded by default. `file.list`, `excel.read_rows`,
  and `browser.extract_table` accept a positive `limit` (default 1000) and
  return `count`, `limit`, and `truncated` metadata. The latter two return their
  records under `rows`; callers should not expect the former bare-array shape.
- Destructive actions support validation-only previews through `dry_run: true`:
  `file.delete`, `db.sqlite_exec`, all database batch/remote exec mutations,
  `system.process_kill`, and `email.send`. Capability checks and input
  validation still run, but no file, database, process, SMTP connection, or
  remote database connection is mutated/opened for the operation.
- Email actions and blocking Excel/PDF/DOCX actions accept `timeout_ms`
  (default 60,000). Excel timeouts also trip the current attempt's cooperative
  interrupt, so orphaned blocking work cannot pass its write-back checkpoint.
  Human actions retain their longer action-specific defaults and hard
  ceiling. `control.parallel.with.max_concurrency` and parallel
  `control.for_each` default to 8 while preserving deterministic result order;
  `control.for_each` remains sequential unless `parallel: true` is set.

`data.json_parse` / `data.json_format` are the document-oriented parse/format
helpers; `json.get` / `json.set` / `json.merge` / `json.keys` / `json.values` /
`json.delete` manipulate an already parsed JSON value. They overlap by history
but are not silent aliases. Transfer verbs historically diverged per backend —
`ftp.upload` ≈ `s3.put` ≈ `http.upload`, `ftp.download` ≈ `s3.get` ≈
`http.download` — so the ftp/s3 families now also register the opposite verb as
a true alias: `ftp.put` / `ftp.get` and `s3.upload` / `s3.download` share the
exact inputs, outputs, capability gates, and implementation of their targets
(`ftp.upload` / `ftp.download` / `s3.put` / `s3.get`). Use whichever verb reads
best; `http.*` keeps only `upload` / `download` (its `put`/`get` are HTTP
methods on `http.request`).

Hash actions accept exactly one of `text` or `path`; file hashing requires
`fs.read`. `archive.zip` / `archive.unzip` accept an optional `password` and use
WinZip AES-256 when it is present.

`util.url_encode` percent-encodes text — by default with `encodeURIComponent`
semantics (structure characters like `/?&=#` are escaped; spaces become `%20`);
pass `component: false` for `encodeURI` semantics (URL structure characters are
preserved, so a whole URL can be encoded without breaking it). `util.url_decode`
reverses percent encoding; `+` is left as-is (treating `+` as a space is form
encoding, not URL decoding).

`db.sqlite_batch` / `db.postgres_batch` / `db.mysql_batch` run their
`statements` array (each `{sql, args}` for sqlite, `{sql, params}` for
postgres/mysql) inside **one explicit transaction**: every statement must
succeed for the batch to commit; any failure — a bad statement, a run cancel
mid-batch, or (postgres/mysql) a `timeout_ms` expiry — rolls the whole batch
back, so nothing is partially written. Output is the per-statement
`rows_affected` array plus `total_affected`. The postgres/mysql variants take
the same `dsn` / resource binding and pass the same `network` capability gate
as `db.postgres_exec` / `db.mysql_exec`.

`excel.set_formula` writes the formula string but does **not** evaluate it —
the workbook carries no cached result, and the calculation only happens when
the file is opened in Excel/WPS. Reading that cell back with `excel.read_cell`
/ `excel.read_rows` therefore yields the stored formula text or an empty
value, never the computed result.

### HTTP proxy and mTLS

Every `http.*` action accepts an optional per-step `proxy` (an `http://` /
`https://` / `socks5://` URL) and `mtls: {cert_path, key_path}` (paths to a
PEM client certificate and its PEM private key, concatenated internally and
fed to the rustls client — no native TLS involved). The proxy URL is itself
subject to the `network` capability gate, and the certificate / key paths
are gated by `fs.read` before they are read — routing through a proxy never
relaxes the SSRF checks on the target URL or on redirect hops. A step
carrying `proxy` or `mtls` builds its own dedicated client: these are
step-private settings, never folded into a shared `http` resource client.
An `http` resource decl may also declare `proxy` (see the resource table)
so every bound step routes through it; mTLS stays per-step only, because
the certificate paths must pass the step's `fs.read` capability gate, which
lives on the step context rather than in the declaration.

### Human-in-the-loop (`human.input` / `human.confirm` / `human.approve`)

`human.input` asks the operator for a line of text and returns `{value}`;
`human.confirm` asks a yes/no question and returns `{confirmed}`;
`human.approve` optionally sends an approval notification first (its `notify`
field takes the exact `notify.send` input shape and passes through the same
`network` capability gate), then waits for the host's verdict and returns
`{approved, by, comment}`. All three wait on a host-injected prompt channel —
the engine itself has no UI.

Waiting semantics:

- Each action takes `timeout_ms` (default 3,600,000 = 1 hour). On timeout,
  `human.input` / `human.confirm` fall back to their optional `default`;
  without a `default` — and always for `human.approve`, which never
  auto-approves — the step fails with a timeout error (`retry.on: [timeout]`
  applies as usual).
- The host's global per-step timeout (`LUMO_STEP_TIMEOUT_MS`; the desktop
  host defaults it to 10 minutes) no longer cuts a human wait short: for
  `human.*` steps the VM raises the effective step ceiling to
  max(global step timeout, the step's `timeout_ms`) plus a small grace, so
  the action's own timeout semantics above (the `default` fallback / the
  approve timeout error) always resolve first. Other steps keep the global
  ceiling unchanged.
- Cancelling the run interrupts the wait immediately.
- Hosts without an interactive channel fail the step immediately with
  "host does not support human interaction" — a prompt never hangs silently.

Host support matrix:

| Host | Channel | Notes |
| --- | --- | --- |
| CLI (`lumo run`) | stderr prompt + stdin line | requires a TTY; piped/CI stdin fails the step immediately. Empty answer takes `default` when present; confirm/approve parse y/yes/n/no. |
| Desktop (Studio) | `human-prompt` event + `human_respond` command | frontend listens for `human-prompt` `{promptId, kind, message, default, timeoutMs, runId, stepPath}` and answers with `human_respond(promptId, value)`, where `value` is a string (input), bool (confirm/approve), or `{approved, by?, comment?}` (approve). |
| `lumo serve` / MCP / sub-flows (`skill.invoke`, `flow.call`) | not supported | the step errors immediately; webhook-based approve callbacks are a planned follow-up. |

`human.*` needs no capability grant of its own (it touches nothing outside
the host UI); only the `notify` part of `human.approve` is gated, by the
`network` capability, exactly like `notify.send`.

### Process control

`system.process_kill` terminates a process by `pid` — graceful by default
(SIGTERM on Unix; `force: true` sends SIGKILL; Windows has no graceful
termination signal, so both paths use TerminateProcess). It refuses to kill
the Lumo process itself (and pid 0), and killing a non-existent pid is an
explicit error, never a silent success. `system.app_start` launches an
external application detached (`program` + `args`, optional `cwd`), returning
the child `pid` without waiting for exit; on macOS a `program` ending in
`.app` is launched via `open -a` (the returned pid is the launcher's — pass
the bundle's inner executable path when the exact pid matters). Both are
implemented for macOS / Windows / Linux and gated behind
`LUMO_ALLOW_PROCESS=1` — the same opt-in tier as `system.shell`'s
`LUMO_ALLOW_SHELL=1`, but a separate switch: allowing shell does not imply
allowing process control, and vice versa.

### Screenshot / PDF artifacts

`browser.screenshot`, `desktop.screenshot`, and `browser.print_pdf` additionally
archive their output as a **run-level artifact** and include an `artifact_id`
field in the step output (the id referencing the run's artifacts store, for
replay/observability). When the host runs without an artifacts directory (e.g.
the CLI's `--no-store` path), archiving is a harmless no-op and `artifact_id`
is `null` — artifact archiving never fails the action itself.

The optional `desktop` feature adds `desktop.move`, `desktop.click`,
`desktop.drag`, `desktop.scroll`, `desktop.key`, `desktop.type`, plus native screen capture and
window management: `desktop.screenshot` (full-screen or `region` capture of a
`display` to a PNG `path` — the path is gated by `fs.write`, output
`{path, width, height}`), `window.list` (visible windows with
`{id, title, app, x, y, width, height, minimized}`), `window.activate` (bring a
window to the foreground by `id` or `title_contains`; multiple matches use the
first and report `matched`), and `window.bounds` (read a window's geometry,
optionally moving/resizing it via `set: {x, y, width, height}`), and
`window.close` / `window.minimize` / `window.maximize` for first-class window
lifecycle control. Window-targeting misses (`id` / `title_contains` matching no
window) fail with `selector_not_found`, and OS-level window operation failures
(missing Accessibility/`wmctrl` support, denied permission) fail with `io`, so
`retry.on: [selector_not_found]` waits for a window to appear without retrying
hard platform errors.

`desktop.click_text` clicks on-screen text located via OCR: it captures the
screen (same `region` / `display` options as `desktop.screenshot`, kept
in-memory — no `fs.write` needed), runs the screenshot through the same
LLM-backed OCR pathway as `image.ocr` (so it needs the `llm` capability and a
configured AI provider; a bounding-box-capable vision model is required — a
plain-text OCR preset yields an explicit error), then converts the matched
text's bbox center from screenshot pixels to screen coordinates (HiDPI/Retina
scaling is derived from the capture itself, so coordinates stay correct on
scaled displays) and clicks it through the `desktop.click` path. Inputs:
`text` (required), `match` (`contains` — default, case-insensitive substring —
or `exact`), `index` (which of multiple matches to click, default 0), `region`
/ `display`, `button` / `double` (as in `desktop.click`), `model` (as in
`image.ocr`), and `dry_run: true` to only locate — returning the bbox and the
converted coordinates without clicking (and requiring only the `screen`
category instead of `screen` + `mouse`). Output:
`{clicked, x, y, matched_text, matches, bbox}`. When no text matches (or
`index` is out of range) the step fails with `selector_not_found`, so
`retry.on: [selector_not_found]` gives you "wait until the text appears, then
click" for free.

Window-management platform support:

| Capability | macOS | Windows | Linux |
| --- | --- | --- | --- |
| `desktop.screenshot` | ✅ CoreGraphics | ✅ GDI | ✅ X11 / Wayland portal |
| `window.list` | ✅ | ✅ | ✅ X11 (limited on Wayland) |
| `window.activate` | ✅ osascript (Accessibility grant) | ✅ SetForegroundWindow | ⚠️ requires `wmctrl` (X11 only) |
| `window.bounds` read | ✅ | ✅ | ✅ X11 |
| `window.bounds` `set` | ✅ osascript (Accessibility grant) | ✅ MoveWindow | ⚠️ requires `wmctrl` (X11 only) |

Platforms that cannot perform an operation fail with an explicit
"not supported on this platform" error instead of silently succeeding. On
macOS, screen capture and window titles need the Screen Recording permission;
`window.activate` / `window.bounds set` need the Accessibility permission.

`image.ocr` can use either the active cloud OCR/vision model or a local
ModelScope OCR preset configured in `ocr_model` (for example
`modelscope/ZhipuAI/GLM-OCR`, `modelscope/PaddlePaddle/PaddleOCR-VL-1.6`, or
`modelscope/deepseek-ai/DeepSeek-OCR-2`). The desktop Models page lists the
supported OCR presets and can download them into the local model cache.

### Driving iframes (`frame:` on browser actions)

`browser.extract`, `browser.eval`, `browser.click`, and `browser.type` take an
optional `frame:` object addressing an `<iframe>`: `url_includes` (first frame
whose URL contains the substring), `name` (exact frame name), or `index`
(zero-based position in the page's frame list). `browser.frame` exposes the
same addressing as a standalone eval/extract action. Omitting `frame:` keeps
the action on the main frame, exactly as before.

For `browser.click` / `browser.type` the element is located **inside the
frame** (same strategy order as the main-frame resolver: id, data_testid, css,
aria_label, text_includes, xpath — the winner is reported in `resolved_by`,
plus `frame: true`), its rect center is translated to top-level viewport
coordinates by accumulating each ancestor `<iframe>`'s offset, and the click /
keystrokes are then dispatched as **real top-level CDP input** — the page sees
genuine trusted events (`isTrusted: true`), not synthetic in-frame JS events.
`browser.type` focuses the element with a real click first; `clear: true`
empties the field in-frame before typing. A selector that matches nothing
inside the frame fails with `selector_not_found`, so
`retry.on: [selector_not_found]` works unchanged.

Cross-origin limitation: translating in-frame coordinates walks
`window.frameElement`, which browsers only permit across **same-origin**
frame chains. A cross-origin iframe (e.g. a third-party payment widget) fails
with an explicit error; drive those with in-frame JS via `browser.frame`
(`op: eval`) or `browser.eval` + `frame:` instead — noting that such JS
dispatches synthetic (`isTrusted: false`) events, and that fully site-isolated
(out-of-process) frames may be unreachable even for the JS bridge. The vision
fallback (`prompt:`) applies only to main-frame resolution and is skipped when
`frame:` is set.

### XML conventions (`xml.parse` / `xml.build` / `xml.xpath`)

`xml.parse` and `xml.build` are inverse mappings sharing one convention:
attributes become `@attr` keys, text content becomes `#text`, repeated
sibling elements of the same name collapse into an array, CDATA merges into
text verbatim, namespace prefixes are kept verbatim in key names
(`soap:Body` is the literal key, `xmlns:*` declarations are ordinary `@`
attributes), leaf elements with no attributes fold to a plain string and
empty elements to `null`. The root element is wrapped in a single-key
object. Round-trip holds as `parse(build(parse(x))) == parse(x)`.

Parsing never resolves DTDs or external entities (quick-xml only expands
the five predefined entities and numeric character references), so XXE is
impossible by construction; inputs are capped by `max_bytes`
(default 10 MiB). `xml.build` accepts `declaration` (default `true`) and
`indent` (default `false`, compact). `xml.xpath` evaluates full XPath 1.0
(sxd-xpath, pure Rust) and returns `matches` + `count`; element matches are
serialized XML fragments (without re-emitting `xmlns` declarations), and
documents with namespaces require a `namespaces` prefix→URI map (or use
`local-name()` in the expression).

## Validation Checklist

Before shipping a flow:

1. Run `cargo run -p lumo-cli -- validate path/to/flow.lumoflow.yaml`.
2. Run `cargo run -p lumo-cli -- actions --show <action>` for every unfamiliar
   action and compare its required `with` fields.
3. Declare capability grants for every filesystem, network, LLM, MCP, or desktop
   operation.
4. Keep step ids unique, including nested control blocks.
5. Prefer `bind` for intermediate outputs that will be reused by later steps.
