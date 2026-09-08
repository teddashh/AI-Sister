use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

use image::{GenericImageView, ImageFormat};
use serde::Deserialize;
use sha2::{Digest, Sha256};

const PERSONAS: [&str; 17] = [
    "chatgpt",
    "claude",
    "gemini",
    "grok",
    "deepseek",
    "qwen",
    "mistral",
    "venice",
    "sakana",
    "perplexity",
    "glm",
    "kimi",
    "hunyuan",
    "minimax",
    "nemotron",
    "cohere",
    "mimo",
];

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Manifest {
    schema: String,
    source_reel_manifest_sha256: String,
    preview_contract: PreviewContract,
    assets: Vec<Asset>,
}

#[derive(Debug, Deserialize)]
struct PreviewContract {
    subject: String,
    canvas: Canvas,
    alpha: String,
    fit: String,
}

#[derive(Debug, Deserialize)]
struct Canvas {
    width: u32,
    height: u32,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Asset {
    id: String,
    file: String,
    bytes: u64,
    sha256: String,
    source_canvas_sha256: String,
}

#[derive(Debug, Deserialize)]
struct ReelManifest {
    rigs: Vec<ReelRig>,
}

#[derive(Debug, Deserialize)]
struct ReelRig {
    id: String,
    canvas_sha256: String,
}

fn bundle_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../apps/desktop/ui/personas")
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut hex = String::with_capacity(64);
    for byte in Sha256::digest(bytes) {
        write!(hex, "{byte:02x}").expect("writing to a String cannot fail");
    }
    hex
}

fn is_lower_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

#[test]
fn bundled_persona_fallbacks_are_the_pinned_transparent_full_body_previews() {
    let root = bundle_root();
    let manifest_raw = fs::read(root.join("manifest.json")).expect("read bundled Persona manifest");
    let manifest: Manifest =
        serde_json::from_slice(&manifest_raw).expect("bundled Persona manifest is valid JSON");

    assert_eq!(manifest.schema, "ai-sister/bundled-personas/v2");
    assert_eq!(manifest.preview_contract.subject, "full-body");
    assert_eq!(manifest.preview_contract.canvas.width, 640);
    assert_eq!(manifest.preview_contract.canvas.height, 640);
    assert_eq!(manifest.preview_contract.alpha, "transparent");
    assert_eq!(manifest.preview_contract.fit, "contain");

    let expected = PERSONAS.into_iter().collect::<BTreeSet<_>>();
    let actual = manifest
        .assets
        .iter()
        .map(|asset| asset.id.as_str())
        .collect::<BTreeSet<_>>();
    assert_eq!(manifest.assets.len(), PERSONAS.len());
    assert_eq!(
        actual.len(),
        manifest.assets.len(),
        "Persona IDs must be unique"
    );
    assert_eq!(actual, expected, "bundled Persona roster changed");

    let on_disk_files = fs::read_dir(&root)
        .expect("read bundled Persona directory")
        .map(|entry| {
            let entry = entry.expect("read bundled Persona directory entry");
            let file_type = entry.file_type().expect("read Persona file type");
            assert!(
                file_type.is_file() && !file_type.is_symlink(),
                "bundled Persona directory only permits reviewed regular files: {}",
                entry.path().display()
            );
            entry.file_name().to_string_lossy().into_owned()
        })
        .collect::<BTreeSet<_>>();
    let mut declared_files = manifest
        .assets
        .iter()
        .map(|asset| asset.file.clone())
        .collect::<BTreeSet<_>>();
    declared_files.extend(["manifest.json", "NOTICE.md", "catalog.js"].map(String::from));
    assert_eq!(
        on_disk_files, declared_files,
        "bundled Persona directory has an unreviewed or missing file"
    );

    let reel_raw = fs::read(root.join("../persona-reels/manifest.json"))
        .expect("read bundled Persona reel manifest");
    assert_eq!(
        manifest.source_reel_manifest_sha256,
        sha256_hex(&reel_raw),
        "fallback manifest is not bound to the shipped Reel manifest bytes"
    );
    let reel_manifest: ReelManifest =
        serde_json::from_slice(&reel_raw).expect("bundled Persona reel manifest is valid JSON");
    let reel_canvases = reel_manifest
        .rigs
        .iter()
        .map(|rig| (rig.id.as_str(), rig.canvas_sha256.as_str()))
        .collect::<BTreeMap<_, _>>();
    assert_eq!(
        reel_canvases.len(),
        reel_manifest.rigs.len(),
        "Persona reel IDs must be unique"
    );
    assert_eq!(
        reel_canvases.keys().copied().collect::<BTreeSet<_>>(),
        expected,
        "fallback and layered-rig rosters differ"
    );

    for asset in &manifest.assets {
        assert_eq!(asset.file, format!("{}.webp", asset.id));
        assert!(
            is_lower_sha256(&asset.sha256),
            "{} SHA-256 must be lowercase hex",
            asset.id
        );
        assert!(
            is_lower_sha256(&asset.source_canvas_sha256),
            "{} source canvas SHA-256 must be lowercase hex",
            asset.id
        );
        assert_eq!(
            Some(asset.source_canvas_sha256.as_str()),
            reel_canvases.get(asset.id.as_str()).copied(),
            "{} fallback is not bound to its layered-rig source canvas",
            asset.id
        );

        let encoded = fs::read(root.join(&asset.file))
            .unwrap_or_else(|error| panic!("read {}: {error}", asset.file));
        assert_eq!(
            u64::try_from(encoded.len()).expect("WebP byte count fits u64"),
            asset.bytes,
            "{} byte count changed",
            asset.id
        );
        assert_eq!(
            sha256_hex(&encoded),
            asset.sha256,
            "{} SHA-256 changed",
            asset.id
        );

        let decoded = image::load_from_memory_with_format(&encoded, ImageFormat::WebP)
            .unwrap_or_else(|error| panic!("decode {} as WebP: {error}", asset.file));
        assert_eq!(
            decoded.dimensions(),
            (
                manifest.preview_contract.canvas.width,
                manifest.preview_contract.canvas.height,
            ),
            "{} does not use the declared full-body preview canvas",
            asset.id
        );
        assert!(
            decoded.color().has_alpha(),
            "{} decoded without an alpha channel",
            asset.id
        );

        let rgba = decoded.into_rgba8();
        // `subject: full-body` 只是 manifest 自述。可見 alpha 必須從頭跨到腳，才不會讓
        // 一張放在透明 640 canvas 中間的半身像也通過；不綁產線的 exact resize 高度。
        let mut has_transparent = false;
        let mut has_opaque = false;
        let mut min_visible_y = None;
        let mut max_visible_y = None;
        let mut visible_in_top_tenth = false;
        let mut visible_in_bottom_tenth = false;
        let canvas_height = manifest.preview_contract.canvas.height;
        for (_, y, pixel) in rgba.enumerate_pixels() {
            if pixel.0[3] == 0 {
                has_transparent = true;
                continue;
            }
            has_opaque |= pixel.0[3] == 255;
            min_visible_y = Some(min_visible_y.map_or(y, |found: u32| found.min(y)));
            max_visible_y = Some(max_visible_y.map_or(y, |found: u32| found.max(y)));
            visible_in_top_tenth |= y < canvas_height / 10;
            visible_in_bottom_tenth |= y >= canvas_height - canvas_height / 10;
        }
        assert!(
            has_transparent,
            "{} has no fully transparent canvas pixel",
            asset.id
        );
        assert!(
            has_opaque,
            "{} has no fully opaque character pixel",
            asset.id
        );
        let (min_visible_y, max_visible_y) = min_visible_y
            .zip(max_visible_y)
            .unwrap_or_else(|| panic!("{} contains no visible character pixel", asset.id));
        let visible_height = max_visible_y - min_visible_y + 1;
        assert!(
            visible_height >= 600,
            "{} visible alpha spans only {visible_height}px vertically",
            asset.id
        );
        assert!(
            visible_in_top_tenth,
            "{} has no visible alpha in the canvas top 10%",
            asset.id
        );
        assert!(
            visible_in_bottom_tenth,
            "{} has no visible alpha in the canvas bottom 10%",
            asset.id
        );
    }
}
