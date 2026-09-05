//! r29 的驗收測試。**在 delegate 交貨之前寫好的。**
//!
//! 證的是「我事先寫下的考題過了」，不是「它的測試通過它自己的修法」。
//!
//! # 病灶（PHASES.md:520 的「還缺」）
//!
//! 交出一步之後，她會等下一張畫面（alpha.81），然後說：
//!
//! ```text
//! 做完之後的畫面憑據是 frame #12（14:05:03），圖在。
//! ```
//!
//! 這句話只證明**畫面變了**，不證明變成了他要的樣子。她開了一個網址，畫面上
//! 換成的可能是瀏覽器的錯誤頁、可能是登入牆、可能是完全另一個分頁——那一列
//! `frames` 上明明記著 `url`，而沒有任何人去比對過。
//!
//! # 這一檔守什麼
//!
//! 「該開的東西真的出現在畫面上了嗎」這個判斷，以及**它說不準的時候要說得像
//! 說不準**。三格：對得上、對不上、說不準；說不準那一格的句子必須自己講出
//! 「這只證明畫面變了，不證明變成你要的樣子」。
//!
//! ⚠ 這一檔在修法落地**之前**編不過（那幾個型別還不存在），而編不過的紅什麼
//!   都沒證明。有價值的是交貨**之後**的突變。
//!
//! ⚠ `url_host` 是 `sister-core` 既有的那一支（`segment.rs`），不要在
//!   `sister-hands` 裡再抄一份——抄一份就會漂。這也是為什麼比對住在
//!   `sister-core`（它依賴 `sister-hands`，反過來不行）。

use sister_core::screen_check::target_on_screen;
use sister_hands::ActionSnapshot;
use sister_hands::semi_action::{
    CannotTell, ScreenAfter, ScreenField, StepEvidence, TargetOnScreen,
};

fn open(url: &str) -> ActionSnapshot {
    ActionSnapshot::OpenUrl { url: url.into() }
}

fn screen(url: Option<&str>, title: Option<&str>) -> ScreenAfter {
    ScreenAfter {
        url: url.map(Into::into),
        window_title: title.map(Into::into),
    }
}

/// 把判斷包成使用者真的會讀到的那句話。
///
/// 只斷言在這一層，不斷言在中間的資料結構上——使用者讀的是句子。
fn sentence(target: TargetOnScreen) -> String {
    evidence(true, target).message()
}

fn evidence(has_image: bool, target: TargetOnScreen) -> StepEvidence {
    StepEvidence::After {
        frame_id: 12,
        frame_at_ms: 1_700_000_000_000,
        has_image,
        target,
    }
}

// ───────────────────────── 判斷本身 ─────────────────────────

/// 開了 example.com，畫面上的網址也在 example.com 上：對得上。
#[test]
fn a_url_that_really_opened_is_matched() {
    assert_eq!(
        target_on_screen(
            &open("https://example.com/a"),
            &screen(Some("https://example.com/a"), None)
        ),
        TargetOnScreen::Matched {
            field: ScreenField::Url,
            saw: "example.com".into(),
            wanted: "example.com".into(),
        }
    );
}

/// 同一個網站、不同路徑（被導去登入頁）仍然算對得上——比的是**網站**。
///
/// 這一條和上面那條一起把判準釘死在 host 上：比整串網址的話，任何一次
/// redirect 都會被講成「沒開成」，那是把一個看不見的問題換成一個天天誤報的
/// 問題。
#[test]
fn the_same_site_on_another_path_still_counts() {
    assert_eq!(
        target_on_screen(
            &open("https://example.com/a"),
            &screen(Some("https://example.com/login?next=/a"), None)
        ),
        TargetOnScreen::Matched {
            field: ScreenField::Url,
            saw: "example.com".into(),
            wanted: "example.com".into(),
        }
    );
}

/// **`www.` 開頭的網址不准被講成對不上。**
///
/// 這是 r29 三鏡頭審查裡唯一一條「阻擋出貨」：使用者按了「好」，Chrome 正確
/// 開了 `https://www.gov.tw/x`，而網址列（＝UIA 寫進 `frames.url` 的東西）
/// 顯示的是 `gov.tw/x`。整串比下去 `www.gov.tw != gov.tw`，於是一次**完全
/// 成功**的動作會被講成「這一步有沒有真的做到，她沒有把握」。
///
/// 這不是我推論出來的，是這個產品自己寫下來的。`config.rs` 在教使用者寫規則
/// 的地方就印著：「以 `www.` 開頭：網址列會省略 `www.`，這條規則不會命中」，
/// 而 `facts.rs:548` 早就在做同一件正規化。新的比對是第三個知道、卻沒做的。
///
/// 兩個方向都要測：使用者打 `www.`、畫面沒有，和使用者沒打、畫面有。
#[test]
fn a_www_prefix_is_not_a_different_site() {
    assert_eq!(
        target_on_screen(
            &open("https://www.gov.tw/x"),
            &screen(Some("https://gov.tw/x"), None)
        ),
        TargetOnScreen::Matched {
            field: ScreenField::Url,
            saw: "gov.tw".into(),
            wanted: "www.gov.tw".into(),
        },
        "使用者打了 www.，網址列省掉了——這一步是成功的"
    );
    assert_eq!(
        target_on_screen(
            &open("https://gov.tw/x"),
            &screen(Some("https://www.gov.tw/x"), None)
        ),
        TargetOnScreen::Matched {
            field: ScreenField::Url,
            saw: "www.gov.tw".into(),
            wanted: "gov.tw".into(),
        },
        "反過來也一樣：正規化要對稱，不能只剝一邊"
    );
}

/// `www.` 是**開頭那一層**，不是「含有 www」，也不是「全部剝光」。
///
/// 兩個方向的錯法各釘一刀：
///
/// - 拿 `www` 去做子字串比對／整串刪除 → `bwww.example.com` 會被當成
///   `example.com`，而它們是兩個不同的網站。
/// - 反覆剝到剝不動為止 → `www.www.evil.com` 會被當成 `evil.com`。
///   （這一刀在下一條 `repeatedly_stripping_www_would_merge_real_sites`。）
#[test]
fn stripping_www_does_not_merge_two_real_sites() {
    assert_eq!(
        target_on_screen(
            &open("https://example.com/"),
            &screen(Some("https://bwww.example.com/"), None)
        ),
        TargetOnScreen::Mismatched {
            field: ScreenField::Url,
            saw: "bwww.example.com".into(),
            wanted: "example.com".into(),
        },
        "`www.` 只在開頭才算前綴；`bwww.example.com` 是別的網站"
    );
    assert_eq!(
        target_on_screen(
            &open("https://www.www.example.com/"),
            &screen(Some("https://www.example.com/"), None)
        ),
        TargetOnScreen::Matched {
            field: ScreenField::Url,
            saw: "www.example.com".into(),
            wanted: "www.www.example.com".into(),
        },
        "差一層 www.：網址列少印的就是那一層，這算對得上"
    );
}

/// 剝一層和剝到底不一樣，而只有「剝到底」會把兩個真的網站併成一個。
///
/// 這一條是上一條的另一半：`www.www.evil.com` 和 `evil.com` 差**兩層**，
/// 網址列不會少印兩層，所以它們是兩個網站。一個「`while` 剝到剝不動」
/// 的修法會讓上一條全綠、這一條紅。
#[test]
fn repeatedly_stripping_www_would_merge_real_sites() {
    assert_eq!(
        target_on_screen(
            &open("https://evil.com/"),
            &screen(Some("https://www.www.evil.com/"), None)
        ),
        TargetOnScreen::Mismatched {
            field: ScreenField::Url,
            saw: "www.www.evil.com".into(),
            wanted: "evil.com".into(),
        },
        "差兩層 www. 是兩個網站，不可以剝到底"
    );
}

/// 畫面上是別的網站：對不上，而且要記下看到的是哪一個。
#[test]
fn another_site_on_screen_is_a_mismatch() {
    assert_eq!(
        target_on_screen(
            &open("https://example.com/a"),
            &screen(Some("https://evil.example/phish"), None)
        ),
        TargetOnScreen::Mismatched {
            field: ScreenField::Url,
            saw: "evil.example".into(),
            wanted: "example.com".into(),
        }
    );
}

/// 那一欄**是空的**：說不準。
///
/// **這一格是預設情況，不是邊角料。** `frames.url` 的唯一產品寫入端是
/// `sister-capture` 的錄影迴圈，而它要三件事同時成立才會有值：跑在 Windows、
/// 前景 app 在 `browsers` 白名單上、UIA 那一次真的讀回來。三件裡少一件就是
/// `None`。
#[test]
fn no_url_on_the_frame_is_not_a_mismatch() {
    assert_eq!(
        target_on_screen(&open("https://example.com/a"), &screen(None, None)),
        TargetOnScreen::CannotTell {
            why: CannotTell::NothingOnScreen {
                field: ScreenField::Url
            }
        }
    );
}

/// 那一欄**有值、但不是一個看得出網站的網址**：說不準，而且要和「是空的」
/// 分成兩句話。
///
/// 分開的理由不是分類癖，是那句話會說謊：r29 早先兩格共用「這台機器沒有記下
/// 那張畫面的網址」，而這一格機器**確實記下了**——它記到的是 `about:blank`。
///
/// 走得到，實測過：UIA 的 `plausible_url` 明文收 `about:` 和 `chrome:`
/// （`sister-capture/src/windows/uia.rs:494`），而 `about:blank` 進 `url_host`
/// 之後算出來的 host 是 `about`，沒有點，`looks_like_host` 不收。開一個新分頁
/// 就是這一格。
#[test]
fn a_url_field_that_is_not_a_url_is_a_third_thing() {
    for recorded in ["about:blank", "chrome://settings", "SPEC.md — AI-Sister"] {
        assert_eq!(
            target_on_screen(
                &open("https://example.com/a"),
                &screen(Some(recorded), None)
            ),
            TargetOnScreen::CannotTell {
                why: CannotTell::ScreenUrlUnreadable
            },
            "`{recorded}` 有記下來，只是抽不出網站名——不可以說成「沒有記下」，\
             更不可以說成「她開錯了」"
        );
    }
}

/// 聚焦視窗比的是**視窗標題**，不是網址。
///
/// ⚠ 不可以拿 `ActionSnapshot::expected_target()` 來做這件事：那一支回答的是
///   「拿什麼跟記憶裡那筆 fact 比」，`FocusWindow` 對它是 `None`。這一支問的
///   是「拿什麼跟做完之後那張畫面比」，而 `FocusWindow` 對它恰恰最有話講。
#[test]
fn focusing_a_window_compares_the_title() {
    assert_eq!(
        target_on_screen(
            &ActionSnapshot::FocusWindow {
                title: "健保存摺".into()
            },
            &screen(None, Some("健保存摺 — Chrome"))
        ),
        TargetOnScreen::Matched {
            field: ScreenField::WindowTitle,
            saw: "健保存摺 — Chrome".into(),
            wanted: "健保存摺".into(),
        }
    );
}

/// 聚焦視窗，畫面上的標題裡沒有那幾個字：對不上。
#[test]
fn focusing_the_wrong_window_is_a_mismatch() {
    assert_eq!(
        target_on_screen(
            &ActionSnapshot::FocusWindow {
                title: "健保存摺".into()
            },
            &screen(None, Some("收件匣 — Outlook"))
        ),
        TargetOnScreen::Mismatched {
            field: ScreenField::WindowTitle,
            saw: "收件匣 — Outlook".into(),
            wanted: "健保存摺".into(),
        }
    );
}

/// 開檔案比的是視窗標題裡有沒有那個**檔名**，不是整條路徑。
#[test]
fn opening_a_file_compares_the_file_name_not_the_whole_path() {
    assert_eq!(
        target_on_screen(
            &ActionSnapshot::OpenFile {
                path: std::path::PathBuf::from(r"C:\Users\ted\報表.xlsx")
            },
            &screen(None, Some("報表.xlsx - Excel"))
        ),
        TargetOnScreen::Matched {
            field: ScreenField::WindowTitle,
            saw: "報表.xlsx - Excel".into(),
            wanted: "報表.xlsx".into(),
        }
    );
}

/// 該比視窗標題、而畫面上沒記標題：說不準。
#[test]
fn no_title_on_the_frame_is_not_a_mismatch() {
    assert_eq!(
        target_on_screen(
            &ActionSnapshot::FocusWindow {
                title: "健保存摺".into()
            },
            &screen(Some("https://example.com/"), None)
        ),
        TargetOnScreen::CannotTell {
            why: CannotTell::NothingOnScreen {
                field: ScreenField::WindowTitle
            }
        }
    );
}

/// 只有空白的視窗標題等於**沒有標題**，不是「標題裡沒有那幾個字」。
///
/// Win32 的 `GetWindowText` 對一個還在開的視窗會回一串空白，而
/// `"   ".contains("健保存摺")` 是 `false`——少了 `trim` 這道濾網，一個正在
/// 載入的視窗會被講成「她開錯了」。
#[test]
fn a_blank_title_is_nothing_to_compare_not_a_mismatch() {
    for blank in ["", "   ", "\t\n"] {
        assert_eq!(
            target_on_screen(
                &ActionSnapshot::FocusWindow {
                    title: "健保存摺".into()
                },
                &screen(None, Some(blank))
            ),
            TargetOnScreen::CannotTell {
                why: CannotTell::NothingOnScreen {
                    field: ScreenField::WindowTitle
                }
            },
            "{blank:?} 是「沒有標題」，不是「標題對不上」"
        );
    }
}

/// 這一步**本身**沒有給出目標：說不準，而且和「目標讀不懂」是兩句話。
#[test]
fn an_empty_ask_has_nothing_to_compare() {
    assert_eq!(
        target_on_screen(
            &ActionSnapshot::FocusWindow { title: "  ".into() },
            &screen(None, Some("健保存摺 — Chrome"))
        ),
        TargetOnScreen::CannotTell {
            why: CannotTell::NothingInTheAsk
        }
    );
}

// ───────────────────────── 使用者讀到的那句話 ─────────────────────────

/// 舊版寫的那些列**沒有比過**，句子要自己說出來，不可以沉默。
///
/// 沉默的話，一列 alpha.95 以前的紀錄讀起來會和一列「比過、說不準」一模一樣。
#[test]
fn a_row_written_before_this_version_says_it_was_never_checked() {
    let s = sentence(TargetOnScreen::CannotTell {
        why: CannotTell::NotChecked,
    });
    assert!(
        s.contains("舊版"),
        "舊版紀錄要講明它沒有比對過；實際印的是 {s}"
    );
}

/// 「這一列是舊版寫的」這句話**到得了**——而且到得了的路只有一條：
/// 一份 alpha.95 以前寫的 `action-log.jsonl`，那時候的 JSON 裡根本沒有
/// `target` 這個鍵。
///
/// 撐著這件事的是 `StepEvidence::After::target` 上的 `#[serde(default)]`。
/// 拿掉它，舊那一列會整列反序列化失敗——那些紀錄不是「少一句比對」，是
/// **整段歷史讀不出來**。這一條就是那個 attribute 的唯一一道針。
#[test]
fn a_log_line_from_before_this_version_still_reads_back() {
    const OLD: &str =
        r#"{"kind":"after","frame_id":12,"frame_at_ms":1700000000000,"has_image":true}"#;
    let back: StepEvidence = serde_json::from_str(OLD).expect(
        "alpha.94 寫的那一列現在讀不回來了——`target` 少了 `#[serde(default)]`，\
         使用者的整段動作紀錄會變成讀不懂",
    );
    assert_eq!(
        back,
        evidence(
            true,
            TargetOnScreen::CannotTell {
                why: CannotTell::NotChecked
            }
        ),
        "舊那一列要長成「沒有比過」，不是任何一種「比過了」"
    );
    assert!(
        back.message().contains("這一列是舊版寫的"),
        "而且它讀出來以後要自己說它是舊的；實際印的是 {}",
        back.message()
    );
}

/// 說不準的那句話，必須自己講出它證明不了什麼。
///
/// 這是這一輪的**招牌句子**：舊版那句「圖在。」讀起來像交差了，而它證明的只
/// 有「畫面變了」。
#[test]
fn cannot_tell_says_what_it_does_not_prove() {
    let s = sentence(TargetOnScreen::CannotTell {
        why: CannotTell::NothingOnScreen {
            field: ScreenField::Url,
        },
    });
    assert!(
        s.contains("不證明變成你要的樣子"),
        "說不準的時候要說清楚它證明不了什麼；實際印的是 {s}"
    );
    assert!(s.contains("網址"), "要說清楚是哪一欄沒記；實際印的是 {s}");
}

/// 對不上的那句話，要同時說出「看到什麼」和「本來要什麼」。
///
/// 只說一半的話，使用者沒辦法判斷是自己按錯還是她開錯。
#[test]
fn a_mismatch_names_both_sides() {
    let s = sentence(TargetOnScreen::Mismatched {
        field: ScreenField::Url,
        saw: "evil.example".into(),
        wanted: "example.com".into(),
    });
    assert!(
        s.contains("evil.example") && s.contains("example.com"),
        "對不上的時候兩邊都要說出來；實際印的是 {s}"
    );
}

/// **對得上那句話也要說出兩邊。** 視窗標題比的是「裡面有沒有這幾個字」，
/// 而句子只印 `saw` 的話會寫成「畫面的標題是 X」——把一次子字串命中講成相等。
///
/// 使用者要 `健保存摺`，畫面上是 `登入 — 健保存摺`：這確實算對得上（她到了
/// 那個站），但句子不可以讓人以為她已經進去了。
#[test]
fn a_title_match_says_which_words_it_matched() {
    let s = sentence(TargetOnScreen::Matched {
        field: ScreenField::WindowTitle,
        saw: "登入 — 健保存摺".into(),
        wanted: "健保存摺".into(),
    });
    assert!(
        s.contains("登入 — 健保存摺") && s.contains("「健保存摺」"),
        "對得上也要說清楚是「標題裡有這幾個字」，兩邊都要印；實際印的是 {s}"
    );
}

/// 對不上不可以讀起來像對得上。
///
/// 這一條是為了擋「三格共用一句話、只換一個名詞」那種修法：三句話拿去互相
/// 比對，一句都不准和另一句相同。
#[test]
fn the_three_verdicts_do_not_read_as_each_other() {
    let matched = sentence(TargetOnScreen::Matched {
        field: ScreenField::Url,
        saw: "example.com".into(),
        wanted: "example.com".into(),
    });
    let mismatched = sentence(TargetOnScreen::Mismatched {
        field: ScreenField::Url,
        saw: "evil.example".into(),
        wanted: "example.com".into(),
    });
    let cannot = sentence(TargetOnScreen::CannotTell {
        why: CannotTell::NothingOnScreen {
            field: ScreenField::Url,
        },
    });
    assert_ne!(matched, mismatched);
    assert_ne!(matched, cannot);
    assert_ne!(mismatched, cannot);
    assert!(
        !matched.contains("不證明變成你要的樣子"),
        "對得上那句不該掛著說不準的免責；實際印的是 {matched}"
    );
    assert!(
        !cannot.contains("沒有把握"),
        "說不準不是「她開錯了」，不可以借對不上那句話；實際印的是 {cannot}"
    );
}

/// 十個結局各有一句話：每一句都要說出自己那件事，而那個片語**不准出現在
/// 另外九句裡**。
///
/// **為什麼有這一條。** 其他測試斷言的是**型別**
/// （`Matched { field: ScreenField::WindowTitle, .. }`），而使用者讀的是
/// **句子**。加這一條之前，`message()` 的 arm 裡只有 `Matched{Url}`、
/// `Mismatched{Url}`、`NothingOnScreen{Url}`、`NotChecked` 有句子層的測試；
/// 視窗標題那三格只有型別層（三句可以被清空或互相對調而沒有人紅），而
/// `NothingInTheAsk` 一個字都沒有人碰。
///
/// 「片語只出現在自己那一句裡」比「十句兩兩不相等」硬：後者只擋得住整句
/// 複製，前者連「把對得上那句改成對不上的說法」都擋得住。
///
/// **這一條證明不了什麼：** 底下那個陣列是手寫的。`phrase()` 的窮舉
/// `match` 只對放進去的值求值，它擋不住「陣列少一格」；它擋得住的是「加了
/// 新的 variant 卻沒有人替它想句子」——那時候這一條會**編不過**，而那個
/// 編譯錯誤就落在陣列旁邊。（r29 收尾把 `CannotTell` 從三格拆成五格，就是
/// 這個編譯錯誤先攔下來的。）
#[test]
fn each_of_the_ten_endings_says_a_thing_the_others_do_not() {
    // 窮舉。加 `TargetOnScreen`／`CannotTell`／`ScreenField` 的 variant，
    // 這裡就編不過。
    fn phrase(target: &TargetOnScreen) -> &'static str {
        match target {
            TargetOnScreen::Matched {
                field: ScreenField::Url,
                ..
            } => "她比的是網站",
            TargetOnScreen::Matched {
                field: ScreenField::WindowTitle,
                ..
            } => "她比的是標題含不含",
            TargetOnScreen::Mismatched {
                field: ScreenField::Url,
                ..
            } => "不是你要開的",
            TargetOnScreen::Mismatched {
                field: ScreenField::WindowTitle,
                ..
            } => "裡面沒有",
            TargetOnScreen::CannotTell {
                why:
                    CannotTell::NothingOnScreen {
                        field: ScreenField::Url,
                    },
            } => "沒有探到那張畫面的網址欄",
            TargetOnScreen::CannotTell {
                why:
                    CannotTell::NothingOnScreen {
                        field: ScreenField::WindowTitle,
                    },
            } => "沒有探到那張畫面的視窗標題",
            TargetOnScreen::CannotTell {
                why: CannotTell::ScreenUrlUnreadable,
            } => "有記到東西",
            TargetOnScreen::CannotTell {
                why: CannotTell::NothingInTheAsk,
            } => "這一步本身沒有給出",
            TargetOnScreen::CannotTell {
                why: CannotTell::AskUrlUnreadable,
            } => "看不懂那個網域",
            TargetOnScreen::CannotTell {
                why: CannotTell::NotChecked,
            } => "這一列是舊版寫的",
        }
    }

    let all = [
        TargetOnScreen::Matched {
            field: ScreenField::Url,
            saw: "example.com".into(),
            wanted: "example.com".into(),
        },
        TargetOnScreen::Matched {
            field: ScreenField::WindowTitle,
            saw: "健保存摺 — Chrome".into(),
            wanted: "健保存摺".into(),
        },
        TargetOnScreen::Mismatched {
            field: ScreenField::Url,
            saw: "evil.example".into(),
            wanted: "example.com".into(),
        },
        TargetOnScreen::Mismatched {
            field: ScreenField::WindowTitle,
            saw: "收件匣 — Outlook".into(),
            wanted: "健保存摺".into(),
        },
        TargetOnScreen::CannotTell {
            why: CannotTell::NothingOnScreen {
                field: ScreenField::Url,
            },
        },
        TargetOnScreen::CannotTell {
            why: CannotTell::NothingOnScreen {
                field: ScreenField::WindowTitle,
            },
        },
        TargetOnScreen::CannotTell {
            why: CannotTell::ScreenUrlUnreadable,
        },
        TargetOnScreen::CannotTell {
            why: CannotTell::NothingInTheAsk,
        },
        TargetOnScreen::CannotTell {
            why: CannotTell::AskUrlUnreadable,
        },
        TargetOnScreen::CannotTell {
            why: CannotTell::NotChecked,
        },
    ];

    let said: Vec<String> = all.iter().cloned().map(sentence).collect();
    for (i, target) in all.iter().enumerate() {
        let needle = phrase(target);
        let hits: Vec<usize> = said
            .iter()
            .enumerate()
            .filter(|(_, s)| s.contains(needle))
            .map(|(j, _)| j)
            .collect();
        assert_eq!(
            hits,
            vec![i],
            "「{needle}」應該只出現在 {target:?} 那一句裡，實際出現在第 {hits:?} 句；\
             十句話是：{said:#?}"
        );
    }
}

/// 目標那一側的兩格，講的**不是同一件事**，而且分辨得出來。
///
/// `NothingInTheAsk` 是「這一步沒有給目標」；`AskUrlUnreadable` 是「給了、
/// 她看不懂」。合成一句的話，一個打了中文網域的使用者會讀到「這一步本身
/// 沒有給出可以拿去跟畫面比的目標」——他明明打了。
#[test]
fn an_unreadable_ask_does_not_read_as_no_ask() {
    let unreadable = sentence(TargetOnScreen::CannotTell {
        why: CannotTell::AskUrlUnreadable,
    });
    assert!(
        !unreadable.contains("這一步本身沒有給出"),
        "使用者給了目標，不可以說他沒給；實際印的是 {unreadable}"
    );
    assert!(
        !unreadable.contains("沒有把握"),
        "看不懂目標不是「她開錯了」；實際印的是 {unreadable}"
    );
}

/// 「她看不懂那個網域」**到得了**——而且不是靠一個壞掉的輸入。
///
/// 兩道閘門的判準不一樣，中間有一條縫：
///
/// - `target_policy::validate_url` 要求 `http(s):` ＋ 後面 `//` 非空，並擋掉
///   空白與控制字元；ASCII 不 ASCII 它不管。
/// - `segment::looks_like_host` 要求 host 有一個點（`localhost` 例外）**且**
///   整串都是 ASCII 英數／`.`／`-`。
///
/// 於是一個**中文／日文網域**過得了白名單、卻抽不出 host。那不是攻擊，是
/// 一個真的網址。
///
/// 這一條把「到得了」寫成**斷言**而不是寫成註解：兩件事在同一條測試裡量，
/// 哪天有人放寬 `looks_like_host`（或收緊 `validate_url`），這裡會紅，而不是
/// 留下一句沒有人再驗過的話。
#[test]
fn an_idn_host_passes_the_allowlist_but_leaves_nothing_to_compare() {
    const IDN: &str = "https://例え.jp/";
    assert!(
        sister_hands::target_policy::validate_url(IDN).is_ok(),
        "這一條的前提是它過得了白名單——過不了的話，上面那段話是假的"
    );
    assert_eq!(
        target_on_screen(&open(IDN), &screen(Some(IDN), None)),
        TargetOnScreen::CannotTell {
            why: CannotTell::AskUrlUnreadable
        },
        "目標抽不出網站名的時候要說「說不準」，不可以說她開錯了"
    );
}

/// 原本那句話的前半段不准被換掉，而且要留在**前面**。
///
/// frame 編號和時刻是這份報告存在的理由，加一句比對結果不可以把它們擠掉——
/// 也不可以把比對結果接到前面去，那樣句子會從一個逗號或句號開頭。
#[test]
fn the_frame_id_and_time_survive_and_stay_in_front() {
    // 時刻不比字面，比**這條軸有沒有在動**：只改 `frame_at_ms`、別的都不動，
    // 兩句話就必須不一樣。抄一份 `replay_copy::at` 的格式進來只會漂（而且
    // 那一支是私有的，硬開放等於為了測試放寬 API）。
    let one = evidence(
        true,
        TargetOnScreen::CannotTell {
            why: CannotTell::NotChecked,
        },
    )
    .message();
    let other = StepEvidence::After {
        frame_id: 12,
        frame_at_ms: 1_700_000_000_000 + 3_661_000, // 一小時零一分零一秒之後
        has_image: true,
        target: TargetOnScreen::CannotTell {
            why: CannotTell::NotChecked,
        },
    }
    .message();
    assert_ne!(
        one, other,
        "只有 frame_at_ms 不一樣，兩句話卻一模一樣——時刻沒有被印出來，\
         而這一條的名字宣稱守著它"
    );

    for target in [
        TargetOnScreen::Matched {
            field: ScreenField::Url,
            saw: "example.com".into(),
            wanted: "example.com".into(),
        },
        TargetOnScreen::Mismatched {
            field: ScreenField::Url,
            saw: "evil.example".into(),
            wanted: "example.com".into(),
        },
        TargetOnScreen::CannotTell {
            why: CannotTell::NotChecked,
        },
    ] {
        let s = sentence(target.clone());
        assert!(
            s.contains("frame #12"),
            "frame 編號不見了（{target:?}）；實際印的是 {s}"
        );
        assert!(
            s.starts_with("做完之後"),
            "比對結果被接到前面去了，句子從標點開頭（{target:?}）；實際印的是 {s}"
        );
    }
}

/// 有沒有截圖是兩句不同的話，而且不可以說反。
///
/// 「圖在」和「圖不在」差一個字，而使用者按圖找證據的時候，這一個字決定他
/// 要不要去翻資料夾。
#[test]
fn whether_the_screenshot_is_there_is_said_correctly() {
    let target = || TargetOnScreen::CannotTell {
        why: CannotTell::NotChecked,
    };
    let with = evidence(true, target()).message();
    let without = evidence(false, target()).message();
    assert!(
        with.contains("圖在") && !with.contains("圖不在"),
        "有截圖的時候要說圖在；實際印的是 {with}"
    );
    assert!(
        without.contains("圖不在"),
        "沒截圖的時候要說圖不在；實際印的是 {without}"
    );
    assert_ne!(with, without, "兩種情況不可以印出一模一樣的句子");
}
