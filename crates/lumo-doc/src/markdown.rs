//! DocIR → Markdown。
//!
//! 表格按 GitHub 风格输出。**Markdown 不支持单元格跨行跨列** —— 合并格的文本
//! 放在锚点位置,被覆盖的位置留空,并在表下用一行注记说明存在合并,免得读者把
//! 空格当成数据缺失。

use crate::ir::*;

pub fn to_markdown(doc: &Document) -> String {
    let mut ordered: Vec<&Block> = doc
        .blocks
        .iter()
        .filter(|b| !matches!(b.block_type, BlockType::Header | BlockType::Footer))
        .collect();
    ordered.sort_by_key(|b| b.reading_order);

    let mut out = String::new();
    for block in ordered {
        match block.block_type {
            BlockType::Heading => {
                let level = block.level.unwrap_or(1).clamp(1, 6) as usize;
                out.push_str(&"#".repeat(level));
                out.push(' ');
                out.push_str(&block.text);
                out.push_str("\n\n");
            }
            BlockType::ListItem => {
                let depth = block.level.unwrap_or(1).saturating_sub(1) as usize;
                out.push_str(&"  ".repeat(depth));
                out.push_str("- ");
                out.push_str(&block.text);
                out.push('\n');
            }
            BlockType::Table => {
                if let Some(table) = block
                    .ref_id
                    .as_ref()
                    .and_then(|id| doc.tables.iter().find(|t| &t.id == id))
                {
                    out.push_str(&table_to_markdown(table));
                    out.push('\n');
                }
            }
            BlockType::Figure => {
                if let Some(image) = block
                    .ref_id
                    .as_ref()
                    .and_then(|id| doc.images.iter().find(|i| &i.id == id))
                {
                    let target = image.path.clone().unwrap_or_else(|| image.id.clone());
                    out.push_str(&format!("![{}]({})\n\n", image.id, target));
                }
            }
            BlockType::Caption => out.push_str(&format!("*{}*\n\n", block.text)),
            BlockType::Paragraph | BlockType::Footnote => {
                out.push_str(&block.text);
                out.push_str("\n\n");
            }
            BlockType::Header | BlockType::Footer => {}
        }
    }
    // 收敛结尾多余空行,输出稳定(快照测试友好)
    while out.ends_with("\n\n\n") {
        out.pop();
    }
    out.trim_end().to_string() + "\n"
}

pub fn table_to_markdown(table: &Table) -> String {
    if table.rows == 0 || table.cols == 0 {
        return String::new();
    }
    // 先铺一张 rows × cols 的文本网格,锚格写文本,被覆盖位置留空。
    let mut grid = vec![vec![String::new(); table.cols]; table.rows];
    let mut has_merge = false;
    for cell in &table.cells {
        if cell.row_span > 1 || cell.col_span > 1 {
            has_merge = true;
        }
        if cell.row < table.rows && cell.col < table.cols {
            grid[cell.row][cell.col] = sanitize(&cell.text);
        }
    }

    let mut out = String::new();
    let header_rows = table.header_rows.min(table.rows);
    let (header, body_start) = if header_rows > 0 {
        (grid[0].clone(), 1)
    } else {
        // 无表头时给一行空表头 —— GitHub 表格语法要求分隔行必须存在
        (vec![String::new(); table.cols], 0)
    };

    out.push_str(&format!("| {} |\n", header.join(" | ")));
    out.push_str(&format!("| {} |\n", vec!["---"; table.cols].join(" | ")));
    for row in grid.iter().skip(body_start) {
        out.push_str(&format!("| {} |\n", row.join(" | ")));
    }
    if has_merge {
        out.push_str("\n> 注:原表含合并单元格,合并区仅锚点位置有值。\n");
    }
    out
}

/// 单元格内的 `|` 会破坏表格语法;换行同理。
fn sanitize(text: &str) -> String {
    text.replace('|', "\\|").replace('\n', "<br>")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table() -> Table {
        Table {
            id: "t1".into(),
            page: 0,
            bbox: None,
            rows: 2,
            cols: 2,
            header_rows: 1,
            cells: vec![
                Cell {
                    row: 0,
                    col: 0,
                    row_span: 1,
                    col_span: 2,
                    text: "合并表头".into(),
                    formula: None,
                    value_type: None,
                    bbox: None,
                },
                Cell {
                    row: 1,
                    col: 0,
                    row_span: 1,
                    col_span: 1,
                    text: "a".into(),
                    formula: None,
                    value_type: None,
                    bbox: None,
                },
                Cell {
                    row: 1,
                    col: 1,
                    row_span: 1,
                    col_span: 1,
                    text: "b|c".into(),
                    formula: None,
                    value_type: None,
                    bbox: None,
                },
            ],
            source: TableSource::Markup,
            confidence: 1.0,
            name: None,
        }
    }

    #[test]
    fn pipes_inside_cells_are_escaped() {
        let md = table_to_markdown(&table());
        assert!(md.contains("b\\|c"), "未转义的 | 会拆断表格: {md}");
    }

    #[test]
    fn merged_tables_carry_an_explicit_note() {
        let md = table_to_markdown(&table());
        assert!(
            md.contains("含合并单元格"),
            "合并信息在 Markdown 里丢失时必须注明,否则空格会被读成数据缺失"
        );
    }

    #[test]
    fn headings_and_lists_render_at_their_level() {
        let mut doc = Document::new(SourceInfo {
            path: "x".into(),
            format: "docx".into(),
            bytes: 0,
            sha256: None,
        });
        doc.blocks = vec![
            Block::text_layer("b1", BlockType::Heading, 0, 0, "标题").with_level(2),
            Block::text_layer("b2", BlockType::ListItem, 0, 1, "条目").with_level(2),
            Block::text_layer("b3", BlockType::Header, 0, 2, "页眉不该出现"),
        ];
        let md = to_markdown(&doc);
        assert!(md.contains("## 标题"));
        assert!(md.contains("  - 条目"));
        assert!(!md.contains("页眉不该出现"));
    }
}
