//! 有线表格检测:框线 → 网格 → 单元格。
//!
//! 只处理**有线表格**(置信度高,`TableSource::RulingLines`)。无线表格靠空白沟壑
//! 投影,准确率明显更低,排在 M4 并会以 `TableSource::Whitespace` + 低置信度产出,
//! 不与本模块混为一谈。

use crate::ir::*;
use crate::pdf::text::Fragment;
use crate::pdf::text::Line as RuleLine;

/// 同一条网格线的坐标容差(pt)。
const SNAP: f64 = 3.0;
/// 构成表格的最小网格规模(2×2 个单元格需要 3 横 3 竖)。
const MIN_GRID: usize = 3;

/// 把相近坐标合并成一条网格线,返回升序去重后的坐标。
fn snap(mut values: Vec<f64>) -> Vec<f64> {
    values.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let mut out: Vec<f64> = Vec::new();
    for value in values {
        match out.last_mut() {
            Some(last) if (value - *last).abs() <= SNAP => *last = (*last + value) / 2.0,
            _ => out.push(value),
        }
    }
    out
}

/// 从框线重建表格。`page_height` 用于翻转到左上原点坐标系。
///
/// 单元格文本取自**片段**而非聚类后的行:同一基线上跨列的文字会被
/// `cluster_lines` 合成一行,用行去落格会把整行塞进第一个单元格。
pub fn detect(
    rules: &[RuleLine],
    fragments: &[Fragment],
    page: usize,
    page_height: f64,
    table_seq: &mut usize,
) -> Vec<Table> {
    let horizontals: Vec<&RuleLine> = rules.iter().filter(|l| l.is_horizontal()).collect();
    let verticals: Vec<&RuleLine> = rules.iter().filter(|l| l.is_vertical()).collect();
    if horizontals.len() < MIN_GRID || verticals.len() < MIN_GRID {
        return Vec::new();
    }

    // y 用降序(页面自上而下即行序);x 升序即列序
    let mut ys = snap(horizontals.iter().map(|l| l.y0).collect());
    ys.reverse();
    let xs = snap(verticals.iter().map(|l| l.x0).collect());
    if ys.len() < MIN_GRID || xs.len() < MIN_GRID {
        return Vec::new();
    }

    let rows = ys.len() - 1;
    let cols = xs.len() - 1;
    let mut cells = Vec::new();

    for row in 0..rows {
        for col in 0..cols {
            let (y_top, y_bottom) = (ys[row], ys[row + 1]);
            let (x_left, x_right) = (xs[col], xs[col + 1]);
            // 落在该格内的片段(按片段中心判定),按基线降序、同基线按 x 升序
            let mut inside: Vec<&Fragment> = fragments
                .iter()
                .filter(|f| {
                    let cx = f.x + f.width / 2.0;
                    f.y <= y_top + SNAP
                        && f.y >= y_bottom - SNAP
                        && cx >= x_left - SNAP
                        && cx <= x_right + SNAP
                })
                .collect();
            inside.sort_by(|a, b| {
                b.y.partial_cmp(&a.y)
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then(a.x.partial_cmp(&b.x).unwrap_or(std::cmp::Ordering::Equal))
            });
            let text = inside
                .iter()
                .map(|f| f.text.trim())
                .filter(|t| !t.is_empty())
                .collect::<Vec<_>>()
                .join(" ");
            if text.trim().is_empty() {
                continue;
            }
            cells.push(Cell {
                row,
                col,
                row_span: 1,
                col_span: 1,
                text: text.trim().to_string(),
                formula: None,
                value_type: None,
                bbox: Some(BBox::new(
                    x_left,
                    page_height - y_top,
                    x_right,
                    page_height - y_bottom,
                )),
            });
        }
    }

    if cells.is_empty() {
        return Vec::new();
    }
    *table_seq += 1;
    vec![Table {
        id: format!("t{table_seq}"),
        page,
        bbox: Some(BBox::new(
            xs[0],
            page_height - ys[0],
            xs[xs.len() - 1],
            page_height - ys[ys.len() - 1],
        )),
        rows,
        cols,
        // 首行是否表头无法从框线确知,不猜。
        header_rows: 0,
        cells,
        source: TableSource::RulingLines,
        // 有线表格结构来自真实框线,置信度高但仍非 markup 那样确定
        confidence: 0.95,
        name: None,
    }]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn h(y: f64, x0: f64, x1: f64) -> RuleLine {
        RuleLine { x0, y0: y, x1, y1: y }
    }
    fn v(x: f64, y0: f64, y1: f64) -> RuleLine {
        RuleLine { x0: x, y0, x1: x, y1 }
    }
    fn text(t: &str, x: f64, y: f64) -> Fragment {
        Fragment {
            text: t.into(),
            x,
            y,
            width: 30.0,
            font_size: 10.0,
            width_is_estimated: true,
        }
    }

    #[test]
    fn nearby_coordinates_snap_to_one_grid_line() {
        // 同一条线被分段绘制时坐标会有零点几的抖动
        let snapped = snap(vec![100.0, 100.4, 300.0]);
        assert_eq!(snapped.len(), 2);
        assert!((snapped[0] - 100.2).abs() < 0.5);
    }

    #[test]
    fn cells_take_text_from_fragments_not_merged_lines() {
        // 「品名」和「数量」在同一基线上,聚类成行后会跨两列 —— 必须按片段落格,
        // 否则整行都会挤进第一个单元格。
        let rules = vec![
            h(700.0, 50.0, 250.0),
            h(680.0, 50.0, 250.0),
            h(660.0, 50.0, 250.0),
            v(50.0, 660.0, 700.0),
            v(150.0, 660.0, 700.0),
            v(250.0, 660.0, 700.0),
        ];
        let texts = vec![
            text("品名", 60.0, 690.0),
            text("数量", 160.0, 690.0),
            text("服务器", 60.0, 670.0),
            text("10", 160.0, 670.0),
        ];
        let mut seq = 0;
        let tables = detect(&rules, &texts, 0, 842.0, &mut seq);
        assert_eq!(tables.len(), 1);
        let table = &tables[0];
        assert_eq!((table.rows, table.cols), (2, 2));
        assert_eq!(table.cell_at(0, 0).unwrap().text, "品名");
        assert_eq!(table.cell_at(0, 1).unwrap().text, "数量");
        assert_eq!(table.cell_at(1, 1).unwrap().text, "10");
        assert_eq!(table.source, TableSource::RulingLines);
        assert!(table.confidence < 1.0, "推断出的结构不能标成满置信");
    }

    #[test]
    fn too_few_lines_is_not_a_table() {
        // 两条横线一条竖线只是分隔符,不该被当成表格
        let rules = vec![h(700.0, 50.0, 250.0), h(680.0, 50.0, 250.0), v(50.0, 680.0, 700.0)];
        let mut seq = 0;
        assert!(detect(&rules, &[], 0, 842.0, &mut seq).is_empty());
    }
}
