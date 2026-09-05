//! 拔手快捷鍵：**按下去之後，她到底還會不會交出東西。**
//!
//! 這一份是收貨考題，在派工出去的同一小時寫的，作者沒有看過成品。所以它只
//! 斷言**使用者摸得到的東西**：拔手開關現在是哪一邊、那句話有沒有把那一邊講
//! 對、系統匣那兩行字。一個字都不綁在修法上——`press_hands_hotkey` 內部怎麼
//! 排、`HandsHotkeyOutcome` 分幾格，這裡都不管。
//!
//! **這份考題的軸是「一致性」，不是「文案」。** 這個 repo 一路在修的病是
//! 「每一行都是真的，湊起來在說謊」，而拔手鍵最貴的那一種說謊是：寫檔失敗了，
//! 於是那句話說「沒能拔掉，她的手還接著」——可是 `is_pulled` 在同一個壞掉的
//! 資料夾上是 fail-closed 的，它回 `true`，`platform.rs` 那道最後檢查什麼都
//! 不會放行。兩句話各自都對，湊起來那個人得到的是相反的結論。
//!
//! 所以底下 `the_sentence_agrees_with_the_only_authority` 那一條把四種情境
//! 掃過去，每一種都問同一個問題：**這句話對「她會不會動」的說法，和
//! `is_pulled` 一不一致。**

use sister_hands::kill_switch::{
    self, hands_hotkey_message, is_pulled, plan_hotkeys, press_hands_hotkey, pulled_since,
    tray_hands_resume_label, tray_hands_stop_label,
};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

/// `root` 一定是一個真的目錄（收尾時整棵刪掉），`target` 才是餵給受測函式的
/// 那條路徑——它可能就是 `root`，也可能是 `root` 底下的一個**檔案**。
struct Tmp {
    root: PathBuf,
    target: PathBuf,
}

impl Tmp {
    fn dir(name: &str) -> Self {
        static N: AtomicU32 = AtomicU32::new(0);
        let root = std::env::temp_dir().join(format!(
            "sister-r30-{}-{name}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("建測試目錄");
        Self {
            target: root.clone(),
            root,
        }
    }

    /// 資料目錄的位置上放的是一個**檔案**。`pull` 寫不進去，而
    /// `is_pulled` 在這一格是 fail-closed 的（`dir_state` 的 `NotADir`）。
    fn file(name: &str) -> Self {
        let mut me = Self::dir(name);
        me.target = me.root.join("其實是一個檔案");
        std::fs::write(&me.target, b"").expect("建檔");
        me
    }

    fn path(&self) -> &Path {
        &self.target
    }
}

impl Drop for Tmp {
    fn drop(&mut self) {
        // 唯讀那一條把目錄權限拿掉了，不放回去就刪不掉。
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&self.root, std::fs::Permissions::from_mode(0o755));
        }
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

const T1: i64 = 1_700_000_000_000; // 2023-11-14
const T2: i64 = 1_700_003_600_000; // 一小時後

// ---------------------------------------------------------------------------
// 一、按下去，手就拔了
// ---------------------------------------------------------------------------

#[test]
fn pressing_the_hotkey_pulls_the_hands() {
    let tmp = Tmp::dir("press");
    assert!(!is_pulled(tmp.path()), "還沒按就已經是拔著的？");

    let outcome = press_hands_hotkey(Some(tmp.path()), T1);

    assert!(
        is_pulled(tmp.path()),
        "按了拔手熱鍵，`is_pulled` 還是說手接著——而 `platform.rs` 那道最後\
         檢查問的就是這一支。回的是 {outcome:?}"
    );
    assert_eq!(
        pulled_since(tmp.path()),
        Some(T1),
        "拔手時間不是按下去那一刻"
    );
}

/// **慌張連按不可以把手裝回去。**
///
/// 這一條是「單向」那個決定的牙齒。做成 toggle 的話這裡第 2、4 下會變綠燈，
/// 而使用者在螢幕上看不到任何差別——他按了五下，以為拔得更牢了。
#[test]
fn mashing_the_hotkey_never_puts_the_hands_back() {
    let tmp = Tmp::dir("mash");
    for i in 0..5 {
        let outcome = press_hands_hotkey(Some(tmp.path()), T1 + i);
        assert!(
            is_pulled(tmp.path()),
            "連按第 {} 下之後手竟然接回去了（{outcome:?}）",
            i + 1
        );
        let says = hands_hotkey_message(&outcome);
        assert!(
            !says.contains("接回去") && !says.contains("恢復"),
            "第 {} 下的那句話在講「接回去」，可是這顆鍵是單向的：{says}",
            i + 1
        );
    }
    assert_eq!(
        pulled_since(tmp.path()),
        Some(T1),
        "連按把第一次拔手的時間洗掉了"
    );
}

/// 第二下講的是**第一下**的時間。
///
/// 不比對格式（那是 `replay_copy::at` 的事，而且它是 `pub(crate)`，這份考題
/// 看不到）。改成讓**第一下的時間**變化、第二下的時間不變：句子如果取錯了
/// 那一個時戳，兩次跑出來的字會一模一樣。
#[test]
fn the_second_press_reports_when_it_was_first_pulled() {
    let say_after = |first: i64| {
        let tmp = Tmp::dir("since");
        press_hands_hotkey(Some(tmp.path()), first);
        hands_hotkey_message(&press_hands_hotkey(Some(tmp.path()), T2))
    };

    let early = say_after(T1);
    let late = say_after(T2 - 1);

    assert_ne!(
        early, late,
        "兩次的第一下差了一小時，第二下的那句話卻一字不差——它報的是**這一下**\
         的時間，不是手被拔掉的那一刻。句子：{early}"
    );
    for says in [&early, &late] {
        assert!(
            !says.contains("手拔掉了"),
            "本來就拔著，這句話卻宣告「手拔掉了」，讀起來像這一下才生效：{says}"
        );
    }
}

// ---------------------------------------------------------------------------
// 二、那句話和唯一的權威一致
// ---------------------------------------------------------------------------

/// **整份考題的核心。**
///
/// 「她現在會不會把東西交給作業系統」只有一個權威：`kill_switch::is_pulled`
/// ——`platform.rs` 交出去之前的最後一刻問的就是它。所以那句話對這件事的說法
/// 必須跟它一致，不管 `pull` 這個**寫入**動作成功還是失敗。
///
/// 最容易寫錯的是第三格：資料目錄的位置上放著一個檔案。`pull` 一定失敗，
/// 於是很自然會寫「沒能拔掉，她的手還接著」——可是 `is_pulled` 在那一格是
/// fail-closed 的，它回 `true`，她一個動作都交不出去。那句話會讓他跑去做
/// 一件已經不需要做的事，而且以為自己還在危險裡。
#[test]
fn the_sentence_agrees_with_the_only_authority() {
    let clean = Tmp::dir("agree-clean");
    let already = Tmp::dir("agree-already");
    kill_switch::pull(already.path(), T1).expect("先拔起來");
    let not_a_dir = Tmp::file("agree-notdir");

    let cases: Vec<(&str, Option<&Path>)> = vec![
        ("乾淨的資料目錄", Some(clean.path())),
        ("本來就拔著", Some(already.path())),
        ("資料目錄的位置上是一個檔案", Some(not_a_dir.path())),
        ("問不出資料目錄", None),
    ];

    for (what, dir) in cases {
        let outcome = press_hands_hotkey(dir, T2);
        let says = hands_hotkey_message(&outcome);
        // `None` 沒有可以問的路徑；那一格她確實還接著（沒有任何開關被寫下）。
        let stopped = dir.map(is_pulled).unwrap_or(false);
        let claims_attached = says.contains("還接著");

        assert_eq!(
            claims_attached,
            !stopped,
            "「{what}」這一格：`is_pulled` 說 {stopped}，而那句話{}說手還接著。\
             這兩件事是同一件事，不可以各講各的。\n  句子：{says}\n  回的是：{outcome:?}",
            if claims_attached { "" } else { "沒有" }
        );
        assert!(
            !says.trim().is_empty(),
            "「{what}」這一格一個字都沒說（{outcome:?}）"
        );
    }
}

/// 說「還接著」的時候，要告訴他還有哪兩條路。
///
/// 一句只講失敗、不講下一步的話，等於把他留在原地——而他按這顆鍵的那一刻
/// 正在出事。
#[test]
fn a_hotkey_that_could_not_pull_names_the_other_two_ways() {
    let says = hands_hotkey_message(&press_hands_hotkey(None, T1));
    assert!(
        says.contains("還接著"),
        "沒有資料目錄卻不說手還接著：{says}"
    );
    assert!(
        says.contains("sister hands stop"),
        "沒告訴他終端機那條路：{says}"
    );
    assert!(
        says.contains("拔掉她的手"),
        "沒告訴他系統匣那顆的字（要和選單上寫的一模一樣，他才找得到）：{says}"
    );
}

/// 寫不進去、而且 `is_pulled` 真的還是「接著」的那一格。
///
/// 唯一在 Linux 上做得出來的重現：把資料目錄本身改成唯讀。`create_new` 會被
/// 權限擋掉，但 `try_exists` 走得進去（`r-x`），所以 `is_pulled` 回 `false`
/// ——這一格她**真的**還接著，那句話說得對。
///
/// 和上面那一格的差別正是這份考題要守的東西：**兩種「拔不掉」，兩句不同的話。**
#[cfg(unix)]
#[test]
fn a_read_only_data_dir_really_does_leave_the_hands_attached() {
    // 這個名字只有這一格用得到，而這一格在 Windows 上編不進來——放在檔頭
    // 的話，Windows target 的 clippy 會因為 `unused_imports` 直接紅。
    use sister_hands::kill_switch::HandsHotkeyOutcome;
    use std::os::unix::fs::PermissionsExt;
    let tmp = Tmp::dir("readonly");
    std::fs::set_permissions(tmp.path(), std::fs::Permissions::from_mode(0o555)).expect("設成唯讀");

    let outcome = press_hands_hotkey(Some(tmp.path()), T1);
    let says = hands_hotkey_message(&outcome);

    // root 底下權限擋不住，那就沒有東西可以驗——跳過，不要假綠。
    if is_pulled(tmp.path()) {
        eprintln!("跳過：這個環境寫得進唯讀目錄（root？）");
        return;
    }
    assert!(
        says.contains("還接著"),
        "唯讀目錄拔不掉，而 `is_pulled` 說手還接著，這句話卻沒講：{says}\n{outcome:?}"
    );
    assert!(
        !matches!(outcome, HandsHotkeyOutcome::Pulled { .. }),
        "什麼都沒寫進去，卻回報拔掉了：{outcome:?}"
    );
}

// ---------------------------------------------------------------------------
// 三、系統匣那兩行字
// ---------------------------------------------------------------------------

/// 兩顆鍵，兩行字，而且**現在按了沒事的那一顆要自己講出來**。
///
/// 這是 `set_record_labels` 上面那條既有的規矩：系統匣的字問的是「按下去會
/// 發生什麼」。現在那兩顆的字是寫死的，所以終端機打完 `sister hands stop`
/// 之後，選單還在說「把手接回去（現在沒拔）」——而那是假的。
#[test]
fn the_tray_lines_track_which_side_we_are_on() {
    let tmp = Tmp::dir("tray");

    let stop_before = tray_hands_stop_label(tmp.path());
    let resume_before = tray_hands_resume_label(tmp.path());
    kill_switch::pull(tmp.path(), T1).expect("拔");
    let stop_after = tray_hands_stop_label(tmp.path());
    let resume_after = tray_hands_resume_label(tmp.path());

    assert_ne!(
        (&stop_before, &resume_before),
        (&stop_after, &resume_after),
        "拔掉前後，系統匣那兩行字一模一樣——那兩行沒有在看現在是哪一邊"
    );
    for (side, stop, resume) in [
        ("沒拔", &stop_before, &resume_before),
        ("拔著", &stop_after, &resume_after),
    ] {
        assert_ne!(
            stop, resume,
            "「{side}」的時候兩顆的字一樣，選單上分不出哪一顆是哪一顆"
        );
        assert!(
            stop.starts_with("拔掉她的手"),
            "「{side}」：第一顆的字不是從「拔掉她的手」開頭，他認不出這是同一顆：{stop}"
        );
        assert!(
            resume.starts_with("把手接回去"),
            "「{side}」：第二顆的字不是從「把手接回去」開頭：{resume}"
        );
    }
    assert!(
        stop_after.contains("已經拔了"),
        "已經拔著了，「拔掉她的手」那一顆沒說這件事：{stop_after}"
    );
    assert!(
        resume_before.contains("沒拔"),
        "現在沒拔，「把手接回去」按下去什麼都不會發生，那一顆沒說：{resume_before}"
    );
}

// ---------------------------------------------------------------------------
// 四、兩顆鍵怎麼分
// ---------------------------------------------------------------------------

/// 兩個欄位設成同一組的時候，**讓掉的必須是暫停那顆。**
///
/// 搶不到暫停鍵，你只是要多走一趟系統匣；搶不到拔手鍵，你是在出事的那一刻
/// 沒有那顆鍵。而且這件事不可以安靜發生——`collided` 要說得出是哪一組。
#[test]
fn when_both_hotkeys_are_the_same_combination_the_kill_switch_wins() {
    let plan = plan_hotkeys("Ctrl+Alt+H", "Ctrl+Alt+H");
    assert_eq!(
        plan.hands.as_deref(),
        Some("Ctrl+Alt+H"),
        "撞號的時候拔手那顆被讓掉了"
    );
    assert_eq!(plan.pause, None, "撞號的時候暫停那顆還是去搶了");
    assert_eq!(
        plan.collided.as_deref(),
        Some("Ctrl+Alt+H"),
        "撞號了卻沒有任何東西記下來——設定頁和 log 都不會提"
    );

    // 只差前後空白算同一組。
    let padded = plan_hotkeys("  Ctrl+Alt+H  ", "Ctrl+Alt+H");
    assert_eq!(padded.pause, None, "只差空白就被當成兩組不同的熱鍵");
}

#[test]
fn two_different_combinations_both_get_registered() {
    let plan = plan_hotkeys("Ctrl+Alt+P", "Ctrl+Alt+H");
    assert_eq!(plan.pause.as_deref(), Some("Ctrl+Alt+P"));
    assert_eq!(plan.hands.as_deref(), Some("Ctrl+Alt+H"));
    assert_eq!(plan.collided, None, "沒撞號卻報了撞號");
}

/// 空字串是**關掉**，不是壞掉的設定。全域熱鍵會從所有程式手上把那個組合搶
/// 走，所以要留一條關得掉的路——這條規矩 `pause_shortcut` 的 doc 已經寫了，
/// 新那一顆照抄。
#[test]
fn an_empty_string_turns_that_hotkey_off_without_touching_the_other() {
    let no_pause = plan_hotkeys("", "Ctrl+Alt+H");
    assert_eq!(no_pause.pause, None);
    assert_eq!(
        no_pause.hands.as_deref(),
        Some("Ctrl+Alt+H"),
        "關掉暫停鍵把拔手鍵一起關掉了"
    );
    assert_eq!(
        no_pause.collided, None,
        "兩邊都空才算撞號？空的不是一組熱鍵"
    );

    let no_hands = plan_hotkeys("Ctrl+Alt+P", "   ");
    assert_eq!(no_hands.pause.as_deref(), Some("Ctrl+Alt+P"));
    assert_eq!(no_hands.hands, None);

    let neither = plan_hotkeys("", "");
    assert_eq!(
        (neither.pause, neither.hands, neither.collided),
        (None, None, None)
    );
}

/// 設定檔裡**沒有**新那一行的時候，讀出來要是預設值，而且別的欄位不受影響。
///
/// `Config` 是 `deny_unknown_fields`，所以加欄位是往回相容的那個方向；但這件事
/// 值得有一條測試，因為反過來（拿掉欄位）會讓所有舊設定檔讀不進去。
#[test]
fn an_old_config_without_the_new_line_still_loads() {
    let tmp = Tmp::dir("config");
    let path = tmp.path().join("config.toml");
    std::fs::write(
        &path,
        "[shell]\npause_shortcut = \"Ctrl+Alt+S\"\ndeveloper_mode = true\n",
    )
    .expect("寫設定檔");

    let config = sister_core::config::Config::load(&path).expect("讀得回來");
    assert_eq!(config.shell.pause_shortcut, "Ctrl+Alt+S", "舊欄位被動到了");
    assert!(config.shell.developer_mode, "舊欄位被動到了");
    assert!(
        !config.shell.hands_stop_shortcut.trim().is_empty(),
        "沒寫新那一行的舊設定檔，拔手熱鍵變成關掉的——那不是他選的"
    );
}
