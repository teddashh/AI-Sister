# 螢幕記憶 App 技術棧調查(Screen Memory / AI-Sister)

調查日期:2026-08-17。目標:open-source、local-first 的桌面「螢幕記憶」app —— 低成本連續擷取 → 本地 OCR → SQLite FTS → 後續 LLM 解讀。首發 Windows + macOS,Linux 延後。UI shell 傾向 Tauri v2。版本號皆於調查日以 crates.io API 及官方來源逐一驗證。

---

## 1. 螢幕擷取(per OS)

### Windows
- **Windows.Graphics.Capture(WGC, WinRT)**:namespace 自 Win10 1803,但背景 app 不走 picker、要用 `IGraphicsCaptureItemInterop::CreateForWindow/CreateForMonitor`(需 **1903+**,https://learn.microsoft.com/en-us/windows/win32/api/windows.graphics.capture.interop/nf-windows-graphics-capture-interop-igraphicscaptureiteminterop-createforwindow)。流程:`GraphicsCaptureItem` → `Direct3D11CaptureFramePool`(BGRA8, GPU texture)→ `FrameArrived`。per-window 與 per-monitor 都是一等公民;視窗被遮擋仍有 frame(勝過 DDA)。
  - **黃框**:Win10 強制;`IsBorderRequired` 只存在 build 20348+(**實務上 = Win11**;Win10 呼叫直接 `E_NOINTERFACE`),關閉前要 `GraphicsCaptureAccess.RequestAccessAsync(GraphicsCaptureAccessKind.Borderless)`(unpackaged app 實際上靜默通過,OBS 就這樣做)(https://learn.microsoft.com/en-us/uwp/api/windows.graphics.capture.graphicscapturesession.isborderrequired、https://github.com/obsproject/obs-studio/discussions/9590)。24H2 起使用者也有系統級開關。
  - **24H2(build 26100)新料,對本 app 關鍵**:`MinUpdateInterval`(把 WGC 壓到例如 1 fps,不再每個 compositor frame 都來)與 `DirtyRegionMode`(frame 帶 dirty-region metadata——WGC 終於補上 DDA 的變化偵測)(https://learn.microsoft.com/en-us/uwp/api/windows.graphics.capture.graphicscapturesession.dirtyregionmode)。
  - **排除自家 overlay 視窗**:`SetWindowDisplayAffinity(hwnd, WDA_EXCLUDEFROMCAPTURE)`(Win10 2004+,DWM 層生效,WGC/DDA 都看不到)(https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-setwindowdisplayaffinity)。
- **DXGI Desktop Duplication(IDXGIOutputDuplication)**:Win8+,per-monitor。殺手級特性:`AcquireNextFrame(timeout)` **桌面沒變化就不回來(idle 零成本)**,且附 `GetFrameDirtyRects`/`GetFrameMoveRects`——免費 diff(https://learn.microsoft.com/en-us/windows/win32/direct3ddxgi/desktop-dup-api)。缺點:secure desktop(UAC)拿到 `E_ACCESSDENIED`、desktop switch 時 `DXGI_ERROR_ACCESS_LOST` 要重建、**每個 output 最多 4 個並行 duplication app**、hybrid-GPU 筆電是經典雷區、無 per-window。HDR 要 `DuplicateOutput1` 拿 FP16 自己 tone-map。
- **選型**:Win11 24H2+ 用 WGC(MinUpdateInterval + DirtyRegionMode + 無框);舊 Win10 用 DDA(無指示、有 dirty rects)。`windows-capture` crate 兩者都包。

### macOS
- **ScreenCaptureKit(SCK)**:macOS 12.3+(`SCStream`/`SCContentFilter`);macOS 14 加 `SCScreenshotManager`(單張)、`SCContentSharingPicker`(走系統 picker 可免 TCC 授權)與 Presenter Overlay;15 加 HDR。`CGDisplayStream`/`CGWindowListCreateImage`/`AVCaptureScreenInput` 自 macOS 14 deprecated,且 **Sequoia 15.1 起走舊 API 會觸發「可能繞過隱私設定」恐嚇彈窗**(xcap 因此遷移:https://github.com/nashaofu/xcap/issues/160)。
- **TCC UX**:`CGPreflightScreenCaptureAccess()` / `CGRequestScreenCaptureAccess()`(符號實際 macOS 11 才有);彈窗只出現一次,之後使用者要自己去 System Settings →「Screen & System Audio Recording」勾選並**重啟 app** 才生效。
- **兩個無法迴避的紅旗**:
  1. **紫色錄影指示**:**Sequoia 15.1 起**,任何 app 進行螢幕擷取,選單列/控制中心會亮紫色 screen-recording 圖示,**連續擷取 = 永遠亮著**;無任何 API/entitlement/picker 能關(https://developer.apple.com/forums/thread/769968、https://github.com/FelixKratz/SketchyBar/issues/641)。產品 UX 必須「正大光明」,把指示變成信任資產。
  2. **週期性重授權 nag**:15.0 每月彈「X 可以取用你的螢幕和音訊」(beta 曾是每週);15.1 起頻率再降(常用 app 很少被問,`replayd` 會刷新核准時間戳),但機制在 **macOS 26 Tahoe 仍在**(https://mjtsai.com/blog/2024/08/08/sequoia-screen-recording-prompts-and-the-persistent-content-capture-entitlement/、https://9to5mac.com/2024/10/07/macos-sequoia-screen-recording-popups/)。正規逃生門:向 Apple 申請 **`com.apple.developer.persistent-content-capture`** entitlement(遠端桌面類;24/7 螢幕記憶 app 是合理申請者,但別把產品賭在核准上)。

### Linux(延後,結論先記)
- **X11**:`XShmGetImage` + XDamage,無權限模型,簡單。
- **Wayland**:portal **`org.freedesktop.portal.ScreenCast`** + PipeWire(https://flatpak.github.io/xdg-desktop-portal/docs/doc-org.freedesktop.portal.ScreenCast.html)。`persist_mode`/`restore_token` 可免重選,但 **token 單次使用**(每次 session 要存新的)、**至少一次互動同意**、鎖屏時 Mutter 拒絕且可能燒掉 token、wlroots portal 只能抓整個 output(無 per-window)。KDE Plasma 6.3+ 有 `kde-authorized` 預授權、6.5 有權限設定頁;GNOME 支援 restore 但鎖屏限制照舊。新協定 `ext-image-copy-capture-v1`(2024-08 併入 staging,含 damage tracking)是未來的路。**結論:Wayland 的「無人值守背景連續擷取」是架構性痛點,Linux 首發支援 X11、Wayland 做「登入後點一次」+ restore_token 盡力而為。**

### Rust crates(2026-08-17 驗證)
| crate | 版本 | 判定 |
|---|---|---|
| `windows-capture` | 2.0.1(2026-08-08) | **Windows 首選**,WGC(+DDA),活躍 |
| `screencapturekit` | 8.0.1(2026-07-18) | **macOS 首選**(doom-fish/screencapturekit-rs,活躍) |
| `cidre` | 0.20.0(2026-08-10) | Apple 全家桶 binding(SCK + Vision OCR),極活躍;Screenpipe 生產環境用它做 OCR |
| `xcap` | 0.9.8(2026-08-01) | 跨平台截圖(含 Wayland portal),活躍;但 frame model 陽春、持續錄影路徑會 CPU 轉換每個 frame(Screenpipe 因此繞開) |
| `ashpd` | 0.13.13(2026-07-17) | xdg-desktop-portal client,Linux 正路 |
| `image_hasher` | 3.1.1(2026-02-21) | dHash/pHash;別用凍結的 `img_hash` |
| `scap` | 0.1.0-beta.1(2025-08;stable 0.0.8) | Cap 團隊,**停滯約一年**,觀望 |
| `crabgrab` / `screenshots` / `captrs` / `scrap`(crates.io 版) | — | 已 archive / 棄置 / 死亡,避開 |

Screenpipe 實際堆疊(讀 source 驗證):macOS 用 pinned fork 的 SCK bindings + xcap fallback,Windows 用 xcap `wgc` feature + 直接 `windows` crate,diff 用 `image-compare` crate(Hellinger histogram + MSSIM)。

### Frame-diff / dedup 與節奏
- OS 免費層:DDA dirty rects / WGC `DirtyRegionMode`(24H2)/ SCK 本來就只在畫面更新時給 frame(`minimumFrameInterval` 限流)/ Mutter PipeWire damage-driven。
- 通用層:縮到 9×8 灰階 **dHash(64-bit)**,連續相同畫面距離 0–2,**門檻 ~4–10 可吸收游標閃爍與時鐘**(https://benhoyt.com/writings/duplicate-image-detection/);更便宜:縮圖 byte-diff 變化比例。pHash 最穩但 3–5× 成本。
- 實戰參數參考(Screenpipe 2026 event-driven engine,讀 source):`min_capture_interval_ms: 200`、`visual_check_interval_ms: 3000`、`visual_change_threshold: 0.05`(~5% 差異觸發)、`idle_capture_interval_ms: 30000`(心跳快照)、JPEG q80、點擊觸發。
- 同類節奏:Rewind 每 2 秒一張(0.5 fps,批次進 H.264)(teardown:https://kevinchen.co/blog/rewind-ai-app-teardown/);Recall 是 change 觸發、最小 ~5s;**建議:事件/damage 觸發 + ≥0.5–1s 間距 clamp + 30–60s idle 心跳 + dHash 門檻 6–10/64 當入庫閘門。**

---

## 2. 前景脈絡擷取(active window / URL / AX 文字)

### Active window(標題 / app / PID)
- **Windows**:事件驅動 `SetWinEventHook(EVENT_SYSTEM_FOREGROUND, WINEVENT_OUTOFCONTEXT)` → `GetForegroundWindow` → `GetWindowTextW` / `GetWindowThreadProcessId` → `QueryFullProcessImageNameW`。免權限。注意 Raymond Chen 提的 async 陷阱:callback 執行時視窗可能已銷毀(https://devblogs.microsoft.com/oldnewthing/20131202-00/?p=2503)。
- **macOS**:app 名/bundle-id/PID 用 `NSWorkspace.frontmostApplication` **免權限**;**視窗標題要權限**——`CGWindowListCopyWindowInfo` 的 `kCGWindowName` 被 Screen Recording gate(未授權就靜默缺欄位,https://developer.apple.com/forums/thread/126860),或走 AX(`kAXFocusedWindowAttribute`→`kAXTitleAttribute`)吃 Accessibility 權限。本 app 反正必拿 Screen Recording,標題順便到手。
- **Linux**:X11 `_NET_ACTIVE_WINDOW`+EWMH 簡單;**Wayland 無標準且碎裂**(wayland.app 逐 compositor 驗證):wlroots 系(Sway/Hyprland/niri)有 `wlr-foreign-toplevel-management`(含 activated 狀態);**GNOME/Mutter 兩個 toplevel 協定都不實作**(要裝 GNOME Shell extension「Focused Window D-Bus」,ActivityWatch 與 x-win 都這樣搞);KWin 走 KWin scripting。`ext-foreign-toplevel-list` 刻意不給 focus 狀態,單獨不夠用。
- Crates:`active-win-pos-rs` 0.11.0(2026-05-26)、`x-win` 5.8.0(2026-08-11,功能較多、還會回 browser URL 與 icon),皆活躍。

### 瀏覽器 URL(三條路,可靠度遞增)
1. **Windows UIA**:讀 omnibox `Edit` element 的 `ValuePattern.Value`。可行但版本脆弱、時靈時不靈(MS 自家 Q&A 都承認)。Screenpipe 的作法值得抄:`ElementFromHandleBuildCache` + `TreeScope_Subtree` 一次跨程序抓整棵、**Chromium/Electron 不填 cached subtree 時 fallback 到 TreeWalker + 250ms 死線**(https://github.com/screenpipe/screenpipe 的 windows_uia.rs)。
2. **macOS AppleScript/Apple Events**:`tell app "Google Chrome" to get URL of active tab`(Safari/Chrome/Brave/Arc 可;**Firefox 不可 script,拿不到 URL**);需 `NSAppleEventsUsageDescription` + 每個 (app, browser) 對一次 Automation 同意彈窗。ActivityWatch 的 printAppStatus.jxa 是參考實作。
3. **Browser extension + Native Messaging(最可靠)**:WebExtension `tabs` API 拿精確 URL/title/incognito,經 native messaging host 或 localhost 丟給桌面端——ActivityWatch `aw-watcher-web` 模式(https://github.com/ActivityWatch/aw-watcher-web)。代價:每瀏覽器安裝、MV3 service worker 生命週期、商店審查。**建議:extension 為主、UIA/AppleScript 為免安裝 fallback**(Screenpipe 兩條都做)。

### UIA / AX 文字讀取(OCR 的「加值層」而非替代)
- Windows UIA `TextPattern` / macOS `AXUIElement` 能拿結構化文字,**Screenpipe 現行架構就是 accessibility-tree 優先、OCR 當 fallback**,並宣稱因此「資源省 100 倍」(https://screenpi.pe/blog/screenpipe-v2-03-accessibility-capture)。
- 但成本與破口要誠實:UIA 是跨程序 COM,逐 property 逐次 round-trip,必須用 **CacheRequest 批次**(https://learn.microsoft.com/en-us/windows/win32/winauto/uiauto-cachingforclients);**Chromium/Electron 的 a11y 預設關閉**,偵測到 AT client 才啟動,而啟動後 renderer 要計算/序列化整棵 a11y 樹——Google 實測 scroll 序列化 20+ 次/秒、修完才改善 825%,只有 ~5–10% 使用者開著 a11y(https://developer.chrome.com/blog/chromium-accessibility-performance)——**你的工具開了它,等於對每個網頁課永久性能稅**;自繪 UI(遊戲、無 bridge 的 Qt、Flutter desktop、Java)整棵樹是空的;Electron on macOS 要戳 `AXManualAccessibility`(https://www.electronjs.org/docs/latest/tutorial/accessibility/)。
- **Microsoft Recall 自己選了截圖+OCR(+Click to Do/Phi-Silica),不是 UIA**——OCR 對所有 app 一視同仁、還能丟 NPU。結論:**OCR 是 universal floor,UIA/AX 只對配合的原生 app 當高保真 overlay。**
- **密碼欄位偵測(隱私底線,必做)**:UIA `AutomationElement.IsPasswordProperty == true` → 跳過(https://learn.microsoft.com/en-us/dotnet/api/system.windows.automation.automationelement.ispasswordproperty);macOS 焦點元素 role `AXSecureTextField` 同理;加私密瀏覽偵測(aw-watcher-web 能回 incognito flag)與 app 黑名單。Recall 預設也做 private-browsing 與敏感資訊過濾。

### Clipboard 監聽
- **Windows**:`AddClipboardFormatListener` + `WM_CLIPBOARDUPDATE`,push,免權限。
- **macOS**:無 push,輪詢 `NSPasteboard.general.changeCount`(0.2–1s,Maccy 模式)。**隱私走向注意**:macOS 15.4 起有 developer-preview 的 pasteboard privacy(讀取非使用者貼上動作觸發「X accessed clipboard」警示),**至 macOS 26 Tahoe 仍是 opt-in 未預設開啟**;新 API `NSPasteboard.detectPatterns`(只看型態不讀內容)與 `accessBehavior` 是合規路徑(https://mjtsai.com/blog/2025/05/12/pasteboard-privacy-preview-in-macos-15-4/)。設計上先用 detectPatterns、真正讀取要節制。
- **Linux**:X11 XFixes;Wayland `ext-data-control-v1`(KWin/wlroots 系有;**GNOME 兩代 data-control 都不支援**——GNOME Wayland 上無 headless 剪貼簿監聽)。
- Crates:`clipboard-rs` 0.3.5(有 `ClipboardWatcher`,三平台);`arboard` 3.6.1(1Password 維護,只有讀寫無監聽);Wayland 用 `wl-clipboard-rs`。
- 禮貌規範:尊重 `org.nspasteboard.ConcealedType`(密碼管理器標記,http://nspasteboard.org)一律不記錄。

---

## 3. 輸入動態訊號(無內容 keylogging)

原則(照抄 ActivityWatch 的 privacy design):只存 `{"presses": n}`、滑鼠 clicks/deltaX/deltaY/scrollX/scrollY 聚合,**callback 內立即丟棄 keycode,永不落地**(https://github.com/ActivityWatch/aw-watcher-afk)。

- **Idle 偵測——每個 OS 都有免權限解**:
  - Windows:`GetLastInputInfo`(免權限;僅限本 session,對一般桌面 app 剛好)。
  - macOS:`CGEventSourceSecondsSinceLastEventType(.hidSystemState, kCGAnyInputEventType)`——**免任何 TCC**,aw-watcher-afk macOS 後端就是它。這是 macOS 上唯一不用權限的輸入相關 API。
  - Linux:X11 `XScreenSaverQueryInfo`;Wayland `ext-idle-notify-v1`(KWin/Sway/Hyprland/niri/COSMIC 有;**GNOME 沒有**,走 `org.gnome.Mutter.IdleMonitor` D-Bus)(https://wayland.app/protocols/ext-idle-notify-v1)。
  - 參考參數:aw-watcher-afk 預設 AFK 門檻 **180s**、輪詢數秒一次。
- **活動強度(要細粒度才需權限)**:
  - Windows:`WH_KEYBOARD_LL`/`WH_MOUSE_LL`(免特殊權限;注意 `LowLevelHooksTimeout` **預設/上限 1000ms,超時 hook 被靜默移除**——callback 只做 atomic increment)或 Raw Input。
  - macOS:`CGEventTap` 即使 listen-only,**看鍵盤事件仍需 Input Monitoring**;`NSEvent.addGlobalMonitorForEvents` 鍵盤事件需 Accessibility(沒權限就是靜默收不到)。若想零權限,用 idle-seconds 按事件型別差分,可得粗粒度「鍵盤/滑鼠有無活動」。
  - Wayland:全域輸入監聽等於 evdev(input group),放棄,用 idle + 視窗切換頻率替代。
- **視窗切換頻率**:直接來自 §2 的 foreground 事件流,免費。
- Crates 現實:`rdev` 0.5.3 上游停滯(要用就用 rustdesk-org fork)、`device_query` 4.0.1(輪詢、X11 only)、`user-idle` 0.6.0 停滯且無 Wayland。**建議 per-OS 薄 FFI 自寫(每平台百行內),Wayland 參考 Rust 版 `awatcher`**(https://github.com/2e3s/awatcher)。

---

## 4. 本地 OCR(繁中+英文混排是硬需求)

### macOS:Apple Vision(該平台無懸念的第一名)
- API:`VNRecognizeTextRequest`(10.15+)/ Swift 新版 `RecognizeTextRequest`(macOS 15+);macOS 26 Tahoe 另有 `RecognizeDocumentsRequest`(段落/表格結構,但 CJK 只有 line-level box)。
- **zh-Hant 一等公民**:支援清單含 `zh-Hant` 與 `yue-Hant`(18 語言,iOS18/macOS15 世代)。三個實務眉角:**CJK 只在 `.accurate` 模式**(`.fast` 僅拉丁);中文要放 `recognitionLanguages` **第一位**、搭 en-US;螢幕 UI 文字建議 `usesLanguageCorrection = false`(Screenpipe 就關掉,避免語言模型「糾正」程式碼/UI 字串);不支援直排(螢幕場景無妨)。
- **速度(實測數字)**:Screenpipe source 註解:3456×2234 Retina 全幅 `.accurate` **~400–1400ms**;其 uniOCR 測 M4 Max **~310ms/幀、90% 準確**(https://github.com/screenpipe/uniOCR)。所以他們做了 OCR gate:先用 ~10–19ms 的傳統文字區域偵測 + pixel hash,**只 OCR 變化的 crop**。照抄這招。
- 從 Rust 呼叫:`cidre` 0.20.0(Screenpipe 生產路徑;記得包 `ar_pool` 否則 Vision 會漏 CoreFoundation/MLModel 物件——他們踩過 memory leak)或 `objc2-vision` 0.3.2,或小 Swift sidecar。

### Windows:誠實的三層現實
1. **`Windows.Media.Ocr`(WinRT)**:快(1080p in-process ~100–250ms;Windrecorder 實測含 spawn 0.262s、**中文準確率 ~90.2%**,https://github.com/yuka-friends/Windrecorder/blob/main/__assets__/third_party_ocr_engine_benchmark_reference.md),但:2015 世代模型、**無 confidence**、**依賴系統語言包**(使用者沒裝繁中包就沒繁中)、**CJK 間距 bug 是招牌傷**(每個漢字回傳成獨立 "word",naive join 變「螢 幕 記 憶」,要 script-aware 重組,https://github.com/TheJoeFin/Text-Grab/issues/191)、`MaxImageDimension` 歷史上 2600px——**4K 畫面超標要縮圖/切塊**。
2. **OneOCR(Snipping Tool 的 oneocr.dll + onemodel)**:現代多語言單模型(34 個 ONNX 打包)、自動偵測語言、CJK 品質遠勝 legacy、CPU 可跑(內帶 onnxruntime)、有 Rust binding `oneocr-rs` 0.3.2(https://github.com/wangfu91/oneocr-rs)。**致命傷:模型是 Microsoft 專有檔案,不可再散布**,只能 runtime 從使用者機器的 Snipping Tool 撈(脆弱、ToS 灰色)。定位:opt-in 加速器。
3. **Windows AI `TextRecognizer`(`Microsoft.Windows.AI.Imaging`,Windows App SDK 1.7.2+ stable)**:`GetReadyState()`→`EnsureReadyAsync()`→`RecognizeTextFromImage()`,含 word-level confidence、自動多語言。**至 2026-08 仍限 Copilot+ NPU 機**,非支援硬體直接 throw;MS 對 RTX GPU 開放的實驗只涵蓋 LLM/Phi-Silica 類、**尚未包含 imaging OCR**(https://learn.microsoft.com/en-us/windows/ai/cards/text-recognition-ocr-platform-card、https://www.pcworld.com/article/3163780/microsoft-chips-away-at-copilot-by-adding-ai-support-to-gpus.html)。定位:能力偵測到就用的機會型升級。

### 跨平台底座:PaddleOCR 家族(繁中的關鍵解)
- **PP-OCRv5(2025)**:單一 rec 模型同時支援**簡中+繁中+拼音+英文+日文**(v4 以前的 "ch" 模型只有簡中+英,繁中要另掛 chinese_cht 模型——這是 v5 的質變)(https://www.paddleocr.ai/main/en/version3.x/algorithm/PP-OCRv5/PP-OCRv5.html)。模型小:mobile det 4.7MB + rec 16.5MB。
- **PP-OCRv6(2026-06)**:tiny/small/medium 三檔(1.5M–34.5M 參數);medium 比 v5_server 高 +5.1 rec 分;**tiny 比 v5_mobile 快 3.9×**;Xeon CPU 上 small 端到端 **~0.59s/圖(OpenVINO)**(https://arxiv.org/abs/2606.13108、https://github.com/Sekinal/ppocrv6-fast-cpu)。桌面 4–8 核 CPU 估 **~250–600ms/1080p 幀(det+rec, mobile/small)**——0.2–1 fps 連續跑可行。
- 部署:**RapidOCR** v3.9.2(2026-07-21,v3.9 已收 v6;注意其預設仍是 v4,要顯式指定 `ocr_version`)(https://github.com/RapidAI/RapidOCR);**Rust 原生:`oar-ocr` 0.9.1**(2026-08-07,PP-OCRv4/v5/**v6** 全 pipeline on `ort`,含繁中/日/韓 checkpoints,CPU+CUDA,Apache-2.0,https://github.com/GreatV/oar-ocr)——「全 Rust 單 binary」的關鍵拼圖;`ort` 本身 2.0.0-rc.13(production-ready 但 API 未 stable,pin 死版本)。
- **其他選項快篩**:Tesseract 5.5.3——螢幕字+繁英混排差(漢字間 spurious spaces 老 bug)、1.5–2s/幀,只當急救;Surya 2——品質好但 **OpenRAIL-M 授權(營收>$5M 要商業授權)**+ CPU 秒級,pass;EasyOCR 停更(2024-09 後無 release)、docTR 無 CJK rec,pass;Qwen2.5-VL 類 VLM——zh-Hant 理解一流但秒級/幀,**只做「重要畫面事後重讀」的第二車道**,不進連續迴圈。

### 各平台建議(含 Screenpipe 對照)
- **macOS**:Vision `["zh-Hant","en-US"]` accurate + correction off + OCR gate(= Screenpipe 現行,他們近期還修成「啟用中文自動附加 zh-Hant」)。
- **Windows**:**預設 bundle PP-OCRv5/v6(oar-ocr/ONNX)**——因為 `Windows.Media.Ocr` 的繁中依賴使用者裝語言包+要 hack 間距;runtime 偵測到 OneOCR / Copilot+ TextRecognizer 再升級。(Screenpipe 目前仍用 Windows.Media.Ocr,confidence 硬編 1.0,是它的弱點而非榜樣。)
- **Linux**:同一套 PP-OCRv5/v6 ONNX(Intel CPU 上 OpenVINO EP);Tesseract 僅急救。
- 搜尋前正規化:全形/半形統一 + OpenCC 繁簡歸一(提升召回)。

---

## 5. 本地儲存與搜尋(SQLite 單檔主義)

(SQLite 3.53.4(2026-07-24)、`rusqlite` 0.40.2、`sqlite-vec` 0.1.9、`fastembed` 6.0.0、`jieba-rs` 0.10.3、`keyring` 4.1.6,皆已驗證。)

### FTS5 的 CJK 分詞
- 預設 `unicode61` 對中文無效(整串漢字一個 token;Signal 踩過:https://github.com/signalapp/Signal-iOS/issues/6169)。
- **(a) `trigram`(內建,SQLite ≥3.34;trigram 的 `remove_diacritics` 3.45+)**:substring 匹配,合 CJK 直覺、免字典、繁中天然支援、加速 indexed `LIKE`(https://www.sqlite.org/fts5.html)。代價:**<3 字查詢查不到**(中文 1–2 字詞極常見)、索引 ~2–4× 原文。混合策略:https://zenn.dev/kanseilink/articles/kanseilink-fts5-trigram-cjk-20260507?locale=en;<3 字補洞:https://github.com/streetwriters/sqlite-better-trigram。
- **(b) ICU 只在 FTS3/4,FTS5 無內建 ICU**(第三方 https://github.com/cwt/fts5-icu-tokenizer 要拖 ICU runtime)。
- **(c) `simple`(wangfenjin)v0.7.1(2026-02-23)**:字級索引+拼音、可選 cppjieba `jieba_query()`,活躍、預編譯全平台(https://github.com/wangfenjin/simple)。字級對繁中 OK,jieba 字典偏簡中。signal-fts5 是 AGPL,pass。
- **(d) 先斷詞再入庫**:`jieba-rs` 0.10.3;繁中要 `dict.txt.big` 或 OpenCC。缺點:snippet/highlight offset 對不回原文、OCR 噪音讓字典斷詞劣化。
- **推薦**:**external-content table + `trigram` 主索引 + `unicode61` 副索引**(cover 英文整詞與短查詢);之後要拼音再上 `simple`。OCR 文字有噪音,substring 匹配比字典斷詞寬容得多。

### 向量搜尋
- **`sqlite-vec` v0.1.9(2026-03-31)**:2025 年曾停滯、**2026 復活**(Mozilla Builders + Fly.io/Turso/SQLite Cloud 贊助);v0.1.6 起有 **metadata 欄位過濾 + partition key**;目前 stable 仍是 **brute-force KNN**,ANN(rescore/IVF/DiskANN)在 v0.1.10-alpha(https://github.com/asg017/sqlite-vec)。`sqlite-vss` 已棄置;DuckDB VSS 的 HNSW 持久化仍 experimental(crash 可能壞索引),不適合持續寫入的 recorder;LanceDB 已 1.0(真 ANN)但是第二套儲存引擎。
- **規模誠實估**:一年去重後 ~0.5–2M chunks;**256-d int8(MRL 截斷)** 1M 向量 ≈ 256MB,brute-force + 以日/週 partition + app/時間 metadata 過濾(螢幕記憶查詢幾乎都帶時間/app 範圍)完全在互動延遲內。**單一 SQLite 檔同時裝 FTS+向量+metadata = 備份/加密一條龍**。>2–5M 向量再掛 `usearch` 2.26.0(mmap HNSW,SQLite 為 source of truth 可重建)。
- 風險:pre-1.0 格式會變 → **存 model-id + dim,設計成可背景 re-embed**。

### 本地 embedding(CPU、多語含 zh-Hant)
- **預設 `EmbeddingGemma-300m`**(2025-09,308M,768-d 可 MRL→256/128,量化後 <200MB RAM,100+ 語言)(https://developers.googleblog.com/en/introducing-embeddinggemma/);**品質檔 `Qwen3-Embedding-0.6B`**(Apache-2.0,MRL 32–1024d,32K ctx,MTEB-ML 64.33,官方 GGUF)(https://huggingface.co/Qwen/Qwen3-Embedding-0.6B);`bge-m3`(568M,dense+sparse)是重砲但 CPU 成本最高;弱機 `multilingual-e5-small`。
- Runtime:**`fastembed` 6.0.0(2026-08-16)已收 bge-m3 / e5 / EmbeddingGemma / Qwen3 系**(https://crates.io/crates/fastembed),底層 `ort` 2.0.0-rc.13;或 llama.cpp + GGUF 零 Python 路線。

### DB 成長率(算給你看)
- 文字:1080p 滿版 ≈2–5KB;8h/天、5–10s 級取樣、去重後 unique 10–30% → **~1–8MB/天**,+trigram 2–4× → **文字層一年 1–5GB,可永久保留**。
- 向量:256-d int8 每 chunk 256B → **每月 <2MB**,rounding error。
- 截圖:1080p 文字畫面 WebP q60 ≈30–120KB;30s 取樣+去重 → **~1–3.5GB/月**(唯一需要 retention 旋鈕的層:全解析度 30 天、縮圖 1 年、文字永久)。
- 對照(皆有出處):Screenpipe README ~20GB/月、docs 1fps ~30GB/月;**Pensieve(5s 截圖+靜態去重)實測 ~400MB/天 ≈ 8GB/月 + 每 10 萬張截圖 ~2.2GB 索引**(https://github.com/arkohut/pensieve);Rewind teardown 實測 **~210MB/h ≈ 每 8h 工作日 1.7GB**(https://kevinchen.co/blog/rewind-ai-app-teardown/)。**我們不存連續影音,量級直接少一個 0。**

### 加密 at rest
- **SQLCipher 4.13.0(2026-01-20)**,官方 guidance 開銷 **5–15%**(https://www.zetetic.net/sqlcipher/performance/);Rust 走 `rusqlite` 的 `bundled-sqlcipher(-vendored-openssl)`(注意 vendored 版可能落後)。注意 FTS/vec 掃描也吃 page 解密稅,要帶真實 page cache 壓測。
- 金鑰:隨機 256-bit 存 OS keychain(`keyring` 4.1.6:Keychain / Credential Manager / Secret Service)。
- 威脅模型要誠實:**Recall 的門檻是 VBS enclave + TPM 綁定 + Windows Hello ESS just-in-time 解密**(https://blogs.windows.com/windowsexperience/2024/09/27/update-on-recall-security-and-privacy-architecture/),userland OSS 做不到;OSS 務實底線 = SQLCipher 全檔 + keychain 金鑰 + 可選生物辨識解鎖 + 積極排除清單(密碼欄位/私密視窗/app 黑名單),README 明講「同使用者權限的 malware 不在防護範圍」。

---

## 6. Tauri v2 外殼

- 版本現況:**tauri 2.11.5**(2026-07-01)、tauri-cli 2.11.3、wry 0.56.1(2026-08-13)、tao 0.36.0;月更節奏、無 v3 風聲,生態成熟。**Screenpipe 桌面端就是 Tauri、Cap(18k stars 錄影 app)也是**——重 Rust 側 capture + Tauri UI 這條路有大型先例。
- **Overlay(always-on-top 透明點穿)**:config 全齊(`alwaysOnTop`/`transparent`/`decorations:false`/`shadow:false`/`skipTaskbar`/`visibleOnAllWorkspaces`)+ runtime `set_ignore_cursor_events(true)`。平台眉角:
  - **Windows**:實作為 `WS_EX_TRANSPARENT|WS_EX_LAYERED`;無 Electron 式 `forward:true`(點穿時拿不到滑鼠座標,https://github.com/tauri-apps/tauri/issues/6164 仍開著);**無 per-pixel 點穿**——實務模式是「整窗點穿 + global shortcut 切換互動模式」(Overlayed 的做法)。
  - **macOS**:`ignoresMouseEvents` 可靠;**透明 webview 背景仍需 `macos-private-api` feature**(flip WKWebView 私有 `drawsBackground`);已知坑:透明窗強制整窗連續重合成(**回報 GPU 功耗 ~8×**,https://github.com/tauri-apps/tauri/issues/15471)、**DMG 打包後透明失效 bug**(https://github.com/tauri-apps/tauri/issues/13415);要蓋別人 fullscreen Space 需 `ActivationPolicy::Accessory` + 原生 `NSWindowCollectionBehavior`(CanJoinAllSpaces|FullScreenAuxiliary)——社群標準解是 **`tauri-nspanel`**(把 Tauri 窗轉非激活 NSPanel,https://github.com/ahkohd/tauri-nspanel + spotlight example)。
  - **Linux**:X11 可用;**Wayland 無 layer-shell 支援,overlay 視同不支援**(https://github.com/tauri-apps/tao/issues/925)。
  - overlay 設計原則:讓 capture 端用 `WDA_EXCLUDEFROMCAPTURE`(Win)/ SCK content filter 排除自家 overlay,避免記憶體裡全是自己的 HUD。
- **Plugins(版本已驗證)**:global-shortcut 2.3.2、autostart 2.5.1、notification 2.3.3、clipboard-manager 2.3.2、single-instance 2.4.3、positioner 2.3.3、shell 2.3.5、updater 2.10.1、deep-link 2.4.9;**tray 是 core feature**(`tray-icon` cargo feature,tray-icon crate 0.24.2)。
- **架構:capture 放哪?(TCC 決定)**:macOS 的 TCC 歸屬有實坑——**裸 helper 執行檔在 Tahoe 26.1 甚至不出現在 Screen Recording 設定面板**、daemon/LaunchAgent 不繼承 app 的授權(https://developer.apple.com/forums/thread/807898)。**建議:capture 跑在 Tauri 主程序(Rust core 反正同語言),sidecar(`bundle.externalBin` + shell plugin,https://v2.tauri.app/develop/sidecar/)留給 OCR/索引 worker**——要 crash isolation 又不碰 TCC。IPC:stdio JSON-lines 或 localhost REST(Screenpipe 模式:core 在 localhost:3030,UI 是殼)。另注意 Screenpipe docs 警告:**macOS 大版本更新常重置 TCC,靜默掉權限**——要做開機自檢。
- **macOS 權限彈窗**:**`NSScreenCaptureUsageDescription`/`NSAccessibilityUsageDescription` 這兩個 plist key 根本不存在**(社群 cargo-cult;Apple 文件驗證 404)——Screen Recording 由系統在首次擷取時自動彈、授權後要重啟 app;Accessibility 用 `AXIsProcessTrustedWithOptions(kAXTrustedCheckOptionPrompt)` 觸發。只有麥克風/相機才要 usage string。輔助 crate:`tauri-plugin-macos-permissions` 2.3.0(check/request Screen Recording、Accessibility、Input Monitoring、FDA,https://github.com/ayangweb/tauri-plugin-macos-permissions);開發期 `tccutil reset ScreenCapture <bundle-id>`。
- **簽名/公證/散布**:
  - macOS:**沒有 screen-capture entitlement 這種東西**——純 TCC;要的是 Developer ID + hardened runtime(Tauri 預設開)+ notarization(Tauri 內建流程,https://v2.tauri.app/distribute/sign/macos/)。**Sequoia 已移除 Ctrl-click 繞過 Gatekeeper**——未簽名散布對「讀你螢幕」的 app 是死路。
  - Windows:indie 正路是 **Azure Trusted Signing($9.99/月,2026-04 起開放美加個人)**(https://melatonin.dev/blog/code-signing-on-windows-with-azure-trusted-signing/);SmartScreen 信譽仍要累積。未簽名 = SmartScreen 攔截 + AV 對截圖行為的誤報地獄。
  - **Mac App Store:實質不可行**——`macos-private-api`(透明 overlay 要用)直接不合規、updater plugin 被禁、persistent-content-capture 是特批;Rewind/Screenpipe/Dayflow/rem 全部 MAS 外發行。

---

## 7. 資源預算現實

### 同類 app 數據(皆有出處)
| 產品 | CPU | RAM | 磁碟 | 狀態/備註 |
|---|---|---|---|---|
| **Screenpipe**(Tauri,YC S26,21k stars) | 自稱 5–10%;自家測試門檻僅「<30%」 | 自稱 0.5–3GB;實測有 1.5–2.7GB/h 洩漏到 OOM 的案例(#2571)、8GB/30min 賞金 bug(#278) | README ~20GB/月;1fps 模式 ~30GB/月 | **2026-06-09 從 MIT 改為 source-available 商業授權**,簽名版 $25/月(https://screenpipe.com/blog/screenpipe-license-update)——「真開源」位置空出 |
| **Rewind**(macOS,closed) | teardown 實測主程序 ~20% 單核,encode 尖峰 >200%;使用者回報**電池 -20–40%** | 中等 | 實測 ~210MB/h(~1.7GB/8h 日);行銷稱 14GB/月 | **已死:Meta 2025-12 收購 Limitless,Mac app 2025-12-19 停止擷取**(https://9to5mac.com/2025/12/05/rewind-limitless-meta-acquisition/)。teardown:https://kevinchen.co/blog/rewind-ai-app-teardown/(0.5fps + 軟體 libx264 是 CPU 兇手;**曾實作「電池模式延後 encode」**) |
| **Windows Recall** | 近 0(NPU offload;仍有 idle 喚醒耗電傳聞) | — | 預設 25GB(256GB 碟)~150GB;~10GB ≈ 35h 歷史;需 50GB 剩餘空間 | 限 Copilot+(40 TOPS NPU + 16GB RAM);opt-in;Hello 綁定加密 |
| **Pensieve**(OSS Python) | 低 | 低 | **~400MB/天(1440p、10h、5s 取樣+去重)** | OSS 圈最好的磁碟數據點(https://github.com/arkohut/pensieve) |
| **Dayflow**(OSS Swift,macOS) | **<1% @ 1fps** | **~100MB** | 低(VLM 摘要後丟原始) | capture 層效率的證明點(https://github.com/JerryZLiu/Dayflow) |
| **rem / xrem**(OSS) | — | — | — | 停滯(2024-05 後無 push) |

### 我們的工程目標(text-first、不錄影、預設不錄音——合理可達)
- **CPU:平均 <3% 單核**(Dayflow 證明 capture 層 <1% 可行;Rewind 的 20% 來自軟體 libx264+PNG 暫存,我們不錄影直接繞過;Screenpipe 的 5–10% 來自 always-on OCR+Whisper)。OCR 以「變化 crop only + 0.1–0.2 有效 fps」攤提。
- **RAM:常駐 <300MB**(Dayflow ~100MB 是地板);embedding/LLM 模型獨立進程、lazy-load、可殺(Screenpipe 0.5–4GB 的教訓:模型與洩漏都別放主進程)。
- **磁碟:文字+索引+向量 ~1–5GB/年;截圖旋鈕 0.03–0.12GB/天** → 遠低於 2GB/天上限,實際 <0.2GB/天。
- **電池(M 系列)**:SCK capture 本身便宜,**耗電大戶是 encode/OCR/STT**。業界既定模式(Rewind 首創、Screenpipe 文件明示):**電池上只做便宜 capture,OCR/embedding/索引延後到插電或 idle+插電;絕不在電池上跑 Whisper 類**。這塊做好就是對 Screenpipe 的直接差異化。

---

## 8. 建議技術棧(總結)

**Rust core(在 Tauri 主程序,TCC 乾淨)**:`windows-capture` 2.0(WGC/DDA + 24H2 DirtyRegionMode)/ `screencapturekit` 8.0 或 `cidre` 0.20(SCK)+ `SetWinEventHook`/`NSWorkspace` 前景事件 + dHash(`image_hasher` 3.1,門檻 6–10/64)+ Screenpipe 式 event-driven 節奏(200ms clamp / 5% 視覺變化 / 30s 心跳)。
**OCR**:macOS Vision(`zh-Hant` 優先、correction off、只 OCR 變化 crop);Windows/Linux bundle `oar-ocr` 0.9(PP-OCRv5/v6);OneOCR/TextRecognizer 做機會型升級;Qwen2.5-VL 留作事後重讀車道。
**儲存(sidecar worker)**:SQLite 3.53 單檔(`rusqlite` 0.40 + SQLCipher 4.13 + `keyring` 4.1)+ FTS5 trigram+unicode61 雙索引 + `sqlite-vec` 0.1.9(256-d int8、以日 partition)+ `fastembed` 6.0(EmbeddingGemma;Qwen3-Embedding-0.6B 品質檔)。
**UI**:Tauri 2.11 + tray(core)+ global-shortcut/autostart/notification plugins;overlay 用 always-on-top 透明窗 + 整窗點穿切換 + `tauri-nspanel`(macOS)+ `WDA_EXCLUDEFROMCAPTURE` 自我排除;Wayland overlay 不做。
**散布**:Developer ID + notarization(macOS)、Azure Trusted Signing(Windows);MAS 不考慮。
**平台順序**:Windows + macOS 首發 → Linux X11 → Wayland 明示降級(portal + restore_token 盡力而為)。
