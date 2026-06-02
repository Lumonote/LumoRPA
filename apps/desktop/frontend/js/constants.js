// Static lookup tables and pure classifiers for the action library / editor.

export const FAMILY_LABEL = {
  favorite: "⭐ 我收藏的指令",
  browser:  "网页自动化",
  condition:"条件判断",
  loop:     "循环",
  wait:     "等待",
  excel:    "Excel / 数据表格",
  file:     "文件 / 操作系统",
  http:     "网络 / API",
  ai:       "AI / 大模型",
  skill:    "自定义指令 (Skills)",
  flow:     "子流程",
  data:     "数据处理",
  control:  "控制流",
  string:   "字符串",
  regex:    "正则表达式",
  date:     "日期 / 时间",
  math:     "数学 / 计算",
  list:     "列表",
  json:     "JSON",
  csv:      "CSV",
  hash:     "哈希 / 加密",
  util:     "通用工具",
  system:   "系统",
  db:       "数据库",
  misc:     "其它",
};

export const FAVORITE_IDS = [
  "browser.open", "browser.type", "browser.click", "browser.extract", "control.log",
];

export const ACTION_ZH = {
  "browser.launch":   { label: "启动浏览器",       hint: "启动并连接 Chromium 会话" },
  "browser.open":     { label: "打开网页",         hint: "在浏览器中打开一个 URL" },
  "browser.click":    { label: "点击元素",         hint: "点击 CSS 选择器匹配到的元素" },
  "browser.type":     { label: "填写输入框",       hint: "在 CSS 选择器匹配的输入框中输入文本" },
  "browser.extract":  { label: "批量抓取数据",     hint: "提取 innerText / 属性 / 字段映射" },
  "browser.close":    { label: "关闭浏览器",       hint: "关闭当前浏览器会话" },

  "control.log":      { label: "打印日志",         hint: "向运行台输出一条日志" },
  "control.sleep":    { label: "等待",             hint: "睡眠指定毫秒数" },
  "control.set_var":  { label: "设置变量",         hint: "向当前运行上下文写入变量" },
  "control.fail":     { label: "中断流程",         hint: "主动抛错终止流程" },
  "control.if":       { label: "条件判断",         hint: "if / else 分支" },
  "control.for":      { label: "计数循环",         hint: "按次数循环执行" },
  "control.for_each": { label: "遍历循环",         hint: "对数组 / Range / 迭代器迭代" },
  "control.parallel": { label: "并行执行",         hint: "同时编排多个分支" },
  "control.try":      { label: "异常处理",         hint: "try / catch / finally" },

  "lumo.flow":        { label: "调用子流程",       hint: "调用另一个 LumoFlow YAML" },

  "data.json_format": { label: "JSON 转字符串",    hint: "将 JSON 值序列化为字符串" },
  "data.json_parse":  { label: "字符串转 JSON",    hint: "解析 JSON 文本到对象" },

  "excel.read_rows":  { label: "读取 Excel",       hint: "读取 .xlsx 表格的行" },
  "excel.write_row":  { label: "写入 Excel",       hint: "追加一行到 .xlsx 表格" },

  "file.read":        { label: "读取文件",         hint: "从本地路径读文件" },
  "file.write":       { label: "写入文件",         hint: "把数据写到本地路径" },
  "file.exists":      { label: "文件存在?",        hint: "判断路径是否存在" },

  "http.request":     { label: "HTTP 请求",        hint: "发起 GET / POST / PUT / DELETE 请求" },

  // ── 字符串 ──
  "string.upper":       { label: "字符串大写",       hint: "把字符串转为大写" },
  "string.lower":       { label: "字符串小写",       hint: "把字符串转为小写" },
  "string.trim":        { label: "去除空白",         hint: "去掉字符串首尾空白" },
  "string.length":      { label: "字符长度",         hint: "按字符计数（不按字节）" },
  "string.split":       { label: "字符串切分",       hint: "按分隔符切成数组" },
  "string.join":        { label: "数组拼接成字符串", hint: "用分隔符把数组连成字符串" },
  "string.replace":     { label: "字符串替换",       hint: "把 from 替换为 to（字面量）" },
  "string.contains":    { label: "包含子串?",        hint: "判断字符串是否包含子串" },
  "string.starts_with": { label: "以…开头?",         hint: "判断是否以前缀开头" },
  "string.ends_with":   { label: "以…结尾?",         hint: "判断是否以后缀结尾" },
  "string.substring":   { label: "截取子串",         hint: "按字符位置切片，支持负数" },
  "string.repeat":      { label: "重复字符串",       hint: "把字符串重复 N 次" },
  "string.pad_left":    { label: "左侧补齐",         hint: "把字符串补齐到指定宽度" },
  "string.pad_right":   { label: "右侧补齐",         hint: "把字符串补齐到指定宽度" },
  "string.format":      { label: "模板替换",         hint: "替换 {key} 占位符" },

  // ── 正则 ──
  "regex.match":        { label: "正则匹配?",        hint: "判断文本是否匹配正则" },
  "regex.find_all":     { label: "正则查找全部",     hint: "返回所有匹配的字符串数组" },
  "regex.replace":      { label: "正则替换",         hint: "支持 $1 $2 反向引用" },
  "regex.captures":     { label: "正则捕获组",       hint: "返回第一个匹配的命名/编号分组" },

  // ── 日期 ──
  "date.now":           { label: "当前时间",         hint: "返回 RFC3339 时间字符串" },
  "date.parse":         { label: "解析时间",         hint: "把任意日期字符串规范成 RFC3339" },
  "date.format":        { label: "格式化时间",       hint: "按 strftime 格式输出" },
  "date.add":           { label: "时间偏移",         hint: "按天/小时/分/秒加减" },
  "date.diff":          { label: "时间差",           hint: "返回 a-b 的差值（天/时/分/秒）" },
  "date.weekday":       { label: "星期几",           hint: "返回 1=周一 … 7=周日" },

  // ── 数学 ──
  "math.round":         { label: "四舍五入",         hint: "保留指定位小数" },
  "math.random":        { label: "随机数",           hint: "范围内随机数（整/浮点）" },
  "math.min":           { label: "最小值",           hint: "数组中最小数" },
  "math.max":           { label: "最大值",           hint: "数组中最大数" },
  "math.sum":           { label: "求和",             hint: "对数组求和" },
  "math.avg":           { label: "平均值",           hint: "对数组求算术平均" },
  "math.abs":           { label: "绝对值",           hint: "取数字的绝对值" },

  // ── 列表 ──
  "list.length":        { label: "列表长度",         hint: "返回数组长度" },
  "list.append":        { label: "追加元素",         hint: "在数组末尾追加一项" },
  "list.sort":          { label: "排序",             hint: "升/降序，支持 by 字段" },
  "list.unique":        { label: "去重",             hint: "保留出现顺序的去重" },
  "list.range":         { label: "生成区间",         hint: "[start, end) 整数数组" },
  "list.contains":      { label: "包含某值?",        hint: "数组是否包含某个值" },
  "list.get":           { label: "按索引取值",       hint: "支持负数索引" },
  "list.slice":         { label: "切片",             hint: "数组切片 [start:end]" },
  "list.reverse":       { label: "倒序",             hint: "倒序排列数组" },
  "list.pluck":         { label: "抽取字段",         hint: "从对象数组中取出某字段" },

  // ── JSON ──
  "json.get":           { label: "按路径取值",       hint: "形如 a.b.0.c 的点号路径" },
  "json.set":           { label: "按路径写值",       hint: "在 JSON 中按点号路径写入" },
  "json.merge":         { label: "对象合并",         hint: "浅合并 a + b，b 优先" },
  "json.keys":          { label: "对象键名",         hint: "返回对象的所有键" },
  "json.values":        { label: "对象值",           hint: "返回对象的所有值" },
  "json.delete":        { label: "按路径删除",       hint: "删除 JSON 中的某个字段" },

  // ── CSV ──
  "csv.parse":          { label: "解析 CSV",         hint: "把 CSV 文本转成数组/对象" },
  "csv.stringify":      { label: "生成 CSV",         hint: "把数组转成 CSV 文本" },
  "csv.read":           { label: "读取 CSV 文件",    hint: "从磁盘读 CSV 并解析" },
  "csv.write":          { label: "写出 CSV 文件",    hint: "把数据写成 CSV 文件" },

  // ── 哈希 / 编码 ──
  "hash.sha256":        { label: "SHA-256",          hint: "SHA-256 十六进制" },
  "hash.sha512":        { label: "SHA-512",          hint: "SHA-512 十六进制" },
  "hash.sha1":          { label: "SHA-1（旧）",      hint: "SHA-1 十六进制" },
  "hash.md5":           { label: "MD5（旧）",        hint: "MD5 十六进制" },
  "util.base64_encode": { label: "Base64 编码",      hint: "把字符串编码为 Base64" },
  "util.base64_decode": { label: "Base64 解码",      hint: "把 Base64 解码为字符串" },
  "util.uuid":          { label: "UUID 生成",        hint: "生成随机 UUID v4" },

  // ── 系统 ──
  "system.shell":       { label: "运行 shell",       hint: "需要 LUMO_ALLOW_SHELL=1" },
  "system.env_get":     { label: "读取环境变量",     hint: "按名字读 env" },
  "system.sleep":       { label: "睡眠",             hint: "等待 N 毫秒" },
  "system.platform":    { label: "系统信息",         hint: "返回 OS / arch" },

  // ── 数据库 ──
  "db.sqlite_query":    { label: "SQLite 查询",      hint: "只读 SELECT，返回行" },
  "db.sqlite_exec":     { label: "SQLite 写入",      hint: "执行 INSERT/UPDATE/DDL" },
};

export function categoryOf(actionId) {
  if (actionId.startsWith("browser."))                     return "browser";
  if (actionId === "control.if")                           return "condition";
  if (actionId === "control.for" || actionId === "control.for_each") return "loop";
  if (actionId === "control.sleep")                        return "wait";
  if (actionId.startsWith("excel."))                       return "excel";
  if (actionId.startsWith("file."))                        return "file";
  if (actionId.startsWith("http."))                        return "http";
  if (actionId.startsWith("ai."))                          return "ai";
  if (actionId.startsWith("skill."))                       return "skill";
  if (actionId === "lumo.flow")                            return "flow";
  if (actionId.startsWith("string."))                      return "string";
  if (actionId.startsWith("regex."))                       return "regex";
  if (actionId.startsWith("date."))                        return "date";
  if (actionId.startsWith("math."))                        return "math";
  if (actionId.startsWith("list."))                        return "list";
  if (actionId.startsWith("json."))                        return "json";
  if (actionId.startsWith("csv."))                         return "csv";
  if (actionId.startsWith("hash."))                        return "hash";
  if (actionId.startsWith("util."))                        return "util";
  if (actionId.startsWith("system."))                      return "system";
  if (actionId.startsWith("db."))                          return "db";
  if (actionId.startsWith("data.") || actionId === "control.set_var") return "data";
  if (actionId.startsWith("control."))                     return "control";
  return "misc";
}

export function zhAction(actionId) {
  return ACTION_ZH[actionId] || { label: actionId || "(未指定)", hint: "" };
}

export const PRESETS = {
  glass: { window: 8, panel: 50 },
  frost: { window: 18, panel: 62 },
  solid: { window: 96, panel: 96 },
  invisible: { window: 0, panel: 36 },
};

export const CAP_KINDS = [
  { key: "network", label: "Network", hint: "host glob, e.g. api.github.com or *.example.com" },
  { key: "fs.read", label: "fs.read", hint: "path or glob, e.g. ./inbox/* or ${HOME}/data/**" },
  { key: "fs.write", label: "fs.write", hint: "path or glob" },
  { key: "llm", label: "llm", hint: "provider/model or `*`" },
  { key: "mcp", label: "mcp", hint: "server, server:tool, or server:tool_*" },
];

export const AI_MODES = ["off", "fallback", "primary"];
export const AI_LABEL = { off: "AI 关", fallback: "AI 兜底", primary: "AI 主导" };
export const AI_HELPER_LABEL = {
  heal_selector: "选择器自愈",
  extract_visual: "视觉抽取",
  decide: "语义决策",
};

export const BLANK_FLOW_TEMPLATE = `apiVersion: lumorpa.io/v1
kind: Flow
metadata:
  id: NAME
  version: 0.1.0
  name: 新流程
spec:
  capabilities:
    network: []
  steps:
    - id: hello
      action: control.log
      with: { message: "hello {{ inputs.name | default('world') }}" }
`;
