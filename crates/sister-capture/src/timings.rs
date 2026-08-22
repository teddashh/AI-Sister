//! 每個階段各花掉多少時間。
//!
//! 為什麼需要這個檔案：足跡報告說得出「CPU 平均 27.1%」，但那是一個
//! **沒有下一步**的數字。我為了它猜過兩次原因，兩次都猜 PNG 編碼太慢，
//! 兩次都被實測打臉——PNG 編一張 1568×882 只要 1.7ms，佔不到千分之一。
//!
//! 「不宣稱、直接示範」用在效能上就是這個樣子：不要猜哪裡慢，讓她自己講。
//! 一個超標 9 倍的預算，配上一份說不出錢花到哪裡去的報告，只會讓人去改
//! 那個最好改的地方，而不是那個最貴的地方。
//!
//! 量測本身必須便宜到可以忽略：`Instant::now()` 在 Windows 上是
//! QueryPerformanceCounter，數十奈秒，一個 tick 取樣六次，對一個每
//! 400ms 才跑一次的迴圈來說不存在。

use std::num::NonZeroU64;
use std::time::Duration;

/// 一個階段的累計耗時與呼叫次數。
///
/// 兩個都要留：`total` 回答「錢花到哪裡去」，`per_call` 回答「這件事
/// 本身貴不貴」。只有總和的話，一個跑一萬次的便宜階段會長得像瓶頸；
/// 只有平均的話，一個很貴但只跑兩次的階段會長得像瓶頸。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Stage {
    pub calls: u64,
    pub total: Duration,
}

impl Stage {
    pub fn record(&mut self, d: Duration) {
        self.calls += 1;
        self.total += d;
    }

    /// 一段時間裡真的包含多次同類呼叫（例如一幀的三個 OCR crop）。
    ///
    /// `calls` 是 NonZero，避免「沒呼叫」被接成一筆 0ms 的實測。
    pub fn record_many(&mut self, d: Duration, calls: NonZeroU64) {
        self.calls += calls.get();
        self.total += d;
    }

    /// 平均一次多久。`None` = 從來沒被呼叫過。
    ///
    /// 不回 0：一個「平均 0ms」的階段會被讀成「快到不用管」，
    /// 而它真正的意思是「這條路徑一次都沒有走過」——那是完全相反的訊息。
    pub fn per_call(&self) -> Option<Duration> {
        (self.calls > 0).then(|| self.total / self.calls as u32)
    }
}

/// 一整段錄製裡，各階段的耗時。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Timings {
    /// 一整個 tick 的耗時。**它是分母，不是階段之一。**
    ///
    /// 沒有它的話，這份報告只能講「我量到的那幾件事誰比較貴」，而那正是
    /// 一份效能報告最會騙人的形狀：沒被量到的那一段完全不會出現在版面上，
    /// 於是「我量到的最大的那一項」看起來就像「最大的那一項」。
    pub tick: Stage,
    /// 問前景視窗是誰（含 UIA 的跨程序往返）。
    pub focus: Stage,
    /// 讀剪貼簿。**每個 tick 都付**，而且是一次跨程序的 Win32 往返。
    pub clipboard: Stage,
    /// 收輸入節奏計數（不含內容）。
    pub input: Stage,
    /// 抓一次螢幕 + 算 dhash。**每個真的睜眼的 tick 都付一次，只付一次。**
    ///
    /// 這裡以前有兩欄（探測、抓圖），因為以前一個 tick 會讀兩次螢幕。
    /// 實測那是白付的：擷取成本由來源像素決定，而兩次讀的是同一個螢幕。
    pub grab: Stage,
    /// 判斷工作幀哪些區域需要重讀。只有真的有 gate 的平台才會有這一列。
    pub ocr_gate: Stage,
    /// OCR 實作的呼叫；局部三塊就是三次，不拿「一幀」冒充一次。
    pub ocr: Stage,
    /// 縮圖 + PNG 編碼 + 寫檔。
    pub store: Stage,
    pub db: Stage,
}

impl Timings {
    /// 依總耗時由大到小。報告只印得下前幾名，而使用者要看的就是前幾名。
    ///
    /// 不含 `tick`：它是這些階段的容器，排進來會永遠是第一名，
    /// 而且會讓百分比加起來超過 100%。
    pub fn ranked(&self) -> Vec<(&'static str, Stage)> {
        let mut v = vec![
            ("脈絡", self.focus),
            ("剪貼簿", self.clipboard),
            ("輸入", self.input),
            ("抓圖", self.grab),
            ("OCR 閘門", self.ocr_gate),
            ("OCR", self.ocr),
            ("存檔", self.store),
            ("資料庫", self.db),
        ];
        v.retain(|(_, s)| s.calls > 0);
        v.sort_by_key(|(_, s)| std::cmp::Reverse(s.total));
        v
    }

    /// 分母：量得到整個 tick 就用它，否則退回各階段的總和。
    ///
    /// 兩者都是「她在忙」的時間，不是牆上時鐘的時間——tick 與 tick 之間
    /// 的睡眠不算在內。
    pub fn total(&self) -> Duration {
        if self.tick.calls > 0 {
            self.tick.total
        } else {
            self.ranked().iter().map(|(_, s)| s.total).sum()
        }
    }

    /// 沒有歸因到任何階段的時間。`None` = 沒量過整個 tick，答不出來。
    ///
    /// 這個數字是**這份報告對自己的誠實度檢查**。它小，代表上面那份排名
    /// 真的解釋了 CPU 花到哪裡去；它大，代表最貴的東西根本沒被量到，
    /// 而排名第一的那一項只是「被量到的裡面最大的」。
    ///
    /// 不回 0：答不出來和「全部都歸因到了」是完全相反的兩件事。
    pub fn unattributed(&self) -> Option<Duration> {
        if self.tick.calls == 0 {
            return None;
        }
        let accounted: Duration = self.ranked().iter().map(|(_, s)| s.total).sum();
        // 巢狀量測會有奈秒級的重疊，saturating 讓它變成 0 而不是 panic
        Some(self.tick.total.saturating_sub(accounted))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 沒跑過的階段不可以假裝自己是 0ms。
    #[test]
    fn a_stage_never_run_says_so_instead_of_reporting_zero() {
        assert_eq!(Stage::default().per_call(), None);
        let mut s = Stage::default();
        s.record(Duration::from_millis(10));
        s.record(Duration::from_millis(20));
        assert_eq!(s.per_call(), Some(Duration::from_millis(15)));
        assert_eq!(s.calls, 2);

        s.record_many(
            Duration::from_millis(30),
            NonZeroU64::new(3).expect("three"),
        );
        assert_eq!(s.calls, 5);
        assert_eq!(s.total, Duration::from_millis(60));
        assert_eq!(s.per_call(), Some(Duration::from_millis(12)));
    }

    /// 排名要真的排名，而且沒跑過的階段不該佔用版面。
    #[test]
    fn the_ranking_names_the_expensive_stage_first_and_hides_the_unused() {
        let mut t = Timings::default();
        t.ocr.record(Duration::from_millis(50));
        t.grab.record(Duration::from_millis(10));

        let names: Vec<&str> = t.ranked().iter().map(|(n, _)| *n).collect();
        assert_eq!(
            names,
            vec!["OCR", "抓圖"],
            "最貴的要排第一，沒跑過的不佔版面"
        );
        assert_eq!(t.total(), Duration::from_millis(60));
    }

    /// 沒量過整個 tick 的時候，「沒歸因到的時間」必須說不知道。
    ///
    /// 回 0 的話，一份**什麼都沒量到**的報告會顯示「歸因率 100%」——
    /// 這是這個模組存在的理由的反面。
    #[test]
    fn without_a_denominator_the_leftover_is_unknown_not_zero() {
        let mut t = Timings::default();
        t.ocr.record(Duration::from_millis(50));
        assert_eq!(t.unattributed(), None);
        assert_eq!(t.total(), Duration::from_millis(50), "沒有分母就退回總和");
    }

    /// tick 是分母，不是階段之一：它不進排名，而沒被量到的那一段要跑出來。
    #[test]
    fn the_time_no_stage_claimed_shows_up_as_its_own_line() {
        let mut t = Timings::default();
        t.tick.record(Duration::from_millis(100));
        t.ocr.record(Duration::from_millis(30));
        t.clipboard.record(Duration::from_millis(10));

        let names: Vec<&str> = t.ranked().iter().map(|(n, _)| *n).collect();
        assert_eq!(names, vec!["OCR", "剪貼簿"], "tick 不可以出現在排名裡");
        assert_eq!(t.total(), Duration::from_millis(100), "分母是整個 tick");
        assert_eq!(
            t.unattributed(),
            Some(Duration::from_millis(60)),
            "60ms 沒有任何階段認領——這正是要印出來的那個數字"
        );
    }

    /// 巢狀量測的奈秒級重疊不可以讓它爆掉。
    #[test]
    fn overlapping_measurements_clamp_to_zero_instead_of_panicking() {
        let mut t = Timings::default();
        t.tick.record(Duration::from_millis(10));
        t.ocr.record(Duration::from_millis(11));
        assert_eq!(t.unattributed(), Some(Duration::ZERO));
    }
}
