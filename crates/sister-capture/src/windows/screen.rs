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
//! - **在 GDI 裡就縮好圖**。縮圖後 4K 從 33MB 掉到 5MB，而我們本來就只存
//!   縮圖，沒有理由先把全解析度的位元組搬進使用者空間再丟掉。

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

pub struct WindowsScreen {
    /// 縮圖後的長邊上限（像素）。
    max_long_edge: u32,
    /// HMONITOR → 穩定的小整數。指標本身太大也不穩，但同一個 session 裡
    /// 出現順序是穩的，而 frames 表只需要「同不同台」這個資訊。
    monitors: Vec<isize>,
}

impl WindowsScreen {
    pub fn new(max_long_edge: u32) -> Self {
        Self {
            max_long_edge: max_long_edge.max(64),
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
}

impl ScreenSource for WindowsScreen {
    fn grab(&mut self, ts: Millis) -> Result<Option<RawFrame>> {
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

        let (dst_w, dst_h) = fit(src_w as u32, src_h as u32, self.max_long_edge);
        let monitor = self.monitor_index(mon);
        let rgba = unsafe { blit(rect, src_w, src_h, dst_w, dst_h)? };

        Ok(Some(RawFrame::from_rgba(ts, monitor, dst_w, dst_h, rgba)))
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
            // HALFTONE 是唯一會做面積平均的模式；其餘模式縮完文字會糊到 OCR 認不出來。
            // 文件要求設完之後補一次 SetBrushOrgEx。
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
    use super::fit;

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
}
