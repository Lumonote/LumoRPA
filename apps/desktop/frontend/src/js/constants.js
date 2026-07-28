// Static lookup tables and pure classifiers for the action library / editor.

export const FAMILY_LABEL = {
  favorite: "⭐ 我收藏的指令",
  browser:  "网页自动化",
  desktop:  "桌面自动化",
  image:    "图像识别",
  pdf:      "PDF",
  docx:     "Word / DOCX",
  condition:"条件判断",
  loop:     "循环",
  wait:     "等待",
  excel:    "Excel / 数据表格",
  file:     "文件 / 操作系统",
  archive:  "压缩 / 解压",
  clipboard:"剪贴板",
  http:     "网络 / API",
  transfer: "文件传输",
  email:    "邮件",
  human:    "人机协同",
  notify:   "通知 / Webhook",
  mcp:      "MCP 工具",
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
  xml:      "XML / SOAP",
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
  "browser.wait":     { label: "等待页面元素",     hint: "等待选择器或文本出现" },
  "browser.info":     { label: "读取网页信息",     hint: "读取 URL、标题、HTML 或文本" },
  "browser.eval":     { label: "执行页面脚本",     hint: "在页面上下文执行 JavaScript" },
  "browser.screenshot": { label: "网页截图",       hint: "截取当前页面并保存为图片" },
  "browser.scroll":   { label: "滚动页面",         hint: "滚动到位置、元素或方向" },
  "browser.hover":    { label: "悬停元素",         hint: "移动鼠标到页面元素" },
  "browser.select":   { label: "选择下拉项",       hint: "按值、标签或索引选择 select 选项" },
  "browser.cookies":  { label: "读取 Cookie",      hint: "读取当前页面 Cookie" },
  "browser.set_cookie": { label: "设置 Cookie",    hint: "写入浏览器 Cookie" },
  "browser.tabs":     { label: "列出标签页",       hint: "列出当前浏览器标签页" },
  "browser.tab":      { label: "切换标签页",       hint: "按目标 ID 或 URL 片段激活/关闭标签页" },
  "browser.upload":   { label: "上传文件",         hint: "向文件输入框设置本地文件" },
  "browser.download_wait": { label: "等待下载完成", hint: "点击触发下载并等待文件落盘" },
  "browser.dialog":   { label: "处理网页弹窗",     hint: "接受或取消 alert / confirm / prompt" },
  "browser.frame":    { label: "操作 iframe",      hint: "在指定 iframe 内执行脚本或提取元素" },
  "browser.extract_table": { label: "提取网页表格", hint: "把 HTML 表格抽取为结构化行列" },
  "browser.drag_and_drop": { label: "拖拽元素",     hint: "把网页元素拖到目标元素或坐标" },
  "browser.print_pdf": { label: "网页保存 PDF",    hint: "把当前页面打印为 PDF 文件" },
  "browser.wait_response": { label: "等待接口响应", hint: "等待匹配 URL 的网络响应并读取内容" },
  "browser.close":    { label: "关闭浏览器",       hint: "关闭当前浏览器会话" },

  "desktop.move":     { label: "移动鼠标",         hint: "移动到屏幕坐标" },
  "desktop.click":    { label: "桌面点击",         hint: "点击屏幕坐标或当前位置" },
  "desktop.scroll":   { label: "桌面滚动",         hint: "发送鼠标滚轮滚动" },
  "desktop.key":      { label: "发送快捷键",       hint: "发送按键或组合键" },
  "desktop.type":     { label: "桌面输入文本",     hint: "向当前焦点输入文本" },
  "desktop.screenshot": { label: "桌面截图",       hint: "截取屏幕或区域并保存图片" },
  "desktop.click_text": { label: "点击屏幕文字",   hint: "OCR 定位屏幕文字并点击" },
  "window.list":      { label: "列出窗口",         hint: "读取当前可见窗口列表" },
  "window.activate":  { label: "激活窗口",         hint: "按窗口 ID 或标题片段切到前台" },
  "window.bounds":    { label: "窗口位置大小",     hint: "读取或设置窗口坐标和尺寸" },

  "control.log":      { label: "打印日志",         hint: "向运行台输出一条日志" },
  "control.sleep":    { label: "等待",             hint: "睡眠指定毫秒数" },
  "control.set_var":  { label: "设置变量",         hint: "向当前运行上下文写入变量" },
  "control.fail":     { label: "中断流程",         hint: "主动抛错终止流程" },
  "control.if":       { label: "条件判断",         hint: "if / else 分支" },
  "control.for":      { label: "计数循环",         hint: "按次数循环执行" },
  "control.for_each": { label: "遍历循环",         hint: "对数组 / Range / 迭代器迭代" },
  "control.while":    { label: "条件循环",         hint: "条件为真时重复执行 do 块" },
  "control.break":    { label: "跳出循环",         hint: "退出最近一层循环" },
  "control.continue": { label: "继续下一轮",       hint: "跳过当前循环剩余步骤" },
  "control.parallel": { label: "并行执行",         hint: "同时编排多个分支" },
  "control.try":      { label: "异常处理",         hint: "try / catch / finally" },

  "flow.call":        { label: "调用子流程",       hint: "调用另一个 LumoFlow YAML" },
  "lumo.flow":        { label: "调用子流程",       hint: "调用另一个 LumoFlow YAML" },

  "data.json_format": { label: "JSON 转字符串",    hint: "将 JSON 值序列化为字符串" },
  "data.json_parse":  { label: "字符串转 JSON",    hint: "解析 JSON 文本到对象" },
  "data.filter":      { label: "过滤数据表",       hint: "按字段条件过滤对象数组" },
  "data.group_by":    { label: "分组聚合",         hint: "按字段分组并统计聚合" },
  "data.join":        { label: "连接数据表",       hint: "按键连接两个对象数组" },
  "data.dedup":       { label: "数据去重",         hint: "按字段去重并保留首条或末条" },
  "data.sort_multi":  { label: "多字段排序",       hint: "按多个字段稳定排序对象数组" },

  "excel.read_rows":  { label: "读取 Excel",       hint: "读取 .xlsx 表格的行" },
  "excel.write_row":  { label: "写入 Excel",       hint: "追加一行到 .xlsx 表格" },
  "excel.sheet_names": { label: "列出工作表",      hint: "读取工作簿中的工作表名称" },
  "excel.read_cell":  { label: "读取单元格",       hint: "按 A1 地址读取单元格" },
  "excel.write_cell": { label: "写入单元格",       hint: "按 A1 地址写入单元格" },
  "excel.read_range": { label: "读取区域",         hint: "按 A1 区域读取二维单元格数据" },
  "excel.write_range": { label: "写入区域",        hint: "把二维数组写入指定工作表区域" },
  "excel.find_replace": { label: "查找替换",       hint: "在工作表范围内查找并替换文本" },
  "excel.set_formula": { label: "写入公式",        hint: "向单元格写入 Excel 公式" },
  "excel.set_style":  { label: "设置样式",         hint: "设置单元格字体、颜色、对齐和边框" },
  "excel.merge_cells": { label: "合并单元格",      hint: "合并指定区域并写入可选文本" },
  "excel.set_column_width": { label: "设置列宽",   hint: "调整一个或多个列的显示宽度" },
  "excel.set_row_height": { label: "设置行高",     hint: "调整一个或多个行的显示高度" },
  "excel.freeze_panes": { label: "冻结窗格",       hint: "冻结工作表首行、首列或指定位置" },
  "excel.add_chart": { label: "添加图表",          hint: "基于工作表数据创建折线、柱状或饼图" },
  "excel.set_conditional_format": { label: "条件格式", hint: "为单元格区域设置条件高亮规则" },
  "excel.autofit_columns": { label: "自动列宽",    hint: "按内容估算并调整列宽" },
  "excel.set_comment": { label: "单元格批注",      hint: "给指定单元格写入批注" },
  "excel.set_data_validation": { label: "数据验证", hint: "给单元格设置列表或数值校验" },
  "excel.lookup":     { label: "表格查找",         hint: "按键值执行类似 VLOOKUP/XLOOKUP 的查询" },

  "file.read":        { label: "读取文件",         hint: "从本地路径读文件" },
  "file.write":       { label: "写入文件",         hint: "把数据写到本地路径" },
  "file.exists":      { label: "文件存在?",        hint: "判断路径是否存在" },
  "file.list":        { label: "列出目录",         hint: "读取目录条目，可递归" },
  "file.mkdir":       { label: "创建目录",         hint: "创建本地目录" },
  "file.copy":        { label: "复制文件",         hint: "复制本地文件" },
  "file.move":        { label: "移动文件",         hint: "移动或重命名文件 / 目录" },
  "file.rename":      { label: "重命名",           hint: "在当前目录内重命名文件 / 目录" },
  "file.delete":      { label: "删除路径",         hint: "删除文件、链接或目录" },
  "file.metadata":    { label: "读取元数据",       hint: "读取大小、类型和时间信息" },
  "file.append":      { label: "追加写入",         hint: "向文件末尾追加文本或字节" },
  "file.wait":        { label: "等待文件",         hint: "等待文件出现、稳定或满足大小条件" },
  "archive.zip":      { label: "创建 ZIP",         hint: "把文件或目录打包成 ZIP" },
  "archive.unzip":    { label: "解压 ZIP",         hint: "安全解压 ZIP 到目标目录" },

  "http.request":     { label: "HTTP 请求",        hint: "发起 GET / POST / PUT / DELETE 请求" },
  "http.download":    { label: "HTTP 下载",        hint: "从 URL 下载文件到本地" },
  "http.upload":      { label: "HTTP 上传",        hint: "上传本地文件到 HTTP 服务" },
  "http.oauth2_token": { label: "获取 OAuth2 Token", hint: "向 Token 端点换取 access_token" },
  "http.paginate":    { label: "分页请求",         hint: "按 next 链接或字段连续抓取分页数据" },

  "pdf.extract_text": { label: "提取 PDF 文本",    hint: "从 PDF 中提取文本" },
  "pdf.info":         { label: "读取 PDF 信息",    hint: "读取页数和 PDF 版本" },
  "pdf.write":        { label: "生成 PDF",         hint: "把文本写成 PDF 文件" },
  "docx.read_text":   { label: "读取 Word 文本",   hint: "提取 .docx 文档里的段落文本" },
  "docx.replace_placeholders": { label: "替换 Word 占位符", hint: "把 .docx 模板中的占位符替换为值" },

  "image.locate":     { label: "定位图片",         hint: "在截图中查找模板图片位置" },
  "image.compare":    { label: "比较图片",         hint: "比较两张图片的差异" },
  "image.ocr":        { label: "识别图片文字",     hint: "通过 OCR/视觉模型提取图片文字" },

  "mcp.call":         { label: "调用 MCP 工具",    hint: "通过 MCP stdio 调用工具" },
  "mcp.discover":     { label: "发现 MCP 工具",    hint: "列出 MCP 服务暴露的工具" },

  "ai.chat":          { label: "大模型对话",       hint: "通过当前模型源发送提示词" },
  "skill.invoke":     { label: "执行 Skill",       hint: "调用本地注册的 Skill 子流程" },

  "email.send":       { label: "发送邮件",         hint: "通过 SMTP 发送邮件" },
  "email.fetch":      { label: "读取邮件",         hint: "通过 IMAP 获取邮件" },
  "email.mark":       { label: "标记邮件",         hint: "按 UID 标记已读、删除或加旗标" },
  "email.move":       { label: "移动邮件",         hint: "按 UID 移动邮件到另一个邮箱目录" },
  "human.input":      { label: "人工输入",         hint: "暂停流程等待操作员输入文本" },
  "human.confirm":    { label: "人工确认",         hint: "暂停流程等待是/否确认" },
  "human.approve":    { label: "人工审批",         hint: "发送审批通知并等待操作员决策" },
  "clipboard.get":    { label: "读取剪贴板",       hint: "读取系统剪贴板文本" },
  "clipboard.set":    { label: "写入剪贴板",       hint: "把文本写入系统剪贴板" },
  "notify.send":      { label: "发送通知",         hint: "发送企业微信/飞书/通用 Webhook" },
  "notify.dingtalk":  { label: "钉钉通知",         hint: "发送钉钉群机器人消息" },
  "notify.feishu":    { label: "飞书通知",         hint: "发送飞书群机器人消息" },
  "notify.wecom":     { label: "企微通知",         hint: "发送企业微信群机器人消息" },
  "ftp.upload":       { label: "FTP 上传",         hint: "上传本地文件到 FTP" },
  "ftp.download":     { label: "FTP 下载",         hint: "从 FTP 下载文件" },
  "s3.put":           { label: "上传 S3",          hint: "上传本地文件到 S3 兼容存储" },
  "s3.get":           { label: "下载 S3",          hint: "从 S3 兼容存储下载对象" },

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
  "string.encode_convert": { label: "编码转换",      hint: "在 UTF-8、GBK、GB18030、Big5 等编码间转换" },

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
  "date.workday_add":   { label: "工作日偏移",       hint: "按工作日加减日期" },

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
  "util.url_encode":    { label: "URL 编码",         hint: "对文本做百分号编码" },
  "util.url_decode":    { label: "URL 解码",         hint: "还原百分号编码文本" },
  "util.uuid":          { label: "UUID 生成",        hint: "生成随机 UUID v4" },

  // ── 系统 ──
  "system.shell":       { label: "运行 shell",       hint: "需要 LUMO_ALLOW_SHELL=1" },
  "system.env_get":     { label: "读取环境变量",     hint: "按名字读 env" },
  "system.sleep":       { label: "睡眠",             hint: "等待 N 毫秒" },
  "system.platform":    { label: "系统信息",         hint: "返回 OS / arch" },
  "system.process_list": { label: "进程列表",        hint: "列出当前系统进程" },
  "system.process_kill": { label: "结束进程",        hint: "按 PID 终止进程，需要显式环境开关" },
  "system.app_start":   { label: "启动应用",         hint: "启动外部程序并返回进程 ID" },

  // ── 数据库 ──
  "db.sqlite_query":    { label: "SQLite 查询",      hint: "只读 SELECT，返回行" },
  "db.sqlite_exec":     { label: "SQLite 写入",      hint: "执行 INSERT/UPDATE/DDL" },
  "db.sqlite_batch":    { label: "SQLite 批处理",    hint: "在事务中连续执行多条 SQL" },
  "db.postgres_query":  { label: "PostgreSQL 查询",  hint: "执行只读查询并返回行" },
  "db.postgres_exec":   { label: "PostgreSQL 写入",  hint: "执行写入或 DDL SQL" },
  "db.mysql_query":     { label: "MySQL 查询",       hint: "执行只读查询并返回行" },
  "db.mysql_exec":      { label: "MySQL 写入",       hint: "执行写入或 DDL SQL" },

  // ── XML ──
  "xml.parse":          { label: "解析 XML",         hint: "把 XML 文本转成 JSON 对象" },
  "xml.build":          { label: "生成 XML",         hint: "把 JSON 对象生成 XML 文本" },
  "xml.xpath":          { label: "XPath 查询",       hint: "用 XPath 1.0 从 XML 中提取值" },
};

// Scenario presets per action: ready-to-tweak `with` payloads surfaced in the
// inspector. Picking one only populates the form; the user still clicks
// "写入 YAML" to commit. Keep these small but real (valid `with` shapes).
export const ACTION_PRESETS = {
  "browser.extract": [
    {
      name: "抓表格",
      with: { selector: "table tr", fields: { 列1: "td:nth-child(1)", 列2: "td:nth-child(2)" } },
    },
    {
      name: "抓列表",
      with: { selector: "ul li", attr: "innerText", multiple: true },
    },
  ],
  "browser.click": [
    { name: "点击按钮", with: { selector: "button" } },
    { name: "点击链接(文本)", with: { selector: "a", text: "下一页" } },
  ],
  "http.request": [
    {
      name: "GET JSON",
      with: { method: "GET", url: "https://api.example.com/items", headers: { Accept: "application/json" } },
    },
    {
      name: "POST JSON",
      with: {
        method: "POST",
        url: "https://api.example.com/items",
        headers: { "Content-Type": "application/json" },
        body: { name: "demo" },
      },
    },
  ],
  "excel.read_rows": [
    { name: "读取首个工作表", with: { path: "./data.xlsx" } },
    { name: "读取指定工作表", with: { path: "./data.xlsx", sheet: "Sheet1" } },
  ],
};

export function categoryOf(actionId) {
  if (actionId.startsWith("browser."))                     return "browser";
  if (actionId.startsWith("desktop."))                     return "desktop";
  if (actionId.startsWith("window."))                      return "desktop";
  if (actionId.startsWith("image."))                       return "image";
  if (actionId.startsWith("pdf."))                         return "pdf";
  if (actionId.startsWith("docx."))                        return "docx";
  if (actionId === "control.if")                           return "condition";
  if (actionId === "control.for" || actionId === "control.for_each") return "loop";
  if (actionId === "control.sleep")                        return "wait";
  if (actionId.startsWith("excel."))                       return "excel";
  if (actionId.startsWith("file."))                        return "file";
  if (actionId.startsWith("archive."))                     return "archive";
  if (actionId.startsWith("clipboard."))                   return "clipboard";
  if (actionId.startsWith("http."))                        return "http";
  if (actionId.startsWith("ftp.") || actionId.startsWith("s3.")) return "transfer";
  if (actionId.startsWith("email."))                       return "email";
  if (actionId.startsWith("human."))                       return "human";
  if (actionId.startsWith("notify."))                      return "notify";
  if (actionId.startsWith("mcp."))                         return "mcp";
  if (actionId.startsWith("ai."))                          return "ai";
  if (actionId.startsWith("skill."))                       return "skill";
  if (actionId === "flow.call" || actionId === "lumo.flow") return "flow";
  if (actionId.startsWith("string."))                      return "string";
  if (actionId.startsWith("regex."))                       return "regex";
  if (actionId.startsWith("date."))                        return "date";
  if (actionId.startsWith("math."))                        return "math";
  if (actionId.startsWith("list."))                        return "list";
  if (actionId.startsWith("json."))                        return "json";
  if (actionId.startsWith("xml."))                         return "xml";
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
  glass: { window: 0, panel: 14 },
  frost: { window: 0, panel: 28 },
  solid: { window: 96, panel: 96 },
  invisible: { window: 0, panel: 8 },
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
