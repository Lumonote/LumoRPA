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
| `retry` | Optional retry policy: `times`, `backoff` (`fixed` \| `exponential`), `initial_ms`, `on` (error kinds: `selector_not_found`, `extract_failed`, `cond_error`, `capability_denied`, `budget_exceeded`, `other`). 注意:步级超时由 VM 在重试循环外层强制执行,**不可被 retry 捕获**——`on` 里没有 `timeout`。 |
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
| `llm` | `ai.chat`, `image.ocr`, and AI hook modes. |
| `mcp` | MCP tool calls. |
| `desktop` | Optional `desktop.*` / `window.*` actions when compiled with the `desktop` feature. Categories: `mouse` (move/click/scroll), `keyboard` (key/type), `screen` (`desktop.screenshot`), `window` (`window.list` / `window.activate` / `window.bounds`). The screenshot destination path is additionally gated by `fs.write`. |

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
| `http` | `http.*` | One pooled HTTP client (keep-alive) | `timeout_ms` (optional) |
| `smtp` | `email.send` | One pooled SMTP transport | — host/credentials come from the step (see below) |
| `ftp` | `ftp.*` | One authenticated FTP session | — host/credentials come from the step (see below) |

A bound step omits the per-call field it would otherwise pass: a `sqlite`-bound
`db.sqlite_*` step omits `db:` (the decl `path` wins); a `chromium.cdp`-bound
`browser.open` omits launch options (`headless`/`profile` come from the decl).
(The IMAP actions `email.fetch`/`email.mark`/`email.move` and `s3.*` are not
resource-backed and behave as before.)

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
  Credential-bearing kinds (`smtp`, `ftp`, `postgres`, `mysql`) take their host and
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
| Browser | `browser.launch`, `browser.close`, `browser.open`, `browser.click`, `browser.type`, `browser.extract`, `browser.wait`, `browser.info`, `browser.eval`, `browser.screenshot`, `browser.scroll`, `browser.hover`, `browser.select`, `browser.cookies`, `browser.set_cookie`, `browser.tabs`, `browser.tab`, `browser.upload`, `browser.download_wait`, `browser.dialog`, `browser.frame`, `browser.extract_table`, `browser.drag_and_drop`, `browser.print_pdf`, `browser.wait_response` |
| Clipboard | `clipboard.get`, `clipboard.set` |
| Control | `control.log`, `control.set_var`, `control.sleep`, `control.if`, `control.for`, `control.for_each`, `control.while`, `control.break`, `control.continue`, `control.try`, `control.parallel`, `control.fail` |
| CSV | `csv.parse`, `csv.stringify`, `csv.read`, `csv.write` |
| Data | `data.json_parse`, `data.json_format`, `data.filter`, `data.group_by`, `data.join`, `data.dedup`, `data.sort_multi` |
| Date | `date.now`, `date.parse`, `date.format`, `date.add`, `date.diff`, `date.weekday`, `date.workday_add` |
| Database | `db.sqlite_query`, `db.sqlite_exec`, `db.sqlite_batch`, `db.postgres_query`, `db.postgres_exec`, `db.mysql_query`, `db.mysql_exec` |
| DOCX | `docx.read_text`, `docx.replace_placeholders` |
| Email | `email.send`, `email.fetch`, `email.mark`, `email.move` |
| Excel | `excel.read_rows`, `excel.write_row`, `excel.sheet_names`, `excel.read_cell`, `excel.write_cell`, `excel.read_range`, `excel.write_range`, `excel.find_replace`, `excel.set_formula`, `excel.set_style`, `excel.merge_cells`, `excel.set_column_width`, `excel.set_row_height`, `excel.freeze_panes`, `excel.add_chart`, `excel.set_conditional_format`, `excel.autofit_columns`, `excel.set_comment`, `excel.set_data_validation`, `excel.lookup` |
| File | `file.read`, `file.write`, `file.exists`, `file.list`, `file.mkdir`, `file.copy`, `file.move`, `file.rename`, `file.delete`, `file.metadata`, `file.append`, `file.wait` |
| Flow | `flow.call` |
| FTP/S3 | `ftp.upload`, `ftp.download`, `s3.put`, `s3.get` |
| Hash/Utility | `hash.sha256`, `hash.sha512`, `hash.sha1`, `hash.md5`, `util.base64_encode`, `util.base64_decode`, `util.url_encode`, `util.url_decode`, `util.uuid` |
| HTTP | `http.request`, `http.download`, `http.upload`, `http.oauth2_token`, `http.paginate` |
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
| System | `system.shell`, `system.env_get`, `system.sleep`, `system.platform`, `system.process_list` |
| XML | `xml.parse`, `xml.build`, `xml.xpath` |
<!-- ACTIONS_END -->

`util.url_encode` percent-encodes text — by default with `encodeURIComponent`
semantics (structure characters like `/?&=#` are escaped; spaces become `%20`);
pass `component: false` for `encodeURI` semantics (URL structure characters are
preserved, so a whole URL can be encoded without breaking it). `util.url_decode`
reverses percent encoding; `+` is left as-is (treating `+` as a space is form
encoding, not URL decoding).

The optional `desktop` feature adds `desktop.move`, `desktop.click`,
`desktop.scroll`, `desktop.key`, `desktop.type`, plus native screen capture and
window management: `desktop.screenshot` (full-screen or `region` capture of a
`display` to a PNG `path` — the path is gated by `fs.write`, output
`{path, width, height}`), `window.list` (visible windows with
`{id, title, app, x, y, width, height, minimized}`), `window.activate` (bring a
window to the foreground by `id` or `title_contains`; multiple matches use the
first and report `matched`), and `window.bounds` (read a window's geometry,
optionally moving/resizing it via `set: {x, y, width, height}`).

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
