//! Excel read/write actions backed by:
//!   * `calamine` — fast pure-Rust reader (xls/xlsx/ods)
//!   * `rust_xlsxwriter` — pure-Rust writer

use async_trait::async_trait;
use calamine::{open_workbook_auto, Data, Reader};
use lumo_core::{Action, ActionRegistry, ActionResult, StepCtx};
use lumo_core::error::StepError;
use rust_xlsxwriter::Workbook;
use serde::Deserialize;
use serde_json::{Map, Value};
use std::path::PathBuf;

pub fn register(r: &mut ActionRegistry) {
    r.register(ReadRowsAction);
    r.register(WriteRowAction);
}

pub struct ReadRowsAction;
#[derive(Deserialize)]
struct ReadIn {
    file: PathBuf,
    #[serde(default)]
    sheet: Option<String>,
    #[serde(default = "default_true")]
    header: bool,
    #[serde(default)]
    limit: Option<usize>,
}
fn default_true() -> bool { true }

#[async_trait]
impl Action for ReadRowsAction {
    fn id(&self) -> &'static str { "excel.read_rows" }
    fn summary(&self) -> &'static str { "Read rows from a workbook; row 1 used as headers if header=true" }
    async fn execute(&self, _ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let ReadIn { file, sheet, header, limit } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("excel.read_rows input invalid: {e}")))?;
        let rows = tokio::task::spawn_blocking(move || -> Result<Vec<Value>, String> {
            let mut wb = open_workbook_auto(&file).map_err(|e| e.to_string())?;
            let sheet_name = match sheet {
                Some(s) => s,
                None => wb.sheet_names().first().cloned()
                    .ok_or_else(|| "workbook has no sheets".to_string())?,
            };
            let range = wb.worksheet_range(&sheet_name).map_err(|e| e.to_string())?;
            let mut out: Vec<Value> = Vec::new();
            let mut iter = range.rows();
            let headers: Vec<String> = if header {
                iter.next().map(|r| header_row(r)).unwrap_or_default()
            } else { Vec::new() };
            for (idx, row) in iter.enumerate() {
                let mut obj = Map::new();
                for (i, cell) in row.iter().enumerate() {
                    let key = headers.get(i).cloned().unwrap_or_else(|| format!("col_{i}"));
                    obj.insert(key, cell_to_json(cell));
                }
                obj.insert("_index".into(), Value::from(idx as i64));
                out.push(Value::Object(obj));
                if let Some(n) = limit { if out.len() >= n { break; } }
            }
            Ok(out)
        }).await
            .map_err(|e| StepError::msg(format!("excel join: {e}")))?
            .map_err(StepError::msg)?;
        Ok(ActionResult::from(Value::Array(rows)))
    }
}

fn header_row(r: &[Data]) -> Vec<String> {
    r.iter().enumerate().map(|(i, c)| match c {
        Data::String(s) => s.clone(),
        Data::Empty => format!("col_{i}"),
        other => other.to_string(),
    }).collect()
}

fn cell_to_json(c: &Data) -> Value {
    match c {
        Data::Empty => Value::Null,
        Data::String(s) => Value::String(s.clone()),
        Data::Float(f) => serde_json::Number::from_f64(*f).map(Value::Number).unwrap_or(Value::Null),
        Data::Int(i) => Value::from(*i),
        Data::Bool(b) => Value::Bool(*b),
        Data::DateTime(dt) => Value::from(dt.as_f64()),
        Data::DateTimeIso(s) | Data::DurationIso(s) => Value::String(s.clone()),
        Data::Error(e) => Value::String(format!("{e:?}")),
    }
}

pub struct WriteRowAction;
#[derive(Deserialize)]
struct WriteRowIn {
    file: PathBuf,
    #[serde(default)]
    sheet: Option<String>,
    row: Value,
    #[serde(default)]
    headers: Option<Vec<String>>,
}

#[async_trait]
impl Action for WriteRowAction {
    fn id(&self) -> &'static str { "excel.write_row" }
    fn summary(&self) -> &'static str { "Append a row to an .xlsx workbook (create if missing)" }
    async fn execute(&self, _ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let WriteRowIn { file, sheet, row, headers } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("excel.write_row input invalid: {e}")))?;
        let sheet = sheet.unwrap_or_else(|| "Sheet1".into());

        tokio::task::spawn_blocking(move || -> Result<usize, String> {
            let mut existing_rows: Vec<Vec<Value>> = Vec::new();
            let mut col_headers: Vec<String> = headers.clone().unwrap_or_default();

            if file.exists() {
                let mut wb = open_workbook_auto(&file).map_err(|e| e.to_string())?;
                if let Ok(range) = wb.worksheet_range(&sheet) {
                    let mut iter = range.rows();
                    if col_headers.is_empty() {
                        if let Some(h) = iter.next() { col_headers = header_row(h); }
                    } else {
                        iter.next();
                    }
                    for r in iter {
                        existing_rows.push(r.iter().map(cell_to_json).collect());
                    }
                }
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
                    col_headers.iter().map(|h| m.get(h).cloned().unwrap_or(Value::Null)).collect()
                }
                _ => return Err(format!("row must be array or object, got {row}")),
            };
            existing_rows.push(new_row);

            let mut wb = Workbook::new();
            let ws = wb.add_worksheet();
            ws.set_name(&sheet).map_err(|e| e.to_string())?;
            for (c, h) in col_headers.iter().enumerate() {
                ws.write_string(0, c as u16, h).map_err(|e| e.to_string())?;
            }
            for (r, row) in existing_rows.iter().enumerate() {
                for (c, v) in row.iter().enumerate() {
                    write_cell(ws, (r + 1) as u32, c as u16, v).map_err(|e| e.to_string())?;
                }
            }
            wb.save(&file).map_err(|e| e.to_string())?;
            Ok(existing_rows.len())
        }).await
            .map_err(|e| StepError::msg(format!("excel join: {e}")))?
            .map_err(StepError::msg)
            .map(|n| ActionResult::from(serde_json::json!({ "rows": n })))
    }
}

fn write_cell(
    ws: &mut rust_xlsxwriter::Worksheet,
    row: u32,
    col: u16,
    v: &Value,
) -> Result<(), rust_xlsxwriter::XlsxError> {
    match v {
        Value::Null => { ws.write_blank(row, col, &rust_xlsxwriter::Format::default())?; }
        Value::Bool(b) => { ws.write_boolean(row, col, *b)?; }
        Value::Number(n) => {
            if let Some(i) = n.as_i64() { ws.write_number(row, col, i as f64)?; }
            else if let Some(f) = n.as_f64() { ws.write_number(row, col, f)?; }
            else { ws.write_string(row, col, &n.to_string())?; }
        }
        Value::String(s) => { ws.write_string(row, col, s)?; }
        other => { ws.write_string(row, col, &other.to_string())?; }
    }
    Ok(())
}
