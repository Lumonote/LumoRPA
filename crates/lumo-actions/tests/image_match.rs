//! Integration coverage for the `image.*` family (F-2 template matching).
//!
//! Pure-logic + filesystem (no network, no browser): synthesize a deterministic
//! pseudo-random haystack (the `x*y` cross term breaks translation-invariance, so
//! a crop has a single unambiguous normalized-cross-correlation peak), crop a
//! known sub-region as the template, write both to a tempdir, and assert
//! `image.locate` recovers the exact coordinates. Gating + validation too.

mod common;
use common::{fs_caps, ok_with, run, run_with};
use image::{GrayImage, Luma};
use serde_json::json;

/// Deterministic non-self-similar image: the `x*y` term makes every crop's
/// autocorrelation peak unique, so NCC has one clear maximum.
fn noise(w: u32, h: u32) -> GrayImage {
    GrayImage::from_fn(w, h, |x, y| {
        let v = (x * 73 + y * 151 + x * y * 229) % 251;
        Luma([v as u8])
    })
}

#[tokio::test]
async fn locate_finds_embedded_template() {
    let dir = tempfile::tempdir().unwrap();
    let caps = fs_caps(dir.path());
    let hay_path = dir.path().join("haystack.png");
    let tpl_path = dir.path().join("template.png");

    // Haystack 120x90; template = the 20x16 block at (40, 30).
    let hay = noise(120, 90);
    let (tx, ty, tw, th) = (40u32, 30u32, 20u32, 16u32);
    let tpl = image::imageops::crop_imm(&hay, tx, ty, tw, th).to_image();
    hay.save(&hay_path).unwrap();
    tpl.save(&tpl_path).unwrap();

    let out = ok_with(
        "image.locate",
        json!({ "image": hay_path, "template": tpl_path }),
        caps,
    )
    .await;

    assert_eq!(
        out.get("found").and_then(|v| v.as_bool()),
        Some(true),
        "out={out}"
    );
    assert_eq!(out.get("x").and_then(|v| v.as_u64()), Some(tx as u64), "out={out}");
    assert_eq!(out.get("y").and_then(|v| v.as_u64()), Some(ty as u64), "out={out}");
    assert_eq!(
        out.get("center_x").and_then(|v| v.as_u64()),
        Some((tx + tw / 2) as u64)
    );
    assert_eq!(
        out.get("center_y").and_then(|v| v.as_u64()),
        Some((ty + th / 2) as u64)
    );
    let score = out.get("score").and_then(|v| v.as_f64()).unwrap();
    assert!(score > 0.99, "near-perfect match expected, got {score}");
}

#[tokio::test]
async fn compare_identical_images_scores_one() {
    let dir = tempfile::tempdir().unwrap();
    let caps = fs_caps(dir.path());
    let a = dir.path().join("a.png");
    let b = dir.path().join("b.png");
    noise(64, 64).save(&a).unwrap();
    noise(64, 64).save(&b).unwrap();

    let out = ok_with("image.compare", json!({ "a": a, "b": b }), caps).await;
    assert_eq!(
        out.get("identical").and_then(|v| v.as_bool()),
        Some(true),
        "out={out}"
    );
}

#[tokio::test]
async fn compare_rejects_size_mismatch() {
    let dir = tempfile::tempdir().unwrap();
    let caps = fs_caps(dir.path());
    let a = dir.path().join("a.png");
    let b = dir.path().join("b.png");
    noise(64, 64).save(&a).unwrap();
    noise(32, 48).save(&b).unwrap();

    let err = run_with("image.compare", json!({ "a": a, "b": b }), caps)
        .await
        .unwrap_err();
    assert!(err.contains("dimension mismatch"), "got: {err}");
}

#[tokio::test]
async fn locate_rejects_oversized_template() {
    let dir = tempfile::tempdir().unwrap();
    let caps = fs_caps(dir.path());
    let hay = dir.path().join("small.png");
    let tpl = dir.path().join("big.png");
    noise(20, 20).save(&hay).unwrap();
    noise(40, 40).save(&tpl).unwrap();

    let err = run_with("image.locate", json!({ "image": hay, "template": tpl }), caps)
        .await
        .unwrap_err();
    assert!(err.contains("larger than image"), "got: {err}");
}

#[tokio::test]
async fn locate_denies_ungranted_path() {
    // No capabilities granted → fs.read gate rejects before reading disk.
    let err = run(
        "image.locate",
        json!({ "image": "/nope/a.png", "template": "/nope/b.png" }),
    )
    .await
    .unwrap_err();
    assert!(err.contains("capability denied"), "got: {err}");
    assert!(err.contains("fs.read"), "got: {err}");
}

#[tokio::test]
async fn locate_rejects_unknown_field() {
    // deny_unknown_fields fires during parse, before the capability gate.
    let err = run_with(
        "image.locate",
        json!({ "image": "/x.png", "template": "/y.png", "bogus": 1 }),
        fs_caps(std::path::Path::new("/tmp")),
    )
    .await
    .unwrap_err();
    assert!(err.contains("input invalid"), "got: {err}");
}
