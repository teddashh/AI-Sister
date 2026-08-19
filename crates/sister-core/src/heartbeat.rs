//! 「現在到底有沒有人在錄」——兩個行程之間的第二條線。
//!
//! [`crate::pause`] 回答的是「她有沒有被叫停」，而那是**另一個問題**。字母人
//! 上那顆暫停鍵沒被按下的時候，它顯示「在聽」；但 `sister record` 是另一個
//! 執行檔，如果根本沒有人把它跑起來，「在聽」就是一句謊話——而且是這個產品
//! 唯一不能說的那種謊。使用者會照著那三個字去相信「她記得住今天」，然後某天
//! 問她「剛剛發生什麼事」，得到一片空白。
//!
//! 判定不能只看 `sessions.ended_at IS NULL`：recorder 當掉的時候那一列會永遠
//! 停在 NULL，於是「她死了」和「她正在錄」長得一模一樣。也不能只看「資料庫
//! 最近有沒有新資料」：閒置閘門本來就會讓一台沒人動的機器好幾秒不寫東西，
//! 那樣會把「安靜」誤判成「死了」。
//!
//! 所以 recorder 每隔幾秒主動蓋一次時戳。**活著才蓋得動**——當掉、被 kill、
//! 整台關機，時戳就停在那裡，而停住的時戳自己會過期。
//!
//! 和暫停旗標刻意相反的一點：這裡**讀不到就當作沒有人在錄**。暫停那邊
//! 「不確定就是暫停」是為了少錄；這邊「不確定就是沒在錄」是為了少吹牛。
//! 兩者是同一個方向——不確定的時候，往「她做得比較少」那邊倒。

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

use crate::model::Millis;

/// 檔名。放在 data dir 裡，跟 `sister.db` 同一層。
const BEAT: &str = "recording.beat";

/// 多久蓋一次。
///
/// 比 tick 慢很多是刻意的：這個檔案的用途是「有沒有人活著」，不是「上一個
/// tick 是什麼時候」。每秒寫一次沒有多回答任何問題，只是多 86,400 次磁碟寫入。
pub const BEAT_EVERY_MS: i64 = 5_000;

/// 超過這麼久沒蓋，就當作沒有人在錄。
///
/// 抓 `BEAT_EVERY_MS` 的三倍多一點：一次寫入被 OS 排程延後、或者某個 tick
/// 剛好卡在一張很大的圖上，都不該讓字母人閃一下「沒有人在記錄」。**寧可晚
/// 一點說她死了，也不要在她活著的時候說她死了**——後者會讓使用者去重開一個
/// 已經在跑的 recorder。
pub const STALE_AFTER_MS: i64 = 16_000;

pub fn beat_path(data_dir: &Path) -> PathBuf {
    data_dir.join(BEAT)
}

/// 心跳的主人現在在做什麼。
///
/// 這個檔案要同時回答兩個**不一樣**的問題，而以前只有一個答案：
///
/// - 「這個資料目錄有人佔著嗎」——`record` 拿它擋第二個 recorder。開機那幾
///   分鐘（大顆資料庫的 migration 會跑很久）**算佔著**，不然第二下按鈕會穿
///   過去，兩個 recorder 各錄一份。
/// - 「她現在在錄嗎」——字母人那顆指示燈、系統匣、`doctor` 拿它講話。開機
///   那幾分鐘**不算在錄**：她一個字都還沒記，而使用者正照著那三個字相信
///   「她記得住今天」。
///
/// 一個述詞同時回答這兩題，就一定有一邊是錯的。而修好「有沒有人佔著」的那
/// 一版，剛好把「她在錄嗎」弄壞了——這正是模組開頭說的那種、這個產品唯一
/// 不能說的謊。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    /// 行程起來了，但還沒開始錄（多半卡在 `Db::open` 的 migration）。
    Booting,
    /// 主迴圈在跑，她真的在記東西。
    Recording,
}

/// 蓋一次時戳。recorder 每 [`BEAT_EVERY_MS`] 呼叫一次。
///
/// 先寫暫存檔再 rename，否則讀的人有機會讀到寫到一半的半行數字。那不會壞掉
/// （解析失敗就當成沒在錄），但會讓字母人無緣無故閃一下。
pub fn beat(data_dir: &Path, ts: Millis) -> Result<()> {
    write_beat(data_dir, ts, Phase::Recording)
}

/// 開機那一段蓋的心跳。見 [`Phase`]。
pub fn beat_booting(data_dir: &Path, ts: Millis) -> Result<()> {
    write_beat(data_dir, ts, Phase::Booting)
}

fn write_beat(data_dir: &Path, ts: Millis, phase: Phase) -> Result<()> {
    let path = beat_path(data_dir);
    let tmp = path.with_extension("beat.tmp");
    // 錄製中就寫一個裸數字，和舊版一模一樣——舊的心跳檔（和舊的 recorder）
    // 讀起來仍然是「在錄」，那也是對的：會寫這個檔案的舊版都在主迴圈裡。
    let body = match phase {
        Phase::Recording => ts.to_string(),
        Phase::Booting => format!("{ts} boot"),
    };
    std::fs::write(&tmp, body).with_context(|| format!("write {}", tmp.display()))?;
    std::fs::rename(&tmp, &path).with_context(|| format!("rename to {}", path.display()))?;
    Ok(())
}

/// 收工。**乾淨結束的時候要自己清掉**，不要留給逾時去猜——那 16 秒裡字母人
/// 會說她還在錄，而她已經走了。
pub fn stop(data_dir: &Path) {
    let _ = std::fs::remove_file(beat_path(data_dir));
    let _ = std::fs::remove_file(beat_path(data_dir).with_extension("beat.tmp"));
}

fn read_beat(data_dir: &Path) -> Option<(Millis, Phase)> {
    let raw = std::fs::read_to_string(beat_path(data_dir)).ok()?;
    let mut fields = raw.split_whitespace();
    let ts: Millis = fields.next()?.parse().ok()?;
    // 認不得的第二欄當成「在錄」而不是丟掉整行：未來多寫一個欄位的版本，
    // 不該讓舊版讀成「沒有人在錄」然後放行第二個 recorder。
    let phase = match fields.next() {
        Some("boot") => Phase::Booting,
        _ => Phase::Recording,
    };
    Some((ts, phase))
}

/// 最後一次心跳的時戳。`None` = 檔案不在、讀不到、或內容不是數字。
pub fn last_beat(data_dir: &Path) -> Option<Millis> {
    read_beat(data_dir).map(|(ts, _)| ts)
}

/// 現在這個資料目錄的狀態。`None` = 沒有人在。
///
/// `now` 由呼叫端給，因為時間在這個 crate 裡一律是參數（測試要能演「三分鐘
/// 前的心跳」，不能等三分鐘）。
///
/// 未來的時戳一樣算活的：使用者調過時鐘、或者兩個行程對時差了幾百毫秒，都
/// 不該被讀成「她死了」。
pub fn phase(data_dir: &Path, now: Millis) -> Option<Phase> {
    let (beat, phase) = read_beat(data_dir)?;
    (now - beat < STALE_AFTER_MS).then_some(phase)
}

/// 她現在**在錄嗎**。正在開機的 recorder 回 `false`：她一個字都還沒記。
///
/// 給那幾個會對使用者講一句話的地方用（字母人的指示燈、系統匣、`doctor`）。
/// 要擋第二個 recorder 請用 [`is_occupied`]。
pub fn is_recording(data_dir: &Path, now: Millis) -> bool {
    phase(data_dir, now) == Some(Phase::Recording)
}

/// 這個資料目錄**有人佔著嗎**。正在開機的也算。
///
/// 給那幾個「要不要再開一個」的判斷用。開機那幾分鐘算佔著，否則第二下按鈕
/// 會穿過去，兩個 recorder 各錄一份，而使用者只會發現磁碟用得比講好的快
/// 一倍。
pub fn is_occupied(data_dir: &Path, now: Millis) -> bool {
    phase(data_dir, now).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Tmp(PathBuf);
    impl Tmp {
        fn new(name: &str) -> Self {
            let p = std::env::temp_dir().join(format!("sister-beat-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&p);
            std::fs::create_dir_all(&p).expect("mkdir");
            Self(p)
        }
    }
    impl Drop for Tmp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// 沒有檔案 = 沒有人在錄。這是**新裝好、還沒跑過 `sister record`** 的
    /// 那台機器，也就是字母人第一次被打開時的樣子。
    #[test]
    fn a_machine_where_nobody_ever_started_her_is_not_recording() {
        let t = Tmp::new("never");
        assert!(!is_recording(&t.0, 1_000_000));
        assert_eq!(last_beat(&t.0), None);
    }

    #[test]
    fn a_fresh_beat_means_she_is_alive() {
        let t = Tmp::new("fresh");
        beat(&t.0, 1_000_000).expect("beat");
        assert!(is_recording(&t.0, 1_000_000));
        assert!(is_recording(&t.0, 1_000_000 + STALE_AFTER_MS - 1));
    }

    /// recorder 被 kill 掉、或整台當機。時戳停在那裡，而停住的時戳會過期
    /// ——這正是 `sessions.ended_at IS NULL` 做不到的判斷。
    #[test]
    fn a_beat_that_stopped_expires_by_itself() {
        let t = Tmp::new("stale");
        beat(&t.0, 1_000_000).expect("beat");
        assert!(!is_recording(&t.0, 1_000_000 + STALE_AFTER_MS));
        assert!(!is_recording(&t.0, 1_000_000 + 3_600_000));
    }

    /// 乾淨結束不要留給逾時去猜：那 16 秒裡字母人會說她還在錄，而她已經走了。
    #[test]
    fn stopping_cleanly_takes_effect_immediately() {
        let t = Tmp::new("stop");
        beat(&t.0, 1_000_000).expect("beat");
        stop(&t.0);
        assert!(!is_recording(&t.0, 1_000_000));
    }

    /// 使用者調了時鐘，或者兩個行程對時差了一點。往未來偏不可以被讀成
    /// 「她死了」——那會讓人去重開一個已經在跑的 recorder。
    #[test]
    fn a_beat_from_the_future_is_still_alive() {
        let t = Tmp::new("future");
        beat(&t.0, 2_000_000).expect("beat");
        assert!(is_recording(&t.0, 1_000_000));
    }

    /// 內容壞掉（寫到一半斷電）當作沒在錄，不是 panic、也不是「還在錄」。
    #[test]
    fn a_corrupt_beat_reads_as_nobody_recording() {
        let t = Tmp::new("corrupt");
        std::fs::write(beat_path(&t.0), "not a number").expect("write");
        assert!(!is_recording(&t.0, 1_000_000));
    }

    /// 蓋第二次要蓋得掉第一次。少了這一條，心跳只會活 16 秒。
    #[test]
    fn beating_again_moves_it_forward() {
        let t = Tmp::new("again");
        beat(&t.0, 1_000_000).expect("beat");
        beat(&t.0, 1_000_000 + 5_000).expect("beat again");
        assert_eq!(last_beat(&t.0), Some(1_005_000));
        assert!(is_recording(&t.0, 1_000_000 + STALE_AFTER_MS));
    }
}
