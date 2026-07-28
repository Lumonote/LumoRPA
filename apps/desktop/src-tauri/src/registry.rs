//! P2-3 action/skill registry cache: fingerprint-invalidated process-level
//! cache plus skills load-error surfacing (`lumo://toast`).
//! Pure move out of `lib.rs`; semantics unchanged.

use super::*;

pub(crate) fn providers_path(home: &Path) -> PathBuf {
    std::env::var_os("LUMO_PROVIDERS_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join("providers.toml"))
}

pub(crate) fn skills_root(home: &Path) -> PathBuf {
    std::env::var_os("LUMO_SKILLS_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join("skills"))
}

// ─── P2-3: action/skill registry cache ──────────────────────────────────────
//
// 此前每个命令（validate/lint/run/list_actions/…）都全量重建注册表：providers
// 磁盘读 + skills 目录递归扫描/解析，且 `let _ = load_dir(..)` 静默吞错。现改为
// 进程级缓存 + 指纹失效：
//
// 选型说明：
// * 缓存体放 `static OnceLock`（任务给的两个选项之一）而非 DesktopState 字段 ——
//   `parse_and_validate` / `execute_flow` 等深层路径拿不到 tauri State（测试还
//   会脱离 tauri 直接驱动 execute_flow），走 State 要给 8 参的 execute_flow 再
//   加一参并穿透全部调用链；键里带 home，不同测试的 tmp home 自然隔离。
// * 失效用「指纹对比」而非目录 mtime 或进程内 generation 计数：skills 可能被
//   外部编辑器改（无进程内信号可 bump generation），而目录 mtime 在常见文件
//   系统上不随既有文件的内容改写变化。指纹 = providers.toml 的 (mtime,len) +
//   按 lumo-skills loader 同样的遍历规则（递归 ≤3 层、只认 SKILL.md）收集的
//   每文件 (path, mtime_ns, len)。逐文件 stat 远比全量读盘+frontmatter/flow
//   解析便宜；代价是同一纳秒内等长改写检测不到（可接受）。
// * 共享安全性：ActionRegistry 是 `Arc<DashMap>` 系的浅 Clone 且显式设计为跨
//   run 存活（RunTeardown 按 run_id 回收、Action 要求 Send+Sync），并发 run
//   共用同一实例与 `lumo serve` 语义一致。
// * 键 = (home, flow_path)：flow_call 的基目录与 flow 同目录 skills/ 都随
//   flow 变化。条目数以打开过的 flow 数为上界，每条只有几个 Arc，无需淘汰。

pub(crate) type RegistryCacheKey = (PathBuf, Option<PathBuf>);

pub(crate) static REGISTRY_CACHE: OnceLock<Mutex<HashMap<RegistryCacheKey, CachedRegistry>>> = OnceLock::new();

/// P2-3：同一条 skills 加载错误只 toast 一次（tracing::warn 每次照记），防止
/// 每个命令都重弹。
pub(crate) static TOASTED_SKILL_ERRORS: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();

pub(crate) struct CachedRegistry {
    fingerprint: RegistryFingerprint,
    registry: ActionRegistry,
    skills: Arc<SkillRegistry>,
    skill_load_errors: Arc<Vec<String>>,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub(crate) struct RegistryFingerprint {
    /// providers.toml 的 (解析路径, (mtime_ns, len))；文件缺失/不可 stat ⇒ None。
    providers: (PathBuf, Option<(u128, u64)>),
    /// 每个 skills 目录一项：home 的 skills 根 +（有 flow 时）flow 同目录 skills/。
    skill_dirs: Vec<(PathBuf, SkillsDirFingerprint)>,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub(crate) enum SkillsDirFingerprint {
    /// 目录下每个 SKILL.md 的 (path, mtime_ns, len)，按路径排序。遍历规则镜像
    /// lumo-skills loader：递归、深度 ≤3、不存在的根 ⇒ 空集。
    Files(Vec<(PathBuf, u128, u64)>),
    /// 遍历失败（权限等）。错误串入指纹：报错内容变化或恢复可读都会触发重建。
    Unreadable(String),
}

pub(crate) fn mtime_ns(md: &std::fs::Metadata) -> u128 {
    md.modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_nanos())
        .unwrap_or(0)
}

pub(crate) fn skills_dir_fingerprint(root: &Path) -> SkillsDirFingerprint {
    fn walk(dir: &Path, depth: u32, out: &mut Vec<(PathBuf, u128, u64)>) -> std::io::Result<()> {
        if depth > 3 {
            return Ok(());
        }
        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            let p = entry.path();
            if p.is_dir() {
                walk(&p, depth + 1, out)?;
            } else if p
                .file_name()
                .is_some_and(|n| n == "SKILL.md" || n == "active.json")
            {
                let md = entry.metadata()?;
                out.push((p, mtime_ns(&md), md.len()));
            }
        }
        Ok(())
    }
    if !root.exists() {
        return SkillsDirFingerprint::Files(Vec::new());
    }
    let mut files = Vec::new();
    match walk(root, 0, &mut files) {
        Ok(()) => {
            files.sort();
            SkillsDirFingerprint::Files(files)
        }
        Err(e) => SkillsDirFingerprint::Unreadable(format!("{}: {e}", root.display())),
    }
}

pub(crate) fn registry_fingerprint(home: &Path, flow_path: Option<&Path>) -> RegistryFingerprint {
    let providers = providers_path(home);
    let providers_stat = std::fs::metadata(&providers)
        .ok()
        .map(|md| (mtime_ns(&md), md.len()));
    let mut skill_dirs = Vec::with_capacity(2);
    let root = skills_root(home);
    skill_dirs.push((root.clone(), skills_dir_fingerprint(&root)));
    if let Some(flow_dir) = flow_path.and_then(Path::parent) {
        let dir = flow_dir.join("skills");
        skill_dirs.push((dir.clone(), skills_dir_fingerprint(&dir)));
    }
    RegistryFingerprint {
        providers: (providers, providers_stat),
        skill_dirs,
    }
}

/// P2-3：注册表（含 skills）带缓存的统一入口。指纹一致 ⇒ 直接浅 Clone 命中
/// 项；否则重建并落缓存。重建期间持锁 —— 重建只在 providers/skills 变化后
/// 发生，持锁可避免并发命令重复重建。
pub(crate) fn cached_registry(
    home: &Path,
    flow_path: Option<&Path>,
) -> (ActionRegistry, Arc<SkillRegistry>, Arc<Vec<String>>) {
    let key: RegistryCacheKey = (home.to_path_buf(), flow_path.map(Path::to_path_buf));
    let fingerprint = registry_fingerprint(home, flow_path);
    let cache = REGISTRY_CACHE.get_or_init(Default::default);
    let mut map = cache.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(hit) = map.get(&key) {
        if hit.fingerprint == fingerprint {
            return (
                hit.registry.clone(),
                hit.skills.clone(),
                hit.skill_load_errors.clone(),
            );
        }
    }
    let built = build_registry_bundle(home, flow_path, fingerprint);
    let out = (
        built.registry.clone(),
        built.skills.clone(),
        built.skill_load_errors.clone(),
    );
    map.insert(key, built);
    out
}

pub(crate) fn build_registry_bundle(
    home: &Path,
    flow_path: Option<&Path>,
    fingerprint: RegistryFingerprint,
) -> CachedRegistry {
    let providers_cfg = ProvidersConfig::load(providers_path(home)).unwrap_or_default();
    let router = Arc::new(AiRouter::from_config(&providers_cfg));

    let mut registry = ActionRegistry::new();
    lumo_actions::register_all(&mut registry);
    registry.register(ChatAction::new(router));

    // P2-3：load_dir 失败不再 `let _ =` 静默吞掉 —— tracing::warn 每次记录，
    // 首次出现的错误经 `lumo://toast` 透给前端，且错误列表随缓存返回，供
    // 校验失败路径附进命令的 Err（否则用户只见 "unknown action"）。
    let skills = Arc::new(SkillRegistry::new());
    let mut skill_load_errors = Vec::new();
    let mut load = |dir: PathBuf| {
        if let Err(e) = skills.load_dir(&dir) {
            let msg = format!("skills load failed at {}: {e}", dir.display());
            tracing::warn!("{}", msg);
            skill_load_errors.push(msg);
        }
    };
    load(skills_root(home));
    if let Some(flow_dir) = flow_path.and_then(Path::parent) {
        load(flow_dir.join("skills"));
    }
    let managed_root = skills_root(home);
    if let Ok(manager) = lumo_agent::SkillManager::new(&managed_root) {
        if let Ok(entries) = std::fs::read_dir(&managed_root) {
            for entry in entries.flatten().filter(|entry| entry.path().is_dir()) {
                let name = entry.file_name().to_string_lossy().into_owned();
                if let Ok(Some(active)) = manager.active(&name) {
                    skills.remove(&name);
                    if active.enabled {
                        match lumo_skills::loader::load_skill_file(&active.path) {
                            Ok(skill) => skills.insert(skill),
                            Err(error) => skill_load_errors.push(format!(
                                "active skill load failed at {}: {error}",
                                active.path.display()
                            )),
                        }
                    }
                }
            }
        }
    }
    for msg in &skill_load_errors {
        toast_once_skill_error(msg);
    }
    register_skill_actions(&mut registry, skills.clone());

    let flow_base = flow_path
        .and_then(Path::parent)
        .map(Path::to_path_buf)
        .unwrap_or_else(|| home.to_path_buf());
    register_flow_call_action(&mut registry, flow_base);

    CachedRegistry {
        fingerprint,
        registry,
        skills,
        skill_load_errors: Arc::new(skill_load_errors),
    }
}

/// P2-3：skills 加载失败首次出现时向前端发 `lumo://toast` 事件（去重防刷屏）。
/// 前端可 `listen("lumo://toast", …)` 接为 toast；尚无监听者时事件无害。
pub(crate) fn toast_once_skill_error(message: &str) {
    let seen = TOASTED_SKILL_ERRORS.get_or_init(Default::default);
    if !seen
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(message.to_string())
    {
        return;
    }
    if let Some(app) = APP_HANDLE.get() {
        let _ = app.emit(
            "lumo://toast",
            serde_json::json!({ "kind": "warning", "message": message }),
        );
    }
}

/// P2-3：步级校验失败时把 skills 加载失败原因附在错误里 —— skills 目录坏掉时
/// 用户看到的不再只是一句 "unknown action"。
pub(crate) fn attach_skill_load_errors(err: String, skill_load_errors: &[String]) -> String {
    if skill_load_errors.is_empty() {
        err
    } else {
        format!("{err}; {}", skill_load_errors.join("; "))
    }
}

pub(crate) fn build_action_registry(home: &Path, flow_path: Option<&Path>) -> ActionRegistry {
    cached_registry(home, flow_path).0
}

pub(crate) fn load_skill_registry(home: &Path, flow_path: Option<&Path>) -> Arc<SkillRegistry> {
    cached_registry(home, flow_path).1
}

pub(crate) fn flow_uses_action(steps: &[Step], action_id: &str) -> bool {
    steps.iter().any(|step| {
        step.action == action_id
            || step
                .children()
                .into_iter()
                .any(|children| flow_uses_action(children, action_id))
    })
}

#[cfg(test)]
mod desktop_action_registry_tests {
    use super::{build_action_registry, cached_registry};
    use crate::features_data::feature_map_data;
    use std::sync::Arc;

    #[test]
    fn desktop_registry_includes_native_desktop_actions() {
        let tmp = tempfile::tempdir().unwrap();
        let registry = build_action_registry(tmp.path(), None);

        for id in [
            "desktop.click",
            "desktop.screenshot",
            "desktop.click_text",
            "window.list",
            "window.activate",
            "window.bounds",
        ] {
            assert!(
                registry.get(id).is_some(),
                "desktop app registry must include `{id}`"
            );
        }
    }

    /// P2-3：注册表缓存 —— 输入不变时命中缓存；skills 目录新增 SKILL.md 或
    /// providers.toml 变化都会使指纹失效并重建。
    #[test]
    fn registry_cache_hits_until_skills_or_providers_change() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();

        let (_r1, s1, e1) = cached_registry(home, None);
        assert!(e1.is_empty(), "clean home must load without errors: {e1:?}");
        let (_r2, s2, _) = cached_registry(home, None);
        assert!(
            Arc::ptr_eq(&s1, &s2),
            "unchanged providers/skills must hit the cache"
        );

        // 新增一个 SKILL.md ⇒ 指纹变化 ⇒ 重建且新技能可解析。
        let dir = home.join("skills").join("greet");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("SKILL.md"),
            "---\nname: greet\n---\n\n# greet\n\n```yaml\nsteps:\n  - id: say\n    action: control.log\n    with: { message: hi }\n```\n",
        )
        .unwrap();
        let (_r3, s3, _) = cached_registry(home, None);
        assert!(
            !Arc::ptr_eq(&s2, &s3),
            "a new SKILL.md must invalidate the cache"
        );
        assert!(s3.get("greet").is_some(), "rebuilt registry sees the skill");

        // providers.toml 变化 ⇒ 同样失效（ChatAction 的 router 由它构建）。
        std::fs::write(home.join("providers.toml"), "profiles = []\n").unwrap();
        let (_r4, s4, _) = cached_registry(home, None);
        assert!(
            !Arc::ptr_eq(&s3, &s4),
            "a providers.toml change must invalidate the cache"
        );
    }

    /// P2-3：load_dir 失败不再被静默吞掉 —— 错误列表随缓存返回（校验失败时会
    /// 附进命令 Err，首个还会走 `lumo://toast`）。
    #[test]
    fn skills_load_failure_surfaces_in_bundle_errors() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        // 把 skills 根做成普通文件：read_dir 必失败。
        std::fs::write(home.join("skills"), "not a directory").unwrap();
        let (_r, _s, errors) = cached_registry(home, None);
        assert_eq!(errors.len(), 1, "load failure must surface: {errors:?}");
        assert!(
            errors[0].contains("skills load failed"),
            "error should name the failing dir: {}",
            errors[0]
        );
    }

    #[test]
    fn feature_map_reflects_shipped_desktop_and_trigger_paths() {
        let sections = feature_map_data();
        let find = |id: &str| {
            sections
                .iter()
                .flat_map(|s| s.items.iter())
                .find(|item| item.id == id)
                .unwrap_or_else(|| panic!("feature map missing {id}"))
        };

        assert_eq!(find("R-02").status, "ready");
        assert!(
            find("R-02").note.contains("DesktopRecorder"),
            "R-02 should point at the shipped desktop recorder path"
        );
        assert_eq!(find("D-13").status, "ready");
        assert_eq!(find("D-15").status, "ready");
        assert_eq!(find("O-13").status, "ready");
        assert_eq!(find("T-05").status, "ready");
        assert_eq!(find("T-07").status, "ready");
    }
}
