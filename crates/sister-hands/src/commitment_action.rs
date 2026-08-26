//! 把承諾卡的 `allowed_next_step` 保守地接到 suggestion 按鈕。
//!
//! 那一欄是自由文字（`db.rs` 的 `commitments` 表），欄位註解寫著「『帶著
//! 上下文接手』的接點：她能替你做的下一步（含權限邊界）」，而在這之前
//! **沒有任何人讀它**。這裡只認一種具名 JSON，讀不懂就說讀不懂——不猜。
//!
//! 住在 `sister-hands` 而不是 `sister-core`：解析一串字不需要認識
//! `CommitmentRow`，而讓記憶那一層依賴行動那一層，會把 SPEC §9 的物理隔離
//! 變成一句文件上的話。

use crate::SuggestionButton;

/// 一張承諾卡的下一步。**三種，不是兩種。**
///
/// 「這張卡沒有下一步」和「有寫但讀不懂」共用一個 `None` 的那一天，
/// 畫面上那顆按鈕會在一張明明寫了東西的卡片上安靜地不出現。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AllowedNextStep {
    /// 欄位是 SQL `NULL` 或整段空白：這張卡真的沒有下一步。
    Missing,
    /// 有字，但不是這裡認得的具名 JSON。原文留著給人看。
    Unparseable { raw: String, reason: String },
    /// 解析出一顆**還沒被按下**的按鈕。它還不是可執行的動作。
    Suggestion(SuggestionButton),
}

pub fn parse_allowed_next_step(raw: Option<&str>) -> AllowedNextStep {
    let Some(raw) = raw else {
        return AllowedNextStep::Missing;
    };
    // 一欄空白字串和一欄 NULL 是同一件事：沒有人寫過下一步。把它送進
    // JSON 解析器只會得到一句「EOF while parsing」，然後畫面上會出現
    // 「這張卡的下一步讀不懂」——而那張卡上根本什麼都沒寫。
    if raw.trim().is_empty() {
        return AllowedNextStep::Missing;
    }
    match SuggestionButton::parse_json(raw) {
        Ok(button) => AllowedNextStep::Suggestion(button),
        Err(error) => AllowedNextStep::Unparseable {
            raw: raw.to_owned(),
            reason: format!("allowed_next_step 解析不出來：{error}"),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allowed_next_step_distinguishes_missing_unparseable_and_parsed() {
        assert!(matches!(
            parse_allowed_next_step(None),
            AllowedNextStep::Missing
        ));
        let bad = parse_allowed_next_step(Some("幫我處理這件事"));
        let AllowedNextStep::Unparseable { raw, reason } = bad else {
            panic!("應回 Unparseable")
        };
        assert!(raw.contains("幫我處理這件事"));
        assert!(reason.contains("解析不出來"));
        assert!(!reason.contains("沒有下一步"));

        let parsed = parse_allowed_next_step(Some(
            r#"{"action":"open_url","url":"https://example.com/task/7"}"#,
        ));
        let AllowedNextStep::Suggestion(button) = parsed else {
            panic!("應解析成按鈕")
        };
        let description = button.describe();
        assert!(description.contains("開啟網址"));
        assert!(description.contains("https://example.com/task/7"));
        assert!(!description.contains("處理這件事"));
    }

    /// 一欄空白字串和一欄 `NULL` 是同一件事：沒有人寫過下一步。
    /// 送進 JSON 解析器的話它會回一句「EOF while parsing」，然後畫面上
    /// 會出現「這張卡的下一步讀不懂」——而那張卡上根本什麼都沒寫。
    #[test]
    fn a_blank_column_is_no_next_step_not_an_unreadable_one() {
        for blank in ["", "   ", "\n", "\t "] {
            assert_eq!(
                parse_allowed_next_step(Some(blank)),
                AllowedNextStep::Missing,
                "{blank:?} 應該算成沒有下一步"
            );
        }
    }
}
