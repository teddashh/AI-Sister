//! Persona v1 固定素材的內嵌 authority。
//!
//! 完整 public schema-v2 manifest 有 946 項；runtime 不需要把 2.1 MB JSON 再複製
//! 一份進每個 binary。正式 app 內嵌 canonical manifest hash、descriptor、origin/path
//! allowlist，以及真正會呈現的 4 張立繪與 8 條聲音 projection。整包 SHA-256 又把
//! ZIP 內全部 946 項釘死；parser 仍逐項驗內容 hash 與檔名相同。

pub const RELEASE_ID: &str = "ai-sister-media-11-voice55-2026.07.23";
pub const CANONICAL_MANIFEST_SHA256: &str =
    "21e4675653ce66b50b61e91260f1623e6e3005177f900991e3a8eeadaf9e6474";
pub const ORIGIN: &str = "https://cdn.ted-h.com";
pub const HOST: &str = "cdn.ted-h.com";
pub const PACK_PATH: &str = "/tokenmonster/characters/v1/packs/ai-sister-media-11-voice55-2026.07.23/7d98e0d18c470f82818e8ada67208847c3cf4ff5c10cb5f99f9215191e981f30.zip";
pub const PACK_URL: &str = "https://cdn.ted-h.com/tokenmonster/characters/v1/packs/ai-sister-media-11-voice55-2026.07.23/7d98e0d18c470f82818e8ada67208847c3cf4ff5c10cb5f99f9215191e981f30.zip";
pub const PACK_MEDIA_TYPE: &str = "application/zip";
pub const PACK_BYTES: usize = 73_261_088;
pub const PACK_SHA256: &str = "7d98e0d18c470f82818e8ada67208847c3cf4ff5c10cb5f99f9215191e981f30";
pub const PACK_ENTRY_COUNT: usize = 946;
pub const PACK_EXTRACTED_BYTES: usize = 73_043_596;

/// 給下載按鈕看的固定承諾。這不是「沒有送任何東西」：host 仍看得到一般網路
/// metadata，下面這句刻意把那一面也寫出來。
pub const REQUEST_BOUNDARY_ZH_TW: &str = "只送出同一個固定 HTTPS GET；不帶角色選擇、使用狀態、畫面、OCR、問題、答案、記憶 ID 或資料庫內容。CDN／DNS／TLS 仍可看見來源 IP、連線時間、固定 host、固定路徑與固定 headers。";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Persona {
    Chatgpt,
    Claude,
    Gemini,
    Grok,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum VoiceLine {
    ChatgptGreeting,
    ChatgptQuiet,
    ClaudeGreeting,
    ClaudeQuiet,
    GeminiGreeting,
    GeminiQuiet,
    GrokGreeting,
    GrokQuiet,
}

impl VoiceLine {
    pub const fn persona(self) -> Persona {
        match self {
            Self::ChatgptGreeting | Self::ChatgptQuiet => Persona::Chatgpt,
            Self::ClaudeGreeting | Self::ClaudeQuiet => Persona::Claude,
            Self::GeminiGreeting | Self::GeminiQuiet => Persona::Gemini,
            Self::GrokGreeting | Self::GrokQuiet => Persona::Grok,
        }
    }

    /// 這句 WAV 在發布審查時核准的逐字稿。它跟 voice line enum、selected object
    /// hash 一起編進 app；renderer 的顯示文字不同時，聲音必須 fail closed。
    pub const fn spoken_text(self) -> &'static str {
        match self {
            Self::ChatgptGreeting => "我在，隨時可以開始。",
            Self::ChatgptQuiet => "我在，安靜地開始也很好。",
            Self::ClaudeGreeting => "慢慢來，隨時可以開始。",
            Self::ClaudeQuiet => "慢慢來，安靜地開始也很好。",
            Self::GeminiGreeting => "一起看看，隨時可以開始。",
            Self::GeminiQuiet => "一起看看，安靜地開始也很好。",
            Self::GrokGreeting => "收到，隨時可以開始。",
            Self::GrokQuiet => "收到，安靜地開始也很好。",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    Webp,
    Wav,
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum Use {
    Portrait(Persona),
    Voice(VoiceLine),
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct Asset {
    pub object: &'static str,
    pub bytes: usize,
    pub kind: Kind,
    pub use_as: Use,
    pub duration_ms: Option<u32>,
    pub review: Review,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct Review {
    pub license_approved: bool,
    pub public_use: bool,
    pub commercial_use: bool,
    pub modify: bool,
    pub redistribute: bool,
    pub brand_approved: bool,
    pub content_approved: bool,
    pub release_approved: bool,
    pub grant_reference_id: &'static str,
    pub content_review_reference_id: &'static str,
    pub content_rating: &'static str,
    pub disclosure_id: &'static str,
    pub voice_locale: Option<&'static str>,
}

const IMAGE_REVIEW: Review = Review {
    license_approved: true,
    public_use: true,
    commercial_use: true,
    modify: true,
    redistribute: true,
    brand_approved: true,
    content_approved: true,
    release_approved: true,
    grant_reference_id: "owner-chat-image-grant-2026.07.21",
    content_review_reference_id: "owner-chat-content-review-2026.07.21",
    content_rating: "general",
    disclosure_id: "tokenmonster-unaffiliated-v1",
    voice_locale: None,
};

const VOICE_REVIEW: Review = Review {
    license_approved: true,
    public_use: true,
    commercial_use: true,
    modify: true,
    redistribute: true,
    brand_approved: true,
    content_approved: true,
    release_approved: true,
    grant_reference_id: "owner-voice-authorization-2026.07.23",
    content_review_reference_id: "owner-voice-publication-approval-2026.07.23",
    content_rating: "general",
    disclosure_id: "tokenmonster-unaffiliated-v1",
    voice_locale: Some("zh-TW"),
};

macro_rules! portrait {
    ($persona:ident, $sha:literal, $bytes:literal) => {
        Asset {
            object: concat!("objects/", $sha, ".webp"),
            bytes: $bytes,
            kind: Kind::Webp,
            use_as: Use::Portrait(Persona::$persona),
            duration_ms: None,
            review: IMAGE_REVIEW,
        }
    };
}

macro_rules! voice {
    ($line:ident, $sha:literal, $bytes:literal, $duration:literal) => {
        Asset {
            object: concat!("objects/", $sha, ".wav"),
            bytes: $bytes,
            kind: Kind::Wav,
            use_as: Use::Voice(VoiceLine::$line),
            duration_ms: Some($duration),
            review: VOICE_REVIEW,
        }
    };
}

/// schema-v2 public manifest 中，AI-Sister Release 1.0 會實際呈現的 projection。
pub(crate) const ASSETS: [Asset; 12] = [
    portrait!(
        Chatgpt,
        "2966f68a3e702c47a29d11c19b901a4d850a8914ec2b78252b617d757c04fca3",
        17_722
    ),
    voice!(
        ChatgptGreeting,
        "5090eff257a191d7eebe029844e5ebbe1a24470902226a054929a059880cdc1f",
        99_954,
        2_266
    ),
    voice!(
        ChatgptQuiet,
        "f09f611a64e97f2d8c0a1f5cf2f67b8c9c41c0220b13ffeb91017f3558531973",
        99_984,
        2_266
    ),
    portrait!(
        Claude,
        "f4b50a1ffa8f717a2717bd14551c84a2258b4985e3e8b7f4b18d6008a660f2ae",
        19_088
    ),
    voice!(
        ClaudeGreeting,
        "d3dd0b061ded630fd26d990a2613574ea20bc6e2a779298b6a662bc69638b3eb",
        102_446,
        2_322
    ),
    voice!(
        ClaudeQuiet,
        "e27b1be5cbd03c15f2068e5d29dd300912df87c7be457fd3d9c531e2fe511e87",
        138_286,
        3_135
    ),
    portrait!(
        Gemini,
        "c4fa29dbd7142705b3ef35477c86aa5372d6c66cc9ef6c5a3f7c55b2bac926ef",
        19_898
    ),
    voice!(
        GeminiGreeting,
        "ce60c0ab6d141b7b6932a72760eb336fcf9596286ca90bcf7f272496770ed84f",
        132_142,
        2_995
    ),
    voice!(
        GeminiQuiet,
        "c531b2412fa4819f03179d8bc6e7f5026446a39c5a983578a774f52babfcab06",
        88_924,
        2_015
    ),
    portrait!(
        Grok,
        "41377cc281b8f5685628da087f82055cbf7806949b33dcc19c8814240c3d4995",
        15_012
    ),
    voice!(
        GrokGreeting,
        "d5ac487e429aa41402abda3da2780942f64dd8e03b9b071f462c780b352101fb",
        89_134,
        2_020
    ),
    voice!(
        GrokQuiet,
        "243be6c173a40399067775919975c5c0d797bd6f3e3b643f347839aa3e9022d7",
        101_422,
        2_299
    ),
];

pub(crate) fn portrait(persona: Persona) -> &'static Asset {
    ASSETS
        .iter()
        .find(|asset| matches!(asset.use_as, Use::Portrait(found) if found == persona))
        .expect("embedded authority has one portrait per persona")
}

pub(crate) fn voice(line: VoiceLine) -> &'static Asset {
    ASSETS
        .iter()
        .find(|asset| matches!(asset.use_as, Use::Voice(found) if found == line))
        .expect("embedded authority has one object per voice line")
}

/// 把 selected public rights projection 也放進啟用判斷，而不是只在文件寫「審過」。
pub(crate) fn selected_rights_are_approved() -> bool {
    ASSETS.iter().all(asset_rights_are_approved)
}

fn asset_rights_are_approved(asset: &Asset) -> bool {
    let review = asset.review;
    review.license_approved
        && review.public_use
        && review.commercial_use
        && review.modify
        && review.redistribute
        && review.brand_approved
        && review.content_approved
        && review.release_approved
        && !review.grant_reference_id.is_empty()
        && !review.content_review_reference_id.is_empty()
        && review.content_rating == "general"
        && review.disclosure_id == "tokenmonster-unaffiliated-v1"
        && match asset.use_as {
            Use::Portrait(_) => review.voice_locale.is_none(),
            Use::Voice(_) => review.voice_locale == Some("zh-TW"),
        }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use serde::Deserialize;

    use super::*;
    use crate::zip::sha256_hex;

    const PUBLIC_PROJECTION: &str =
        include_str!("../tests/fixtures/public-manifest-selected-v2.json");
    const PUBLIC_PROJECTION_SHA256: &str =
        "046e26f1e17728ad7fe5369b33ce7a90107b1ab5e6cdc067842e3fde07e26d49";

    #[derive(Debug, Deserialize)]
    #[serde(rename_all = "camelCase", deny_unknown_fields)]
    struct Fixture {
        fixture_schema: u32,
        manifest: Manifest,
        descriptor: Descriptor,
        selected_assets: Vec<SelectedAsset>,
    }

    #[derive(Debug, Deserialize)]
    #[serde(rename_all = "camelCase", deny_unknown_fields)]
    struct Manifest {
        schema_version: String,
        release_id: String,
        canonical_sha256: String,
    }

    #[derive(Debug, Deserialize)]
    #[serde(rename_all = "camelCase", deny_unknown_fields)]
    struct Descriptor {
        origin: String,
        path: String,
        media_type: String,
        bytes: usize,
        entries: usize,
        extracted_bytes: usize,
        sha256: String,
    }

    #[derive(Debug, Deserialize)]
    #[serde(rename_all = "camelCase", deny_unknown_fields)]
    struct SelectedAsset {
        asset_id: String,
        association: Association,
        output: Output,
        rights: Rights,
        review: PublicReview,
        voice_locale: Option<String>,
        spoken_text: Option<String>,
        release_status: String,
    }

    #[derive(Debug, Deserialize)]
    #[serde(rename_all = "camelCase", deny_unknown_fields)]
    struct Association {
        kind: String,
        character_id: String,
        line_id: Option<String>,
        trigger: Option<String>,
    }

    #[derive(Debug, Deserialize)]
    #[serde(rename_all = "camelCase", deny_unknown_fields)]
    struct Output {
        path: String,
        bytes: usize,
        sha256: String,
        media_type: String,
        duration_ms: Option<u32>,
    }

    #[derive(Debug, Deserialize)]
    #[serde(rename_all = "camelCase", deny_unknown_fields)]
    struct Rights {
        license_status: String,
        grant_reference_id: String,
        public_use: bool,
        commercial_use: bool,
        modify: bool,
        redistribute: bool,
    }

    #[derive(Debug, Deserialize)]
    #[serde(rename_all = "camelCase", deny_unknown_fields)]
    struct PublicReview {
        brand_status: String,
        content_status: String,
        content_review_reference_id: String,
        content_rating: String,
        disclosure_id: String,
    }

    #[test]
    fn compact_public_manifest_projection_matches_runtime_authority() {
        assert_eq!(
            sha256_hex(PUBLIC_PROJECTION.as_bytes()),
            PUBLIC_PROJECTION_SHA256
        );
        let fixture: Fixture = serde_json::from_str(PUBLIC_PROJECTION)
            .expect("pinned public projection is valid JSON");

        assert_eq!(fixture.fixture_schema, 2);
        assert_eq!(fixture.manifest.schema_version, "2");
        assert_eq!(fixture.manifest.release_id, RELEASE_ID);
        assert_eq!(fixture.manifest.canonical_sha256, CANONICAL_MANIFEST_SHA256);
        assert_eq!(fixture.descriptor.origin, ORIGIN);
        assert_eq!(fixture.descriptor.path, PACK_PATH);
        assert_eq!(fixture.descriptor.media_type, PACK_MEDIA_TYPE);
        assert_eq!(fixture.descriptor.bytes, PACK_BYTES);
        assert_eq!(fixture.descriptor.entries, PACK_ENTRY_COUNT);
        assert_eq!(fixture.descriptor.extracted_bytes, PACK_EXTRACTED_BYTES);
        assert_eq!(fixture.descriptor.sha256, PACK_SHA256);
        assert_eq!(
            format!("{}{}", fixture.descriptor.origin, fixture.descriptor.path),
            PACK_URL
        );

        assert_eq!(fixture.selected_assets.len(), ASSETS.len());
        let mut asset_ids = HashSet::new();
        let mut output_paths = HashSet::new();
        for (projected, asset) in fixture.selected_assets.iter().zip(&ASSETS) {
            let expected = identity(asset.use_as);
            assert_eq!(projected.asset_id, expected.asset_id);
            assert_eq!(projected.association.kind, expected.kind);
            assert_eq!(projected.association.character_id, expected.character_id);
            assert_eq!(projected.association.line_id.as_deref(), expected.line_id);
            assert_eq!(projected.association.trigger.as_deref(), expected.trigger);
            assert_eq!(projected.output.path, asset.object);
            assert_eq!(projected.output.bytes, asset.bytes);
            assert_eq!(projected.output.sha256, object_sha(asset.object));
            assert_eq!(projected.output.media_type, expected.media_type);
            assert_eq!(projected.output.duration_ms, asset.duration_ms);

            let review = asset.review;
            assert_eq!(
                projected.rights.license_status == "approved",
                review.license_approved
            );
            assert_eq!(
                projected.rights.grant_reference_id,
                review.grant_reference_id
            );
            assert_eq!(projected.rights.public_use, review.public_use);
            assert_eq!(projected.rights.commercial_use, review.commercial_use);
            assert_eq!(projected.rights.modify, review.modify);
            assert_eq!(projected.rights.redistribute, review.redistribute);
            assert_eq!(
                projected.review.brand_status == "approved",
                review.brand_approved
            );
            assert_eq!(
                projected.review.content_status == "approved",
                review.content_approved
            );
            assert_eq!(
                projected.review.content_review_reference_id,
                review.content_review_reference_id
            );
            assert_eq!(projected.review.content_rating, review.content_rating);
            assert_eq!(projected.review.disclosure_id, review.disclosure_id);
            assert_eq!(projected.voice_locale.as_deref(), review.voice_locale);
            assert_eq!(
                projected.spoken_text.as_deref(),
                match asset.use_as {
                    Use::Portrait(_) => None,
                    Use::Voice(line) => Some(line.spoken_text()),
                }
            );
            assert_eq!(
                projected.release_status == "approved",
                review.release_approved
            );
            assert!(asset_ids.insert(projected.asset_id.as_str()));
            assert!(output_paths.insert(projected.output.path.as_str()));
            assert!(asset_rights_are_approved(asset));
        }
    }

    #[test]
    #[ignore = "release/manual：需要 AI_SISTER_PUBLIC_ASSET_MANIFEST 指到完整公開 manifest"]
    fn full_public_manifest_canonical_hash_and_projection_match() {
        let path = std::env::var_os("AI_SISTER_PUBLIC_ASSET_MANIFEST")
            .expect("set AI_SISTER_PUBLIC_ASSET_MANIFEST");
        let raw = std::fs::read(path).expect("read full public manifest");
        let manifest: serde_json::Value =
            serde_json::from_slice(&raw).expect("full public manifest is JSON");

        // serde_json::Map 在沒有 preserve_order feature 時是 BTreeMap；compact UTF-8
        // serialization 正好是上游產生 canonical digest 的 sort-keys/no-whitespace
        // 規則。先驗 hash，底下的欄位投影才真的屬於這一份 public manifest。
        let canonical = serde_json::to_vec(&manifest).expect("canonicalize public manifest");
        assert_eq!(sha256_hex(&canonical), CANONICAL_MANIFEST_SHA256);
        assert_eq!(manifest["schemaVersion"], "2");
        assert_eq!(manifest["releaseId"], RELEASE_ID);
        let full_assets = manifest["assets"].as_array().expect("assets array");
        assert_eq!(full_assets.len(), PACK_ENTRY_COUNT);

        let fixture: Fixture = serde_json::from_str(PUBLIC_PROJECTION)
            .expect("pinned public projection is valid JSON");
        for projected in &fixture.selected_assets {
            let full = full_assets
                .iter()
                .find(|asset| asset["assetId"] == projected.asset_id)
                .expect("selected asset exists in canonical manifest");
            assert_eq!(full["association"]["kind"], projected.association.kind);
            assert_eq!(
                full["association"]["characterId"],
                projected.association.character_id
            );
            assert_eq!(
                full["association"].get("lineId").and_then(|v| v.as_str()),
                projected.association.line_id.as_deref()
            );
            assert_eq!(
                full["association"].get("trigger").and_then(|v| v.as_str()),
                projected.association.trigger.as_deref()
            );
            assert_eq!(full["output"]["path"], projected.output.path);
            assert_eq!(full["output"]["bytes"], projected.output.bytes);
            assert_eq!(full["output"]["sha256"], projected.output.sha256);
            assert_eq!(
                full["output"]["media"]["mediaType"],
                projected.output.media_type
            );
            assert_eq!(
                full["output"]["media"]
                    .get("durationMs")
                    .and_then(|v| v.as_u64()),
                projected.output.duration_ms.map(u64::from)
            );
            assert_eq!(
                full["rights"]["licenseStatus"],
                projected.rights.license_status
            );
            assert_eq!(
                full["rights"]["grantReferenceId"],
                projected.rights.grant_reference_id
            );
            assert_eq!(
                full["rights"]["scopes"]["publicUse"],
                projected.rights.public_use
            );
            assert_eq!(
                full["rights"]["scopes"]["commercialUse"],
                projected.rights.commercial_use
            );
            assert_eq!(full["rights"]["scopes"]["modify"], projected.rights.modify);
            assert_eq!(
                full["rights"]["scopes"]["redistribute"],
                projected.rights.redistribute
            );
            assert_eq!(full["review"]["brandStatus"], projected.review.brand_status);
            assert_eq!(
                full["review"]["contentStatus"],
                projected.review.content_status
            );
            assert_eq!(
                full["review"]["contentReviewReferenceId"],
                projected.review.content_review_reference_id
            );
            assert_eq!(
                full["review"]["contentRating"],
                projected.review.content_rating
            );
            assert_eq!(
                full["review"]["disclosureId"],
                projected.review.disclosure_id
            );
            assert_eq!(
                full.get("voiceEvidence")
                    .and_then(|v| v.get("locale"))
                    .and_then(|v| v.as_str()),
                projected.voice_locale.as_deref()
            );
            assert_eq!(full["releaseStatus"], projected.release_status);
        }
    }

    #[test]
    fn every_new_rights_field_is_fail_closed() {
        let asset = ASSETS[0];

        let mut commercial_use = asset;
        commercial_use.review.commercial_use = false;
        assert!(!asset_rights_are_approved(&commercial_use));

        let mut modify = asset;
        modify.review.modify = false;
        assert!(!asset_rights_are_approved(&modify));

        let mut content_rating = asset;
        content_rating.review.content_rating = "unknown";
        assert!(!asset_rights_are_approved(&content_rating));

        let mut disclosure = asset;
        disclosure.review.disclosure_id = "";
        assert!(!asset_rights_are_approved(&disclosure));
    }

    #[derive(Debug)]
    struct Identity {
        asset_id: &'static str,
        kind: &'static str,
        character_id: &'static str,
        line_id: Option<&'static str>,
        trigger: Option<&'static str>,
        media_type: &'static str,
    }

    fn identity(use_as: Use) -> Identity {
        match use_as {
            Use::Portrait(Persona::Chatgpt) => portrait_identity("chatgpt"),
            Use::Portrait(Persona::Claude) => portrait_identity("claude"),
            Use::Portrait(Persona::Gemini) => portrait_identity("gemini"),
            Use::Portrait(Persona::Grok) => portrait_identity("grok"),
            Use::Voice(line) => voice_identity(line),
        }
    }

    fn portrait_identity(character_id: &'static str) -> Identity {
        let asset_id = match character_id {
            "chatgpt" => "asset:chatgpt:avatar",
            "claude" => "asset:claude:avatar",
            "gemini" => "asset:gemini:avatar",
            "grok" => "asset:grok:avatar",
            _ => unreachable!("closed persona enum"),
        };
        Identity {
            asset_id,
            kind: "avatar",
            character_id,
            line_id: None,
            trigger: None,
            media_type: "image/webp",
        }
    }

    fn voice_identity(line: VoiceLine) -> Identity {
        let (asset_id, character_id, line_id, trigger) = match line {
            VoiceLine::ChatgptGreeting => (
                "asset:chatgpt:voice:chatgpt-greeting:greeting",
                "chatgpt",
                "chatgpt-greeting",
                "greeting",
            ),
            VoiceLine::ChatgptQuiet => (
                "asset:chatgpt:voice:chatgpt-quiet:quiet",
                "chatgpt",
                "chatgpt-quiet",
                "quiet",
            ),
            VoiceLine::ClaudeGreeting => (
                "asset:claude:voice:claude-greeting:greeting",
                "claude",
                "claude-greeting",
                "greeting",
            ),
            VoiceLine::ClaudeQuiet => (
                "asset:claude:voice:claude-quiet:quiet",
                "claude",
                "claude-quiet",
                "quiet",
            ),
            VoiceLine::GeminiGreeting => (
                "asset:gemini:voice:gemini-greeting:greeting",
                "gemini",
                "gemini-greeting",
                "greeting",
            ),
            VoiceLine::GeminiQuiet => (
                "asset:gemini:voice:gemini-quiet:quiet",
                "gemini",
                "gemini-quiet",
                "quiet",
            ),
            VoiceLine::GrokGreeting => (
                "asset:grok:voice:grok-greeting:greeting",
                "grok",
                "grok-greeting",
                "greeting",
            ),
            VoiceLine::GrokQuiet => (
                "asset:grok:voice:grok-quiet:quiet",
                "grok",
                "grok-quiet",
                "quiet",
            ),
        };
        Identity {
            asset_id,
            kind: "voice",
            character_id,
            line_id: Some(line_id),
            trigger: Some(trigger),
            media_type: "audio/wav",
        }
    }

    fn object_sha(path: &str) -> &str {
        path.strip_prefix("objects/")
            .and_then(|name| name.split_once('.'))
            .map(|(sha, _)| sha)
            .expect("embedded object path shape")
    }
}
