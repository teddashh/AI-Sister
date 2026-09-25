//! 產品與 replay harness 共用的檢索接線。
//!
//! `Db::search` 是文字索引（FTS trigram/unicode61/bigram，必要時 LIKE fallback）；
//! [`crate::answer::answers`] 是 L1 facts。這裡用型別選擇要不要接上後者，讓 CLI、
//! 字母人與評測不必各手抄一次「時間題不能跑 facts」那組分支。

use anyhow::{Result, ensure};

use crate::activity::Activity;
use crate::answer::{Answer, Answers, answers_during};
use crate::db::{Db, IndexedCandidate};
use crate::model::{Millis, SearchHit};
use crate::question::{self, Shape};

/// 呼叫端保留原因與實際比對字，不得由原問句重算。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(tag = "kind", content = "terms", rename_all = "snake_case")]
pub enum SearchAdjustment {
    Glued(String),
    Relaxed(String),
    /// 放寬過、他問的是「什麼時候」（[`crate::facts::WHEN_ASKS`]），而且放寬之後
    /// 底下有東西。改找的字裡已經沒有那幾個字了，底下的時間只剩她記下每一筆的
    /// 時刻，所以要講明。放寬之後還是空手的話用 [`Self::Relaxed`]。
    RelaxedWhen(String),
}

impl SearchAdjustment {
    pub fn message(&self) -> String {
        match self {
            Self::Glued(x) => format!(
                "我拿去比對的是「{x}」——那是從你打的字黏出來的，不是一個詞。直接打你要的那個詞再問一次。"
            ),
            Self::Relaxed(x) => format!("我對不到你打的那一串，所以改用「{x}」去找。"),
            Self::RelaxedWhen(x) => format!(
                "我對不到你打的那一串，所以改用「{x}」去找。底下每一筆的時間，是我記下那一筆的時候。"
            ),
        }
    }
}

/// 第一次查詢整段都空了之後，能不能改拿較短的一段再查一次。
///
/// 第一次拿去比對的字一個都不改。這裡只決定第二次。
///
/// alpha.157 第一輪只問索引：頭尾連續沒看過的條件可以拿掉，看過的就停。
/// 帳單夾具裡「怎麼」「哪裡」「請問」從來沒出現，所以那一輪是綠的。把 repo
/// 的中文文件（約 24 萬字，主題字都排除）當背景，灌進同一套演算法之後，
/// 這些字一定看過，放寬就停在它們前面：
///
/// - `ERR_DEPLOY_42 怎麼回事`：沒有背景改用「ERR_DEPLOY_42」並找到；有背景改用
///   「ERR_DEPLOY_42 怎麼」，0 筆。
/// - `部署失敗怎麼辦`／`部署失敗是什麼原因`：沒有背景找到；有背景空手。
/// - `客服電話怎麼打`：沒有背景找到號碼；有背景改用「客服電話怎麼」，空手。
/// - `月報連結在哪裡`／`告訴我 月報連結`／`電信帳單寄到哪`：沒有背景找到；有背景空手。
/// - `幫我看一下客服專線`：沒有背景找到；有背景空手。
///
/// 主題也有同一個洞：`退款電話怎麼打` 的主題被算成「退款 怎麼打」，「怎麼」看過，
/// 保護放行，改去找「電話怎麼打」。
///
/// 第二輪在問索引之前先走一張封閉的問句詞清單 [`QUESTION_WORDS`]，不靠那些字
/// 剛好沒看過。問句詞是這一張；口語開頭是旁邊另一張 [`SPOKEN_LEAD_INS`]，
/// 各只有一份，不複製到 facts 或索引。問句詞中文整段比對；英文是獨立 token、
/// 不分大小寫，邊界與類型詞同一支 [`crate::facts::kind_word_match_end`]（`show`
/// 裡的 how 不算）。口語開頭的英文另有更嚴的邊界，見 [`ascii_lead_in_end`]。落在類型詞裡面的不算：「多少錢」「繳多少」
/// 「付多少」「欠多少」是答案種類，不是問句。問時間的那幾個是例外，見下一段。
/// 前面已經有內容，就只留前面，後面連類型詞一起丟、不接回去。前面只有空白或虛字，
/// 就只拿掉這個詞再往後看，
/// 所以「為什麼部署失敗」留下「部署失敗」，「怎麼找客服電話」留下「找客服電話」，
/// 再交給索引把「找」拿掉。
///
/// 問時間的說法（[`crate::facts::WHEN_ASKS`]：「什麼時候」「幾點」「何時」、when）
/// 自己也是類型詞，alpha.161 起照樣在這裡切。alpha.157 到 alpha.160 不切，理由是
/// 「什麼時候」是他要的日期，畫面上沒有日期就該空手，`common_words_relax.rs` 有一條
/// 測試釘著這一句。推翻它的理由：錄影機對「什麼時候」本來就有一個答案，就是她記下
/// 它的那一刻。對「多少錢」沒有，截圖的時間不是金額，所以錢那幾個照舊不切。第一次
/// 查詢照樣去找寫在主題旁邊的日期時間，這裡只管那一趟整段空手之後。
///
/// 代價：問的是還沒到的事（「預算表什麼時候交」，而畫面上沒寫日期），找到的是她看到
/// 「預算表」那幾張畫面的時間，不是要交的那一天。所以這種放寬另用
/// [`SearchAdjustment::RelaxedWhen`]，多講一句「底下每一筆的時間，是我記下那一筆的
/// 時候」，不沿用籠統的那一句。`when_questions.rs` 釘著這個代價。
///
/// 刻意不收：單獨的「幾」（「幾號」分不出日期和號碼，還有「幾乎」；「幾點」在
/// [`crate::facts::WHEN_ASKS`]）、嗎／呢／吧（[`question::terms`] 已經剝頭尾虛字）。
///
/// 問句頭尾另外套 [`crate::facts::strip_fact_question_edges`]，和 facts 主題
/// 同一張表。那張表多的是「幫我看一下」「幫我看」「查一下」「找一下」「看一下」。
/// 口語開頭不進那張表。切完再跑一次 [`question::terms`]，把「敗是」「案在」這種
/// 交界虛字剝掉。交界雙字在用過的索引裡常常看過，只靠尾端零命中拿不掉。
///
/// 主題保護改看這段剝完的字，不看原問句：原問句有類型詞，而且剝完之後的主題
/// 在索引裡一個條件都沒看過，就不放寬。「不用改」和「太長沒檢查」仍然不是
/// 「沒看過」。剝完是空的，或最後的候選字和原本的 terms 相同，也不放寬。
///
/// 代價（`relax_peel_pays_the_measured_cost` 量的，帳單那一張畫面）：
///
/// - 問句詞後面的類型詞會一起丟掉。「客服怎麼打電話」改用「客服」。facts 答案
///   0 筆，因為候選裡已經沒有「電話」；原文 1 筆，就是「客服專線 0800-000-123」。
/// - 沒有具體主題時，剩下的普通字會被拿去找，但是兩種走法不一樣。「這個東西是什麼」
///   的「這個」「是什麼」本來就是虛字，第一次查詢就拿「東西」去找：畫面上沒有，
///   就是 0 筆、不說改用了什麼，也不再縮。畫面上有「這個東西在桌上」時，第一次
///   就命中，同樣不放寬。「這個東西怎麼用」的「怎麼」不是虛字，第一次「東西怎麼用」
///   對不到，放寬才改用「東西」，那一筆就找到了。
///
/// 第三輪用同一份 repo 中文文件（約 24 萬字，主題字都排除）當背景，量問句詞
/// 前面的口語。那些字在用過的索引裡一定看過，第二輪又規定前面有內容就留下：
///
/// - `所以為什麼部署失敗` 改用「所以」，5 張不相干的畫面。沒有背景時索引沒看過
///   「所以」，整段 [`IndexedCandidate::NoneSeen`]，連「部署失敗」都不再找。
///   `我想問為什麼部署失敗` 一樣：有背景改用「想問」，沒有背景空手。
/// - `可是怎麼辦` 改用「可是」，4 張。
/// - `怎麼回事`／`這是怎麼回事` 改用「回」。[`question::terms`] 的理由是：一個字的
///   查詢不是查詢，是掃描。
/// - `電話怎麼打` 改用「電話」，任意一支號碼。`應繳金額怎麼算` 改用「應繳金額」，
///   5 筆不相干的金額。`哪個網址` 改用「網址」，5 個任意網址。alpha.155 已經出貨
///   的話是：整句話裡她看過的字只剩「多少錢」這種類型詞，那時候她寧可說不知道，
///   也不會拿另一個數字來湊。
/// - `哪裡有客服電話` 只切掉「哪」，改用「裡有客服電話」，0 筆。
///
/// 口語開頭因此另做一張封閉清單，而且只給這一次放寬用。封閉是因為只收這份背景
/// 裡量到、又會被第二輪留在前面的那些說法；「找」「查」「打」這種索引分得出來的
/// 字不寫進來。只給放寬用，是因為 [`crate::facts::strip_fact_question_edges`]
/// 那張表 facts 主題也在用，加進去第一次查詢的答案就變了。
/// 「到底」「那麼」的第一個字是虛字。先跑 [`question::terms`] 再認開頭，整段
/// 已經不在了，所以每一圈先看原樣開頭；不是口語開頭才跑 terms、剝問句頭尾，
/// 再認一次。「所以我想問為什麼部署失敗」先拿掉「所以」，terms 剝掉「我」，
/// 再拿掉「想問」。
///
/// 「哪」單獨一個字會切在詞中間，所以「哪裡」「哪個」「哪一個」整段收進
/// [`QUESTION_WORDS`]。同一位置本來就取最長。「哪裡有客服電話」拿掉「哪裡」，
/// 最後一次 [`question::terms`] 再剝掉「有」。
///
/// 最後的候選字，不論索引改過（[`IndexedCandidate::Changed`]）還是原樣留著
/// （[`IndexedCandidate::Unchanged`]），都再過兩道出口，過不了就不放寬：
///
/// - 去掉空白後不到兩個字。一個字的查詢不是查詢，是掃描。
/// - 有類型詞、而且沒有主題：[`crate::facts::kinds_for_query`] 非空，並且
///   [`crate::facts::topic_constraint`] 是 `None`。只剩類型詞就寧可說不知道。
///
/// 第三輪自己量到的代價（背景探針：bg=0 是基準語料加「月報連結已更新」，沒有
/// 那 24 萬字；bg=1 再灌進 repo 中文文件）：
///
/// - `應繳金額怎麼算`：沒有那 24 萬字時，上一輪改用「應繳金額」答得出 NT$1,350。
///   這一輪 bg=0 和 bg=1 都是 `searched=None`、answers 空、hits 0。候選字只剩
///   類型詞，不放寬，所以不答那筆 NT$1,350，也不拿背景裡別的金額來湊。
/// - `到底部設定在哪`：「到底」正好是「到底部」的開頭，整段被拿掉。bg=0 剩下的
///   「部設定」沒看過，不放寬；畫面上的 terms 仍是「底部設定在哪」（「到」本來
///   就是虛字），answers 空、hits 0。bg=1 索引把「部」拿掉，改用「設定」，
///   answers 空、hits 5，是不相干的原文。
/// - `誰改了月報連結`：bg=1 改用「改了月報連結」，answers 空、hits 0。
///   「誰」拿掉之後「改了」在那 24 萬字裡看過，整段留著，對不到「月報連結已更新」。
///   bg=0「改了」沒看過，會縮成「月報連結」並命中那一筆，answers 仍是空的。
///   這一支到現在還是這樣；alpha.162 起由下一步 [`joint_retry`] 在「了」切開接住。
fn retry_candidate(db: &Db, query: &str, peeled: &str) -> Result<Option<String>> {
    let original = question::terms(query);
    let candidate = match db.indexed_candidate(peeled)? {
        IndexedCandidate::Changed(candidate) => candidate,
        IndexedCandidate::Unchanged => peeled.to_string(),
        IndexedCandidate::NoneSeen | IndexedCandidate::TooLong => return Ok(None),
    };
    if candidate.is_empty() || candidate == original {
        return Ok(None);
    }
    // `Changed` 和 `Unchanged` 都已經折成 `candidate`，兩道出口只寫一次。
    // 切段那一步（`joint_retry`）也過同一支 `candidate_refused`。
    if candidate_refused(&candidate) {
        return Ok(None);
    }
    Ok(Some(candidate))
}

/// 空手之後要依序找的候選字。
#[derive(Debug, PartialEq, Eq)]
struct RelaxPlan {
    /// [`retry_candidate`]：索引拿掉頭尾沒看過的條件。有的話先找它。
    indexed: Option<String>,
    /// [`joint_retry`]：`indexed` 沒有或空手才找。
    joint: Option<String>,
}

/// 產品路徑和測試都從這裡拿放寬的候選字。`None` 就是這一題不放寬。
fn relax_plan(db: &Db, query: &str) -> Result<Option<RelaxPlan>> {
    let Some(peeled) = relax_base(db, query)? else {
        return Ok(None);
    };
    let indexed = retry_candidate(db, query, peeled)?;
    // 第一次查詢拿去比對的就是這一串（`question::terms_with_retreat` 的第一格）。
    let original = question::terms(query);
    let joint = joint_retry(peeled, &[Some(original), indexed.as_deref()]);
    Ok(Some(RelaxPlan { indexed, joint }))
}

/// 兩步放寬共用的前提：剝完的字，和類型詞保護。`None` 就是這一題不放寬，
/// [`retry_candidate`] 和 [`joint_retry`] 都不跑。
fn relax_base<'q>(db: &Db, query: &'q str) -> Result<Option<&'q str>> {
    // 傳原句，不傳 `question::terms(query)`。「到」「那」是虛字，先跑 terms 會把
    // 「到底」「那麼」吃成「底」「麼」，口語開頭整段對不到。剝完再和它比。
    let peeled = peel_retry_terms(query);
    if peeled.is_empty() {
        return Ok(None);
    }
    if !crate::facts::kinds_for_query(query).is_empty()
        && let Some(topic) = crate::facts::topic_constraint(peeled)
        && db.indexed_candidate(&topic)? == IndexedCandidate::NoneSeen
    {
        return Ok(None);
    }
    Ok(Some(peeled))
}

/// [`joint_retry`] 切段的字。空白和中文標點另外也切。
const JOINTS: &[char] = &['的', '了'];

/// 「的」「了」是詞的一部分、不是接頭的：切開會剩一個字而被丟掉。
const JOINT_INSIDE_WORDS: &[&str] = &["目的", "了解"];

/// [`retry_candidate`] 也空手（或根本沒有候選字）之後的最後一步。
///
/// 那一步整段照原樣對。問句裡的「的」「了」是接頭，畫面上不一定有：
/// 「誰改了月報連結」剝掉「誰」剩「改了月報連結」，畫面寫的是「月報連結已更新」；
/// 「月報的連結」對不到「月報連結」。用過一陣子的索引裡「改了」看過，那一步
/// 不會把它當成沒看過的頭拿掉。
///
/// 這一步把剝完的字在空白、中文標點和「的」「了」切開，每段再剝一次頭尾虛字，
/// 剩下的每一段都是必要條件：「月報 連結」「更新 月報連結」。
///
/// - 不到兩個字的段不要：一個字的條件不是條件，是掃描（和 [`candidate_refused`]
///   同一個理由）。「改了」的「改」只能丟掉。
/// - 兩個字以上的段一律留著，看過沒看過都一樣。沒看過的段和別的段不可能在同一
///   筆字裡，所以「客服 退款 專線」照舊空手；看過的段可能就是他要的那個人或那件事，
///   丟掉就是答另一題（`indexed_noise_remains_a_required_condition`）。
/// - [`JOINT_INSIDE_WORDS`] 裡的詞不切：「出差的目的地」是「出差 目的地」。
///
/// 從剝完的字切，不從上一步的候選字切。上一步會把沒看過的頭切掉一半：「會議的
/// 密碼」在沒看過「會議」的索引上是「議的密碼」，再切就剩「議」一個字被丟掉，
/// 等於整個主題不見。從候選字切的那一版在 58 萬字那份上量過：改用「密碼」，
/// 拿兩筆不相干的字來湊。
///
/// 過 [`candidate_refused`]，和第一次查詢或上一步一樣的字不再找。空手就照舊報
/// 上一步的候選字，這一步不出聲。
///
/// 探針（基準語料加「月報連結已更新」；背景三份：沒有、`common_words_relax.rs`
/// 那 17 句、repo 中文文件 58 萬字且主題字的行排除）：
///
/// - 三份都從 0 筆變成找到那一張：`誰更新了月報連結`（改用「更新 月報連結」）、
///   `月報的連結`、`月報的連結在哪`（「月報 連結」）、`where is the release candidate`
///   和 `when is the release candidate`（「is release candidate」，後者多講記下的時間）。
///   `誰改了月報連結`、`為了部署失敗`、`部署失敗的話怎麼辦` 以前只有 58 萬字那份空手。
/// - 還是空手：58 萬字那份上的 `部署失敗的原因`、`上次看到的月報連結`（「原因」
///   「上次看到」看過，是必要條件）；三份都空手的 `部署失敗的時間`、`部署失敗的期限`
///   （類型詞）、`退款的電話`、`上次看到的連結`。英文 is 仍是條件，`when is
///   ERR_DEPLOY_42` 空手（`when_questions.rs`）。
///
/// 代價：`部署失敗的目的` 改用「部署失敗」。[`question::terms`] 先把句尾的「的」
/// 當虛字剝掉，剩「部署失敗的目」，「目」只剩一個字；另外兩份以前就是這樣答
/// （上一步把沒看過的尾巴拿掉）。「的」「了」在別的詞裡、又不在
/// [`JOINT_INSIDE_WORDS`] 的照切：「受不了部署失敗」去找「受不 部署失敗」。
fn joint_retry(base: &str, tried: &[Option<&str>]) -> Option<String> {
    let inside: Vec<std::ops::Range<usize>> = JOINT_INSIDE_WORDS
        .iter()
        .flat_map(|word| base.match_indices(word))
        .map(|(at, word)| at..at + word.len())
        .collect();
    let mut pieces = Vec::new();
    let mut start = 0;
    for (at, c) in base.char_indices() {
        let joint = JOINTS.contains(&c) && !inside.iter().any(|word| word.contains(&at));
        if joint || c.is_whitespace() || question::is_cjk_punct(c) {
            pieces.push(&base[start..at]);
            start = at + c.len_utf8();
        }
    }
    pieces.push(&base[start..]);
    let candidate = pieces
        .into_iter()
        .map(question::terms)
        .filter(|piece| !question::only_filler(piece) && piece.chars().count() >= 2)
        .collect::<Vec<_>>()
        .join(" ");
    if candidate.is_empty()
        || tried.contains(&Some(candidate.as_str()))
        || candidate_refused(&candidate)
    {
        return None;
    }
    Some(candidate)
}

/// 去掉空白後不到兩個字，或只剩類型詞、沒有主題。
fn candidate_refused(candidate: &str) -> bool {
    let chars = candidate.chars().filter(|c| !c.is_whitespace()).count();
    if chars < 2 {
        return true;
    }
    !crate::facts::kinds_for_query(candidate).is_empty()
        && crate::facts::topic_constraint(candidate).is_none()
}

/// 封閉問句詞。加字之前先看 [`retry_candidate`] 上面為什麼不收。
/// 「哪裡」「哪個」「哪一個」要整段在這張表裡；只留「哪」會切在詞中間。
/// 口語開頭不在這裡，見 [`SPOKEN_LEAD_INS`]。
const QUESTION_WORDS: &[&str] = &[
    "為什麼",
    "为什么",
    "為何",
    "为何",
    "怎麼",
    "怎麽",
    "怎么",
    "怎樣",
    "怎样",
    "如何",
    "什麼",
    "什麽",
    "什么",
    "甚麼",
    "啥",
    "哪裡",
    "哪裏",
    "哪里",
    "哪兒",
    "哪儿",
    "哪邊",
    "哪边",
    "哪個",
    "哪个",
    "哪些",
    "哪一個",
    "哪一个",
    "哪位",
    "哪",
    "誰",
    "谁",
    "多少",
    "what",
    "how",
    "why",
    "where",
    "who",
    "which",
];

/// 口語開頭。只給放寬用，不進 [`crate::facts::strip_fact_question_edges`]。
/// 中文整段、只認開頭、同一位置取最長；英文只認獨立的一個字，不認複數 s、
/// 不認連字號，見 [`ascii_lead_in_end`]。
const SPOKEN_LEAD_INS: &[&str] = &[
    "所以",
    "但是",
    "可是",
    "不過",
    "不过",
    "然後",
    "然后",
    "那麼",
    "那么",
    "而且",
    "還有",
    "还有",
    "另外",
    "如果",
    "到底",
    "究竟",
    "想問",
    "想问",
    "想請問",
    "想请问",
    "問一下",
    "问一下",
    "問你",
    "问你",
    "請教",
    "请教",
    "記得",
    "记得",
    "還記得",
    "还记得",
    "知道",
    "so",
    "but",
    "and",
    "then",
    "also",
];

/// 開頭是口語開頭就拿掉，回傳剩下的那段。不是開頭就 `None`。
fn strip_spoken_lead_in(text: &str) -> Option<&str> {
    let trimmed = text.trim_start();
    if trimmed.is_empty() {
        return None;
    }
    let (lower, orig_at) = crate::facts::lowercase_with_orig_bytes(trimmed);
    let mut best_end: Option<usize> = None;
    for word in SPOKEN_LEAD_INS {
        let end = if word.is_ascii() {
            let Some(end) = ascii_lead_in_end(trimmed, &lower, &orig_at, word) else {
                continue;
            };
            end
        } else if trimmed.starts_with(word) {
            word.len()
        } else {
            continue;
        };
        if end > 0 && best_end.is_none_or(|prev| end > prev) {
            best_end = Some(end);
        }
    }
    best_end.map(|end| &trimmed[end..])
}

/// 英文口語開頭只認一個獨立的字：後面接空白、中文、句讀（`,` `:` `;` `!` `?`）
/// 或整句結束。不借類型詞那支邊界：它認複數 s，會把 `SOS` 當成 so 切掉；
/// 連字號也算它的邊界，`So-net 帳單怎麼繳` 會剩「net 帳單」，在用過的索引上
/// 拿任意幾筆金額來湊。
fn ascii_lead_in_end(trimmed: &str, lower: &str, orig_at: &[usize], word: &str) -> Option<usize> {
    if !lower.starts_with(word) {
        return None;
    }
    let end = orig_at[word.len()];
    match trimmed[end..].chars().next() {
        None => Some(end),
        Some(c)
            if c.is_whitespace() || !c.is_ascii() || matches!(c, ',' | ':' | ';' | '!' | '?') =>
        {
            Some(end)
        }
        Some(_) => None,
    }
}

/// 口語開頭、頭尾問句用語、問句詞、再一次 [`question::terms`]。空字串表示沒有東西可找。
fn peel_retry_terms(terms: &str) -> &str {
    let mut current = terms;
    // 口語開頭至少一個字，拿掉之後一定比這一圈開始時短；長度有下界，所以迴圈一定結束。
    // 沒有變短就停，避免空詞在原地打轉。
    // 先看原樣開頭：`question::terms` 會把虛字「到」「那」從「到底」「那麼」的頭上拿掉。
    loop {
        let before = current;
        if let Some(rest) = strip_spoken_lead_in(current) {
            if rest.len() >= before.len() {
                break;
            }
            current = rest;
            continue;
        }
        let termed = question::terms(current);
        let edged = crate::facts::strip_fact_question_edges(termed);
        let Some(rest) = strip_spoken_lead_in(edged) else {
            current = edged;
            break;
        };
        if rest.len() >= before.len() {
            current = edged;
            break;
        }
        current = rest;
    }
    let cut = cut_question_words(current);
    if question::only_filler(cut) {
        ""
    } else {
        question::terms(cut)
    }
}

/// 由左往右第一個問句詞；落在類型詞裡的跳過，問時間的不跳。前面有內容就留下
/// 前面；前面只有虛字就拿掉這個詞，繼續看後面。
fn cut_question_words(text: &str) -> &str {
    let (lower, orig_at) = crate::facts::lowercase_with_orig_bytes(text);
    let mut search_from = 0;
    loop {
        let Some(found) = find_question_word(&lower[search_from..]) else {
            return text;
        };
        let abs_lo = search_from + found.lo;
        let abs_hi = search_from + found.hi;
        let orig_lo = orig_at[abs_lo];
        let orig_hi = orig_at[abs_hi];
        // 問時間的字自己也是類型詞，所以要先看它：不先看的話，下一行會把它當成
        // 「多少錢」那一種跳過去。
        if !found.asks_when && crate::facts::condition_is_kind_word(text, orig_lo, orig_hi) {
            search_from = abs_hi;
            continue;
        }
        if question::only_filler(&text[..orig_lo]) {
            return cut_question_words(&text[orig_hi..]);
        }
        return &text[..orig_lo];
    }
}

/// [`find_question_word`] 找到的那一個。`lo`／`hi` 是 `lower` 的 byte 範圍。
struct FoundQuestionWord {
    lo: usize,
    hi: usize,
    /// 是 [`crate::facts::WHEN_ASKS`] 裡的字，不是 [`QUESTION_WORDS`] 裡的。
    asks_when: bool,
}

/// `lower` 裡最左邊的問句詞，同一位置取最長的：「什麼時候」贏過「什麼」。
fn find_question_word(lower: &str) -> Option<FoundQuestionWord> {
    let words = QUESTION_WORDS
        .iter()
        .map(|word| (*word, false))
        .chain(crate::facts::WHEN_ASKS.iter().map(|word| (*word, true)));
    let mut best: Option<FoundQuestionWord> = None;
    let mut i = 0;
    while i < lower.len() {
        if best.as_ref().is_some_and(|found| i > found.lo) {
            break;
        }
        for (word, asks_when) in words.clone() {
            let Some(end) = crate::facts::kind_word_match_end(lower, word, i) else {
                continue;
            };
            let replace = best.as_ref().is_none_or(|found| {
                i < found.lo || (i == found.lo && end - i > found.hi - found.lo)
            });
            if replace {
                best = Some(FoundQuestionWord {
                    lo: i,
                    hi: end,
                    asks_when,
                });
            }
        }
        let ch = lower[i..].chars().next().expect("i 在字元邊界");
        i += ch.len_utf8();
    }
    best
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RetrievalProfile {
    /// 文字檢索產品路徑；不是 raw FTS，必要時仍會走正確性用的 LIKE fallback。
    TextOnly,
    /// 文字檢索之前先列出規則抽出的 L1 facts，和目前產品畫面相同。
    TextAndFacts,
    /// 文字 + facts，再加上活動級章節。章節是一個範圍，不是單一時刻。
    TextFactsAndSession,
}

impl RetrievalProfile {
    pub fn name(self) -> &'static str {
        match self {
            Self::TextOnly => "baseline_text",
            Self::TextAndFacts => "facts",
            Self::TextFactsAndSession => "facts_session",
        }
    }

    pub fn wants_facts(self) -> bool {
        matches!(self, Self::TextAndFacts | Self::TextFactsAndSession)
    }

    pub fn wants_session(self) -> bool {
        matches!(self, Self::TextFactsAndSession)
    }

    pub fn retrieve(self, db: &mut Db, question: &str, limit: usize) -> Result<Retrieval> {
        self.retrieve_at(db, question, RetrievalLimits::same(limit), crate::now_ms())
    }

    pub fn retrieve_with_limits(
        self,
        db: &mut Db,
        question: &str,
        limits: RetrievalLimits,
    ) -> Result<Retrieval> {
        self.retrieve_at(db, question, limits, crate::now_ms())
    }

    /// `now` 是解「昨天」用的鐘。產品路徑傳 [`crate::now_ms`]；評測傳語料終點。
    pub fn retrieve_at(
        self,
        db: &mut Db,
        question: &str,
        limits: RetrievalLimits,
        now: Millis,
    ) -> Result<Retrieval> {
        self.retrieve_for_question_at(db, question, question, limits, now)
    }

    /// CLI 可以改寫檢索詞，但原問題指定的日期優先；整題共用同一個時鐘。
    pub fn retrieve_for_question_at(
        self,
        db: &mut Db,
        query: &str,
        question: &str,
        limits: RetrievalLimits,
        now: Millis,
    ) -> Result<Retrieval> {
        ensure!(
            limits.answers > 0 && limits.text > 0,
            "retrieval limits 必須大於 0"
        );
        ensure!(!question.trim().is_empty(), "retrieval question 不可為空");
        ensure!(!query.trim().is_empty(), "retrieval query 不可為空");

        let shape = question::shape(query);
        let range =
            question::time_range(question, now).or_else(|| question::time_range(query, now));
        let mut activities = Vec::new();
        let mut activities_truncated = false;
        if self.wants_session()
            && let Some((_, acts)) = db.chapters_for_question(question, now)?
        {
            activities_truncated = acts.len() > limits.text;
            activities = acts;
            activities.truncate(limits.text);
        }

        let mut searched = None;
        let (terms, answer_set, mut hits) = match shape {
            Shape::Recent | Shape::Range => {
                let hits = match range.as_ref() {
                    Some(range) => db.chunks_in_range(range.from, range.to, limits.text + 1)?,
                    None if shape == Shape::Recent => db.recent(limits.text + 1)?,
                    None => Vec::new(),
                };
                (None, Default::default(), hits)
            }
            Shape::Keywords => {
                let (original, changed) = question::terms_with_retreat(query);
                let mut terms = original.to_string();
                searched = changed.then(|| SearchAdjustment::Glued(terms.clone()));
                let mut answer_set = if self.wants_facts() {
                    answers_during(db, query, limits.answers, range.as_ref())?
                } else {
                    Default::default()
                };
                let mut hits = db.search_during(&terms, limits.text + 1, range.as_ref())?;
                // 保留所有既有命中的答案與排序；facts、原文與章節都空手才放寬。
                if answer_set.items.is_empty()
                    && hits.is_empty()
                    && activities.is_empty()
                    && let Some(plan) = relax_plan(db, query)?
                {
                    let wants_facts = self.wants_facts();
                    let search = |db: &Db, candidate: &str| -> Result<(Answers, Vec<SearchHit>)> {
                        let answers = if wants_facts {
                            answers_during(db, candidate, limits.answers, range.as_ref())?
                        } else {
                            Default::default()
                        };
                        let hits =
                            db.search_indexed_during(candidate, limits.text + 1, range.as_ref())?;
                        Ok((answers, hits))
                    };
                    let has_any = |(answers, hits): &(Answers, Vec<SearchHit>)| {
                        !answers.items.is_empty() || !hits.is_empty()
                    };
                    let mut relaxed = None;
                    if let Some(candidate) = plan.indexed {
                        let result = search(db, &candidate)?;
                        relaxed = Some((candidate, result));
                    }
                    // 上一步找得到就不切段。切段也空手，照舊報上一步的候選字。
                    if !relaxed.as_ref().is_some_and(|(_, result)| has_any(result))
                        && let Some(candidate) = plan.joint
                    {
                        let result = search(db, &candidate)?;
                        if has_any(&result) {
                            relaxed = Some((candidate, result));
                        }
                    }
                    if let Some((candidate, (answers, relaxed_hits))) = relaxed {
                        terms = candidate;
                        answer_set = answers;
                        hits = relaxed_hits;
                        // 候選字裡不會再有問時間的字：`cut_question_words` 碰到就切。
                        // 「底下每一筆的時間」只在底下有東西時講，空手就是籠統那一句。
                        let found = !answer_set.items.is_empty() || !hits.is_empty();
                        searched = Some(if found && crate::facts::asks_when(query) {
                            SearchAdjustment::RelaxedWhen(terms.clone())
                        } else {
                            SearchAdjustment::Relaxed(terms.clone())
                        });
                    }
                }
                (Some(terms), answer_set, hits)
            }
        };

        let hits_truncated = hits.len() > limits.text;
        hits.truncate(limits.text);

        Ok(Retrieval {
            profile: self,
            shape,
            time_range: range,
            terms,
            searched,
            answers: answer_set.items,
            hits,
            activities,
            answers_truncated: answer_set.truncated,
            hits_truncated,
            activities_truncated,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetrievalLimits {
    pub answers: usize,
    pub text: usize,
}

impl RetrievalLimits {
    pub const fn new(answers: usize, text: usize) -> Self {
        Self { answers, text }
    }

    pub const fn same(limit: usize) -> Self {
        Self::new(limit, limit)
    }
}

#[derive(Debug, Clone)]
pub struct Retrieval {
    pub profile: RetrievalProfile,
    pub shape: Shape,
    /// 真正交給資料庫的時間窗；空結果的掃描說明也要使用同一個範圍。
    pub time_range: Option<question::TimeRange>,
    /// `None` 代表時間題，沒有拿任何字去比對。
    pub terms: Option<String>,
    /// 退格或空結果後的索引放寬，實際比對的字。呼叫端不可從原問句重算。
    pub searched: Option<SearchAdjustment>,
    pub answers: Vec<Answer>,
    pub hits: Vec<SearchHit>,
    /// 活動級章節。只有 [`RetrievalProfile::TextFactsAndSession`] 會填。
    /// 沒認到時間範圍是空的（沒去算）；認到但切不出來也是空的——兩者靠
    /// [`crate::db::Db::chapters_for_question`] 的 `Option` 在呼叫端分開。
    pub activities: Vec<Activity>,
    pub answers_truncated: bool,
    pub hits_truncated: bool,
    pub activities_truncated: bool,
}

impl Retrieval {
    pub fn truncated(&self) -> bool {
        self.answers_truncated || self.hits_truncated || self.activities_truncated
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{FocusSnapshot, FrameCapture, OcrBlock};

    /// 空手之後會依序拿去找的每一串字，兩步都算。產品路徑用的是同一個
    /// [`relax_plan`]，這裡只是把它攤平。
    fn tries(db: &Db, query: &str) -> Vec<String> {
        relax_plan(db, query)
            .unwrap()
            .map(|plan| plan.indexed.into_iter().chain(plan.joint).collect())
            .unwrap_or_default()
    }

    /// 切段只看字，不問資料庫。
    #[test]
    fn a_joint_keeps_every_piece_of_two_or_more_and_drops_single_characters() {
        for (base, want) in [
            ("改了月報連結", "月報連結"),
            ("更新了月報連結", "更新 月報連結"),
            ("月報的連結", "月報 連結"),
            ("客服，退款的專線", "客服 退款 專線"),
            ("is the release candidate", "is release candidate"),
            // `question::terms` 已經把「目的」句尾的「的」剝掉了，「目」只剩一個字。
            ("部署失敗的目", "部署失敗"),
            // 「的」「了」在別的詞裡的照切。
            ("受不了部署失敗", "受不 部署失敗"),
        ] {
            assert_eq!(joint_retry(base, &[]).as_deref(), Some(want), "{base}");
        }
    }

    #[test]
    fn a_word_that_holds_a_joint_is_not_split() {
        assert_eq!(
            joint_retry("出差的目的地", &[]).as_deref(),
            Some("出差 目的地")
        );
        assert_eq!(
            joint_retry("我想了解部署", &[]).as_deref(),
            Some("想了解部署")
        );
    }

    #[test]
    fn a_joint_never_retries_what_was_tried_or_what_is_refused() {
        // 和第一次查詢或上一步一樣。
        assert_eq!(
            joint_retry("客服 退款 專線", &[Some("客服 退款 專線")]),
            None
        );
        assert_eq!(joint_retry("月報的連結", &[None, Some("月報 連結")]), None);
        // 只剩類型詞。
        assert_eq!(joint_retry("我的電話", &[]), None);
        // 只剩一個字、或全是虛字。
        assert_eq!(joint_retry("改了", &[]), None);
        assert_eq!(joint_retry("是的 the", &[]), None);
    }

    /// 呼叫端把第一次查詢那一串也算進「找過」：切出來和原句一樣就不再找。
    #[test]
    fn a_joint_equal_to_the_first_query_is_not_tried() {
        let db = db_with_bill();
        assert!(
            !db.indexed_term_exists("退款").unwrap(),
            "前提：沒看過「退款」"
        );
        assert_eq!(tries(&db, "客服 退款 專線"), Vec::<String>::new());
    }

    /// 上一步已經找過一樣的字（而且空手，才會輪到切段），不再找一次。
    #[test]
    fn a_joint_equal_to_the_previous_candidate_is_not_tried_twice() {
        let db = db_with_bill();
        assert!(
            !db.indexed_term_exists("改了").unwrap(),
            "前提：沒看過「改了」，上一步才會拿掉它"
        );
        assert_eq!(tries(&db, "誰改了客服專線"), vec!["客服專線".to_string()]);
    }

    /// 桌面收到的就是這個形狀：`kind` 決定畫哪一句，`terms` 是實際拿去找的字。
    /// 多一種就要在 `app.js` 多一句，`check-pet-says-why.mjs` 會逐種對字。
    #[test]
    fn every_adjustment_serializes_as_kind_and_terms() {
        for (adjustment, kind) in [
            (SearchAdjustment::Glued("個板".into()), "glued"),
            (SearchAdjustment::Relaxed("客服電話".into()), "relaxed"),
            (
                SearchAdjustment::RelaxedWhen("ERR_DEPLOY_42".into()),
                "relaxed_when",
            ),
        ] {
            let terms = match &adjustment {
                SearchAdjustment::Glued(x)
                | SearchAdjustment::Relaxed(x)
                | SearchAdjustment::RelaxedWhen(x) => x.clone(),
            };
            assert_eq!(
                serde_json::to_value(&adjustment).unwrap(),
                serde_json::json!({"kind": kind, "terms": terms})
            );
        }
    }

    #[test]
    fn relaxed_when_says_what_relaxed_says_and_then_which_time() {
        let plain = SearchAdjustment::Relaxed("ERR_DEPLOY_42".into()).message();
        let when = SearchAdjustment::RelaxedWhen("ERR_DEPLOY_42".into()).message();
        assert_eq!(
            when,
            format!("{plain}底下每一筆的時間，是我記下那一筆的時候。")
        );
    }

    fn db_with_bill() -> Db {
        let mut db = Db::open_in_memory().expect("db");
        let session = db.start_session("test", "test").expect("session");
        db.insert_frame(
            session,
            &FrameCapture {
                assistive: Vec::new(),
                ts: 100,
                monitor: 0,
                width: 800,
                height: 600,
                dhash: 1,
                image: None,
                image_ext: "png",
                ocr: vec![OcrBlock {
                    text: "客服專線 0800-000-123".into(),
                    x: 0,
                    y: 0,
                    w: 300,
                    h: 20,
                    confidence: 1.0,
                }],
                focus: FocusSnapshot::default(),
            },
            None,
            0,
        )
        .expect("frame");
        db
    }

    fn generated_noise(db: &Db, count: usize) -> Vec<String> {
        let mut out = Vec::new();
        for code in 0x4e00..0x9fff {
            let a = char::from_u32(code).unwrap();
            let b = char::from_u32(code + 1).unwrap();
            let noise = format!("{a}{b}{a}");
            if question::terms(&noise) == noise
                && !db.indexed_term_exists(&format!("{a}{b}")).unwrap()
                && !db.indexed_term_exists(&format!("{b}{a}")).unwrap()
                && !db.indexed_term_exists(&format!("{a}客")).unwrap()
            {
                out.push(noise);
                if out.len() == count {
                    return out;
                }
            }
        }
        panic!("沒有產生足夠的零命中雜訊");
    }

    #[test]
    fn generated_zero_gram_prefixes_preserve_evidence() {
        let corpus: crate::replay::Corpus = serde_json::from_str(include_str!(
            "../../../scenarios/recall-baseline.corpus.json"
        ))
        .unwrap();
        let mut db = Db::open_in_memory().unwrap();
        db.import_replay(&corpus, 100).unwrap();
        let noises = generated_noise(&db, 32);
        assert_eq!(noises.len(), 32);
        for noise in noises {
            for base in ["客服電話", "ERR_DEPLOY_42"] {
                let original = RetrievalProfile::TextAndFacts
                    .retrieve(&mut db, base, 5)
                    .unwrap();
                assert!(!original.answers.is_empty() || !original.hits.is_empty());
                let query = if base == "客服電話" {
                    format!("{noise}{base}")
                } else {
                    format!("{noise} {base}")
                };
                let found = RetrievalProfile::TextAndFacts
                    .retrieve(&mut db, &query, 5)
                    .unwrap();
                assert_eq!(
                    format!("{:?}", found.answers),
                    format!("{:?}", original.answers),
                    "{query}"
                );
                assert_eq!(
                    format!("{:?}", found.hits),
                    format!("{:?}", original.hits),
                    "{query}"
                );
                assert_eq!(
                    found.searched,
                    Some(SearchAdjustment::Relaxed(base.into())),
                    "{query}"
                );
            }
        }
    }

    #[test]
    fn indexed_noise_remains_a_required_condition() {
        let mut db = db_with_bill();
        let noise = generated_noise(&db, 1).pop().unwrap();
        let privacy = crate::config::PrivacyConfig {
            remember_told: true,
            ..Default::default()
        };
        db.remember_told(&privacy, 200, &noise).unwrap();
        assert!(db.indexed_term_exists(&noise).unwrap());
        for query in [format!("{noise} 客服電話"), format!("{noise} 客服專線")] {
            let result = RetrievalProfile::TextAndFacts
                .retrieve(&mut db, &query, 5)
                .unwrap();
            assert!(
                result.answers.is_empty() && result.hits.is_empty(),
                "{query}"
            );
            assert_eq!(result.searched, None, "存在的條件不可丟掉");
        }
    }

    #[test]
    fn all_zero_topic_never_becomes_an_unconstrained_fact_query() {
        let mut db = db_with_bill();
        for noise in generated_noise(&db, 8) {
            let query = format!("{noise}電話");
            let result = RetrievalProfile::TextAndFacts
                .retrieve(&mut db, &query, 5)
                .unwrap();
            assert!(
                result.answers.is_empty() && result.hits.is_empty(),
                "{query}"
            );
            assert_eq!(result.searched, None);
        }
    }

    #[test]
    fn phrasing_never_replaces_an_existing_session_answer() {
        let corpus: crate::replay::Corpus = serde_json::from_str(include_str!(
            "../../../scenarios/recall-baseline.corpus.json"
        ))
        .unwrap();
        let mut db = Db::open_in_memory().unwrap();
        let now = crate::now_ms();
        let noon = question::time_range("今天", now).unwrap().from + 13 * 60 * 60 * 1000;
        db.import_replay(&corpus, noon).unwrap();
        let query = "今天找客服電話";
        let original = db
            .chapters_for_question(query, noon + corpus.duration_ms)
            .unwrap()
            .unwrap()
            .1;
        assert!(!original.is_empty(), "對照組必須真的有章節答案");
        let result = RetrievalProfile::TextFactsAndSession
            .retrieve_at(
                &mut db,
                query,
                RetrievalLimits::same(5),
                noon + corpus.duration_ms,
            )
            .unwrap();
        assert!(
            result.answers.is_empty(),
            "既有章節已能回答，不可加入放寬後排在前面的 facts"
        );
        assert_eq!(
            format!("{:?}", result.activities),
            format!("{original:?}"),
            "既有章節內容和排名不能變"
        );
        assert_eq!(result.searched, None, "沿用原查詢的章節不宣告改字");
    }

    #[test]
    fn spoken_prefix_must_not_remain_a_required_fact_topic() {
        let mut db = db_with_bill();
        assert_eq!(question::shape("找客服電話"), Shape::Keywords);
        assert_eq!(question::terms("找客服電話"), "找客服電話");
        assert_eq!(
            crate::facts::topic_constraint("找客服電話").as_deref(),
            Some("找客服")
        );
        assert_eq!(
            crate::facts::topic_constraint("幫我找客服電話").as_deref(),
            Some("客服")
        );
        assert_eq!(
            crate::db::fts_query(question::terms("查 ERR_DEPLOY_42")),
            "\"查\" AND \"ERR_DEPLOY_42\""
        );
        let result = RetrievalProfile::TextAndFacts
            .retrieve(&mut db, "找客服電話", 5)
            .unwrap();
        assert_eq!(
            result.answers.len(),
            1,
            "口語前綴『找』殘留在 facts 必要主題『找客服』；原查詢空手後應以『客服電話』重查"
        );
    }

    #[test]
    fn phrasing_reports_actual_terms_even_when_the_second_lookup_is_empty() {
        let mut db = db_with_bill();
        for query in [
            "找客服電話",
            "查客服電話",
            "打客服電話",
            "我打客服電話",
            "我要打客服電話",
        ] {
            let result = RetrievalProfile::TextAndFacts
                .retrieve(&mut db, query, 5)
                .unwrap();
            assert_eq!(
                result.answers[0].latest.raw, "0800-000-123",
                "{query}: 放寬後仍查同一支電話"
            );
            assert_eq!(
                result.searched,
                Some(SearchAdjustment::Relaxed("客服電話".into())),
                "{query}: 要呈現實際放寬的字"
            );
        }
        for query in [
            "查 ERR_DEPLOY_42",
            "查一下 ERR_DEPLOY_42",
            "幫我找 ERR_DEPLOY_42",
        ] {
            let result = RetrievalProfile::TextAndFacts
                .retrieve(&mut db, query, 5)
                .unwrap();
            assert!(
                result.hits.is_empty() && result.answers.is_empty(),
                "fixture 沒有錯誤碼，不可拿電話作答"
            );
            assert_eq!(result.searched, None, "全部零筆不重試");
        }
    }

    #[test]
    fn phrasing_never_replaces_existing_facts_or_text() {
        let mut db = db_with_bill();
        for query in ["客服電話", "幫我找客服電話"] {
            let result = RetrievalProfile::TextAndFacts
                .retrieve(&mut db, query, 5)
                .unwrap();
            let original = answers_during(&db, query, 5, None).unwrap();
            assert_eq!(
                format!("{:?}", result.answers),
                format!("{:?}", original.items),
                "{query}: 既有 facts 不可替換、重排或改出處"
            );
            assert_eq!(
                result.terms.as_deref(),
                Some(query),
                "{query}: 已命中就不改查詢"
            );
            assert_eq!(result.searched, None, "{query}: 沿用原字不多說一句");
        }
        let privacy = crate::config::PrivacyConfig {
            remember_told: true,
            ..Default::default()
        };
        db.remember_told(&privacy, 200, "找客服電話").unwrap();
        db.remember_told(&privacy, 300, "客服電話").unwrap();
        let original = db.search("找客服電話", 6).unwrap();
        for profile in [RetrievalProfile::TextOnly, RetrievalProfile::TextAndFacts] {
            let result = profile.retrieve(&mut db, "找客服電話", 5).unwrap();
            assert_eq!(
                format!("{:?}", result.hits),
                format!("{original:?}"),
                "原文已命中不可放寬，否則會帶入較新的別筆文字"
            );
            assert!(
                result.answers.is_empty(),
                "不能替原有文字結果增加放寬的 facts"
            );
            assert_eq!(result.searched, None, "原文命中不宣告改字");
        }
    }

    #[test]
    fn phrasing_keeps_the_original_calendar_window() {
        let mut db = db_with_bill();
        let result = RetrievalProfile::TextAndFacts
            .retrieve_at(&mut db, "昨天查客服電話", RetrievalLimits::same(5), 200)
            .unwrap();
        assert!(
            result.answers.is_empty() && result.hits.is_empty(),
            "昨天不能拿到今天的電話"
        );
        assert_eq!(
            result.searched,
            Some(SearchAdjustment::Relaxed("客服電話".into())),
            "帶日期的原查詢空手後也有實際比對字"
        );
    }

    #[test]
    fn facts_add_the_synonym_answer_without_changing_the_text_baseline() {
        let mut db = db_with_bill();
        let text = RetrievalProfile::TextOnly
            .retrieve(&mut db, "電話是什麼", 5)
            .expect("text");
        let facts = RetrievalProfile::TextAndFacts
            .retrieve(&mut db, "電話是什麼", 5)
            .expect("facts");
        assert!(text.answers.is_empty());
        assert!(text.hits.is_empty(), "螢幕上沒有『電話』兩字");
        assert_eq!(facts.answers.len(), 1);
        assert_eq!(facts.answers[0].latest.raw, "0800-000-123");
    }

    #[test]
    fn recent_questions_are_the_same_in_both_profiles() {
        let mut db = db_with_bill();
        let text = RetrievalProfile::TextOnly
            .retrieve(&mut db, "剛剛發生什麼事", 5)
            .expect("text");
        let facts = RetrievalProfile::TextAndFacts
            .retrieve(&mut db, "剛剛發生什麼事", 5)
            .expect("facts");
        assert_eq!(text.shape, Shape::Recent);
        assert!(text.answers.is_empty() && facts.answers.is_empty());
        assert_eq!(
            text.hits.iter().map(|hit| hit.chunk_id).collect::<Vec<_>>(),
            facts
                .hits
                .iter()
                .map(|hit| hit.chunk_id)
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn calendar_questions_list_the_window_instead_of_searching_time_words() {
        use crate::model::{FocusSnapshot, FrameCapture, OcrBlock};
        use chrono::{Local, TimeZone};

        let now = Local
            .with_ymd_and_hms(2026, 8, 26, 15, 30, 0)
            .single()
            .expect("local")
            .timestamp_millis();
        let yesterday_15 = Local
            .with_ymd_and_hms(2026, 8, 25, 15, 0, 0)
            .single()
            .expect("local")
            .timestamp_millis();

        let mut db = Db::open_in_memory().expect("db");
        let session = db.start_session("test", "test").expect("session");
        db.insert_frame(
            session,
            &FrameCapture {
                assistive: Vec::new(),
                ts: yesterday_15,
                monitor: 0,
                width: 800,
                height: 600,
                dhash: 1,
                image: None,
                image_ext: "png",
                ocr: vec![OcrBlock {
                    text: "SQLite user_version".into(),
                    x: 0,
                    y: 0,
                    w: 300,
                    h: 20,
                    confidence: 1.0,
                }],
                focus: FocusSnapshot {
                    app_id: Some("chrome.exe".into()),
                    window_title: Some("SQLite 文件".into()),
                    ..FocusSnapshot::default()
                },
            },
            None,
            0,
        )
        .expect("frame");

        let got = RetrievalProfile::TextOnly
            .retrieve_at(&mut db, "我昨天下午在弄什麼", RetrievalLimits::same(5), now)
            .expect("range");
        assert_eq!(got.shape, Shape::Range);
        assert!(got.terms.is_none(), "日曆題沒有拿字去比對");
        assert_eq!(got.hits.len(), 1, "那段時間的原文要列得出來");
        assert!(got.hits[0].text.contains("SQLite"));
    }

    #[test]
    fn dated_keywords_select_yesterdays_evidence_before_limiting_results() {
        use chrono::{Local, TimeZone};

        let now = Local
            .with_ymd_and_hms(2026, 9, 20, 15, 0, 0)
            .single()
            .expect("local")
            .timestamp_millis();
        let range = question::time_range("昨天", now).expect("yesterday");
        let mut db = Db::open_in_memory().expect("db");
        let session = db.start_session("test", "test").expect("session");
        let mut expected_source = None;
        for (ts, phone) in [
            (range.from - 1, "0800-000-123"),
            (range.from, "0800-000-123"),
            (range.from + 1_200_000, "0800-000-123"),
            (range.to, "0800-000-123"),
            (now, "0800-000-123"),
            (now + 1, "0800-000-124"),
            (now + 2, "0800-000-125"),
            (now + 3, "0800-000-126"),
        ] {
            let inserted = db
                .insert_frame(
                    session,
                    &FrameCapture {
                        assistive: Vec::new(),
                        ts,
                        monitor: 0,
                        width: 800,
                        height: 600,
                        dhash: 1,
                        image: None,
                        image_ext: "png",
                        ocr: vec![OcrBlock {
                            text: format!("帳單客服專線 ID {phone}"),
                            x: 0,
                            y: 0,
                            w: 300,
                            h: 20,
                            confidence: 1.0,
                        }],
                        focus: FocusSnapshot::default(),
                    },
                    None,
                    0,
                )
                .expect("frame");
            if ts == range.from + 1_200_000 {
                expected_source = Some(inserted.0);
            }
        }

        // 兩字中文、長中文、短英文整詞與數字子字串涵蓋四條取證路徑。
        // 今天的同文紀錄必須在 LIMIT 前排除，不能先取一筆再 retain。
        for query in ["昨天帳單", "昨天客服專線", "昨天 ID", "昨天 80"] {
            let got = RetrievalProfile::TextAndFacts
                .retrieve_at(&mut db, query, RetrievalLimits::same(1), now)
                .expect("retrieval");
            assert_eq!(got.hits.len(), 1, "{query}");
            assert_eq!(got.hits[0].ts, range.from + 1_200_000, "{query}");
            assert_eq!(got.hits[0].frame_id, expected_source, "仍保留同筆畫面出處");
            assert!(got.hits_truncated, "昨天還有第二筆原文");
        }

        let got = RetrievalProfile::TextAndFacts
            .retrieve_at(&mut db, "昨天電話", RetrievalLimits::same(1), now)
            .expect("facts");
        assert_eq!(got.answers.len(), 1);
        assert_eq!(got.answers[0].latest.ts, range.from + 1_200_000);
        assert_eq!(got.answers[0].latest.frame_id, expected_source);
        assert_eq!(got.answers[0].sightings, 2, "只數問句時間內的目擊");
        assert!(!got.answers_truncated, "重複目擊不是多一個答案");

        for query in ["帳單", "今天帳單"] {
            let rewritten = RetrievalProfile::TextAndFacts
                .retrieve_for_question_at(&mut db, query, "昨天帳單", RetrievalLimits::same(1), now)
                .expect("rewritten retrieval");
            assert_eq!(rewritten.hits.len(), 1);
            assert_eq!(
                rewritten.hits[0].ts,
                range.from + 1_200_000,
                "改寫成 {query} 也不能丟掉原問句的昨天"
            );
        }

        let undated = RetrievalProfile::TextAndFacts
            .retrieve_at(&mut db, "電話", RetrievalLimits::same(1), now)
            .expect("undated");
        assert_eq!(
            undated.answers[0].latest.ts,
            now + 3,
            "沒有指定日期仍取最新出處"
        );

        for query in ["前天早上客服專線", "前天早上電話"] {
            let missing = RetrievalProfile::TextAndFacts
                .retrieve_at(&mut db, query, RetrievalLimits::same(1), now)
                .expect("no match");
            assert!(missing.time_range.is_some());
            assert!(
                missing.hits.is_empty() && missing.answers.is_empty(),
                "{query} 沒有命中不能放寬成所有日期"
            );
        }
    }

    /// `remember_told` 把那句話的時間記成呼叫端給的 `now`。
    /// 關鍵字題的時間窗會套在這筆上，沒有另開一條 told 規則。
    /// 窗裡有現在（今天、這禮拜）就找得到；窗在現在之前（昨天、上禮拜）就找不到。
    /// 沒有時間詞的同一句不受窗限制。關掉再開同一顆檔案，那筆還在。
    #[test]
    fn told_now_is_inside_today_and_outside_last_week() {
        use crate::model::SourceKind;
        use chrono::{Local, TimeZone};

        let now = Local
            .with_ymd_and_hms(2026, 9, 23, 15, 0, 0)
            .single()
            .expect("local")
            .timestamp_millis();
        let text = "客服電話 0800-111-222";
        let privacy = crate::config::PrivacyConfig::default();
        assert!(privacy.remember_told);

        let last = question::time_range("上禮拜的客服電話", now).expect("上禮拜");
        let yesterday = question::time_range("昨天的客服電話", now).expect("昨天");
        let today = question::time_range("今天的客服電話", now).expect("今天");
        let this_week = question::time_range("這禮拜的客服電話", now).expect("這禮拜");
        assert!(
            now < last.from || now >= last.to,
            "now={now} 必須在上禮拜 [{}, {}) 外面",
            last.from,
            last.to
        );
        assert!(
            now < yesterday.from || now >= yesterday.to,
            "now={now} 必須在昨天 [{}, {}) 外面",
            yesterday.from,
            yesterday.to
        );
        assert!(now >= today.from && now < today.to, "今天必須含現在");
        assert!(
            now >= this_week.from && now < this_week.to,
            "這禮拜必須含現在"
        );
        assert_eq!(question::terms("上禮拜的客服電話"), "客服電話");
        assert_eq!(question::shape("上禮拜的客服電話"), Shape::Keywords);

        let dir =
            std::env::temp_dir().join(format!("sister-a156-told-{}-{}", std::process::id(), now));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp");
        let path = dir.join("sister.db");
        struct Cleanup(std::path::PathBuf);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
        let _cleanup = Cleanup(dir);

        {
            let mut db = Db::open(&path).expect("open");
            let id = db
                .remember_told(&privacy, now, text)
                .expect("remember")
                .expect("id");
            assert!(id > 0);
        }
        let mut db = Db::open(&path).expect("reopen");
        let kept = db.search("客服電話", 5).expect("search after reopen");
        assert_eq!(kept.len(), 1, "重開之後同一題要找得到");
        assert_eq!(kept[0].text, text);
        assert_eq!(kept[0].ts, now);
        assert_eq!(kept[0].source_kind, SourceKind::Told);

        let undated = RetrievalProfile::TextAndFacts
            .retrieve_at(&mut db, "客服電話", RetrievalLimits::same(5), now)
            .expect("undated");
        assert!(undated.time_range.is_none());
        assert_eq!(undated.hits.len(), 1);

        let today_hit = RetrievalProfile::TextAndFacts
            .retrieve_at(&mut db, "今天的客服電話", RetrievalLimits::same(5), now)
            .expect("today");
        assert_eq!(today_hit.hits.len(), 1, "今天含現在，不該被時間窗擋掉");
        assert_eq!(today_hit.hits[0].source_kind, SourceKind::Told);

        let week_hit = RetrievalProfile::TextAndFacts
            .retrieve_at(&mut db, "這禮拜的客服電話", RetrievalLimits::same(5), now)
            .expect("this week");
        assert_eq!(week_hit.hits.len(), 1, "這禮拜含現在，不該被時間窗擋掉");

        let last_hit = RetrievalProfile::TextAndFacts
            .retrieve_at(&mut db, "上禮拜的客服電話", RetrievalLimits::same(5), now)
            .expect("last week");
        assert!(last_hit.time_range.is_some());
        assert!(
            last_hit.hits.is_empty() && last_hit.answers.is_empty(),
            "上禮拜不含現在，剛記住的那筆被時間窗擋掉"
        );

        let yesterday_hit = RetrievalProfile::TextAndFacts
            .retrieve_at(&mut db, "昨天的客服電話", RetrievalLimits::same(5), now)
            .expect("yesterday");
        assert!(
            yesterday_hit.hits.is_empty() && yesterday_hit.answers.is_empty(),
            "昨天不含現在"
        );

        let recent = RetrievalProfile::TextAndFacts
            .retrieve_at(&mut db, "剛剛發生什麼事", RetrievalLimits::same(5), now)
            .expect("recent");
        assert_eq!(recent.shape, Shape::Recent);
        assert!(recent.time_range.is_none());
        assert!(
            recent.hits.iter().any(|hit| hit.text == text),
            "剛剛沒有日曆窗，最新那筆就是剛記住的"
        );

        let last_range = RetrievalProfile::TextAndFacts
            .retrieve_at(&mut db, "上禮拜", RetrievalLimits::same(5), now)
            .expect("range");
        assert_eq!(last_range.shape, Shape::Range);
        assert!(last_range.hits.iter().all(|hit| hit.text != text));

        let this_range = RetrievalProfile::TextAndFacts
            .retrieve_at(&mut db, "這禮拜", RetrievalLimits::same(5), now)
            .expect("this week range");
        assert_eq!(this_range.shape, Shape::Range);
        assert!(
            this_range.hits.iter().any(|hit| hit.text == text),
            "這禮拜的日曆窗含現在，chunks_in_range 會列出剛記住的那筆"
        );

        eprintln!(
            "a156 told_now now={now} last_week=[{}, {}) yesterday=[{}, {}) today=[{}, {}) this_week=[{}, {}) reopen_hits={} today_hits={} this_week_hits={} last_week_hits={} yesterday_hits={} recent_has_told={} last_range_hits={} this_range_has_told={}",
            last.from,
            last.to,
            yesterday.from,
            yesterday.to,
            today.from,
            today.to,
            this_week.from,
            this_week.to,
            kept.len(),
            today_hit.hits.len(),
            week_hit.hits.len(),
            last_hit.hits.len(),
            yesterday_hit.hits.len(),
            recent.hits.iter().any(|hit| hit.text == text),
            last_range.hits.len(),
            this_range.hits.iter().any(|hit| hit.text == text),
        );
    }

    #[test]
    fn session_profile_returns_activities_the_text_profile_does_not() {
        use crate::model::{FocusEvent, FocusKind, FocusSnapshot};
        use chrono::{Local, TimeZone};

        let now = Local
            .with_ymd_and_hms(2026, 8, 26, 15, 30, 0)
            .single()
            .expect("local")
            .timestamp_millis();
        let t0 = Local
            .with_ymd_and_hms(2026, 8, 25, 13, 0, 0)
            .single()
            .expect("local")
            .timestamp_millis();

        let mut db = Db::open_in_memory().expect("db");
        let session = db.start_session("test", "test").expect("session");
        let min = 60_000i64;
        for (ts, app, title) in [
            (t0, "code.exe", "db.rs — AI-Sister"),
            (t0 + 45 * min, "chrome.exe", "SQLite user_version 文件"),
            (t0 + 70 * min, "notion.exe", "週報"),
            (t0 + 115 * min, "notion.exe", "週報"),
        ] {
            db.insert_focus(
                session,
                &FocusEvent {
                    ts,
                    kind: FocusKind::Focus,
                    snapshot: FocusSnapshot {
                        app_id: Some(app.into()),
                        window_title: Some(title.into()),
                        ..FocusSnapshot::default()
                    },
                },
            )
            .expect("focus");
        }

        let text = RetrievalProfile::TextOnly
            .retrieve_at(&mut db, "我昨天下午在弄什麼", RetrievalLimits::same(5), now)
            .expect("text");
        assert!(text.activities.is_empty(), "文字配置不進章節");

        let session = RetrievalProfile::TextFactsAndSession
            .retrieve_at(&mut db, "我昨天下午在弄什麼", RetrievalLimits::same(5), now)
            .expect("session");
        assert_eq!(session.activities.len(), 3, "三件事");
        assert_eq!(
            session
                .activities
                .iter()
                .map(|a| a.segment_count)
                .collect::<Vec<_>>(),
            vec![5, 3, 5]
        );
    }

    #[test]
    fn unchanged_and_too_long_are_not_nothing_seen() {
        let mut db = db_with_bill();
        assert_eq!(
            db.indexed_candidate("客服").unwrap(),
            IndexedCandidate::Unchanged
        );
        assert_eq!(
            db.indexed_candidate("沒看過").unwrap(),
            IndexedCandidate::NoneSeen
        );
        assert_eq!(
            db.indexed_candidate("請問客服").unwrap(),
            IndexedCandidate::Changed("客服".into())
        );
        let within = format!("{}客服", "啊".repeat(126));
        assert_eq!(within.chars().count(), 128);
        assert_eq!(
            db.indexed_candidate(&within).unwrap(),
            IndexedCandidate::Changed("客服".into())
        );
        let over = format!("{}客服", "啊".repeat(127));
        assert_eq!(over.chars().count(), 129);
        assert_eq!(
            db.indexed_candidate(&over).unwrap(),
            IndexedCandidate::TooLong
        );
        let result = RetrievalProfile::TextAndFacts
            .retrieve(&mut db, &over, 5)
            .unwrap();
        assert_eq!(result.searched, None, "太長沒檢查，不放寬");
    }

    #[test]
    fn trailing_relaxation_pays_the_documented_cost() {
        let corpus: crate::replay::Corpus = serde_json::from_str(include_str!(
            "../../../scenarios/recall-baseline.corpus.json"
        ))
        .unwrap();
        let mut db = Db::open_in_memory().unwrap();
        db.import_replay(&corpus, 100).unwrap();

        assert_eq!(
            db.indexed_candidate("電信帳戶").unwrap(),
            IndexedCandidate::Changed("電信帳".into())
        );
        let clean = RetrievalProfile::TextAndFacts
            .retrieve(&mut db, "電信帳", 5)
            .unwrap();
        assert!(clean.searched.is_none() && !clean.hits.is_empty());
        let cut = RetrievalProfile::TextAndFacts
            .retrieve(&mut db, "電信帳戶", 5)
            .unwrap();
        assert_eq!(
            cut.searched,
            Some(SearchAdjustment::Relaxed("電信帳".into()))
        );
        assert_eq!(format!("{:?}", cut.hits), format!("{:?}", clean.hits));

        let clean = RetrievalProfile::TextAndFacts
            .retrieve(&mut db, "部署失敗", 5)
            .unwrap();
        assert!(clean.searched.is_none() && !clean.hits.is_empty());
        let shrunk = RetrievalProfile::TextAndFacts
            .retrieve(&mut db, "部署失敗 退款流程", 5)
            .unwrap();
        assert_eq!(
            shrunk.searched,
            Some(SearchAdjustment::Relaxed("部署失敗".into()))
        );
        assert_eq!(format!("{:?}", shrunk.hits), format!("{:?}", clean.hits));
        assert_eq!(
            format!("{:?}", shrunk.answers),
            format!("{:?}", clean.answers)
        );
    }

    /// 「敗是」在別的句子裡看過。索引那一圈會把「部署失敗是」整段留下；
    /// 再跑一次 terms 才把尾端的「是」剝掉。
    #[test]
    fn relax_peel_drops_a_seen_boundary_filler() {
        let mut db = Db::open_in_memory().unwrap();
        let privacy = crate::config::PrivacyConfig {
            remember_told: true,
            ..Default::default()
        };
        db.remember_told(&privacy, 100, "部署失敗的紀錄").unwrap();
        db.remember_told(&privacy, 200, "打敗是另一件事").unwrap();
        assert!(
            db.indexed_term_exists("敗是").unwrap(),
            "前提：交界雙字「敗是」看過"
        );
        assert_eq!(
            db.indexed_candidate("部署失敗是").unwrap(),
            IndexedCandidate::Unchanged,
            "前提：索引不會自己把看過的「是」拿掉"
        );

        let result = RetrievalProfile::TextAndFacts
            .retrieve(&mut db, "部署失敗是什麼原因", 5)
            .unwrap();
        assert_eq!(
            result.searched,
            Some(SearchAdjustment::Relaxed("部署失敗".into()))
        );
        assert!(
            result
                .hits
                .iter()
                .any(|hit| hit.text.contains("部署失敗的紀錄")),
            "{:?}",
            result.hits
        );
        assert!(
            result.hits.iter().all(|hit| !hit.text.contains("打敗是")),
            "不該改去找那句只為了讓「敗是」看過的話: {:?}",
            result.hits
        );
    }

    /// `showtime` 裡的 how 不是獨立的問句詞。不看邊界的話會從 h 切開，
    /// 候選變成「部署失敗」。
    #[test]
    fn relax_peel_does_not_cut_how_inside_show() {
        let mut db = Db::open_in_memory().unwrap();
        let privacy = crate::config::PrivacyConfig {
            remember_told: true,
            ..Default::default()
        };
        db.remember_told(&privacy, 100, "部署失敗 showtime")
            .unwrap();
        let result = RetrievalProfile::TextAndFacts
            .retrieve(&mut db, "部署失敗 showtime 怎麼辦", 5)
            .unwrap();
        assert_eq!(
            result.searched,
            Some(SearchAdjustment::Relaxed("部署失敗 showtime".into())),
            "show 裡的 how 不是問句詞"
        );
        assert!(
            result
                .hits
                .iter()
                .any(|hit| hit.text.contains("部署失敗 showtime")),
            "{:?}",
            result.hits
        );
    }

    /// 問句詞清單的代價。數字寫在 `retry_candidate` 上面，這裡把量到的結果釘住。
    #[test]
    fn relax_peel_pays_the_measured_cost() {
        let mut db = db_with_bill();
        let cut = RetrievalProfile::TextAndFacts
            .retrieve(&mut db, "客服怎麼打電話", 5)
            .unwrap();
        assert_eq!(cut.searched, Some(SearchAdjustment::Relaxed("客服".into())));
        assert!(
            cut.answers.is_empty(),
            "「電話」在問句詞後面，跟著被丟掉，facts 沒有種類: {:?}",
            cut.answers
        );
        assert_eq!(cut.hits.len(), 1, "{:?}", cut.hits);
        assert_eq!(cut.hits[0].text, "客服專線 0800-000-123");

        let missing = RetrievalProfile::TextAndFacts
            .retrieve(&mut db, "這個東西是什麼", 5)
            .unwrap();
        assert_eq!(missing.terms.as_deref(), Some("東西"));
        assert_eq!(missing.searched, None);
        assert!(missing.answers.is_empty() && missing.hits.is_empty());

        let privacy = crate::config::PrivacyConfig {
            remember_told: true,
            ..Default::default()
        };
        db.remember_told(&privacy, 200, "這個東西在桌上").unwrap();
        let seen = RetrievalProfile::TextAndFacts
            .retrieve(&mut db, "這個東西是什麼", 5)
            .unwrap();
        assert_eq!(seen.terms.as_deref(), Some("東西"));
        assert_eq!(seen.searched, None, "第一次就用「東西」找到，不再放寬");
        assert!(
            seen.hits
                .iter()
                .any(|hit| hit.text.contains("這個東西在桌上")),
            "{:?}",
            seen.hits
        );

        let ordinary = RetrievalProfile::TextAndFacts
            .retrieve(&mut db, "這個東西怎麼用", 5)
            .unwrap();
        assert_eq!(
            ordinary.searched,
            Some(SearchAdjustment::Relaxed("東西".into()))
        );
        assert!(
            ordinary
                .hits
                .iter()
                .any(|hit| hit.text.contains("這個東西在桌上")),
            "{:?}",
            ordinary.hits
        );
    }

    /// 「所以」拿掉之後還有「我想問」。只剝一次、不回頭跑 [`question::terms`]，
    /// 「我」還擋在「想問」前面，剝完不是「部署失敗」。
    #[test]
    fn stacked_spoken_lead_ins_peel_to_the_topic() {
        assert_eq!(peel_retry_terms("所以我想問為什麼部署失敗"), "部署失敗");
        // 「想請問」整段在表上。表裡沒有哪個詞是另一個詞的開頭，所以這一條
        // 不守「同一位置取最長」；那一行現在沒有例子可以測。
        assert_eq!(peel_retry_terms("想請問為什麼部署失敗"), "部署失敗");
        // 「到」是虛字。先跑 terms 再認開頭，會把「到底」吃成「底」。
        assert_eq!(peel_retry_terms("到底為什麼部署失敗"), "部署失敗");
    }

    /// `butter`／`sonic`／`thence` 的開頭幾個字母不是獨立的 but／so／then。
    #[test]
    fn english_lead_in_does_not_strip_inside_a_longer_token() {
        assert_eq!(
            peel_retry_terms("butter 為什麼部署失敗"),
            "butter",
            "but 沒有切在 butter 裡面"
        );
        assert_eq!(
            peel_retry_terms("sonic 怎麼辦"),
            "sonic",
            "so 沒有切在 sonic 裡面"
        );
        assert_eq!(
            peel_retry_terms("thence 為什麼部署失敗"),
            "thence",
            "then 沒有切在 thence 裡面"
        );
    }

    /// 類型詞那支邊界認複數 s，也把連字號當邊界。口語開頭借它的話，
    /// `SOS` 會被當成 so，`So-net` 的 So 會被當成口語切掉。
    #[test]
    fn english_lead_in_is_not_a_plural_or_part_of_a_hyphenated_name() {
        assert_eq!(peel_retry_terms("SOS 怎麼回事"), "SOS");
        assert_eq!(peel_retry_terms("So-net 帳單怎麼繳"), "So-net 帳單");
        // 句讀照樣是一個字的結尾。
        assert_eq!(peel_retry_terms("But, why ERR_DEPLOY_42"), "ERR_DEPLOY_42");
    }

    #[test]
    fn na_question_words_are_removed_whole() {
        assert_eq!(peel_retry_terms("哪一個客服電話"), "客服電話");
        assert_eq!(peel_retry_terms("哪裡有客服電話"), "客服電話");
    }

    /// 整句都是口語開頭。迴圈要結束，而且不能拿其中一個字去放寬。
    #[test]
    fn only_spoken_lead_ins_finish_and_do_not_relax() {
        let mut db = Db::open_in_memory().unwrap();
        let privacy = crate::config::PrivacyConfig {
            remember_told: true,
            ..Default::default()
        };
        db.remember_told(&privacy, 100, "所以為什麼會這樣").unwrap();
        db.remember_told(&privacy, 200, "但是我不知道").unwrap();
        db.remember_told(&privacy, 300, "可是我不想去").unwrap();
        assert_eq!(
            db.indexed_candidate("所以").unwrap(),
            IndexedCandidate::Unchanged,
            "前提：「所以」看過；沒剝乾淨就會被放出去"
        );
        assert_eq!(
            db.indexed_candidate("但是").unwrap(),
            IndexedCandidate::Unchanged,
            "前提：「但是」看過"
        );
        // 「是」是虛字。整句已經夠長時，尾端那個「是」不會被退回來，
        // 「可是」對不到整段，最後剩「可」。迴圈要在這裡停，而且不放寬。
        let peeled = peel_retry_terms("所以但是可是");
        assert!(
            peeled.chars().filter(|c| !c.is_whitespace()).count() < 2,
            "只有口語開頭，剝完不該還有兩個字：「{peeled}」"
        );
        assert_eq!(tries(&db, "所以但是可是"), Vec::<String>::new());
    }

    /// 「回」是索引裡的一個 token，`indexed_candidate` 回 `Unchanged`。
    /// 出口若只寫在 `Changed` 那一臂，這一條會放行。
    #[test]
    fn one_leftover_character_is_not_relaxed_when_the_index_has_seen_it() {
        let mut db = Db::open_in_memory().unwrap();
        let privacy = crate::config::PrivacyConfig {
            remember_told: true,
            ..Default::default()
        };
        db.remember_told(&privacy, 100, "請按 回 上一頁").unwrap();
        assert_eq!(peel_retry_terms("怎麼回事"), "回");
        assert_eq!(
            db.indexed_candidate("回").unwrap(),
            IndexedCandidate::Unchanged,
            "前提：沒有出口的話，「回」會被放出去"
        );
        assert_eq!(tries(&db, "怎麼回事"), Vec::<String>::new());
    }

    /// 「電話」看過，而且是類型詞、沒有主題。沒有出口就會改用「電話」。
    #[test]
    fn a_bare_type_word_is_not_relaxed_when_the_index_has_seen_it() {
        let mut db = Db::open_in_memory().unwrap();
        let privacy = crate::config::PrivacyConfig {
            remember_told: true,
            ..Default::default()
        };
        db.remember_told(&privacy, 100, "晚點打電話給你").unwrap();
        assert_eq!(peel_retry_terms("電話怎麼打"), "電話");
        assert!(
            !crate::facts::kinds_for_query("電話").is_empty(),
            "前提：「電話」是類型詞"
        );
        assert_eq!(
            crate::facts::topic_constraint("電話"),
            None,
            "前提：「電話」沒有主題"
        );
        assert_eq!(
            db.indexed_candidate("電話").unwrap(),
            IndexedCandidate::Unchanged,
            "前提：沒有出口的話，「電話」會被放出去"
        );
        assert_eq!(tries(&db, "電話怎麼打"), Vec::<String>::new());
    }
}
