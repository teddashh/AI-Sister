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

use crate::traits::{RawFrame, ScreenSource};

/// 完整解析度抓圖的長邊上限。
///
/// 這不是畫質旋鈕，是一道防線，所以刻意寫死而不是開成設定：8K 螢幕一張
/// RGBA 是 132MB，而 Windows 的 OCR 引擎本身也只吃到 10000 像素。4096
/// 讓 4K（3840）以下的螢幕全部拿到**原生**像素——也就是絕大多數人——
/// 更大的才縮，而且縮一半仍然遠好過為了它把每個人都降到 1568。
const OCR_LONG_EDGE: u32 = 4096;

/// 縮圖時怎麼取樣。
///
/// 只剩一種：面積平均（`HALFTONE`）。這裡曾經有第二種（`COLORONCOLOR`，
/// 「直接丟像素」），因為探測圖的唯一讀者是一個 9×8 的雜湊、不需要細節。
/// 實測兩種一樣貴（256px：面積平均 27.4 ms、丟像素 24.2 ms），而探測圖
/// 本身也已經不存在了。留一個沒有人選的選項，只會讓下一個人以為它有用。
///
/// 這一段留著是因為它記著一件事：目的地像素差 12 倍、取樣模式換掉，
/// 時間都幾乎不動——**擷取的錢花在讀來源，不在寫目的地。**
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
    pub(crate) fn capture(&mut self, ts: Millis, long_edge: u32) -> Result<Option<RawFrame>> {
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
}

impl ScreenSource for WindowsScreen {
    fn grab(&mut self, ts: Millis) -> Result<Option<RawFrame>> {
        self.capture(ts, OCR_LONG_EDGE)
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
        use super::{
            BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, DeleteDC, DeleteObject, GetDC,
            HGDIOBJ, OCR_LONG_EDGE, RECT, ReleaseDC, SRCCOPY, SelectObject, WindowsScreen, blit,
            read_pixels,
        };
        use std::time::Instant;
        use windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow;

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

        // 上面那張表全部都是「整個螢幕 → 某個大小」。它回答不了下一個問題：
        // 這 27 ms 到底是花在**讀了多少來源像素**上，還是「跟顯示驅動要一次
        // 畫面」這個**固定成本**上？兩個答案通往完全不同的下一步——
        //
        //   花在來源像素   → 只抓螢幕的一小塊當變化偵測就好，很便宜。
        //   固定成本       → 抓多小都一樣，只有 DXGI Desktop Duplication
        //                    （它能在畫面沒變時直接回「沒有新的一張」、
        //                    一個位元組都不搬）救得了。
        //
        // 所以量一次。中央 256×256 一比一、不縮放，來源像素大約是全螢幕的
        // 3%——如果時間也掉到 3%，答案是前者；如果幾乎沒變，答案是後者。
        if let Some((_, full)) = super::focused_monitor(unsafe { GetForegroundWindow() }) {
            const SIDE: i32 = 256;
            let cx = (full.left + full.right) / 2;
            let cy = (full.top + full.bottom) / 2;
            let patch = RECT {
                left: cx - SIDE / 2,
                top: cy - SIDE / 2,
                right: cx + SIDE / 2,
                bottom: cy + SIDE / 2,
            };
            // 熱身一次再計時，理由同上。
            let warm = unsafe { blit(patch, SIDE, SIDE, SIDE as u32, SIDE as u32) };
            if warm.is_ok() {
                let t = Instant::now();
                let mut ok = true;
                for _ in 0..ROUNDS {
                    ok &= unsafe { blit(patch, SIDE, SIDE, SIDE as u32, SIDE as u32) }.is_ok();
                }
                if ok {
                    println!(
                        "  中央一小塊 / 不縮放\t{SIDE}x{SIDE}\t每次 {:.1} ms（來源像素約全螢幕的 {:.0}%）",
                        t.elapsed().as_secs_f64() * 1000.0 / ROUNDS as f64,
                        (SIDE as f64 * SIDE as f64)
                            / ((full.right - full.left) as f64 * (full.bottom - full.top) as f64)
                            * 100.0
                    );
                }
            }
        }

        // 上面那一列告訴我們「來源像素變少 ⇒ 時間變少，但**不成比例**」：
        // 來源砍到 8%，時間只掉到 57%。所以底下還壓著一筆固定成本，而那筆
        // 才是決定要不要去寫 DXGI 的關鍵。
        //
        // 它有兩個嫌疑犯，各自的下一步天差地遠：
        //
        //   GDI 物件的手續費 → 我們**每一次擷取**都新建 DC 和 bitmap 再刪掉。
        //                      如果錢在這裡，改法是快取它們，一天就做得完。
        //   顯示驅動的過路費 → 每次跟驅動要畫面就是要付。那 GDI 這條路
        //                      到頭了，只有 Desktop Duplication（畫面沒變
        //                      時直接回「沒有新的一張」）救得了。
        //
        // 所以再量一次：同一塊 256×256，但 DC 與 bitmap 在計時之外只建一次。
        if let Some((_, full)) = super::focused_monitor(unsafe { GetForegroundWindow() }) {
            const SIDE: i32 = 256;
            let cx = (full.left + full.right) / 2;
            let cy = (full.top + full.bottom) / 2;
            let (x, y) = (cx - SIDE / 2, cy - SIDE / 2);

            unsafe {
                let screen = GetDC(None);
                if !screen.is_invalid() {
                    let mem = CreateCompatibleDC(Some(screen));
                    let bmp = CreateCompatibleBitmap(screen, SIDE, SIDE);
                    if !mem.is_invalid() && !bmp.is_invalid() {
                        let mut ok = true;
                        // 熱身
                        for _ in 0..2 {
                            let old = SelectObject(mem, HGDIOBJ(bmp.0));
                            ok &=
                                BitBlt(mem, 0, 0, SIDE, SIDE, Some(screen), x, y, SRCCOPY).is_ok();
                            SelectObject(mem, old);
                            ok &= read_pixels(mem, bmp, SIDE as u32, SIDE as u32).is_ok();
                        }
                        let t = Instant::now();
                        for _ in 0..ROUNDS {
                            let old = SelectObject(mem, HGDIOBJ(bmp.0));
                            ok &=
                                BitBlt(mem, 0, 0, SIDE, SIDE, Some(screen), x, y, SRCCOPY).is_ok();
                            SelectObject(mem, old);
                            ok &= read_pixels(mem, bmp, SIDE as u32, SIDE as u32).is_ok();
                        }
                        if ok {
                            println!(
                                "  中央一小塊 / 重用 DC\t{SIDE}x{SIDE}\t每次 {:.1} ms（DC 與 bitmap 只建一次）",
                                t.elapsed().as_secs_f64() * 1000.0 / ROUNDS as f64
                            );
                        }
                    }
                    if !bmp.is_invalid() {
                        let _ = DeleteObject(HGDIOBJ(bmp.0));
                    }
                    if !mem.is_invalid() {
                        let _ = DeleteDC(mem);
                    }
                    ReleaseDC(None, screen);
                }
            }
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
                .chunks_exact(4)
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
