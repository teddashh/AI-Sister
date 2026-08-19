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
//!
//! ## 那張截圖的下一題
//!
//! 「剛剛發生什麼事」修好之後，下一句自然是**「剛剛那個優惠方案」**——同時
//! 帶著時間和內容。這種問題走的是關鍵字那條路（句子裡有他真正想問的東西），
//! 而那條路上藏著同一個形狀錯誤：拿去比對的是**整句話**。中文沒有空白，
//! 所以 FTS 那邊會把「剛剛那個優惠方案」當成一整串子字串去找，而沒有人的
//! 螢幕上會出現這八個字。實測（`scenarios/bill-lookup.json`）：
//!
//! | 問題 | 修之前 |
//! |---|---|
//! | 優惠方案 | 1 筆原文 |
//! | 剛剛那個優惠方案 | **0 筆** |
//! | 繳費期限 | 3 筆答案 + 1 筆原文 |
//! | 剛剛看到的期限 | 2 筆答案 + **0 筆原文** |
//!
//! [`terms`] 就是那個修法：**把頭尾的時間詞和虛字剝掉，中間原樣留著。**
//! 只剝頭尾是刻意的——中文問句把時間放句首（「剛剛…」）、把疑問詞放句尾
//! （「…是什麼」），內容夾在中間而且是連著的，剝完還是一段完整的子字串。
//! 如果連中間的虛字也一起挑掉，「剛剛 chrome 上那個網址」會碎成
//! `chrome` AND `上` AND `網址` 三段，比原本更找不到。

/// 一個問題該用什麼方式回答。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shape {
    /// 拿字去比對她記下來的字。
    Keywords,
    /// 他問的是「最近」。答案來自時間，不是來自比對。
    Recent,
}

impl Shape {
    /// 寫出去的時候叫什麼。
    ///
    /// 只有這一個地方定義：`sister query --json` 的 `shape` 欄位、字母人的
    /// `Answer.kind`、題庫的 `queries.shape` 是同一組字串。各寫各的 `match`
    /// 遲早會有一邊改了字，而分岔之後，Phase 2 拿題庫回頭統計「時間問題到底
    /// 答得好不好」的時候會靜靜地少算一半。
    pub fn name(self) -> &'static str {
        match self {
            Shape::Keywords => "keywords",
            Shape::Recent => "recent",
        }
    }
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
    "這", "耶", "呀", "個",
];
const FILLER_ASCII: &[&str] = &[
    "what", "happened", "happen", "was", "were", "i", "im", "doing", "did", "do", "the", "a", "on",
    "up", "to", "am", "you", "see", "saw", "screen", "my", "me", "s", "here", "going", "went",
    "been",
];

/// 一個詞在這句話裡扮演什麼角色。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Role {
    /// 「剛剛」。指向時間。
    Recent,
    /// 虛字。拿掉不改變他在問什麼。
    Filler,
    /// 認不得——那多半就是他真正想問的東西。
    Content,
}

/// 把問題切成一個一個詞，標點和空白直接丟掉。
///
/// [`shape`] 和 [`terms`] 都走這一支。兩邊各切一次的話，遲早會有一邊多認得
/// 一個詞，然後同一句話在「要不要當成時間問題」和「拿哪幾個字去比對」上給出
/// 對不起來的答案。這根釘子這個 repo 已經踩過五次了（見 `crate::consent`）。
fn words(question: &str) -> Vec<(usize, usize, Role)> {
    let mut out = Vec::new();
    let mut i = 0;

    while i < question.len() {
        let rest = &question[i..];
        let c = rest.chars().next().expect("not empty");

        // 標點和空白直接跳過。「剛剛發生什麼事？」和沒有問號的那句是同一句。
        if c.is_whitespace() || (c.is_ascii_punctuation() || is_cjk_punct(c)) {
            i += c.len_utf8();
            continue;
        }

        // 英數字整個詞一起看，否則 `justify` 會被讀成 `just`——那是一個把
        // 「查一個變數名」變成「講最近發生的事」的誤判。
        if c.is_ascii_alphanumeric() {
            let end = rest
                .find(|ch: char| !ch.is_ascii_alphanumeric())
                .unwrap_or(rest.len());
            let word = rest[..end].to_ascii_lowercase();
            let role = if RECENT_ASCII.contains(&word.as_str()) {
                Role::Recent
            } else if FILLER_ASCII.contains(&word.as_str()) {
                Role::Filler
            } else {
                Role::Content
            };
            out.push((i, i + end, role));
            i += end;
            continue;
        }

        // 中文：由長到短試，「剛剛」要贏過「剛」。
        match longest(rest) {
            Some((len, recent)) => {
                out.push((i, i + len, if recent { Role::Recent } else { Role::Filler }));
                i += len;
            }
            // 認不得的中文一個字一個字往前走。連著的幾個字在 `terms` 那邊會
            // 自己接回一段——中文的內容詞本來就是連著的。
            None => {
                out.push((i, i + c.len_utf8(), Role::Content));
                i += c.len_utf8();
            }
        }
    }

    out
}

/// 他在問什麼形狀的問題。
pub fn shape(question: &str) -> Shape {
    let words = words(question);
    // 只要句子裡還剩下任何一個講得出內容的詞，就走關鍵字——誤判的代價不對稱，
    // 見模組開頭。
    if words.iter().any(|&(_, _, r)| r == Role::Content) {
        return Shape::Keywords;
    }
    if words.iter().any(|&(_, _, r)| r == Role::Recent) {
        Shape::Recent
    } else {
        Shape::Keywords
    }
}

/// 這句話裡真正該拿去比對螢幕的是哪一段：**從第一個內容詞到最後一個內容詞。**
///
/// 頭尾的時間詞和虛字被剝掉，中間原樣留著（含中間的虛字和標點）——理由見模組
/// 開頭那一節。回傳的是原字串的一段，不是重組出來的新字串：重組會在詞與詞之間
/// 塞進空白，而空白正是 [`crate::db::fts_query`] 用來斷詞的東西，等於把一段
/// 連續的中文切成好幾個必須同時出現的條件。
///
/// 一個內容詞都不剩的時候回**原句**，不是空字串。那多半是「發生什麼事」這種
/// 整句都是虛字的問法，而她能做的最不意外的事就是照著他打的字去找——和這個
/// 模組其他地方一樣，看不懂就退回今天的行為。
///
/// ## 剝到只剩一個中文字，就是剝過頭了
///
/// 虛字清單裡有一堆**單字**（「事」「看」「到」「個」「有」），而中文的雙字詞
/// 常常正好由一個虛字加一個實字組成。於是「事件」會被剝成「件」、「看板」剝成
/// 「板」、「個資」剝成「資」——每一個都是把他問的東西換成另一個東西。
///
/// 而且那不只是精確度變差：一個中文字在這個 schema 底下**沒有索引可用**
/// （trigram 要 3 個字、bigram 要 2 個字，見 [`crate::db`] 開頭），所以它會
/// 一路掉到全表掃描，然後掃回一堆不相干的東西。一個字的查詢不是查詢，是掃描。
///
/// 所以剝完不足兩個字的時候，把邊界往回退——**先退左邊**：中文的修飾語在前、
/// 中心語在後，緊鄰左邊那個字比較可能和它是同一個詞。「剛剛那個事件」會停在
/// 「事件」，不會退回整句。
pub fn terms(question: &str) -> &str {
    terms_with_retreat(question).0
}

/// 同上，外加**這次有沒有退過邊界**。
///
/// 退邊界是這個函式唯一一種「她拿去比對的字不是一個詞」的來源。上面那段文件
/// 講的是它救回「事件」「看板」「個資」的那一面；它的另一面是同一個動作也會
/// 黏出不是詞的東西：
///
/// ```text
/// 剛剛那個板     → 個板
/// 剛剛看到的人   → 的人
/// 剛剛那個錢     → 個錢
/// ```
///
/// 光看字串分不出這兩種——「個資」和「個板」長得一樣，要有字典才知道前者是
/// 詞。但**退過**這件事本身是知道的，而它正好圈住了全部的風險：沒退過的時候
/// 邊界兩端都是實字，她比對的就是他打的那幾個字。
///
/// 所以不去猜哪一次退對了，只把退過這件事講出來，讓他自己看一眼。兩種完全
/// 不同的處境本來會印出同一句「沒有找到」：他打的字真的沒出現過，跟她根本
/// 沒找他打的字——而後者他只要把那個詞重打一次就好。
pub fn terms_with_retreat(question: &str) -> (&str, bool) {
    /// 剝完至少要留下這麼多個字，不然就退回去。理由見上面。
    const MIN_CHARS: usize = 2;

    let words = words(question);
    let Some(mut lo) = words.iter().position(|&(_, _, r)| r == Role::Content) else {
        return (question.trim(), false);
    };
    let mut hi = words
        .iter()
        .rposition(|&(_, _, r)| r == Role::Content)
        .expect("有第一個就有最後一個");

    let span = |lo: usize, hi: usize| &question[words[lo].0..words[hi].1];
    let mut retreated = false;
    while span(lo, hi).chars().count() < MIN_CHARS {
        if lo > 0 {
            lo -= 1;
            retreated = true;
        } else if hi + 1 < words.len() {
            hi += 1;
            retreated = true;
        } else {
            break; // 整句就這麼短，沒有東西可以退了
        }
    }
    (span(lo, hi), retreated)
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

    /// 這四句是實測出來的（`scenarios/bill-lookup.json`，見模組開頭那張表）。
    /// 右邊那幾個字單獨拿去問答得出來，加上「剛剛」之後就變成零筆。
    #[test]
    fn the_time_word_must_not_end_up_in_what_she_looks_for_on_screen() {
        assert_eq!(terms("剛剛那個優惠方案"), "優惠方案");
        assert_eq!(terms("剛剛看到的期限"), "期限");
        assert_eq!(terms("最近的帳單金額"), "帳單金額");
        assert_eq!(terms("what just happened to the deploy"), "deploy");
    }

    /// 只剝頭尾。中間那些虛字留著，因為剝掉會把一段連續的中文切成好幾個
    /// 必須同時出現的條件——理由見 [`terms`] 的註解。
    #[test]
    fn the_middle_is_left_exactly_as_he_typed_it() {
        assert_eq!(terms("剛剛 chrome 上那個網址"), "chrome 上那個網址");
        assert_eq!(terms("錯誤訊息，還有那個網址"), "錯誤訊息，還有那個網址");
    }

    /// 虛字清單裡有一堆單字，而中文的雙字詞常常正好是一個虛字加一個實字。
    /// 照字面剝的話「事件」會變成「件」——那不只是精確度差一點，一個中文字
    /// 在這個 schema 底下沒有索引可用，會一路掉到全表掃描再撈回一堆不相干
    /// 的東西。剝到不足兩個字就往回退。
    #[test]
    fn stripping_must_not_eat_half_of_a_two_character_word() {
        assert_eq!(terms("事件"), "事件");
        assert_eq!(terms("看板"), "看板");
        assert_eq!(terms("個資"), "個資");
        // 退到夠用就停，不是退回整句——「剛剛」還是不該進去比對。
        assert_eq!(terms("剛剛那個事件"), "事件");
        assert_eq!(terms("剛剛的事件"), "事件");
    }

    /// 整句就只有一個字的時候沒有東西可以退，照原樣送出去。
    #[test]
    fn a_one_character_question_is_left_alone() {
        assert_eq!(terms("板"), "板");
        assert_eq!(terms("的"), "的");
    }

    /// **退邊界救回「事件」的同一個動作，也黏得出「個板」。**
    ///
    /// 光看字串分不出來（「個資」是詞、「個板」不是），但退過這件事本身知道
    /// ——而它正好圈住全部的風險：沒退過就代表兩端都是實字，她比對的就是他
    /// 打的那幾個字。畫面只在退過的時候多講一句。
    #[test]
    fn a_needle_glued_out_of_a_particle_announces_itself() {
        for (q, needle) in [
            ("剛剛那個板", "個板"),
            ("剛剛看到的人", "的人"),
            ("剛剛那個錢", "個錢"),
            // 救回來的那幾個也一樣算退過——她確實不是照他打的字去找的。
            ("剛剛那個事件", "事件"),
            ("個資", "個資"),
        ] {
            let (t, retreated) = terms_with_retreat(q);
            assert_eq!(t, needle, "{q}");
            assert!(retreated, "{q} → {t} 是黏出來的，要說得出口");
        }

        // 沒退過的一律安靜：兩端都是實字，她找的就是他打的。
        for q in [
            "剛剛那個優惠方案",
            "客服專線",
            "ERR_CONNECTION_REFUSED",
            "板",
            "剛剛發生什麼事",
        ] {
            assert!(!terms_with_retreat(q).1, "{q} 沒有黏過東西，不該多講一句");
        }
    }

    /// 一句完全沒有虛字的問題不該被動到一個字。
    #[test]
    fn a_question_made_of_nothing_but_content_comes_back_whole() {
        for q in ["客服專線", "ERR_CONNECTION_REFUSED", "E0308"] {
            assert_eq!(terms(q), q);
        }
    }

    /// 剝到什麼都不剩的時候回原句，不是空字串：她退回今天的行為，照著他打的
    /// 字去找。（`Shape::Recent` 那幾句根本不會走到比對那條路，這裡驗的是
    /// 「發生什麼事」這種沒有時間詞、也沒有內容詞的問法。）
    #[test]
    fn stripping_everything_falls_back_to_what_he_typed() {
        assert_eq!(terms("發生什麼事"), "發生什麼事");
        assert_eq!(terms("剛剛發生什麼事"), "剛剛發生什麼事");
        assert_eq!(terms(""), "");
        assert_eq!(terms("   "), "");
    }

    /// 尾巴的標點跟著虛字一起走，開頭的也是。
    #[test]
    fn punctuation_at_the_edges_does_not_survive() {
        assert_eq!(terms("剛剛那個優惠方案？"), "優惠方案");
        assert_eq!(terms("「客服專線」"), "客服專線");
    }

    /// **剝出來的一定是他打的那句話裡的一段連續子字串。**
    ///
    /// 這一條是整個做法安不安全的關鍵，值得寫成一條測試而不是只寫在註解裡。
    /// FTS 對中文做的是子字串比對，所以 `terms(q)` ⊆ `q` 直接推出：原本比對
    /// 得到的東西，剝完之後一樣比對得到。**剝字只會讓精確度變差（多撈一些
    /// 不相干的），不會讓她找不到本來找得到的東西。**
    ///
    /// 反過來說，哪天有人想在這裡「重組」出更好的查詢字串（換同義詞、補空白、
    /// 加 OR），這條測試會先紅——而那正是該停下來想清楚的時候，因為那一步會
    /// 把「不會變差」這個保證換成一個需要被評測的猜測。
    #[test]
    fn what_she_looks_for_is_always_a_slice_of_what_he_typed() {
        for q in [
            "剛剛那個網址",
            "我剛剛複製的東西",
            "那個錯誤代碼是什麼",
            "上禮拜的會議記錄",
            "我的密碼放哪",
            "剛剛看到的那個人名",
            "有沒有優惠",
            "這個月的帳單",
            "剛剛那個 error",
            "那個 pull request",
            "剛才那封信",
            "身分證字號",
            "剛剛那個 IP",
            "事件",
            "ERR_CONNECTION_REFUSED",
            "",
            "   ",
            "？？？",
        ] {
            let t = terms(q);
            assert!(
                q.contains(t),
                "{q:?} 剝出了一段它自己沒有的字：{t:?}——比對就可能反而變少"
            );
        }
    }

    /// `terms` 不可以改變 `shape` 的答案——兩支走同一個切詞，這一條就是在釘
    /// 「以後有人只改其中一邊」。
    #[test]
    fn the_two_readings_of_a_question_never_disagree() {
        for q in [
            "剛剛發生什麼事",
            "剛剛那個優惠方案",
            "客服",
            "what just happened",
            "justify",
            "",
        ] {
            let stripped = terms(q);
            if shape(q) == Shape::Keywords && stripped != q {
                // 剝完剩下的一定還是關鍵字問題：剝掉的全是虛字和時間詞。
                assert_eq!(shape(stripped), Shape::Keywords, "{q:?} 剝完變了形狀");
            }
        }
    }
}
