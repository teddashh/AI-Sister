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
    StepEvidence::After {
        frame_id: 12,
        frame_at_ms: 1_700_000_000_000,
        has_image: true,
        target,
    }
    .message()
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
        }
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

/// 這台機器沒有記下那張畫面的網址（UIA 讀不到、不是瀏覽器、Linux）：說不準。
///
/// **這一格是預設情況，不是邊角料。** `frames.url` 只有 Windows 的 UIA 探得到
/// 的視窗才有值。
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

/// 欄位有值、但抽不出網站名（例如記到的是一句視窗標題）：一樣是說不準，
/// 不是對不上。
///
/// 這一條和上面那條分開寫，是因為最容易寫錯的方向是把它歸到 `Mismatched`
/// ——那會讓一次讀不到 UIA 被講成「她開錯了」。
#[test]
fn a_url_field_that_is_not_a_url_is_still_cannot_tell() {
    assert_eq!(
        target_on_screen(
            &open("https://example.com/a"),
            &screen(Some("SPEC.md — AI-Sister"), None)
        ),
        TargetOnScreen::CannotTell {
            why: CannotTell::NothingOnScreen {
                field: ScreenField::Url
            }
        }
    );
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

/// 對不上不可以讀起來像對得上。
///
/// 這一條是為了擋「三格共用一句話、只換一個名詞」那種修法：三句話拿去互相
/// 比對，一句都不准和另一句相同。
#[test]
fn the_three_verdicts_do_not_read_as_each_other() {
    let matched = sentence(TargetOnScreen::Matched {
        field: ScreenField::Url,
        saw: "example.com".into(),
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

/// 八個結局各有一句話：每一句都要說出自己那件事，而那個片語**不准出現在
/// 另外七句裡**。
///
/// **為什麼補這一條。** 上面那些測試斷言的是**型別**
/// （`Matched { field: ScreenField::WindowTitle, .. }`），而使用者讀的是
/// **句子**。我把 `message()` 的八個 arm 對回這一檔數了一次：
///
/// | 結局 | 加這一條之前 |
/// |---|---|
/// | `Matched{Url}`、`Mismatched{Url}`、`NothingOnScreen{Url}`、`NotChecked` | 有句子層的測試 |
/// | 視窗標題那三格 | 只有型別層——那三句可以被清空或互相對調而沒有人紅 |
/// | `NothingInTheAsk` | **一個字都沒有人碰** |
///
/// 「片語只出現在自己那一句裡」比「八句兩兩不相等」硬：後者只擋得住整句
/// 複製，前者連「把對得上那句改成對不上的說法」都擋得住。
///
/// **這一條證明不了什麼：** 底下那個八格陣列是手寫的。`phrase()` 的窮舉
/// `match` 只對放進去的值求值，它擋不住「陣列少一格」；它擋得住的是「加了
/// 新的 variant 卻沒有人替它想句子」——那時候這一條會**編不過**，而那個
/// 編譯錯誤就落在陣列旁邊。
#[test]
fn each_of_the_eight_endings_says_a_thing_the_other_seven_do_not() {
    // 窮舉。加 `TargetOnScreen`／`CannotTell`／`ScreenField` 的 variant，
    // 這裡就編不過。
    fn phrase(target: &TargetOnScreen) -> &'static str {
        match target {
            TargetOnScreen::Matched {
                field: ScreenField::Url,
                ..
            } => "而且那張畫面的網址就在",
            TargetOnScreen::Matched {
                field: ScreenField::WindowTitle,
                ..
            } => "而且那張畫面的視窗標題是",
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
            } => "沒有記下那張畫面的網址",
            TargetOnScreen::CannotTell {
                why:
                    CannotTell::NothingOnScreen {
                        field: ScreenField::WindowTitle,
                    },
            } => "沒有記下那張畫面的視窗標題",
            TargetOnScreen::CannotTell {
                why: CannotTell::NothingInTheAsk,
            } => "這一步的目標沒有可以拿去跟畫面比的東西",
            TargetOnScreen::CannotTell {
                why: CannotTell::NotChecked,
            } => "這一列是舊版寫的",
        }
    }

    let all = [
        TargetOnScreen::Matched {
            field: ScreenField::Url,
            saw: "example.com".into(),
        },
        TargetOnScreen::Matched {
            field: ScreenField::WindowTitle,
            saw: "健保存摺 — Chrome".into(),
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
            why: CannotTell::NothingInTheAsk,
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
             八句話是：{said:#?}"
        );
    }
}

/// 「這一步的目標沒有可以拿去跟畫面比的東西」**到得了**——而且不是靠一個
/// 壞掉的輸入。
///
/// 兩道閘門的判準不一樣，中間有一條縫：
///
/// - `target_policy::validate_url` 只要求 `http(s):` ＋ 後面 `//` 非空。
/// - `segment::looks_like_host` 要求 host 全部是 ASCII 英數／`.`／`-`。
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
            why: CannotTell::NothingInTheAsk
        },
        "目標抽不出網站名的時候要說「說不準」，不可以說她開錯了"
    );
}

/// 原本那句話的前半段不准被換掉。
///
/// frame 編號和時刻是這份報告存在的理由，加一句比對結果不可以把它們擠掉。
#[test]
fn the_frame_id_and_time_survive() {
    for target in [
        TargetOnScreen::Matched {
            field: ScreenField::Url,
            saw: "example.com".into(),
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
    }
}
