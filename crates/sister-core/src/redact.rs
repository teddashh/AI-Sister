//! 秘密偵測：剪貼簿內容在**落地之前**判斷該不該存。
//!
//! SPEC §11.2：偵測到秘密就不落地，只記「複製了一個秘密」這件事。
//! 這是 capture 當下的排除，不是事後刪除——事後刪除救不了已經寫進磁碟的東西。

/// 偵測到的秘密型別。只用來寫進 detail 欄位供使用者稽核，內容永不落地。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SecretKind {
    ApiKey,
    PrivateKey,
    Jwt,
    HighEntropy,
}

impl SecretKind {
    pub fn as_str(self) -> &'static str {
        match self {
            SecretKind::ApiKey => "api_key",
            SecretKind::PrivateKey => "private_key",
            SecretKind::Jwt => "jwt",
            SecretKind::HighEntropy => "high_entropy",
        }
    }
}

/// 已知的憑證前綴。命中即視為秘密，不看熵值。
const KEY_PREFIXES: &[&str] = &[
    "sk-",
    "sk_live_",
    "sk_test_",
    "pk_live_",
    "rk_live_", // OpenAI / Stripe 類
    "ghp_",
    "gho_",
    "ghu_",
    "ghs_",
    "ghr_",
    "github_pat_", // GitHub
    "xoxb-",
    "xoxp-",
    "xoxa-",
    "xoxs-", // Slack
    "AKIA",
    "ASIA",   // AWS access key id
    "AIza",   // Google API key
    "ya29.",  // Google OAuth token
    "hf_",    // HuggingFace
    "glpat-", // GitLab
    "dop_v1_",
    "doo_v1_", // DigitalOcean
    "shpat_",
    "shpss_", // Shopify
    "npm_",   // npm
    "sq0atp-",
    "sq0csp-", // Square
];

const PRIVATE_KEY_MARKERS: &[&str] = &[
    "-----BEGIN RSA PRIVATE KEY-----",
    "-----BEGIN PRIVATE KEY-----",
    "-----BEGIN OPENSSH PRIVATE KEY-----",
    "-----BEGIN EC PRIVATE KEY-----",
    "-----BEGIN DSA PRIVATE KEY-----",
    "-----BEGIN PGP PRIVATE KEY BLOCK-----",
];

/// 判斷一段文字是否疑似秘密。
///
/// 寧可誤判成秘密（代價：少存一段剪貼簿），也不要漏判（代價：API key 進資料庫）。
pub fn looks_like_secret(text: &str) -> Option<SecretKind> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return None;
    }

    for marker in PRIVATE_KEY_MARKERS {
        if trimmed.contains(marker) {
            return Some(SecretKind::PrivateKey);
        }
    }

    // 前綴比對只看每個 whitespace-token 的開頭，避免整段文章誤觸
    for token in trimmed.split_whitespace() {
        for prefix in KEY_PREFIXES {
            if token.len() >= prefix.len() + 8 && token.starts_with(prefix) {
                return Some(SecretKind::ApiKey);
            }
        }
        if is_jwt(token) {
            return Some(SecretKind::Jwt);
        }
    }

    // 單一 token、夠長、字元集像隨機、熵夠高 → 疑似憑證
    if !trimmed.contains(char::is_whitespace)
        && trimmed.len() >= 32
        && trimmed.len() <= 512
        && looks_random(trimmed)
        && shannon_entropy_per_char(trimmed) >= 3.6
    {
        return Some(SecretKind::HighEntropy);
    }

    None
}

fn is_jwt(token: &str) -> bool {
    // header.payload.signature，且 header 以 base64 的 `{"` 開頭（eyJ）
    let parts: Vec<&str> = token.split('.').collect();
    parts.len() == 3
        && parts[0].starts_with("eyJ")
        && parts.iter().all(|p| p.len() >= 8)
        && parts.iter().all(|p| {
            p.chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '=')
        })
}

/// 字元集是否像隨機憑證：僅 ASCII 可見字元、且同時含大小寫或數字。
fn looks_random(s: &str) -> bool {
    if !s.is_ascii() {
        return false;
    }
    let mut has_digit = false;
    let mut has_alpha = false;
    for c in s.chars() {
        if !(c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '+' | '/' | '=' | '.')) {
            return false;
        }
        if c.is_ascii_digit() {
            has_digit = true;
        }
        if c.is_ascii_alphabetic() {
            has_alpha = true;
        }
    }
    has_digit && has_alpha
}

/// 每字元的 Shannon 熵（bits）。英文散文約 3.0–3.5，base64 憑證約 4.5–6.0。
pub fn shannon_entropy_per_char(s: &str) -> f64 {
    if s.is_empty() {
        return 0.0;
    }
    let mut counts = [0usize; 256];
    let mut total = 0usize;
    for b in s.bytes() {
        counts[b as usize] += 1;
        total += 1;
    }
    let total_f = total as f64;
    let mut entropy = 0.0;
    for c in counts.iter().filter(|c| **c > 0) {
        let p = *c as f64 / total_f;
        entropy -= p * p.log2();
    }
    entropy
}

/// 剪貼簿文字的落地上限（SPEC §2.1：>64KB 截斷）。
pub const CLIPBOARD_MAX_BYTES: usize = 64 * 1024;

/// 依 UTF-8 字元邊界安全截斷。回傳 (截斷後文字, 是否被截斷)。
pub fn truncate_utf8(text: &str, max_bytes: usize) -> (&str, bool) {
    if text.len() <= max_bytes {
        return (text, false);
    }
    let mut end = max_bytes;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    (&text[..end], true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_known_key_prefixes() {
        assert_eq!(
            looks_like_secret("sk-proj-abc123def456ghi789jkl"),
            Some(SecretKind::ApiKey)
        );
        assert_eq!(
            looks_like_secret("ghp_16CharsAtLeastHereOk123"),
            Some(SecretKind::ApiKey)
        );
        assert_eq!(
            looks_like_secret("AKIAIOSFODNN7EXAMPLE"),
            Some(SecretKind::ApiKey)
        );
        assert_eq!(
            looks_like_secret("xoxb-123456789012-abcdefghijkl"),
            Some(SecretKind::ApiKey)
        );
        // 前綴出現在句子中間也要抓到
        assert_eq!(
            looks_like_secret("export KEY=\nghp_16CharsAtLeastHereOk123"),
            Some(SecretKind::ApiKey)
        );
    }

    #[test]
    fn detects_private_keys_and_jwt() {
        assert_eq!(
            looks_like_secret("-----BEGIN OPENSSH PRIVATE KEY-----\nabc\n"),
            Some(SecretKind::PrivateKey)
        );
        let jwt = "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.dozjgNryP4J3jVmNHl0w5N_XgL0n3I9PlFUP0THsR8U";
        assert_eq!(looks_like_secret(jwt), Some(SecretKind::Jwt));
    }

    #[test]
    fn detects_high_entropy_blobs() {
        assert_eq!(
            looks_like_secret("Zx9Kq2Lm7Pw4Rt8Yv1Nb6Hs3Jd5Gf0Ac2Ee4Ww8Qq"),
            Some(SecretKind::HighEntropy)
        );
    }

    #[test]
    fn ordinary_text_is_not_a_secret() {
        assert_eq!(looks_like_secret(""), None);
        assert_eq!(looks_like_secret("   "), None);
        assert_eq!(looks_like_secret("hello world"), None);
        assert_eq!(
            looks_like_secret("客服電話 0800-123-456，帳單金額 NT$13,450"),
            None
        );
        assert_eq!(
            looks_like_secret("https://github.com/teddashh/AI-Sister/blob/main/docs/SPEC.md"),
            None
        );
        // 長但是有空白的散文，不該被熵規則抓走
        assert_eq!(
            looks_like_secret("這是一段很長的中文說明文字，用來測試不會被誤判成秘密。"),
            None
        );
        // 純數字長串（訂單號）不是秘密
        assert_eq!(
            looks_like_secret("1234567890123456789012345678901234"),
            None
        );
        // 短的 sk- 開頭（例如 "sk-" 本身或縮寫）不該誤判
        assert_eq!(looks_like_secret("sk-123"), None);
    }

    #[test]
    fn entropy_ranks_random_above_prose() {
        let prose = shannon_entropy_per_char("the quick brown fox jumps over the lazy dog");
        let random = shannon_entropy_per_char("Zx9Kq2Lm7Pw4Rt8Yv1Nb6Hs3Jd5Gf0Ac");
        assert!(
            random > prose,
            "random {random} should exceed prose {prose}"
        );
        assert_eq!(shannon_entropy_per_char(""), 0.0);
        assert_eq!(shannon_entropy_per_char("aaaa"), 0.0);
    }

    #[test]
    fn truncate_respects_utf8_boundaries() {
        let (s, cut) = truncate_utf8("hello", 100);
        assert_eq!((s, cut), ("hello", false));

        // 每個中文字 3 bytes，砍在 4 bytes 應退回 3
        let (s, cut) = truncate_utf8("帳單金額", 4);
        assert_eq!(s, "帳");
        assert!(cut);
        assert!(s.is_char_boundary(s.len()));

        let (s, cut) = truncate_utf8("帳單", 0);
        assert_eq!((s, cut), ("", true));
    }
}
