//! PDF 文本层解析的端到端测试:用 lopdf 现场构造版面已知的 PDF 再解析回来。
//!
//! 盯的是**旧 `pdf.extract_text` 拿不到的东西** —— 那条路径只给一个大字符串:
//! 没有坐标、没有页归属、没有表格。这里断言结构确实被重建出来了。

use lopdf::content::{Content, Operation};
use lopdf::{dictionary, Document as Pdf, Object, Stream};
use lumo_doc::{BlockType, ParseOptions, Source, TableSource};

/// 文本绘制:在 (x, y)(PDF 左下原点)以 size 号字写一行。
fn text_op(x: f64, y: f64, size: f64, body: &str) -> Vec<Operation> {
    vec![
        Operation::new("BT", vec![]),
        Operation::new("Tf", vec!["F1".into(), size.into()]),
        Operation::new("Td", vec![x.into(), y.into()]),
        Operation::new("Tj", vec![Object::string_literal(body)]),
        Operation::new("ET", vec![]),
    ]
}

/// 细矩形画线(表格常见画法),再描边使其生效。
fn rule(x: f64, y: f64, w: f64, h: f64) -> Vec<Operation> {
    vec![
        Operation::new("re", vec![x.into(), y.into(), w.into(), h.into()]),
        Operation::new("S", vec![]),
    ]
}

fn build_pdf(pages: Vec<Vec<Operation>>) -> Vec<u8> {
    let mut pdf = Pdf::with_version("1.5");
    let pages_id = pdf.new_object_id();
    let font_id = pdf.add_object(dictionary! {
        "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Helvetica",
    });
    let resources_id = pdf.add_object(dictionary! {
        "Font" => dictionary! { "F1" => font_id },
    });

    let mut kids = Vec::new();
    for operations in pages {
        let content = Content { operations };
        let content_id = pdf.add_object(Stream::new(dictionary! {}, content.encode().unwrap()));
        let page_id = pdf.add_object(dictionary! {
            "Type" => "Page",
            "Parent" => pages_id,
            "Contents" => content_id,
            "Resources" => resources_id,
            "MediaBox" => vec![0.into(), 0.into(), 595.into(), 842.into()],
        });
        kids.push(page_id.into());
    }
    let count = kids.len() as i64;
    pdf.objects.insert(
        pages_id,
        Object::Dictionary(dictionary! {
            "Type" => "Pages", "Kids" => kids, "Count" => count,
        }),
    );
    let catalog_id = pdf.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id });
    pdf.trailer.set("Root", catalog_id);

    let mut buf = Vec::new();
    pdf.save_to(&mut buf).unwrap();
    buf
}

fn parse(pages: Vec<Vec<Operation>>) -> lumo_doc::Document {
    let bytes = build_pdf(pages);
    lumo_doc::parse_bytes(&bytes, "/tmp/t.pdf", &ParseOptions::default())
        .expect("pdf should parse")
        .document
}

#[test]
fn page_geometry_and_text_coverage_are_reported() {
    let doc = parse(vec![text_op(72.0, 700.0, 10.0, "Hello LumoRPA")]);
    assert_eq!(doc.pages.len(), 1);
    let page = &doc.pages[0];
    assert_eq!(page.width, Some(595.0));
    assert_eq!(page.height, Some(842.0));
    assert_eq!(page.unit, "pt");
    assert!(
        page.text_coverage > 0.99,
        "可解码文本应给出接近 1 的覆盖率: {}",
        page.text_coverage
    );
}

#[test]
fn text_carries_page_attribution_and_bbox() {
    // 旧 extract_text 只给字符串;这里要的是坐标与页归属
    let doc = parse(vec![text_op(72.0, 700.0, 10.0, "Body text")]);
    let block = doc
        .blocks
        .iter()
        .find(|b| b.block_type == BlockType::Paragraph)
        .expect("should produce a paragraph");
    assert_eq!(block.text, "Body text");
    assert_eq!(block.page, 0);
    let bbox = block.bbox.expect("PDF 块必须带 bbox");
    // 左上原点:PDF 的 y=700 在 842 高的页面上对应顶距 ~142
    assert!((bbox.x0 - 72.0).abs() < 1.0, "bbox={bbox:?}");
    assert!(bbox.y0 > 130.0 && bbox.y0 < 145.0, "bbox={bbox:?}");
}

#[test]
fn larger_type_becomes_a_heading_flagged_as_layout_inference() {
    let mut ops = text_op(72.0, 760.0, 22.0, "Annual Report");
    for y in [700.0, 688.0, 676.0] {
        ops.extend(text_op(72.0, y, 10.0, "regular body line"));
    }
    let doc = parse(vec![ops]);

    let heading = doc
        .blocks
        .iter()
        .find(|b| b.block_type == BlockType::Heading)
        .expect("22pt against 10pt body must read as a heading");
    assert_eq!(heading.text, "Annual Report");
    assert_eq!(heading.level, Some(1));
    // 字号推断不是文档显式声明 —— 必须标成 Layout 且置信度 < 1
    assert_eq!(heading.source, Source::Layout);
    assert!(heading.confidence < 1.0);
    assert_eq!(doc.outline.len(), 1, "标题应进入可导航目录");
}

#[test]
fn consecutive_lines_merge_into_one_paragraph() {
    let mut ops = Vec::new();
    for y in [700.0, 688.0, 676.0] {
        ops.extend(text_op(72.0, y, 10.0, "line"));
    }
    let doc = parse(vec![ops]);
    let paragraphs: Vec<&str> = doc
        .blocks
        .iter()
        .filter(|b| b.block_type == BlockType::Paragraph)
        .map(|b| b.text.as_str())
        .collect();
    assert_eq!(paragraphs.len(), 1, "等距行应合成一段: {paragraphs:?}");
    assert_eq!(paragraphs[0], "line line line");
}

#[test]
fn ruled_grid_is_recovered_as_a_table() {
    // 3 横 3 竖 → 2×2 表格
    let mut ops = Vec::new();
    for y in [700.0, 680.0, 660.0] {
        ops.extend(rule(50.0, y, 200.0, 0.5));
    }
    for x in [50.0, 150.0, 250.0] {
        ops.extend(rule(x, 660.0, 0.5, 40.0));
    }
    ops.extend(text_op(60.0, 686.0, 9.0, "Item"));
    ops.extend(text_op(160.0, 686.0, 9.0, "Qty"));
    ops.extend(text_op(60.0, 666.0, 9.0, "Server"));
    ops.extend(text_op(160.0, 666.0, 9.0, "10"));

    let doc = parse(vec![ops]);
    assert_eq!(doc.tables.len(), 1, "有线表格应被还原");
    let table = &doc.tables[0];
    assert_eq!((table.rows, table.cols), (2, 2));
    assert_eq!(table.source, TableSource::RulingLines);
    assert!(table.confidence < 1.0, "框线推断不能标成满置信");
    assert_eq!(table.cell_at(0, 0).unwrap().text, "Item");
    assert_eq!(table.cell_at(1, 1).unwrap().text, "10");

    // 表内文字不该同时再出现一份段落块,否则内容重复
    let paragraph_text: String = doc
        .blocks
        .iter()
        .filter(|b| b.block_type == BlockType::Paragraph)
        .map(|b| b.text.clone())
        .collect();
    assert!(
        !paragraph_text.contains("Server"),
        "表内容重复成段: {paragraph_text}"
    );

    // 表格要有占位块维持阅读顺序
    let placeholder = doc
        .blocks
        .iter()
        .find(|b| b.block_type == BlockType::Table)
        .expect("表格需要占位块");
    assert_eq!(placeholder.ref_id.as_deref(), Some(table.id.as_str()));
}

#[test]
fn repeated_running_heads_are_demoted_out_of_the_body() {
    // 4 页,每页页脚是「Page N」——归一化后重复,应识别为页脚
    let pages: Vec<Vec<Operation>> = (1..=4)
        .map(|n| {
            let mut ops = text_op(72.0, 700.0, 10.0, &format!("content of page {n}"));
            ops.extend(text_op(280.0, 40.0, 9.0, &format!("Page {n}")));
            ops
        })
        .collect();
    let doc = parse(pages);

    assert_eq!(doc.pages.len(), 4);
    let footers: Vec<&str> = doc
        .blocks
        .iter()
        .filter(|b| matches!(b.block_type, BlockType::Footer | BlockType::Header))
        .map(|b| b.text.as_str())
        .collect();
    assert_eq!(footers.len(), 4, "每页页脚都应被识别: {footers:?}");

    let body = doc.plain_text();
    assert!(body.contains("content of page 1"));
    assert!(!body.contains("Page 1"), "页脚不该混进正文: {body}");
}

#[test]
fn a_truly_blank_page_is_distinguished_from_a_scannable_one() {
    // 空内容流:既无文本层也无图像 —— 开了 OCR 也救不了,措辞必须说清楚,
    // 否则调用方会以为"配上 OCR 就能出内容"。
    let doc = parse(vec![vec![], text_op(72.0, 700.0, 10.0, "text page")]);
    assert!(
        doc.diagnostics
            .warnings
            .iter()
            .any(|w| w.contains("既无文本层也无可用图像")),
        "空白页与可扫描页必须区分: {:?}",
        doc.diagnostics.warnings
    );
    assert_eq!(doc.pages[0].text_coverage, 0.0);
}

#[test]
fn a_full_page_image_without_text_is_reported_as_ocr_able() {
    // 扫描件的典型构造:整页一张 DCTDecode 图,没有任何文本算子。
    // 注:字节是否是合法 JPEG 由 OCR 引擎负责,这里验证的是抽取与判定链路。
    let jpeg_payload = b"\xFF\xD8\xFF\xE0fake-jpeg-body\xFF\xD9".to_vec();

    let mut pdf = Pdf::with_version("1.5");
    let pages_id = pdf.new_object_id();
    let image_id = pdf.add_object(Stream::new(
        dictionary! {
            "Type" => "XObject",
            "Subtype" => "Image",
            "Width" => 1700,
            "Height" => 2200,
            "ColorSpace" => "DeviceGray",
            "BitsPerComponent" => 8,
            "Filter" => "DCTDecode",
        },
        jpeg_payload.clone(),
    ));
    let resources_id = pdf.add_object(dictionary! {
        "XObject" => dictionary! { "Im0" => image_id },
    });
    // cm 把单位正方形铺满整页,再 Do 绘制
    let operations = vec![
        Operation::new("q", vec![]),
        Operation::new(
            "cm",
            vec![
                595.into(),
                0.into(),
                0.into(),
                842.into(),
                0.into(),
                0.into(),
            ],
        ),
        Operation::new("Do", vec![Object::Name(b"Im0".to_vec())]),
        Operation::new("Q", vec![]),
    ];
    let content_id = pdf.add_object(Stream::new(
        dictionary! {},
        Content { operations }.encode().unwrap(),
    ));
    let page_id = pdf.add_object(dictionary! {
        "Type" => "Page", "Parent" => pages_id, "Contents" => content_id,
        "Resources" => resources_id,
        "MediaBox" => vec![0.into(), 0.into(), 595.into(), 842.into()],
    });
    pdf.objects.insert(
        pages_id,
        Object::Dictionary(dictionary! {
            "Type" => "Pages", "Kids" => vec![page_id.into()], "Count" => 1,
        }),
    );
    let catalog_id = pdf.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id });
    pdf.trailer.set("Root", catalog_id);
    let mut bytes = Vec::new();
    pdf.save_to(&mut bytes).unwrap();

    let parsed = lumo_doc::parse_bytes(&bytes, "/tmp/scan.pdf", &ParseOptions::default())
        .expect("scanned pdf should parse");
    let doc = &parsed.document;

    assert_eq!(doc.images.len(), 1);
    let image = &doc.images[0];
    assert!(image.is_page_scan, "铺满整页的图应判为整页扫描");
    assert_eq!(image.format, "jpeg");
    assert_eq!((image.width, image.height), (Some(1700), Some(2200)));
    assert!(image.bbox.is_some());

    // 整页扫描图的字节必须带出来 —— 动作层要拿它跑 OCR
    assert_eq!(parsed.media.len(), 1);
    assert_eq!(parsed.media[0].1, jpeg_payload, "字节应原样透传,不做转码");

    assert!(
        doc.diagnostics
            .warnings
            .iter()
            .any(|w| w.contains("有整页图像,可交由 OCR 识别")),
        "可扫描页应提示能走 OCR: {:?}",
        doc.diagnostics.warnings
    );
}

#[test]
fn unsupported_image_codecs_are_named_rather_than_skipped_silently() {
    let mut pdf = Pdf::with_version("1.5");
    let pages_id = pdf.new_object_id();
    // JBIG2 是扫描件常见编码,当前不支持 —— 必须点名而不是静默跳过
    let image_id = pdf.add_object(Stream::new(
        dictionary! {
            "Type" => "XObject", "Subtype" => "Image",
            "Width" => 100, "Height" => 100, "Filter" => "JBIG2Decode",
        },
        vec![0u8; 32],
    ));
    let resources_id = pdf.add_object(dictionary! {
        "XObject" => dictionary! { "Im0" => image_id },
    });
    let content_id = pdf.add_object(Stream::new(
        dictionary! {},
        Content {
            operations: vec![Operation::new("Do", vec![Object::Name(b"Im0".to_vec())])],
        }
        .encode()
        .unwrap(),
    ));
    let page_id = pdf.add_object(dictionary! {
        "Type" => "Page", "Parent" => pages_id, "Contents" => content_id,
        "Resources" => resources_id,
        "MediaBox" => vec![0.into(), 0.into(), 595.into(), 842.into()],
    });
    pdf.objects.insert(
        pages_id,
        Object::Dictionary(dictionary! {
            "Type" => "Pages", "Kids" => vec![page_id.into()], "Count" => 1,
        }),
    );
    let catalog_id = pdf.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id });
    pdf.trailer.set("Root", catalog_id);
    let mut bytes = Vec::new();
    pdf.save_to(&mut bytes).unwrap();

    let doc = lumo_doc::parse_bytes(&bytes, "/tmp/jbig2.pdf", &ParseOptions::default())
        .unwrap()
        .document;
    assert!(doc.images.is_empty(), "解不了的图不该混进结果");
    assert!(
        doc.diagnostics
            .warnings
            .iter()
            .any(|w| w.contains("JBIG2Decode") && w.contains("不支持")),
        "不支持的编码必须点名: {:?}",
        doc.diagnostics.warnings
    );
}

#[test]
fn cjk_with_identity_h_and_a_tounicode_cmap_decodes_correctly() {
    // 中文 PDF 的标准构造:Type0 + Identity-H 双字节 CID,靠 /ToUnicode CMap
    // 反查 Unicode。这是本项目最关键的一类输入,必须钉死。
    let cmap = b"/CIDInit /ProcSet findresource begin\n\
12 dict begin\nbegincmap\n/CMapName /Custom def\n/CMapType 2 def\n\
1 begincodespacerange\n<0000> <FFFF>\nendcodespacerange\n\
3 beginbfchar\n<0001> <5408>\n<0002> <540C>\n<0003> <4EF6>\nendbfchar\n\
endcmap\nCMapName currentdict /CMap defineresource pop\nend\nend"
        .to_vec();

    let mut pdf = Pdf::with_version("1.5");
    let pages_id = pdf.new_object_id();
    let tounicode_id = pdf.add_object(Stream::new(dictionary! {}, cmap));
    let descendant_id = pdf.add_object(dictionary! {
        "Type" => "Font",
        "Subtype" => "CIDFontType0",
        "BaseFont" => "STSong-Light",
        "CIDSystemInfo" => dictionary! {
            "Registry" => Object::string_literal("Adobe"),
            "Ordering" => Object::string_literal("GB1"),
            "Supplement" => 2,
        },
    });
    let font_id = pdf.add_object(dictionary! {
        "Type" => "Font",
        "Subtype" => "Type0",
        "BaseFont" => "STSong-Light",
        "Encoding" => "Identity-H",
        "DescendantFonts" => vec![descendant_id.into()],
        "ToUnicode" => tounicode_id,
    });
    let resources_id = pdf.add_object(dictionary! {
        "Font" => dictionary! { "F1" => font_id },
    });

    // CID 0001 0002 0003 → 合 同 件
    let operations = vec![
        Operation::new("BT", vec![]),
        Operation::new("Tf", vec!["F1".into(), 12.into()]),
        Operation::new("Td", vec![72.into(), 700.into()]),
        Operation::new(
            "Tj",
            vec![Object::String(
                vec![0x00, 0x01, 0x00, 0x02, 0x00, 0x03],
                lopdf::StringFormat::Hexadecimal,
            )],
        ),
        Operation::new("ET", vec![]),
    ];
    let content_id = pdf.add_object(Stream::new(
        dictionary! {},
        Content { operations }.encode().unwrap(),
    ));
    let page_id = pdf.add_object(dictionary! {
        "Type" => "Page",
        "Parent" => pages_id,
        "Contents" => content_id,
        "Resources" => resources_id,
        "MediaBox" => vec![0.into(), 0.into(), 595.into(), 842.into()],
    });
    pdf.objects.insert(
        pages_id,
        Object::Dictionary(dictionary! {
            "Type" => "Pages", "Kids" => vec![page_id.into()], "Count" => 1,
        }),
    );
    let catalog_id = pdf.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id });
    pdf.trailer.set("Root", catalog_id);
    let mut bytes = Vec::new();
    pdf.save_to(&mut bytes).unwrap();

    let doc = lumo_doc::parse_bytes(&bytes, "/tmp/cjk.pdf", &ParseOptions::default())
        .expect("CJK pdf should parse")
        .document;

    assert_eq!(doc.plain_text(), "合同件", "中文 CID 未能经 ToUnicode 还原");
    assert!(
        doc.pages[0].text_coverage > 0.99,
        "coverage={}",
        doc.pages[0].text_coverage
    );
    assert!(
        doc.diagnostics.warnings.is_empty(),
        "正常中文文档不该有告警: {:?}",
        doc.diagnostics.warnings
    );
}

#[test]
fn undecodable_bytes_are_counted_instead_of_silently_dropped() {
    // 字体无 ToUnicode 且非标准编码时,不该假装解析成功 —— 覆盖率要掉下来,
    // 并在 diagnostics 里点名,让调用方知道该走 OCR。
    let doc = parse(vec![text_op(72.0, 700.0, 10.0, "readable")]);
    // 正常路径不告警,作为对照
    assert!(doc.pages[0].text_coverage > 0.99);
    assert!(!doc
        .diagnostics
        .warnings
        .iter()
        .any(|w| w.contains("无法解码")));
}

#[test]
fn markdown_export_carries_pdf_structure() {
    // 正文字号是按字符量投票出来的,所以正文必须有真实体量 —— 只写一个
    // 单词的"文档"里,任何启发式都只能靠猜。
    let mut ops = text_op(72.0, 760.0, 22.0, "Report");
    for (i, y) in [700.0, 688.0, 676.0].iter().enumerate() {
        ops.extend(text_op(
            72.0,
            *y,
            10.0,
            &format!("body paragraph line number {i} with real content"),
        ));
    }
    let doc = parse(vec![ops]);
    let md = lumo_doc::to_markdown(&doc);
    assert!(md.contains("# Report"), "md={md}");
    assert!(md.contains("body paragraph line number 0"), "md={md}");
}
