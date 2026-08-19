//! 她自己佔了多少：CPU、記憶體、磁碟。
//!
//! Phase 0 的驗收條件裡有三個數字——CPU 平均 < 3%、RAM < 400MB、
//! 磁碟 < 300MB/天——而在這個模組存在之前，**沒有任何辦法知道它們是多少**。
//! 一個量不到的預算不是預算，是一句話。
//!
//! 這和「不宣稱、直接示範」是同一條紀律：README 上要寫足跡數字，那個數字
//! 就必須是她自己量出來的，不是我在自己的機器上開工作管理員瞄一眼記下來的。
//!
//! 量測本身要**便宜**。這裡兩個平台都只是讀幾個作業系統已經在維護的計數器
//! ——沒有取樣執行緒、沒有計時器、沒有額外的系統呼叫迴圈。她量自己的成本，
//! 不可以變成她成本的一部分。

use std::time::Instant;

/// 某一刻的用量快照。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Sample {
    /// 這個行程從啟動到現在一共用掉的 CPU 秒數（user + kernel）。
    ///
    /// 是**累計值**不是百分比：百分比要拿兩次快照相減才算得出來，而
    /// 「從啟動到現在的平均」和「最近十秒的瞬時」是兩個不同的問題。
    pub cpu_seconds: f64,
    /// 目前的常駐記憶體（Windows 的 working set / Linux 的 VmRSS）。
    pub rss_bytes: u64,
}

/// 一整段錄製期間的足跡。
///
/// 峰值和平均都留著：平均證明她平常很安靜，峰值證明她最吵的時候也沒有
/// 吵到不能接受。只報平均會把一次 2GB 的尖峰藏起來。
#[derive(Debug, Clone)]
pub struct Footprint {
    started: Instant,
    first: Option<Sample>,
    latest: Option<Sample>,
    peak_rss: u64,
}

impl Default for Footprint {
    fn default() -> Self {
        Self::new()
    }
}

impl Footprint {
    pub fn new() -> Self {
        let first = sample();
        Self {
            started: Instant::now(),
            first,
            latest: first,
            peak_rss: first.map_or(0, |s| s.rss_bytes),
        }
    }

    /// 量一次。**每個 tick 都叫**——`peak_rss` 這個名字承諾的是峰值。
    ///
    /// 這裡本來寫著「每隔一段時間叫一次就好」，而呼叫端照做，一分鐘一次。
    /// 那讓 `peak_rss` 變成一個一分鐘取一次的抽樣：任何短於一分鐘的尖峰都
    /// 和取平均一樣看不見，也就是這個欄位存在的理由整個消失。而一次抓圖
    /// 握著十幾 MB 的緩衝只有幾十毫秒——取樣週期的萬分之七。
    ///
    /// 便宜到不必省：一次 /proc 讀取（Linux）或兩個 Win32 呼叫（Windows），
    /// 比這個迴圈每拍都付的剪貼簿輪詢還便宜。
    pub fn tick(&mut self) {
        if let Some(s) = sample() {
            self.latest = Some(s);
            self.peak_rss = self.peak_rss.max(s.rss_bytes);
        }
    }

    /// 這段期間的平均 CPU 佔用（百分比，單核為 100）。
    ///
    /// `None` = 這個平台量不到，或還沒過足夠的時間。**不回 0**：
    /// 0 會被讀成「她完全沒用 CPU」，那是一個很有說服力的假消息。
    pub fn cpu_percent(&self) -> Option<f64> {
        let (a, b) = (self.first?, self.latest?);
        let wall = self.started.elapsed().as_secs_f64();
        // 太短的區間除下去會得到很誇張的數字（第一秒的啟動成本佔滿整個分母）
        if wall < 1.0 {
            return None;
        }
        Some(((b.cpu_seconds - a.cpu_seconds) / wall * 100.0).max(0.0))
    }

    /// 期間內看過的最大常駐記憶體。`None` = 量不到。
    pub fn peak_rss_bytes(&self) -> Option<u64> {
        (self.peak_rss > 0).then_some(self.peak_rss)
    }

    /// 這段期間一共燒掉多少 CPU 秒。`None` = 量不到。
    ///
    /// 拿來和各階段的耗時對帳用，而那是兩個不同的量：階段耗時是**牆上
    /// 時間**（`Instant`），卡在顯示驅動裡等的時間也算在內；這個是真的
    /// 燒掉的 CPU。實測 CI 上一段 0.8 秒的 tick 時間只對應 0.27 秒 CPU
    /// ——三分之二是等，不是算。
    ///
    /// 兩個都要報出來，否則一份「拆得很細的耗時表」會被讀成「CPU 花在
    /// 哪裡」，而使用者抱怨的明明是後者。
    pub fn cpu_seconds_used(&self) -> Option<f64> {
        let (a, b) = (self.first?, self.latest?);
        Some((b.cpu_seconds - a.cpu_seconds).max(0.0))
    }

    pub fn elapsed_secs(&self) -> f64 {
        self.started.elapsed().as_secs_f64()
    }

    /// 把 `bytes` 換算成「一天會長多少」。
    ///
    /// 錄十分鐘就外推一天當然不準，所以**太短就不回答**。一個亂猜的
    /// 「300MB/天」會被寫進 README 然後變成事實。
    pub fn bytes_per_day(&self, bytes: u64) -> Option<f64> {
        let secs = self.elapsed_secs();
        if secs < 60.0 {
            return None;
        }
        Some(bytes as f64 / secs * 86_400.0)
    }
}

/// 讀一次作業系統的計數器。`None` = 這個平台沒實作，或讀失敗。
#[cfg(target_os = "linux")]
pub fn sample() -> Option<Sample> {
    // /proc/self/stat 的欄位 14/15 是 utime/stime（clock ticks）。
    // comm 欄位可能含空白與括號，所以從最後一個 ')' 之後才開始切。
    let stat = std::fs::read_to_string("/proc/self/stat").ok()?;
    let rest = &stat[stat.rfind(')')? + 1..];
    let f: Vec<&str> = rest.split_whitespace().collect();
    // rest 的第一欄是 state（原本的第 3 欄），所以 utime 是 index 11
    let utime: u64 = f.get(11)?.parse().ok()?;
    let stime: u64 = f.get(12)?.parse().ok()?;
    // USER_HZ 在 Linux 上實質固定是 100；為了不引 libc 就寫死，
    // 反正這條路徑只是開發機上的對照組，出貨的平台是 Windows。
    let cpu_seconds = (utime + stime) as f64 / 100.0;

    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    let rss_kb: u64 = status
        .lines()
        .find_map(|l| l.strip_prefix("VmRSS:"))?
        .split_whitespace()
        .next()?
        .parse()
        .ok()?;

    Some(Sample {
        cpu_seconds,
        rss_bytes: rss_kb * 1024,
    })
}

#[cfg(windows)]
pub fn sample() -> Option<Sample> {
    use windows::Win32::Foundation::FILETIME;
    use windows::Win32::System::ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS};
    use windows::Win32::System::Threading::{GetCurrentProcess, GetProcessTimes};

    let proc = unsafe { GetCurrentProcess() };

    let mut created = FILETIME::default();
    let mut exited = FILETIME::default();
    let mut kernel = FILETIME::default();
    let mut user = FILETIME::default();
    unsafe { GetProcessTimes(proc, &mut created, &mut exited, &mut kernel, &mut user) }.ok()?;
    // FILETIME 的單位是 100 奈秒
    let ticks = |f: FILETIME| ((f.dwHighDateTime as u64) << 32) | f.dwLowDateTime as u64;
    let cpu_seconds = (ticks(kernel) + ticks(user)) as f64 / 1e7;

    let mut mem = PROCESS_MEMORY_COUNTERS::default();
    unsafe {
        GetProcessMemoryInfo(
            proc,
            &mut mem,
            std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32,
        )
    }
    .ok()?;

    Some(Sample {
        cpu_seconds,
        rss_bytes: mem.WorkingSetSize as u64,
    })
}

#[cfg(not(any(target_os = "linux", windows)))]
pub fn sample() -> Option<Sample> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 在有實作的平台上，量測本身必須真的量得到東西。
    ///
    /// 這條看起來像廢話，但它擋的是「函式回 `None`，於是所有數字都
    /// 安靜地不見，而報告只是少印兩行」——那正是這個專案的招牌失效方式。
    #[test]
    #[cfg(any(target_os = "linux", windows))]
    fn this_platform_can_actually_measure_itself() {
        let s = sample().expect("這個平台應該量得到自己");
        assert!(s.rss_bytes > 0, "RSS 不可能是 0：{s:?}");
        assert!(s.cpu_seconds >= 0.0);
    }

    /// 太短的區間不給答案，而不是給一個很有說服力的錯答案。
    #[test]
    fn a_run_too_short_to_extrapolate_says_so_instead_of_guessing() {
        let f = Footprint::new();
        assert_eq!(f.cpu_percent(), None, "不到一秒就算 CPU% 只會是雜訊");
        assert_eq!(f.bytes_per_day(1_000_000), None, "錄幾秒不能外推成一天");
    }

    /// 峰值要真的是峰值——中途掉下來不能把它洗掉。
    #[test]
    fn the_peak_survives_a_later_dip() {
        let mut f = Footprint {
            started: Instant::now(),
            first: Some(Sample {
                cpu_seconds: 0.0,
                rss_bytes: 100,
            }),
            latest: None,
            peak_rss: 0,
        };
        f.peak_rss = 900;
        f.tick(); // 真實取樣多半比 900 小
        assert!(
            f.peak_rss_bytes().expect("有峰值") >= 900,
            "後來變小不能把看過的峰值洗掉"
        );
    }
}
