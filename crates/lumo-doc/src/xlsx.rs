//! XLSX 结构化解析:工作簿 → DocIR。
//!
//! 与 `excel.read_rows` 的定位差异:那条路径把工作表当**数据表**读(表头 + 行
//! 数组,喂给 `control.for_each`);本模块把工作簿当**文档**读 —— 每个 sheet 是
//! 一个章节,单元格保留合并跨度、公式原文与值类型。两者并存,互不取代。
//!
//! 三个刻意的取舍:
//!
//! * **公式与值双通道**:`Cell.formula` 存 `=SUM(A1:A9)` 原文,`Cell.text` 存缓存
//!   的计算值。审计场景常常要看公式本身,只给值等于丢证据。
//! * **`value_type` 显式带出**:Excel 日期在底层就是浮点序列号(45292 = 2023-12-31)。
//!   不标类型的话下游会把日期当普通数字算,这是 Excel 解析最常见的坑。
//! * **合并单元格只产出锚格**:从属位置不出现在 `cells` 里,避免下游重复计数。

use crate::error::DocError;
use crate::ir::*;
use calamine::{Data, Dimensions, Reader, Xlsx};
use std::io::Cursor;

/// 单个工作表的单元格数上界 —— 防御 1M 行 × 16K 列的病态表把内存打爆。
const MAX_CELLS_PER_SHEET: usize = 2_000_000;

pub fn parse(bytes: &[u8], source: SourceInfo) -> Result<Document, DocError> {
    let mut workbook: Xlsx<Cursor<&[u8]>> = calamine::open_workbook_from_rs(Cursor::new(bytes))
        .map_err(|e: calamine::XlsxError| DocError::container("xlsx", e.to_string()))?;
    // 合并区需要显式加载,否则 merged_regions_by_sheet 恒为空。加载失败不该让
    // 整份文档解析失败 —— 降级为「无合并」并把这件事写进 diagnostics。
    let merge_warning = workbook
        .load_merged_regions()
        .err()
        .map(|e| format!("合并单元格信息不可用,已按无合并处理: {e}"));
    parse_with(workbook, source, merge_warning)
}

fn parse_with(
    mut workbook: Xlsx<Cursor<&[u8]>>,
    source: SourceInfo,
    merge_warning: Option<String>,
) -> Result<Document, DocError> {
    let mut doc = Document::new(source);
    if let Some(warning) = merge_warning {
        doc.diagnostics.warn(warning);
    }

    let sheet_names = workbook.sheet_names().to_vec();
    let mut order = 0usize;

    for (sheet_index, name) in sheet_names.iter().enumerate() {
        let range = workbook
            .worksheet_range(name)
            .map_err(|e| DocError::malformed(name.clone(), e.to_string()))?;
        // 公式是可选的:没有公式的表这里会给出空 Range,不该连坐失败。
        let formulas = workbook.worksheet_formula(name).ok();
        let merges: Vec<Dimensions> = workbook
            .merged_regions_by_sheet(name)
            .into_iter()
            .map(|(_, _, dim)| *dim)
            .collect();

        let (rows, cols) = range.get_size();
        if rows.saturating_mul(cols) > MAX_CELLS_PER_SHEET {
            return Err(DocError::LimitExceeded {
                message: format!(
                    "sheet `{name}` has {rows}x{cols} cells, exceeding {MAX_CELLS_PER_SHEET}"
                ),
            });
        }

        // sheet 名作为一级标题,让 outline 可导航。
        let heading = Block::text_layer(
            format!("b{}", order + 1),
            BlockType::Heading,
            sheet_index,
            order,
            name.clone(),
        )
        .with_level(1);
        doc.blocks.push(heading);
        order += 1;

        let table_id = format!("t{}", doc.tables.len() + 1);
        let start = range.start().unwrap_or((0, 0));
        let mut cells = Vec::new();

        for (row_offset, row) in range.rows().enumerate() {
            for (col_offset, value) in row.iter().enumerate() {
                let abs_row = start.0 as usize + row_offset;
                let abs_col = start.1 as usize + col_offset;

                // 被合并覆盖的从属格:跳过,跨度记在锚格上。
                if covered_by_merge(&merges, abs_row, abs_col) {
                    continue;
                }
                let formula = formulas
                    .as_ref()
                    .and_then(|f| f.get((row_offset, col_offset)))
                    .filter(|s| !s.is_empty())
                    .map(|s| format!("={s}"));
                if matches!(value, Data::Empty) && formula.is_none() {
                    continue;
                }
                let (row_span, col_span) = merge_span(&merges, abs_row, abs_col);
                cells.push(Cell {
                    row: row_offset,
                    col: col_offset,
                    row_span,
                    col_span,
                    text: render(value),
                    formula,
                    value_type: Some(value_type(value).to_string()),
                    bbox: None,
                });
            }
        }

        if !cells.is_empty() {
            doc.blocks.push(
                Block::text_layer(
                    format!("b{}", order + 1),
                    BlockType::Table,
                    sheet_index,
                    order,
                    String::new(),
                )
                .with_ref(&table_id),
            );
            order += 1;
            doc.tables.push(Table {
                id: table_id,
                page: sheet_index,
                bbox: None,
                rows,
                cols,
                // 首行是否表头无法从 xlsx 确知,不猜 —— 交给调用方按需处理。
                header_rows: 0,
                cells,
                source: TableSource::Markup,
                confidence: 1.0,
                name: Some(name.clone()),
            });
        }

        doc.pages.push(Page {
            index: sheet_index,
            width: None,
            height: None,
            unit: "pt".into(),
            rotation: 0,
            text_coverage: 1.0,
            source: Source::TextLayer,
            name: Some(name.clone()),
        });
    }

    doc.meta.page_count = doc.pages.len();
    doc.rebuild_outline();
    Ok(doc)
}

/// 该坐标是否落在某个合并区内、且**不是**其左上锚点。
fn covered_by_merge(merges: &[Dimensions], row: usize, col: usize) -> bool {
    merges.iter().any(|dim| {
        let (r0, c0) = (dim.start.0 as usize, dim.start.1 as usize);
        let (r1, c1) = (dim.end.0 as usize, dim.end.1 as usize);
        (r0..=r1).contains(&row) && (c0..=c1).contains(&col) && !(row == r0 && col == c0)
    })
}

/// 锚点处的合并跨度;非锚点或未合并返回 (1, 1)。
fn merge_span(merges: &[Dimensions], row: usize, col: usize) -> (usize, usize) {
    for dim in merges {
        let (r0, c0) = (dim.start.0 as usize, dim.start.1 as usize);
        let (r1, c1) = (dim.end.0 as usize, dim.end.1 as usize);
        if row == r0 && col == c0 {
            return (r1 - r0 + 1, c1 - c0 + 1);
        }
    }
    (1, 1)
}

fn render(value: &Data) -> String {
    match value {
        Data::Empty => String::new(),
        Data::String(s) => s.clone(),
        Data::Float(f) => {
            // 整数值的浮点不显示小数尾巴:1.0 → "1"
            if f.fract() == 0.0 && f.abs() < 1e15 {
                format!("{}", *f as i64)
            } else {
                f.to_string()
            }
        }
        Data::Int(i) => i.to_string(),
        Data::Bool(b) => b.to_string(),
        Data::DateTime(dt) => dt.as_f64().to_string(),
        Data::DateTimeIso(s) | Data::DurationIso(s) => s.clone(),
        Data::Error(e) => format!("{e:?}"),
    }
}

fn value_type(value: &Data) -> &'static str {
    match value {
        Data::Empty => "empty",
        Data::String(_) => "string",
        Data::Float(_) | Data::Int(_) => "number",
        Data::Bool(_) => "bool",
        Data::DateTime(_) | Data::DateTimeIso(_) => "date",
        Data::DurationIso(_) => "duration",
        Data::Error(_) => "error",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dim(start: (u32, u32), end: (u32, u32)) -> Dimensions {
        Dimensions { start, end }
    }

    #[test]
    fn merge_anchor_reports_span_and_followers_are_skipped() {
        // A1:B2 合并
        let merges = vec![dim((0, 0), (1, 1))];
        assert_eq!(merge_span(&merges, 0, 0), (2, 2));
        assert!(!covered_by_merge(&merges, 0, 0), "锚点自身不算被覆盖");
        assert!(covered_by_merge(&merges, 0, 1));
        assert!(covered_by_merge(&merges, 1, 1));
        assert!(!covered_by_merge(&merges, 2, 0), "合并区之外不受影响");
    }

    #[test]
    fn integral_floats_lose_the_decimal_tail() {
        assert_eq!(render(&Data::Float(1.0)), "1");
        assert_eq!(render(&Data::Float(1.5)), "1.5");
    }

    #[test]
    fn dates_are_typed_apart_from_plain_numbers() {
        // Excel 日期底层就是浮点序列号,类型标注是下游区分二者的唯一依据
        assert_eq!(value_type(&Data::Float(45292.0)), "number");
        assert_eq!(value_type(&Data::DateTimeIso("2023-12-31".into())), "date");
    }
}
