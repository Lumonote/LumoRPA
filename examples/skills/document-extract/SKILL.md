---
name: document-extract
description: 把 Word/Excel 文档解析为结构化数据(标题、段落、表格含合并单元格),可输出 Markdown 或 JSON。
version: 0.1.0
inputs:
  - { name: file, type: file, required: true }
  - { name: want, type: string, default: markdown }
outputs:
  - { name: result, type: string }
triggers: [解析文档, 提取表格, 读取合同, parse document, extract tables, docx, xlsx]
tags: [document, extraction, table]
---

# document-extract

按魔数识别格式(不信扩展名 —— RPA 的输入常来自下载或邮件附件),解析为 DocIR
后按 `want` 输出:

* `markdown` —— 带标题层级与表格的 Markdown,适合喂给 LLM 或写进报告
* `json` —— 完整 DocIR,适合程序化取字段

与 `docx.read_text` 的区别:那条路径把表格拍平成段落,本技能保留表格结构与
**合并单元格**跨度。

当前覆盖 DOCX / XLSX / PDF(文本层 + 版面重建 + 有线表格)。扫描件需要 OCR:
默认只用本地小模型且**内容不出网**;要走远程接口或云视觉模型,必须同时给出
`ocr.allow_cloud: true` 与对应的 `network` / `llm` 能力 —— 两道闸是「与」关系,
光有能力声明不足以让文档内容出网。

```yaml
inputs:
  - { name: file, type: file, required: true }
  - { name: want, type: string, default: markdown }
outputs:
  - { name: result, type: string }
steps:
  - id: parse
    action: doc.parse
    with:
      path: "{{ inputs.file }}"
      limit: 2000

  - id: as_markdown
    action: doc.to_markdown
    when: "{{ inputs.want != 'json' }}"
    with:
      doc: "{{ steps.parse.result }}"

  - id: keep_markdown
    action: control.set_var
    when: "{{ inputs.want != 'json' }}"
    with:
      name: result
      value: "{{ steps.as_markdown.result.markdown }}"

  - id: keep_json
    action: control.set_var
    when: "{{ inputs.want == 'json' }}"
    with:
      name: result
      value: "{{ steps.parse.result | tojson }}"
```
