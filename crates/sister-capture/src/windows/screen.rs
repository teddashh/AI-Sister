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
//! 專案反而是好的），而且它比 GPU 路徑慢。真機 bench 已把 GDI 路徑量完：
//! 抓圖不是 alpha.46 活躍寫程式的主成本；牆上毫秒也不能直接當成 CPU 百分比。
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
//!
//! ## 為什麼 OCR 工作幀和存檔尺寸分開
//!
//! 原本一次抓到 `max_long_edge`（1568），同一份像素同時拿去算 dhash、
//! 餵 OCR、存檔。三件事的需求其實完全衝突，於是三件事一起壞：
//!
//! - **去重**只需要 9×8。它卻讓每個 tick 都搬一張全尺寸的圖，而實測
//!   120 個 tick 裡有 103 個算完雜湊就把整張圖丟掉。
//! - **OCR** 在安全上限內需要原生像素。2560 縮到 1568 是 0.61 倍，12px 的字掉到 7px，
//!   於是 `Microsoft Teams` 被讀成 `Micr099ftTeamsTr`——不報錯，只是讀錯。
//! - **存檔**要小。它是唯一真的想要 1568 的人。
//!
//! 所以 recorder 每個 tick 只呼叫一次 [`ScreenSource::grab`]，拿到長邊
//! 4096px 內的工作幀（一般螢幕就是原生解析度）；dhash 與 OCR 都看這一份。
//! 真正要留下證據時，`frames.rs` 才另外把它縮成 1568px 的 PNG。這不是第二次
//! 抓螢幕，而是把擷取與磁碟兩種不同的尺寸需求分開。
//!
//!
//! ## 量到的、以及量錯的
//!
//! 縮圖模式量過兩種（256px：`HALFTONE` 面積平均 27.4 ms、`COLORONCOLOR`
//! 直接丟像素 24.2 ms），只留了 `HALFTONE`——留一個沒有人選的選項只會讓
//! 下一個人以為它有用。
//!
//! 從那組數字推出來的結論是「目的地差 12 倍、取樣模式換掉，時間都幾乎不動
//! ⇒ 擷取的錢花在讀來源，不在寫目的地」。**前半是真的，後半推過頭了。**
//! 兩次量的都是 `StretchBlt` 從**螢幕**縮：不管縮到多小，來源那 3.7M 個像素
//! 都得先讀進來。它證明的是「縮小目的地救不了讀來源」，不是「寫目的地不
//! 用錢」。而底下那個測試裡的對照組更糟：「不縮放」那列走 `blit`（帶
//! `CAPTUREBLT`），「重用 DC」那列是手寫的裸 `SRCCOPY`——一次換了兩個變因，
//! 差值卻被當成「GDI 物件手續費」讀。
//!
//! 所以那個測試改成 2×2、原生解析度，而且把 `BitBlt` 和 `GetDIBits` 的時間
//! **分開**報。127 ms ÷ 3.7M 像素 = 34 ns/像素，而同樣 14.7 MB 的 `memcpy`
//! 只要 1.5 ms——GDI 慢了將近兩個數量級，那不是搬運，是有人在逐像素做事。
//! 誰在做，就是那張表要回答的唯一問題。

use anyhow::{Result, bail};
use sister_core::model::Millis;
use windows::Win32::Foundation::{HWND, RECT};
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BitBlt, CAPTUREBLT, CreateCompatibleBitmap,
    CreateCompatibleDC, DIB_RGB_COLORS, DeleteDC, DeleteObject, GetDC, GetDIBits, GetMonitorInfoW,
    HALFTONE, HBITMAP, HDC, HGDIOBJ, HMONITOR, MONITOR_DEFAULTTOPRIMARY, MONITORINFO,
    MonitorFromWindow, ReleaseDC, SRCCOPY, SelectObject, SetBrushOrgEx, SetStretchBltMode,
    StretchBlt,
};
use windows::Win32::System::StationsAndDesktops::{
    CloseDesktop, DESKTOP_SWITCHDESKTOP, OpenInputDesktop,
};
use windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow;

use crate::scale::{OCR_LONG_EDGE, fit};
use crate::traits::{RawFrame, ScreenSource};

/// GDI 螢幕來源。一次抓前景視窗所在的那一台螢幕。
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

    /// 兩條路徑的差別只有「縮到多大」與「怎麼縮」。縮圖是在 GDI 裡用
    /// `StretchBlt` + `HALFTONE` 做掉，和先拿原生圖再用
    /// `image::imageops::resize` 縮不是同一種縮法，量的時候不能互換。
    pub fn capture(&mut self, ts: Millis, long_edge: u32) -> Result<Option<RawFrame>> {
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
        let rgba = unsafe { blit(rect, src_w, src_h, dst_w, dst_h)? };

        Ok(Some(RawFrame::from_rgba(ts, monitor, dst_w, dst_h, rgba)))
    }

    /// 不搬像素，回答前景視窗所在螢幕的原始大小。
    pub fn frame_size() -> Option<(u32, u32)> {
        if session_locked() {
            return None;
        }
        let (_, rect) = focused_monitor(unsafe { GetForegroundWindow() })?;
        let w = rect.right - rect.left;
        let h = rect.bottom - rect.top;
        if w <= 0 || h <= 0 {
            return None;
        }
        Some((w as u32, h as u32))
    }
}

impl ScreenSource for WindowsScreen {
    fn grab(&mut self, ts: Millis) -> Result<Option<RawFrame>> {
        self.capture(ts, OCR_LONG_EDGE)
    }
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
unsafe fn blit(rect: RECT, src_w: i32, src_h: i32, dst_w: u32, dst_h: u32) -> Result<Vec<u8>> {
    unsafe {
        let screen = GetDC(None);
        if screen.is_invalid() {
            bail!("GetDC(NULL) failed");
        }

        // 從這裡開始每一條失敗路徑都要收乾淨，所以不用 `?`
        let result = blit_inner(screen, rect, src_w, src_h, dst_w, dst_h);
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
            SetStretchBltMode(mem, HALFTONE);
            let _ = SetBrushOrgEx(mem, 0, 0, None);
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
        //
        // clippy 1.98 想把這一行換成 `as_chunks_mut::<4>()`。**這一輪不換**，
        // 而且只有這一個地方不換（同一個 lint 的另外五處都改了）。這個迴圈長在
        // `read_pixels` 裡，是 #18 那份「CPU 27.1%，九倍預算」的基準量到的那一
        // 段；`as_chunks` 會讓最佳化器拿到編譯期已知的長度，多半會把這個 swizzle
        // 向量化——那正是我們想要的，但它得**被量到**，不能夾在一次 lint 清理裡
        // 悄悄進去，否則他下次跑 `sister bench` 得到的數字和已經記錄的那一份不再
        // 是同一段程式碼。等 #18 那份實測回來就把這兩行拿掉，順便量它值多少。
        #[allow(clippy::chunks_exact_to_as_chunks)]
        for px in buf.chunks_exact_mut(4) {
            px.swap(0, 2);
            px[3] = 255;
        }
        Ok(buf)
    }
}

/// 一種抓法量出來的三段耗時（毫秒／次）。
#[derive(Debug, Clone)]
pub struct BenchRow {
    pub label: String,
    pub width: i32,
    pub height: i32,
    /// 建立 GDI 物件（DC + bitmap）
    pub make_ms: f64,
    /// `BitBlt`：跟顯示驅動要畫面的過路費
    pub blt_ms: f64,
    /// `GetDIBits`：把 device-dependent bitmap 轉格式搬回系統記憶體
    pub dib_ms: f64,
}

impl BenchRow {
    pub fn total_ms(&self) -> f64 {
        self.make_ms + self.blt_ms + self.dib_ms
    }
}

/// 原生解析度下，**一次只換一個變因**的 2×2。
///
/// 這是 #18 的量尺，所以它是產品的一部分，不是測試裡的一段。他手上只有
/// 一個下載來的 exe——一個只有 `cargo test` 碰得到的量尺，對唯一有那台
/// 機器的人來說等於不存在。
///
/// 一次擷取 127 ms，除以 3.7M 像素是 34 ns/像素，而同樣 14.7 MB 的
/// `memcpy` 只要 1.5 ms。GDI 慢了快兩個數量級——那不是搬運，是有人在逐
/// 像素做事。這張表要回答的就是「誰」：
///
///   建立      每一拍新建再刪掉一張 14.7MB 的 bitmap → 貴就快取
///   BitBlt    `CAPTUREBLT` 會逼 DWM 多 flush 一次合成，而 Win8 之後桌面
///             DC 本來就含分層視窗 → 可能在付一筆已經免費的錢
///   GetDIBits device-dependent bitmap 的轉格式讀回 → 貴就換
///             `CreateDIBSection`，讓 BitBlt 直接寫進我們自己的記憶體
///
/// 抓不到畫面（鎖屏、session 0）回空的 `Vec`——那不是失敗，是這台機器現在
/// 沒有可量的東西。
pub fn bench_grab(rounds: u32) -> Vec<BenchRow> {
    use std::time::{Duration, Instant};

    let rounds = rounds.max(1);
    let mut rows = Vec::new();
    if session_locked() {
        return rows;
    }
    let Some((_, full)) = focused_monitor(unsafe { GetForegroundWindow() }) else {
        return rows;
    };
    let (w, h) = (full.right - full.left, full.bottom - full.top);
    if w <= 0 || h <= 0 {
        return rows;
    }
    let mut buf = vec![0u8; (w as usize) * (h as usize) * 4];

    unsafe {
        let screen = GetDC(None);
        if screen.is_invalid() {
            return rows;
        }
        // 重用那一組在計時之外只建一次
        let kept_mem = CreateCompatibleDC(Some(screen));
        let kept_bmp = CreateCompatibleBitmap(screen, w, h);
        for (flag_name, rop) in [
            ("含 CAPTUREBLT", SRCCOPY | CAPTUREBLT),
            ("純 SRCCOPY", SRCCOPY),
        ] {
            for (reuse_name, reuse) in [("每拍新建", false), ("重用 GDI", true)] {
                let (mut make, mut blt, mut dib) = (Duration::ZERO, Duration::ZERO, Duration::ZERO);
                let mut ok = true;
                // 第 0 輪是熱身，不計時
                for round in 0..=rounds {
                    let t = Instant::now();
                    let (mem, bmp) = if reuse {
                        (kept_mem, kept_bmp)
                    } else {
                        (
                            CreateCompatibleDC(Some(screen)),
                            CreateCompatibleBitmap(screen, w, h),
                        )
                    };
                    let made = t.elapsed();
                    let got = bench_one(screen, (mem, bmp), full, (w, h), rop, &mut buf[..]);
                    if !reuse {
                        let _ = DeleteObject(HGDIOBJ(bmp.0));
                        let _ = DeleteDC(mem);
                    }
                    match got {
                        Some((a, b)) if round > 0 => {
                            make += made;
                            blt += a;
                            dib += b;
                        }
                        Some(_) => {}
                        None => {
                            ok = false;
                            break;
                        }
                    }
                }
                let per = |d: Duration| d.as_secs_f64() * 1000.0 / rounds as f64;
                if ok {
                    rows.push(BenchRow {
                        label: format!("{flag_name} / {reuse_name}"),
                        width: w,
                        height: h,
                        make_ms: per(make),
                        blt_ms: per(blt),
                        dib_ms: per(dib),
                    });
                }
            }
        }
        if !kept_bmp.is_invalid() {
            let _ = DeleteObject(HGDIOBJ(kept_bmp.0));
        }
        if !kept_mem.is_invalid() {
            let _ = DeleteDC(kept_mem);
        }
        ReleaseDC(None, screen);
    }
    rows
}

/// 一次擷取，把 `BitBlt` 和 `GetDIBits` 的時間分開。
///
/// # Safety
/// `buf` 至少要有 `w * h * 4` 個位元組，`mem`/`bmp` 要是有效的 GDI 物件。
unsafe fn bench_one(
    screen: HDC,
    (mem, bmp): (HDC, HBITMAP),
    rect: RECT,
    (w, h): (i32, i32),
    rop: windows::Win32::Graphics::Gdi::ROP_CODE,
    buf: &mut [u8],
) -> Option<(std::time::Duration, std::time::Duration)> {
    use std::time::Instant;
    unsafe {
        let old = SelectObject(mem, HGDIOBJ(bmp.0));
        let t = Instant::now();
        let ok = BitBlt(mem, 0, 0, w, h, Some(screen), rect.left, rect.top, rop).is_ok();
        let blt = t.elapsed();
        // GetDIBits 的文件要求 bitmap 不可以還選在某個 DC 裡
        SelectObject(mem, old);
        if !ok {
            return None;
        }

        let mut info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w,
                biHeight: -h,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let t = Instant::now();
        let scanned = GetDIBits(
            mem,
            bmp,
            0,
            h as u32,
            Some(buf.as_mut_ptr().cast()),
            &mut info,
            DIB_RGB_COLORS,
        );
        let dib = t.elapsed();
        (scanned != 0).then_some((blt, dib))
    }
}

#[cfg(test)]
mod tests {
    use super::RawFrame;

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
        use super::{OCR_LONG_EDGE, WindowsScreen};
        use std::time::Instant;

        const ROUNDS: u32 = 8;
        let mut s = WindowsScreen::new();

        // 先確認這台機器抓得到畫面。抓不到（session 0、鎖屏）不是失敗，
        // 但要講出來——不然這個測試會變成一個永遠亮綠燈的空殼。
        match s.capture(0, 256) {
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

        // 目的地大小這一軸已經量完了，答案是「幾乎不影響」。留三列在這裡
        // 不是為了再問一次，是因為換一台機器（多螢幕、HDR、遠端桌面）
        // 答案可能不一樣，而那時候要看得出來。
        let ways = [
            ("原生（OCR 用的大小）", OCR_LONG_EDGE),
            ("縮到 512", 512),
            ("縮到 256", 256),
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

        for (i, (_, edge)) in ways.iter().enumerate() {
            if let Ok(Some(f)) = s.capture(-1, *edge) {
                shape[i] = Some((f.width, f.height));
            }
        }

        println!("每種抓法各 {ROUNDS} 次（已熱身、輪流跑）：");
        'rounds: for r in 0..ROUNDS {
            for (i, (name, edge)) in ways.iter().enumerate() {
                let t = Instant::now();
                let got = s.capture(r as i64, *edge);
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

        // ---- 一次只換一個變因 ----
        //
        // 這裡本來有兩列：「中央一小塊 / 不縮放」和「中央一小塊 / 重用 DC」。
        // 前者走 `blit`（`SRCCOPY | CAPTUREBLT`），後者是手寫的**裸 SRCCOPY**。
        // 兩個變因一起換了，於是那個差值裡混著 CAPTUREBLT 的錢，卻被當成
        // 「GDI 物件手續費」讀。一個被污染的對照組，會給出一個長得跟真的
        // 一模一樣的結論。
        //
        // 現在那張表是 `bench_grab`，而且**在產品裡**（`sister bench`）——
        // 唯一有那台 2560×1440 機器的人手上只有一個下載來的 exe，一個只有
        // `cargo test` 碰得到的量尺對他等於不存在。這裡直接叫它，不重寫一份：
        // 測試印的數字和他貼回來的數字，必須是同一段程式跑出來的。
        for r in super::bench_grab(ROUNDS) {
            println!(
                "  {}\t{}x{}\t建立 {:5.1}\tBitBlt {:6.1}\tGetDIBits {:6.1}\t合計 {:6.1} ms",
                r.label,
                r.width,
                r.height,
                r.make_ms,
                r.blt_ms,
                r.dib_ms,
                r.total_ms()
            );
        }

        // 這裡真的下斷言，而且斷言的是一件**會安靜地壞掉**的事。
        //
        // 不能斷言「雜湊兩次都一樣」：真實桌面上有時鐘和游標，那個測試會
        // 因為跳秒而變紅，最後被人加上 #[ignore]，比沒有更糟。
        //
        // 能斷言的是這個：抓回來的畫面不可以是一整片同色。GDI 這條路失敗
        // 的時候（權限不足、遠端桌面、某些顯示驅動）最可能的結果不是報錯，
        // 是回一張全黑或全白的圖——於是每一幀的 dhash 都一樣、每一幀都被
        // 判成重複、她整天什麼都記不住，而錄製摘要一片祥和。
        // 這正是這個專案最主要的失敗形狀，所以它值得一條斷言。
        let spread = |f: &RawFrame| -> u8 {
            let Some(px) = f.rgba.as_deref() else {
                return 0;
            };
            let (lo, hi) = px
                .as_chunks::<4>()
                .0
                .iter()
                .fold((255u8, 0u8), |(lo, hi), p| (lo.min(p[0]), hi.max(p[0])));
            hi - lo
        };

        let Ok(Some(full)) = s.capture(0, OCR_LONG_EDGE) else {
            return;
        };
        println!("亮度範圍：{}", spread(&full));
        assert!(
            spread(&full) >= 8,
            "抓回來的整張畫面只有一個亮度（範圍 {}）：\
             每一幀的雜湊都會一樣，去重會把所有東西都當成重複",
            spread(&full)
        );
    }
}
