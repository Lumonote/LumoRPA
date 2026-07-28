//! Excel read/write actions backed by:
//!   * `calamine` — fast pure-Rust reader (xls/xlsx/ods)
//!   * `rust_xlsxwriter` — pure-Rust writer
//!   * `umya-spreadsheet` — pure-Rust read+write that **preserves** styles
//!
//! 样式/排版动作族(`excel.set_style`/`merge_cells`/`set_column_width`/
//! `set_row_height`/`freeze_panes`)统一走 umya:它读取时保留既有格式,因此多个
//! 样式动作可以**叠加组合**(设置格式 → 合并 → 列宽,各步互不覆盖)。模型固定为
//! `umya read → mutate → umya write`,原地按路径写回,在 `spawn_blocking` 中执行。
//!
//! ⚠️ 组合注意:基于 `rust_xlsxwriter` 的 `excel.write_row/write_cell/write_range/
//! set_formula` 会**从数据整表重写并丢弃样式**(calamine 读取时也会丢格式)。所以在
//! 一个流程里,务必**先写完批量数据,再施加样式**,否则样式会被后续数据写入覆盖。
use async_trait::async_trait;
use calamine::{open_workbook_auto, Data, Reader};
use lumo_core::error::StepError;
use lumo_core::{Action, ActionRegistry, ActionResult, ResourceFactory, StepCtx};
use lumo_dsl::ResourceDecl;
use once_cell::sync::Lazy;
use rust_xlsxwriter::Workbook;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Map, Value};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

pub(crate) const XLSX_KIND: &str = "xlsx";

struct TimedExcelAction<A> {
    inner: A,
    schema: &'static Value,
}

impl<A: Action> TimedExcelAction<A> {
    fn new(inner: A) -> Self {
        let mut schema = inner.schema().clone();
        schema
            .as_object_mut()
            .and_then(|root| root.get_mut("properties"))
            .and_then(Value::as_object_mut)
            .expect("Excel action schema must have object properties")
            .insert(
                "timeout_ms".into(),
                serde_json::json!({
                    "type": "integer",
                    "format": "uint64",
                    "minimum": 1,
                    "default": crate::contracts::DEFAULT_ACTION_TIMEOUT_MS,
                }),
            );
        Self {
            inner,
            schema: Box::leak(Box::new(schema)),
        }
    }
}

#[async_trait]
impl<A: Action> Action for TimedExcelAction<A> {
    fn id(&self) -> &'static str {
        self.inner.id()
    }

    fn summary(&self) -> &'static str {
        self.inner.summary()
    }

    fn schema(&self) -> &'static Value {
        self.schema
    }

    async fn execute(
        &self,
        ctx: &mut StepCtx,
        mut input: Value,
    ) -> Result<ActionResult, StepError> {
        let timeout_ms = input
            .as_object_mut()
            .and_then(|object| object.remove("timeout_ms"))
            .map(|value| {
                value.as_u64().filter(|value| *value > 0).ok_or_else(|| {
                    StepError::msg(format!("{}: `timeout_ms` must be >= 1", self.id()))
                })
            })
            .transpose()?
            .unwrap_or(crate::contracts::DEFAULT_ACTION_TIMEOUT_MS);
        let interrupt = ctx.step_interrupt();
        crate::contracts::with_interruptible_timeout(
            timeout_ms,
            interrupt,
            self.inner.execute(ctx, input),
        )
        .await
    }
}

fn register_excel(r: &mut ActionRegistry, action: impl Action) {
    r.register(TimedExcelAction::new(action));
}

pub fn register(r: &mut ActionRegistry) {
    register_excel(r, ReadRowsAction);
    register_excel(r, WriteRowAction);
    register_excel(r, SheetNamesAction);
    register_excel(r, ReadCellAction);
    register_excel(r, WriteCellAction);
    register_excel(r, ReadRangeAction);
    register_excel(r, WriteRangeAction);
    register_excel(r, FindReplaceAction);
    register_excel(r, SetFormulaAction);
    register_excel(r, SetStyleAction);
    register_excel(r, MergeCellsAction);
    register_excel(r, SetColumnWidthAction);
    register_excel(r, SetRowHeightAction);
    register_excel(r, FreezePanesAction);
    register_excel(r, AddChartAction);
    register_excel(r, SetConditionalFormatAction);
    register_excel(r, AutofitColumnsAction);
    register_excel(r, SetCommentAction);
    register_excel(r, SetDataValidationAction);
    register_excel(r, LookupAction);
    register_excel(r, AddSheetAction);
    register_excel(r, DeleteSheetAction);
    register_excel(r, RenameSheetAction);
    register_excel(r, InsertRowsAction);
    register_excel(r, DeleteRowsAction);
    register_excel(r, InsertColumnsAction);
    register_excel(r, DeleteColumnsAction);
    r.register_resource_factory(Arc::new(XlsxFactory));
}

struct XlsxFactory;

fn input_with_xlsx_resource(ctx: &StepCtx, mut input: Value) -> Result<Value, StepError> {
    let Some(resource_name) = ctx.current_resource() else {
        return Ok(input);
    };
    let decl = ctx.resource_decl(&resource_name)?;
    if decl.kind != XLSX_KIND {
        return Err(StepError::msg(format!(
            "excel action requires an `{XLSX_KIND}` resource, but `{resource_name}` has kind `{}`",
            decl.kind
        )));
    }
    let path = decl
        .config
        .get("path")
        .and_then(|value| value.as_str())
        .filter(|path| !path.trim().is_empty())
        .ok_or_else(|| StepError::msg("xlsx resource requires a non-empty `path`"))?;
    let object = input
        .as_object_mut()
        .ok_or_else(|| StepError::msg("excel action input must be an object"))?;
    object
        .entry("file".to_string())
        .or_insert_with(|| Value::String(path.to_string()));
    Ok(input)
}

#[async_trait]
impl ResourceFactory for XlsxFactory {
    fn kind(&self) -> &str {
        XLSX_KIND
    }

    async fn open(&self, decl: &ResourceDecl, _run_id: &str, _name: &str) -> Result<(), StepError> {
        let path = decl
            .config
            .get("path")
            .and_then(|value| value.as_str())
            .ok_or_else(|| StepError::msg("xlsx resource requires a `path`"))?;
        if path.trim().is_empty() {
            return Err(StepError::msg("xlsx resource `path` must not be empty"));
        }
        Ok(())
    }
}

pub struct ReadRowsAction;
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ReadIn {
    #[serde(default)]
    file: PathBuf,
    #[serde(default)]
    sheet: Option<String>,
    #[serde(default = "default_true")]
    header: bool,
    #[serde(default = "crate::contracts::default_collection_limit")]
    limit: usize,
}
fn default_true() -> bool {
    true
}

#[async_trait]
impl Action for ReadRowsAction {
    fn id(&self) -> &'static str {
        "excel.read_rows"
    }
    fn summary(&self) -> &'static str {
        "Read rows from a workbook; row 1 used as headers if header=true"
    }
    fn schema(&self) -> &'static serde_json::Value {
        static SCHEMA: Lazy<Value> = Lazy::new(crate::schema::derive::<ReadIn>);
        &SCHEMA
    }
    async fn execute(&self, ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let ReadIn {
            file,
            sheet,
            header,
            limit,
        } = serde_json::from_value(input_with_xlsx_resource(ctx, input)?)
            .map_err(|e| StepError::msg(format!("excel.read_rows input invalid: {e}")))?;
        let limit = crate::contracts::checked_limit("excel.read_rows", limit)?;
        ctx.ensure_fs_read(&file)?;
        let rows = tokio::task::spawn_blocking(move || -> Result<Vec<Value>, String> {
            let mut wb = open_workbook_auto(&file).map_err(|e| e.to_string())?;
            let sheet_name = match sheet {
                Some(s) => s,
                None => wb
                    .sheet_names()
                    .first()
                    .cloned()
                    .ok_or_else(|| "workbook has no sheets".to_string())?,
            };
            let range = wb.worksheet_range(&sheet_name).map_err(|e| e.to_string())?;
            let mut out: Vec<Value> = Vec::new();
            let mut iter = range.rows();
            let headers: Vec<String> = if header {
                iter.next().map(header_row).unwrap_or_default()
            } else {
                Vec::new()
            };
            for (idx, row) in iter.enumerate() {
                let mut obj = Map::new();
                for (i, cell) in row.iter().enumerate() {
                    let key = headers
                        .get(i)
                        .cloned()
                        .unwrap_or_else(|| format!("col_{i}"));
                    obj.insert(key, cell_to_json(cell));
                }
                obj.insert("_index".into(), Value::from(idx as i64));
                out.push(Value::Object(obj));
                if out.len() > limit {
                    break;
                }
            }
            Ok(out)
        })
        .await
        .map_err(|e| StepError::io(format!("excel.read_rows join: {e}")))?
        .map_err(|e| StepError::io(format!("excel.read_rows: {e}")))?;
        let mut rows = rows;
        let truncated = crate::contracts::truncate_with_flag(&mut rows, limit);
        Ok(ActionResult::from(serde_json::json!({
            "rows": rows,
            "count": rows.len(),
            "limit": limit,
            "truncated": truncated,
        })))
    }
}

fn header_row(r: &[Data]) -> Vec<String> {
    r.iter()
        .enumerate()
        .map(|(i, c)| match c {
            Data::String(s) => s.clone(),
            Data::Empty => format!("col_{i}"),
            other => other.to_string(),
        })
        .collect()
}

fn cell_to_json(c: &Data) -> Value {
    match c {
        Data::Empty => Value::Null,
        Data::String(s) => Value::String(s.clone()),
        Data::Float(f) => serde_json::Number::from_f64(*f)
            .map(Value::Number)
            .unwrap_or(Value::Null),
        Data::Int(i) => Value::from(*i),
        Data::Bool(b) => Value::Bool(*b),
        Data::DateTime(dt) => Value::from(dt.as_f64()),
        Data::DateTimeIso(s) | Data::DurationIso(s) => Value::String(s.clone()),
        Data::Error(e) => Value::String(format!("{e:?}")),
    }
}

pub struct WriteRowAction;
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct WriteRowIn {
    #[serde(default)]
    file: PathBuf,
    #[serde(default)]
    sheet: Option<String>,
    row: Value,
    #[serde(default)]
    headers: Option<Vec<String>>,
    #[serde(default)]
    replace_sheet: bool,
}

#[async_trait]
impl Action for WriteRowAction {
    fn id(&self) -> &'static str {
        "excel.write_row"
    }
    fn summary(&self) -> &'static str {
        "Append a row to an .xlsx workbook (create if missing)"
    }
    fn schema(&self) -> &'static serde_json::Value {
        static SCHEMA: Lazy<Value> = Lazy::new(crate::schema::derive::<WriteRowIn>);
        &SCHEMA
    }
    async fn execute(&self, ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let WriteRowIn {
            file,
            sheet,
            row,
            headers,
            replace_sheet,
        } = serde_json::from_value(input_with_xlsx_resource(ctx, input)?)
            .map_err(|e| StepError::msg(format!("excel.write_row input invalid: {e}")))?;
        ctx.ensure_fs_write(&file)?;
        let sheet = sheet.unwrap_or_else(|| "Sheet1".into());

        let interrupt = ctx.step_interrupt();
        tokio::task::spawn_blocking(move || -> Result<usize, String> {
            let mut sheets = load_sheets(&file)?;
            let mut col_headers: Vec<String> = headers.clone().unwrap_or_default();

            if let Some(rows) = sheets.get(&sheet) {
                if col_headers.is_empty() {
                    if let Some(h) = rows.first() {
                        col_headers = h
                            .iter()
                            .enumerate()
                            .map(|(i, v)| match v {
                                Value::String(s) if !s.is_empty() => s.clone(),
                                Value::Null => format!("col_{i}"),
                                other => other.to_string(),
                            })
                            .collect();
                    }
                }
            }
            let mut existing_rows = if replace_sheet {
                let _ = sheets.remove(&sheet);
                Vec::new()
            } else {
                sheets.remove(&sheet).unwrap_or_default()
            };
            if !existing_rows.is_empty() {
                existing_rows.remove(0);
            }

            let new_row: Vec<Value> = match &row {
                Value::Array(a) => {
                    if col_headers.is_empty() {
                        col_headers = (0..a.len()).map(|i| format!("col_{i}")).collect();
                    }
                    a.clone()
                }
                Value::Object(m) => {
                    if col_headers.is_empty() {
                        col_headers = m.keys().cloned().collect();
                    }
                    col_headers
                        .iter()
                        .map(|h| m.get(h).cloned().unwrap_or(Value::Null))
                        .collect()
                }
                _ => return Err(format!("row must be array or object, got {row}")),
            };
            existing_rows.push(new_row);

            let mut target_rows = Vec::with_capacity(existing_rows.len() + 1);
            target_rows.push(col_headers.iter().cloned().map(Value::String).collect());
            target_rows.extend(existing_rows.clone());
            sheets.insert(sheet.clone(), target_rows);

            let mut wb = Workbook::new();
            for (sheet_name, rows) in &sheets {
                let ws = wb.add_worksheet();
                ws.set_name(sheet_name).map_err(|e| e.to_string())?;
                for (r, row) in rows.iter().enumerate() {
                    for (c, v) in row.iter().enumerate() {
                        write_cell(ws, r as u32, c as u16, v).map_err(|e| e.to_string())?;
                    }
                }
            }
            // P0-2:save 前检查点 —— 判死后文件不得写回。
            if interrupt.is_interrupted() {
                return Err("excel write-back interrupted (file untouched)".into());
            }
            wb.save(&file).map_err(|e| e.to_string())?;
            Ok(existing_rows.len())
        })
        .await
        .map_err(|e| StepError::io(format!("excel join: {e}")))?
        .map_err(StepError::io)
        .map(|n| ActionResult::from(serde_json::json!({ "rows": n })))
    }
}

pub struct SheetNamesAction;

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SheetNamesIn {
    #[serde(default)]
    file: PathBuf,
}

#[async_trait]
impl Action for SheetNamesAction {
    fn id(&self) -> &'static str {
        "excel.sheet_names"
    }
    fn summary(&self) -> &'static str {
        "List worksheet names in a workbook"
    }
    fn schema(&self) -> &'static serde_json::Value {
        static SCHEMA: Lazy<Value> = Lazy::new(crate::schema::derive::<SheetNamesIn>);
        &SCHEMA
    }
    async fn execute(&self, ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let SheetNamesIn { file } =
            serde_json::from_value(input_with_xlsx_resource(ctx, input)?)
                .map_err(|e| StepError::msg(format!("excel.sheet_names input invalid: {e}")))?;
        ctx.ensure_fs_read(&file)?;
        let names = tokio::task::spawn_blocking(move || -> Result<Vec<String>, String> {
            let wb = open_workbook_auto(&file).map_err(|e| e.to_string())?;
            Ok(wb.sheet_names().to_vec())
        })
        .await
        .map_err(|e| StepError::io(format!("excel join: {e}")))?
        .map_err(StepError::io)?;
        Ok(ActionResult::from(serde_json::json!({
            "sheets": names,
        })))
    }
}

pub struct ReadCellAction;

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ReadCellIn {
    #[serde(default)]
    file: PathBuf,
    #[serde(default)]
    sheet: Option<String>,
    cell: String,
}

#[async_trait]
impl Action for ReadCellAction {
    fn id(&self) -> &'static str {
        "excel.read_cell"
    }
    fn summary(&self) -> &'static str {
        "Read a single cell by A1 reference"
    }
    fn schema(&self) -> &'static serde_json::Value {
        static SCHEMA: Lazy<Value> = Lazy::new(crate::schema::derive::<ReadCellIn>);
        &SCHEMA
    }
    async fn execute(&self, ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let input = input_with_xlsx_resource(ctx, input)?;
        let ReadCellIn { file, sheet, cell } =
            serde_json::from_value(input_with_xlsx_resource(ctx, input)?)
                .map_err(|e| StepError::msg(format!("excel.read_cell input invalid: {e}")))?;
        ctx.ensure_fs_read(&file)?;
        let (row, col) = parse_a1(&cell).map_err(StepError::msg)?;
        let out = tokio::task::spawn_blocking(move || -> Result<Value, String> {
            let mut wb = open_workbook_auto(&file).map_err(|e| e.to_string())?;
            let sheet_name = match sheet {
                Some(s) => s,
                None => wb
                    .sheet_names()
                    .first()
                    .cloned()
                    .ok_or_else(|| "workbook has no sheets".to_string())?,
            };
            let range = wb.worksheet_range(&sheet_name).map_err(|e| e.to_string())?;
            Ok(range
                .get_value((row as u32, col as u32))
                .map(cell_to_json)
                .unwrap_or(Value::Null))
        })
        .await
        .map_err(|e| StepError::io(format!("excel join: {e}")))?
        .map_err(StepError::io)?;
        Ok(ActionResult::from(out))
    }
}

pub struct WriteCellAction;

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct WriteCellIn {
    #[serde(default)]
    file: PathBuf,
    #[serde(default)]
    sheet: Option<String>,
    cell: String,
    value: Value,
}

#[async_trait]
impl Action for WriteCellAction {
    fn id(&self) -> &'static str {
        "excel.write_cell"
    }
    fn summary(&self) -> &'static str {
        "Write a single cell by A1 reference (create workbook if missing)"
    }
    fn schema(&self) -> &'static serde_json::Value {
        static SCHEMA: Lazy<Value> = Lazy::new(crate::schema::derive::<WriteCellIn>);
        &SCHEMA
    }
    async fn execute(&self, ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let WriteCellIn {
            file,
            sheet,
            cell,
            value,
        } = serde_json::from_value(input_with_xlsx_resource(ctx, input)?)
            .map_err(|e| StepError::msg(format!("excel.write_cell input invalid: {e}")))?;
        ctx.ensure_fs_write(&file)?;
        let sheet = sheet.unwrap_or_else(|| "Sheet1".into());
        let (row, col) = parse_a1(&cell).map_err(StepError::msg)?;
        if col > u16::MAX as usize {
            return Err(StepError::msg("excel.write_cell column exceeds XLSX limit"));
        }

        let file_for_task = file.clone();
        let sheet_for_task = sheet.clone();
        let interrupt = ctx.step_interrupt();
        tokio::task::spawn_blocking(move || -> Result<(), String> {
            let mut sheets = load_sheets(&file_for_task)?;
            let rows = sheets.entry(sheet_for_task.clone()).or_default();
            while rows.len() <= row {
                rows.push(Vec::new());
            }
            while rows[row].len() <= col {
                rows[row].push(Value::Null);
            }
            rows[row][col] = value;
            save_sheets(&file_for_task, &sheets, &interrupt)
        })
        .await
        .map_err(|e| StepError::io(format!("excel join: {e}")))?
        .map_err(StepError::io)?;

        Ok(ActionResult::from(serde_json::json!({
            "file": file,
            "sheet": sheet,
            "cell": cell,
        })))
    }
}

pub struct ReadRangeAction;

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ReadRangeIn {
    #[serde(default)]
    file: PathBuf,
    #[serde(default)]
    sheet: Option<String>,
    range: String,
}

#[async_trait]
impl Action for ReadRangeAction {
    fn id(&self) -> &'static str {
        "excel.read_range"
    }
    fn summary(&self) -> &'static str {
        "Read an A1:D100 range into a 2D array of cell values"
    }
    fn schema(&self) -> &'static serde_json::Value {
        static SCHEMA: Lazy<Value> = Lazy::new(crate::schema::derive::<ReadRangeIn>);
        &SCHEMA
    }
    async fn execute(&self, ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let ReadRangeIn { file, sheet, range } =
            serde_json::from_value(input_with_xlsx_resource(ctx, input)?)
                .map_err(|e| StepError::msg(format!("excel.read_range input invalid: {e}")))?;
        ctx.ensure_fs_read(&file)?;
        let ((r0, c0), (r1, c1)) = parse_range(&range).map_err(StepError::msg)?;
        let rows = tokio::task::spawn_blocking(move || -> Result<Vec<Value>, String> {
            let mut wb = open_workbook_auto(&file).map_err(|e| e.to_string())?;
            let sheet_name = match sheet {
                Some(s) => s,
                None => wb
                    .sheet_names()
                    .first()
                    .cloned()
                    .ok_or_else(|| "workbook has no sheets".to_string())?,
            };
            let sheet_range = wb.worksheet_range(&sheet_name).map_err(|e| e.to_string())?;
            let mut out: Vec<Value> = Vec::with_capacity(r1 - r0 + 1);
            for r in r0..=r1 {
                let mut row: Vec<Value> = Vec::with_capacity(c1 - c0 + 1);
                for c in c0..=c1 {
                    let v = sheet_range
                        .get_value((r as u32, c as u32))
                        .map(cell_to_json)
                        .unwrap_or(Value::Null);
                    row.push(v);
                }
                out.push(Value::Array(row));
            }
            Ok(out)
        })
        .await
        .map_err(|e| StepError::io(format!("excel join: {e}")))?
        .map_err(StepError::io)?;
        Ok(ActionResult::from(Value::Array(rows)))
    }
}

pub struct WriteRangeAction;

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct WriteRangeIn {
    #[serde(default)]
    file: PathBuf,
    #[serde(default)]
    sheet: Option<String>,
    /// Top-left anchor cell (A1) or full range (A1:D100); only the start cell is
    /// used as the anchor — the `values` 2D array determines extent.
    cell: String,
    values: Vec<Vec<Value>>,
}

#[async_trait]
impl Action for WriteRangeAction {
    fn id(&self) -> &'static str {
        "excel.write_range"
    }
    fn summary(&self) -> &'static str {
        "Write a 2D array starting at an anchor cell (create workbook if missing)"
    }
    fn schema(&self) -> &'static serde_json::Value {
        static SCHEMA: Lazy<Value> = Lazy::new(crate::schema::derive::<WriteRangeIn>);
        &SCHEMA
    }
    async fn execute(&self, ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let WriteRangeIn {
            file,
            sheet,
            cell,
            values,
        } = serde_json::from_value(input_with_xlsx_resource(ctx, input)?)
            .map_err(|e| StepError::msg(format!("excel.write_range input invalid: {e}")))?;
        ctx.ensure_fs_write(&file)?;
        let sheet = sheet.unwrap_or_else(|| "Sheet1".into());
        // Accept either a single anchor cell or a range — anchor is the start.
        let (row0, col0) = match parse_range(&cell) {
            Ok(((r, c), _)) => (r, c),
            Err(_) => parse_a1(&cell).map_err(StepError::msg)?,
        };
        let max_cols = values.iter().map(Vec::len).max().unwrap_or(0);
        if col0 + max_cols > u16::MAX as usize + 1 {
            return Err(StepError::msg(
                "excel.write_range column exceeds XLSX limit",
            ));
        }

        let file_for_task = file.clone();
        let sheet_for_task = sheet.clone();
        let interrupt = ctx.step_interrupt();
        let written = tokio::task::spawn_blocking(move || -> Result<usize, String> {
            let mut sheets = load_sheets(&file_for_task)?;
            let rows = sheets.entry(sheet_for_task).or_default();
            let mut cells = 0usize;
            for (dr, row) in values.iter().enumerate() {
                let r = row0 + dr;
                while rows.len() <= r {
                    rows.push(Vec::new());
                }
                for (dc, v) in row.iter().enumerate() {
                    let c = col0 + dc;
                    while rows[r].len() <= c {
                        rows[r].push(Value::Null);
                    }
                    rows[r][c] = v.clone();
                    cells += 1;
                }
            }
            save_sheets(&file_for_task, &sheets, &interrupt)?;
            Ok(cells)
        })
        .await
        .map_err(|e| StepError::io(format!("excel join: {e}")))?
        .map_err(StepError::io)?;

        Ok(ActionResult::from(serde_json::json!({
            "file": file,
            "sheet": sheet,
            "cells": written,
        })))
    }
}

pub struct FindReplaceAction;

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct FindReplaceIn {
    #[serde(default)]
    file: PathBuf,
    #[serde(default)]
    sheet: Option<String>,
    find: String,
    replace: String,
    /// When true, only whole-cell string matches are replaced; otherwise any
    /// substring within a string cell is replaced.
    #[serde(default)]
    whole_cell: bool,
}

#[async_trait]
impl Action for FindReplaceAction {
    fn id(&self) -> &'static str {
        "excel.find_replace"
    }
    fn summary(&self) -> &'static str {
        "Find/replace text in a sheet's string cells (whole-cell or substring)"
    }
    fn schema(&self) -> &'static serde_json::Value {
        static SCHEMA: Lazy<Value> = Lazy::new(crate::schema::derive::<FindReplaceIn>);
        &SCHEMA
    }
    async fn execute(&self, ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let FindReplaceIn {
            file,
            sheet,
            find,
            replace,
            whole_cell,
        } = serde_json::from_value(input_with_xlsx_resource(ctx, input)?)
            .map_err(|e| StepError::msg(format!("excel.find_replace input invalid: {e}")))?;
        ctx.ensure_fs_read(&file)?;
        ctx.ensure_fs_write(&file)?;
        if find.is_empty() {
            return Err(StepError::msg(
                "excel.find_replace `find` must not be empty",
            ));
        }

        let file_for_task = file.clone();
        let interrupt = ctx.step_interrupt();
        let replaced = tokio::task::spawn_blocking(move || -> Result<u64, String> {
            let mut sheets = load_sheets(&file_for_task)?;
            let mut count = 0u64;
            let target: Vec<String> = match sheet {
                Some(s) => vec![s],
                None => sheets.keys().cloned().collect(),
            };
            for name in &target {
                let Some(rows) = sheets.get_mut(name) else {
                    continue;
                };
                for row in rows.iter_mut() {
                    for v in row.iter_mut() {
                        if let Value::String(s) = v {
                            if whole_cell {
                                if s == &find {
                                    *s = replace.clone();
                                    count += 1;
                                }
                            } else if s.contains(&find) {
                                count += s.matches(&find).count() as u64;
                                *s = s.replace(&find, &replace);
                            }
                        }
                    }
                }
            }
            save_sheets(&file_for_task, &sheets, &interrupt)?;
            Ok(count)
        })
        .await
        .map_err(|e| StepError::io(format!("excel join: {e}")))?
        .map_err(StepError::io)?;

        Ok(ActionResult::from(serde_json::json!({
            "file": file,
            "replaced": replaced,
        })))
    }
}

pub struct SetFormulaAction;

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SetFormulaIn {
    #[serde(default)]
    file: PathBuf,
    #[serde(default)]
    sheet: Option<String>,
    cell: String,
    /// Formula string with or without a leading `=` (e.g. `SUM(A1:A10)`).
    formula: String,
}

#[async_trait]
impl Action for SetFormulaAction {
    fn id(&self) -> &'static str {
        "excel.set_formula"
    }
    fn summary(&self) -> &'static str {
        "Write a formula to a cell by A1 reference (create workbook if missing); the formula is stored, NOT evaluated — reading the cell back yields the formula text/empty value, not a computed result (Excel/WPS computes it on open)"
    }
    fn schema(&self) -> &'static serde_json::Value {
        static SCHEMA: Lazy<Value> = Lazy::new(crate::schema::derive::<SetFormulaIn>);
        &SCHEMA
    }
    async fn execute(&self, ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let SetFormulaIn {
            file,
            sheet,
            cell,
            formula,
        } = serde_json::from_value(input_with_xlsx_resource(ctx, input)?)
            .map_err(|e| StepError::msg(format!("excel.set_formula input invalid: {e}")))?;
        ctx.ensure_fs_read(&file)?;
        ctx.ensure_fs_write(&file)?;
        let sheet = sheet.unwrap_or_else(|| "Sheet1".into());
        let (row, col) = parse_a1(&cell).map_err(StepError::msg)?;
        if col > u16::MAX as usize {
            return Err(StepError::msg(
                "excel.set_formula column exceeds XLSX limit",
            ));
        }

        // Sentinel-wrap the formula so it survives the BTreeMap<…, Value> sheet
        // model (which only knows JSON scalars) and is re-emitted via
        // `write_formula` rather than `write_string` by `write_cell`.
        let file_for_task = file.clone();
        let sheet_for_task = sheet.clone();
        let interrupt = ctx.step_interrupt();
        tokio::task::spawn_blocking(move || -> Result<(), String> {
            let mut sheets = load_sheets(&file_for_task)?;
            let rows = sheets.entry(sheet_for_task).or_default();
            while rows.len() <= row {
                rows.push(Vec::new());
            }
            while rows[row].len() <= col {
                rows[row].push(Value::Null);
            }
            rows[row][col] = Value::String(format!("{FORMULA_SENTINEL}{formula}"));
            save_sheets(&file_for_task, &sheets, &interrupt)
        })
        .await
        .map_err(|e| StepError::io(format!("excel join: {e}")))?
        .map_err(StepError::io)?;

        Ok(ActionResult::from(serde_json::json!({
            "file": file,
            "sheet": sheet,
            "cell": cell,
        })))
    }
}

// ─────────────────────────────────────────────────────────────────────────
// 样式/排版动作族(umya-spreadsheet,读写保留格式,可叠加组合)
// ─────────────────────────────────────────────────────────────────────────

/// 以 umya 读取工作簿,定位 `sheet`(None 取第一个),施加 `mutate`,再写回 `file`。
/// 全程同步、阻塞,供 `spawn_blocking` 调用;错误以 `String` 返回。
/// P0-2:写回前检查 `interrupt` —— 步骤被引擎判超时/取消后,孤儿阻塞任务到此
/// 必须止步,文件不得落盘(读取与 mutate 都在内存里,白做即可,落盘才是闸口)。
fn umya_mutate<F>(
    file: &PathBuf,
    sheet: Option<String>,
    interrupt: &lumo_core::StepInterrupt,
    mutate: F,
) -> Result<(), String>
where
    F: FnOnce(&mut umya_spreadsheet::Worksheet) -> Result<(), String>,
{
    let mut book = umya_spreadsheet::reader::xlsx::read(file)
        .map_err(|e| format!("excel umya read `{}`: {e:?}", file.display()))?;
    {
        let ws = match sheet {
            Some(ref name) => book
                .get_sheet_by_name_mut(name)
                .ok_or_else(|| format!("sheet `{name}` not found"))?,
            None => book
                .get_sheet_mut(&0usize)
                .ok_or_else(|| "workbook has no sheets".to_string())?,
        };
        mutate(ws)?;
    }
    if interrupt.is_interrupted() {
        return Err("excel write-back interrupted (file untouched)".into());
    }
    umya_spreadsheet::writer::xlsx::write(&book, file)
        .map_err(|e| format!("excel umya write `{}`: {e:?}", file.display()))?;
    Ok(())
}

async fn umya_mutate_book<R, F>(ctx: &StepCtx, file: PathBuf, mutate: F) -> Result<R, StepError>
where
    R: Send + 'static,
    F: FnOnce(&mut umya_spreadsheet::Spreadsheet) -> Result<R, String> + Send + 'static,
{
    ctx.ensure_fs_read(&file)?;
    ctx.ensure_fs_write(&file)?;
    let interrupt = ctx.step_interrupt();
    tokio::task::spawn_blocking(move || {
        let mut book = umya_spreadsheet::reader::xlsx::read(&file)
            .map_err(|e| format!("excel umya read `{}`: {e:?}", file.display()))?;
        let result = mutate(&mut book)?;
        if interrupt.is_interrupted() {
            return Err(String::from(
                "excel write-back interrupted (file untouched)",
            ));
        }
        umya_spreadsheet::writer::xlsx::write(&book, &file)
            .map_err(|e| format!("excel umya write `{}`: {e:?}", file.display()))?;
        Ok(result)
    })
    .await
    .map_err(|e| StepError::io(format!("excel structure task: {e}")))?
    .map_err(StepError::io)
}

fn checked_structure_count(action: &str, count: u32) -> Result<u32, StepError> {
    if count == 0 {
        Err(StepError::msg(format!("{action}: `count` must be >= 1")))
    } else {
        Ok(count)
    }
}

// ─────────────────────────────────────────────────────────────────────────
// Workbook structure actions (sheets / rows / columns)
// ─────────────────────────────────────────────────────────────────────────

pub struct AddSheetAction;
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct AddSheetIn {
    #[serde(default)]
    file: PathBuf,
    name: String,
}

#[async_trait]
impl Action for AddSheetAction {
    fn id(&self) -> &'static str {
        "excel.add_sheet"
    }
    fn summary(&self) -> &'static str {
        "Add a worksheet to an existing workbook"
    }
    fn schema(&self) -> &'static Value {
        static S: Lazy<Value> = Lazy::new(crate::schema::derive::<AddSheetIn>);
        &S
    }
    async fn execute(&self, ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let AddSheetIn { file, name } =
            serde_json::from_value(input_with_xlsx_resource(ctx, input)?)
                .map_err(|e| StepError::msg(format!("excel.add_sheet input invalid: {e}")))?;
        let out_name = name.clone();
        umya_mutate_book(ctx, file.clone(), move |book| {
            book.new_sheet(name).map_err(str::to_string)?;
            Ok(())
        })
        .await?;
        Ok(ActionResult::from(
            serde_json::json!({"file": file, "sheet": out_name}),
        ))
    }
}

pub struct DeleteSheetAction;
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct DeleteSheetIn {
    #[serde(default)]
    file: PathBuf,
    sheet: String,
}

#[async_trait]
impl Action for DeleteSheetAction {
    fn id(&self) -> &'static str {
        "excel.delete_sheet"
    }
    fn summary(&self) -> &'static str {
        "Delete a worksheet from a workbook"
    }
    fn schema(&self) -> &'static Value {
        static S: Lazy<Value> = Lazy::new(crate::schema::derive::<DeleteSheetIn>);
        &S
    }
    async fn execute(&self, ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let DeleteSheetIn { file, sheet } =
            serde_json::from_value(input_with_xlsx_resource(ctx, input)?)
                .map_err(|e| StepError::msg(format!("excel.delete_sheet input invalid: {e}")))?;
        let target = sheet.clone();
        umya_mutate_book(ctx, file.clone(), move |book| {
            if book.get_sheet_count() <= 1 {
                return Err("excel.delete_sheet: cannot remove the workbook's last sheet".into());
            }
            book.remove_sheet_by_name(&sheet).map_err(str::to_string)
        })
        .await?;
        Ok(ActionResult::from(
            serde_json::json!({"file": file, "sheet": target, "deleted": true}),
        ))
    }
}

pub struct RenameSheetAction;
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct RenameSheetIn {
    #[serde(default)]
    file: PathBuf,
    sheet: String,
    new_name: String,
}

#[async_trait]
impl Action for RenameSheetAction {
    fn id(&self) -> &'static str {
        "excel.rename_sheet"
    }
    fn summary(&self) -> &'static str {
        "Rename a worksheet"
    }
    fn schema(&self) -> &'static Value {
        static S: Lazy<Value> = Lazy::new(crate::schema::derive::<RenameSheetIn>);
        &S
    }
    async fn execute(&self, ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let RenameSheetIn {
            file,
            sheet,
            new_name,
        } = serde_json::from_value(input_with_xlsx_resource(ctx, input)?)
            .map_err(|e| StepError::msg(format!("excel.rename_sheet input invalid: {e}")))?;
        let old_name = sheet.clone();
        let result_name = new_name.clone();
        umya_mutate_book(ctx, file.clone(), move |book| {
            let index = book
                .get_sheet_collection_no_check()
                .iter()
                .position(|ws| ws.get_name() == sheet)
                .ok_or_else(|| format!("sheet `{sheet}` not found"))?;
            book.set_sheet_name(index, new_name).map_err(str::to_string)
        })
        .await?;
        Ok(ActionResult::from(serde_json::json!({
            "file": file,
            "from": old_name,
            "to": result_name,
        })))
    }
}

macro_rules! row_structure_action {
    ($action:ident, $input:ident, $id:literal, $summary:literal, $method:ident) => {
        pub struct $action;
        #[derive(Deserialize, JsonSchema)]
        #[serde(deny_unknown_fields)]
        struct $input {
            #[serde(default)]
    file: PathBuf,
            sheet: String,
            row: u32,
            #[serde(default = "default_structure_count")]
            count: u32,
        }
        #[async_trait]
        impl Action for $action {
            fn id(&self) -> &'static str { $id }
            fn summary(&self) -> &'static str { $summary }
            fn schema(&self) -> &'static Value {
                static S: Lazy<Value> = Lazy::new(crate::schema::derive::<$input>);
                &S
            }
            async fn execute(&self, ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
                let $input { file, sheet, row, count } = serde_json::from_value(input_with_xlsx_resource(ctx, input)?)
                    .map_err(|e| StepError::msg(format!("{} input invalid: {e}", self.id())))?;
                if row == 0 { return Err(StepError::msg(format!("{}: `row` must be >= 1", self.id()))); }
                let count = checked_structure_count(self.id(), count)?;
                let target_sheet = sheet.clone();
                umya_mutate_book(ctx, file.clone(), move |book| {
                    if book.get_sheet_by_name(&sheet).is_none() { return Err(format!("sheet `{sheet}` not found")); }
                    book.$method(&sheet, &row, &count);
                    Ok(())
                }).await?;
                Ok(ActionResult::from(serde_json::json!({"file": file, "sheet": target_sheet, "row": row, "count": count})))
            }
        }
    };
}

macro_rules! column_structure_action {
    ($action:ident, $input:ident, $id:literal, $summary:literal, $method:ident) => {
        pub struct $action;
        #[derive(Deserialize, JsonSchema)]
        #[serde(deny_unknown_fields)]
        struct $input {
            #[serde(default)]
    file: PathBuf,
            sheet: String,
            column: String,
            #[serde(default = "default_structure_count")]
            count: u32,
        }
        #[async_trait]
        impl Action for $action {
            fn id(&self) -> &'static str { $id }
            fn summary(&self) -> &'static str { $summary }
            fn schema(&self) -> &'static Value {
                static S: Lazy<Value> = Lazy::new(crate::schema::derive::<$input>);
                &S
            }
            async fn execute(&self, ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
                let $input { file, sheet, column, count } = serde_json::from_value(input_with_xlsx_resource(ctx, input)?)
                    .map_err(|e| StepError::msg(format!("{} input invalid: {e}", self.id())))?;
                parse_col(&column).map_err(StepError::msg)?;
                let count = checked_structure_count(self.id(), count)?;
                let target_sheet = sheet.clone();
                let target_column = column.to_ascii_uppercase();
                let column_for_task = target_column.clone();
                umya_mutate_book(ctx, file.clone(), move |book| {
                    if book.get_sheet_by_name(&sheet).is_none() { return Err(format!("sheet `{sheet}` not found")); }
                    book.$method(&sheet, &column_for_task, &count);
                    Ok(())
                }).await?;
                Ok(ActionResult::from(serde_json::json!({"file": file, "sheet": target_sheet, "column": target_column, "count": count})))
            }
        }
    };
}

const fn default_structure_count() -> u32 {
    1
}

row_structure_action!(
    InsertRowsAction,
    InsertRowsIn,
    "excel.insert_rows",
    "Insert rows before a 1-based row index",
    insert_new_row
);
row_structure_action!(
    DeleteRowsAction,
    DeleteRowsIn,
    "excel.delete_rows",
    "Delete rows from a 1-based row index",
    remove_row
);
column_structure_action!(
    InsertColumnsAction,
    InsertColumnsIn,
    "excel.insert_columns",
    "Insert columns before an A1 column",
    insert_new_column
);
column_structure_action!(
    DeleteColumnsAction,
    DeleteColumnsIn,
    "excel.delete_columns",
    "Delete columns from an A1 column",
    remove_column
);

/// Parse a single column letter run (e.g. `A`, `AB`) into a 0-based column index.
fn parse_col(s: &str) -> Result<usize, String> {
    let trimmed = s.trim();
    if trimmed.is_empty() || !trimmed.bytes().all(|b| b.is_ascii_alphabetic()) {
        return Err(format!("invalid column reference `{s}`"));
    }
    let mut col: usize = 0;
    for ch in trimmed.chars() {
        col = col
            .checked_mul(26)
            .and_then(|v| v.checked_add((ch.to_ascii_uppercase() as u8 - b'A' + 1) as usize))
            .ok_or_else(|| "column overflow".to_string())?;
    }
    Ok(col - 1)
}

/// Parse a column range `A` or `A:D` into an inclusive 0-based `(start, end)`,
/// normalizing so start ≤ end.
fn parse_col_range(s: &str) -> Result<(usize, usize), String> {
    let (a, b) = match s.split_once(':') {
        Some((a, b)) => (a, b),
        None => (s, s),
    };
    let c0 = parse_col(a)?;
    let c1 = parse_col(b)?;
    Ok((c0.min(c1), c0.max(c1)))
}

/// Parse a row range `1` or `1:5` into an inclusive 1-based `(start, end)`,
/// normalizing so start ≤ end.
fn parse_row_range(s: &str) -> Result<(u32, u32), String> {
    let (a, b) = match s.split_once(':') {
        Some((a, b)) => (a, b),
        None => (s, s),
    };
    let parse_one = |p: &str| -> Result<u32, String> {
        let n = p
            .trim()
            .parse::<u32>()
            .map_err(|_| format!("invalid row reference `{s}`"))?;
        if n == 0 {
            return Err(format!("invalid row reference `{s}`"));
        }
        Ok(n)
    };
    let r0 = parse_one(a)?;
    let r1 = parse_one(b)?;
    Ok((r0.min(r1), r0.max(r1)))
}

/// Convert a 0-based column index back into its A1 letter run (0 → `A`).
fn col_to_letters(mut col: usize) -> String {
    let mut out = Vec::new();
    loop {
        out.push(b'A' + (col % 26) as u8);
        if col < 26 {
            break;
        }
        col = col / 26 - 1;
    }
    out.reverse();
    String::from_utf8(out).expect("ascii")
}

pub struct SetStyleAction;

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SetStyleIn {
    #[serde(default)]
    file: PathBuf,
    /// 工作表名;留空取第一个工作表。
    #[serde(default)]
    sheet: Option<String>,
    /// 目标单元格或范围(如 `A1`、`A1:D10`),范围内每个单元格都会被应用。
    range: String,
    /// 粗体。
    #[serde(default)]
    bold: Option<bool>,
    /// 斜体。
    #[serde(default)]
    italic: Option<bool>,
    /// 字号(磅)。
    #[serde(default)]
    font_size: Option<f64>,
    /// 字体名(如 `Arial`、`微软雅黑`)。
    #[serde(default)]
    font_name: Option<String>,
    /// 字体颜色,ARGB 十六进制(如 `FFFF0000` 或 `FF0000`)。
    #[serde(default)]
    font_color: Option<String>,
    /// 背景填充色,ARGB 十六进制。
    #[serde(default)]
    bg_color: Option<String>,
    /// 水平对齐:`left`/`center`/`right`。
    #[serde(default)]
    align: Option<String>,
    /// 垂直对齐:`top`/`middle`/`bottom`。
    #[serde(default)]
    valign: Option<String>,
    /// 自动换行。
    #[serde(default)]
    wrap: Option<bool>,
    /// 数字格式代码(如 `#,##0.00`)。
    #[serde(default)]
    number_format: Option<String>,
    /// 边框粗细:`thin`/`medium`/`thick`(应用于四边)。
    #[serde(default)]
    border: Option<String>,
}

/// Normalize an ARGB/RGB hex string: 6-digit RGB is prefixed with an opaque
/// `FF` alpha; 8-digit ARGB is passed through. Both are upper-cased.
fn normalize_argb(s: &str) -> Result<String, String> {
    let h = s.trim().trim_start_matches('#');
    if !h.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(format!("invalid hex color `{s}`"));
    }
    match h.len() {
        6 => Ok(format!("FF{}", h.to_ascii_uppercase())),
        8 => Ok(h.to_ascii_uppercase()),
        _ => Err(format!(
            "color `{s}` must be 6 (RGB) or 8 (ARGB) hex digits"
        )),
    }
}

#[async_trait]
impl Action for SetStyleAction {
    fn id(&self) -> &'static str {
        "excel.set_style"
    }
    fn summary(&self) -> &'static str {
        "Apply font/fill/alignment/number-format/border styling to a cell or range (umya, preserves existing styles)"
    }
    fn schema(&self) -> &'static serde_json::Value {
        static SCHEMA: Lazy<Value> = Lazy::new(crate::schema::derive::<SetStyleIn>);
        &SCHEMA
    }
    async fn execute(&self, ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let SetStyleIn {
            file,
            sheet,
            range,
            bold,
            italic,
            font_size,
            font_name,
            font_color,
            bg_color,
            align,
            valign,
            wrap,
            number_format,
            border,
        } = serde_json::from_value(input_with_xlsx_resource(ctx, input)?)
            .map_err(|e| StepError::msg(format!("excel.set_style input invalid: {e}")))?;
        ctx.ensure_fs_read(&file)?;
        ctx.ensure_fs_write(&file)?;
        let ((r0, c0), (r1, c1)) = parse_range(&range).map_err(StepError::msg)?;

        // Pre-validate/normalize colors and enum strings off-thread so a bad
        // input fails fast with a clear message before the blocking work.
        let font_color = font_color
            .map(|c| normalize_argb(&c))
            .transpose()
            .map_err(StepError::msg)?;
        let bg_color = bg_color
            .map(|c| normalize_argb(&c))
            .transpose()
            .map_err(StepError::msg)?;
        let halign = match align.as_deref() {
            None => None,
            Some("left") => Some(umya_spreadsheet::HorizontalAlignmentValues::Left),
            Some("center") => Some(umya_spreadsheet::HorizontalAlignmentValues::Center),
            Some("right") => Some(umya_spreadsheet::HorizontalAlignmentValues::Right),
            Some(o) => {
                return Err(StepError::msg(format!(
                    "excel.set_style align must be left|center|right, got `{o}`"
                )))
            }
        };
        let valign_v = match valign.as_deref() {
            None => None,
            Some("top") => Some(umya_spreadsheet::VerticalAlignmentValues::Top),
            Some("middle") => Some(umya_spreadsheet::VerticalAlignmentValues::Center),
            Some("bottom") => Some(umya_spreadsheet::VerticalAlignmentValues::Bottom),
            Some(o) => {
                return Err(StepError::msg(format!(
                    "excel.set_style valign must be top|middle|bottom, got `{o}`"
                )))
            }
        };
        let border_style = match border.as_deref() {
            None => None,
            Some("thin") => Some(umya_spreadsheet::Border::BORDER_THIN),
            Some("medium") => Some(umya_spreadsheet::Border::BORDER_MEDIUM),
            Some("thick") => Some(umya_spreadsheet::Border::BORDER_THICK),
            Some(o) => {
                return Err(StepError::msg(format!(
                    "excel.set_style border must be thin|medium|thick, got `{o}`"
                )))
            }
        };

        let file_for_task = file.clone();
        let interrupt = ctx.step_interrupt();
        tokio::task::spawn_blocking(move || -> Result<(), String> {
            umya_mutate(&file_for_task, sheet, &interrupt, |ws| {
                for r in r0..=r1 {
                    for c in c0..=c1 {
                        let coord = format!("{}{}", col_to_letters(c), r + 1);
                        let s = ws.get_style_mut(&*coord);
                        if let Some(b) = bold {
                            s.get_font_mut().set_bold(b);
                        }
                        if let Some(i) = italic {
                            s.get_font_mut().set_italic(i);
                        }
                        if let Some(sz) = font_size {
                            s.get_font_mut().set_size(sz);
                        }
                        if let Some(ref name) = font_name {
                            s.get_font_mut().set_name(name.clone());
                        }
                        if let Some(ref col) = font_color {
                            s.get_font_mut().get_color_mut().set_argb(col.clone());
                        }
                        if let Some(ref col) = bg_color {
                            s.set_background_color(col.clone());
                        }
                        if let Some(h) = halign.clone() {
                            s.get_alignment_mut().set_horizontal(h);
                        }
                        if let Some(v) = valign_v.clone() {
                            s.get_alignment_mut().set_vertical(v);
                        }
                        if let Some(w) = wrap {
                            s.get_alignment_mut().set_wrap_text(w);
                        }
                        if let Some(ref fmt) = number_format {
                            s.get_number_format_mut().set_format_code(fmt.clone());
                        }
                        if let Some(bs) = border_style {
                            let borders = s.get_borders_mut();
                            borders.get_top_mut().set_border_style(bs);
                            borders.get_bottom_mut().set_border_style(bs);
                            borders.get_left_mut().set_border_style(bs);
                            borders.get_right_mut().set_border_style(bs);
                        }
                    }
                }
                Ok(())
            })
        })
        .await
        .map_err(|e| StepError::io(format!("excel join: {e}")))?
        .map_err(StepError::io)?;

        Ok(ActionResult::from(serde_json::json!({
            "file": file,
            "range": range,
        })))
    }
}

pub struct MergeCellsAction;

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct MergeCellsIn {
    #[serde(default)]
    file: PathBuf,
    /// 工作表名;留空取第一个工作表。
    #[serde(default)]
    sheet: Option<String>,
    /// 要合并的范围(如 `A1:D1`)。
    range: String,
    /// 可选:合并后写入左上角单元格的值。
    #[serde(default)]
    value: Option<Value>,
}

#[async_trait]
impl Action for MergeCellsAction {
    fn id(&self) -> &'static str {
        "excel.merge_cells"
    }
    fn summary(&self) -> &'static str {
        "Merge a cell range; optionally set the top-left cell value (umya, preserves styles)"
    }
    fn schema(&self) -> &'static serde_json::Value {
        static SCHEMA: Lazy<Value> = Lazy::new(crate::schema::derive::<MergeCellsIn>);
        &SCHEMA
    }
    async fn execute(&self, ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let MergeCellsIn {
            file,
            sheet,
            range,
            value,
        } = serde_json::from_value(input_with_xlsx_resource(ctx, input)?)
            .map_err(|e| StepError::msg(format!("excel.merge_cells input invalid: {e}")))?;
        ctx.ensure_fs_read(&file)?;
        ctx.ensure_fs_write(&file)?;
        let ((r0, c0), _) = parse_range(&range).map_err(StepError::msg)?;
        let top_left = format!("{}{}", col_to_letters(c0), r0 + 1);

        let file_for_task = file.clone();
        let range_for_task = range.clone();
        let interrupt = ctx.step_interrupt();
        tokio::task::spawn_blocking(move || -> Result<(), String> {
            umya_mutate(&file_for_task, sheet, &interrupt, |ws| {
                ws.add_merge_cells(range_for_task);
                if let Some(v) = value {
                    let text = match v {
                        Value::Null => String::new(),
                        Value::String(s) => s,
                        other => other.to_string(),
                    };
                    ws.get_cell_mut(&*top_left).set_value(text);
                }
                Ok(())
            })
        })
        .await
        .map_err(|e| StepError::io(format!("excel join: {e}")))?
        .map_err(StepError::io)?;

        Ok(ActionResult::from(serde_json::json!({
            "file": file,
            "range": range,
        })))
    }
}

pub struct SetColumnWidthAction;

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SetColumnWidthIn {
    #[serde(default)]
    file: PathBuf,
    /// 工作表名;留空取第一个工作表。
    #[serde(default)]
    sheet: Option<String>,
    /// 目标列或列范围(如 `A` 或 `A:D`)。
    columns: String,
    /// 列宽(Excel 字符宽度单位)。
    width: f64,
}

#[async_trait]
impl Action for SetColumnWidthAction {
    fn id(&self) -> &'static str {
        "excel.set_column_width"
    }
    fn summary(&self) -> &'static str {
        "Set the width of one column or a column range (umya, preserves styles)"
    }
    fn schema(&self) -> &'static serde_json::Value {
        static SCHEMA: Lazy<Value> = Lazy::new(crate::schema::derive::<SetColumnWidthIn>);
        &SCHEMA
    }
    async fn execute(&self, ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let SetColumnWidthIn {
            file,
            sheet,
            columns,
            width,
        } = serde_json::from_value(input_with_xlsx_resource(ctx, input)?)
            .map_err(|e| StepError::msg(format!("excel.set_column_width input invalid: {e}")))?;
        ctx.ensure_fs_read(&file)?;
        ctx.ensure_fs_write(&file)?;
        let (c0, c1) = parse_col_range(&columns).map_err(StepError::msg)?;

        let file_for_task = file.clone();
        let interrupt = ctx.step_interrupt();
        tokio::task::spawn_blocking(move || -> Result<(), String> {
            umya_mutate(&file_for_task, sheet, &interrupt, |ws| {
                for c in c0..=c1 {
                    let letters = col_to_letters(c);
                    ws.get_column_dimension_mut(&letters).set_width(width);
                }
                Ok(())
            })
        })
        .await
        .map_err(|e| StepError::io(format!("excel join: {e}")))?
        .map_err(StepError::io)?;

        Ok(ActionResult::from(serde_json::json!({
            "file": file,
            "columns": columns,
        })))
    }
}

pub struct SetRowHeightAction;

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SetRowHeightIn {
    #[serde(default)]
    file: PathBuf,
    /// 工作表名;留空取第一个工作表。
    #[serde(default)]
    sheet: Option<String>,
    /// 目标行或行范围(如 `1` 或 `1:5`)。
    rows: String,
    /// 行高(磅)。
    height: f64,
}

#[async_trait]
impl Action for SetRowHeightAction {
    fn id(&self) -> &'static str {
        "excel.set_row_height"
    }
    fn summary(&self) -> &'static str {
        "Set the height of one row or a row range (umya, preserves styles)"
    }
    fn schema(&self) -> &'static serde_json::Value {
        static SCHEMA: Lazy<Value> = Lazy::new(crate::schema::derive::<SetRowHeightIn>);
        &SCHEMA
    }
    async fn execute(&self, ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let SetRowHeightIn {
            file,
            sheet,
            rows,
            height,
        } = serde_json::from_value(input_with_xlsx_resource(ctx, input)?)
            .map_err(|e| StepError::msg(format!("excel.set_row_height input invalid: {e}")))?;
        ctx.ensure_fs_read(&file)?;
        ctx.ensure_fs_write(&file)?;
        let (r0, r1) = parse_row_range(&rows).map_err(StepError::msg)?;

        let file_for_task = file.clone();
        let interrupt = ctx.step_interrupt();
        tokio::task::spawn_blocking(move || -> Result<(), String> {
            umya_mutate(&file_for_task, sheet, &interrupt, |ws| {
                for r in r0..=r1 {
                    ws.get_row_dimension_mut(&r).set_height(height);
                }
                Ok(())
            })
        })
        .await
        .map_err(|e| StepError::io(format!("excel join: {e}")))?
        .map_err(StepError::io)?;

        Ok(ActionResult::from(serde_json::json!({
            "file": file,
            "rows": rows,
        })))
    }
}

pub struct FreezePanesAction;

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct FreezePanesIn {
    #[serde(default)]
    file: PathBuf,
    /// 工作表名;留空取第一个工作表。
    #[serde(default)]
    sheet: Option<String>,
    /// 冻结锚点单元格:`A2` 冻结第 1 行;`B2` 冻结第 1 行 + A 列;`B1` 冻结 A 列。
    top_left_cell: String,
}

#[async_trait]
impl Action for FreezePanesAction {
    fn id(&self) -> &'static str {
        "excel.freeze_panes"
    }
    fn summary(&self) -> &'static str {
        "Freeze rows/columns above/left of an anchor cell (umya, preserves styles)"
    }
    fn schema(&self) -> &'static serde_json::Value {
        static SCHEMA: Lazy<Value> = Lazy::new(crate::schema::derive::<FreezePanesIn>);
        &SCHEMA
    }
    async fn execute(&self, ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let FreezePanesIn {
            file,
            sheet,
            top_left_cell,
        } = serde_json::from_value(input_with_xlsx_resource(ctx, input)?)
            .map_err(|e| StepError::msg(format!("excel.freeze_panes input invalid: {e}")))?;
        ctx.ensure_fs_read(&file)?;
        ctx.ensure_fs_write(&file)?;
        // (row, col) are 0-based; the number of frozen rows/cols equals the
        // anchor's 0-based row/col (e.g. `B2` → freeze 1 row + 1 col).
        let (row, col) = parse_a1(&top_left_cell).map_err(StepError::msg)?;
        let anchor = top_left_cell.trim().to_string();

        let file_for_task = file.clone();
        let interrupt = ctx.step_interrupt();
        tokio::task::spawn_blocking(move || -> Result<(), String> {
            umya_mutate(&file_for_task, sheet, &interrupt, |ws| {
                let mut pane = umya_spreadsheet::Pane::default();
                pane.set_state(umya_spreadsheet::PaneStateValues::Frozen);
                pane.set_vertical_split(col as f64);
                pane.set_horizontal_split(row as f64);
                pane.set_active_pane(umya_spreadsheet::PaneValues::BottomRight);
                pane.get_top_left_cell_mut().set_coordinate(&anchor);

                let views = ws.get_sheet_views_mut();
                let list = views.get_sheet_view_list_mut();
                if list.is_empty() {
                    list.push(umya_spreadsheet::SheetView::default());
                }
                list[0].set_pane(pane);
                Ok(())
            })
        })
        .await
        .map_err(|e| StepError::io(format!("excel join: {e}")))?
        .map_err(StepError::io)?;

        Ok(ActionResult::from(serde_json::json!({
            "file": file,
            "top_left_cell": top_left_cell,
        })))
    }
}

// ─────────────────────────────────────────────────────────────────────────
// Round 3:图表 / 条件格式 / 自动列宽 / 批注 / 数据校验(同样走 umya,保留样式)
// ─────────────────────────────────────────────────────────────────────────

pub struct AddChartAction;

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct AddChartIn {
    #[serde(default)]
    file: PathBuf,
    /// 工作表名;留空取第一个工作表。
    #[serde(default)]
    sheet: Option<String>,
    /// 图表类型:`line`/`bar`/`column`/`pie`/`doughnut`/`scatter`/`area`/`radar`。
    /// 映射到 umya `ChartType`:`column` 与 `bar` 均映射为 `BarChart`(umya 2.3.3
    /// 不区分横/竖柱)。
    chart_type: String,
    /// 数据系列范围列表(如 `["A1:A10","B1:B10"]`)。会被自动**限定到本工作表**并
    /// 转成绝对引用形式 `Sheet1!$A$1:$A$10`。**数据须已写入工作表**,本动作只新增
    /// 引用这些范围的图表,不写数据。
    series: Vec<String>,
    /// 图表锚点左上角单元格(如 `C1`)。
    from_cell: String,
    /// 图表锚点右下角单元格(如 `H15`)。
    to_cell: String,
    /// 可选图表标题。
    #[serde(default)]
    title: Option<String>,
}

/// Sheet-qualify a plain A1 range (`A1:B10` / `A1`) into an absolute,
/// sheet-prefixed reference (`Sheet1!$A$1:$B$10`) for chart series. Reuses
/// `parse_range` + `col_to_letters` so it tracks the rest of the family.
fn qualify_series_range(sheet_name: &str, range: &str) -> Result<String, String> {
    let ((r0, c0), (r1, c1)) = parse_range(range)?;
    let start = format!("${}${}", col_to_letters(c0), r0 + 1);
    let end = format!("${}${}", col_to_letters(c1), r1 + 1);
    // A sheet name with spaces/specials must be quoted in a formula reference.
    let needs_quote = sheet_name
        .chars()
        .any(|ch| !ch.is_ascii_alphanumeric() && ch != '_');
    let qualified_sheet = if needs_quote {
        format!("'{}'", sheet_name.replace('\'', "''"))
    } else {
        sheet_name.to_string()
    };
    Ok(format!("{qualified_sheet}!{start}:{end}"))
}

#[async_trait]
impl Action for AddChartAction {
    fn id(&self) -> &'static str {
        "excel.add_chart"
    }
    fn summary(&self) -> &'static str {
        "Add a chart referencing existing data ranges (umya; data must already be in the sheet)"
    }
    fn schema(&self) -> &'static serde_json::Value {
        static SCHEMA: Lazy<Value> = Lazy::new(crate::schema::derive::<AddChartIn>);
        &SCHEMA
    }
    async fn execute(&self, ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let AddChartIn {
            file,
            sheet,
            chart_type,
            series,
            from_cell,
            to_cell,
            title,
        } = serde_json::from_value(input_with_xlsx_resource(ctx, input)?)
            .map_err(|e| StepError::msg(format!("excel.add_chart input invalid: {e}")))?;
        ctx.ensure_fs_read(&file)?;
        ctx.ensure_fs_write(&file)?;
        if series.is_empty() {
            return Err(StepError::msg("excel.add_chart `series` must not be empty"));
        }
        let kind = match chart_type.as_str() {
            "line" => umya_spreadsheet::structs::ChartType::LineChart,
            "bar" | "column" => umya_spreadsheet::structs::ChartType::BarChart,
            "pie" => umya_spreadsheet::structs::ChartType::PieChart,
            "doughnut" => umya_spreadsheet::structs::ChartType::DoughnutChart,
            "scatter" => umya_spreadsheet::structs::ChartType::ScatterChart,
            "area" => umya_spreadsheet::structs::ChartType::AreaChart,
            "radar" => umya_spreadsheet::structs::ChartType::RadarChart,
            o => {
                return Err(StepError::msg(format!(
                    "excel.add_chart chart_type must be line|bar|column|pie|doughnut|scatter|area|radar, got `{o}`"
                )))
            }
        };
        // Validate anchors up front for a clear error.
        let from_cell = from_cell.trim().to_string();
        let to_cell = to_cell.trim().to_string();
        parse_a1(&from_cell).map_err(StepError::msg)?;
        parse_a1(&to_cell).map_err(StepError::msg)?;
        let series_count = series.len();

        let file_for_task = file.clone();
        let interrupt = ctx.step_interrupt();
        tokio::task::spawn_blocking(move || -> Result<(), String> {
            umya_mutate(&file_for_task, sheet, &interrupt, |ws| {
                let sheet_name = ws.get_name().to_string();
                let qualified: Vec<String> = series
                    .iter()
                    .map(|r| qualify_series_range(&sheet_name, r))
                    .collect::<Result<_, _>>()?;
                let refs: Vec<&str> = qualified.iter().map(String::as_str).collect();

                let mut from_marker =
                    umya_spreadsheet::structs::drawing::spreadsheet::MarkerType::default();
                from_marker.set_coordinate(&from_cell);
                let mut to_marker =
                    umya_spreadsheet::structs::drawing::spreadsheet::MarkerType::default();
                to_marker.set_coordinate(&to_cell);

                let mut chart = umya_spreadsheet::structs::Chart::default();
                chart.new_chart(kind, from_marker, to_marker, refs);
                if let Some(ref t) = title {
                    chart.set_title(t.clone());
                }
                ws.add_chart(chart);
                Ok(())
            })
        })
        .await
        .map_err(|e| StepError::io(format!("excel join: {e}")))?
        .map_err(StepError::io)?;

        Ok(ActionResult::from(serde_json::json!({
            "file": file,
            "series": series_count,
        })))
    }
}

pub struct SetConditionalFormatAction;

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SetConditionalFormatIn {
    #[serde(default)]
    file: PathBuf,
    /// 工作表名;留空取第一个工作表。
    #[serde(default)]
    sheet: Option<String>,
    /// 应用范围(sqref,如 `A1:A20`)。
    range: String,
    /// 比较运算符:`greater_than`/`less_than`/`equal`/`greater_equal`/
    /// `less_equal`/`not_equal`。
    ///
    /// 注:umya 2.3.3 的 `ConditionalFormattingRule` 只能承载**单个**公式,故
    /// `between` 不受支持 —— 传入会报错(可改用两条单值规则)。
    operator: String,
    /// 比较阈值公式 1(数字或字符串,如 `30`)。
    formula1: String,
    /// 命中时高亮的背景色,ARGB 十六进制(复用 `normalize_argb`,接受 6 或 8 位)。
    bg_color: String,
}

#[async_trait]
impl Action for SetConditionalFormatAction {
    fn id(&self) -> &'static str {
        "excel.set_conditional_format"
    }
    fn summary(&self) -> &'static str {
        "Add a CellIs conditional-formatting rule highlighting matching cells (umya; single-formula operators)"
    }
    fn schema(&self) -> &'static serde_json::Value {
        static SCHEMA: Lazy<Value> = Lazy::new(crate::schema::derive::<SetConditionalFormatIn>);
        &SCHEMA
    }
    async fn execute(&self, ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let SetConditionalFormatIn {
            file,
            sheet,
            range,
            operator,
            formula1,
            bg_color,
        } = serde_json::from_value(input_with_xlsx_resource(ctx, input)?).map_err(|e| {
            StepError::msg(format!("excel.set_conditional_format input invalid: {e}"))
        })?;
        ctx.ensure_fs_read(&file)?;
        ctx.ensure_fs_write(&file)?;
        let op = match operator.as_str() {
            "greater_than" => umya_spreadsheet::ConditionalFormattingOperatorValues::GreaterThan,
            "less_than" => umya_spreadsheet::ConditionalFormattingOperatorValues::LessThan,
            "equal" => umya_spreadsheet::ConditionalFormattingOperatorValues::Equal,
            "greater_equal" => {
                umya_spreadsheet::ConditionalFormattingOperatorValues::GreaterThanOrEqual
            }
            "less_equal" => {
                umya_spreadsheet::ConditionalFormattingOperatorValues::LessThanOrEqual
            }
            "not_equal" => umya_spreadsheet::ConditionalFormattingOperatorValues::NotEqual,
            "between" => {
                return Err(StepError::msg(
                    "excel.set_conditional_format `between` is unsupported by umya 2.3.3's single-formula rule; use two single-value rules instead",
                ))
            }
            o => {
                return Err(StepError::msg(format!(
                    "excel.set_conditional_format operator must be greater_than|less_than|equal|greater_equal|less_equal|not_equal, got `{o}`"
                )))
            }
        };
        let bg = normalize_argb(&bg_color).map_err(StepError::msg)?;
        let sqref = range.trim().to_string();

        let file_for_task = file.clone();
        let interrupt = ctx.step_interrupt();
        tokio::task::spawn_blocking(move || -> Result<(), String> {
            umya_mutate(&file_for_task, sheet, &interrupt, |ws| {
                let mut formula = umya_spreadsheet::Formula::default();
                formula.set_string_value(formula1);

                let mut style = umya_spreadsheet::Style::default();
                style.set_background_color(bg);

                let mut rule = umya_spreadsheet::ConditionalFormattingRule::default();
                rule.set_type(umya_spreadsheet::ConditionalFormatValues::CellIs);
                rule.set_operator(op);
                rule.set_formula(formula);
                rule.set_style(style);
                rule.set_priority(1);

                let mut cf = umya_spreadsheet::ConditionalFormatting::default();
                cf.get_sequence_of_references_mut().set_sqref(sqref);
                cf.add_conditional_collection(rule);

                ws.add_conditional_formatting_collection(cf);
                Ok(())
            })
        })
        .await
        .map_err(|e| StepError::io(format!("excel join: {e}")))?
        .map_err(StepError::io)?;

        Ok(ActionResult::from(serde_json::json!({
            "file": file,
            "range": range,
        })))
    }
}

pub struct AutofitColumnsAction;

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct AutofitColumnsIn {
    #[serde(default)]
    file: PathBuf,
    /// 工作表名;留空取第一个工作表。
    #[serde(default)]
    sheet: Option<String>,
    /// 目标列或列范围(如 `A` 或 `A:F`)。
    columns: String,
}

#[async_trait]
impl Action for AutofitColumnsAction {
    fn id(&self) -> &'static str {
        "excel.autofit_columns"
    }
    fn summary(&self) -> &'static str {
        "Mark one column or a column range as auto-width (umya resolves it to a computed width on write)"
    }
    fn schema(&self) -> &'static serde_json::Value {
        static SCHEMA: Lazy<Value> = Lazy::new(crate::schema::derive::<AutofitColumnsIn>);
        &SCHEMA
    }
    async fn execute(&self, ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let AutofitColumnsIn {
            file,
            sheet,
            columns,
        } = serde_json::from_value(input_with_xlsx_resource(ctx, input)?)
            .map_err(|e| StepError::msg(format!("excel.autofit_columns input invalid: {e}")))?;
        ctx.ensure_fs_read(&file)?;
        ctx.ensure_fs_write(&file)?;
        let (c0, c1) = parse_col_range(&columns).map_err(StepError::msg)?;

        let file_for_task = file.clone();
        let interrupt = ctx.step_interrupt();
        tokio::task::spawn_blocking(move || -> Result<(), String> {
            umya_mutate(&file_for_task, sheet, &interrupt, |ws| {
                for c in c0..=c1 {
                    let letters = col_to_letters(c);
                    ws.get_column_dimension_mut(&letters).set_auto_width(true);
                }
                Ok(())
            })
        })
        .await
        .map_err(|e| StepError::io(format!("excel join: {e}")))?
        .map_err(StepError::io)?;

        Ok(ActionResult::from(serde_json::json!({
            "file": file,
            "columns": columns,
        })))
    }
}

pub struct SetCommentAction;

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SetCommentIn {
    #[serde(default)]
    file: PathBuf,
    /// 工作表名;留空取第一个工作表。
    #[serde(default)]
    sheet: Option<String>,
    /// 目标单元格(如 `B2`)。
    cell: String,
    /// 批注文本。
    text: String,
    /// 可选批注作者。
    #[serde(default)]
    author: Option<String>,
}

#[async_trait]
impl Action for SetCommentAction {
    fn id(&self) -> &'static str {
        "excel.set_comment"
    }
    fn summary(&self) -> &'static str {
        "Attach a comment/note to a cell (umya, preserves styles)"
    }
    fn schema(&self) -> &'static serde_json::Value {
        static SCHEMA: Lazy<Value> = Lazy::new(crate::schema::derive::<SetCommentIn>);
        &SCHEMA
    }
    async fn execute(&self, ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let SetCommentIn {
            file,
            sheet,
            cell,
            text,
            author,
        } = serde_json::from_value(input_with_xlsx_resource(ctx, input)?)
            .map_err(|e| StepError::msg(format!("excel.set_comment input invalid: {e}")))?;
        ctx.ensure_fs_read(&file)?;
        ctx.ensure_fs_write(&file)?;
        // Validate the cell ref up front for a clear error.
        parse_a1(&cell).map_err(StepError::msg)?;
        let cell_ref = cell.trim().to_string();

        let file_for_task = file.clone();
        let interrupt = ctx.step_interrupt();
        tokio::task::spawn_blocking(move || -> Result<(), String> {
            umya_mutate(&file_for_task, sheet, &interrupt, |ws| {
                let mut comment = umya_spreadsheet::Comment::default();
                comment.new_comment(&*cell_ref);
                comment.set_text_string(text);
                if let Some(a) = author {
                    comment.set_author(a);
                }
                ws.add_comments(comment);
                Ok(())
            })
        })
        .await
        .map_err(|e| StepError::io(format!("excel join: {e}")))?
        .map_err(StepError::io)?;

        Ok(ActionResult::from(serde_json::json!({
            "file": file,
            "cell": cell,
        })))
    }
}

pub struct SetDataValidationAction;

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SetDataValidationIn {
    #[serde(default)]
    file: PathBuf,
    /// 工作表名;留空取第一个工作表。
    #[serde(default)]
    sheet: Option<String>,
    /// 应用范围(sqref,如 `A1:A20`)。
    range: String,
    /// 校验类型:`list`(下拉)、`whole`(整数)、`decimal`(小数)。
    kind: String,
    /// `list` 下拉候选项(转成带引号的逗号串 `"a,b,c"`);也可改用 `formula1` 传范围。
    #[serde(default)]
    values: Option<Vec<String>>,
    /// `whole`/`decimal` 的比较运算符:`between`/`equal`/`greater_than`/`less_than`/
    /// `greater_equal`/`less_equal`/`not_equal`/`not_between`。
    #[serde(default)]
    operator: Option<String>,
    /// 数字校验阈值公式 1(`list` 时若未给 `values`,作为来源范围/公式)。
    #[serde(default)]
    formula1: Option<String>,
    /// `between`/`not_between` 的阈值公式 2。
    #[serde(default)]
    formula2: Option<String>,
}

#[async_trait]
impl Action for SetDataValidationAction {
    fn id(&self) -> &'static str {
        "excel.set_data_validation"
    }
    fn summary(&self) -> &'static str {
        "Add a list dropdown or whole/decimal number validation to a range (umya, preserves styles)"
    }
    fn schema(&self) -> &'static serde_json::Value {
        static SCHEMA: Lazy<Value> = Lazy::new(crate::schema::derive::<SetDataValidationIn>);
        &SCHEMA
    }
    async fn execute(&self, ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let SetDataValidationIn {
            file,
            sheet,
            range,
            kind,
            values,
            operator,
            formula1,
            formula2,
        } = serde_json::from_value(input_with_xlsx_resource(ctx, input)?)
            .map_err(|e| StepError::msg(format!("excel.set_data_validation input invalid: {e}")))?;
        ctx.ensure_fs_read(&file)?;
        ctx.ensure_fs_write(&file)?;

        // Resolve the (type, formula1, formula2, operator) tuple off-thread so a
        // bad combination fails fast before the blocking work.
        let (dv_type, f1, f2, dv_op) = match kind.as_str() {
            "list" => {
                let f1 = match (&values, &formula1) {
                    (Some(vs), _) if !vs.is_empty() => format!("\"{}\"", vs.join(",")),
                    (_, Some(f)) => f.clone(),
                    _ => {
                        return Err(StepError::msg(
                            "excel.set_data_validation `list` requires `values` or `formula1`",
                        ))
                    }
                };
                (
                    umya_spreadsheet::DataValidationValues::List,
                    Some(f1),
                    None,
                    None,
                )
            }
            "whole" | "decimal" => {
                let op_str = operator.as_deref().ok_or_else(|| {
                    StepError::msg(
                        "excel.set_data_validation `whole`/`decimal` requires `operator`",
                    )
                })?;
                let op = match op_str {
                    "between" => umya_spreadsheet::DataValidationOperatorValues::Between,
                    "not_between" => umya_spreadsheet::DataValidationOperatorValues::NotBetween,
                    "equal" => umya_spreadsheet::DataValidationOperatorValues::Equal,
                    "not_equal" => umya_spreadsheet::DataValidationOperatorValues::NotEqual,
                    "greater_than" => umya_spreadsheet::DataValidationOperatorValues::GreaterThan,
                    "less_than" => umya_spreadsheet::DataValidationOperatorValues::LessThan,
                    "greater_equal" => {
                        umya_spreadsheet::DataValidationOperatorValues::GreaterThanOrEqual
                    }
                    "less_equal" => umya_spreadsheet::DataValidationOperatorValues::LessThanOrEqual,
                    o => {
                        return Err(StepError::msg(format!(
                            "excel.set_data_validation operator invalid: `{o}`"
                        )))
                    }
                };
                let f1 = formula1.ok_or_else(|| {
                    StepError::msg(
                        "excel.set_data_validation `whole`/`decimal` requires `formula1`",
                    )
                })?;
                let needs_two = matches!(op_str, "between" | "not_between");
                if needs_two && formula2.is_none() {
                    return Err(StepError::msg(
                        "excel.set_data_validation `between`/`not_between` requires `formula2`",
                    ));
                }
                let dv_type = if kind == "whole" {
                    umya_spreadsheet::DataValidationValues::Whole
                } else {
                    umya_spreadsheet::DataValidationValues::Decimal
                };
                (dv_type, Some(f1), formula2.clone(), Some(op))
            }
            o => {
                return Err(StepError::msg(format!(
                    "excel.set_data_validation kind must be list|whole|decimal, got `{o}`"
                )))
            }
        };
        let sqref = range.trim().to_string();

        let file_for_task = file.clone();
        let interrupt = ctx.step_interrupt();
        tokio::task::spawn_blocking(move || -> Result<(), String> {
            umya_mutate(&file_for_task, sheet, &interrupt, |ws| {
                let mut dv = umya_spreadsheet::DataValidation::default();
                dv.set_type(dv_type);
                if let Some(op) = dv_op {
                    dv.set_operator(op);
                }
                if let Some(f) = f1 {
                    dv.set_formula1(f);
                }
                if let Some(f) = f2 {
                    dv.set_formula2(f);
                }
                dv.set_allow_blank(true);
                dv.get_sequence_of_references_mut().set_sqref(sqref);

                if ws.get_data_validations().is_none() {
                    ws.set_data_validations(umya_spreadsheet::DataValidations::default());
                }
                ws.get_data_validations_mut()
                    .expect("just ensured present")
                    .add_data_validation_list(dv);
                Ok(())
            })
        })
        .await
        .map_err(|e| StepError::io(format!("excel join: {e}")))?
        .map_err(StepError::io)?;

        Ok(ActionResult::from(serde_json::json!({
            "file": file,
            "range": range,
        })))
    }
}

pub struct LookupAction;

/// 列定位:`2`(1-based 序号)/ `"B"`(列字母)/ `"姓名"`(表头名)。
/// 字符串的解析顺序:①纯数字 → 1-based 序号;②与表头行单元格精确匹配 → 表头名;
/// ③1~3 个 ASCII 字母 → 列字母;④否则报错。表头名优先于列字母,因为像 `"Name"`
/// 这样的表头同时也是合法字母串(列 #264777),按表头解释才符合直觉。
#[derive(Deserialize, JsonSchema)]
#[serde(untagged)]
enum ColumnRef {
    /// 1-based 列序号。
    Index(u64),
    /// 列字母(`A`/`AB`)或表头名。
    Name(String),
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct LookupIn {
    #[serde(default)]
    file: PathBuf,
    /// 工作表名;留空取第一个工作表。
    #[serde(default)]
    sheet: Option<String>,
    /// 在哪一列里找 `key_value`(序号/字母/表头名,见 `ColumnRef`)。
    key_column: ColumnRef,
    /// 要查找的键值。匹配为同类型精确比较,见动作实现里的类型说明。
    key_value: Value,
    /// 命中后取哪一列的值(序号/字母/表头名)。
    value_column: ColumnRef,
    /// 表头所在行(1-based),默认 1。表头名定位时从该行读表头;数据扫描从
    /// `header_row + 1` 行开始。设为 0 表示无表头(从第 1 行扫起,此时不能用表头名)。
    #[serde(default = "default_header_row")]
    header_row: u32,
    /// false(默认)只取第一个命中;true 返回全部命中(数组)。
    #[serde(default)]
    all_matches: bool,
    /// 单值模式未命中时返回它(`found: false`)而不报错;不给且未命中 → 报错。
    /// `all_matches: true` 时未命中返回空数组,不报错,本字段不参与。
    #[serde(default)]
    default: Option<Value>,
}

fn default_header_row() -> u32 {
    1
}

/// 键值与单元格的匹配策略:**同类型精确比较**。
/// * 字符串 ↔ 字符串单元格:逐字符相等;
/// * 数字 ↔ 数字单元格:按 f64 数值比较(`1`、`1.0` 视为相等);
/// * 布尔 ↔ 布尔单元格:相等比较;
/// * 跨类型一律不匹配 —— Excel 里数字 `1` 与文本 `"1"` 是不同的单元格类型,
///   静默互转极易掩盖脏数据;需要跨类型匹配时由调用方先把键转成目标类型。
fn lookup_matches(key: &Value, cell: &Data) -> bool {
    match (key, cell) {
        (Value::String(k), Data::String(c)) => k == c,
        (Value::Number(k), Data::Float(c)) => k.as_f64() == Some(*c),
        (Value::Number(k), Data::Int(c)) => k.as_f64() == Some(*c as f64),
        (Value::Number(k), Data::DateTime(c)) => k.as_f64() == Some(c.as_f64()),
        (Value::Bool(k), Data::Bool(c)) => k == c,
        _ => false,
    }
}

/// 把 `ColumnRef` 解析成 0-based 绝对列号。`headers` 是表头行单元格文本
/// (`header_row >= 1` 时才有),`(列号, 文本)` 对。
fn resolve_lookup_column(
    which: &str,
    col: &ColumnRef,
    headers: &[(usize, String)],
    header_row: u32,
) -> Result<usize, String> {
    match col {
        ColumnRef::Index(n) => {
            if *n == 0 {
                return Err(format!("excel.lookup {which} index is 1-based, got 0"));
            }
            Ok((*n - 1) as usize)
        }
        ColumnRef::Name(s) => {
            let t = s.trim();
            if t.is_empty() {
                return Err(format!("excel.lookup {which} must not be empty"));
            }
            // ① 纯数字字符串按 1-based 序号。
            if t.bytes().all(|b| b.is_ascii_digit()) {
                let n: usize = t
                    .parse()
                    .map_err(|_| format!("excel.lookup {which} `{t}` overflows"))?;
                if n == 0 {
                    return Err(format!("excel.lookup {which} index is 1-based, got 0"));
                }
                return Ok(n - 1);
            }
            // ② 表头名精确匹配(优先于字母,见 ColumnRef 文档)。
            if let Some((c, _)) = headers.iter().find(|(_, h)| h == t) {
                return Ok(*c);
            }
            // ③ 1~3 个 ASCII 字母按列字母(XLSX 最大列 XFD 即 3 个字母)。
            if t.len() <= 3 && t.bytes().all(|b| b.is_ascii_alphabetic()) {
                return parse_col(t);
            }
            Err(format!(
                "excel.lookup {which} `{t}` not found in header row {header_row} and not a valid column letter/index"
            ))
        }
    }
}

#[async_trait]
impl Action for LookupAction {
    fn id(&self) -> &'static str {
        "excel.lookup"
    }
    fn summary(&self) -> &'static str {
        "VLOOKUP-style search: find key_value in key_column, return value_column from matching row(s)"
    }
    fn schema(&self) -> &'static serde_json::Value {
        static SCHEMA: Lazy<Value> = Lazy::new(crate::schema::derive::<LookupIn>);
        &SCHEMA
    }
    async fn execute(&self, ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let LookupIn {
            file,
            sheet,
            key_column,
            key_value,
            value_column,
            header_row,
            all_matches,
            default,
        } = serde_json::from_value(input_with_xlsx_resource(ctx, input)?)
            .map_err(|e| StepError::msg(format!("excel.lookup input invalid: {e}")))?;
        // 只读不写,仅需 fs.read(与 read_rows/read_range 一致,走 calamine)。
        ctx.ensure_fs_read(&file)?;

        let out = tokio::task::spawn_blocking(move || -> Result<Value, String> {
            let mut wb = open_workbook_auto(&file).map_err(|e| e.to_string())?;
            let sheet_name = match sheet {
                Some(s) => s,
                None => wb
                    .sheet_names()
                    .first()
                    .cloned()
                    .ok_or_else(|| "workbook has no sheets".to_string())?,
            };
            let range = wb.worksheet_range(&sheet_name).map_err(|e| e.to_string())?;

            // 表头行单元格文本(header_row >= 1 时),供表头名定位。
            let mut headers: Vec<(usize, String)> = Vec::new();
            if header_row >= 1 {
                if let (Some(start), Some(end)) = (range.start(), range.end()) {
                    let hr = header_row - 1; // 0-based 绝对行号
                    for c in start.1..=end.1 {
                        if let Some(cell) = range.get_value((hr, c)) {
                            let text = match cell {
                                Data::String(s) => s.clone(),
                                Data::Empty => continue,
                                other => other.to_string(),
                            };
                            headers.push((c as usize, text));
                        }
                    }
                }
            }

            let key_col =
                resolve_lookup_column("key_column", &key_column, &headers, header_row)?;
            let val_col =
                resolve_lookup_column("value_column", &value_column, &headers, header_row)?;

            // 数据扫描从 header_row + 1 行(1-based)开始;header_row = 0 → 从第 1 行。
            let mut values: Vec<Value> = Vec::new();
            let mut row_numbers: Vec<u64> = Vec::new();
            if let Some(end) = range.end() {
                for r in header_row..=end.0 {
                    let hit = range
                        .get_value((r, key_col as u32))
                        .is_some_and(|cell| lookup_matches(&key_value, cell));
                    if hit {
                        let v = range
                            .get_value((r, val_col as u32))
                            .map(cell_to_json)
                            .unwrap_or(Value::Null);
                        values.push(v);
                        row_numbers.push(u64::from(r) + 1); // 1-based 工作表行号
                        if !all_matches {
                            break;
                        }
                    }
                }
            }

            let found = !values.is_empty();
            if all_matches {
                // all_matches 模式:未命中返回空数组即可,这本身就是合法答案,
                // 不报错、不使用 default。
                return Ok(serde_json::json!({
                    "found": found,
                    "values": values,
                    "row_numbers": row_numbers,
                }));
            }
            if found {
                return Ok(serde_json::json!({
                    "found": true,
                    "value": values[0],
                    "row_number": row_numbers[0],
                }));
            }
            match default {
                Some(d) => Ok(serde_json::json!({ "found": false, "value": d })),
                None => Err(format!(
                    "excel.lookup: key {key_value} not found in sheet `{sheet_name}` (no `default` given)"
                )),
            }
        })
        .await
        .map_err(|e| StepError::io(format!("excel join: {e}")))?
        .map_err(StepError::io)?;
        Ok(ActionResult::from(out))
    }
}

/// Prefix marking a string cell as a formula so `write_cell` emits it via
/// `write_formula`. Round-tripping through calamine drops it (calamine reports
/// the cached value, not the formula), so it is purely a write-side marker.
const FORMULA_SENTINEL: &str = "\u{0}__lumo_formula__\u{0}";

fn load_sheets(file: &PathBuf) -> Result<BTreeMap<String, Vec<Vec<Value>>>, String> {
    let mut sheets: BTreeMap<String, Vec<Vec<Value>>> = BTreeMap::new();
    if !file.exists() {
        return Ok(sheets);
    }
    let mut wb = open_workbook_auto(file).map_err(|e| e.to_string())?;
    for sheet_name in wb.sheet_names().to_vec() {
        if let Ok(range) = wb.worksheet_range(&sheet_name) {
            let rows = range
                .rows()
                .map(|r| r.iter().map(cell_to_json).collect::<Vec<_>>())
                .collect::<Vec<_>>();
            sheets.insert(sheet_name, rows);
        }
    }
    Ok(sheets)
}

fn save_sheets(
    file: &PathBuf,
    sheets: &BTreeMap<String, Vec<Vec<Value>>>,
    interrupt: &lumo_core::StepInterrupt,
) -> Result<(), String> {
    let mut wb = Workbook::new();
    for (sheet_name, rows) in sheets {
        let ws = wb.add_worksheet();
        ws.set_name(sheet_name).map_err(|e| e.to_string())?;
        for (r, row) in rows.iter().enumerate() {
            for (c, v) in row.iter().enumerate() {
                write_cell(ws, r as u32, c as u16, v).map_err(|e| e.to_string())?;
            }
        }
    }
    // P0-2:save 是副作用唯一落地点 —— 步骤判死后孤儿任务到此止步,文件不动。
    if interrupt.is_interrupted() {
        return Err("excel write-back interrupted (file untouched)".into());
    }
    wb.save(file).map_err(|e| e.to_string())
}

fn parse_a1(cell: &str) -> Result<(usize, usize), String> {
    let trimmed = cell.trim();
    if trimmed.is_empty() {
        return Err("cell must be an A1 reference".into());
    }
    let mut col: usize = 0;
    let mut row_part = String::new();
    for ch in trimmed.chars() {
        if ch.is_ascii_alphabetic() && row_part.is_empty() {
            col = col
                .checked_mul(26)
                .and_then(|v| v.checked_add((ch.to_ascii_uppercase() as u8 - b'A' + 1) as usize))
                .ok_or_else(|| "cell column overflow".to_string())?;
        } else if ch.is_ascii_digit() {
            row_part.push(ch);
        } else {
            return Err(format!("invalid A1 cell reference `{cell}`"));
        }
    }
    if col == 0 || row_part.is_empty() {
        return Err(format!("invalid A1 cell reference `{cell}`"));
    }
    let row = row_part
        .parse::<usize>()
        .map_err(|_| format!("invalid A1 row in `{cell}`"))?;
    if row == 0 {
        return Err(format!("invalid A1 row in `{cell}`"));
    }
    Ok((row - 1, col - 1))
}

/// Inclusive 0-based cell range: `((start_row, start_col), (end_row, end_col))`.
type CellRange = ((usize, usize), (usize, usize));

/// Parse an `A1:D100` range into inclusive `((r0,c0),(r1,c1))` (0-based),
/// normalizing so start ≤ end on both axes. A bare `A1` is treated as a 1×1
/// range.
fn parse_range(range: &str) -> Result<CellRange, String> {
    let trimmed = range.trim();
    let (start, end) = match trimmed.split_once(':') {
        Some((a, b)) => (a, b),
        None => (trimmed, trimmed),
    };
    let (r0, c0) = parse_a1(start)?;
    let (r1, c1) = parse_a1(end)?;
    Ok(((r0.min(r1), c0.min(c1)), (r0.max(r1), c0.max(c1))))
}

fn write_cell(
    ws: &mut rust_xlsxwriter::Worksheet,
    row: u32,
    col: u16,
    v: &Value,
) -> Result<(), rust_xlsxwriter::XlsxError> {
    match v {
        Value::Null => {
            ws.write_blank(row, col, &rust_xlsxwriter::Format::default())?;
        }
        Value::Bool(b) => {
            ws.write_boolean(row, col, *b)?;
        }
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                ws.write_number(row, col, i as f64)?;
            } else if let Some(f) = n.as_f64() {
                ws.write_number(row, col, f)?;
            } else {
                ws.write_string(row, col, n.to_string())?;
            }
        }
        Value::String(s) => {
            if let Some(formula) = s.strip_prefix(FORMULA_SENTINEL) {
                ws.write_formula(row, col, rust_xlsxwriter::Formula::new(formula))?;
            } else {
                ws.write_string(row, col, s)?;
            }
        }
        other => {
            ws.write_string(row, col, other.to_string())?;
        }
    }
    Ok(())
}
