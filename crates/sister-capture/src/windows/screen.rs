//! 螢幕擷取：GDI `BitBlt` + `StretchBlt`。
//!
//! **為什麼 Phase 0 不用 WGC**（SPEC §14 選的是 WGC/DDA，這裡是刻意的階段性
//! 選擇，不是遺漏）：
//!
//! - 黃框。Win10 的 WGC 強制在螢幕外緣畫一圈黃色邊框，`IsBorderRequired`
//!   要 build 20348+ 才存在。一個整天開著的錄製程式在使用者螢幕上永久
//!   畫一道黃框，是不可能接受的。
//! - WGC 是 callback/執行緒模型，要塞進 `grab(ts)` 這個同步介面就得再養
//!   一條背景執行緒與一個共享格位。那個複雜度應該換到實際的好處再付。
//!
//! GDI 的代價要誠實說：DRM 保護內容與獨佔全螢幕會拍成全黑（這件事對本
//! 專案反而是好的），而且它比 GPU 路徑慢。但 1 fps 下 BitBlt 一個 4K 畫面
//! 約 15–40ms，離 CPU 預算還很遠。
//!
//! 升級到 WGC 的時機是「量到 GDI 真的不夠」，不是「WGC 聽起來比較好」。
//!
//! 兩個不能省的細節：
//! - **只拍前景視窗所在的那一台螢幕**。她在看的就是那一台；拍整個虛擬桌面
//!   等於為了沒人在看的畫面付三倍的 CPU 與磁碟。
//! - **在 GDI 裡就縮好圖**，別搬進使用者空間再用 `image::imageops::resize`
//!   ——同一張圖那樣縮要 54ms。
//!
//!   但「交給 `StretchBlt` 幾乎不用錢」這句話原本也寫在這裡，而它是錯的。
//!   真的 Windows 上量到：整張原生解析度 `BitBlt` **16.0ms**，縮到 256 的
//!   `StretchBlt` + `HALFTONE` **30.0ms**。縮圖比不縮還貴一倍，因為
//!   `HALFTONE` 是 CPU 逐一平均每個來源像素——省下來的是 `GetDIBits` 的
//!   搬運量，付出去的是整張圖的算術。
//!
//!   所以取樣模式跟著消費者走，見 [`Sampling`]。
//!
//! ## 為什麼有兩條抓圖路徑
//!
//! 原本只有一條，一次抓到 `max_long_edge`（1568），同一份像素同時拿去
//! 算 dhash、餵 OCR、存檔。三件事的需求其實完全衝突，於是三件事一起壞：
//!
//! - **去重**只需要 9×8。它卻讓每個 tick 都搬一張全尺寸的圖，而實測
//!   120 個 tick 裡有 103 個算完雜湊就把整張圖丟掉。
//! - **OCR** 需要原生像素。2560 縮到 1568 是 0.61 倍，12px 的字掉到 7px，
//!   於是 `Microsoft Teams` 被讀成 `Micr099ftTeamsTr`——不報錯，只是讀錯。
//! - **存檔**要小。它是唯一真的想要 1568 的人。
//!
//! 所以拆成 [`ScreenSource::probe`]（便宜、只保證 dhash）與
//! [`ScreenSource::grab`]（原生解析度）。存檔的縮圖留在 `frames.rs`，
//! 因為那是磁碟預算的事，不是擷取的事。

use anyhow::{Result, bail};
use sister_core::model::Millis;
use windows::Win32::Foundation::{HWND, RECT};
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BitBlt, CAPTUREBLT, COLORONCOLOR, CreateCompatibleBitmap,
    CreateCompatibleDC, DIB_RGB_COLORS, DeleteDC, DeleteObject, GetDC, GetDIBits, GetMonitorInfoW,
    HALFTONE, HBITMAP, HDC, HGDIOBJ, HMONITOR, MONITOR_DEFAULTTOPRIMARY, MONITORINFO,
    MonitorFromWindow, ReleaseDC, SRCCOPY, SelectObject, SetBrushOrgEx, SetStretchBltMode,
    StretchBlt,
};
use windows::Win32::System::StationsAndDesktops::{
    CloseDesktop, DESKTOP_SWITCHDESKTOP, OpenInputDesktop,
};
use windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow;

use crate::traits::{RawFrame, ScreenSource};

/// 完整解析度抓圖的長邊上限。
///
/// 這不是畫質旋鈕，是一道防線，所以刻意寫死而不是開成設定：8K 螢幕一張
/// RGBA 是 132MB，而 Windows 的 OCR 引擎本身也只吃到 10000 像素。4096
/// 讓 4K（3840）以下的螢幕全部拿到**原生**像素——也就是絕大多數人——
/// 更大的才縮，而且縮一半仍然遠好過為了它把每個人都降到 1568。
const OCR_LONG_EDGE: u32 = 4096;

/// 探測圖的長邊。
///
/// dhash 會把任何尺寸 box-average 成 9×8，所以這個數字只要大到保住版面
/// 結構就夠——實測 2560×1440 的原圖與 256×144 的探測圖算出來的 dhash
/// **完全相同**（hamming 距離 0）。
///
/// 省下來的是搬運：`GetDIBits` 從 14MB 掉到 147KB，之後那個 BGRA→RGBA
/// 的逐像素迴圈從 370 萬次掉到 3.7 萬次。而這是每個 tick 都要付的錢。
///
/// **這筆帳一開始只算了一半。** 縮圖本身也要錢，而且用 `HALFTONE` 縮的
/// 話比省下來的還多（實測 30.0ms vs 原生不縮的 16.0ms）。所以探測圖用
/// [`Sampling::Fast`]——尺寸負責搬運量，取樣模式負責算術量，兩件事都得算。
const PROBE_LONG_EDGE: u32 = 256;

/// 縮圖時怎麼取樣。**這是一個「誰要看這張圖」的決定，不是畫質旋鈕。**
///
/// 一開始兩條路共用 `HALFTONE`，理由寫的是「其餘模式縮完文字會糊到 OCR
/// 認不出來」——那句話對 OCR 是真的，對探測圖是假的：探測圖唯一的讀者是
/// 一個 9×8 的雜湊，它本來就看不到任何細節。
///
/// 這是同一個錯誤的第二次：一份像素被拿去餵三個需求不同的消費者。
/// 第一次發生在解析度上（1568 對三邊都是錯的），這次發生在取樣模式上。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Sampling {
    /// 面積平均（`HALFTONE`）。縮完的字還讀得出來，代價是 CPU 逐像素平均。
    Area,
    /// 直接丟像素（`COLORONCOLOR`）。給只看得到輪廓的消費者用。
    Fast,
}

pub struct WindowsScreen {
    /// HMONITOR → 穩定的小整數。指標本身太大也不穩，但同一個 session 裡
    /// 出現順序是穩的，而 frames 表只需要「同不同台」這個資訊。
    monitors: Vec<isize>,
}

impl Default for WindowsScreen {
    fn default() -> Self {
        Self::new()
    }
}

impl WindowsScreen {
    pub fn new() -> Self {
        Self {
            monitors: Vec::new(),
        }
    }

    fn monitor_index(&mut self, mon: HMONITOR) -> i32 {
        let raw = mon.0 as isize;
        match self.monitors.iter().position(|m| *m == raw) {
            Some(i) => i as i32,
            None => {
                self.monitors.push(raw);
                (self.monitors.len() - 1) as i32
            }
        }
    }

    /// 兩條路徑的差別只有「縮到多大」與「怎麼縮」。
    pub(crate) fn capture(
        &mut self,
        ts: Millis,
        long_edge: u32,
        sampling: Sampling,
    ) -> Result<Option<RawFrame>> {
        // 鎖定時不擷取。這裡要主動問，不能只靠「拍出來是黑的」——
        // 黑畫面會被當成一張正常的畫面存起來、算 dhash、跑 OCR。
        if session_locked() {
            return Ok(None);
        }

        let hwnd = unsafe { GetForegroundWindow() };
        let Some((mon, rect)) = focused_monitor(hwnd) else {
            return Ok(None);
        };

        let src_w = rect.right - rect.left;
        let src_h = rect.bottom - rect.top;
        if src_w <= 0 || src_h <= 0 {
            return Ok(None);
        }

        let (dst_w, dst_h) = fit(src_w as u32, src_h as u32, long_edge);
        let monitor = self.monitor_index(mon);
        let rgba = unsafe { blit(rect, src_w, src_h, dst_w, dst_h, sampling)? };

        Ok(Some(RawFrame::from_rgba(ts, monitor, dst_w, dst_h, rgba)))
    }
}

impl ScreenSource for WindowsScreen {
    fn grab(&mut self, ts: Millis) -> Result<Option<RawFrame>> {
        self.capture(ts, OCR_LONG_EDGE, Sampling::Area)
    }

    fn probe(&mut self, ts: Millis) -> Result<Option<RawFrame>> {
        self.capture(ts, PROBE_LONG_EDGE, Sampling::Fast)
    }
}

/// 把 `w×h` 等比縮到長邊不超過 `max`。本來就夠小就原樣返回。
fn fit(w: u32, h: u32, max: u32) -> (u32, u32) {
    let long = w.max(h);
    if long <= max || long == 0 {
        return (w, h);
    }
    let scale = max as f64 / long as f64;
    (
        ((w as f64 * scale).round() as u32).max(1),
        ((h as f64 * scale).round() as u32).max(1),
    )
}

/// 工作站是否鎖定。
///
/// 鎖定時輸入桌面切到 `Winlogon`，本程序就打不開它了。這比去猜
/// 「畫面是不是全黑」可靠得多。
pub fn session_locked() -> bool {
    unsafe {
        match OpenInputDesktop(Default::default(), false, DESKTOP_SWITCHDESKTOP) {
            Ok(desk) => {
                let _ = CloseDesktop(desk);
                false
            }
            Err(_) => true,
        }
    }
}

/// 前景視窗所在的那一台螢幕，以及它的桌面座標矩形。
fn focused_monitor(hwnd: HWND) -> Option<(HMONITOR, RECT)> {
    unsafe {
        // hwnd 為 null 時這個旗標會給主螢幕，正是我們要的退路
        let mon = MonitorFromWindow(hwnd, MONITOR_DEFAULTTOPRIMARY);
        if mon.0.is_null() {
            return None;
        }
        let mut info = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        GetMonitorInfoW(mon, &mut info).ok().ok()?;
        Some((mon, info.rcMonitor))
    }
}

/// 真正搬像素的地方。回傳 RGBA8。
///
/// # Safety
/// 呼叫端要保證 `rect` 是一個有效的桌面矩形，且長寬為正。
unsafe fn blit(
    rect: RECT,
    src_w: i32,
    src_h: i32,
    dst_w: u32,
    dst_h: u32,
    sampling: Sampling,
) -> Result<Vec<u8>> {
    unsafe {
        let screen = GetDC(None);
        if screen.is_invalid() {
            bail!("GetDC(NULL) failed");
        }

        // 從這裡開始每一條失敗路徑都要收乾淨，所以不用 `?`
        let result = blit_inner(screen, rect, src_w, src_h, dst_w, dst_h, sampling);
        ReleaseDC(None, screen);
        result
    }
}

unsafe fn blit_inner(
    screen: HDC,
    rect: RECT,
    src_w: i32,
    src_h: i32,
    dst_w: u32,
    dst_h: u32,
    sampling: Sampling,
) -> Result<Vec<u8>> {
    unsafe {
        let mem = CreateCompatibleDC(Some(screen));
        if mem.is_invalid() {
            bail!("CreateCompatibleDC failed");
        }
        let bmp = CreateCompatibleBitmap(screen, dst_w as i32, dst_h as i32);
        if bmp.is_invalid() {
            let _ = DeleteDC(mem);
            bail!("CreateCompatibleBitmap failed");
        }
        let old = SelectObject(mem, HGDIOBJ(bmp.0));

        let copied = if dst_w as i32 == src_w && dst_h as i32 == src_h {
            // CAPTUREBLT 才拍得到分層視窗（很多 IME、下拉選單是分層的）
            BitBlt(
                mem,
                0,
                0,
                src_w,
                src_h,
                Some(screen),
                rect.left,
                rect.top,
                SRCCOPY | CAPTUREBLT,
            )
            .is_ok()
        } else {
            // HALFTONE 是唯一會做面積平均的模式，縮完的字 OCR 還讀得出來——
            // 代價是它逐一平均每個來源像素，實測比整張原生 BitBlt 還貴。
            // 只看輪廓的消費者（探測圖的 9×8 雜湊）不需要付這筆錢。
            // 文件要求 HALFTONE 設完之後補一次 SetBrushOrgEx。
            match sampling {
                Sampling::Area => {
                    SetStretchBltMode(mem, HALFTONE);
                    let _ = SetBrushOrgEx(mem, 0, 0, None);
                }
                Sampling::Fast => {
                    SetStretchBltMode(mem, COLORONCOLOR);
                }
            }
            StretchBlt(
                mem,
                0,
                0,
                dst_w as i32,
                dst_h as i32,
                Some(screen),
                rect.left,
                rect.top,
                src_w,
                src_h,
                SRCCOPY | CAPTUREBLT,
            )
            .as_bool()
        };

        // **先把 bitmap 從 DC 裡選出來再讀。** GetDIBits 的文件寫得很明白：
        // 「hbmp 指定的 bitmap 在呼叫時不可以是被選進某個 DC 的狀態」。
        // 違反它多數時候還是拿得到像素，所以這種 bug 會活很久——直到某台
        // 機器上它開始回傳 0 條掃描線，而症狀是「她什麼都記不住」。
        SelectObject(mem, old);

        let out = if copied {
            read_pixels(mem, bmp, dst_w, dst_h)
        } else {
            Err(anyhow::anyhow!("BitBlt/StretchBlt failed"))
        };

        let _ = DeleteObject(HGDIOBJ(bmp.0));
        let _ = DeleteDC(mem);
        out
    }
}

/// 把 GDI bitmap 讀成 RGBA8。
unsafe fn read_pixels(mem: HDC, bmp: HBITMAP, w: u32, h: u32) -> Result<Vec<u8>> {
    unsafe {
        let mut info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w as i32,
                // 負的高度 = top-down，不然拿到的是上下顛倒的圖
                biHeight: -(h as i32),
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };

        let mut buf = vec![0u8; (w as usize) * (h as usize) * 4];
        let scanned = GetDIBits(
            mem,
            bmp,
            0,
            h,
            Some(buf.as_mut_ptr().cast()),
            &mut info,
            DIB_RGB_COLORS,
        );
        if scanned == 0 {
            bail!("GetDIBits returned no scanlines");
        }

        // GDI 給的是 BGRX，我們要 RGBA
        for px in buf.chunks_exact_mut(4) {
            px.swap(0, 2);
            px[3] = 255;
        }
        Ok(buf)
    }
}

#[cfg(test)]
mod tests {
    use super::{RawFrame, fit};

    #[test]
    fn fit_leaves_small_images_alone() {
        assert_eq!(fit(800, 600, 1568), (800, 600));
        assert_eq!(fit(1568, 900, 1568), (1568, 900));
    }

    #[test]
    fn fit_scales_by_the_long_edge_and_keeps_aspect() {
        // 4K 橫向
        assert_eq!(fit(3840, 2160, 1568), (1568, 882));
        // 直立螢幕：長邊是高度
        assert_eq!(fit(2160, 3840, 1568), (882, 1568));
        // 正方形
        assert_eq!(fit(4000, 4000, 1000), (1000, 1000));
    }

    #[test]
    fn fit_never_returns_a_zero_dimension() {
        // 極端長寬比縮完會讓短邊四捨五入成 0，那會做出一張沒有像素的圖
        let (w, h) = fit(10_000, 3, 100);
        assert!(w >= 1 && h >= 1, "got {w}x{h}");
        assert_eq!(fit(0, 0, 100), (0, 0));
    }

    /// 真的去抓這台機器的螢幕，把每一種抓法的耗時印出來。
    ///
    /// 存在的理由：探測圖這條路是我**推論**出來的便宜——「`GetDIBits` 從
    /// 14MB 掉到 147KB」——然後在真的 Windows 上量到探測和抓圖一樣貴。
    /// 縮小目的地並不會讓來源變小：錢花在把整個畫面從顯示驅動搬過來，
    /// 而那筆錢跟你要縮到多小關係不大。推論算得很漂亮，方向卻是反的。
    ///
    /// 所以這裡不再推論。時間會印進 CI log，換一種抓法就換一組數字。
    /// 它**不對耗時下斷言**（那會隨機器飄），只斷言一件真的會壞的事：
    /// 同一個靜止畫面，`Fast` 取樣算出來的雜湊必須是穩定的——不穩的話
    /// 每一幀都會被判成「新的」，去重整個失效，而症狀是磁碟爆掉。
    #[test]
    fn how_expensive_is_each_way_of_grabbing_this_screen() {
        use super::{OCR_LONG_EDGE, PROBE_LONG_EDGE, Sampling, WindowsScreen};
        use std::time::Instant;

        const ROUNDS: u32 = 8;
        let mut s = WindowsScreen::new();

        // 先確認這台機器抓得到畫面。抓不到（session 0、鎖屏）不是失敗，
        // 但要講出來——不然這個測試會變成一個永遠亮綠燈的空殼。
        match s.capture(0, PROBE_LONG_EDGE, Sampling::Fast) {
            Ok(Some(_)) => {}
            Ok(None) => {
                println!("略過：這個 session 抓不到畫面（鎖屏或沒有互動桌面）");
                return;
            }
            Err(e) => {
                println!("略過：抓圖失敗 {e:#}");
                return;
            }
        }

        let ways = [
            ("原生 / 面積平均", OCR_LONG_EDGE, Sampling::Area),
            ("256 / 面積平均", PROBE_LONG_EDGE, Sampling::Area),
            ("256 / 丟像素", PROBE_LONG_EDGE, Sampling::Fast),
            ("512 / 丟像素", 512, Sampling::Fast),
        ];

        // 量測順序本身會污染結果。上一版是「一種抓法連跑 8 次、換下一種」，
        // 於是 CI 上量到 512px 比 256px **還快**——第一種抓法替後面所有人
        // 付了 DC 建立與快取預熱的錢。一個順序敏感的基準會給出順序敏感的
        // 結論，而那種結論剛好長得像真的。
        //
        // 改成：先各跑一輪熱身（不計時），再一輪一輪輪流跑、各自累加。
        // 這樣任何隨時間的漂移（別的 job 在同一台機器上搶 CPU）會平均攤到
        // 每一種抓法身上，而不是全壓在第一個。
        let mut total = vec![std::time::Duration::ZERO; ways.len()];
        let mut shape = vec![None; ways.len()];

        for (i, (_, edge, sampling)) in ways.iter().enumerate() {
            if let Ok(Some(f)) = s.capture(-1, *edge, *sampling) {
                shape[i] = Some((f.width, f.height));
            }
        }

        println!("每種抓法各 {ROUNDS} 次（已熱身、輪流跑）：");
        'rounds: for r in 0..ROUNDS {
            for (i, (name, edge, sampling)) in ways.iter().enumerate() {
                let t = Instant::now();
                let got = s.capture(r as i64, *edge, *sampling);
                total[i] += t.elapsed();
                if !matches!(got, Ok(Some(_))) {
                    println!("  {name}：中途抓不到了（{got:?} 之類），整張表作廢");
                    break 'rounds;
                }
            }
        }

        for (i, (name, ..)) in ways.iter().enumerate() {
            let Some((w, h)) = shape[i] else { continue };
            println!(
                "  {name}\t{w}x{h}\t每次 {:.1} ms",
                total[i].as_secs_f64() * 1000.0 / ROUNDS as f64
            );
        }

        // 這裡真的下斷言，而且斷言的是一件**會安靜地壞掉**的事。
        //
        // 不能斷言「雜湊四次都一樣」：真實桌面上有時鐘和游標，那個測試會
        // 因為跳秒而變紅，最後被人加上 #[ignore]，比沒有更糟。
        //
        // 能斷言的是這個：換了取樣模式之後，探測圖不可以變成一整片同色。
        // `COLORONCOLOR` 如果在某台機器上失敗或取樣取歪了，最可能的結果
        // 不是報錯，是回一張平的圖——於是每一幀的 dhash 都一樣、每一幀
        // 都被判成重複、她整天什麼都記不住，而錄製摘要一片祥和。
        // 這正是這個專案最主要的失敗形狀，所以它值得一條斷言。
        let spread = |f: &RawFrame| -> u8 {
            let Some(px) = f.rgba.as_deref() else {
                return 0;
            };
            let (lo, hi) = px
                .chunks_exact(4)
                .fold((255u8, 0u8), |(lo, hi), p| (lo.min(p[0]), hi.max(p[0])));
            hi - lo
        };

        let Ok(Some(area)) = s.capture(0, PROBE_LONG_EDGE, Sampling::Area) else {
            return;
        };
        let Ok(Some(fast)) = s.capture(1, PROBE_LONG_EDGE, Sampling::Fast) else {
            return;
        };
        println!(
            "亮度範圍：面積平均 {}、丟像素 {}",
            spread(&area),
            spread(&fast)
        );

        // 面積平均那張本來就是平的（純色桌布、全黑螢幕），就沒得比。
        if spread(&area) >= 16 {
            assert!(
                spread(&fast) >= 8,
                "換成丟像素之後探測圖變平了（面積平均 {}、丟像素 {}）：\
                 每一幀的雜湊都會一樣，去重會把所有東西都當成重複",
                spread(&area),
                spread(&fast)
            );
        }
    }
}
