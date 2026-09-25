//! 讀出畫面上每一扇看得見的頂層視窗的原始事實，交給 [`crate::own_windows`] 判斷。
//!
//! 這裡只讀、不判斷。而且**不准呼叫任何會送視窗訊息的 API**（`GetWindowTextW`、
//! `SendMessage*` 之類）：recorder 可能就跑在她自己的行程裡，送一則訊息給她
//! 自己的 UI 執行緒、而那條執行緒正在等 recorder，兩邊就卡死。底下用到的每一支
//! 都只讀視窗管理員或 DWM 手上的狀態：`EnumWindows`、`IsWindowVisible`、`IsIconic`、
//! `GetWindowThreadProcessId`、`OpenProcess`＋`QueryFullProcessImageNameW`、
//! `DwmGetWindowAttribute`、`GetWindowLongW`、`GetLayeredWindowAttributes`、
//! `GetWindowRgnBox`、`GetWindowRect`。

use std::collections::HashMap;

use anyhow::{Context, Result};
use windows::Win32::Foundation::{HWND, LPARAM, RECT};
use windows::Win32::Graphics::Dwm::{
    DWMWA_CLOAKED, DWMWA_EXTENDED_FRAME_BOUNDS, DwmGetWindowAttribute,
};
use windows::Win32::Graphics::Gdi::{GetWindowRgnBox, RGN_ERROR};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GWL_EXSTYLE, GetLayeredWindowAttributes, GetWindowLongW, GetWindowRect, IsIconic,
    IsWindowVisible, LAYERED_WINDOW_ATTRIBUTES_FLAGS, LWA_ALPHA, LWA_COLORKEY, WS_EX_LAYERED,
};
use windows::core::BOOL;

use super::focus::{file_name, process_id, process_image_path_for_pid};
use crate::own_windows::{DesktopRect, LayeredAttributes, WindowFacts, layered_is_see_through};

/// 每一扇看得見的頂層視窗，z-order 最上面的排第一個（`EnumWindows` 的順序）。
///
/// 看不見的直接不收：`own_parts` 本來就跳過它們，而畫面上幾百扇視窗大多是
/// 看不見的，沒有理由每一扇都去開一次程序問檔名。
pub(crate) fn window_facts() -> Result<Vec<WindowFacts>> {
    let mut windows: Vec<HWND> = Vec::new();
    // SAFETY: `lparam` 指向上面這個 Vec，`EnumWindows` 回來之前它都活著，
    // 而 callback 只在這一條執行緒上、一次一個地被叫。
    unsafe {
        EnumWindows(
            Some(collect),
            LPARAM(&mut windows as *mut Vec<HWND> as isize),
        )
    }
    .context("列不出畫面上的視窗，這一拍沒有抓畫面")?;

    // 同一個程式通常開好幾扇；pid 只在這一次列舉裡快取，下一拍重問——pid 會被回收。
    let mut names: HashMap<u32, Option<String>> = HashMap::new();
    Ok(windows
        .into_iter()
        .filter(|&hwnd| unsafe { IsWindowVisible(hwnd) }.as_bool())
        .map(|hwnd| facts(hwnd, &mut names))
        .collect())
}

unsafe extern "system" fn collect(hwnd: HWND, lparam: LPARAM) -> BOOL {
    // SAFETY: 見 `window_facts`。
    let windows = unsafe { &mut *(lparam.0 as *mut Vec<HWND>) };
    windows.push(hwnd);
    // 一律往下列：中途停下來，`EnumWindows` 會回報失敗。
    true.into()
}

/// 一扇**看得見**的視窗（呼叫端已經篩過）。
fn facts(hwnd: HWND, names: &mut HashMap<u32, Option<String>>) -> WindowFacts {
    let exe_name = process_id(hwnd).and_then(|pid| {
        names
            .entry(pid)
            .or_insert_with(|| process_image_path_for_pid(pid).map(|path| file_name(&path)))
            .clone()
    });
    let ex_style = unsafe { GetWindowLongW(hwnd, GWL_EXSTYLE) } as u32;
    let layered = ex_style & WS_EX_LAYERED.0 != 0;
    let attributes = if layered {
        layered_attributes(hwnd)
    } else {
        None
    };
    let mut region = RECT::default();
    WindowFacts {
        exe_name,
        visible: true,
        minimized: unsafe { IsIconic(hwnd) }.as_bool(),
        cloaked: cloaked(hwnd),
        see_through: layered_is_see_through(layered, attributes),
        // 沒有 `SetWindowRgn` 設過形狀的視窗回 `RGN_ERROR`（出錯也是它）。
        shaped: unsafe { GetWindowRgnBox(hwnd, &mut region) } != RGN_ERROR,
        window_rect: window_rect(hwnd),
        frame_bounds: frame_bounds(hwnd),
    }
}

fn cloaked(hwnd: HWND) -> Option<bool> {
    let mut value = 0u32;
    unsafe {
        DwmGetWindowAttribute(
            hwnd,
            DWMWA_CLOAKED,
            (&mut value as *mut u32).cast(),
            size_of::<u32>() as u32,
        )
    }
    .ok()
    .map(|()| value != 0)
}

fn layered_attributes(hwnd: HWND) -> Option<LayeredAttributes> {
    let mut alpha = 0u8;
    let mut flags = LAYERED_WINDOW_ATTRIBUTES_FLAGS(0);
    unsafe { GetLayeredWindowAttributes(hwnd, None, Some(&mut alpha), Some(&mut flags)) }.ok()?;
    Some(LayeredAttributes {
        color_key: flags.0 & LWA_COLORKEY.0 != 0,
        alpha: (flags.0 & LWA_ALPHA.0 != 0).then_some(alpha),
    })
}

fn window_rect(hwnd: HWND) -> Option<DesktopRect> {
    let mut rect = RECT::default();
    unsafe { GetWindowRect(hwnd, &mut rect) }.ok()?;
    Some(desktop_rect(rect))
}

fn frame_bounds(hwnd: HWND) -> Option<DesktopRect> {
    let mut rect = RECT::default();
    unsafe {
        DwmGetWindowAttribute(
            hwnd,
            DWMWA_EXTENDED_FRAME_BOUNDS,
            (&mut rect as *mut RECT).cast(),
            size_of::<RECT>() as u32,
        )
    }
    .ok()?;
    Some(desktop_rect(rect))
}

/// 錄製的行程是 per-monitor DPI aware（`enable_dpi_awareness`；桌面程式靠
/// manifest），所以 `GetWindowRect`、DWM 外框和 `rcMonitor` 都是同一套實體像素。
pub(crate) fn desktop_rect(rect: RECT) -> DesktopRect {
    DesktopRect {
        left: rect.left,
        top: rect.top,
        right: rect.right,
        bottom: rect.bottom,
    }
}
