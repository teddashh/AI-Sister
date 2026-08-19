//! 他問的到底是「哪幾個字」，還是「剛剛」。
//!
//! 這個模組存在的理由是一張截圖。字母人第一次跑在真的 Windows 上，第一個被
//! 打進輸入框的問題是**「剛剛發生什麼事」**——而那是這整個產品的招牌問題。
//! 當時的引擎只會做一件事：拿那七個字去比對她記下來的字。於是唯一可能的答案
//! 是「這件事我沒看到過」，除非螢幕上剛好出現過「剛剛發生什麼事」這七個字。
//!
//! 那不是一個 bug，是一個形狀錯誤：「剛剛」是**時間**，不是關鍵字。用比對字
//! 的方式去回答一個問時間的問題，永遠只會答錯，而且會錯得像「她什麼都沒記到」。
//!
//! 判斷刻意做得很膽小：**看不懂就當成關鍵字。** 把關鍵字問題誤判成時間問題，
//! 代價是她無視他打的字、改講最近發生的事——那是一個會讓人不再信任她的錯。
//! 反過來把時間問題當成關鍵字，最差也只是回到今天的行為。

/// 一個問題該用什麼方式回答。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shape {
    /// 拿字去比對她記下來的字。
    Keywords,
    /// 他問的是「最近」。答案來自時間，不是來自比對。
    Recent,
}

/// 「剛剛」。看到其中一個才**可能**是時間問題。
const RECENT_CJK: &[&str] = &["剛剛", "剛才", "方才", "最近", "剛"];
const RECENT_ASCII: &[&str] = &["just", "recently", "lately", "now"];

/// 這些字自己不帶內容，把它們拿掉不會改變他在問什麼。
///
/// 長的排前面沒關係，比對時會自己按長度排。要小心的是**別把有內容的字放進來**
/// ——每多一個，就多一種「他明明問了某件事，她卻改講最近」的可能。
const FILLER_CJK: &[&str] = &[
    "發生", "什麼", "甚麼", "事情", "幹嘛", "幹麼", "一下", "剛好", "我", "你", "她", "他", "在",
    "做", "了", "過", "的", "是", "有", "嗎", "呢", "吧", "啊", "喔", "事", "看", "到", "些", "那",
    "這", "耶", "呀",
];
const FILLER_ASCII: &[&str] = &[
    "what", "happened", "happen", "was", "were", "i", "im", "doing", "did", "do", "the", "a", "on",
    "up", "to", "am", "you", "see", "saw", "screen", "my", "me", "s", "here", "going", "went",
    "been",
];

/// 他在問什麼形狀的問題。
pub fn shape(question: &str) -> Shape {
    let q = question.trim();
    if q.is_empty() {
        return Shape::Keywords;
    }

    let mut saw_recent = false;
    let mut rest = q;

    while !rest.is_empty() {
        let c = rest.chars().next().expect("not empty");

        // 標點和空白直接跳過。「剛剛發生什麼事？」和沒有問號的那句是同一句。
        if c.is_whitespace() || (c.is_ascii_punctuation() || is_cjk_punct(c)) {
            rest = &rest[c.len_utf8()..];
            continue;
        }

        // 英數字整個詞一起看，否則 `justify` 會被讀成 `just`——那是一個把
        // 「查一個變數名」變成「講最近發生的事」的誤判。
        if c.is_ascii_alphanumeric() {
            let end = rest
                .find(|ch: char| !ch.is_ascii_alphanumeric())
                .unwrap_or(rest.len());
            let word = rest[..end].to_ascii_lowercase();
            if RECENT_ASCII.contains(&word.as_str()) {
                saw_recent = true;
            } else if !FILLER_ASCII.contains(&word.as_str()) {
                return Shape::Keywords;
            }
            rest = &rest[end..];
            continue;
        }

        // 中文：由長到短試，「剛剛」要贏過「剛」。
        match longest(rest) {
            Some((len, recent)) => {
                saw_recent |= recent;
                rest = &rest[len..];
            }
            // 一個認不得的字就夠了。那多半就是他真正想問的東西。
            None => return Shape::Keywords,
        }
    }

    if saw_recent {
        Shape::Recent
    } else {
        Shape::Keywords
    }
}

/// 在開頭比對得到的最長那個詞，以及它是不是「剛剛」。
fn longest(rest: &str) -> Option<(usize, bool)> {
    let mut best: Option<(usize, bool)> = None;
    for (list, recent) in [(RECENT_CJK, true), (FILLER_CJK, false)] {
        for token in list {
            if rest.starts_with(token) && best.is_none_or(|(len, _)| token.len() > len) {
                best = Some((token.len(), recent));
            }
        }
    }
    best
}

fn is_cjk_punct(c: char) -> bool {
    matches!(
        c,
        '，' | '。' | '？' | '！' | '、' | '；' | '：' | '「' | '」' | '（' | '）' | '…' | '　'
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 這一句就是那張截圖。它非過不可。
    #[test]
    fn the_question_from_the_screenshot() {
        assert_eq!(shape("剛剛發生什麼事"), Shape::Recent);
    }

    #[test]
    fn the_ways_people_actually_ask_about_just_now() {
        for q in [
            "剛剛發生什麼事",
            "剛剛發生什麼事？",
            "我剛剛在做什麼",
            "剛剛在幹嘛",
            "剛才我看到什麼",
            "最近發生什麼事",
            "剛剛做了什麼",
            "what just happened",
            "what did i just do",
            "what was i doing just now",
        ] {
            assert_eq!(shape(q), Shape::Recent, "{q:?} 問的是時間");
        }
    }

    /// 誤判的代價不對稱：把有內容的問題當成「剛剛」，她會無視他打的字。
    /// 所以只要句子裡還剩下任何一個講得出內容的詞，就走關鍵字。
    #[test]
    fn a_question_with_anything_left_in_it_is_still_a_search() {
        for q in [
            "剛剛那個電話號碼",
            "剛剛的錯誤訊息",
            "最近的帳單金額",
            "剛剛 chrome 上那個網址",
            "客服",
            "what just happened to the deploy",
            "ERR_CONNECTION_REFUSED",
        ] {
            assert_eq!(shape(q), Shape::Keywords, "{q:?} 有他真正想問的東西");
        }
    }

    /// 沒有「剛剛」就不是時間問題，就算整句都是虛字也一樣。
    #[test]
    fn filler_alone_is_not_a_time_question() {
        for q in ["什麼", "發生什麼事", "我在做什麼", "what happened", ""] {
            assert_eq!(shape(q), Shape::Keywords, "{q:?} 沒有指向時間");
        }
    }

    /// `just` 不能從 `justify` 裡被讀出來。整個詞比對，不是子字串。
    #[test]
    fn an_english_word_is_matched_whole_or_not_at_all() {
        assert_eq!(shape("justify"), Shape::Keywords);
        assert_eq!(shape("nowhere"), Shape::Keywords);
        assert_eq!(shape("recently"), Shape::Recent);
    }

    /// 「剛剛」要贏過「剛」，否則剩下的那個「剛」會變成認不得的字。
    #[test]
    fn the_longer_token_wins() {
        assert_eq!(shape("剛剛"), Shape::Recent);
        assert_eq!(shape("剛才"), Shape::Recent);
    }
}
