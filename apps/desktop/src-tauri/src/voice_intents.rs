//! 语音快捷指令：final transcript 在进入 agent 规划前的本地即时匹配层。
//! 纯逻辑模块——短语归一化、意图匹配、流程名解析、配置校验；
//! 副作用执行（stop/run/静音等）在 `voice_commands::handle_quick_intent`。

use crate::dto::FlowSummary;
use pinyin::ToPinyin;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// 语音口令可打开的视图（id 必须是 main.js `lumo://open-view` 白名单的子集）。
const QUICK_VIEWS: &[(&str, &str, &[&str])] = &[
    (
        "mission-control",
        "任务中心",
        &["任务中心", "控制中心", "任务面板", "missioncontrol"],
    ),
    (
        "capability-hub",
        "能力中心",
        &["能力中心", "能力面板", "capabilityhub"],
    ),
    ("settings", "设置", &["设置", "设置页", "系统设置", "settings"]),
    (
        "runs",
        "运行记录",
        &["运行记录", "执行记录", "运行历史", "runs"],
    ),
    ("design", "编排画布", &["编排画布", "画布", "流程编辑", "design"]),
    ("recorder", "录制器", &["录制器", "录制", "recorder"]),
];

const VIEW_VERBS: &[&str] = &[
    "打开", "进入", "切换到", "切到", "查看", "看看", "去", "open", "show", "goto",
];
const STOP_ALIASES: &[&str] = &[
    "停止", "停下", "停", "取消", "停止任务", "取消任务", "全部停止", "别做了", "stop", "cancel",
    "stopall",
];
const MUTE_ALIASES: &[&str] = &["静音", "闭麦", "别听了", "不要听", "mute"];
const UNMUTE_ALIASES: &[&str] = &[
    "取消静音", "解除静音", "恢复声音", "恢复拾音", "开麦", "unmute",
];
const LISTEN_ALIASES: &[&str] = &[
    "开始听写", "开始录音", "听我说", "我要说话", "startlistening", "listen",
];
const STATUS_ALIASES: &[&str] = &[
    "现在什么状态", "现在状态", "当前状态", "系统状态", "任务状态", "状态", "status",
];
const RUN_PREFIXES: &[&str] = &["运行", "执行", "启动", "跑一下", "跑", "run", "execute"];
const FLOW_SUFFIXES: &[&str] = &["流程", "任务", "flow"];

/// 归一化时剥离的前导唤醒/客套词（循环剥离直到稳定）。
const STRIP_PREFIXES: &[&str] = &[
    "你好lumo", "heylumo", "lumo", "麻烦你", "麻烦", "请你", "请", "帮我", "帮忙", "给我", "先",
];

/// 多指令连接词（长词在前，避免「紧接着」被「接着」截断）。
const SEQUENCE_CONNECTORS: &[&str] = &["紧接着", "然后", "接着", "随后", "andthen"];

pub(crate) const QUICK_ACTIONS: &[&str] = &[
    "open_view", "run_flow", "stop", "mute", "unmute", "status", "listen",
];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct QuickCommandDto {
    pub(crate) id: String,
    pub(crate) phrase: String,
    pub(crate) action: String,
    #[serde(default)]
    pub(crate) argument: String,
    #[serde(default = "default_enabled")]
    pub(crate) enabled: bool,
    /// 执行前需要语音二次确认（说「确认」/「取消」）。
    #[serde(default)]
    pub(crate) confirm: bool,
}

fn default_enabled() -> bool {
    true
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum QuickIntent {
    OpenView {
        view: &'static str,
        label: &'static str,
    },
    RunFlow {
        query: String,
    },
    StopAll,
    SetMuted(bool),
    StartListening,
    Status,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum FlowResolution {
    Unique { path: String, name: String },
    Ambiguous(Vec<String>),
    NotFound,
}

/// 匹配结果：意图 + 是否需要二次确认（来自自定义指令的 confirm 标志）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MatchedCommand {
    pub(crate) intent: QuickIntent,
    pub(crate) confirm: bool,
}

/// 小写 + 去掉空白与标点（保留字母/数字/CJK）。用于短语与流程名比较。
pub(crate) fn normalize_text(text: &str) -> String {
    text.to_lowercase()
        .chars()
        .filter(|character| character.is_alphanumeric())
        .collect()
}

/// `normalize_text` + 循环剥离前导唤醒/客套词。用于用户话语。
pub(crate) fn normalize_utterance(text: &str) -> String {
    let mut current = normalize_text(text);
    while let Some(next) = STRIP_PREFIXES
        .iter()
        .find_map(|prefix| current.strip_prefix(prefix))
    {
        if next.is_empty() {
            break;
        }
        current = next.to_string();
    }
    current
}

/// 拼音键：汉字转无声调全拼，其余字符原样。中文 STT 的同音错字
/// （「人物中心」/「任务中心」同为 renwuzhongxin）在此层等价。
pub(crate) fn pinyin_key(normalized: &str) -> String {
    let mut out = String::with_capacity(normalized.len() * 2);
    for (character, syllable) in normalized.chars().zip(normalized.to_pinyin()) {
        match syllable {
            Some(syllable) => out.push_str(syllable.plain()),
            None => out.push(character),
        }
    }
    out
}

/// 短语等价：字面相等，或拼音键相等（谐音容错）。
fn phrases_match(left: &str, right: &str) -> bool {
    left == right || pinyin_key(left) == pinyin_key(right)
}

fn view_meta(view_id: &str) -> Option<(&'static str, &'static str)> {
    QUICK_VIEWS
        .iter()
        .find(|(view, _, _)| *view == view_id.trim())
        .map(|(view, label, _)| (*view, *label))
}

fn matches_any(text: &str, aliases: &[&str]) -> bool {
    aliases.iter().any(|alias| phrases_match(text, alias))
}

fn custom_intent(command: &QuickCommandDto) -> Option<QuickIntent> {
    match command.action.as_str() {
        "open_view" => {
            view_meta(&command.argument).map(|(view, label)| QuickIntent::OpenView { view, label })
        }
        "run_flow" => Some(QuickIntent::RunFlow {
            query: command.argument.trim().to_string(),
        }),
        "stop" => Some(QuickIntent::StopAll),
        "mute" => Some(QuickIntent::SetMuted(true)),
        "unmute" => Some(QuickIntent::SetMuted(false)),
        "status" => Some(QuickIntent::Status),
        "listen" => Some(QuickIntent::StartListening),
        _ => None,
    }
}

fn parse_run_flow(text: &str) -> Option<QuickIntent> {
    for prefix in RUN_PREFIXES {
        let Some(rest) = text.strip_prefix(prefix) else {
            continue;
        };
        let mut query = rest;
        for suffix in FLOW_SUFFIXES {
            if let Some(stripped) = query.strip_suffix(suffix) {
                query = stripped;
                break;
            }
        }
        if !query.is_empty() {
            return Some(QuickIntent::RunFlow {
                query: query.to_string(),
            });
        }
    }
    None
}

/// 匹配顺序：自定义短语（完全匹配）→ 视图 → 停止 → 取消静音 → 静音 →
/// 听写 → 状态 → 运行流程。全部基于归一化后的完全匹配（含拼音谐音层），
/// 返回 None 时话语照旧走 agent 规划。（测试视角的单意图入口；
/// 生产路径走 `match_quick_commands`。）
#[cfg(test)]
pub(crate) fn match_quick_intent(text: &str, custom: &[QuickCommandDto]) -> Option<QuickIntent> {
    match_one(text, custom).map(|matched| matched.intent)
}

fn match_one(text: &str, custom: &[QuickCommandDto]) -> Option<MatchedCommand> {
    let normalized = normalize_utterance(text);
    if normalized.is_empty() {
        return None;
    }
    for command in custom.iter().filter(|command| command.enabled) {
        if phrases_match(&normalize_text(&command.phrase), &normalized) {
            if let Some(intent) = custom_intent(command) {
                return Some(MatchedCommand {
                    intent,
                    confirm: command.confirm,
                });
            }
        }
    }
    let builtin = |intent: QuickIntent| {
        Some(MatchedCommand {
            intent,
            confirm: false,
        })
    };
    for (view, label, aliases) in QUICK_VIEWS {
        for alias in *aliases {
            if phrases_match(&normalized, alias)
                || VIEW_VERBS
                    .iter()
                    .any(|verb| phrases_match(&normalized, &format!("{verb}{alias}")))
            {
                return builtin(QuickIntent::OpenView { view, label });
            }
        }
    }
    if matches_any(&normalized, STOP_ALIASES) {
        return builtin(QuickIntent::StopAll);
    }
    if matches_any(&normalized, UNMUTE_ALIASES) {
        return builtin(QuickIntent::SetMuted(false));
    }
    if matches_any(&normalized, MUTE_ALIASES) {
        return builtin(QuickIntent::SetMuted(true));
    }
    if matches_any(&normalized, LISTEN_ALIASES) {
        return builtin(QuickIntent::StartListening);
    }
    if matches_any(&normalized, STATUS_ALIASES) {
        return builtin(QuickIntent::Status);
    }
    parse_run_flow(&normalized).and_then(builtin)
}

/// 多指令：按连接词切分「打开任务中心然后运行日报」，**全部**片段命中
/// 才返回顺序列表；任一片段不命中则整句回落 agent（避免半执行）。
pub(crate) fn match_quick_commands(
    text: &str,
    custom: &[QuickCommandDto],
) -> Option<Vec<MatchedCommand>> {
    let normalized = normalize_text(text);
    if normalized.is_empty() {
        return None;
    }
    let mut segments = vec![normalized];
    for connector in SEQUENCE_CONNECTORS {
        segments = segments
            .iter()
            .flat_map(|segment| segment.split(connector).map(str::to_string))
            .collect();
    }
    let segments: Vec<String> = segments
        .into_iter()
        .filter(|segment| !segment.is_empty())
        .collect();
    if segments.len() <= 1 {
        return match_one(text, custom).map(|matched| vec![matched]);
    }
    segments
        .iter()
        .map(|segment| match_one(segment, custom))
        .collect()
}

/// 兼容视图：仅意图序列（测试断言用）。
#[cfg(test)]
pub(crate) fn match_quick_intents(
    text: &str,
    custom: &[QuickCommandDto],
) -> Option<Vec<QuickIntent>> {
    match_quick_commands(text, custom)
        .map(|commands| commands.into_iter().map(|matched| matched.intent).collect())
}

const AFFIRMATIVE_REPLIES: &[&str] = &[
    "确认", "确定", "是", "是的", "对", "对的", "好", "好的", "执行", "继续", "yes", "confirm",
    "ok",
];
const NEGATIVE_REPLIES: &[&str] = &[
    "取消", "不", "不要", "不用", "算了", "否", "停止", "别", "no", "cancel",
];

/// 确认对话的肯定答复（含谐音层）。
pub(crate) fn is_affirmative_reply(text: &str) -> bool {
    matches_any(&normalize_utterance(text), AFFIRMATIVE_REPLIES)
}

/// 确认/参数对话的否定与取消答复。
pub(crate) fn is_negative_reply(text: &str) -> bool {
    matches_any(&normalize_utterance(text), NEGATIVE_REPLIES)
}

/// 语音回答 → 流程输入值的粗类型转换。number/boolean 之外一律按字符串。
pub(crate) fn coerce_input_value(kind: &str, reply: &str) -> Result<Value, String> {
    let trimmed = reply.trim().trim_matches(|c: char| "。，！？!?,.".contains(c));
    match kind.to_ascii_lowercase().as_str() {
        "number" | "integer" | "float" => {
            let cleaned: String = trimmed
                .chars()
                .filter(|c| c.is_ascii_digit() || matches!(c, '.' | '-'))
                .collect();
            if let Ok(int) = cleaned.parse::<i64>() {
                Ok(Value::from(int))
            } else if let Ok(float) = cleaned.parse::<f64>() {
                serde_json::Number::from_f64(float)
                    .map(Value::Number)
                    .ok_or_else(|| "请说一个有效数字".into())
            } else {
                Err("请说一个数字".into())
            }
        }
        "boolean" | "bool" => {
            if is_affirmative_reply(trimmed) {
                Ok(Value::Bool(true))
            } else if is_negative_reply(trimmed) {
                Ok(Value::Bool(false))
            } else {
                Err("请回答「是」或「否」".into())
            }
        }
        _ => {
            if trimmed.is_empty() {
                Err("没有听到内容，请再说一次".into())
            } else {
                Ok(Value::String(trimmed.to_string()))
            }
        }
    }
}

/// 历史/播报用的意图标签。
pub(crate) fn intent_label(intent: &QuickIntent) -> String {
    match intent {
        QuickIntent::OpenView { label, .. } => format!("打开{label}"),
        QuickIntent::RunFlow { query } => format!("运行流程「{query}」"),
        QuickIntent::StopAll => "停止全部".into(),
        QuickIntent::SetMuted(true) => "静音".into(),
        QuickIntent::SetMuted(false) => "恢复拾音".into(),
        QuickIntent::StartListening => "开始听写".into(),
        QuickIntent::Status => "状态播报".into(),
    }
}

fn flow_display_name(flow: &FlowSummary) -> String {
    flow.name.clone().unwrap_or_else(|| {
        flow.file_name
            .trim_end_matches(".yaml")
            .trim_end_matches(".yml")
            .to_string()
    })
}

/// 按名称解析流程：先精确（归一化后相等），再包含（双向）；唯一命中才可执行。
pub(crate) fn resolve_flow(query: &str, flows: &[FlowSummary]) -> FlowResolution {
    let needle = normalize_text(query);
    if needle.is_empty() {
        return FlowResolution::NotFound;
    }
    let candidates: Vec<(String, String, &FlowSummary)> = flows
        .iter()
        .filter(|flow| flow.valid)
        .map(|flow| {
            let name = normalize_text(&flow_display_name(flow));
            let key = pinyin_key(&name);
            (name, key, flow)
        })
        .filter(|(name, _, _)| !name.is_empty())
        .collect();
    let needle_key = pinyin_key(&needle);
    let pick = |matched: Vec<&(String, String, &FlowSummary)>| match matched.as_slice() {
        [] => None,
        [(_, _, flow)] => Some(FlowResolution::Unique {
            path: flow.path.clone(),
            name: flow_display_name(flow),
        }),
        many => Some(FlowResolution::Ambiguous(
            many.iter()
                .take(5)
                .map(|(_, _, flow)| flow_display_name(flow))
                .collect(),
        )),
    };
    let exact: Vec<_> = candidates
        .iter()
        .filter(|(name, key, _)| *name == needle || *key == needle_key)
        .collect();
    if let Some(resolution) = pick(exact) {
        return resolution;
    }
    let partial: Vec<_> = candidates
        .iter()
        .filter(|(name, key, _)| {
            name.contains(&needle)
                || needle.contains(name.as_str())
                || key.contains(&needle_key)
                || needle_key.contains(key.as_str())
        })
        .collect();
    pick(partial).unwrap_or(FlowResolution::NotFound)
}

pub(crate) fn validate_quick_commands(commands: &[QuickCommandDto]) -> Result<(), String> {
    let mut seen = std::collections::HashSet::new();
    for command in commands.iter().filter(|command| command.enabled) {
        let phrase = normalize_text(&command.phrase);
        if phrase.is_empty() {
            return Err("voice quick command phrase must not be empty".into());
        }
        if !seen.insert(phrase) {
            return Err(format!(
                "duplicate voice quick command phrase `{}`",
                command.phrase
            ));
        }
        if !QUICK_ACTIONS.contains(&command.action.as_str()) {
            return Err(format!(
                "unsupported voice quick command action `{}`",
                command.action
            ));
        }
        match command.action.as_str() {
            "open_view" if view_meta(&command.argument).is_none() => {
                return Err(format!(
                    "voice quick command view `{}` is not available",
                    command.argument
                ));
            }
            "run_flow" if command.argument.trim().is_empty() => {
                return Err("voice quick command run_flow requires a flow name".into());
            }
            _ => {}
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn custom(phrase: &str, action: &str, argument: &str) -> QuickCommandDto {
        QuickCommandDto {
            id: format!("qc-{phrase}"),
            phrase: phrase.into(),
            action: action.into(),
            argument: argument.into(),
            enabled: true,
            confirm: false,
        }
    }

    fn flow(path: &str, name: Option<&str>, file_name: &str) -> FlowSummary {
        FlowSummary {
            path: path.into(),
            file_name: file_name.into(),
            name: name.map(str::to_string),
            valid: true,
            ..FlowSummary::default()
        }
    }

    #[test]
    fn normalization_strips_punctuation_whitespace_and_wake_prefixes() {
        assert_eq!(normalize_utterance("  Lumo，打开 任务中心！"), "打开任务中心");
        assert_eq!(normalize_utterance("你好Lumo，请帮我停止。"), "停止");
        assert_eq!(normalize_utterance("Hey Lumo, OPEN Settings?"), "opensettings");
        // 全部是前导词时保留最后一段，避免归一化成空串
        assert_eq!(normalize_utterance("lumo"), "lumo");
    }

    #[test]
    fn builtin_view_commands_match_with_and_without_verbs() {
        for (utterance, expected_view) in [
            ("打开任务中心", "mission-control"),
            ("Lumo，切换到能力中心", "capability-hub"),
            ("设置", "settings"),
            ("查看运行记录", "runs"),
            ("open settings", "settings"),
            ("进入编排画布", "design"),
            ("打开录制器", "recorder"),
        ] {
            match match_quick_intent(utterance, &[]) {
                Some(QuickIntent::OpenView { view, .. }) => assert_eq!(view, expected_view),
                other => panic!("`{utterance}` should open {expected_view}, got {other:?}"),
            }
        }
    }

    #[test]
    fn builtin_control_commands_match() {
        assert_eq!(match_quick_intent("停止任务", &[]), Some(QuickIntent::StopAll));
        assert_eq!(match_quick_intent("lumo别做了", &[]), Some(QuickIntent::StopAll));
        assert_eq!(match_quick_intent("静音", &[]), Some(QuickIntent::SetMuted(true)));
        assert_eq!(
            match_quick_intent("取消静音", &[]),
            Some(QuickIntent::SetMuted(false))
        );
        assert_eq!(
            match_quick_intent("开始听写", &[]),
            Some(QuickIntent::StartListening)
        );
        assert_eq!(match_quick_intent("当前状态", &[]), Some(QuickIntent::Status));
    }

    #[test]
    fn run_flow_command_extracts_flow_query() {
        assert_eq!(
            match_quick_intent("运行发票归档流程", &[]),
            Some(QuickIntent::RunFlow {
                query: "发票归档".into()
            })
        );
        assert_eq!(
            match_quick_intent("Lumo，执行 日报 任务", &[]),
            Some(QuickIntent::RunFlow {
                query: "日报".into()
            })
        );
        // 只有动词没有名称时不误触发
        assert_eq!(match_quick_intent("运行", &[]), None);
    }

    #[test]
    fn custom_commands_win_over_builtins_and_respect_enabled() {
        let commands = vec![custom("停止", "open_view", "runs")];
        assert_eq!(
            match_quick_intent("停止", &commands),
            Some(QuickIntent::OpenView {
                view: "runs",
                label: "运行记录"
            })
        );
        let disabled = vec![QuickCommandDto {
            enabled: false,
            ..custom("停止", "open_view", "runs")
        }];
        assert_eq!(match_quick_intent("停止", &disabled), Some(QuickIntent::StopAll));
        let flow = vec![custom("开工", "run_flow", "晨间日报")];
        assert_eq!(
            match_quick_intent("Lumo，开工！", &flow),
            Some(QuickIntent::RunFlow {
                query: "晨间日报".into()
            })
        );
    }

    #[test]
    fn unmatched_utterances_fall_through_to_agent() {
        assert_eq!(match_quick_intent("帮我总结这份报表并发邮件", &[]), None);
        assert_eq!(match_quick_intent("", &[]), None);
        assert_eq!(match_quick_intent("，。！", &[]), None);
    }

    #[test]
    fn flow_resolution_prefers_exact_then_unique_partial() {
        let flows = vec![
            flow("/f/daily.yaml", Some("晨间日报"), "daily.yaml"),
            flow("/f/invoice.yaml", Some("发票归档"), "invoice.yaml"),
            flow("/f/invoice-eu.yaml", Some("发票归档-欧盟"), "invoice-eu.yaml"),
            flow("/f/noname.yaml", None, "backup-db.yaml"),
        ];
        assert_eq!(
            resolve_flow("晨间日报", &flows),
            FlowResolution::Unique {
                path: "/f/daily.yaml".into(),
                name: "晨间日报".into()
            }
        );
        // 「发票归档」精确命中同名流程，而不是多义
        assert_eq!(
            resolve_flow("发票归档", &flows),
            FlowResolution::Unique {
                path: "/f/invoice.yaml".into(),
                name: "发票归档".into()
            }
        );
        assert!(matches!(resolve_flow("发票", &flows), FlowResolution::Ambiguous(names) if names.len() == 2));
        assert_eq!(resolve_flow("不存在", &flows), FlowResolution::NotFound);
        // 无 name 的流程用文件名匹配
        assert_eq!(
            resolve_flow("backup db", &flows),
            FlowResolution::Unique {
                path: "/f/noname.yaml".into(),
                name: "backup-db".into()
            }
        );
    }

    #[test]
    fn pinyin_tolerance_matches_homophones() {
        // STT 常见同音错字：人物中心 → 任务中心
        assert!(matches!(
            match_quick_intent("打开人物中心", &[]),
            Some(QuickIntent::OpenView {
                view: "mission-control",
                ..
            })
        ));
        // 自定义短语谐音：开公 → 开工
        let custom_commands = vec![custom("开工", "run_flow", "晨间日报")];
        assert_eq!(
            match_quick_intent("开公", &custom_commands),
            Some(QuickIntent::RunFlow {
                query: "晨间日报".into()
            })
        );
        assert_eq!(pinyin_key("任务中心"), pinyin_key("人物中心"));
        // 非谐音不误报，英文路径不受影响
        assert_eq!(match_quick_intent("打开日志", &[]), None);
        assert!(matches!(
            match_quick_intent("open settings", &[]),
            Some(QuickIntent::OpenView { view: "settings", .. })
        ));
    }

    #[test]
    fn flow_resolution_tolerates_homophones() {
        let flows = vec![flow("/f/daily.yaml", Some("晨间日报"), "daily.yaml")];
        assert_eq!(
            resolve_flow("陈间日报", &flows),
            FlowResolution::Unique {
                path: "/f/daily.yaml".into(),
                name: "晨间日报".into()
            }
        );
    }

    #[test]
    fn sequential_commands_split_and_all_or_nothing() {
        let intents = match_quick_intents("打开任务中心，然后运行日报流程", &[]).unwrap();
        assert_eq!(intents.len(), 2);
        assert!(matches!(
            intents[0],
            QuickIntent::OpenView {
                view: "mission-control",
                ..
            }
        ));
        assert_eq!(
            intents[1],
            QuickIntent::RunFlow {
                query: "日报".into()
            }
        );
        // 任一片段不命中 → 整句回落 agent
        assert_eq!(match_quick_intents("打开任务中心然后写一份周报总结", &[]), None);
        // 单指令等价于原路径
        assert_eq!(
            match_quick_intents("停止", &[]),
            Some(vec![QuickIntent::StopAll])
        );
        // 「紧接着」不被「接着」误切
        let chained = match_quick_intents("打开设置紧接着静音", &[]).unwrap();
        assert_eq!(chained.len(), 2);
        assert_eq!(chained[1], QuickIntent::SetMuted(true));
    }

    #[test]
    fn quick_command_validation_rejects_bad_configs() {
        assert!(validate_quick_commands(&[custom("开工", "run_flow", "日报")]).is_ok());
        assert!(validate_quick_commands(&[custom(" ", "stop", "")])
            .unwrap_err()
            .contains("phrase"));
        assert!(
            validate_quick_commands(&[custom("开工", "stop", ""), custom("开 工", "status", "")])
                .unwrap_err()
                .contains("duplicate")
        );
        assert!(validate_quick_commands(&[custom("开工", "reboot", "")])
            .unwrap_err()
            .contains("unsupported"));
        assert!(validate_quick_commands(&[custom("开工", "open_view", "nope")])
            .unwrap_err()
            .contains("not available"));
        assert!(validate_quick_commands(&[custom("开工", "run_flow", " ")])
            .unwrap_err()
            .contains("flow name"));
        // 禁用项不参与校验
        assert!(validate_quick_commands(&[QuickCommandDto {
            enabled: false,
            ..custom("", "reboot", "")
        }])
        .is_ok());
    }
}
