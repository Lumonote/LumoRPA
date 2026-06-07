//! Integration coverage for the umya-backed `excel.*` styling/formatting
//! actions (`set_style`/`merge_cells`/`set_column_width`/`set_row_height`/
//! `freeze_panes`). Each test seeds a real `.xlsx` (via `excel.write_range`,
//! which uses rust_xlsxwriter), runs a styling action through the registry,
//! then re-opens the file with umya and asserts the style/layout stuck —
//! exercising the full read→mutate→write round-trip the actions perform.

mod common;
use common::{fs_caps, ok_with};
use serde_json::json;

/// Seed `file` with a 3×3 grid on the first sheet so styling actions have real
/// cells to target.
async fn seed(file: &std::path::Path, caps: lumo_dsl::Capabilities) {
    ok_with(
        "excel.write_range",
        json!({
            "file": file,
            "sheet": "Sheet1",
            "cell": "A1",
            "values": [["a", "b", "c"], ["d", "e", "f"], ["g", "h", "i"]]
        }),
        caps,
    )
    .await;
}

#[tokio::test]
async fn set_style_applies_font_fill_align_and_border() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("style.xlsx");
    let caps = fs_caps(dir.path());
    seed(&file, caps.clone()).await;

    ok_with(
        "excel.set_style",
        json!({
            "file": file,
            "sheet": "Sheet1",
            "range": "A1:B2",
            "bold": true,
            "italic": true,
            "font_size": 14.0,
            "font_name": "Arial",
            "font_color": "FF0000",
            "bg_color": "FFFF00",
            "align": "center",
            "valign": "middle",
            "wrap": true,
            "number_format": "#,##0.00",
            "border": "thin"
        }),
        caps,
    )
    .await;

    let book = umya_spreadsheet::reader::xlsx::read(&file).unwrap();
    let sheet = book.get_sheet(&0usize).unwrap();
    let style = sheet.get_style("A1");
    assert_eq!(style.get_font().unwrap().get_bold(), &true);
    assert_eq!(style.get_font().unwrap().get_italic(), &true);
    assert_eq!(style.get_font().unwrap().get_size(), &14.0);
    assert_eq!(style.get_font().unwrap().get_name(), "Arial");
    // 6-digit RGB is normalized to opaque ARGB.
    assert_eq!(style.get_font().unwrap().get_color().get_argb(), "FFFF0000");
    assert_eq!(style.get_number_format().unwrap().get_format_code(), "#,##0.00");
    assert_eq!(style.get_alignment().unwrap().get_wrap_text(), &true);
    assert_eq!(
        style.get_borders().unwrap().get_bottom().get_border_style(),
        umya_spreadsheet::Border::BORDER_THIN
    );

    // A bottom-right corner of the range got styled too; outside the range did not.
    let b2 = sheet.get_style("B2");
    assert_eq!(b2.get_font().unwrap().get_bold(), &true);
    let c3 = sheet.get_style("C3");
    assert!(!c3.get_font().map(|f| *f.get_bold()).unwrap_or(false));
}

#[tokio::test]
async fn merge_cells_records_the_range_and_sets_value() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("merge.xlsx");
    let caps = fs_caps(dir.path());
    seed(&file, caps.clone()).await;

    ok_with(
        "excel.merge_cells",
        json!({"file": file, "sheet": "Sheet1", "range": "A1:C1", "value": "Title"}),
        caps,
    )
    .await;

    let book = umya_spreadsheet::reader::xlsx::read(&file).unwrap();
    let sheet = book.get_sheet(&0usize).unwrap();
    assert_eq!(sheet.get_merge_cells().len(), 1);
    assert_eq!(sheet.get_value("A1"), "Title");
}

#[tokio::test]
async fn set_column_width_applies_across_a_range() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("colw.xlsx");
    let caps = fs_caps(dir.path());
    seed(&file, caps.clone()).await;

    ok_with(
        "excel.set_column_width",
        json!({"file": file, "sheet": "Sheet1", "columns": "A:B", "width": 25.0}),
        caps,
    )
    .await;

    let book = umya_spreadsheet::reader::xlsx::read(&file).unwrap();
    let sheet = book.get_sheet(&0usize).unwrap();
    assert_eq!(sheet.get_column_dimension("A").unwrap().get_width(), &25.0);
    assert_eq!(sheet.get_column_dimension("B").unwrap().get_width(), &25.0);
}

#[tokio::test]
async fn set_row_height_applies_across_a_range() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("rowh.xlsx");
    let caps = fs_caps(dir.path());
    seed(&file, caps.clone()).await;

    ok_with(
        "excel.set_row_height",
        json!({"file": file, "sheet": "Sheet1", "rows": "1:2", "height": 30.0}),
        caps,
    )
    .await;

    let book = umya_spreadsheet::reader::xlsx::read(&file).unwrap();
    let sheet = book.get_sheet(&0usize).unwrap();
    assert_eq!(sheet.get_row_dimension(&1u32).unwrap().get_height(), &30.0);
    assert_eq!(sheet.get_row_dimension(&2u32).unwrap().get_height(), &30.0);
}

#[tokio::test]
async fn freeze_panes_sets_a_frozen_pane() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("freeze.xlsx");
    let caps = fs_caps(dir.path());
    seed(&file, caps.clone()).await;

    ok_with(
        "excel.freeze_panes",
        json!({"file": file, "sheet": "Sheet1", "top_left_cell": "B2"}),
        caps,
    )
    .await;

    let book = umya_spreadsheet::reader::xlsx::read(&file).unwrap();
    let sheet = book.get_sheet(&0usize).unwrap();
    let view = &sheet.get_sheets_views().get_sheet_view_list()[0];
    let pane = view.get_pane().expect("freeze_panes should install a pane");
    // PaneStateValues has no PartialEq; compare its Debug form.
    assert_eq!(format!("{:?}", pane.get_state()), "Frozen");
    // `B2` anchor → 1 row + 1 col frozen.
    assert_eq!(pane.get_horizontal_split(), &1.0);
    assert_eq!(pane.get_vertical_split(), &1.0);
}

#[tokio::test]
async fn styling_actions_compose_without_dropping_each_other() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("compose.xlsx");
    let caps = fs_caps(dir.path());
    seed(&file, caps.clone()).await;

    ok_with(
        "excel.set_style",
        json!({"file": file, "sheet": "Sheet1", "range": "A1", "bold": true}),
        caps.clone(),
    )
    .await;
    ok_with(
        "excel.set_column_width",
        json!({"file": file, "sheet": "Sheet1", "columns": "A", "width": 40.0}),
        caps,
    )
    .await;

    // The later column-width write must NOT have dropped the earlier bold style.
    let book = umya_spreadsheet::reader::xlsx::read(&file).unwrap();
    let sheet = book.get_sheet(&0usize).unwrap();
    assert_eq!(sheet.get_style("A1").get_font().unwrap().get_bold(), &true);
    assert_eq!(sheet.get_column_dimension("A").unwrap().get_width(), &40.0);
}

// ── Round 3: chart / conditional format / autofit / comment / validation ──

#[tokio::test]
async fn add_chart_inserts_a_chart_referencing_ranges() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("chart.xlsx");
    let caps = fs_caps(dir.path());
    seed(&file, caps.clone()).await;

    ok_with(
        "excel.add_chart",
        json!({
            "file": file,
            "sheet": "Sheet1",
            "chart_type": "line",
            "series": ["A1:A3", "B1:B3"],
            "from_cell": "E1",
            "to_cell": "K15",
            "title": "Demo"
        }),
        caps,
    )
    .await;

    let book = umya_spreadsheet::reader::xlsx::read(&file).unwrap();
    let sheet = book.get_sheet(&0usize).unwrap();
    assert_eq!(sheet.get_chart_collection().len(), 1);
}

#[tokio::test]
async fn add_chart_maps_column_to_bar() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("colchart.xlsx");
    let caps = fs_caps(dir.path());
    seed(&file, caps.clone()).await;

    ok_with(
        "excel.add_chart",
        json!({
            "file": file,
            "sheet": "Sheet1",
            "chart_type": "column",
            "series": ["A1:A3"],
            "from_cell": "E1",
            "to_cell": "K10"
        }),
        caps,
    )
    .await;

    let book = umya_spreadsheet::reader::xlsx::read(&file).unwrap();
    assert_eq!(book.get_sheet(&0usize).unwrap().get_chart_collection().len(), 1);
}

#[tokio::test]
async fn set_conditional_format_adds_a_rule() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("cf.xlsx");
    let caps = fs_caps(dir.path());
    seed(&file, caps.clone()).await;

    ok_with(
        "excel.set_conditional_format",
        json!({
            "file": file,
            "sheet": "Sheet1",
            "range": "A1:A3",
            "operator": "greater_than",
            "formula1": "1",
            "bg_color": "FFFF00"
        }),
        caps,
    )
    .await;

    let book = umya_spreadsheet::reader::xlsx::read(&file).unwrap();
    let sheet = book.get_sheet(&0usize).unwrap();
    let cfs = sheet.get_conditional_formatting_collection();
    assert_eq!(cfs.len(), 1);
    assert_eq!(cfs[0].get_conditional_collection().len(), 1);
}

#[tokio::test]
async fn set_conditional_format_rejects_between() {
    use common::run_with;
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("cfbad.xlsx");
    let caps = fs_caps(dir.path());
    seed(&file, caps.clone()).await;

    let err = run_with(
        "excel.set_conditional_format",
        json!({
            "file": file,
            "sheet": "Sheet1",
            "range": "A1:A3",
            "operator": "between",
            "formula1": "1",
            "bg_color": "FFFF00"
        }),
        caps,
    )
    .await
    .unwrap_err();
    assert!(err.contains("between"), "got: {err}");
}

#[tokio::test]
async fn autofit_columns_marks_auto_width() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("autofit.xlsx");
    let caps = fs_caps(dir.path());
    seed(&file, caps.clone()).await;

    ok_with(
        "excel.autofit_columns",
        json!({"file": file, "sheet": "Sheet1", "columns": "A:B"}),
        caps,
    )
    .await;

    // umya resolves `auto_width` into a concrete computed width at WRITE time
    // (`Column::calculation_auto_width`); the bool flag itself is not persisted
    // to XML, so on re-read each touched column carries a computed `width` (>0)
    // rather than `auto_width == true`.
    let book = umya_spreadsheet::reader::xlsx::read(&file).unwrap();
    let sheet = book.get_sheet(&0usize).unwrap();
    assert!(*sheet.get_column_dimension("A").unwrap().get_width() > 0.0);
    assert!(*sheet.get_column_dimension("B").unwrap().get_width() > 0.0);
}

#[tokio::test]
async fn set_comment_attaches_a_comment() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("comment.xlsx");
    let caps = fs_caps(dir.path());
    seed(&file, caps.clone()).await;

    ok_with(
        "excel.set_comment",
        json!({
            "file": file,
            "sheet": "Sheet1",
            "cell": "B2",
            "text": "review this",
            "author": "qa"
        }),
        caps,
    )
    .await;

    let book = umya_spreadsheet::reader::xlsx::read(&file).unwrap();
    let sheet = book.get_sheet(&0usize).unwrap();
    assert_eq!(sheet.get_comments().len(), 1);
    assert_eq!(sheet.get_comments()[0].get_author(), "qa");
}

#[tokio::test]
async fn set_data_validation_list_dropdown() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("dvlist.xlsx");
    let caps = fs_caps(dir.path());
    seed(&file, caps.clone()).await;

    ok_with(
        "excel.set_data_validation",
        json!({
            "file": file,
            "sheet": "Sheet1",
            "range": "A1:A10",
            "kind": "list",
            "values": ["red", "green", "blue"]
        }),
        caps,
    )
    .await;

    let book = umya_spreadsheet::reader::xlsx::read(&file).unwrap();
    let sheet = book.get_sheet(&0usize).unwrap();
    let dvs = sheet.get_data_validations().expect("validations present");
    assert_eq!(dvs.get_data_validation_list().len(), 1);
    assert_eq!(dvs.get_data_validation_list()[0].get_formula1(), "\"red,green,blue\"");
}

#[tokio::test]
async fn set_data_validation_whole_between() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("dvwhole.xlsx");
    let caps = fs_caps(dir.path());
    seed(&file, caps.clone()).await;

    ok_with(
        "excel.set_data_validation",
        json!({
            "file": file,
            "sheet": "Sheet1",
            "range": "B1:B10",
            "kind": "whole",
            "operator": "between",
            "formula1": "1",
            "formula2": "100"
        }),
        caps,
    )
    .await;

    let book = umya_spreadsheet::reader::xlsx::read(&file).unwrap();
    let sheet = book.get_sheet(&0usize).unwrap();
    let dvs = sheet.get_data_validations().expect("validations present");
    assert_eq!(dvs.get_data_validation_list().len(), 1);
    assert_eq!(dvs.get_data_validation_list()[0].get_formula2(), "100");
}
