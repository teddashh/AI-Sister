//! 拔手熱鍵按完那一句話：**七種結局，七句不一樣的話，而且每一句都指得出下一步。**
//!
//! 這一份是 r31 的收貨考題，在派工出去的同一小時寫的，作者沒有看過成品。所以它
//! 一個字都不綁在修法上——`WhyNotWritten` 多不多一格、句子怎麼排、`press_hands_hotkey`
//! 內部怎麼走，這裡都不管。它只問使用者摸得到的三件事：
//!
//! 1. **兩個不同的結局不可以講同一句話。** r30 那一版有一刀實測全綠：把「開關沒
//!    寫下來，可是她讀到的狀態是拔著」那一臂，整個換成「手拔掉了」——五個斷言
//!    一個都沒紅。那兩件事的差別是旗標**到底有沒有落地**：沒落地的話，把擋路的
//!    那個檔案挪走、或換一台機器讀同一個 data dir，她立刻恢復交件，而他手上唯一
//!    的憑證是那句「拔掉了」。
//! 2. **句子裡不可以有空格子。** 同一輪另一刀：`WhyNotWritten::CannotWrite` 的
//!    中文換成 `""`，全綠——他讀到的是「，沒能拔掉。她的手還接著——」，開頭一個
//!    孤零零的逗號，而唯一告訴他「為什麼拔不掉」的那幾個字沒了。
//! 3. **指出去的路要真的走得通。** 「去系統匣按『拔掉她的手』」在這句話印得出來
//!    的每一格都是死路：系統匣那顆 handler 讀的是同一個 `Shell.data_dir`、呼叫的
//!    是同一支 `kill_switch::pull`（`main.rs:3462-3477`）。`NoDataDir` 那一格它
//!    直接回「資料目錄讀不到」；唯讀目錄那一格它撞同一個 EACCES。
//!
//! 底下沒有一條斷言去比對完整的句子字面——那種測試會跟著產品一起漂
//! （產品和測試各抄一份，兩邊一起改就永遠不會紅）。這裡比的是**關係**。

use sister_hands::kill_switch::{
    HandsHotkeyOutcome as Outcome, WhyNotWritten as Why, hands_hotkey_message as says,
};

/// 使用者按下那顆鍵之後，可能落在的每一格。
///
/// `os_error` 一律給 `None`：`hands_hotkey_message` 不讀它（它只進 log，見
/// `WhyNotWritten` 的 doc），測試餵一個沒人讀的欄位等於什麼都沒驗。
fn every_ending() -> Vec<(&'static str, Outcome)> {
    let not_written = |why, stopped| Outcome::NotWritten {
        why,
        stopped,
        os_error: None,
    };
    vec![
        (
            "這一下真的拔掉了",
            Outcome::Pulled {
                at_ms: 1_700_000_000_000,
            },
        ),
        (
            "本來就拔著，讀得到時間",
            Outcome::AlreadyPulled {
                since_ms: Some(1_700_000_000_000),
            },
        ),
        (
            "本來就拔著，讀不到時間",
            Outcome::AlreadyPulled { since_ms: None },
        ),
        (
            "問不出資料目錄，手還接著",
            not_written(Why::NoDataDir, false),
        ),
        (
            "問不出資料目錄，可是她讀到的是拔著",
            not_written(Why::NoDataDir, true),
        ),
        ("寫不進去，手還接著", not_written(Why::CannotWrite, false)),
        (
            "寫不進去，可是她讀到的是拔著",
            not_written(Why::CannotWrite, true),
        ),
    ]
}

/// **兩個不同的結局，不可以拿到同一句話。**
///
/// 這是整份考題的核心。他按完那一下之後唯一能讀到的就是這一句；兩格共用一句話，
/// 等於分辨它們的那個條件在畫面上零覆蓋，而其中兩格的下一步是相反的。
#[test]
fn no_two_endings_share_a_sentence() {
    let endings = every_ending();
    for (i, (what_a, a)) in endings.iter().enumerate() {
        for (what_b, b) in endings.iter().skip(i + 1) {
            assert_ne!(
                says(a),
                says(b),
                "「{what_a}」和「{what_b}」拿到同一句話。\
                 他讀完之後分不出自己在哪一格，而這兩格的下一步不一樣。\n  句子：{}",
                says(a)
            );
        }
    }
}

/// **句子裡不可以有空格子。**
///
/// 拼進去的那一段（`WhyNotWritten::zh()` 這一族）如果變成空字串，句子照樣印得
/// 出來、照樣通過所有「有沒有含某個字」的斷言，而他讀到的是一句開頭掛著標點的
/// 殘句。這一條不看格式，只看**症狀**：沒有一句話可以用標點開頭。
///
/// **第一版比這個寬，而且它自己在說謊。** 我原本還檢查「兩個標點連在一起」，
/// 於是「（拔手時間讀不到）。」被判成空格子——`）` 和 `。` 都在標點表裡，而那是
/// 一句完全正常的中文。**一個會對正確的碼變紅的檢查，比沒有檢查更糟**：它會逼
/// 下一個人去改一句沒有壞的話。開頭那一格就夠了：`{}，⋯` 這種拼法，空的那一段
/// 一定落在句首。中間的空格子這一條抓不到，寫在這裡不要假裝。
#[test]
fn no_sentence_has_an_empty_slot() {
    const PUNCT: [char; 8] = ['，', '。', '、', '；', '：', '—', '（', '）'];
    for (what, outcome) in every_ending() {
        let line = says(&outcome);
        let first = line.chars().next().unwrap_or(' ');
        assert!(
            !PUNCT.contains(&first),
            "「{what}」那一句用標點開頭——前面那個格子是空的：{line}"
        );
    }
}

/// **「沒寫成」不可以穿上「寫成了」的那件衣服。**
///
/// 上面 `no_two_endings_share_a_sentence` 已經涵蓋這一刀，這一條把理由單獨寫下來，
/// 因為它是 r30 那一版實測全綠、而且錯在最貴方向的那一格：旗標沒落地，畫面卻宣告
/// 拔掉了。她現在停著只是 `is_pulled` fail-closed 的副作用，不是那個開關生效了。
#[test]
fn a_write_that_never_landed_never_wears_the_success_sentence() {
    let landed = says(&Outcome::Pulled { at_ms: 1 });
    for why in [Why::NoDataDir, Why::CannotWrite] {
        for stopped in [true, false] {
            let never_landed = says(&Outcome::NotWritten {
                why,
                stopped,
                os_error: None,
            });
            assert_ne!(
                landed, never_landed,
                "旗標沒有落地（{why:?}／她讀到的狀態是 {stopped}），\
                 句子卻和「真的拔掉了」一字不差。"
            );
        }
    }
}

/// **指出去的路，要在那一格真的走得通。**
///
/// 兩格都是「手還接著」，但可走的路不一樣，所以不可以共用一句下一步：
///
/// - `NoDataDir`：這個行程問不出 data dir，系統匣那顆讀的是**同一個**
///   `Shell.data_dir`，按下去只會回「資料目錄讀不到」。活路只剩另一個行程——
///   `sister hands stop` 自己解析 data dir。
/// - `CannotWrite`：她寫不進那個資料夾。系統匣那顆和 `sister hands stop` 走的是
///   同一支 `pull`、對同一條路徑，兩條都會撞同一道牆。**這一格不可以把系統匣那顆
///   當成出路。**
#[test]
fn the_way_out_is_a_way_that_actually_works() {
    let attached = |why| {
        says(&Outcome::NotWritten {
            why,
            stopped: false,
            os_error: None,
        })
    };

    let no_dir = attached(Why::NoDataDir);
    assert!(
        no_dir.contains("sister hands stop"),
        "問不出資料目錄的時候，唯一走得通的是另一個行程，而那句話沒提它：{no_dir}"
    );

    let cannot_write = attached(Why::CannotWrite);
    assert!(
        !cannot_write.contains("拔掉她的手"),
        "她寫不進那個資料夾，這句話卻叫他去按系統匣那顆——那顆走的是同一支 `pull`、\
         對同一條路徑，按下去撞同一道牆。把他支使去做一件註定失敗的事，\
         而他正在出事：{cannot_write}"
    );
    assert_ne!(
        no_dir, cannot_write,
        "兩格的可行路不一樣，句子卻一模一樣——其中一格在指死路。"
    );
}

/// 手還接著的時候一定要說出來；她其實停著的時候一定不可以說。
///
/// 這一條是 r30 已經有的不變式（`the_sentence_agrees_with_the_only_authority`）的
/// 純函式版本：那一條走真的檔案系統、受平台影響，這一條把四格直接擺出來。兩條都要
/// 有——真檔案那條證的是 `press_hands_hotkey` 接對了，這條證的是句子本身分得開。
#[test]
fn the_sentence_says_whether_the_hands_are_still_attached() {
    for why in [Why::NoDataDir, Why::CannotWrite] {
        for stopped in [true, false] {
            let line = says(&Outcome::NotWritten {
                why,
                stopped,
                os_error: None,
            });
            assert_eq!(
                line.contains("還接著"),
                !stopped,
                "{why:?}／她讀到的狀態是 {stopped}，而那句話{}說手還接著：{line}",
                if line.contains("還接著") {
                    ""
                } else {
                    "沒有"
                }
            );
        }
    }
}
