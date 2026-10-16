//! PDF 图像 XObject 抽取 —— 扫描件 OCR 的输入源。
//!
//! **为什么不做页面栅格化**:纯 Rust 生态没有可用的 PDF 渲染器(pdfium 是 C++,
//! MuPDF 是 AGPL 会踩 `deny.toml`)。但扫描件的典型构造就是**每页一张整页图**,
//! 直接把这张图 XObject 取出来送 OCR 即可,不需要渲染引擎。矢量/混合版面才需
//! 要真正的栅格化,那属于可选的 `pdf-render-native`(见设计文档 M5)。
//!
//! 编码支持:
//! * `DCTDecode` —— 即 JPEG,原样取出即可用,扫描件最常见。
//! * `JPXDecode`(JPEG2000)/ `JBIG2Decode` / `CCITTFaxDecode` —— **明确不支持**,
//!   返回带原因的告警而不是静默跳过。静默的空结果在 RPA 里比失败更危险。

use crate::error::DocError;
use crate::pdf::text::Placement;
use lopdf::{Document as PdfDocument, Object};

/// 抽出的一张图。`bytes` 是可直接喂给 OCR 的编码图像。
#[derive(Debug, Clone)]
pub struct ExtractedImage {
    pub name: String,
    pub format: String,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub bytes: Vec<u8>,
    /// 页面坐标(左下原点)下的占位框。
    pub placement: Option<Placement>,
}

/// 单张图的字节上限,防御构造的超大图像。
const MAX_IMAGE_BYTES: usize = 64 * 1024 * 1024;

/// 取出某页所有可用的图像 XObject。无法解码的编码进 `warnings`,不进结果。
pub fn extract_page_images(
    pdf: &PdfDocument,
    page_id: (u32, u16),
    placements: &[Placement],
    warnings: &mut Vec<String>,
    page_number: usize,
) -> Result<Vec<ExtractedImage>, DocError> {
    // Resources 既可能内联在页字典里,也可能是间接引用(真实 PDF 里后者占绝大
    // 多数)。lopdf 把两者分开返回,只看内联那一份会漏掉几乎所有文档的图像。
    let Ok((inline, referenced)) = pdf.get_page_resources(page_id) else {
        return Ok(Vec::new());
    };
    let mut resource_dicts: Vec<&lopdf::Dictionary> = Vec::new();
    if let Some(dict) = inline {
        resource_dicts.push(dict);
    }
    for id in referenced {
        if let Ok(dict) = pdf.get_dictionary(id) {
            resource_dicts.push(dict);
        }
    }

    let mut xobject_entries: Vec<(&Vec<u8>, &Object)> = Vec::new();
    for resources in resource_dicts {
        // XObject 字典本身同样可能是间接引用
        let xobjects = match resources.get(b"XObject") {
            Ok(Object::Dictionary(dict)) => Some(dict),
            Ok(Object::Reference(id)) => pdf.get_dictionary(*id).ok(),
            _ => None,
        };
        if let Some(dict) = xobjects {
            xobject_entries.extend(dict.iter());
        }
    }

    let mut out = Vec::new();
    for (name, value) in xobject_entries {
        let Ok(id) = value.as_reference() else {
            continue;
        };
        let Ok(Object::Stream(stream)) = pdf.get_object(id) else {
            continue;
        };
        let dict = &stream.dict;
        if dict.get(b"Subtype").and_then(|o| o.as_name()).ok() != Some(b"Image".as_ref()) {
            continue;
        }

        let name_string = String::from_utf8_lossy(name).into_owned();
        let filters = filter_names(dict);
        let width = dict.get(b"Width").and_then(|o| o.as_i64()).ok().map(|v| v as u32);
        let height = dict.get(b"Height").and_then(|o| o.as_i64()).ok().map(|v| v as u32);

        // 只有 JPEG 能原样取出;其余编码需要解码器,当前不支持
        let format = match filters.last().map(|f| f.as_str()) {
            Some("DCTDecode") => "jpeg",
            Some(other) => {
                warnings.push(format!(
                    "第 {page_number} 页图像 {name_string} 使用 {other} 编码,当前不支持解码(需 M5 的原生渲染/解码器),已跳过"
                ));
                continue;
            }
            None => {
                warnings.push(format!(
                    "第 {page_number} 页图像 {name_string} 是未压缩位图,当前不支持转码,已跳过"
                ));
                continue;
            }
        };

        if stream.content.len() > MAX_IMAGE_BYTES {
            warnings.push(format!(
                "第 {page_number} 页图像 {name_string} 大小 {} 字节超过上限,已跳过",
                stream.content.len()
            ));
            continue;
        }

        out.push(ExtractedImage {
            placement: placements.iter().find(|p| p.name == *name).cloned(),
            name: name_string,
            format: format.to_string(),
            width,
            height,
            // DCTDecode 的 stream.content 就是原始 JPEG 字节流
            bytes: stream.content.clone(),
        });
    }
    Ok(out)
}

/// `/Filter` 可能是单个名字或名字数组(多级编码)。
fn filter_names(dict: &lopdf::Dictionary) -> Vec<String> {
    match dict.get(b"Filter") {
        Ok(Object::Name(name)) => vec![String::from_utf8_lossy(name).into_owned()],
        Ok(Object::Array(items)) => items
            .iter()
            .filter_map(|item| match item {
                Object::Name(name) => Some(String::from_utf8_lossy(name).into_owned()),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    }
}

/// 该图是否覆盖了页面的绝大部分 —— 「整页扫描图」的判据。
pub fn is_page_scan(placement: Option<&Placement>, page_width: f64, page_height: f64) -> bool {
    let Some(placement) = placement else {
        return false;
    };
    let page_area = page_width * page_height;
    if page_area <= 0.0 {
        return false;
    }
    placement.area() / page_area >= 0.8
}

#[cfg(test)]
mod tests {
    use super::*;
    use lopdf::dictionary;

    fn placement(x1: f64, y1: f64) -> Placement {
        Placement {
            name: b"Im0".to_vec(),
            x0: 0.0,
            y0: 0.0,
            x1,
            y1,
        }
    }

    #[test]
    fn a_full_bleed_image_is_a_page_scan() {
        assert!(is_page_scan(Some(&placement(595.0, 842.0)), 595.0, 842.0));
        // 占满九成也算(扫描件常留白边)
        assert!(is_page_scan(Some(&placement(580.0, 800.0)), 595.0, 842.0));
    }

    #[test]
    fn a_small_logo_is_not_a_page_scan() {
        assert!(!is_page_scan(Some(&placement(100.0, 60.0)), 595.0, 842.0));
        assert!(!is_page_scan(None, 595.0, 842.0), "无占位信息时不能臆断");
    }

    #[test]
    fn filter_arrays_and_single_names_both_resolve() {
        let single = dictionary! { "Filter" => "DCTDecode" };
        assert_eq!(filter_names(&single), vec!["DCTDecode"]);

        let chained = dictionary! {
            "Filter" => vec![Object::Name(b"FlateDecode".to_vec()), Object::Name(b"DCTDecode".to_vec())],
        };
        // 取最后一级 —— 那才是像素数据的实际编码
        assert_eq!(filter_names(&chained).last().unwrap(), "DCTDecode");

        let none = dictionary! { "Width" => 10 };
        assert!(filter_names(&none).is_empty());
    }
}
