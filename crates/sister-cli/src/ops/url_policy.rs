//! `sister url-policy`——她問那一句，他答那一句（PHASES #42）。
//!
//! 這一格**不是設定頁裡的一個開關**，是她開口問的一個問題。差別在預設值：
//! 開關有一邊是預設的，而預設的那一邊等於產品替他選了；問題沒有預設值，
//! 沒答之前就是沒答。
//!
//! 所以這支指令有三種畫面，不是兩種：
//!
//! 1. 還沒答過——她把問題和兩個答案端出來，**並且說清楚在他回答之前她怎麼做**
//!    （不開，但理由是「我還沒問」不是「你說了不要」）。
//! 2. 答過了——她複述他選的那一句，還有怎麼改。
//! 3. 他現在就要答——寫進設定檔，然後把新的那一句唸回去。

use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use sister_core::Config;
use sister_hands::url_policy::UrlOpenAnswer;

// 問句和「還沒答之前怎麼做」那一句住在 `sister_hands::url_policy`，因為**字母人
// 也在問同一題**。抄一份到這裡的話，兩個入口會在某一版分家。
use sister_hands::url_policy::{BEFORE_YOU_ANSWER, QUESTION};

pub(crate) fn run(
    explicit_config: Option<&Path>,
    set: Option<UrlOpenAnswer>,
    out: &mut impl Write,
) -> Result<()> {
    let path = resolve(explicit_config)?;
    let mut config = load(&path, explicit_config.is_some())?;
    match set {
        Some(answer) => {
            config.hands.url_open = Some(answer);
            config
                .save(&path)
                .with_context(|| format!("寫入設定 {}", path.display()))?;
            writeln!(out, "{}", answer.recorded_line())?;
            writeln!(out, "存到 {}", path.display())?;
            let change = format!(
                "{} --set {}",
                super::url_policy_cmd(explicit_config),
                other(answer).key()
            );
            writeln!(out, "改主意的話再跑一次 `{change}`。",)?;
        }
        None => {
            writeln!(out, "{QUESTION}")?;
            writeln!(out)?;
            for answer in UrlOpenAnswer::ALL {
                writeln!(out, "  --set {}", answer.key())?;
                writeln!(out, "      {}", answer.line())?;
            }
            writeln!(out)?;
            match config.hands.url_open {
                Some(answer) => {
                    writeln!(out, "你現在的答案：{}", answer.line())?;
                    writeln!(out, "（存在 {}）", path.display())?;
                }
                None => {
                    writeln!(out, "你還沒回答過。")?;
                    writeln!(out, "{BEFORE_YOU_ANSWER}")?;
                }
            }
        }
    }
    Ok(())
}

fn other(answer: UrlOpenAnswer) -> UrlOpenAnswer {
    match answer {
        UrlOpenAnswer::OnlyOnMyPress => UrlOpenAnswer::WhenYouCanNameTheOrigin,
        UrlOpenAnswer::WhenYouCanNameTheOrigin => UrlOpenAnswer::OnlyOnMyPress,
    }
}

/// 他親手指的那一份優先；否則就是**讀設定的那支指令會讀的**那一份。
///
/// 這裡不可以自己造一個「資料目錄底下的 config.toml」：`sister do` 沒有
/// `--config` 的時候讀的是 [`Config::default_path`]，寫到別的地方等於他答了
/// 而她讀不到——而畫面上兩邊都不會報錯。
fn resolve(explicit: Option<&Path>) -> Result<PathBuf> {
    match explicit {
        Some(p) => Ok(p.to_path_buf()),
        None => Config::default_path()
            .context("找不到設定檔該放哪裡（這台機器沒有標準設定目錄）；請用 --config 指一個"),
    }
}

fn load(path: &Path, explicitly_named: bool) -> Result<Config> {
    match path
        .try_exists()
        .with_context(|| format!("檢查設定 {}", path.display()))?
    {
        true => Config::load(path).with_context(|| format!("讀設定 {}", path.display())),
        false if explicitly_named => anyhow::bail!("找不到設定檔：{}", path.display()),
        false => {
            // 預設位置沒有設定檔是正常的第一回。**但預設值裡那一欄仍然是
            // `None`**——「檔案不存在」不可以被讀成一個答案。
            Ok(Config::default())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn say(set: Option<UrlOpenAnswer>, path: &Path) -> String {
        let mut out = Vec::new();
        run(Some(path), set, &mut out).expect("跑得起來");
        String::from_utf8(out).expect("utf-8")
    }

    /// 沒答過的畫面必須**同時**做到兩件事：把問題問出來，以及說清楚
    /// 「還沒答」不等於「答了不要」。少了後面那半，一個從來沒被問過的人
    /// 會以為現在這個行為是他自己選的。
    #[test]
    fn the_unanswered_screen_says_it_is_a_question_not_a_decision_he_made() {
        let dir = crate::ops::tmp::Tmp::new("url-policy-unanswered");
        let path = dir.0.join("config.toml");
        Config::default().save(&path).expect("seed explicit config");
        let said = say(None, &path);
        assert!(said.contains(QUESTION), "問題沒問出來：{said}");
        assert!(said.contains("你還沒回答過"), "沒說他還沒答：{said}");
        assert!(
            said.contains("不是**因為你選了「不要」") || said.contains("不是"),
            "沒把「還沒問」跟「你說了不要」分開：{said}"
        );
        for answer in UrlOpenAnswer::ALL {
            assert!(
                said.contains(answer.key()),
                "少了一個答案：{}",
                answer.key()
            );
        }
    }

    /// 答了要寫得進去，而且**下一次讀得回來**。寫進去卻讀不回來的話，
    /// 他每次跑都會再被問一次，而畫面上看不出是哪一邊壞了。
    #[test]
    fn an_answer_survives_being_written_and_read_back() {
        let dir = crate::ops::tmp::Tmp::new("url-policy-roundtrip");
        let path = dir.0.join("config.toml");
        Config::default().save(&path).expect("seed explicit config");
        for answer in UrlOpenAnswer::ALL {
            let wrote = say(Some(answer), &path);
            assert!(wrote.contains(answer.line()), "沒複述他選的那一句：{wrote}");
            assert!(
                wrote.contains("--config") && wrote.contains(&path.display().to_string()),
                "改答案的指令沒有帶回同一份設定檔：{wrote}"
            );

            let back = Config::load(&path).expect("讀回來");
            assert_eq!(
                back.hands.url_open,
                Some(answer),
                "寫進去的答案讀不回來：{}",
                answer.key()
            );
            let shown = say(None, &path);
            assert!(
                shown.contains(answer.line()),
                "沒把現在的答案唸回去：{shown}"
            );
            assert!(
                !shown.contains("你還沒回答過"),
                "他答過了，畫面卻還說沒答：{shown}"
            );
        }
    }

    /// 兩個答案在畫面上不可以長得一樣——這是他做選擇時唯一的依據。
    #[test]
    fn the_two_answers_do_not_read_the_same() {
        let a = UrlOpenAnswer::OnlyOnMyPress;
        let b = UrlOpenAnswer::WhenYouCanNameTheOrigin;
        assert_ne!(a.line(), b.line());
        assert_ne!(a.key(), b.key());
        assert_eq!(other(a), b);
        assert_eq!(other(b), a);
    }

    /// 預設位置還沒有設定檔 ≠ 一個答案；親手指定的不存在路徑則是打錯了。
    #[test]
    fn only_a_missing_default_config_is_a_first_run() {
        let dir = crate::ops::tmp::Tmp::new("url-policy-nofile");
        let path = dir.0.join("nowhere").join("config.toml");
        assert!(!path.exists());
        assert_eq!(
            load(&path, false)
                .expect("預設位置第一次要載得起來")
                .hands
                .url_open,
            None
        );
        for set in [None, Some(UrlOpenAnswer::OnlyOnMyPress)] {
            let mut out = Vec::new();
            let error = run(Some(&path), set, &mut out).expect_err("明指的路徑不存在要報錯");
            assert!(error.to_string().contains("找不到設定檔"), "{error:#}");
            assert!(out.is_empty(), "失敗以前不該先說成成功：{out:?}");
            assert!(!path.exists(), "打錯的路徑不可以被偷偷建立");
        }
    }
}
