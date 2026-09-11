# PRIVACY — 承諾、邊界，以及我們做不到的事

AI-Sister 每一拍做完後預設等 400ms 再看；沒有人動鍵盤滑鼠時可以不擷取，
但最多盲 5 秒。這份文件說明她拿這些畫面做什麼、不做什麼，以及有哪些事她
**做不到**。

細節見 [DATA_INVENTORY.md](DATA_INVENTORY.md)（逐欄位）與
[THREAT_MODEL.md](THREAT_MODEL.md)（會怎麼出錯）。

---

## 三件事

**一、畫面不離開這台機器；內建網路能力只有列得出名字的兩條。**
沒有帳號、沒有遙測、沒有崩潰回報。`sister.exe` 與 recorder／core／capture／brain／
hands 沒有 HTTP client 或監聽埠；WebView 也只走 Tauri IPC，CSP 不開遠端來源。desktop
只有兩條內建 outbound：你看完揭露、明確按下後取得 Persona 固定公開素材的 GET；
以及 alpha.110 預設關閉、簽獨立同意並明確啟用後，只替最新新答案或手動重播送正文的 TTS POST。
這不是一句「大致上本機」：`crates/sister-assets` 預設不開 `download`，`crates/sister-tts`
預設不開 `azure`，只有 desktop 明確啟用；CI 逐棵相依樹與 renderer/CSP 檢查這兩條邊界
（`scripts/check-no-network.sh`），不是靠我們記得。

供 L2/L3 解讀與 S1 問答成句的螢幕 **OCR 文字原文**（不是畫面）只在你簽了第二張
同意書、而且接好一支大腦 CLI 之後，才會交給那支你自己已經在跑的 CLI。S1 先把當前問題
交給 CLI，讓它回最多三條自然語言查詢；AI-Sister 在 SQLite 本機代查，再送最多 12 筆
命中來源文字及其時間、app、title、URL metadata。問題副本最多 2 KiB，圍欄內資料合計
最多 12 KiB；CLI 不取得 DB path、SQL、整份資料庫或畫面。沒簽或沒設定就不 spawn；本機
零命中時 CLI 仍已處理查詢規劃，但不會再收到來源或生成無來源事實答案。外送紀錄只記
結構和計數，不抄問題、來源或 prompt 原文。Azure TTS 不取得
整份 OCR corpus；成句存在時只取得通過逐句本機來源驗證的 1–3 句答案正文，不取得 source
ref、按鈕文字、metadata 或底下重複的 facts／OCR。答案正文自己可能引用或逐字重複記憶
內容，所以不能把「只送答案」誤寫成「不會含螢幕上的字」。

這條邊界已經擋掉過一個功能：OCR 本來要用 PP-OCRv5，但它的模型下載會把一個
HTTP client 連進長時間運作的 recorder。最後改用系統內建的 OCR，順帶少了 35MB；
Persona 與 Azure TTS 的窄能力都不能拿來替 OCR、brain 或 hands 開例外。

**二、暴力要暴在保存，不要暴在生成。**
她盡可能忠實地把發生過的事記下來（那是不可逆的——沒記到就永遠沒有了），
但**不**拿這些去推測、生成、腦補——除非你簽了第二張同意書並設定了 CLI。
程式自己仍然沒有推論引擎（同一支腳本的名單裡列著 `ort`、`candle`、
`tract`、`llama-cpp`：它們進了出貨的相依樹，CI 就會紅）。模型呼叫只走
`std::process::Command` spawn 你指定的那支 CLI。

**三、每一句話都要能追回出處。**
她說的每件事都能一路追回到當時的那張畫面。她不能「就是知道」某件事。

---

## Persona 本機素材與固定 GET

四姊妹與 13 位閨密的 17 套 workplace 分層 rig 與 WebP 退路都隨程式提供，
不下載也能完整使用；沒有 Neutral 或字母 fallback。Reel 只從 5.2 GB 本機
候選選出 402 張 PNG（35,140,885 bytes），不夾帶其他服裝、reaction、raw
receipt、私有 path 或 debug 圖；畫面也只解碼目前那一人。S1 的記錄、搜尋、
證據、刪除與匯出一項都不少。

17 人的角色語音素材也隨 desktop 安裝：每人基本包 8 句、擴充包 24 句，共 544 段
Ogg Opus、8,918,728 bytes；另有 17×4、共 68 段同意書朗讀。開程式、開設定、hover、切換角色、重開、點角色與
播放這些同源聲音，都不授權下載，也不會背景預抓或自動更新。只有角色點擊會播放
本機 bundled Ogg，不會呼叫系統 TTS、CLI、Azure 或其他網路服務；輸入框送出的每個
文字問題則一律走已選 CLI 與本機記憶路徑，不用固定音檔旁路。
同意書錄音也在程式裡；只有你在正在閱讀的那張按「念給我聽」才會播放目前角色的
錄音，不會自動播放。顯示條文仍只信 native core；錄音逐字稿不同就整段停用，不能讓
舊錄音替改版後的新授權說話。
一般動態答案的手動本機朗讀繼續只接受 WebView 明確標成 `localService` 的中文 voice；
找不到就靜音，不會改用 remote voice。

只有你在同一個揭露畫面看見下列三件事，再明確按「下載」後，desktop 才可嘗試：

- host 是 `cdn.ted-h.com`；固定 ZIP 是 **73,261,088 bytes**
- 請求不會放入 persona 選擇或狀態、OCR、畫面、問題、答案、記憶 ID、資料庫內容
  或其他私人內容
- DNS／CDN 仍會看見一般網路 metadata；CDN 會看見來源 IP、時間、TLS、固定的
  host／path／headers。這些是真的送出，不會被寫成「什麼都沒送」

按下後獲准的是**至多一個** HTTPS `GET`，固定到
`https://cdn.ted-h.com/tokenmonster/characters/v1/packs/ai-sister-media-11-voice55-2026.07.23/7d98e0d18c470f82818e8ada67208847c3cf4ff5c10cb5f99f9215191e981f30.zip`。
它不跟 redirect、不走 proxy、不送 cookie／credentials／authorization／referrer／
query／body，不 retry、不先 `HEAD`、也不逐檔連線。17 位角色與任何本機狀態都走相同
method／URL／headers／body；下載失敗後要再看揭露、再按一次，程式不能自己重試。

正式簽章的 app 內嵌 compact authority：descriptor、exact origin/path allowlist、四位
角色的 selected public rights projection、八段選用聲音的核准逐字稿，以及完整
schema-v2 manifest 的 canonical SHA-256
`21e4675653ce66b50b61e91260f1623e6e3005177f900991e3a8eeadaf9e6474`。
**約 2.1 MB 的完整 11 人 manifest 沒有嵌進執行檔。** descriptor 另外綁住 release
ID、73,261,088 bytes、946 entries 與 ZIP SHA-256
`7d98e0d18c470f82818e8ada67208847c3cf4ff5c10cb5f99f9215191e981f30`；整包 digest
pin 住那 946 項，真正會呈現／播放的 selected entries 再逐檔驗大小、hash 與 rights
binding；顯示文字不同於核准逐字稿時，那段聲音也不會進播放 allowlist。先驗完整
response 的大小/hash，才解析 ZIP；安全路徑、regular-file 集合與
entry count 全通過，才從同檔案系統 staging 原子啟用。全新 cache 收到半包、缺檔、
損毀或權利 binding 不符時不會建立可用 cache，畫面保留 `Available` 並顯示那次錯誤；
既有 cache 損毀、留下 staging 或撤回不完整才是 `RepairNeeded`。兩種都保留隨程式
提供的所選角色圖，而且不自動連線修復。

cache 固定在 `Config::default_data_dir()/persona-assets-v1`。它是公開素材 cache，不是
記憶：`--data-dir` 不搬它，memory export、`forget`、`prune` 都不讀、不複製、不刪它；
Persona 撤回才停掉 fixed voice 並精準刪除該 release；bundled 角色圖不受影響。cache 刪不掉就明講
`RepairNeeded`，不假裝已經撤乾淨，也不因此連網。

cache 旁會保留一個空的跨行程 lock、一個首次安裝嘗試或撤回建立的 OS-random epoch、
後續的 OS-random 撤回 tickets、每次完整安裝成功時對 epoch＋ticket 名稱集合寫下的
authorization digest，以及最後一次 blocking remove 已結束處理的 settled digest。
新 ticket 會先讓舊 authorization 與 settled checkpoint 同時失效；remove 結束前，另一個
行程不能下載或修復，結束後也只有新的明確下載／修復能重新授權素材。內容只含 schema／
固定 release identity，或集合 digest；隨機值只在檔名，不含角色、PID、網路 metadata、
記憶、撤回次數或程式寫入的時間；檔案系統 metadata 與 ticket 數量仍可透露大致取得／
撤回活動。它們不進 memory export，也不由 `forget`／`prune` 刪除，因為那會讓另一個
行程的舊下載在撤回後重新被啟用。

這個當下下載按鈕不是持久同意書，也不能借用下面第四張 `azure-tts`；每一個新的 GET
都要重新揭露、重新按。舊包的下載授權不會變成 bundled 日常對話、Azure 或任何其他請求的授權。
動態答案必須由你另按「用本機聲音朗讀」，那顆按鈕仍只走 localService；沒有本機中文 voice
就靜音，不自動改用 Azure。

---

## Azure 可選 TTS 的固定 POST

alpha.110 的 Azure 繁中朗讀仍是**預設關閉**的第二條內建 outbound，不是本機語音的
fallback。找不到 `localService` 中文 voice 時仍靜音；Azure 失敗時也不自動改用本機或
別的雲端。只有這四道 gate 同時成立才可請求：設定明確啟用、region／voice 是 typed allowlist、
Windows Credential Manager 找得到 subscription key、現行第四張 `azure-tts` 同意有效。
此時每份使用者新問題的最新答案完成後自動送一次；trusted 手動重播會再送一次。開 app、
開設定、status 重讀、demo、舊答案重畫、選角色、錄製、記憶事件或本機播放都不會補送答案。

region 嚴格只有 `eastasia`、`southeastasia`、`japaneast`；native Rust 只可對所選
區域的 `https://<region>.tts.speech.microsoft.com/cognitiveservices/v1` 做一個 HTTPS
`POST`，不跟 redirect、不走 proxy、不 retry。renderer 不能指定 URL，WebView CSP 也
沒有 Azure host。每次 POST 的唯一使用者內容是**當前答案正文原文**，可能含姓名、
電話與金額而且不先遮罩；不送截圖、來源連結／出處 chip、memory id、整份資料庫或
其他文字。它不授權 OCR、歷史答案、問題、Persona 台詞或其他 UI 文字出境。

subscription key 存在目前 Windows 使用者的 Credential Manager，固定 target 是
`ted-h/AI-Sister/AzureSpeech/v1`；不寫進 `config.toml`、log、DB 或 memory export。
你輸入時 key 會短暫存在 password 欄位、renderer 記憶體與 Tauri IPC；送到 native 後
欄位立即清空。之後設定頁只取得 Present／Missing／Unreadable／Unsupported 狀態，不能
把已存 key 讀回頁面。這保護的是 app 不把 secret 當普通設定；同一 Windows 使用者權限的惡意程式
仍可能讀取該使用者的 Credential Manager，這不是 OS account compromise 的防線。

程式沒有文字或 MP3 的磁碟 cache，也不保留可供重播沿用的記憶體 cache；每份新答案與
每次 trusted 手動重播都可能產生一個新 POST。停止、換題或新播放會立刻讓舊 playback generation 失效，晚回來的 MP3
不播放、不快取；但已取得送出權的 blocking native POST **不能中途 abort**，仍可能跑到
45 秒 timeout，而且 Azure 可能已把它計入用量。介面只能說「不再播放」，不能說網路
請求已經取消。

第四張的送出與撤回會在同一把跨行程鎖裡排序，而且已入場的 request 會把 shared lock
保留到 transport 結束。native 另把最後一次 generation 檢查與 transport 放在和設定、
key、consent mutation／cancel 共用的 fence 裡。因此 transport 若先取得送出權，這些操作
可能等到最多 45 秒 timeout 才回覆；一旦任一操作回覆成功，較早讀到的設定／同意快照
不可能在那之後才開始送。renderer 仍會在等待 native cancel 時先停止播放。

Microsoft 目前公開列出的 Azure Speech F0 neural TTS 額度是每月 0.5 million
characters；免費與否、可用額度及費用仍以你的 Azure 帳號、resource、方案與 Microsoft
當下規則為準，AI-Sister 不提供或保證這份額度。見
[Azure Speech 定價](https://azure.microsoft.com/en-us/pricing/details/speech/)。

---

## 四張同意書

她開始看或把答案正文交給 Azure 之前，你要分別決定四個開關。它們**各自獨立、各自
隨時撤得掉**，而且**沒有一顆「全部同意」的按鈕**——一顆按下去打開四張的按鈕，會讓其他張在你
沒有分別想過的情況下被打開。

| 這一張 | 沒簽會怎樣 |
|---|---|
| 在我的硬碟上記錄我的螢幕 | `sister record` 拒絕啟動 |
| 把輸入的問題交給所選 CLI 決定本機記憶查詢，再把命中文字與出處交回同一支 CLI | L2/L3 解讀與 S1 問答都不會呼叫那支 CLI |
| 保留變化幀的截圖，而不是只留上面的字 | 她照樣記，但只記螢幕上的字 |
| 在設定開啟 Azure 新答案自動朗讀時，每份新答案完成後把該答案正文原文交給所選區域的 Microsoft Azure 語音服務並自動播放 | 一次都不呼叫 Azure；本機朗讀不受影響 |

- **第一張是硬閘門。** 沒簽的時候 `sister record` 不會開始錄，也不會「印個警告
  然後照錄」。它把該打的那行指令印出來，然後結束
- **第三張是降級，不是拒絕。** 沒簽她照常記錄，只是設定裡的 `store_images` 會
  被當場關掉——那一次錄製一張截圖都不寫
- **第二張是出境閘門。** 沒簽，問題、L2/L3 解讀與 S1 記憶文字一次都不會送給那支 CLI。而且這扇門
  不是每個呼叫端自己寫的 `if`：送出函式要一個只有檢查同意書才鑄得出來的憑證
  型別。條文改版後舊簽名失效，要重簽。
- **第四張是另一個出境閘門。** 它只鑄出 Azure TTS permit，不能借第二張、Persona
  下載按鈕或 `voice_enabled` 代替。條文明講正文可能含姓名、電話與金額且不遮罩，
  也明講不送截圖、來源連結、memory id、DB 或其他文字；沒簽一次都不 POST。
- **讀不到就當作沒簽。** 檔案不見、權限不足、TOML 壞掉、版本對不上，一律視為
  四張都沒簽——和暫停旗標同一條規則：不確定的時候往安全的那邊倒
- **條文改版，對應舊簽名失效。** 你當初按下去的是那一句話，不是那個欄位的名字。
  前三張共用條文版本；第四張有獨立 terms version。alpha.109 第一次讀到沒有 Azure
  欄位的同版本舊檔時，原三張簽名保留、第四張未簽；alpha.110 擴大為自動送新答案時，
  click-only 的第四張顯示為過期、只重簽第四張，前三張不被清掉
- **「隨時」是真的隨時，不是「下次重開」。** 正在跑的 `sister record` 每 5 秒
  重讀一次同意書；撤回第一張後最多再錄 5 秒加一拍，`capture.min_interval_ms`
  超過 5 秒時，主要會等那一拍，然後停止整場錄製（不是暫停——暫停是「先別
  看」，撤回是「我收回那句話」，稽核紀錄上不該留一筆理由是假的 pause）。撤回
  第三張，它從那一刻起只記字、不再寫圖；再簽回來也會當場恢復，因為「撤得掉卻
  改不回來」會讓人分不出這兩件事有什麼不同

狀態存在 `consent.toml`（和 `sister.db` 同一個目錄，純文字，看得懂也改得動），
記的是**何時**簽的而不是一個是非題——所以「你什麼時候同意的」答得出來。
第一次開啟時，還沒回答的四張會在主對話氣泡逐張顯示，使用者在原輸入框回答「同意」
或「不同意」；不另開視窗。每張保存成功才前進，保存失敗則留在原張。一題若剛好被新
版第二張擋住，原問題會保留，回答完後再交給目前選定的 CLI。回答「不同意」不會取得
權限，但會記住目前條文已問過，不在每次啟動反覆追問；條文改版仍會只重問變動的那張。
主視窗上方齒輪與系統匣都能再開設定裡的完整四張卡片；主對話、設定與
`sister consent` 都從 core 取同一份條文與未簽後果。`sister doctor` 讀同一個檔案，
另外報告目前是否簽署及會發生什麼事。

撤回是 `sister consent --revoke local-recording`，或在設定的完整卡片上把勾拿掉。
撤回**不會刪掉已經記下來的東西**——那是〈忘掉某一段時間〉的工作。

---

## Windows 登入啟動與 recorder 重試

登入啟動是獨立、立即生效且**預設關閉**的 opt-in 設定。它不在
`config.toml` 另存一個 bool；真相只是目前使用者的
`HKCU\Software\Microsoft\Windows\CurrentVersion\Run\AI-Sister`。缺值是
`disabled`；只有 `"<目前安裝的 sister-desktop.exe>" --ai-sister-login` 逐字相同
才是 `enabled`；另一個路徑、少參數或非字串是 `mismatch`，讀不到是
`unreadable`，portable／非 exact 安裝副本是 `unsupported`。後三種都不可被 UI
畫成「關閉」。

只有 current-user uninstaller metadata 的 `InstallLocation` 精確對應目前 exe parent，
這份程式才能修改 Run value；portable 不能替安裝版打開、關閉或修正它。
AI-Sister 不管 Windows `StartupApproved`；因此 `enabled` 只證明 Run value 已登錄，
「啟動應用程式」仍可另外停用它。關掉這個設定只影響之後的 Windows 登入；
它不會擅自停掉這一輪 recorder。

`--ai-sister-login` 啟動後只留在系統匣，不顯示或聚焦字母人；同一個 desktop worker
只承接第一個 login intent，delayed／secondary duplicate 都忽略且不 reveal。有效的
`local-recording` 同意才能請 desktop 啟動一個自己擁有的 recorder。同意沒簽、讀不到或版本失效時，
它不啟動 recorder，**也不自動彈出 onboarding 誘導簽署**。已存在的
pause 對內容擷取與寫入仍是硬門；login 可以把 recorder 行程開起來，但它必須
維持暫停。狀態讀壞也是 fail closed，這條路沒有 resume 能力。Login 不是一次真人
重新開始：它不清人工 Stop／`consent-revoked`，也不重設同一個 worker 的
backoff／GaveUp。

登入當下若碰到可重試的 transaction 或上一輪 shutdown handoff，preflight 每 500ms
從 consent、stop、lease 與 heartbeat 重新判定，最多五分鐘；它是獨立的有界等待，
不增加 watchdog failure count，也不借用 1／5／30 秒 retry budget。正常退出另寫強度
較弱的 typed `desktop-quit`；**只有這個 reason** 可讓下一次全新的 opt-in login 在
五分鐘內等舊 lease／fresh heartbeat 退場，再於 lease 已空且 heartbeat 是
NeverStarted／Stopped／Stalled 時走完整 barrier。stop marker 是 Absent 卻看見任何
lease／heartbeat owner 證據時就分類為 External；它稍後消失也不讓這份 login intent
takeover／restart。一般 external 的 stale／missing／unreadable 永遠不是已離場證明，
仍只接受 `stopped` 墓碑；typed `desktop-quit` 是有正常退出因果的窄 handoff 例外。

deadline 在每次 500ms attempt **之前**與最後 clear／spawn commit **之前**都重查，
不能在等待或 I/O 後 late spawn。同一 worker 收到真人 Stop、Quit 或 Explicit Start 會先
取消 pending Login；磁碟上的 `requested`／`consent-revoked`、無效 consent、無法安全
判讀的 consent／control 也直接取消，不因之後修好而偷偷再試。每一個失敗或逾時的
barrier 都保留原 stop marker。只有 bounded handoff 完整成功才可清 `desktop-quit`，
否則正常 shutdown 的 reason 留在磁碟上。

撤回 `local-recording` 時，CLI 與 desktop 都在 exclusive `consent.lock` transaction
裡、寫同意檔**之前**先 atomic publish 獨立的 `consent-revoke.barrier`，再 exact readback。
這道 barrier 有 fresh 256-bit generation，不由 recorder 消費，也不准 Explicit Start
清除；若 consent atomic save 失敗，舊同意可能仍有效，但 automatic／Explicit Start
仍須尊重這道停止條件，文案也不能反過來宣稱同意書已撤回。成功 commit 的 local
regrant 才能拿 writer 內捕捉的 generation ticket 清同一代；舊票遇到較新 revoke 只回
`Superseded`。清 barrier 前若普通 stop marker 確定不存在，先留下
`consent-revoked` stop latch；若已有 `requested`／`desktop-quit` 則原封保留。於是快速
revoke→regrant 也不會漏停目前 recorder，重簽本身更不是自動 restart。
同時改不同張同意也不可以是「後寫的整份蓋掉先寫的」。CLI 與 desktop 的每一次
grant／revoke 都要先持有 data-dir 空檔 `consent.lock` 的 OS exclusive lock，在鎖內
重讀最新檔案、只套這次指定的 sheet，再 atomic save。鎖或 save 失敗就報錯；
撤回第一張的獨立 barrier 必須先於 locked save，成功 regrant 的 generation-safe clear
則必須晚於 commit。`consent.lock` 若是 symlink／
non-regular path 或無法取得，這次 mutation fail closed，不跟著鎖到資料目錄外。

Recorder start 也必須跟 consent mutation 有唯一先後，不是只在鎖外 `load()` 兩次。
CLI／desktop／supervised child 的 lock order 固定為 shared `consent.lock` →
`stop.lock` → `recording.lock`／舊 heartbeat barrier。CLI recorder 的 shared guard 活過
stop clear 與第一拍；desktop parent 的 guard 活過 preflight／clear 到
`Command::spawn` 回來便立即放掉，絕不等待 heartbeat，child 自己 nonblocking 重拿並
持到第一拍。revoke／grant writer 已先握住 exclusive `consent.lock` 時，nonblocking
Login／retry／desktop Start 不排在 writer 後面偷跑，這一拍必須是零 clear、零 spawn、
零 heartbeat；真人 CLI Explicit 則等 writer commit 後在鎖內重讀。反過來若 start 先
取得 shared guard，它就明確排在 revoke 前；晚到的撤回仍由 durable barrier 與 recorder
檢查停止。這是線性順序，不是承諾兩邊同時都「贏」。

desktop 只對它自己 spawn 的 recorder child 重試。連續第一、二、三次失敗後
分別等 1 秒、5 秒、30 秒；第四次後放棄。只有新鮮而且相對上一個已觀察樣本
真的更新的 `Recording` heartbeat 能推進十分鐘連續區間；反覆 poll 同一拍
不算新證據。missing／stalled／unreadable／`Thinking` 一旦被觀察到就把區間
歸零，不從中斷前繼續累加；這才不會拿「child 還活著」冒充十分鐘健康錄製。
exit 0、人手停止、撤回本機記錄同意或 desktop quit 都取消 retry。真人 Stop 一送進
worker，就先取消 pending Login、watchdog timer 與之後的 automatic spawn，再做可能
失敗的 durable stop write；若寫入失敗，畫面必須照實保留「目前 recorder 可能仍在跑」，
但不能因此把已取消的 retry 復活。使用者仍可修好後再按 Stop；只有下一次真人 Explicit
Start **成功通過並 commit** 完整 barrier 才解除這道 in-memory automatic-start latch，
invalid／busy／timeout／spawn 前失敗的 Start 都不算。retry 到時仍要重新
檢查 stop intent、consent 與 occupancy；狀態不明就放棄，已占用就不開第二個。
外部啟動的 recorder 不會被接管或重啟；desktop 自己當掉時也沒有 self-relaunch。
owned child 消失後仍新鮮的舊 heartbeat 只會顯示成「正在確認」，不會冒充現在仍在錄，
也不開放 retry；後續持續更新的 heartbeat 才能證明是外部 recorder，並取消
desktop 原本的 retry。外部 recorder 隨後只由 heartbeat 顯示，desktop 不取得它的 Child
handle。後來變 stale 的一拍、missing 或 unreadable 都不證明 external 已退出；
Start／retry 繼續關閉，已送 stop 時也維持「正在停止外部 recorder」，直到看見明確的
`stopped` 墓碑。`try_wait` 本身暫時失敗時也保留原 handle、禁止另開；下一次成功 probe
才恢復 Running 或處理真正 exit。
使用者要結束 desktop 時，它同樣先取消 Login／retry，再把 durable stop 寫成功。如果資料目錄權限／磁碟錯誤
讓這一寫失敗，in-memory retry 會立刻取消，但 desktop **不退出**；它會把字母人
顯示、取得焦點並說明錯誤，也不接新 start。修好後再停／再結束，不能拿 UI
已消失冒充 recorder 已停。

heartbeat 檢查不是原子的 simultaneous-start 鎖。所有 CLI 與 desktop recorder 在第一拍
`Booting` heartbeat 前，都必須 nonblocking 取得 data-dir `recording.lock` 的 OS
whole-file exclusive lock，整場持有。拿不到或無法判定就不開；process crash／
handle drop 由 OS 自動釋放。這個空檔可以持久留著；**看到檔案存在不代表
有人在錄**，只有當下的 lock acquisition 能阻止同時 CLI／desktop 各開一個。
已存路徑是 symlink 或 non-regular file 時也拒絕，不跟著鎖到資料目錄外或把錯誤
檔案當正常鎖。

stop request、recorder consume 與下一次顯式 start 的 clear 全由永久空檔 `stop.lock`
線性化；probe 只做 nonblocking shared lock，鎖正忙或任何內容讀取不明都回 unknown，
automatic retry 不會因此猜成沒有停止意圖。顯式 start 先持上述 shared consent guard，
再持 exclusive stop transaction、通過 recorder lease 與舊 heartbeat barrier；全部成功才
清舊 marker。同時到達的
stop／quit 因而一定排在 clear 前或後。Desktop 的 parent 先以 nonblocking stop guard
暫持 recorder lease，通過 heartbeat 且 commit 後才 clear；再放行一律
supervised 的 child 重新取得整場 lease，並在第一拍前讀 stop／consent。child 無權清掉
稍後到達的 Stop／Quit。Login 只可在全新 worker 用同一套 barrier 清上一輪
`desktop-quit`；人工 `requested`／`consent-revoked` 都保留，retry 永不 clear。Windows live lock handle
拒絕刪除；Unix Preview 的 advisory lock 不阻止 unlink／replace，因此任何 AI-Sister 行程
仍在時都不可手動刪除或替換 `stop.lock`、`consent.lock`、`recording.lock`。

純命令／五態政策、watchdog 狀態轉移、renderer fixture、recorder lease 與 consent
locked mutation 有自動測試，Windows backend 也有只碰 test subkey 的 registry test。
這些只證明政策、lock protocol 與 API
read／write／readback，不是正式 alpha.107 安裝檔在真 Windows 登入、故障時序與
uninstall 的人工通過證據；那份 checklist 目前仍未勾。

---

## 什麼會被記錄

- 螢幕畫面（降採樣的 PNG，可關閉）
- 畫面上的文字（OCR）
- 前景視窗：程式名、視窗標題、網址
- 剪貼簿文字
- 打字與滑鼠的**計數與節奏**

## 什麼不會

- **按鍵內容。** 不是「過濾掉」，是從來沒讀過——Windows 的鍵盤 hook
  拿得到 `vkCode`，程式碼從頭到尾沒有解參考那個指標。
  **這句話每次 push 都會被檢查一次**（`scripts/check-no-keylogging.py`）：
  `kb_proc` 的函式體裡，那個指標只准原封不動交還給 `CallNextHookEx`。
  用正面表列而不是去抓 `*lparam` 這種字樣，是因為黑名單擋不住「先把位址
  存進區域變數再解」——把指標搬去別的地方，就已經算違規了。
  這個檢查自己也會被檢查：同一條規則必須抓得到隔壁 `mouse_proc` 那行真的
  解參考的程式碼，不然它就只是一個永遠是綠的檢查。
- 麥克風、攝影機、系統音訊或使用者音訊。Persona pack 的預錄固定台詞是公開素材，
  不是從這台機器錄來的聲音
- 網路流量、DNS 或封包內容（程式不擷取這些；明確發起的 Persona GET 與 Azure TTS
  POST 本身仍會發生）
- 檔案內容（除非它顯示在螢幕上）
- 位置、聯絡人、行事曆

---

## 預設就是安全的

你**不需要設定任何東西**就能得到以下保護：

| 保護 | 預設 |
|---|---|
| 密碼管理員 | KeePass(XC) / 1Password / Bitwarden / Dashlane / LastPass / Enpass 等整段不擷取 |
| 網銀與登入頁 | 16 條網址規則（讀的是位址列上的縮寫網址，見下） |
| 敏感欄 | 焦點在敏感欄時整幀不擷取；所有前景 app 都問，問不出來也不讀內容 |
| 敏感視窗標題 | `*password*`、`*密碼*`、`*無痕*`、`*private browsing*` |
| 會議 app | Zoom / Teams / Meet / WebEx / TeamViewer 等前景時自動暫停 |
| 剪貼簿秘密 | 疑似 API key / 私鑰時只記「發生過」，不記內容 |

排除發生在**擷取當下**：前置閘門已命中時，recorder 不讀剪貼簿內容，也不呼叫
螢幕擷取；不是先寫進資料庫或 PNG 再刪。但隱私檢查與 OS 擷取不是同一個原子操作：
若前景在兩者之間改變，`BitBlt` 可能已讓 pixels 短暫進入工作 RAM。recorder 會在
剪貼簿讀取後與螢幕擷取後重驗同一張 capture permit 和系統狀態；不一致就丟棄
staged bytes／frame，不讓它進 dedup、OCR、DB 或 PNG。這是目前能驗證的邊界，
不能縮寫成「任何被排除的 pixel 從未存在於記憶體」。

---

## 你的控制權

```bash
sister stats     # 她記了多少、佔多少空間
sister doctor    # 所有規則、目前失效的保護、schema 版本、四張同意書
sister query X   # 查任何東西，每筆都附出處
sister consent   # 四張同意書現在的狀態；--grant / --revoke 改它
```

- 設定檔是純文字 TOML，隨你改。**改完 5 秒內生效**，不用重開 `sister record`
  ——她會印一行字告訴你新的規則有幾條。這條很重要：以前設定只在啟動時讀一次，
  所以一個開著錄三天的人中途加的排除規則，三天內都不會生效，而且沒有任何地方
  會講。（截圖間隔、去重門檻那幾項仍然要重開；它們改錯只會讓畫面錄得比較醜，
  不會讓她錄到不該錄的東西。）
- 設定檔壞掉或不見了，她**繼續用舊的那一份**，不會退回預設值——預設值比任何
  一份自訂 blocklist 都寬鬆，一個打錯的 TOML 不該把你的排除規則整組拿掉
- 整份記憶就是一個 `sister.db` 檔加一個 `frames/` 目錄（她動過手的話再加一個
  `action-log.jsonl`）——刪除是一個確定的動作。
  但**要備份的時候不要自己複製那個檔案**：資料庫跑在 WAL 模式，她正在錄的時候
  最近寫進去的東西還躺在旁邊的 `sister.db-wal` 裡，只複製主檔的備份會安靜地
  少掉最後那一段，而你會在真的需要它的那天才發現。用 `sister export`（下面）
- 預設保留期：畫面 30 天、文字與事實 365 天。過 30 天刪掉的是 PNG 檔本身，
  上面的字留到 365 天。錄製開始時自動清一次；`sister prune --dry-run`
  可以先看它會刪掉什麼

### 忘掉某一段時間

字母人的時間軸（拖曳條上的 ▤、或系統匣的「她記得的每一天」）左邊列出她有紀錄
的每一天，右邊是那一天的內容。底下那一條可以指定範圍，把那一段**當作沒發生過**。

- **兩下才會刪。** 第一下只是問，她會把「會刪掉 N 段文字、M 張畫面（X MB）」
  擺在你眼前；第二下按鈕變成紅色的「確定刪掉」才真的動手。改了時間、換了日期，
  都會退回第一下——你確認過的是那一段，不是後來改成的另一段
- **和保留期不一樣，這裡不留字。** 保留期是兩段式的（先丟圖、字留著），因為
  那時候的前提是「你還想記得這件事，只是不需要那張圖」。你親手選一段按下忘掉
  的時候，前提正好相反：那一段的文字、事實、截圖、focus/剪貼簿/輸入節奏事件，
  全部一起消失
- **她那段時間動過的手也一起走。** `action-log.jsonl` 是資料庫**旁邊的另一個
  檔案**，裡面有完整的網址和檔案路徑。**alpha.70 起，只要你跑過一次
  `sister do`，即使一步都沒答應，它就已經存在了**——她提議過的每一串網址，
  和你打在 `--task` 上的那句話，都是在你回答之前就寫下去的。忘掉一段時間會把
  落在那段裡的列從檔案裡刪掉——刪的是那串
  字本身，不是蓋一個「已刪除」的旗標。有幾列讀不懂、問不出時間的話，它們一併
  刪掉，而且會分開報數字給你看
- **沒有回收桶，沒有復原。** 截圖是先從磁碟上刪掉、才動資料庫的——中途斷電
  最多留下一列指著不存在的圖，而不是一張沒有任何紀錄指向它的截圖
- 刪不掉的檔案（權限、正被開著）會**指名道姓地報出來**，不會被算進成功數字裡
- **留下來的通常只有一個位元：她曾經錄過。** 沒有時間、沒有長度、沒有版本，
  重建不出任何東西——而少了它，`sister stats`、`sister doctor` 和字母人那一頁會
  在你刪完的下一秒說「還沒錄過，先去按開始記錄」，也就是叫你重做一件你剛剛才
  刻意做掉的事。那一頁現在會說「她錄過，那些東西被忘掉了或過了保留期」，講的是
  **現在這顆資料庫**，不是那幾天。想連這個位元都不要，就整個刪掉 `sister.db`
  （只要跑過 `sister do`——**不必答應過任何一步**——或真的按過字母人
  「要我幫你打開嗎」那顆按鈕，旁邊另一個檔案 `action-log.jsonl` 就可能存在，
  要一起刪；字母人只有在你按下去、讓她執行時才寫）
- **有一個例外，而它不只一個位元：上一場錄製當掉的話，那一列 `sessions` 會留
  下來。** 那一列帶著開始時間、程式版本、平台——「那天 13:02 開始錄，Windows，
  alpha.33」。刪不掉它是因為它和**此刻正在錄的那一場**在資料庫裡長得一模一樣
  （都是「還沒收尾」的最新一列），而刪掉一場活著的錄製，接下來每一筆紀錄都會
  指向一個不存在的東西。所以這裡選擇不刪，並且講出來：`sister forget` 刪完會
  當場多印一行說那一列留著了，`sister stats` 上那個「工作階段 1」旁邊會標
  「空殼」（那兩個地方都會直說是**當掉**還是**她此刻正在錄**——分得出來就不
  印「或」）。它什麼時候會走，兩種原因的答案剛好相反：
  - **當掉的那一場**：等**她再開始錄之後**，那一列就不再是最新的一列，接下來
    任何一次清理都會把它帶走。**注意開錄那一次清理砍不到它**——那一刀跑在新
    的一場開始之前，那時候它還是最新的一列，所以整場錄製期間「工作階段」都會
    多一個。想馬上清掉，就在開始錄之後跑一次 `sister prune`
  - **她此刻正在錄的那一場**（也就是你錄到一半按下刪除）：等她**收工**。收尾
    的時候如果那一場還是一列都不剩，那一列會跟著走，連同它自己那兩列開始／
    結束的標籤

  連那一列都不想等，就整個刪掉 `sister.db`（同上，`action-log.jsonl` 要一起刪）

---

## 我們做不到的事

**這一節是這份文件存在的主要理由。**

### 旁人沒有同意被記錄

同事傳給你的私訊、視訊會議裡對方的畫面、你幫朋友查資料時螢幕上的他的個資
——**這些人沒有同意，而且不知道**。

會議 app 前景時會自動暫停。但 **Slack 與 Discord 不在那份清單裡**：它們平時
是主要的工作與對話場所，光憑 app 名稱分不出「正在分享畫面」與「正在聊天」，
把它們列進去等於讓整個工作日最重要的對話永遠不被記得。

這是刻意的取捨，而且代價**由你的同事承擔**。如果你常在 Slack 開螢幕分享，
請自己把它加進 blocklist。

### 忘掉一段之後，她說不出「這裡以前有東西」

刪掉一整顆資料庫的內容她認得出來（那一個位元，見上一節），刪掉**其中一段**
就認不出來了。你把上禮拜二忘掉，然後問她那天的事——她會說「她記的每一段裡
都沒有這個字」，而那句話講的是剩下來的那些段。它不會告訴你那裡有一個洞。

這是刻意不做的。要做得到就得留一份「你刪掉了哪幾段時間」的清單，而**那份
清單本身比它保護的東西更敏感**：一份「他星期二下午兩點到四點有東西不想留」
的紀錄，正好是那次刪除想消滅的推論。她記不得那個洞，是那個洞唯一真正被
補起來的方式。

代價你要知道：她的「沒有」永遠只是**現在這顆資料庫裡沒有**。

### 排除是規則式的，不是語意式的

沒有寫進 blocklist 的敏感網站會被完整記錄。預設清單只涵蓋常見的台灣銀行
與幾個登入頁。她不會「看出來」某個畫面很敏感。

### 前景規則不會遮住背景視窗

Windows GDI 擷取的是**前景視窗所在的整個螢幕**，不是只截那一個視窗。app、標題、
網址與敏感欄規則判斷的是當下前景；同一個螢幕上仍看得見的背景密碼管理員、私訊或
其他敏感視窗，可能一起出現在 frame 裡。前景規則不承諾保護這些背景 pixels；需要
這個邊界時，請先暫停、把敏感視窗最小化／移出該螢幕，或把目前前景整段排除。

### 我們讀到的網址是位址列上那串字

Windows 上唯一讀得到網址的地方是瀏覽器的位址列，而 Chromium 顯示的是
**給人看的縮寫版**：`https://www.example.com/a` 會顯示成 `example.com/a`。
所以規則要寫成子字串（預設值都是），寫成 `https://*bank*` 的規則永遠不會
命中。`sister doctor` 會把這類「寫了也不會命中」的規則挑出來給你看。

你在位址列打字的時候我們**不讀**——那時候上面是「搜尋或輸入網址」這種
散文，把它存進紀錄只會讓排除規則去比對一堆廢話。

`doctor` 現在也會真的對你當下的前景視窗問一次網址並印出來。
一個 ✓ 必須是某件事真的做成了，不是某個元件建得起來。

無人值守 URL 的「她看過這個 host」只信 exact session identity
`windows/windows-gdi-uia-focused-url-v2`。alpha.100–102 的
`windows/windows-gdi-uia-focused-url-v1` 歷史列仍可讀、可顯示，但當時的 global
focused element 與 stale URL cache 沒有證明 exact HWND／當拍 live value，所以不再
授權；升級後必須由 v2 recorder 實際看過一次。

### 瀏覽器剪貼簿沒有可證明的來源網址

剪貼簿能在同一次 Windows clipboard lock 裡讀到 owner／來源 app，但沒有一份可靠的
「這段文字是從哪個 URL、哪個視窗標題複製」證明。使用者可以在瀏覽器複製網銀內容，
立刻切到普通編輯器；拿下一拍的前景網址或標題補上去，會把來源配錯。alpha.103
不做這個猜測：只要設定了任何 URL 排除規則，來源 app 是瀏覽器、而事件沒有原始
網址證明，該筆剪貼簿內容就保守丟棄。這會造成一段記憶空洞，但不把「現在前景安全」
冒充「剛才複製來源安全」；URL／標題規則目前不能單獨保護快速 copy → switch 的來源。

### 敏感欄偵測會問每一個前景 app

前置檢查已確認焦點在敏感欄時，不讀剪貼簿內容，也不開始螢幕擷取；**問不出來時
也不放行**。UIA 的密碼屬性查詢對每個前景 app 都做；只有昂貴的瀏覽器位址列
樹遍歷仍限瀏覽器。UIA 暫時出錯、持續失效或前景視窗拿不到時，recorder 會在
內容來源前停下；持續失效另顯示能力缺口，不會把保護關掉。若焦點在檢查後、
`BitBlt` 期間才改變，pixels 仍可能短暫進工作 RAM；後驗不一致會在 dedup／OCR／
DB／PNG 前丟棄，詳見上方「預設就是安全的」。

代價也要明講：每個前景 app 現在都多一次可能卡住、而且無法取消的跨程序
UIA 往返。連續卡住三次後不再漏執行緒，但這一場會保持 fail closed，直到重開
recorder；能力報告會把原因顯示出來。

按下「顯示密碼」的那個畫面仍然會被記下來。

### Windows 鎖定與電源是取樣邊界

Release 1.0 的 Windows 最低版本是 Windows 10。recorder 只有在 WTS 同時回報
`WTSActive` 與 unlocked 時才允許讀內容；鎖定與電源是兩個正交狀態，wake 不會被
當成 unlock。狀態未知、互相矛盾或讀取失敗都 fail closed。

目前 WTS 是在一拍裡多次輪詢、比較**相鄰可觀測樣本**；它不是原生通知訂閱，也不能
承諾看見兩個樣本之間發生又恢復的每一次 lock／unlock。第一個樣本或 Unknown 空洞
後的第一個已知樣本只重建 baseline，不捏造一筆轉換或精確事件時刻。同步 Windows
source 目前也不產生 sleep／wake；那兩種 audit 要等真正的 power notification，不能
拿時鐘空洞猜。

來源已交出的同一次 observation 會先完整留在 recorder 行程的 pending 記憶體；其
轉換 audit 用一個 SQLite transaction 全寫或全不寫。DB 失敗時，下一拍會在任何新 poll
或內容讀取前重試，狀態、時間與 sequence watermark 也只在 transaction 成功後一起
推進。這是**行程存活期間的 best effort**：若程式在 source 已交出事件、transaction
尚未成功之間當掉，RAM 裡的 pending 會消失；它不是 crash-safe exactly-once event log。

### 讀字的是作業系統，不是我們

Windows 上用的是系統內建的 OCR（`Windows.Media.Ocr`）。它是本機的、離線的，
畫面不會因此離開這台機器——但處理那些像素的元件不是我們寫的，也不在我們的
稽核範圍內。

換來的是：不需要下載模型、不需要外掛 DLL、recorder 執行檔仍然只有一個檔案，
而且它的相依樹**沒有任何套件把 HTTP client 帶進來**。desktop 的 Persona GET 與
Azure TTS POST 是另外兩個明確、固定且由使用者設定／操作授權的能力，都不屬於 OCR。

代價是她**只讀得懂你裝了語言包的那些語言**。沒裝中文的話，她會安靜地退回
英文，然後把滿螢幕的中文讀成空白。`sister doctor` 會直接告訴你實際用的是
哪個語言，就是為了不讓這件事默默發生。

### 資料庫沒有加密

依賴 OS 的磁碟加密（BitLocker / LUKS / FileVault）。應用層加密會帶來金鑰管理
問題，而做錯的金鑰管理比不加密更糟。

### 保留期管不到她動過的手

`action-log.jsonl`（見上）不在 `sister prune` 的清理範圍裡。畫面 30 天、文字
365 天到期會自動消失，這個檔案不會——三年前你按下的那顆按鈕，那串網址今天
還在裡面。

沒有一起補，是因為要先答一個我們還沒答的問題：**這個檔案是記憶，還是稽核
紀錄？** 當它是記憶，就該跟文字一樣到期；當它是稽核紀錄，自動刪掉她做過什麼
正是稽核最不該做的事。alpha.69 把它當記憶處理（`sister forget` 帶得走、
`sister export` 帶得走），但沒有替「自動過期」挑一個我們還沒想清楚的答案。

實務上它只有在你按下那顆按鈕的時候才長一列。要它消失：跑一次涵蓋那段時間的
`sister forget`，或直接把這個檔案刪掉。

### 停用不等於刪除

暫停只是不再新增。已經記下來的還在，要另外刪。

怎麼停：全域熱鍵（預設 `Ctrl+Alt+P`）、字母人拖曳條上的 `⏸`、系統匣選單裡的
「暫停記錄」，或者

```bash
sister pause     # 正在跑的 record 下一個 tick 就停
sister resume
```

熱鍵是**不用先找到她**的那條路——「我現在不想被看」最常發生的時機，正是她收
在系統匣裡、而你手上正忙著別的事的時候。但全域熱鍵是先搶先贏的：別的常駐程式
可能早就佔走了同一組，而作業系統不會講，它只會讓你按了沒反應。所以設定頁上那
一行會**直說現在搶到了沒**，搶不到的時候用警告色寫出原因。一顆按了以為停了、
其實還在錄的暫停鍵，比沒有那顆鍵危險。（那也是為什麼系統匣那一顆留著：它不會
被別人搶走。）

按下熱鍵停下來的時候，字母人會自己出現在畫面上——一個看得到的灰色字母人，就是
那句「好，我停了」。恢復的方向不會叫她出來：停錯了是隱私問題，恢復錯了只是白按
一次，而每次切換都彈一個視窗，會讓收進系統匣這個動作失去意義。

五件講清楚的事：

1. **她不會自己醒來。** 沒有計時器、沒有「暫停 30 分鐘」。停了就一直停到有人
   明確解除為止。會自己恢復的暫停，等於在使用者不知道的時候恢復。
2. **不確定的時候算暫停。** 控制面由 data dir 裡的 `paused.flag`、`pause.state`
   和 `pause.lock` 組成；任一讀不到、格式互相矛盾、權限不足，或 lock 不是一般
   檔案，一律當成暫停。反過來那個版本（看不出來就繼續錄）才是會讓這一整段
   變成空話的失效模式。
3. **按鍵與寫入有唯一先後。** Desktop 在 `pause.lock` 的 exclusive lock 裡完成
   toggle；recorder 的最後一次快照持有 shared lock 到 PNG／DB transaction 結束。
   `pause.state` 的 generation 讓完整 pause→resume 即使發生在兩次探測之間也不會
   消失。也就是說，一次內容寫入完整排在 pause 前或 pause 後，不會卡在兩者中間。
4. **暫停期間的鍵盤滑鼠節奏也不會補記。** 進入暫停會先擋住新的 hook callback、
   等已在途 callback 離開，再清掉按鍵／滑鼠計數與時間、位置基線；解除時仍在
   disabled 狀態下推進並清空。暫停中發生的輸入不會在恢復後從累加器流進資料庫。
5. **暫停期間仍然會推剪貼簿的水位。** 聽起來反直覺，但方向是對的：不推的話，
   你暫停時複製的東西還躺在剪貼簿上，一解除就在下一秒被撈進資料庫——那種
   暫停只是延後洩漏。推水位不讀內容；只有 Windows 回報成功且非零的 sequence
   才算建立水位，回 0、讀不到或出錯都維持空洞並繼續 fail closed。

正常恢復只用 `sister --data-dir <同一個資料目錄> resume`（字母人的繼續鍵走同一
份 transaction），不要只刪 `paused.flag`：新版 `pause.state` 仍可能維持同一代
pause。若畫面說控制狀態無法檢查，先關閉 recorder、字母人及其他 AI-Sister 行程，
再修好資料目錄權限；`pause.lock` 必須是一般檔案，損毀的 `pause.state` 只能在所有
行程關閉後刪除。不要在任何行程仍執行時刪 `pause.lock`，否則不同 process 可能各自
鎖到不同檔案，原本的先後保證就不存在。

而且暫停**留得下紀錄**：進出各寫一筆 `capture_paused` / `capture_resumed`。
沒有那兩筆的話，資料裡的三小時空洞和「那三小時什麼都沒發生」事後分不出來。

`sister record` 開場、每分鐘的狀態行、以及 `sister doctor` 的「隱私」第一列
都會講她現在有沒有在看——因為暫停最危險的失效模式不是停不掉，是停了以後
看起來跟沒停一樣。

### 那「另外刪」怎麼刪

兩條路，刪的是同一批東西：

```bash
sister forget --last 2h          # 先看會刪掉什麼，一個位元組都不動
sister forget --last 2h --yes    # 真的刪
```

字母人那邊是時間軸上框一段，一樣是先看數字、再按第二下。

**預設只看不刪，是刻意和 `prune` 相反的。** `prune` 刪的是保留期已經答應要
刪的東西，所以它預設就做；`forget` 刪的是你本來想留、現在改變主意的東西，
搞錯的後果不一樣。沒有回收桶，也沒有復原——那段時間裡的字、事實、畫面檔、
事件、連你自己問過的話，全部一起走。

兩件容易踩到的事：

- **單位不可以省。** `--last 30` 會被拒絕，因為它看起來像 30 分鐘也一樣像
  30 天，而猜錯的那一邊是一次刪掉一個月的記憶。
- **她還在錄的話，刪掉的東西下一秒可能又被記一次**——你要忘掉的畫面通常還
  在螢幕上。所以刪完會提醒你先 `sister pause`。

### 帶著整份記憶走

```bash
sister export --to ~/sister-backup                 # 只有資料庫
sister export --to ~/sister-backup --with-frames   # 連畫面檔一起
```

匯出的目的地**就是一個資料目錄**——不是另一種格式，是同一種。所以「還原」
不需要任何工具，也不需要這個專案還活著：

```bash
sister --data-dir ~/sister-backup query 電話
```

這是 SPEC §11.8 那句「就算本專案死了，你的記憶還是你的」實際長什麼樣子。
一個要靠原廠程式才讀得回來的匯出檔，不算資料主權。

匯出用的是 SQLite 的 `VACUUM INTO`：一個交易裡把整份內容重寫出去，WAL 裡的
東西一起進去，而且**不需要停下正在錄的那個行程**。順帶把檔案壓實，所以匯出
檔通常比原檔小——那不是漏了東西。目的地已經有 `sister.db` 就拒絕，不覆蓋：
一次打錯路徑的匯出不該蓋掉上一份備份。

沒帶走的是 `consent.toml`（四張同意書的簽名）和 `config.toml`（設定）。Windows
Credential Manager 裡的 Azure key 也不在匯出裡。這些是這台機器的授權／設定，
不是你的記憶。

**這是把整份記憶搬出去的地方，所以講清楚：匯出檔沒有加密。**
它和原本那份一樣，靠的是這顆硬碟的加密（上面那條）。換句話說它放到哪裡，就
只被保護到哪裡——丟進雲端同步資料夾，等於把她記得的全部東西一次上傳。指令
本身也會在跑完的時候講一次這句話。

我們不去猜哪些路徑是雲端資料夾然後擋下來：各家的名字、各國的在地化、各版本的
預設路徑都不一樣，而猜錯的方向特別糟——沒攔到的那一次，你會因為「它沒說什麼」
而更放心。

### 把一段記憶做成 replay 語料

`sister export` 是完整備份；`sister replay export` 是另一件事。後者為了讓同一段
真實工作流能在不同版本上重跑，只匯出去敏後的 replay corpus：

```bash
sister replay export --last 24h
sister replay import <匯出時印出的 Draft 路徑> --dry-run
```

- **永遠不帶圖。** 沒有 PNG、沒有畫面位元組、沒有 `image_path`，也沒有來源資料
  目錄或圖檔路徑。它和 `sister export --with-frames` 不是兩種寫法，是相反的用途。
- **時間與文字先縮小爆炸半徑。** 時間軸改成相對值；螢幕文字、視窗標題、URL、
  剪貼簿文字和來源程式名稱先走現有的自動去敏規則。
- **自動去敏不是分享許可。** 程式不知道哪個詞是同事的名字、哪串是內部案號。
  所以匯出結果一定先是私有 **Draft**，JSON 的 `review` 是 `draft`；只有人工逐項
  看過、把它標成 `reviewed` 的 **Reviewed** corpus 才可分享。
- **Draft 照樣可以在本機重播。** `import --dry-run` 在記憶體資料庫驗證它，
  並從去敏後的 L0 重建搜尋索引與 L1；尚未 Reviewed 不影響本機重播，
  但仍不可分享。
- **Draft 也沒有加密。** 它是純文字 JSON，放到哪裡就只被那個磁碟的加密與
  目錄權限保護到哪裡。自動去敏降低了敏感度，沒有把它變成公開資料。

這兩個 replay 指令本身不會連網，也不會把資料送給模型商。人工審查後自己把
Reviewed 檔案複製到別處，和上面的完整備份一樣，是使用者明確做的檔案動作；
它不會悄悄多開一條網路路徑，也不改寫四張同意書的效力。

同一份 corpus 可以在本機跑評測：

```bash
sister replay evaluate <corpus> <questions> --k 5 --runs 3
```

真實 query log 可在同一次 corpus export 變成待標註題庫：

```bash
sister replay export --last 14d --to <corpus> --questions-to <questions>
```

這條路只讀本機資料庫、沒有網路；輸出採 0600、拒絕覆寫，並固定是 private
Draft。它保留使用者輸入的問題原話，**沒有自動去敏，也沒有額外加密**；當時的
回傳數、點擊與 ★ 只作標註提示，`expected` 是 `null`，不會自動猜成答案。
question set 綁著去敏後 corpus 的 fingerprint，但仍有自己的 Draft／Reviewed
狀態，不能借 corpus 的審查結果通關。人工標註與逐題審查前不要分享。

標註器仍是純本機檔案與 SQLite 檢索，沒有網路路徑：

```bash
sister replay questions annotate <corpus> <questions-draft> --to <labeled-draft>
sister replay questions review <corpus> <labeled-draft> --to <reviewed-questions> \
  --confirm-private-text-reviewed
```

第一條只有在使用者明確進入互動流程後才把題目原話印到終端；產品候選、當時 hits、
click 與 ★ 永遠只標成提示，不會自動產生 answer/no_answer。兩條都只建立另一個
新檔，不原地改寫或覆寫來源。第二條的確認旗標不是自動去敏：它表示執行者已逐題
檢查原問句、答案與 evidence；沒有確認、仍有未標題或 fingerprint 不合都不產生
Reviewed 檔案。題庫 Reviewed 也不會改掉 corpus 自己的 Draft 狀態。

目前的 `baseline_text` 和 `facts` 都只走本機 SQLite 檢索，沒有網路與模型路徑；
因此這兩個配置的模型呼叫 0、US$0/天是路徑事實。它不代表未來新增的配置也一定是
0。提醒誤報／漏報、斷句 F1、Reviewer 回查率、CPU、RAM、電池與磁碟目前沒有量測
來源，完整 JSON 裡是 `null`，不是 0。

題庫可能直接放著使用者問過的話，所以 question-set 和 corpus 各有自己的
`review: draft | reviewed`，一邊 Reviewed 不會替另一邊背書；完整 report 也會放題目、回傳值和 corpus
`event_index`，其中回傳值可能逐字重複去敏後的螢幕文字。沒有指定輸出選項時只印
摘要；`--json` 會把完整 report 印到 stdout，`--to <檔案>` 會寫到新檔且拒絕
覆寫。這些輸出沒有額外加密。

所以 Draft 的界線會跟著評測走：任一輸入是 Draft 都可以在本機 evaluate，但 CLI
會明說報告仍是私有資料；corpus、題庫與 report 都人工審查完成前不要分享。repo 內建的
`scenarios/recall-baseline.corpus.json` 與 `scenarios/recall-baseline.questions.json`
是純合成的 Reviewed smoke fixture，不含真實工作日資料，也不是一張把其他 Draft
自動變安全的通行證。

桌面的「評測指標…」預設隱藏，只有 `[shell] developer_mode = true` 且重開桌面殼
後才出現在系統匣。它不會自己掃資料夾；人從原生選檔器挑中一份 report 後，完整
JSON 會短暫進入本機 WebView，再經同一行程的 IPC 交給 Rust 嚴格解析。Rust 回給
頁面保存與渲染的 projection 只有 corpus／題庫狀態與數量、數值參數、各配置的
aggregate 指標和任一 QA 指標未通過的 1-based 題號。corpus／題庫名稱、fingerprint、
ranking、自由填寫的題目 id、逐題原問句與 returned values **全部不在 projection**。
這是顯示邊界，不是假裝 raw report 從未進過 renderer。

這條路的 CSP 和程式都沒有網路輸出，不上傳，也不呼叫模型；頁面不改原檔、不另存
副本。磁碟上的 report 原檔仍包含題目和檢索文字，也沒有額外加密。任一輸入是 Draft
時，指標頁會常駐 private Draft 警告；關掉開發者入口、關掉頁面或看到一份去文字
摘要，都不會把那份原檔變成 Reviewed。

---

## 我們怎麼證明，而不是只是宣稱

```bash
cargo test -p sister-capture --test privacy
./scripts/check-no-network.sh
```

這個測試跑一段踩滿地雷的腳本（密碼管理員、網銀、螢幕分享、剪貼簿秘密），
然後把**整個資料目錄當成位元組**掃過，確認不該存在的字串一個都不在。

它刻意不去查特定欄位——那樣只能證明我想到要檢查的地方是乾淨的。掃位元組
才能在未來多一個欄位、多一張表、多一個索引時仍然抓得到洩漏。

第二條檢查不是再宣稱整個 repo 沒有 HTTP client；它要逐棵證明 root workspace 在
預設 feature 下仍無 Persona download 或 Azure transport、recorder／core／capture／
brain／hands 沒有 client，只有 desktop 能經 `sister-assets/download` 抵達固定 Persona
GET、經 `sister-tts/azure` 抵達三個 fixed Azure POST。它同時繼續
拒絕我們自己的 Rust 原始碼直接開任意 TCP／UDP／Unix socket、renderer 的
`fetch`／WebSocket／遠端資源，以及任何把 CDN 或 Azure 放進 WebView CSP 的改動。Linux X11
preflight 使用 pinned `x11rb` dependency 連本機 X server 的 Unix socket；它會在連線前
拒絕 TCP／SSH `DISPLAY`，不是對外內容路徑，也不是這個 source-level socket gate
掃描得到的東西。branch CI 用本機 transport/cache 測試守住 0／1 request、固定
request metadata、失敗／取消與跨行程鎖；不碰 CDN。tag CI 才另外從 public CDN 取
完整 manifest 重算 authority，並在原生 Windows 上走一次固定 GET、驗證、安裝與撤回。
按下前 0 request、四位 request 完全相同及系統 proxy 對照仍列在真 Windows 出貨清單，
不拿一次成功下載冒充 packet trace。

開發過程中，這類驗證實際抓到過三個「規則讀起來正確但什麼都沒比對到」的
bug。詳見 [THREAT_MODEL.md](THREAT_MODEL.md#最危險的失效模式安靜地不生效)。

---

## 回報

<https://github.com/teddashh/AI-Sister/issues>

最有價值的回報是：**「這條規則在我的機器上沒有生效。」**
