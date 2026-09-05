//! 拔手開關：兩個行程之間唯一的那條線。
//!
//! `sister do` 可能卡在等回答，桌面 tray 和另一個終端機仍要能立刻讓下一個動作
//! 交不出去。這個 repo 沒有獨立的 hands process，所以不能靠殺行程；一個放在
//! data dir 的檔案，是所有執行隘口都看得到、模型碰不到、行程當掉後仍保留的牆。
//!
//! 三條規則跟錄製暫停相同：不確定就是拔掉、不會自己過期、第一次拔掉的時間不能
//! 被重按洗掉。判定只看檔案在不在；內容只是顯示用，壞掉仍然算拔掉。

use std::path::{Path, PathBuf};

const SWITCH: &str = "hands.stop";

pub fn switch_path(data_dir: &Path) -> PathBuf {
    data_dir.join(SWITCH)
}

/// data dir 本人的狀態。抽出來是因為 Windows 會把「父路徑是檔案」的子路徑
/// `try_exists` 回成 `Ok(false)`，Linux 卻回 `Err(NotADirectory)`；若只測 IO
/// 薄殼，Linux 永遠站不到 Windows 出貨時的那一格。
///
/// 這份判定刻意和 `sister-core::pause` 各留一份：兩個 crate 不能互相形成正常相依。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum DirState {
    Dir,
    NotADir,
    Absent,
    Unreadable,
}

fn dir_state(data_dir: &Path) -> DirState {
    match std::fs::metadata(data_dir) {
        Ok(metadata) if metadata.is_dir() => DirState::Dir,
        Ok(_) => DirState::NotADir,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => DirState::Absent,
        Err(_) => DirState::Unreadable,
    }
}

/// `child` 是 `data_dir/hands.stop` 的 `try_exists` 答案；錯誤內容在這一步不重要。
fn decide(child: Result<bool, ()>, dir: DirState) -> bool {
    match child {
        Ok(true) | Err(()) => true,
        Ok(false) => match dir {
            DirState::Dir | DirState::Absent => false,
            DirState::NotADir | DirState::Unreadable => true,
        },
    }
}

fn decide_for(data_dir: &Path, child: Result<bool, ()>) -> bool {
    decide(child, dir_state(data_dir))
}

/// **這一行沒有任何 Linux 測試守得住，別照 Linux 的綠燈改它。**
///
/// 把它改回 `switch_path(data_dir).try_exists().unwrap_or(true)`，Linux 上
/// 47 條測試全綠——實測過。原因是 child 查詢要穿過 data dir 本人，所以只要
/// data dir 不是可走的目錄，那個查詢在 Linux 上一定先回 `Err`，兩種寫法就
/// 觀察不出差別。Windows 不是這樣：它把同一個情境回成 `Ok(false)`，於是
/// 舊寫法會說「我確定開關不在」——fail-open，而且錯在最不該錯的方向。
///
/// 真正守住這一行的是 `unreadable_path_is_pulled_fail_closed`，而它只有在
/// Windows CI 上才走得到那一格（alpha.75 就是被它擋下來的）。下面那些
/// `decide` 的單元測試證明的是規則對，不是這一行有照著規則走。
pub fn is_pulled(data_dir: &Path) -> bool {
    decide_for(data_dir, switch_path(data_dir).try_exists().map_err(|_| ()))
}

pub fn pull(data_dir: &Path, at_ms: i64) -> std::io::Result<bool> {
    let path = switch_path(data_dir);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    use std::io::Write;
    match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
    {
        Ok(mut file) => {
            file.write_all(at_ms.to_string().as_bytes())?;
            Ok(true)
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => Ok(false),
        Err(error) => Err(error),
    }
}

pub fn release(data_dir: &Path) -> std::io::Result<bool> {
    match std::fs::remove_file(switch_path(data_dir)) {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    }
}

pub fn pulled_since(data_dir: &Path) -> Option<i64> {
    std::fs::read_to_string(switch_path(data_dir))
        .ok()?
        .trim()
        .parse()
        .ok()
}

/// 寫不下那個旗標的原因。粒度只有兩格，因為他做得出來的下一步只有兩種。
///
/// **這裡刻意不含 `io::Error` 的原文。** 這幾個字會被拼進一句出現在中文介面
/// 上的話，而 `io::Error::to_string()` 是英文的——資料目錄的位置上放著一個
/// 檔案時實測拿到的是 `File exists (os error 17)`，那串字對按下熱鍵的人沒有
/// 任何用處。原文走 [`HandsHotkeyOutcome::NotWritten::os_error`]，只進 log。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WhyNotWritten {
    /// 這個行程問不出資料目錄在哪。
    NoDataDir,
    /// 資料目錄寫不進去：權限、唯讀、被別的程式鎖著、或者那條路上根本不是
    /// 一個資料夾。
    CannotWrite,
}

impl WhyNotWritten {
    /// 拼進句子裡的那一段。後面會接「，⋯⋯」，所以這裡不帶標點。
    pub fn zh(self) -> &'static str {
        match self {
            Self::NoDataDir => "問不出資料目錄在哪",
            Self::CannotWrite => "資料目錄寫不進去",
        }
    }
}

/// 按下拔手熱鍵之後，真的發生了什麼。
///
/// 分這幾格是因為使用者的下一步不一樣，而不是因為好看。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HandsHotkeyOutcome {
    /// 這一下真的把手拔掉了。
    Pulled { at_ms: i64 },
    /// 本來就拔著；時間是第一次拔的時間。
    AlreadyPulled { since_ms: Option<i64> },
    /// 那個旗標**沒有**被寫下來。
    NotWritten {
        why: WhyNotWritten,
        /// 寫失敗之後**再問一次** [`is_pulled`] 的答案。
        ///
        /// **「寫成功了沒」和「她會不會動」是兩個問題，而且答案會相反。**
        /// 交出去之前的最後一刻問的是 [`is_pulled`]（`platform.rs`），而它
        /// 是 fail-closed 的：資料目錄的位置上放著一個檔案的時候 `pull` 一定
        /// 失敗，`is_pulled` 卻回 `true`，她一個動作都交不出去。那一格說
        /// 「她的手還接著」是假話，而且錯在最貴的方向——他會跑去做一件已經
        /// 不必做的事，同時以為自己還在危險裡。
        stopped: bool,
        /// 作業系統的原文。**只給 log，永遠不要拼進句子。** 見 [`WhyNotWritten`]。
        os_error: Option<String>,
    },
}

pub fn press_hands_hotkey(data_dir: Option<&Path>, at_ms: i64) -> HandsHotkeyOutcome {
    let Some(data_dir) = data_dir else {
        return HandsHotkeyOutcome::NotWritten {
            why: WhyNotWritten::NoDataDir,
            // 沒有路徑就沒有東西可以問 `is_pulled`。倒向「她還會動」，因為那
            // 是要他再去做一件事的方向；把沒停說成停了，是這顆鍵唯一不能犯
            // 的錯。
            stopped: false,
            os_error: None,
        };
    };
    match pull(data_dir, at_ms) {
        Ok(true) => HandsHotkeyOutcome::Pulled { at_ms },
        Ok(false) => HandsHotkeyOutcome::AlreadyPulled {
            since_ms: pulled_since(data_dir),
        },
        Err(error) => HandsHotkeyOutcome::NotWritten {
            why: WhyNotWritten::CannotWrite,
            stopped: is_pulled(data_dir),
            os_error: Some(error.to_string()),
        },
    }
}

/// 按完之後對他說的那一句。
pub fn hands_hotkey_message(outcome: &HandsHotkeyOutcome) -> String {
    match outcome {
        HandsHotkeyOutcome::Pulled { .. } => "手拔掉了。她現在什麼都不會交給作業系統。".into(),
        HandsHotkeyOutcome::AlreadyPulled { since_ms: Some(t) } => {
            format!("手本來就是拔著的（從 {} 起）。", crate::replay_copy::at(*t))
        }
        HandsHotkeyOutcome::AlreadyPulled { since_ms: None } => {
            "手本來就是拔著的（拔手時間讀不到）。".into()
        }
        // 沒寫成，可是她照樣停著。不講那個開關的人會以為自己白按了一下。
        HandsHotkeyOutcome::NotWritten {
            why,
            stopped: true,
            ..
        } => format!(
            "{}，那個開關沒寫下來。不過她現在讀到的狀態就是「拔著」，一樣什麼都不會交給作業系統。",
            why.zh()
        ),
        HandsHotkeyOutcome::NotWritten {
            why,
            stopped: false,
            ..
        } => format!(
            "{}，沒能拔掉。她的手還接著——去系統匣按『拔掉她的手』，或在終端機打 `sister hands stop`。",
            why.zh()
        ),
    }
}

/// 這一輪要真的去搶哪幾組。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HotkeyPlan {
    pub pause: Option<String>,
    pub hands: Option<String>,
    pub collided: Option<String>,
}

/// 排兩顆熱鍵；拔手撞號時優先，因為暫停仍可從系統匣操作。
///
/// 只 trim 後逐字比較，不假裝維護 Tauri 的別名表。因此 `Ctrl+Alt+P` 和
/// `Control+Alt+P` 的撞號認不出來，其中一顆會在註冊當下回報失敗。
pub fn plan_hotkeys(pause: &str, hands: &str) -> HotkeyPlan {
    let pause = nonempty(pause);
    let hands = nonempty(hands);
    if pause.is_some() && pause == hands {
        return HotkeyPlan {
            pause: None,
            hands: hands.clone(),
            collided: hands,
        };
    }
    HotkeyPlan {
        pause,
        hands,
        collided: None,
    }
}

fn nonempty(value: &str) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_string())
}

/// 系統匣「拔掉她的手」那一顆的字。
pub fn tray_hands_stop_label(data_dir: &Path) -> String {
    if !is_pulled(data_dir) {
        return "拔掉她的手".into();
    }
    match pulled_since(data_dir) {
        Some(t) => format!(
            "拔掉她的手（已經拔了，從 {} 起）",
            crate::replay_copy::at(t)
        ),
        None => "拔掉她的手（已經拔了）".into(),
    }
}

/// 系統匣「把手接回去」那一顆的字。
pub fn tray_hands_resume_label(data_dir: &Path) -> String {
    if is_pulled(data_dir) {
        "把手接回去".into()
    } else {
        "把手接回去（現在沒拔）".into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    struct Tmp(PathBuf);
    impl Tmp {
        fn new(name: &str) -> Self {
            static N: AtomicU32 = AtomicU32::new(0);
            let dir = std::env::temp_dir().join(format!(
                "sister-hands-switch-{}-{name}-{}",
                std::process::id(),
                N.fetch_add(1, Ordering::Relaxed)
            ));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }
    }
    impl Drop for Tmp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
            let _ = std::fs::remove_file(&self.0);
        }
    }

    /// **這是整個 repo 唯一真的走過 `is_pulled` 的 Windows 那一格的測試。**
    /// 在 Linux 它從 `Err(NotADirectory)` 那條路過關，證不了什麼；在 Windows
    /// 它從 `Ok(false)` 那條路過關，而那正是 alpha.75 之前會 fail-open 的格子
    /// ——這條測試就是在 Windows CI 上把那個 bug 擋下來的人。別因為下面的
    /// `decide` 單元測試看起來涵蓋一樣的規則就刪掉它，那些測試碰不到薄殼。
    #[test]
    fn unreadable_path_is_pulled_fail_closed() {
        let tmp = Tmp::new("fail-closed");
        std::fs::remove_dir_all(&tmp.0).unwrap();
        std::fs::write(&tmp.0, "not a directory").unwrap();
        assert!(is_pulled(&tmp.0));
    }

    #[test]
    fn windows_child_missing_with_non_directory_parent_is_fail_closed() {
        // Linux 的 child `try_exists` 會先回 Err，跑不出 Windows 的 Ok(false) 組合。
        assert!(decide(Ok(false), DirState::NotADir));
    }

    #[test]
    fn absent_data_dir_is_not_pulled() {
        assert!(!decide(Ok(false), DirState::Absent));
    }

    #[test]
    fn directory_without_switch_is_not_pulled() {
        assert!(!decide(Ok(false), DirState::Dir));
    }

    #[test]
    fn present_switch_and_unreadable_child_are_pulled() {
        assert!(decide(Ok(true), DirState::NotADir));
        assert!(decide(Err(()), DirState::Dir));
    }

    #[test]
    fn shell_maps_real_paths_to_the_right_states() {
        let tmp = Tmp::new("shell-states");
        assert!(!is_pulled(&tmp.0));

        let absent = tmp.0.join("absent");
        assert!(!is_pulled(&absent));

        let file = tmp.0.join("file");
        std::fs::write(&file, "not a directory").unwrap();
        assert!(is_pulled(&file));
        // Windows 對這個真實路徑的 child 查詢回 Ok(false)；Linux 無法自然產生，
        // 所以在 IO 邊界明確餵入該答案，並仍由薄殼讀取真實 data dir 狀態。
        assert!(decide_for(&file, Ok(false)));
    }

    #[test]
    fn pulling_twice_does_not_replace_the_first_timestamp() {
        let tmp = Tmp::new("idempotent");
        assert!(pull(&tmp.0, 1000).unwrap());
        assert!(!pull(&tmp.0, 9999).unwrap());
        assert_eq!(pulled_since(&tmp.0), Some(1000));
    }

    #[test]
    fn release_distinguishes_removed_from_already_attached() {
        let tmp = Tmp::new("release");
        pull(&tmp.0, 1000).unwrap();
        assert!(release(&tmp.0).unwrap());
        assert!(!release(&tmp.0).unwrap());
    }

    #[test]
    fn hotkey_pulls_a_clean_directory() {
        let tmp = Tmp::new("hotkey-pull");
        assert_eq!(
            press_hands_hotkey(Some(&tmp.0), 123),
            HandsHotkeyOutcome::Pulled { at_ms: 123 }
        );
        assert!(is_pulled(&tmp.0));
    }

    #[test]
    fn hotkey_twice_preserves_the_first_timestamp() {
        let tmp = Tmp::new("hotkey-twice");
        press_hands_hotkey(Some(&tmp.0), 123);
        assert_eq!(
            press_hands_hotkey(Some(&tmp.0), 999),
            HandsHotkeyOutcome::AlreadyPulled {
                since_ms: Some(123)
            }
        );
    }

    #[test]
    fn hotkey_without_data_dir_writes_nothing() {
        let tmp = Tmp::new("hotkey-none");
        let before = std::fs::read_dir(&tmp.0).unwrap().count();
        assert_eq!(
            press_hands_hotkey(None, 123),
            HandsHotkeyOutcome::NotWritten {
                why: WhyNotWritten::NoDataDir,
                stopped: false,
                os_error: None,
            }
        );
        assert_eq!(std::fs::read_dir(&tmp.0).unwrap().count(), before);
    }

    /// 資料目錄的位置上放著一個**檔案**：`pull` 一定失敗，而 `is_pulled` 對
    /// 那一格是 fail-closed 的。
    ///
    /// **這一條的重點是那兩個斷言擺在一起。** 這個測試的前一版只斷言了
    /// 「回的是失敗那一格」和 `is_pulled` 為真——兩句都對，可是它們湊起來
    /// 說的是「寫失敗了，而她已經停了」，而當時的句子寫著「她的手還接著」。
    /// 所以現在把 `stopped` 一起釘住：它是**同一件事**的唯一權威。
    #[test]
    fn a_data_dir_that_is_a_file_cannot_be_written_but_still_stops_her() {
        let tmp = Tmp::new("hotkey-failed");
        std::fs::remove_dir_all(&tmp.0).unwrap();
        std::fs::write(&tmp.0, "file").unwrap();
        let outcome = press_hands_hotkey(Some(&tmp.0), 123);
        assert!(
            matches!(
                &outcome,
                HandsHotkeyOutcome::NotWritten {
                    why: WhyNotWritten::CannotWrite,
                    stopped: true,
                    os_error: Some(_),
                }
            ),
            "{outcome:?}"
        );
        assert!(is_pulled(&tmp.0), "fail-closed 那一格不成立了，前提變了");
        let _ = std::fs::remove_file(&tmp.0);
    }

    /// 作業系統的原文**不可以**出現在那句話裡。
    ///
    /// `io::Error::to_string()` 是英文的（上面那一格實測是
    /// `File exists (os error 17)`），而這句話會被畫在中文介面上。
    #[test]
    fn the_sentence_never_carries_the_operating_systems_own_words() {
        for stopped in [true, false] {
            let says = hands_hotkey_message(&HandsHotkeyOutcome::NotWritten {
                why: WhyNotWritten::CannotWrite,
                stopped,
                os_error: Some("File exists (os error 17)".into()),
            });
            assert!(
                !says.contains("os error") && !says.contains("File exists"),
                "作業系統的原文漏進中文句子了（stopped={stopped}）：{says}"
            );
        }
    }

    #[test]
    fn pulled_hotkey_message_claims_the_completed_fact() {
        assert_eq!(
            hands_hotkey_message(&HandsHotkeyOutcome::Pulled { at_ms: 1 }),
            "手拔掉了。她現在什麼都不會交給作業系統。"
        );
    }

    #[test]
    fn already_pulled_hotkey_message_keeps_the_first_time() {
        assert_eq!(
            hands_hotkey_message(&HandsHotkeyOutcome::AlreadyPulled { since_ms: Some(1) }),
            format!("手本來就是拔著的（從 {} 起）。", crate::replay_copy::at(1))
        );
    }

    #[test]
    fn already_pulled_hotkey_message_admits_an_unreadable_time() {
        assert_eq!(
            hands_hotkey_message(&HandsHotkeyOutcome::AlreadyPulled { since_ms: None }),
            "手本來就是拔著的（拔手時間讀不到）。"
        );
    }

    #[test]
    fn no_data_dir_message_says_the_hands_are_still_attached() {
        let message = hands_hotkey_message(&HandsHotkeyOutcome::NotWritten {
            why: WhyNotWritten::NoDataDir,
            stopped: false,
            os_error: None,
        });
        assert!(message.contains("還接著"), "{message}");
        assert!(!message.contains("拔掉了。"), "{message}");
        assert!(message.contains("問不出資料目錄在哪"), "{message}");
    }

    #[test]
    fn write_failure_message_says_the_hands_are_still_attached() {
        let message = hands_hotkey_message(&HandsHotkeyOutcome::NotWritten {
            why: WhyNotWritten::CannotWrite,
            stopped: false,
            os_error: Some("磁碟滿".into()),
        });
        assert!(message.contains("還接著"), "{message}");
        assert!(!message.contains("拔掉了。"), "{message}");
    }

    /// 同一個 `why`，`stopped` 兩邊要說**相反**的事。
    ///
    /// 只驗一邊的話，把 `stopped` 整個忽略掉（兩臂共用一句）也是綠的——而
    /// 那正是這一輪修掉的那個 bug。
    #[test]
    fn the_two_halves_of_a_failed_write_do_not_share_a_sentence() {
        let says = |stopped| {
            hands_hotkey_message(&HandsHotkeyOutcome::NotWritten {
                why: WhyNotWritten::CannotWrite,
                stopped,
                os_error: None,
            })
        };
        let attached = says(false);
        let stopped = says(true);
        assert_ne!(attached, stopped, "兩臂講同一句話，`stopped` 等於沒人讀");
        assert!(attached.contains("還接著"), "{attached}");
        assert!(
            !stopped.contains("還接著"),
            "她已經停了，這句話還在說手接著：{stopped}"
        );
        assert!(
            stopped.contains("什麼都不會交給作業系統"),
            "沒告訴他「其實已經停了」，他會白跑一趟：{stopped}"
        );
    }

    #[test]
    fn hotkey_plan_handles_enabled_and_disabled_sides() {
        assert_eq!(
            plan_hotkeys("P", "H"),
            HotkeyPlan {
                pause: Some("P".into()),
                hands: Some("H".into()),
                collided: None
            }
        );
        assert_eq!(
            plan_hotkeys("P", ""),
            HotkeyPlan {
                pause: Some("P".into()),
                hands: None,
                collided: None
            }
        );
        assert_eq!(
            plan_hotkeys("", "H"),
            HotkeyPlan {
                pause: None,
                hands: Some("H".into()),
                collided: None
            }
        );
        assert_eq!(
            plan_hotkeys(" ", ""),
            HotkeyPlan {
                pause: None,
                hands: None,
                collided: None
            }
        );
    }

    #[test]
    fn hotkey_plan_gives_collisions_to_hands_after_trimming() {
        for (pause, hands) in [("P", "P"), (" P ", "P")] {
            assert_eq!(
                plan_hotkeys(pause, hands),
                HotkeyPlan {
                    pause: None,
                    hands: Some("P".into()),
                    collided: Some("P".into())
                }
            );
        }
    }

    #[test]
    fn hotkey_plan_does_not_guess_aliases() {
        assert_eq!(plan_hotkeys("Ctrl+Alt+P", "Control+Alt+P").collided, None);
    }

    #[test]
    fn tray_hands_labels_cover_attached_and_pulled_states() {
        let tmp = Tmp::new("tray-labels");
        assert_eq!(tray_hands_stop_label(&tmp.0), "拔掉她的手");
        assert_eq!(tray_hands_resume_label(&tmp.0), "把手接回去（現在沒拔）");
        pull(&tmp.0, 1).unwrap();
        assert_eq!(
            tray_hands_stop_label(&tmp.0),
            format!(
                "拔掉她的手（已經拔了，從 {} 起）",
                crate::replay_copy::at(1)
            )
        );
        assert_eq!(tray_hands_resume_label(&tmp.0), "把手接回去");
        std::fs::write(switch_path(&tmp.0), "broken").unwrap();
        assert_eq!(tray_hands_stop_label(&tmp.0), "拔掉她的手（已經拔了）");
        assert_eq!(tray_hands_resume_label(&tmp.0), "把手接回去");
        release(&tmp.0).unwrap();
    }

    #[test]
    fn pulled_tray_labels_keep_status_on_only_one_action() {
        let tmp = Tmp::new("tray-distinct");
        pull(&tmp.0, 1).unwrap();
        let stop = tray_hands_stop_label(&tmp.0);
        let resume = tray_hands_resume_label(&tmp.0);
        assert_ne!(stop, resume);
        assert_eq!(
            [stop, resume]
                .iter()
                .filter(|s| s.contains("已經拔了"))
                .count(),
            1
        );
    }
}
