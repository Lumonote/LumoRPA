//! Integration coverage for the `xml.*` action family (P0 指令集缺口第一批)。
//!
//! 映射约定(见 src/xml_ops.rs 头注):`@attr` 属性、`#text` 文本、重复同名子
//! 元素聚数组、CDATA 并入文本、命名空间前缀原样保留、叶子折叠为字符串。

mod common;
use common::{ok, run};
use serde_json::json;

// ---------------------------------------------------------------------------
// xml.parse
// ---------------------------------------------------------------------------

#[tokio::test]
async fn parse_attributes_and_nesting() {
    let xml = r#"<order id="42" status="paid"><item sku="A1">Pen</item></order>"#;
    assert_eq!(
        ok("xml.parse", json!({"xml": xml})).await,
        json!({
            "order": {
                "@id": "42",
                "@status": "paid",
                "item": { "@sku": "A1", "#text": "Pen" }
            }
        })
    );
}

#[tokio::test]
async fn parse_repeated_children_become_array() {
    let xml = "<list><item>a</item><item>b</item><item>c</item></list>";
    assert_eq!(
        ok("xml.parse", json!({"xml": xml})).await,
        json!({"list": {"item": ["a", "b", "c"]}})
    );
}

#[tokio::test]
async fn parse_cdata_merges_into_text() {
    let xml = "<msg><![CDATA[a < b && c]]></msg>";
    assert_eq!(
        ok("xml.parse", json!({"xml": xml})).await,
        json!({"msg": "a < b && c"})
    );
}

#[tokio::test]
async fn parse_chinese_content_and_entities() {
    let xml =
        r#"<发票 类型="增值税专用发票"><购买方>晨光&amp;文具</购买方><金额>1234.56</金额></发票>"#;
    assert_eq!(
        ok("xml.parse", json!({"xml": xml})).await,
        json!({
            "发票": {
                "@类型": "增值税专用发票",
                "购买方": "晨光&文具",
                "金额": "1234.56"
            }
        })
    );
}

#[tokio::test]
async fn parse_namespace_prefixes_kept_verbatim() {
    let xml = r#"<soap:Envelope xmlns:soap="http://schemas.xmlsoap.org/soap/envelope/"><soap:Body><GetUser/></soap:Body></soap:Envelope>"#;
    assert_eq!(
        ok("xml.parse", json!({"xml": xml})).await,
        json!({
            "soap:Envelope": {
                "@xmlns:soap": "http://schemas.xmlsoap.org/soap/envelope/",
                "soap:Body": { "GetUser": null }
            }
        })
    );
}

#[tokio::test]
async fn parse_empty_element_is_null_and_whitespace_trimmed() {
    let xml = "<root>\n  <empty/>\n  <leaf> padded </leaf>\n</root>";
    assert_eq!(
        ok("xml.parse", json!({"xml": xml})).await,
        json!({"root": {"empty": null, "leaf": "padded"}})
    );
}

#[tokio::test]
async fn parse_malformed_xml_errors() {
    let err = run("xml.parse", json!({"xml": "<a><b></a>"}))
        .await
        .expect_err("mismatched tags must error");
    assert!(
        err.contains("xml.parse"),
        "error should name the action: {err}"
    );
}

#[tokio::test]
async fn parse_rejects_oversized_input() {
    let err = run(
        "xml.parse",
        json!({"xml": "<a>xxxxxxxxxx</a>", "max_bytes": 8}),
    )
    .await
    .expect_err("input above max_bytes must error");
    assert!(
        err.contains("max_bytes"),
        "error should mention the limit: {err}"
    );
}

// ---------------------------------------------------------------------------
// xml.build
// ---------------------------------------------------------------------------

#[tokio::test]
async fn build_compact_with_declaration() {
    let value = json!({"order": {"@id": "42", "item": ["a", "b"], "note": null}});
    assert_eq!(
        ok("xml.build", json!({"value": value})).await,
        json!("<?xml version=\"1.0\" encoding=\"UTF-8\"?><order id=\"42\"><item>a</item><item>b</item><note/></order>")
    );
}

#[tokio::test]
async fn build_escapes_text_and_attributes() {
    let value = json!({"a": {"@q": "x\"<y", "#text": "1 < 2 & 3"}});
    assert_eq!(
        ok("xml.build", json!({"value": value, "declaration": false})).await,
        json!("<a q=\"x&quot;&lt;y\">1 &lt; 2 &amp; 3</a>")
    );
}

#[tokio::test]
async fn build_indented_output() {
    let value = json!({"r": {"a": "1", "b": {"c": "2"}}});
    let out = ok(
        "xml.build",
        json!({"value": value, "declaration": false, "indent": true}),
    )
    .await;
    assert_eq!(
        out,
        json!("<r>\n  <a>1</a>\n  <b>\n    <c>2</c>\n  </b>\n</r>")
    );
}

#[tokio::test]
async fn build_rejects_multi_root_and_root_array() {
    let err = run("xml.build", json!({"value": {"a": 1, "b": 2}}))
        .await
        .expect_err("two root keys must error");
    assert!(err.contains("one root"), "{err}");
    let err = run("xml.build", json!({"value": {"a": [1, 2]}}))
        .await
        .expect_err("array at root must error");
    assert!(err.contains("root"), "{err}");
}

/// round-trip:parse → build → parse 必须语义等价(以 parse 输出为基准)。
#[tokio::test]
async fn round_trip_parse_build_parse_is_stable() {
    let xml = r#"<soap:Envelope xmlns:soap="http://schemas.xmlsoap.org/soap/envelope/">
  <soap:Body>
    <发票查询结果 代码="011001900111">
      <明细><名称>办公用品</名称><金额>88.00</金额></明细>
      <明细><名称>耗材&amp;配件</名称><金额>12.50</金额></明细>
      <备注/>
    </发票查询结果>
  </soap:Body>
</soap:Envelope>"#;
    let parsed = ok("xml.parse", json!({"xml": xml})).await;
    let rebuilt = ok("xml.build", json!({"value": parsed.clone()})).await;
    let reparsed = ok("xml.parse", json!({"xml": rebuilt})).await;
    assert_eq!(
        reparsed, parsed,
        "parse(build(parse(x))) must equal parse(x)"
    );
}

// ---------------------------------------------------------------------------
// xml.xpath
// ---------------------------------------------------------------------------

const BOOKS: &str = r#"<library><book id="1" lang="zh"><title>三体</title><price>45</price></book><book id="2" lang="en"><title>Dune</title><price>60</price></book></library>"#;

#[tokio::test]
async fn xpath_text_function() {
    assert_eq!(
        ok(
            "xml.xpath",
            json!({"xml": BOOKS, "expr": "/library/book/title/text()"})
        )
        .await,
        json!({"matches": ["三体", "Dune"], "count": 2})
    );
}

#[tokio::test]
async fn xpath_predicate_and_attribute() {
    // 谓词:按属性筛选第二本书,再取其 @lang。
    assert_eq!(
        ok(
            "xml.xpath",
            json!({"xml": BOOKS, "expr": "/library/book[@id='2']/@lang"})
        )
        .await,
        json!({"matches": ["en"], "count": 1})
    );
    // 位置谓词。
    assert_eq!(
        ok(
            "xml.xpath",
            json!({"xml": BOOKS, "expr": "//book[2]/title/text()"})
        )
        .await,
        json!({"matches": ["Dune"], "count": 1})
    );
}

#[tokio::test]
async fn xpath_element_match_serializes_node() {
    let out = ok(
        "xml.xpath",
        json!({"xml": BOOKS, "expr": "/library/book[1]/title"}),
    )
    .await;
    assert_eq!(out, json!({"matches": ["<title>三体</title>"], "count": 1}));
}

#[tokio::test]
async fn xpath_scalar_results() {
    assert_eq!(
        ok("xml.xpath", json!({"xml": BOOKS, "expr": "count(//book)"})).await,
        json!({"matches": [2.0], "count": 1})
    );
    assert_eq!(
        ok(
            "xml.xpath",
            json!({"xml": BOOKS, "expr": "sum(//price) > 100"})
        )
        .await,
        json!({"matches": [true], "count": 1})
    );
}

#[tokio::test]
async fn xpath_with_namespaces() {
    let xml = r#"<s:Envelope xmlns:s="http://schemas.xmlsoap.org/soap/envelope/"><s:Body><Code>OK</Code></s:Body></s:Envelope>"#;
    assert_eq!(
        ok(
            "xml.xpath",
            json!({
                "xml": xml,
                "expr": "/soap:Envelope/soap:Body/Code/text()",
                "namespaces": {"soap": "http://schemas.xmlsoap.org/soap/envelope/"}
            })
        )
        .await,
        json!({"matches": ["OK"], "count": 1}),
        "前缀映射按 URI 匹配,与文档内前缀拼写无关"
    );
}

#[tokio::test]
async fn xpath_invalid_expression_and_malformed_xml_error() {
    let err = run("xml.xpath", json!({"xml": BOOKS, "expr": "///"}))
        .await
        .expect_err("invalid xpath must error");
    assert!(err.contains("xml.xpath"), "{err}");
    let err = run("xml.xpath", json!({"xml": "<a><b></a>", "expr": "/a"}))
        .await
        .expect_err("malformed xml must error");
    assert!(err.contains("malformed"), "{err}");
}
