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
    /// 問前景視窗是誰（含 UIA 的跨程序往返）。
    pub focus: Stage,
    /// 便宜的探測抓圖 + dhash。**每個 tick 都付**。
    pub probe: Stage,
    /// 完整解析度抓圖。只有畫面真的變了才付。
    pub grab: Stage,
    pub ocr: Stage,
    /// 縮圖 + PNG 編碼 + 寫檔。
    pub store: Stage,
    pub db: Stage,
}

impl Timings {
    /// 依總耗時由大到小。報告只印得下前幾名，而使用者要看的就是前幾名。
    pub fn ranked(&self) -> Vec<(&'static str, Stage)> {
        let mut v = vec![
            ("脈絡", self.focus),
            ("探測", self.probe),
            ("抓圖", self.grab),
            ("OCR", self.ocr),
            ("存檔", self.store),
            ("資料庫", self.db),
        ];
        v.retain(|(_, s)| s.calls > 0);
        v.sort_by_key(|(_, s)| std::cmp::Reverse(s.total));
        v
    }

    /// 所有階段的總和。這是「她在忙」的時間，不是牆上時鐘的時間。
    pub fn total(&self) -> Duration {
        self.ranked().iter().map(|(_, s)| s.total).sum()
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
    }

    /// 排名要真的排名，而且沒跑過的階段不該佔用版面。
    #[test]
    fn the_ranking_names_the_expensive_stage_first_and_hides_the_unused() {
        let mut t = Timings::default();
        t.probe.record(Duration::from_millis(1));
        t.ocr.record(Duration::from_millis(50));
        t.grab.record(Duration::from_millis(10));

        let names: Vec<&str> = t.ranked().iter().map(|(n, _)| *n).collect();
        assert_eq!(names, vec!["OCR", "抓圖", "探測"], "最貴的要排第一");
        assert_eq!(t.total(), Duration::from_millis(61));
    }
}
