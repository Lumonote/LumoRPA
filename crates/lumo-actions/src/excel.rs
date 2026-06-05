//! Excel read/write actions backed by:
//!   * `calamine` — fast pure-Rust reader (xls/xlsx/ods)
//!   * `rust_xlsxwriter` — pure-Rust writer

use async_trait::async_trait;
use calamine::{open_workbook_auto, Data, Reader};
use lumo_core::error::StepError;
use lumo_core::{Action, ActionRegistry, ActionResult, StepCtx};
use once_cell::sync::Lazy;
use rust_xlsxwriter::Workbook;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Map, Value};
use std::collections::BTreeMap;
use std::path::PathBuf;

pub fn register(r: &mut ActionRegistry) {
    r.register(ReadRowsAction);
    r.register(WriteRowAction);
    r.register(SheetNamesAction);
    r.register(ReadCellAction);
    r.register(WriteCellAction);
    r.register(ReadRangeAction);
    r.register(WriteRangeAction);
    r.register(FindReplaceAction);
    r.register(SetFormulaAction);
}

pub struct ReadRowsAction;
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ReadIn {
    file: PathBuf,
    #[serde(default)]
    sheet: Option<String>,
    #[serde(default = "default_true")]
    header: bool,
    #[serde(default)]
    limit: Option<usize>,
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
        } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("excel.read_rows input invalid: {e}")))?;
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
                if let Some(n) = limit {
                    if out.len() >= n {
                        break;
                    }
                }
            }
            Ok(out)
        })
        .await
        .map_err(|e| StepError::msg(format!("excel join: {e}")))?
        .map_err(StepError::msg)?;
        Ok(ActionResult::from(Value::Array(rows)))
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
        } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("excel.write_row input invalid: {e}")))?;
        ctx.ensure_fs_write(&file)?;
        let sheet = sheet.unwrap_or_else(|| "Sheet1".into());

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
            wb.save(&file).map_err(|e| e.to_string())?;
            Ok(existing_rows.len())
        })
        .await
        .map_err(|e| StepError::msg(format!("excel join: {e}")))?
        .map_err(StepError::msg)
        .map(|n| ActionResult::from(serde_json::json!({ "rows": n })))
    }
}

pub struct SheetNamesAction;

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SheetNamesIn {
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
        let SheetNamesIn { file } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("excel.sheet_names input invalid: {e}")))?;
        ctx.ensure_fs_read(&file)?;
        let names = tokio::task::spawn_blocking(move || -> Result<Vec<String>, String> {
            let wb = open_workbook_auto(&file).map_err(|e| e.to_string())?;
            Ok(wb.sheet_names().to_vec())
        })
        .await
        .map_err(|e| StepError::msg(format!("excel join: {e}")))?
        .map_err(StepError::msg)?;
        Ok(ActionResult::from(serde_json::json!({
            "sheets": names,
        })))
    }
}

pub struct ReadCellAction;

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ReadCellIn {
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
        let ReadCellIn { file, sheet, cell } = serde_json::from_value(input)
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
        .map_err(|e| StepError::msg(format!("excel join: {e}")))?
        .map_err(StepError::msg)?;
        Ok(ActionResult::from(out))
    }
}

pub struct WriteCellAction;

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct WriteCellIn {
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
        } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("excel.write_cell input invalid: {e}")))?;
        ctx.ensure_fs_write(&file)?;
        let sheet = sheet.unwrap_or_else(|| "Sheet1".into());
        let (row, col) = parse_a1(&cell).map_err(StepError::msg)?;
        if col > u16::MAX as usize {
            return Err(StepError::msg("excel.write_cell column exceeds XLSX limit"));
        }

        let file_for_task = file.clone();
        let sheet_for_task = sheet.clone();
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
            save_sheets(&file_for_task, &sheets)
        })
        .await
        .map_err(|e| StepError::msg(format!("excel join: {e}")))?
        .map_err(StepError::msg)?;

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
        let ReadRangeIn { file, sheet, range } = serde_json::from_value(input)
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
        .map_err(|e| StepError::msg(format!("excel join: {e}")))?
        .map_err(StepError::msg)?;
        Ok(ActionResult::from(Value::Array(rows)))
    }
}

pub struct WriteRangeAction;

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct WriteRangeIn {
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
        } = serde_json::from_value(input)
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
            return Err(StepError::msg("excel.write_range column exceeds XLSX limit"));
        }

        let file_for_task = file.clone();
        let sheet_for_task = sheet.clone();
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
            save_sheets(&file_for_task, &sheets)?;
            Ok(cells)
        })
        .await
        .map_err(|e| StepError::msg(format!("excel join: {e}")))?
        .map_err(StepError::msg)?;

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
        } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("excel.find_replace input invalid: {e}")))?;
        ctx.ensure_fs_read(&file)?;
        ctx.ensure_fs_write(&file)?;
        if find.is_empty() {
            return Err(StepError::msg("excel.find_replace `find` must not be empty"));
        }

        let file_for_task = file.clone();
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
            save_sheets(&file_for_task, &sheets)?;
            Ok(count)
        })
        .await
        .map_err(|e| StepError::msg(format!("excel join: {e}")))?
        .map_err(StepError::msg)?;

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
        "Write a formula to a cell by A1 reference (create workbook if missing)"
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
        } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("excel.set_formula input invalid: {e}")))?;
        ctx.ensure_fs_read(&file)?;
        ctx.ensure_fs_write(&file)?;
        let sheet = sheet.unwrap_or_else(|| "Sheet1".into());
        let (row, col) = parse_a1(&cell).map_err(StepError::msg)?;
        if col > u16::MAX as usize {
            return Err(StepError::msg("excel.set_formula column exceeds XLSX limit"));
        }

        // Sentinel-wrap the formula so it survives the BTreeMap<…, Value> sheet
        // model (which only knows JSON scalars) and is re-emitted via
        // `write_formula` rather than `write_string` by `write_cell`.
        let file_for_task = file.clone();
        let sheet_for_task = sheet.clone();
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
            save_sheets(&file_for_task, &sheets)
        })
        .await
        .map_err(|e| StepError::msg(format!("excel join: {e}")))?
        .map_err(StepError::msg)?;

        Ok(ActionResult::from(serde_json::json!({
            "file": file,
            "sheet": sheet,
            "cell": cell,
        })))
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

fn save_sheets(file: &PathBuf, sheets: &BTreeMap<String, Vec<Vec<Value>>>) -> Result<(), String> {
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
