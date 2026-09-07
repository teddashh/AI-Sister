//! 瀏覽器網址與密碼欄：UI Automation。
//!
//! 這個模組補上 `sister doctor` 一直在喊的那個洞：**沒有它，
//! `excluded_urls` 整組規則一條都不會生效**，網銀與登入頁只能靠視窗標題擋。
//!
//! ## 為什麼是一條「可以被丟掉」的執行緒
//!
//! UIA 的呼叫**沒有辦法取消**。對面那個 app（也就是瀏覽器）如果卡住，
//! 我們這邊就跟著卡住，卡多久由對方決定，而我們連中止的手段都沒有。
//! `IUIAutomation2::SetTransactionTimeout` 只是部分緩解——NVDA 的
//! issue #6533 從 2016 年開到現在都沒關，他們最後採用的解法是：把 UIA
//! 關在一條專屬執行緒裡，卡住就**整條放棄**，不等它、不 join 它。
//!
//! 這裡照做。放棄掉的執行緒會漏著（它還卡在那個回不來的呼叫裡），所以
//! 放棄的次數有上限：連續放棄 [`MAX_ABANDONS`] 次之後就宣告這台機器上
//! 讀不到網址，不再嘗試——一個誠實的「做不到」，好過無限期地漏執行緒。
//!
//! ## 為什麼不用 `FindFirst(TreeScope_Descendants)`
//!
//! 那一行看起來最短，而且到處都查得到有人這樣寫。它的代價是：UIA 會為了
//! 滿足這次查詢去**具現化整棵子樹**，包含 Chromium 的 renderer——等於逼
//! 瀏覽器為了我們讀一次網址，把整個網頁的 accessibility tree 建出來。
//! 那是使用者感覺得到的頓，而且是我們每次 tick 都要求他付一次。
//!
//! 所以這裡自己走 `ControlViewWalker`，而且**主動剪掉**網頁內容那一支
//! （見 [`is_web_content`]）。位址列是瀏覽器自己的 UI（Chromium 的 Views），
//! 不在網頁那一支裡，剪掉它完全不影響我們要的東西。
//!
//! ## 讀回來的網址是**縮寫過的**
//!
//! Chromium 位址列顯示的是給人看的字串，不是真正的 URL：
//! `kFormatUrlOmitHTTPS | kFormatUrlOmitTrivialSubdomains` 會把
//! `https://www.example.com/a?b=c` 顯示成 `example.com/a?b=c`。
//! 路徑與查詢字串留著，scheme 和 `www.` 沒了。
//!
//! 這對排除規則不是問題——規則本來就是子字串比對（見 THREAT_MODEL
//! 「安靜地不生效」第 1、2 條）——但**寫規則的人必須知道**，否則他會寫
//! `https://*bank*` 然後永遠不命中。`config::suspicious_url_rules`
//! 就是在抓這種規則。

use std::sync::mpsc::{Receiver, SyncSender, sync_channel};
use std::time::Duration;

use sister_core::model::BrowserUrlState;
use windows::Win32::Foundation::HWND;
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoUninitialize,
};
use windows::Win32::UI::Accessibility::{
    CUIAutomation8, IUIAutomation, IUIAutomation2, IUIAutomationElement, IUIAutomationTreeWalker,
    IUIAutomationValuePattern, UIA_DocumentControlTypeId, UIA_EditControlTypeId,
    UIA_ValuePatternId,
};
use windows::core::Interface;

/// 等一次 UIA 回答的預算。
///
/// 超過就放棄那條執行緒。訂得比 `TransactionTimeout` 大一點點，讓 UIA
/// 自己的逾時有機會先生效——它至少會把執行緒還給我們。
const ASK_BUDGET: Duration = Duration::from_millis(900);

/// 連上目標程序的逾時（毫秒）。預設 2000 對一個每秒跑一次的迴圈太久。
const CONNECTION_TIMEOUT_MS: u32 = 400;
/// 單次跨程序交易的逾時（毫秒）。預設是 20000。
const TRANSACTION_TIMEOUT_MS: u32 = 700;

/// 連續放棄幾次之後宣告投降。
///
/// 每放棄一次就漏一條執行緒（它還卡在回不來的 UIA 呼叫裡），所以這個
/// 數字同時是「最多漏幾條」。三條的代價可以接受，無限條不行。
const MAX_ABANDONS: u32 = 3;

/// 走訪樹的預算。位址列在 Chromium 上大約是第 4～6 層。
const MAX_DEPTH: u32 = 8;
const MAX_NODES: u32 = 400;

/// 連續問不出「焦點在不在密碼欄上」幾次後，宣告能力已失效。
///
/// 短暫與持續的不知道都繼續 fail closed。這個門檻只決定什麼時候
/// 要把「能力壞了」告訴使用者，不再是什麼時候把保護關掉。
const MAX_UNKNOWN_STREAK: u32 = 5;

/// 對某個視窗問到的東西。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reading {
    /// 非瀏覽器、已讀到網址、或瀏覽器網址沒量到必須是三個不同答案。
    pub browser_url: BrowserUrlState,
    /// 鍵盤焦點是不是在密碼欄上。
    ///
    /// `None` 是「問不出來」，**不是 `false`**。這個區別就是這一欄存在的
    /// 理由：把「不知道」壓成「沒有」會產生一個永遠不會被發現的漏擋。
    pub password_focused: Option<bool>,
}

impl Default for Reading {
    fn default() -> Self {
        Self {
            browser_url: BrowserUrlState::Unknown,
            password_focused: None,
        }
    }
}

impl Reading {
    /// 這一幀該不該因為「可能正在輸入密碼」而整個放棄擷取。
    ///
    /// **不知道也算要。** 少擋了不會有任何症狀，使用者永遠不會發現；
    /// 多擋了她查不到東西，會來抱怨，然後我們就修得掉。兩個方向不對等，
    /// 所以往看得見的那邊倒（THREAT_MODEL「最危險的失效模式」）。
    ///
    /// UIA 整個掛掉時不會產生 `Reading`；呼叫端會把整個 privacy
    /// context 當成 Unknown，同樣在任何內容來源之前擋住。
    pub fn should_skip_frame(&self) -> bool {
        self.password_focused.unwrap_or(true)
    }
}

/// UIA 客戶端。持有那條隨時可以被丟掉的工作執行緒。
pub struct Uia {
    worker: Option<SyncSender<Job>>,
    abandons: u32,
    /// 放棄太多次了。這台機器上就是讀不到，別再漏執行緒。
    surrendered: bool,
    /// 連續幾次問不出密碼欄狀態。見 [`MAX_UNKNOWN_STREAK`]。
    unknown_streak: u32,
}

struct Job {
    hwnd: isize,
    /// 是否為瀏覽器。密碼欄每次都問；瀏覽器每拍重讀位址列 Value。
    browser: bool,
    reply: SyncSender<Reading>,
}

impl Default for Uia {
    fn default() -> Self {
        Self::new()
    }
}

impl Uia {
    pub fn new() -> Self {
        Self {
            worker: None,
            abandons: 0,
            surrendered: false,
            unknown_streak: 0,
        }
    }

    /// 這台機器上還讀不讀得到。放棄之後永遠是 `false`。
    pub fn is_alive(&self) -> bool {
        !self.surrendered
    }

    /// 密碼欄那一路已經連續問不出來太多次了。
    ///
    /// 這只是給能力報告的狀態，不能用來繞過 privacy gate；不論短暫或
    /// 持續不知道，呼叫端都必須繼續 fail closed。
    pub fn password_check_broken(&self) -> bool {
        self.unknown_streak >= MAX_UNKNOWN_STREAK
    }

    /// 一次問答回來了。兩個計數器都在這裡結帳。
    ///
    /// 卡住的計數是**連續**的：中間只要有一次回得來，就證明這條路還通，
    /// 前面那幾次是偶發而不是壞掉。所以歸零，而不是累積到某天湊滿三次
    /// 就讓 UIA 永久停工——那會讓一台好機器在跑了一整個下午之後，
    /// 因為三次分散的抽筋而開始 fail closed、不再留下內容。
    fn note_answer_arrived(&mut self, password_seen: Option<bool>) {
        self.abandons = 0;
        self.note_password_reading(password_seen);
    }

    /// 記一次「執行緒卡住、被丟掉」，回傳這一幀還問不問得出東西。
    ///
    /// 搬出來的理由跟 [`Self::note_password_reading`] 一樣，而且更急：
    /// 這段以前長在 `read()` 中間，而 `read()` 要一條活的 COM 工作執行緒，
    /// 於是這個**會讓整個 privacy context 永久 Unknown** 的狀態機，
    /// 一條測試都沒有。它關掉之後 recorder 會 fail closed；能力報告仍要說明，
    /// 否則使用者只看到記憶無故中斷。
    ///
    /// 跟密碼欄那個計數不一樣，這裡**沒有復原**：每放棄一次就漏一條卡在
    /// UIA 裡回不來的執行緒，「再試一次」的代價是再漏一條。誠實地宣告
    /// 做不到，比為了一個可能好不了的機會繼續漏執行緒划算。
    fn note_abandoned_thread(&mut self) -> Option<Reading> {
        // 對面卡住了，而我們沒有辦法把它叫回來。整條丟掉。
        // **不要 join。** join 就等於把自己也賠進去。
        self.worker = None;
        self.abandons += 1;
        if self.abandons >= MAX_ABANDONS {
            self.surrendered = true;
            tracing::warn!("UIA 連續卡住 {MAX_ABANDONS} 次，放棄讀取網址");
            return None;
        }
        // 這一次問不出來。焦點狀態未知 → 照 `should_skip_frame`
        // 的規則往安全的那邊倒。
        Some(Reading::default())
    }

    /// 記一次密碼欄問答的結果，維護那個連續失敗計數。
    ///
    /// 這段本來長在 `ask()` 中間。搬出來讓「第 5 次開始對外報告能力
    /// 失效」的門檻能被測；它不會把 fail-closed 保護關掉。
    fn note_password_reading(&mut self, seen: Option<bool>) {
        match seen {
            // 問得出來就歸零。機器好起來了，保護就該回來——
            // 這是刻意的：一次成功的問答就足以證明這條路是通的。
            Some(_) => self.unknown_streak = 0,
            None => {
                self.unknown_streak += 1;
                // `==` 不是 `>=`：只在跨過那條線的**那一次**講，
                // 之後每一幀都喊一次只會變成沒有人看的雜訊。
                if self.unknown_streak == MAX_UNKNOWN_STREAK {
                    tracing::warn!(
                        "連續 {MAX_UNKNOWN_STREAK} 次問不出焦點是否在密碼欄上；\
                         持續 fail closed，並將能力標成失效"
                    );
                }
            }
        }
    }

    /// UIA 這個東西在這台機器上叫不叫得動。
    ///
    /// 只建一次 COM 物件就丟掉，**不碰任何視窗**——所以它不會卡在別人的
    /// 訊息迴圈上，`doctor` 可以安心呼叫。但也因此它只證明了「UIA 在」，
    /// 沒有證明「讀得到位址列」。那兩件事的差別，正是這個專案這兩天
    /// 學到的那一課（THREAT_MODEL 安靜失效 #6）。
    pub fn probe() -> bool {
        // 開在另一條執行緒上：`CoInitializeEx` 會把呼叫端的 apartment
        // 定下來，而主執行緒的 apartment 是 OCR 在用的，不該被這裡影響。
        std::thread::Builder::new()
            .name("sister-uia-probe".into())
            .spawn(|| {
                unsafe {
                    let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
                }
                let ok = create_automation().is_some();
                unsafe { CoUninitialize() };
                ok
            })
            .ok()
            .and_then(|h| h.join().ok())
            .unwrap_or(false)
    }

    /// 問一個前景視窗：它的網址是什麼、焦點是不是在密碼欄上。
    ///
    /// 回 `None` 代表 UIA 在這台機器上不能用（或已經投降）。呼叫端會
    /// 把整個 privacy context 當成 Unknown，在內容來源前停下。
    ///
    /// 每個前景 app 都問焦點是否在敏感欄；`want_url_for_window`
    /// 只控制貴得多的位址列樹遍歷，只有瀏覽器會開。
    pub fn read(&mut self, hwnd: HWND, browser: bool) -> Option<Reading> {
        if self.surrendered {
            return None;
        }

        let (tx, rx) = sync_channel(1);
        let job = Job {
            hwnd: hwnd.0 as isize,
            browser,
            reply: tx,
        };

        let worker = self.worker.get_or_insert_with(spawn_worker);
        if worker.send(job).is_err() {
            // 上一條執行緒已經收工（多半是它自己建不出 UIA 物件）。
            // 這不算「卡住」，所以不記在 abandons 上，但也不無限重試。
            self.worker = None;
            self.surrendered = true;
            return None;
        }

        match rx.recv_timeout(ASK_BUDGET) {
            Ok(reading) => {
                self.note_answer_arrived(reading.password_focused);
                Some(reading)
            }
            Err(_) => self.note_abandoned_thread(),
        }
    }
}

fn spawn_worker() -> SyncSender<Job> {
    // 容量 1：客戶端一次只問一題，而且問完就等。排隊沒有意義——
    // 排在後面的請求對應的是已經過去的那一幀。
    let (tx, rx) = sync_channel::<Job>(1);
    let spawned = std::thread::Builder::new()
        .name("sister-uia".into())
        .spawn(move || worker_main(rx));
    if spawned.is_err() {
        tracing::warn!("開不出 UIA 執行緒");
    }
    tx
}

/// 工作執行緒。所有 COM 物件的生命週期都關在這裡面——它們不是 `Send`，
/// 而且這條執行緒隨時可能被拋棄，讓它們跟著一起走是最乾淨的。
fn worker_main(rx: Receiver<Job>) {
    unsafe {
        // MTA：我們不會跑訊息迴圈，而 STA 的 COM 需要有人抽訊息。
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
    }

    if let Some(automation) = create_automation() {
        // 只快取 COM element locator，不快取它的字。SPA、同標題導覽及
        // 位址列編輯都可能在 HWND 不變時改 URL；Value 每一拍都重讀。
        let mut cached_address: Option<(isize, IUIAutomationElement)> = None;
        while let Ok(job) = rx.recv() {
            let reading = probe(
                &automation,
                HWND(job.hwnd as *mut _),
                job.browser,
                &mut cached_address,
            );
            // 客戶端可能已經放棄我們了（容量 1 的 channel 不會阻塞）
            let _ = job.reply.send(reading);
        }
    }

    unsafe { CoUninitialize() };
}

fn create_automation() -> Option<IUIAutomation> {
    let automation: IUIAutomation =
        unsafe { CoCreateInstance(&CUIAutomation8, None, CLSCTX_INPROC_SERVER) }.ok()?;

    // `IUIAutomation2` 才有逾時設定。拿不到就照預設跑——預設的
    // 20 秒交易逾時對我們太久，但有總比沒有好。
    if let Ok(a2) = automation.cast::<IUIAutomation2>() {
        unsafe {
            // **關掉自動設焦點。** 預設情況下 UIA 客戶端在某些操作上會
            // 把焦點搶過去。一個背景錄製程式把使用者打字打到一半的焦點
            // 搶走，是不可接受的。
            let _ = a2.SetAutoSetFocus(false);
            let _ = a2.SetConnectionTimeout(CONNECTION_TIMEOUT_MS);
            let _ = a2.SetTransactionTimeout(TRANSACTION_TIMEOUT_MS);
        }
    }
    Some(automation)
}

fn probe(
    automation: &IUIAutomation,
    hwnd: HWND,
    browser: bool,
    cached_address: &mut Option<(isize, IUIAutomationElement)>,
) -> Reading {
    let Ok(root) = (unsafe { automation.ElementFromHandle(hwnd) }) else {
        *cached_address = None;
        return Reading::default();
    };

    // 全桌面的 GetFocusedElement 只有在能沿 parent chain 證明屬於這個 exact
    // HWND root 時才有意義。否則別的視窗的普通欄不能替這個視窗背書。
    let password_focused = focused_is_password(automation, &root);

    let browser_url = if browser {
        read_browser_url(automation, hwnd, &root, cached_address)
    } else {
        *cached_address = None;
        BrowserUrlState::NotApplicable
    };

    Reading {
        browser_url,
        password_focused,
    }
}

/// 鍵盤焦點是不是在密碼欄上。`None` = 問不出來。
///
/// 用 `CurrentIsPassword()` 這個型別化的存取器，而不是
/// `GetCurrentPropertyValue`——後者回傳 `VARIANT`，會逼我們開
/// `Win32_System_Variant` 這個 Cargo feature，只為了讀一個 bool。
fn focused_is_password(automation: &IUIAutomation, root: &IUIAutomationElement) -> Option<bool> {
    let element = unsafe { automation.GetFocusedElement() }.ok()?;
    belongs_to_root(automation, &element, root)?;
    Some(unsafe { element.CurrentIsPassword() }.ok()?.as_bool())
}

/// `element` 是 `root` 本身或 descendant 才回 Some。走不到 root、COM
/// 問不出來都回 None；呼叫端會把敏感欄狀態當 Unknown。
fn belongs_to_root(
    automation: &IUIAutomation,
    element: &IUIAutomationElement,
    root: &IUIAutomationElement,
) -> Option<()> {
    if unsafe { automation.CompareElements(element, root) }
        .ok()?
        .as_bool()
    {
        return Some(());
    }
    let walker = unsafe { automation.ControlViewWalker() }.ok()?;
    let mut current = element.clone();
    // UIA tree 理論上有限；上限避免壞 provider 造出 parent cycle。
    for _ in 0..64 {
        current = unsafe { walker.GetParentElement(&current) }.ok()?;
        if unsafe { automation.CompareElements(&current, root) }
            .ok()?
            .as_bool()
        {
            return Some(());
        }
    }
    None
}

fn read_browser_url(
    automation: &IUIAutomation,
    hwnd: HWND,
    root: &IUIAutomationElement,
    cached: &mut Option<(isize, IUIAutomationElement)>,
) -> BrowserUrlState {
    let key = hwnd.0 as isize;
    if cached
        .as_ref()
        .is_some_and(|(cached_key, _)| *cached_key != key)
    {
        *cached = None;
    }

    if let Some((_, element)) = cached.as_ref() {
        if belongs_to_root(automation, element, root).is_some()
            && let Some(state) = read_cached_address(element)
        {
            return state;
        }
        *cached = None;
    }

    let Some((element, state)) = find_address_bar(automation, root) else {
        return BrowserUrlState::Unknown;
    };
    *cached = Some((key, element));
    state
}

/// 從視窗根節點往下找位址列。
fn find_address_bar(
    automation: &IUIAutomation,
    root: &IUIAutomationElement,
) -> Option<(IUIAutomationElement, BrowserUrlState)> {
    let walker = unsafe { automation.ControlViewWalker() }.ok()?;
    let mut budget = MAX_NODES;
    walk(&walker, root, 0, &mut budget)
}

fn walk(
    walker: &IUIAutomationTreeWalker,
    element: &IUIAutomationElement,
    depth: u32,
    budget: &mut u32,
) -> Option<(IUIAutomationElement, BrowserUrlState)> {
    if depth > MAX_DEPTH || *budget == 0 {
        return None;
    }
    *budget -= 1;

    if is_web_content(element) {
        // 網頁那一支整支不進去。進去一次的代價是瀏覽器要建整棵樹。
        return None;
    }
    if let Some(state) = address_candidate(element) {
        return Some((element.clone(), state));
    }

    let mut child = unsafe { walker.GetFirstChildElement(element) }.ok();
    while let Some(node) = child {
        if let Some(found) = walk(walker, &node, depth + 1, budget) {
            return Some(found);
        }
        child = unsafe { walker.GetNextSiblingElement(&node) }.ok();
    }
    None
}

/// 這個節點是不是網頁內容（也就是「不要進去」）。
///
/// 兩層判斷刻意都留著：
/// - `Document` 控制項型別是通用的答案，跨瀏覽器都對
/// - class name 是更早的一道閘門，在 HWND 這一層就攔下來，
///   連問這個節點的控制項型別都不必——而「問」本身就可能觸發具現化
fn is_web_content(element: &IUIAutomationElement) -> bool {
    if let Ok(class) = unsafe { element.CurrentClassName() } {
        let class = class.to_string();
        if class == "Chrome_RenderWidgetHostHWND" || class == "MozillaContentWindowClass" {
            return true;
        }
    }
    matches!(
        unsafe { element.CurrentControlType() },
        Ok(t) if t == UIA_DocumentControlTypeId
    )
}

/// 已快取為位址列的 element，每拍都重讀 Value 與 keyboard focus。
/// `None` 只表示 element 已不是 Edit，呼叫端會重新找 locator；任何讀取
/// 失敗或正在編輯都明確是 browser URL Unknown。
fn read_cached_address(element: &IUIAutomationElement) -> Option<BrowserUrlState> {
    if unsafe { element.CurrentControlType() }.ok()? != UIA_EditControlTypeId {
        return None;
    }

    let focused = match unsafe { element.CurrentHasKeyboardFocus() } {
        Ok(value) => value.as_bool(),
        Err(_) => return Some(BrowserUrlState::Unknown),
    };
    let value = match current_value(element) {
        Some(value) => value,
        None => return Some(BrowserUrlState::Unknown),
    };
    if focused {
        return Some(BrowserUrlState::Unknown);
    }
    Some(
        plausible_url(&value)
            .map(BrowserUrlState::Known)
            .unwrap_or(BrowserUrlState::Unknown),
    )
}

/// 走樹時只把「目前有 plausible URL 的 Edit」認成位址列。若它正被
/// 編輯仍可快取 element，但這一拍一定回 Unknown。
fn address_candidate(element: &IUIAutomationElement) -> Option<BrowserUrlState> {
    if unsafe { element.CurrentControlType() }.ok()? != UIA_EditControlTypeId {
        return None;
    }
    let focused = unsafe { element.CurrentHasKeyboardFocus() }.ok()?.as_bool();
    let value = current_value(element)?;
    let url = plausible_url(&value)?;
    Some(if focused {
        BrowserUrlState::Unknown
    } else {
        BrowserUrlState::Known(url)
    })
}

fn current_value(element: &IUIAutomationElement) -> Option<String> {
    let pattern: IUIAutomationValuePattern = unsafe {
        element
            .GetCurrentPattern(UIA_ValuePatternId)
            .ok()?
            .cast()
            .ok()?
    };
    Some(unsafe { pattern.CurrentValue() }.ok()?.to_string())
}

// **焦點在位址列上的時候不要讀。** 使用者正在打字或用方向鍵選建議項，
// 那時候 `Value` 是給人看的一句話，不是已確認網址。問不出焦點也不是
// 「沒有焦點」；兩種情況都由上面的 typed `BrowserUrlState::Unknown` 擋住。

/// 位址列讀回來的字看起來像不像一個位址。
///
/// 純字串判斷，所以可以在 Linux 上測——這個模組其餘部分都不行。
///
/// 判斷刻意寬鬆：這個值不是拿來顯示的，是拿來餵排除規則的。寧可讓一個
/// 不太像網址的字串進來被規則比對，也不要因為過度嚴格而讓一個真的網銀
/// 網址被擋在門外——那樣的話規則同樣不會生效，而且原因更難查。
fn plausible_url(value: &str) -> Option<String> {
    let v = value.trim();
    if v.is_empty() || v.len() > 2048 {
        return None;
    }
    // 位址列在沒有分頁時是空的，或者是一句提示。有空白幾乎一定是提示語
    // （真的網址裡的空白會被編碼成 %20）。
    if v.contains(char::is_whitespace) {
        return None;
    }
    // 至少要有一個點（網域）或一個已知的 scheme。`localhost:3000`
    // 這種沒有點的也放行，因為開發時整天都在看它。
    let has_scheme = v.contains("://") || v.starts_with("about:") || v.starts_with("chrome:");
    if has_scheme || v.contains('.') || v.starts_with("localhost") {
        Some(v.to_string())
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 「不知道」必須跟「沒有密碼欄」分開。壓成同一個布林的話，
    /// 每一次 UIA 出錯都變成一次沒有人會發現的漏擋。
    #[test]
    fn not_knowing_is_treated_as_maybe_a_password() {
        assert!(Reading::default().should_skip_frame(), "不知道要往安全的擋");
        assert!(
            Reading {
                password_focused: Some(true),
                ..Default::default()
            }
            .should_skip_frame()
        );
        assert!(
            !Reading {
                password_focused: Some(false),
                ..Default::default()
            }
            .should_skip_frame()
        );
    }

    /// 持續失敗到了門檻要被能力報告看見，但安全後果不會反轉。
    #[test]
    fn persistent_password_unknown_is_reported_without_disabling_the_guard() {
        let mut uia = Uia::new();
        assert!(!uia.password_check_broken(), "一開始不該是壞的");

        // 差一次就到。這裡還在「保守」的那一段，保護仍然生效。
        for i in 1..MAX_UNKNOWN_STREAK {
            uia.note_password_reading(None);
            assert!(
                !uia.password_check_broken(),
                "第 {i} 次問不出來就放棄了，門檻是 {MAX_UNKNOWN_STREAK}"
            );
        }

        uia.note_password_reading(None);
        assert!(
            uia.password_check_broken(),
            "連續 {MAX_UNKNOWN_STREAK} 次之後該宣告做不到"
        );
        assert!(
            Reading::default().should_skip_frame(),
            "能力報告變成失效後，不知道仍然必須擋住"
        );
    }

    /// 一次問得出來的答案就把能力報告復原。
    ///
    /// 這是刻意的：連續失敗代表這條路不通，而一次成功就證明它通了。
    /// 沒有這一條的話，一台偶爾抽風的機器會永久顯示錯誤能力狀態。
    #[test]
    fn one_good_answer_recovers_the_sensitive_field_capability_report() {
        let mut uia = Uia::new();
        for _ in 0..MAX_UNKNOWN_STREAK {
            uia.note_password_reading(None);
        }
        assert!(uia.password_check_broken());

        uia.note_password_reading(Some(false));
        assert!(!uia.password_check_broken(), "問得出來之後報告該復原");

        // 而且是真的歸零，不是減一——否則下一次失敗就又壞掉。
        uia.note_password_reading(None);
        assert!(!uia.password_check_broken(), "歸零之後不該一次失敗就再壞掉");
    }

    /// 卡住三次之後投降，而在那之前每一幀都還是往安全的那邊倒。
    ///
    /// 這是這個檔案裡代價最大的一個狀態機：踩到之後 `read()` 永遠回
    /// `None`。呼叫端必須把整個 privacy context 當 Unknown，不能把
    /// `None` 換成一張看似安全的空 snapshot。
    #[test]
    fn three_stuck_threads_make_the_whole_context_unknown() {
        let mut uia = Uia::new();
        assert!(uia.is_alive(), "一開始不該是投降狀態");

        // 還沒到門檻：仍然給得出一幀，而且那一幀是「不知道」→ 擋住。
        for i in 1..MAX_ABANDONS {
            let reading = uia.note_abandoned_thread();
            let reading =
                reading.unwrap_or_else(|| panic!("第 {i} 次就投降了，門檻是 {MAX_ABANDONS}"));
            assert!(
                reading.should_skip_frame(),
                "問不出來的時候要擋掉這一幀，不是放它過去"
            );
            assert!(uia.is_alive());
        }

        assert!(
            uia.note_abandoned_thread().is_none(),
            "連續 {MAX_ABANDONS} 次之後該投降"
        );
        assert!(
            !uia.is_alive(),
            "投降之後 is_alive() 要是 false，能力報告才能說明 privacy context 不可用"
        );
    }

    /// 三次**分散**的抽筋不算投降。
    ///
    /// 沒有這一條的話，一台跑了整個下午的好機器會因為三次互不相干的
    /// 逾時而永久進入 fail-closed 空洞。註解寫的是「連續」，這裡是那兩個字
    /// 唯一被執行的地方——`abandons = 0` 那一行刪掉，測試套件其餘部分
    /// 不會有任何反應。
    #[test]
    fn a_machine_that_recovers_in_between_does_not_surrender() {
        let mut uia = Uia::new();
        for _ in 0..MAX_ABANDONS * 3 {
            for _ in 1..MAX_ABANDONS {
                assert!(uia.note_abandoned_thread().is_some());
            }
            // 中間回得來一次，就證明這條路還通。
            uia.note_answer_arrived(Some(false));
            assert!(uia.is_alive(), "分散的逾時不該累積成投降");
        }
    }

    /// 投降是**單向**的：跟密碼欄那個計數不一樣，它不會自己好起來。
    ///
    /// 這是刻意的不對稱——每放棄一次就漏一條回不來的執行緒，所以
    /// 「再試一次」的代價是再漏一條。兩個計數器長得像，行為卻相反，
    /// 所以要有東西釘住這件事。
    #[test]
    fn surrender_is_permanent_even_if_an_answer_shows_up_later() {
        let mut uia = Uia::new();
        for _ in 0..MAX_ABANDONS {
            uia.note_abandoned_thread();
        }
        assert!(!uia.is_alive());

        uia.note_answer_arrived(Some(false));
        assert!(
            !uia.is_alive(),
            "投降之後不該因為一次成功就回來——那會再開始漏執行緒"
        );

        // 而且是真的不再問了。這一行走的是 `read()` 開頭那道閘門：
        // 投降之後它在碰任何 COM 之前就回 `None`，所以拿一個空的 HWND
        // 問它是安全的——如果哪天那道閘門不見了，這裡會直接當掉，
        // 而不是安靜地又開始漏執行緒。
        assert!(
            uia.read(HWND(std::ptr::null_mut()), false).is_none(),
            "投降之後不該再送出任何一個 job"
        );
    }

    /// 位址列在「還沒輸入」時是提示語，不是網址。那種字串進了
    /// `frames.url` 之後，排除規則會拿散文去比對網銀關鍵字。
    #[test]
    fn prompts_and_prose_are_not_urls() {
        for prose in [
            "",
            "   ",
            "搜尋或輸入網址",
            "Search Google or type a URL",
            "台灣銀行 — Google 搜尋",
            "no-dots-here",
        ] {
            assert_eq!(plausible_url(prose), None, "不該被當成網址：{prose:?}");
        }
    }

    /// `plausible_url` **擋不住**使用者正在打的字——這條釘的是它的極限，
    /// 不是它的能力。
    ///
    /// 真正擋住「打到一半的位址列」的是 `url_from` 裡那道
    /// `CurrentHasKeyboardFocus` 閘門。`plausible_url` 只濾掉有空白的
    /// 提示語，而一串沒有空白、又帶點的字——在位址列裡搜的 email、
    /// 內網主機、打錯的網域——會一路通過，存進 `frames.url`。
    ///
    /// 所以這裡刻意斷言它們**會通過**。哪天有人覺得「反正有
    /// `plausible_url` 把關」而把那道焦點閘門拿掉，這條測試會告訴他
    /// 把關的從來不是這裡。那道閘門沒有辦法用單元測試驗（它要一個活的
    /// COM 元素），所以它的重要性只能寫在這裡。
    #[test]
    fn plausible_url_is_not_a_substitute_for_the_keyboard_focus_gate() {
        for typed in [
            "john.smith@company.com", // 在位址列裡搜一個 email
            "192.168.1.50",           // 內網主機
            "cathaybk.com",           // 打到一半的網域
        ] {
            assert_eq!(
                plausible_url(typed).as_deref(),
                Some(typed),
                "這個字串被擋下來了，代表這條測試的前提變了——請重讀 url_from"
            );
        }
    }

    /// 真實的位址列內容——包括 Chromium **縮寫過**的那種形狀。
    #[test]
    fn real_address_bar_values_survive() {
        for value in [
            // Chromium 縮寫後的樣子：沒有 scheme、沒有 www.
            "ebank.taiwanbank.com.tw/login",
            "example.com/a?b=c&d=e",
            // Firefox 給完整 URL
            "https://www.example.com/path",
            "localhost:3000/admin",
            "about:blank",
            "chrome://settings/passwords",
        ] {
            assert_eq!(
                plausible_url(value).as_deref(),
                Some(value),
                "真的網址被擋掉了：{value}"
            );
        }
    }

    /// 縮寫過的網址仍然必須讓預設的排除規則命中，否則整條路白做了。
    ///
    /// 這條是把兩個模組接起來的斷言：UIA 給的是縮寫形式，而規則是
    /// 在 `config` 那邊寫的——各自看起來都對，接起來才知道通不通。
    #[test]
    fn an_elided_banking_url_still_trips_the_default_rules() {
        let config = sister_core::config::Config::default();
        let elided = plausible_url("ebank.taiwanbank.com.tw/login").expect("是網址");

        let focus = sister_core::model::PrivacyContext::known(
            sister_core::model::FocusSnapshot {
                app_id: Some("chrome.exe".into()),
                window_title: Some("臺灣銀行".into()),
                ..Default::default()
            },
            sister_core::model::SensitiveFieldState::Clear,
            sister_core::model::BrowserUrlState::Known(elided),
        );
        assert!(
            config.privacy.check(&focus).reason().is_some(),
            "縮寫過的網銀網址沒有被擋下來——UIA 讀到了，規則卻不認得"
        );
    }
}
