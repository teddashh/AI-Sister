//! 她自己的視窗，在真的 Windows 桌面上被塗掉。
//!
//! Linux 那一半（`own_windows_mask.rs`）驗的是規則；這一支驗的是 EnumWindows／
//! DWM 讀回來的東西、座標，和正式的 `WindowsScreen::capture` 接起來之後對不對。
//! Linux 的 `cargo test` 連編都不編它，這是 Win32 那一半唯一被執行到的地方。
//!
//! 測試把自己的執行檔複製一份叫 `sister-desktop.exe` 當成「她」，複製成別的名字
//! 當成別人，讓那份複本開一扇純色視窗。認的是正式那份名單——產品程式碼裡沒有
//! 任何給測試用的開關。
#![cfg(windows)]

use std::io::{BufRead, BufReader, Read, Write};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use sister_capture::RawFrame;
use sister_capture::scale::OCR_LONG_EDGE;
use sister_capture::windows::screen::WindowsScreen;
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Dwm::{
    DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_DONOTROUND, DwmFlush, DwmSetWindowAttribute,
};
use windows::Win32::Graphics::Gdi::{
    CreateSolidBrush, GetMonitorInfoW, MONITOR_DEFAULTTOPRIMARY, MONITORINFO, MonitorFromWindow,
    UpdateWindow,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DispatchMessageW, GetForegroundWindow, GetMessageW,
    HWND_TOPMOST, LWA_ALPHA, MSG, PM_REMOVE, PeekMessageW, RegisterClassW, SW_SHOWNOACTIVATE,
    SWP_NOACTIVATE, SWP_SHOWWINDOW, SetLayeredWindowAttributes, SetWindowPos, ShowWindow,
    TranslateMessage, WNDCLASSW, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST,
    WS_POPUP,
};
use windows::core::w;

const HELPER: &str = "SISTER_OWN_WINDOW_HELPER";
const HELPER_RECT: &str = "SISTER_OWN_WINDOW_RECT";
const HELPER_COLOR: &str = "SISTER_OWN_WINDOW_COLOR";
const HELPER_ALPHA: &str = "SISTER_OWN_WINDOW_ALPHA";
/// 輔助視窗畫好之後印的那一行。libtest 單執行緒時會先印 `test 名字 ... `
/// 不換行，所以這一行可能接在它後面，只比結尾。
const READY: &str = "SISTER_OWN_WINDOW_READY";

const MAGENTA: [u8; 3] = [255, 0, 255];
const CYAN: [u8; 3] = [0, 255, 255];
const GREEN: [u8; 3] = [0, 255, 0];

// ─── 輔助視窗：被複製出去的那一份執行檔跑的是這一條 ────────────────────

/// 平常直接回傳。環境變數有設的時候，這一個行程就是一扇純色、最上層、不搶
/// 焦點的視窗，印出 [`READY`] 之後一直等到被砍掉。
#[test]
fn own_window_helper() {
    if std::env::var(HELPER).as_deref() != Ok("1") {
        return;
    }
    let rect = numbers(&std::env::var(HELPER_RECT).expect(HELPER_RECT));
    let color = numbers(&std::env::var(HELPER_COLOR).expect(HELPER_COLOR));
    let alpha = std::env::var(HELPER_ALPHA)
        .ok()
        .map(|value| value.parse::<u8>().expect(HELPER_ALPHA));
    let [x, y, width, height] = rect[..] else {
        panic!("{HELPER_RECT} 要四個數字");
    };
    let [r, g, b] = color[..] else {
        panic!("{HELPER_COLOR} 要三個數字");
    };
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let instance = GetModuleHandleW(None).expect("GetModuleHandleW");
        let class = w!("SisterOwnWindowHelper");
        let brush = CreateSolidBrush(COLORREF(r as u32 | (g as u32) << 8 | (b as u32) << 16));
        let window_class = WNDCLASSW {
            lpfnWndProc: Some(window_proc),
            hInstance: instance.into(),
            hbrBackground: brush,
            lpszClassName: class,
            ..Default::default()
        };
        assert_ne!(RegisterClassW(&window_class), 0, "RegisterClassW");
        let mut ex_style = WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE;
        if alpha.is_some() {
            ex_style |= WS_EX_LAYERED;
        }
        let hwnd = CreateWindowExW(
            ex_style,
            class,
            w!("AI-Sister own window helper"),
            WS_POPUP,
            x,
            y,
            width,
            height,
            None,
            None,
            Some(instance.into()),
            None,
        )
        .expect("CreateWindowExW");
        if let Some(alpha) = alpha {
            // layered 視窗在設屬性之前根本不會畫出來。
            SetLayeredWindowAttributes(hwnd, COLORREF(0), alpha, LWA_ALPHA)
                .expect("SetLayeredWindowAttributes");
        }
        // Windows 11 會替某些視窗切圓角；角落露出底下的東西會讓「一格都不准是
        // 洋紅色」那條斷言紅在不相干的地方。舊版 Windows 沒有這個屬性，失敗無妨。
        let corner = DWMWCP_DONOTROUND;
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_WINDOW_CORNER_PREFERENCE,
            std::ptr::from_ref(&corner).cast(),
            size_of_val(&corner) as u32,
        );
        let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        SetWindowPos(
            hwnd,
            Some(HWND_TOPMOST),
            x,
            y,
            width,
            height,
            SWP_NOACTIVATE | SWP_SHOWWINDOW,
        )
        .expect("SetWindowPos");
        let _ = UpdateWindow(hwnd);
        let mut msg = MSG::default();
        while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        let _ = DwmFlush();
        println!("{READY}");
        std::io::stdout().flush().expect("flush");
        loop {
            let got = GetMessageW(&mut msg, None, 0, 0);
            if got.0 <= 0 {
                break;
            }
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
    std::process::exit(0);
}

unsafe extern "system" fn window_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

fn numbers(text: &str) -> Vec<i32> {
    text.split(',')
        .map(|n| n.trim().parse().expect("數字"))
        .collect()
}

// ─── 主測試那一邊 ──────────────────────────────────────────────────────

/// 桌面座標，左上角加寬高。
#[derive(Debug, Clone, Copy)]
struct Area {
    x: i32,
    y: i32,
    w: i32,
    h: i32,
}

impl Area {
    fn contains(&self, x: i32, y: i32) -> bool {
        self.x <= x && x < self.x + self.w && self.y <= y && y < self.y + self.h
    }
}

/// 一份被複製出去、開著一扇純色視窗的執行檔。掉出範圍就砍掉、刪乾淨。
struct Helper {
    child: Child,
    dir: PathBuf,
}

impl Helper {
    fn spawn(name: &'static str, area: Area, color: [u8; 3], alpha: Option<u8>) -> Helper {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let dir = std::env::temp_dir().join(format!(
            "sister-own-window-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let exe = dir.join(name);
        std::fs::copy(std::env::current_exe().expect("current_exe"), &exe).expect("copy exe");
        let mut command = Command::new(&exe);
        command
            .args([
                "own_window_helper",
                "--exact",
                "--nocapture",
                "--test-threads=1",
            ])
            .env(HELPER, "1")
            .env(
                HELPER_RECT,
                format!("{},{},{},{}", area.x, area.y, area.w, area.h),
            )
            .env(
                HELPER_COLOR,
                format!("{},{},{}", color[0], color[1], color[2]),
            )
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        match alpha {
            Some(alpha) => command.env(HELPER_ALPHA, alpha.to_string()),
            None => command.env_remove(HELPER_ALPHA),
        };
        let mut child = command.spawn().expect("spawn helper");
        let stdout = child.stdout.take().expect("stdout");
        let (ready, said) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                if line.trim_end().ends_with(READY) {
                    let _ = ready.send(());
                }
            }
        });
        if said.recv_timeout(Duration::from_secs(30)).is_err() {
            let _ = child.kill();
            let _ = child.wait();
            let mut stderr = String::new();
            if let Some(mut pipe) = child.stderr.take() {
                let _ = pipe.read_to_string(&mut stderr);
            }
            let _ = std::fs::remove_dir_all(&dir);
            panic!("{name} 三十秒內沒有畫好（{READY}）。stderr：{stderr}");
        }
        Helper { child, dir }
    }
}

impl Drop for Helper {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        // 執行檔剛結束時可能還被鎖著；刪不掉就留在暫存資料夾裡。
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// 擷取會抓的那一台：前景視窗所在的螢幕，和 `WindowsScreen::capture` 同一個問法。
fn captured_monitor() -> RECT {
    unsafe {
        let monitor = MonitorFromWindow(GetForegroundWindow(), MONITOR_DEFAULTTOPRIMARY);
        let mut info = MONITORINFO {
            cbSize: size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        assert!(
            GetMonitorInfoW(monitor, &mut info).as_bool(),
            "GetMonitorInfoW"
        );
        info.rcMonitor
    }
}

fn grab(screen: &mut WindowsScreen) -> RawFrame {
    screen
        .capture(sister_core::now_ms(), OCR_LONG_EDGE)
        .expect("capture")
        .expect("工作站沒鎖，應該抓得到畫面")
}

fn pixel(frame: &RawFrame, monitor: RECT, x: i32, y: i32) -> [u8; 4] {
    let rgba = frame.rgba.as_ref().expect("RGBA");
    let (fx, fy) = ((x - monitor.left) as usize, (y - monitor.top) as usize);
    let at = (fy * frame.width as usize + fx) * 4;
    [rgba[at], rgba[at + 1], rgba[at + 2], rgba[at + 3]]
}

/// `inside` 裡、而且不在 `outside` 裡的每一個像素。
fn pixels(frame: &RawFrame, monitor: RECT, inside: Area, outside: Option<Area>) -> Vec<[u8; 4]> {
    let mut out = Vec::new();
    for y in inside.y..inside.y + inside.h {
        for x in inside.x..inside.x + inside.w {
            if outside.is_some_and(|o| o.contains(x, y)) {
                continue;
            }
            out.push(pixel(frame, monitor, x, y));
        }
    }
    out
}

fn is(color: [u8; 3]) -> impl Fn(&[u8; 4]) -> bool {
    move |px| (0..3).all(|i| px[i].abs_diff(color[i]) <= 24)
}

fn is_black(px: &[u8; 4]) -> bool {
    *px == [0, 0, 0, 255]
}

fn is_greenish(px: &[u8; 4]) -> bool {
    px[1] as i32 > px[0] as i32 + 40 && px[1] as i32 > px[2] as i32 + 40
}

fn share(pixels: &[[u8; 4]], test: impl Fn(&[u8; 4]) -> bool) -> f64 {
    pixels.iter().filter(|px| test(px)).count() as f64 / pixels.len().max(1) as f64
}

/// 一塊裡有幾個洋紅色（她的顏色）的像素。
fn magenta(frame: &RawFrame, monitor: RECT, area: Area) -> usize {
    let hers = is(MAGENTA);
    pixels(frame, monitor, area, None)
        .iter()
        .filter(|px| hers(px))
        .count()
}

/// 失敗時印出來的東西：每一塊裡各種顏色各有幾格。
fn describe(frame: &RawFrame, monitor: RECT, areas: &[(&str, Area, Option<Area>)]) -> String {
    let mut out = format!(
        "畫面 {}×{}，螢幕 {:?}\n",
        frame.width,
        frame.height,
        (monitor.left, monitor.top, monitor.right, monitor.bottom),
    );
    for (name, inside, outside) in areas {
        let px = pixels(frame, monitor, *inside, *outside);
        let count = |test: &dyn Fn(&[u8; 4]) -> bool| px.iter().filter(|p| test(p)).count();
        out += &format!(
            "  {name}：{} 格，洋紅 {}、黑 {}、綠 {}、青 {}、偏綠 {}\n",
            px.len(),
            count(&is(MAGENTA)),
            count(&is_black),
            count(&is(GREEN)),
            count(&is(CYAN)),
            count(&is_greenish),
        );
    }
    out
}

/// 每 200 ms 抓一張，等到 `ready` 成立；十秒還沒有就帶著 `describe` 紅掉。
fn wait_for(
    screen: &mut WindowsScreen,
    what: &str,
    monitor: RECT,
    areas: &[(&str, Area, Option<Area>)],
    ready: impl Fn(&RawFrame) -> bool,
) -> RawFrame {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let frame = grab(screen);
        if ready(&frame) {
            return frame;
        }
        if Instant::now() > deadline {
            panic!("{what}\n{}", describe(&frame, monitor, areas));
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

#[test]
#[ignore = "owns top-most windows on the real desktop; CI runs it alone"]
fn her_visible_windows_are_blanked_in_her_own_capture() {
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
    let monitor = captured_monitor();
    let (mx, my) = (monitor.left, monitor.top);
    let her = Area {
        x: mx + 80,
        y: my + 80,
        w: 240,
        h: 160,
    };
    let beacon = Area {
        x: mx + 80,
        y: my + 320,
        w: 120,
        h: 120,
    };
    let over = Area {
        x: mx + 200,
        y: my + 120,
        w: 240,
        h: 160,
    };
    // 她那一塊連同外圍一圈：縮圖或座標差一格，她的顏色會漏在邊上。整張螢幕
    // 不數——桌面上本來就可能有洋紅色的圖示，那和她無關。
    let around = Area {
        x: her.x - 24,
        y: her.y - 24,
        w: her.w + 48,
        h: her.h + 48,
    };
    let overlap = Area {
        x: over.x,
        y: over.y,
        w: her.x + her.w - over.x,
        h: her.y + her.h - over.y,
    };
    let areas = [
        ("她連同外圍一圈", around, None),
        ("她（R）", her, None),
        ("她沒被蓋住的那塊（R∖R2）", her, Some(over)),
        ("重疊（R∩R2）", overlap, None),
        ("上面那扇露出來的（R2∖R）", over, Some(her)),
        ("對照（B）", beacon, None),
    ];
    let mut screen = WindowsScreen::new();

    let first = grab(&mut screen);
    assert_eq!(
        (first.width as i32, first.height as i32),
        (monitor.right - monitor.left, monitor.bottom - monitor.top),
        "這一支的座標換算假設畫面沒有縮圖；這台螢幕比 {OCR_LONG_EDGE} 還寬，要改測試"
    );
    assert_eq!(
        magenta(&first, monitor, around),
        0,
        "還沒開任何視窗，她那一塊附近就有洋紅色，底下分不出是誰的\n{}",
        describe(&first, monitor, &areas)
    );

    // 1) 對照組：同一個位置、同一個顏色，名字不是她的時候要看得到。
    {
        let _not_her = Helper::spawn("not-her.exe", her, MAGENTA, None);
        wait_for(
            &mut screen,
            "對照組：別人的洋紅色視窗一直沒出現在擷取裡，底下每一條都證明不了什麼",
            monitor,
            &areas,
            |frame| share(&pixels(frame, monitor, her, None), is(MAGENTA)) >= 0.9,
        );
    }

    // 2) 她：先開她，再開一扇青色的對照，等到對照出現，她那一塊要全黑。
    let _her = Helper::spawn("sister-desktop.exe", her, MAGENTA, None);
    {
        let _beacon = Helper::spawn("not-her.exe", beacon, CYAN, None);
        let frame = wait_for(
            &mut screen,
            "青色對照視窗一直沒出現在擷取裡",
            monitor,
            &areas,
            |frame| share(&pixels(frame, monitor, beacon, None), is(CYAN)) >= 0.9,
        );
        for frame in [frame, grab(&mut screen), grab(&mut screen)] {
            let why = describe(&frame, monitor, &areas);
            assert_eq!(
                magenta(&frame, monitor, around),
                0,
                "她那一塊附近還看得到她的顏色\n{why}"
            );
            assert!(
                pixels(&frame, monitor, her, None).iter().all(is_black),
                "她那一塊沒有整塊塗黑\n{why}"
            );
        }
    }

    // 3) 一扇不透明的別人蓋在她上面：蓋住的那一塊是別人的，其餘還是黑的。
    {
        let _above = Helper::spawn("occluder.exe", over, GREEN, None);
        let frame = wait_for(
            &mut screen,
            "綠色那扇一直沒出現在擷取裡",
            monitor,
            &areas,
            |frame| share(&pixels(frame, monitor, over, Some(her)), is(GREEN)) >= 0.9,
        );
        let why = describe(&frame, monitor, &areas);
        assert!(
            share(&pixels(&frame, monitor, overlap, None), is(GREEN)) >= 0.9,
            "蓋在她上面的那一塊是別人的畫面，不該被塗掉\n{why}"
        );
        assert!(
            pixels(&frame, monitor, her, Some(over))
                .iter()
                .all(is_black),
            "她沒被蓋住的那一塊要全黑\n{why}"
        );
        assert_eq!(
            magenta(&frame, monitor, around),
            0,
            "她那一塊附近還看得到她的顏色\n{why}"
        );
    }

    // 4) 一扇半透明的別人蓋在她上面：擋不住她，她整塊照塗。
    //    要是把它當成擋得住，重疊那一塊會是洋紅和綠混出來的灰。
    {
        let _above = Helper::spawn("occluder.exe", over, GREEN, Some(128));
        let frame = wait_for(
            &mut screen,
            "半透明的綠色那扇一直沒出現在擷取裡",
            monitor,
            &areas,
            |frame| share(&pixels(frame, monitor, over, Some(her)), is_greenish) >= 0.9,
        );
        let why = describe(&frame, monitor, &areas);
        assert!(
            pixels(&frame, monitor, her, None).iter().all(is_black),
            "半透明的視窗底下看得到她，她整塊都要塗黑\n{why}"
        );
        assert_eq!(
            magenta(&frame, monitor, around),
            0,
            "她那一塊附近還看得到她的顏色\n{why}"
        );
    }
}
