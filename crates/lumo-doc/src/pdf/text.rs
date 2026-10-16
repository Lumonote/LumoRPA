//! PDF 内容流 → **带坐标的文本片段**。
//!
//! 既有 `pdf.extract_text` 调 `lopdf::extract_text`,拿到的是一整个字符串:没有
//! 坐标、没有页归属,因此无法重建段落、栏、表格。本模块自己走内容流算子,把每
//! 次 `Tj`/`TJ` 的**文本渲染矩阵**算出来,得到片段位置。
//!
//! 坐标系:PDF 原生左下原点、y 向上;DocIR 统一左上原点、y 向下,故出口处翻转。
//!
//! **已知近似**:字形前进宽度在有 `/Widths`(简单字体)时按表取值,CID 字体缺表
//! 时按「西文 0.5em / CJK 1.0em」估算。位置(来自 Td/Tm/CTM)是精确的,宽度只
//! 影响 bbox 与分栏判定,估算够用;这一点在 `Fragment::width_is_estimated` 上
//! 显式标出,不假装精确。

use lopdf::content::Content;
use lopdf::{Dictionary, Document, Object};
use std::collections::BTreeMap;

/// 2D 仿射矩阵 [a b c d e f],语义同 PDF 规范。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Matrix {
    pub a: f64,
    pub b: f64,
    pub c: f64,
    pub d: f64,
    pub e: f64,
    pub f: f64,
}

impl Matrix {
    pub const IDENTITY: Matrix = Matrix {
        a: 1.0,
        b: 0.0,
        c: 0.0,
        d: 1.0,
        e: 0.0,
        f: 0.0,
    };

    /// `self` 先作用,再作用 `other`(即 self × other)。
    pub fn then(&self, other: &Matrix) -> Matrix {
        Matrix {
            a: self.a * other.a + self.b * other.c,
            b: self.a * other.b + self.b * other.d,
            c: self.c * other.a + self.d * other.c,
            d: self.c * other.b + self.d * other.d,
            e: self.e * other.a + self.f * other.c + other.e,
            f: self.e * other.b + self.f * other.d + other.f,
        }
    }

    pub fn translate(tx: f64, ty: f64) -> Matrix {
        Matrix {
            e: tx,
            f: ty,
            ..Matrix::IDENTITY
        }
    }

    pub fn apply(&self, x: f64, y: f64) -> (f64, f64) {
        (
            self.a * x + self.c * y + self.e,
            self.b * x + self.d * y + self.f,
        )
    }

    /// 缩放因子(行列式开方)—— 用于把字号从文本空间换算到页面空间。
    pub fn scale(&self) -> f64 {
        (self.a * self.d - self.b * self.c).abs().sqrt()
    }
}

/// 一段同字体、同基线的文本及其在**页面坐标**(左下原点)里的位置。
#[derive(Debug, Clone)]
pub struct Fragment {
    pub text: String,
    pub x: f64,
    /// 基线 y(左下原点)。
    pub y: f64,
    pub width: f64,
    pub font_size: f64,
    pub width_is_estimated: bool,
}

/// 页面上的一条轴对齐直线(表格框线的候选)。
#[derive(Debug, Clone, Copy)]
pub struct Line {
    pub x0: f64,
    pub y0: f64,
    pub x1: f64,
    pub y1: f64,
}

impl Line {
    pub fn is_horizontal(&self) -> bool {
        (self.y1 - self.y0).abs() < 1.5 && (self.x1 - self.x0).abs() > 3.0
    }
    pub fn is_vertical(&self) -> bool {
        (self.x1 - self.x0).abs() < 1.5 && (self.y1 - self.y0).abs() > 3.0
    }
}

/// 页面上一次 XObject 绘制(`Do`)及其占位。扫描件的典型形态就是整页一张图,
/// 靠这个占位框判断「是否整页扫描」。
#[derive(Debug, Clone)]
pub struct Placement {
    /// XObject 资源名(如 `Im0`)。
    pub name: Vec<u8>,
    pub x0: f64,
    pub y0: f64,
    pub x1: f64,
    pub y1: f64,
}

impl Placement {
    pub fn area(&self) -> f64 {
        (self.x1 - self.x0).abs() * (self.y1 - self.y0).abs()
    }
}

#[derive(Debug, Default)]
pub struct PageContent {
    pub fragments: Vec<Fragment>,
    pub lines: Vec<Line>,
    /// 本页绘制过的 XObject 占位。
    pub placements: Vec<Placement>,
    /// 解不出 Unicode 的字节数 —— 用于 `text_coverage`,宁可显式暴露乱码率
    /// 也不静默产出垃圾文本。
    pub undecodable: usize,
    pub decoded_chars: usize,
}

/// 简单字体的 `/Widths` 表:code → 宽度(千分之一 em)。
struct FontMetrics {
    first_char: i64,
    widths: Vec<f64>,
}

impl FontMetrics {
    fn width_of(&self, code: u8) -> Option<f64> {
        let index = code as i64 - self.first_char;
        if index < 0 {
            return None;
        }
        self.widths.get(index as usize).copied()
    }
}

fn font_metrics(font: &Dictionary) -> Option<FontMetrics> {
    let first_char = font.get(b"FirstChar").ok()?.as_i64().ok()?;
    let widths = font
        .get(b"Widths")
        .ok()?
        .as_array()
        .ok()?
        .iter()
        .filter_map(|w| w.as_f32().ok().map(|v| v as f64))
        .collect::<Vec<_>>();
    (!widths.is_empty()).then_some(FontMetrics { first_char, widths })
}

/// 估算前进宽度:CJK 按 1 em,其余按 0.5 em。仅在无 `/Widths` 时使用。
fn estimate_width(text: &str, font_size: f64) -> f64 {
    text.chars()
        .map(|ch| {
            let em = if (ch as u32) >= 0x2E80 { 1.0 } else { 0.5 };
            em * font_size
        })
        .sum()
}

struct TextState {
    tm: Matrix,
    tlm: Matrix,
    font_size: f64,
    leading: f64,
    char_spacing: f64,
    word_spacing: f64,
    horizontal_scale: f64,
    font_key: Vec<u8>,
}

impl Default for TextState {
    fn default() -> Self {
        Self {
            tm: Matrix::IDENTITY,
            tlm: Matrix::IDENTITY,
            font_size: 0.0,
            leading: 0.0,
            char_spacing: 0.0,
            word_spacing: 0.0,
            horizontal_scale: 1.0,
            font_key: Vec::new(),
        }
    }
}

/// 单页内容流解压上限 —— 解压炸弹防护,与 images.rs 的 MAX_IMAGE_BYTES 同一思路。
/// 一页正常内容流远小于此(文本+矢量算子通常几十 KB 量级),64 MiB 对合法 PDF 宽裕。
const MAX_PAGE_CONTENT_BYTES: usize = 64 * 1024 * 1024;

/// 走一页的内容流,产出文本片段与框线候选。
pub fn walk_page(
    doc: &Document,
    page_id: (u32, u16),
    fonts: &BTreeMap<Vec<u8>, &Dictionary>,
) -> Result<PageContent, lopdf::Error> {
    // lopdf 0.42 起 get_page_content 不再返回 Result(失败流回退为原始字节),
    // 直接返回 Vec<u8>,原来的 `?` 因此失去意义。这里改用 0.45 提供的
    // get_page_content_with_limit:它保留同样的宽松回退,但给解压总量设上界 ——
    // 我们解析的是不可信 PDF,无界解压正是要防的那类输入。
    let data = doc.get_page_content_with_limit(page_id, MAX_PAGE_CONTENT_BYTES)?;
    let content = Content::decode(&data)?;

    // 每个字体的编码与宽度表只解一次
    let encodings: BTreeMap<Vec<u8>, lopdf::Encoding> = fonts
        .iter()
        .filter_map(|(name, font)| {
            font.get_font_encoding(doc)
                .ok()
                .map(|enc| (name.clone(), enc))
        })
        .collect();
    let metrics: BTreeMap<Vec<u8>, FontMetrics> = fonts
        .iter()
        .filter_map(|(name, font)| font_metrics(font).map(|m| (name.clone(), m)))
        .collect();

    let mut out = PageContent::default();
    let mut ctm = Matrix::IDENTITY;
    let mut ctm_stack: Vec<Matrix> = Vec::new();
    let mut ts = TextState::default();
    // 路径构造状态(框线检测)
    let mut current_point: Option<(f64, f64)> = None;
    let mut pending_segments: Vec<Line> = Vec::new();

    for op in &content.operations {
        let operands = &op.operands;
        match op.operator.as_str() {
            "q" => ctm_stack.push(ctm),
            "Q" => ctm = ctm_stack.pop().unwrap_or(Matrix::IDENTITY),
            "cm" => {
                if let Some(m) = matrix_from(operands) {
                    ctm = m.then(&ctm);
                }
            }
            "BT" => {
                ts.tm = Matrix::IDENTITY;
                ts.tlm = Matrix::IDENTITY;
            }
            "ET" => {}
            "Tf" => {
                if let Some(Object::Name(name)) = operands.first() {
                    ts.font_key = name.clone();
                }
                ts.font_size = num(operands.get(1)).unwrap_or(ts.font_size);
            }
            "TL" => ts.leading = num(operands.first()).unwrap_or(0.0),
            "Tc" => ts.char_spacing = num(operands.first()).unwrap_or(0.0),
            "Tw" => ts.word_spacing = num(operands.first()).unwrap_or(0.0),
            "Tz" => ts.horizontal_scale = num(operands.first()).unwrap_or(100.0) / 100.0,
            "Td" => {
                let tx = num(operands.first()).unwrap_or(0.0);
                let ty = num(operands.get(1)).unwrap_or(0.0);
                ts.tlm = Matrix::translate(tx, ty).then(&ts.tlm);
                ts.tm = ts.tlm;
            }
            "TD" => {
                let tx = num(operands.first()).unwrap_or(0.0);
                let ty = num(operands.get(1)).unwrap_or(0.0);
                ts.leading = -ty;
                ts.tlm = Matrix::translate(tx, ty).then(&ts.tlm);
                ts.tm = ts.tlm;
            }
            "Tm" => {
                if let Some(m) = matrix_from(operands) {
                    ts.tlm = m;
                    ts.tm = m;
                }
            }
            "T*" => {
                ts.tlm = Matrix::translate(0.0, -ts.leading).then(&ts.tlm);
                ts.tm = ts.tlm;
            }
            "Tj" | "'" | "\"" => {
                // `'` 与 `"` 先换行;`"` 的前两个操作数是字/词间距
                if op.operator != "Tj" {
                    if op.operator == "\"" {
                        ts.word_spacing = num(operands.first()).unwrap_or(ts.word_spacing);
                        ts.char_spacing = num(operands.get(1)).unwrap_or(ts.char_spacing);
                    }
                    ts.tlm = Matrix::translate(0.0, -ts.leading).then(&ts.tlm);
                    ts.tm = ts.tlm;
                }
                if let Some(Object::String(bytes, _)) = operands.last() {
                    show_text(&mut out, &mut ts, &ctm, &encodings, &metrics, bytes);
                }
            }
            "TJ" => {
                if let Some(Object::Array(items)) = operands.first() {
                    for item in items {
                        match item {
                            Object::String(bytes, _) => {
                                show_text(&mut out, &mut ts, &ctm, &encodings, &metrics, bytes)
                            }
                            other => {
                                // 数字是以千分之一 em 为单位的**反向**位移
                                if let Some(adjust) = num(Some(other)) {
                                    let dx = -adjust / 1000.0 * ts.font_size * ts.horizontal_scale;
                                    ts.tm = Matrix::translate(dx, 0.0).then(&ts.tm);
                                }
                            }
                        }
                    }
                }
            }
            // ── XObject 绘制:图像占位 ────────────────────────────────────
            // 图像 XObject 画在单位正方形上,由 CTM 变换到页面 —— 变换四角即
            // 得到占位框。扫描件判定全靠它。
            "Do" => {
                if let Some(Object::Name(name)) = operands.first() {
                    let corners = [
                        ctm.apply(0.0, 0.0),
                        ctm.apply(1.0, 0.0),
                        ctm.apply(0.0, 1.0),
                        ctm.apply(1.0, 1.0),
                    ];
                    let xs: Vec<f64> = corners.iter().map(|c| c.0).collect();
                    let ys: Vec<f64> = corners.iter().map(|c| c.1).collect();
                    out.placements.push(Placement {
                        name: name.clone(),
                        x0: xs.iter().cloned().fold(f64::INFINITY, f64::min),
                        y0: ys.iter().cloned().fold(f64::INFINITY, f64::min),
                        x1: xs.iter().cloned().fold(f64::NEG_INFINITY, f64::max),
                        y1: ys.iter().cloned().fold(f64::NEG_INFINITY, f64::max),
                    });
                }
            }
            // ── 路径:框线候选 ────────────────────────────────────────────
            "re" => {
                let (x, y, w, h) = (
                    num(operands.first()).unwrap_or(0.0),
                    num(operands.get(1)).unwrap_or(0.0),
                    num(operands.get(2)).unwrap_or(0.0),
                    num(operands.get(3)).unwrap_or(0.0),
                );
                // 细长矩形就是框线(表格常用填充矩形画线而非描边)
                let (p0, p1) = if h.abs() <= 3.0 {
                    (ctm.apply(x, y), ctm.apply(x + w, y))
                } else if w.abs() <= 3.0 {
                    (ctm.apply(x, y), ctm.apply(x, y + h))
                } else {
                    // 粗矩形按四条边收集,覆盖「单元格背景块」画法
                    for (a, b) in [
                        ((x, y), (x + w, y)),
                        ((x, y + h), (x + w, y + h)),
                        ((x, y), (x, y + h)),
                        ((x + w, y), (x + w, y + h)),
                    ] {
                        let p0 = ctm.apply(a.0, a.1);
                        let p1 = ctm.apply(b.0, b.1);
                        pending_segments.push(Line {
                            x0: p0.0,
                            y0: p0.1,
                            x1: p1.0,
                            y1: p1.1,
                        });
                    }
                    continue;
                };
                pending_segments.push(Line {
                    x0: p0.0,
                    y0: p0.1,
                    x1: p1.0,
                    y1: p1.1,
                });
            }
            "m" => {
                let p = ctm.apply(
                    num(operands.first()).unwrap_or(0.0),
                    num(operands.get(1)).unwrap_or(0.0),
                );
                current_point = Some(p);
            }
            "l" => {
                let p = ctm.apply(
                    num(operands.first()).unwrap_or(0.0),
                    num(operands.get(1)).unwrap_or(0.0),
                );
                if let Some(from) = current_point {
                    pending_segments.push(Line {
                        x0: from.0,
                        y0: from.1,
                        x1: p.0,
                        y1: p.1,
                    });
                }
                current_point = Some(p);
            }
            // 描边/填充才让路径“生效”;仅构造不绘制的路径(如裁剪)不算框线
            "S" | "s" | "f" | "F" | "f*" | "B" | "B*" | "b" | "b*" => {
                out.lines.extend(
                    pending_segments
                        .drain(..)
                        .filter(|l| l.is_horizontal() || l.is_vertical()),
                );
                current_point = None;
            }
            "n" => {
                pending_segments.clear();
                current_point = None;
            }
            _ => {}
        }
    }
    Ok(out)
}

fn show_text(
    out: &mut PageContent,
    ts: &mut TextState,
    ctm: &Matrix,
    encodings: &BTreeMap<Vec<u8>, lopdf::Encoding>,
    metrics: &BTreeMap<Vec<u8>, FontMetrics>,
    bytes: &[u8],
) {
    let text = match encodings.get(&ts.font_key) {
        Some(encoding) => match Document::decode_text(encoding, bytes) {
            Ok(text) => text,
            Err(_) => {
                out.undecodable += bytes.len();
                return;
            }
        },
        None => {
            // 没有编码信息时不猜 —— 计入乱码率,交给上层决定是否走 OCR
            out.undecodable += bytes.len();
            return;
        }
    };
    if text.is_empty() {
        return;
    }

    // 文本渲染矩阵 = [Tfs*Th 0 0 Tfs 0 0] × Tm × CTM
    let scale_matrix = Matrix {
        a: ts.font_size * ts.horizontal_scale,
        b: 0.0,
        c: 0.0,
        d: ts.font_size,
        e: 0.0,
        f: 0.0,
    };
    let trm = scale_matrix.then(&ts.tm).then(ctm);
    let page_scale = ts.tm.then(ctm).scale();
    let effective_size = ts.font_size * page_scale;

    // 前进宽度:有 /Widths 用真值,否则估算
    let metric = metrics.get(&ts.font_key);
    let (advance_text_space, estimated) = match metric {
        Some(m) if bytes.iter().all(|b| m.width_of(*b).is_some()) => (
            bytes
                .iter()
                .filter_map(|b| m.width_of(*b))
                .map(|w| w / 1000.0 * ts.font_size)
                .sum::<f64>(),
            false,
        ),
        _ => (estimate_width(&text, ts.font_size), true),
    };
    let spacing = ts.char_spacing * text.chars().count() as f64
        + ts.word_spacing * text.matches(' ').count() as f64;
    let advance = (advance_text_space + spacing) * ts.horizontal_scale;

    out.decoded_chars += text.chars().count();
    out.fragments.push(Fragment {
        text,
        x: trm.e,
        y: trm.f,
        width: advance * page_scale,
        font_size: effective_size,
        width_is_estimated: estimated,
    });

    ts.tm = Matrix::translate(advance, 0.0).then(&ts.tm);
}

fn matrix_from(operands: &[Object]) -> Option<Matrix> {
    if operands.len() < 6 {
        return None;
    }
    Some(Matrix {
        a: num(operands.first())?,
        b: num(operands.get(1))?,
        c: num(operands.get(2))?,
        d: num(operands.get(3))?,
        e: num(operands.get(4))?,
        f: num(operands.get(5))?,
    })
}

fn num(object: Option<&Object>) -> Option<f64> {
    match object? {
        Object::Integer(i) => Some(*i as f64),
        Object::Real(r) => Some(*r as f64),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matrix_composition_matches_pdf_semantics() {
        // 先平移 (10,20),再放大 2 倍 → 点 (0,0) 落在 (20,40)
        let translate = Matrix::translate(10.0, 20.0);
        let scale = Matrix {
            a: 2.0,
            d: 2.0,
            ..Matrix::IDENTITY
        };
        let combined = translate.then(&scale);
        assert_eq!(combined.apply(0.0, 0.0), (20.0, 40.0));
    }

    #[test]
    fn scale_factor_comes_from_the_determinant() {
        let m = Matrix {
            a: 3.0,
            d: 3.0,
            ..Matrix::IDENTITY
        };
        assert!((m.scale() - 3.0).abs() < 1e-9);
    }

    #[test]
    fn cjk_is_estimated_at_full_em_latin_at_half() {
        assert!((estimate_width("ab", 10.0) - 10.0).abs() < 1e-9);
        assert!((estimate_width("中文", 10.0) - 20.0).abs() < 1e-9);
    }

    #[test]
    fn thin_geometry_is_classified_as_ruling_lines() {
        let horizontal = Line {
            x0: 0.0,
            y0: 100.0,
            x1: 200.0,
            y1: 100.4,
        };
        assert!(horizontal.is_horizontal() && !horizontal.is_vertical());
        let vertical = Line {
            x0: 50.0,
            y0: 0.0,
            x1: 50.2,
            y1: 300.0,
        };
        assert!(vertical.is_vertical() && !vertical.is_horizontal());
        // 又短又斜的不是框线
        let diagonal = Line {
            x0: 0.0,
            y0: 0.0,
            x1: 20.0,
            y1: 20.0,
        };
        assert!(!diagonal.is_horizontal() && !diagonal.is_vertical());
    }

    #[test]
    fn font_widths_table_indexes_from_first_char() {
        let metrics = FontMetrics {
            first_char: 65, // 'A'
            widths: vec![500.0, 600.0],
        };
        assert_eq!(metrics.width_of(b'A'), Some(500.0));
        assert_eq!(metrics.width_of(b'B'), Some(600.0));
        assert_eq!(metrics.width_of(b'C'), None);
        assert_eq!(metrics.width_of(b'0'), None, "小于 FirstChar 不该回绕");
    }
}
