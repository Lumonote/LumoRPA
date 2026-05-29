# P1-3 加密 Vault — 设计文档

- 状态:已批准(设计阶段)
- 日期:2026-05-30
- 关联路线图:`docs/04-优化与补充开发-路线图.md` 的 **P1-3**
- 范围:在已存在但无实现的 `vault_items` 表之上,落地 age 加解密、`lumo vault` 命令,并让 `{{vault.*}}` 解析走加密库。

## 1. 目标与背景

`vault_items` 表(`schema.rs`)早已声明却无任何读写实现;`{{vault.*}}` 当前只从 `LUMO_VAULT_<NAME>_<KEY>` 环境变量解析(`crates/lumo-core/src/ctx.rs` 的 `resolve_vault_expr`)。本设计补齐三件事:

1. age 加解密的 repo 方法(在 `lumo-storage`);
2. `lumo vault` CLI 命令(init/add/get/list/rm/path);
3. `{{vault.*}}` 解析在 env 未命中时回退到加密库。

现有的"两段式"占位符机制保持不变,是本设计的安全基石:
- `spec.vault: Vec<String>`(`ast.rs`)声明流程可用的 vault 命名空间;
- 模板渲染前,`template.rs` 把 `{{ vault.x.y }}` 预处理成**字面量** `${{ vault.x.y }}`,因此密文从不经过模板引擎、不出现在渲染后的 YAML / 日志里;
- 真正解析发生在动作执行时,由 `StepCtx::resolve_vault_placeholders` → `resolve_vault_expr` 完成(本设计仅改这一步的取值来源)。

## 2. 已定决策

| 维度 | 决策 | 理由 |
|---|---|---|
| 密钥模型 | **age X25519 身份文件** `$LUMO_HOME/age-identity.txt`(0600) | 无人值守运行(`lumo run`/`serve`/cron)零交互;保护落在文件权限 |
| 解析优先级 | **env 优先,加密 store 回退** | 现有 `LUMO_VAULT_*` 流程与测试零改动;env 用于 CI/临时覆盖,store 为持久本地密钥 |
| 架构 | **方案 A:age 加解密 + Vault 门面内聚在 `lumo-storage`** | 表/行/加解密同处一 crate;`lumo-core` 仅需在 `repo` 旁多持一个不透明身份句柄 |

显式排除(YAGNI,列为后续):密钥轮换、多接收者/共享、passphrase 模式、桌面 UI 集成、env 批量迁移工具。

## 3. 架构与 crate 布局

### 3.1 `lumo-storage::vault`(新模块)

- `pub struct VaultIdentity`:包裹 `age::x25519::Identity` 的**不透明** newtype。
  - `generate() -> Self`
  - `load(path) -> Result<Self>`(解析 `AGE-SECRET-KEY-1…`)
  - `save(&self, path) -> Result<()>`(写入并 `chmod 600`,父目录按需创建)
  - `recipient(&self) -> age::x25519::Recipient`
- 加解密自由函数:
  - `encrypt(recipient, plaintext: &[u8]) -> Result<Vec<u8>>`(age 二进制密文)
  - `decrypt(identity, ciphertext: &[u8]) -> Result<Vec<u8>>`
- 门面 `pub struct Vault<'a> { repo: &'a Repo, identity: &'a VaultIdentity }`:
  - `put(name, fields: &Map<String,String>, meta: VaultMeta) -> Result<()>`(序列化 JSON → 加密 → `repo.vault_put`)
  - `get(name) -> Result<Option<Map<String,String>>>`(`repo.vault_get` → 解密 → 反序列化)
  - `list() -> Result<Vec<VaultListed>>`(name + 非敏感 metadata + updated_at,**永不解密**)
  - `delete(name) -> Result<()>`
- 运行时取值助手(供 lumo-core 调用,避免 core 直依赖 age):
  - `pub fn get_field(repo, identity, name, key) -> Result<Option<String>>`

### 3.2 `Repo` 裸行方法(`repo.rs`,仿 `insert_artifact`)

- `vault_put(&self, name, age_ciphertext: &[u8], metadata: &str, updated_at: i64) -> Result<(), StorageError>`(UPSERT)
- `vault_get(&self, name) -> Result<Option<VaultRow>, StorageError>`
- `vault_list(&self) -> Result<Vec<VaultRow>, StorageError>`
- `vault_delete(&self, name) -> Result<(), StorageError>`

`vault_items` 表已在 baseline DDL(`schema.rs`),无需新迁移。

### 3.3 `lumo-core::StepCtx`(最小改动)

- 新增字段 `vault_identity: Option<Arc<lumo_storage::VaultIdentity>>`,builder `with_vault(identity)`(仿 `with_ai`)。
- `lumo-core` **不直接依赖 age**:解析时调 `lumo_storage::vault::get_field(repo, identity, name, key)`。
- `FlowVm` 加 `with_vault(Arc<VaultIdentity>)`,在 `run` 时把身份注入 `StepCtx`。

### 3.4 CLI `cmd::vault`(新)

注册进 `main.rs` 的 `Cmd::Vault(cmd::vault::Args)` 与 `cmd/mod.rs` 的 `pub mod vault;`,签名 `pub async fn run(home: PathBuf, args: Args) -> anyhow::Result<()>`(仿 `cmd::providers`)。

## 4. 数据模型

- `vault_items` 行:
  - `name` = 命名空间(如 `smtp`),主键;
  - `age_ciphertext` = age 加密后的 JSON 对象 `{"user":"…","pass":"…"}`(BLOB);
  - `metadata` = 非敏感 JSON:`{ "description": String?, "keys": [String…], "created_at": i64 }`(`keys` 仅是字段名,非密文);
  - `updated_at` = epoch 毫秒。
- 身份文件:`$LUMO_HOME/age-identity.txt`,age 标准格式,权限 `0600`,路径可经 `LUMO_VAULT_IDENTITY` 覆盖(仿 `providers_path`/`skills_root`)。

## 5. CLI 界面

| 命令 | 行为 |
|---|---|
| `lumo vault init [--force]` | 生成身份文件(已存在则拒,除非 `--force`);打印公钥;提示加入 `.gitignore` |
| `lumo vault add <name> [--key K]` | 隐藏交互读密文值(`rpassword`),或 `--stdin` 从管道读;**绝不**用 `--value` 上 argv(防 history/ps 泄露);落 `{K: value}`,同 name 多 key 合并 |
| `lumo vault get <name> [--key K] [--reveal]` | 默认只出 key 名 + 掩码;`--reveal` 才出明文 |
| `lumo vault list` | comfy_table 列 name / keys / updated_at,**永不出明文** |
| `lumo vault rm <name> [--key K]` | 删整项;给 `--key` 则删单字段(整项空了则删行) |
| `lumo vault path` | 打印身份文件路径与库路径 |

`--key` 缺省时键名为空串 `""`(对应标量 secret,与现有 `LUMO_VAULT_<NAME>`(key 空)语义一致)。

## 6. 运行时解析(改 `ctx.rs::resolve_vault_expr`)

解析 `${{ vault.<NAME>.<KEY> }}`(`KEY` 为剩余段以 `_` 连接,沿用现有逻辑):

1. **env 命中即用**:`LUMO_VAULT_<NAME>_<KEY>`(或 key 空时 `LUMO_VAULT_<NAME>`)——现有行为/测试零改动;
2. **否则**:若 `ctx` 同时持有 `repo` 与 `vault_identity`,调 `vault::get_field(repo, id, name, key)` 解密取值;
3. **都没有 → 报错**(同今天)。

约束与降级:
- `NAME` 仍须在 `spec.vault` 声明(沿用现有校验,未声明即报错);
- **键名形态**:env 路径沿用现有 `sanitize_env`(大写化、非字母数字→`_`,如 `LUMO_VAULT_SMTP_USER`);store 路径用**原样** `KEY`(模板里写什么就查什么,如 `user`、多段 `conn.host`→`conn_host`)。`lumo vault add` 存入的键名即此原样形态;
- 身份文件缺失时 store 分支静默跳过 → 退化为纯 env(优雅降级,不崩);
- 注入路径:`cmd::run`/`cmd::serve` 从身份文件载入 → `FlowVm::with_vault` → `StepCtx`。

## 7. 错误处理

全部 `StepError::msg` / `anyhow`,文案可操作:

- 需解密 store 但身份缺失:`vault identity not found; run 'lumo vault init'`;
- 解密失败(身份不匹配 / 密文损坏):指明 name;
- `key` 不在该 item;
- `name` 未在 `spec.vault` 声明;
- item 不存在。

## 8. 测试策略(TDD)

- **storage**:`generate→encrypt→decrypt` 往返;`put→get` 经真实加密往返;`list` / `delete`;错误身份解密必败;`metadata` 不含密文;`VaultIdentity::save` 权限为 0600。(tempdir + `Repo::open_in_memory` 或临时库)
- **ctx**:env 胜过 store;env 缺失走 store;两者皆无 → 错误;`name` 未声明 → 错误;身份缺失 → 优雅降级到 env。
- **CLI**:`init`(幂等 / `--force`);`add`(`--stdin`)→ `get --reveal` 往返;`list` 掩码;`rm`(整项 / 单 key)。

## 9. 依赖

- workspace 加 `age = "0.11"`(纯 Rust,无 C 依赖,利好 P1-9 交叉编译);
- `rpassword = "7"`(CLI 隐藏输入;仅 lumo-cli 依赖)。

## 10. 安全说明

- 密文从不进入模板引擎 / 日志(沿用两段式占位符);
- 身份文件 `0600` + 提示 `.gitignore`;
- `metadata` 只存非敏感信息(字段名、描述、时间);
- `get` 默认掩码,`--reveal` 才出明文;`list` 永不出明文;
- argv 不接收密文值(只 prompt / stdin)。
