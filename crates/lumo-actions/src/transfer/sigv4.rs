//! 极简 AWS Signature Version 4 签名器(F-6 对象存储)。
//!
//! 只覆盖 S3 兼容存储 `PUT`/`GET` 单对象所需:UNSIGNED-PAYLOAD 不走;我们对
//! 请求体做完整 SHA-256(`x-amz-content-sha256`),signed headers 固定为
//! `host;x-amz-content-sha256;x-amz-date`。这样既能对接 AWS S3,也能对接
//! MinIO / 阿里云 OSS 等自定义 endpoint 的 S3 兼容实现。
//!
//! 不引第三方 SDK:rust-s3/aws-creds 会经 attohttpc 强行启用 `rustls/default`,
//! 拉入 `aws-lc-sys`(C 依赖),破坏 aarch64 信创交叉编译。这里仅用本 crate 既有
//! 的 `hmac` + `sha2`,配合 workspace 的 reqwest(rustls/ring),零新增重依赖。

use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};

type HmacSha256 = Hmac<Sha256>;

/// SHA-256 十六进制摘要(小写),用于 canonical request 与 payload hash。
pub(crate) fn sha256_hex(data: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(data);
    hex_lower(&h.finalize())
}

fn hmac(key: &[u8], msg: &[u8]) -> Vec<u8> {
    let mut mac = HmacSha256::new_from_slice(key).expect("HMAC accepts any key length");
    mac.update(msg);
    mac.finalize().into_bytes().to_vec()
}

fn hex_lower(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

/// URI 路径编码(RFC 3986,保留 `/`)。S3 key 段需逐字符编码,`/` 作为路径分隔符
/// 不编码,以保证 canonical URI 与服务端一致。
fn uri_encode_path(path: &str) -> String {
    let mut out = String::with_capacity(path.len());
    for &b in path.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// 一次签名所需的全部输入。`amz_date` 形如 `20240101T000000Z`;`date_stamp`
/// 形如 `20240101`(均为 UTC)。
pub(crate) struct SigV4Input<'a> {
    pub method: &'a str,
    pub host: &'a str,
    /// 规范化后的对象路径(以 `/` 开头,如 `/bucket/key`)。
    pub canonical_path: &'a str,
    pub region: &'a str,
    pub access_key: &'a str,
    pub secret_key: &'a str,
    pub payload_sha256: &'a str,
    pub amz_date: &'a str,
    pub date_stamp: &'a str,
}

/// 计算 `Authorization` 头的值。service 固定为 `s3`。
pub(crate) fn authorization_header(input: &SigV4Input) -> String {
    let signed_headers = "host;x-amz-content-sha256;x-amz-date";
    let canonical_headers = format!(
        "host:{}\nx-amz-content-sha256:{}\nx-amz-date:{}\n",
        input.host, input.payload_sha256, input.amz_date
    );
    // canonical request(query 为空:单对象 PUT/GET 不带 query)。
    let canonical_request = format!(
        "{}\n{}\n\n{}\n{}\n{}",
        input.method,
        uri_encode_path(input.canonical_path),
        canonical_headers,
        signed_headers,
        input.payload_sha256,
    );

    let scope = format!("{}/{}/s3/aws4_request", input.date_stamp, input.region);
    let string_to_sign = format!(
        "AWS4-HMAC-SHA256\n{}\n{}\n{}",
        input.amz_date,
        scope,
        sha256_hex(canonical_request.as_bytes()),
    );

    // 派生签名密钥:kDate -> kRegion -> kService -> kSigning。
    let k_date = hmac(
        format!("AWS4{}", input.secret_key).as_bytes(),
        input.date_stamp.as_bytes(),
    );
    let k_region = hmac(&k_date, input.region.as_bytes());
    let k_service = hmac(&k_region, b"s3");
    let k_signing = hmac(&k_service, b"aws4_request");
    let signature = hex_lower(&hmac(&k_signing, string_to_sign.as_bytes()));

    format!(
        "AWS4-HMAC-SHA256 Credential={}/{}, SignedHeaders={}, Signature={}",
        input.access_key, scope, signed_headers, signature
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha256_hex_matches_known_vector() {
        // 空串的 SHA-256(AWS UNSIGNED 对照值)。
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn uri_encode_keeps_slash_encodes_space() {
        assert_eq!(uri_encode_path("/bucket/a b"), "/bucket/a%20b");
        assert_eq!(uri_encode_path("/bucket/k"), "/bucket/k");
    }

    #[test]
    fn authorization_header_is_deterministic_and_well_formed() {
        let payload = sha256_hex(b"hello");
        let input = SigV4Input {
            method: "PUT",
            host: "127.0.0.1:9000",
            canonical_path: "/bucket/key.txt",
            region: "us-east-1",
            access_key: "AKID",
            secret_key: "SECRET",
            payload_sha256: &payload,
            amz_date: "20240101T000000Z",
            date_stamp: "20240101",
        };
        let a = authorization_header(&input);
        let b = authorization_header(&input);
        assert_eq!(a, b, "签名对相同输入必须确定");
        assert!(
            a.starts_with("AWS4-HMAC-SHA256 Credential=AKID/20240101/us-east-1/s3/aws4_request")
        );
        assert!(a.contains("SignedHeaders=host;x-amz-content-sha256;x-amz-date"));
        assert!(a.contains("Signature="));
        // 不得泄漏 secret。
        assert!(!a.contains("SECRET"));
    }
}
