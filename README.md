# AI-Sister

> 一個站在桌面角落的姊妹。她一直都在，看得見你的一天，記得住細節，
> 95% 的時間安靜，該說話的時候才說話——說的每一句都能點開證據。
>
> An open-source, local-first desktop companion: a filing cabinet that never
> forgets, an event-driven brain that can admit it's wrong, and a desktop sister
> who knows when to stay quiet. Screen pixels never leave your machine; after
> explicit opt-in, OCR text can be handed to the local CLI you configured. A
> separate, default-off Azure TTS option can send only each newly completed
> answer body after its own consent; manual replay sends it again. Local speech remains the default.

**Status: Windows alpha 已經從記錄、L2/L3、Gatekeeper 接到 Phase 6 的手；Persona
的四姊妹 fixed voice 與 fixed CDN pack 也已接通，alpha.104 的發版 gate 在真
Windows 走完下載、驗證、原子安裝與精準移除。真人設定頁／播放／撤回的正式 artifact
實測與 packet trace 仍未回收。alpha.106 已接上 Windows current-user 離線安裝包與
single-instance，並在真 Windows CI 走完 fresh install、行程交棒、已存活行程拒絕、
同版 reinstall 與移除。alpha.107 再接上預設關閉的 Windows 登入啟動，以及只管理
desktop 自己啟動之 recorder 的 bounded supervisor；純 policy／狀態機、renderer fixture
與 Windows registry test-subkey 都有自動測試，但正式 alpha.107 安裝檔的真登入、暫停、
重試與移除流程仍待 Windows 人工實測，不能寫成已通過。alpha.108 移除單字母角色，
直接內建四姊妹＋13 位閨密的 17 張 current 角色圖，並接上 trusted-click、localService-only
的本機中文語音與答案朗讀；這一段仍待正式 Windows artifact 人工聽驗。alpha.109 再加入
預設關閉、沒有自動 fallback 的 Azure 繁中 TTS：第四張獨立同意、Windows Credential
Manager 金鑰與三個固定區域的 native POST 已接線，正式 Windows artifact 的語音、封包
與取消時序仍待人工勾驗。alpha.110 把已明確啟用且重簽第四張的 Azure 改為每份最新
新答案完成後自動朗讀一次，仍可隨時關閉；alpha.109 的 click-only 第四張不會被沿用。
同版將 5.2 GB Reel 候選縮成 17 套 workplace 分層 rig：402 張 PNG、
35,140,885 bytes，只解碼目前角色，並保留原 WebP 當解碼失敗退路。alpha.111 tag
原定加入圖像選角與跨版安裝驗證，但 tag-only Windows gate 依賴 ambient native stdout
編碼，中文標題被錯誤解碼後失敗；Release job 因此跳過，沒有公開 release 或下載資產。
alpha.112 把 native CLI stdout 明確固定用 UTF-8 解碼，並把 17 張 bundled WebP
做成設定頁的圖像選角卡；點選先留在本機預覽，儲存成功才嘗試即時通知主視窗換人，
通知失敗會明講重開 desktop 後生效。同版 Windows CI 從最後一個有公開安裝檔的
alpha.110 真正安裝後，再由 alpha.112 installer 原地升級；Run absent／enabled 兩條路、
舊 DB 查詢、四張同意與 synthetic 證據檔關聯都要讀回來，Persona／Azure 非密設定則要
原 bytes 保留並仍可由新版解析。這不是正式 WebView 點圖或真 OCR 的人工證據。
alpha.113 先讓本版產生的 Setup／uninstaller 覆寫 Tauri stock running-app macro；舊 image
scanner 回報 `sister-desktop.exe`／`sister.exe` 命中時只拒絕，不提供或執行強制關閉。
alpha.114 把早已出貨的 17 套完整 1280×1280 透明 rig 從舊的上方
640×640 crop 還原成全身，移除頭像底色、邊框、圓角與厚投影，改成跟著人物
alpha 的浮空陰影。原本 17 張不透明半身 WebP 也換成由同一批 workplace canvas
縮出的 640×640 透明全身預覽，冷啟動、壞圖退路與設定選角都不會倒回相框半身；
它們合計 1,031,124 bytes。402 張 PNG 仍是 35,140,885 bytes，5.2 GB 裡其他服裝
和 reaction 仍不出貨。應用程式圖示也改從同一張 ChatGPT 全身預覽裁出；exact 五檔
另以 manifest／NOTICE 固定來源、recipe、464,143 bytes 與授權範圍。
alpha.115 再改用 pinned tauri-bundler 2.9.4 custom NSIS template，修掉真人升級時
PageLeave child 把自己當 direct uninstall、撞上 parent lifecycle mutex 的路徑。Setup 現在
絕不巢狀執行已安裝的 NSIS uninstaller；同版 repair 或舊版升新版會在 mutex 下綁定
current-user 產品鍵所記的 exact root 原地覆蓋，並與 uninstall key 交叉核對。GUI、passive `/P` 與 silent `/S`
都在 WebView2、payload 或安裝登錄 mutation 前重驗 `DisplayVersion`、root 與 quoted
`UninstallString`；downgrade 要關閉 Setup，再從 Windows「已安裝的應用程式」分開移除。
direct uninstaller 在 `PREUNINSTALL` 後也重驗 exact version／root／string，避免停在確認頁的
舊 uninstaller 隨後刪掉已覆蓋的新版。帶協定的 desktop／CLI 與 installer 仍用 product event
及 mutex／event／mutex handshake 交接。
原生 Windows CI 除既有 `/S` admission lanes，另用真正進入 `PageReinstall` 的 `/P`、
alternate `/D` 與 `PING.EXE` child witness 驗證沒有巢狀執行，再用 `/S` 驗 downgrade 在
mutation 前退出；這仍不是滑鼠真人互動證據。
alpha.116 另修正桌面問「她知道了什麼」時把「知道」當全文關鍵字、撈到設定頁 OCR 的
答非所問：這組窄問法現在直接讀本機 current L2，最多列三張來自最近四張候選、資料庫
仍標有畫面出處的理解卡；每張都是可修正假設，不是確定事實。只有原始紀錄、目前沒有
記憶內容與最近候選沒有畫面出處會分開說，不再用 OCR 墊答案。帶主題的
「妳知道客服電話嗎」仍走一般檢索。這次提問不叫 CLI，也不增加網路能力；Azure 已明確
啟用時仍只照既有第四張同意送答案正文。
舊 scanner 沒有 Unknown，process 列舉或 token／SID 查詢失敗會和「未找到」合併；舊 binary
也不持有 event。已經出貨或複製到 temp 的 alpha.114 uninstaller 不能由新版 retroactively
改寫；alpha.115 的保證是自己的 Setup 不再啟動它。Windows loader 在 Rust
`main` 前映射 executable，最後一次 scan 到 NSIS `File` 之間仍有窄窗，所以不能把這版
寫成跨版本完整原子 lifecycle。
code signing、跨層 master stop 與 installer late-start 窄窗仍未完成，
所以現在還不是 Release 1.0。** Windows 10+
會是 1.0 的正式支援平台；macOS 與 Linux X11 先走 Preview。
可以從 [Releases](https://github.com/teddashh/AI-Sister/releases) 下載目前的 alpha。

macOS 現在有一條 **feature-gated 原生診斷，不是產品擷取後端或 Preview**：alpha.105
的 `macos-15` Apple Silicon job 由 LaunchServices 啟動 ad-hoc signed、hardened `.app`，
再以 live PPID 與 `proc_pidpath` 對回 `sister-desktop` → bundle 內 exact `sister`。那場
runner shell 的 CoreGraphics preflight 是 true，exact child 則回
`not_granted_or_undetermined`／`not_attempted`，因此沒有呼叫 ScreenCaptureKit capture。
這證明的是原生 app-tree 拓撲與「未授權就停」的 diagnostic fail-closed 路徑，**沒有
執行 pixel path，也不是 macOS Preview**。七天 Actions artifact 雖可下載，名稱明標
`NOT-PREVIEW`，不是 release asset；production `record`、Vision／AX、產品
consent/TCC lifecycle 與完整 S1 都還沒接。

她開始看或把答案文字交給 Azure 之前有**四張各自獨立、隨時撤得掉的同意書**，條文和效力就是：

- `local-recording`：「我同意在我的硬碟上記錄我的螢幕。」沒有這一張，`sister record`
  不會開始錄；錄到一半撤回，正在跑的 record 每 5 秒重讀同意書，最多再錄 5 秒加一拍；
  `capture.min_interval_ms` 超過 5 秒時，主要會等那一拍。沒簽時拒絕啟動、回非零；
  簽了才准在本機記錄。
- `cloud-reading`：「我同意把螢幕上的文字原文（OCR 抽出來的字，永不含畫面）交給我在設定裡指定的本機 CLI，由那支程式去做解讀。裡面有什麼就送什麼，不會先遮掉。」
  沒有這一張，解釋層一次都不會呼叫那支 CLI。畫面永不離開這台機器；出去的是 OCR 抽出來的字，**原文，不遮**——記憶要能跨段把同一個人認出來，代號做不到。要先看清楚會送什麼：`sister interpret --dry-run` 會把那段字整份印出來，一個字都不送。
- `frame-storage`：「我同意保留變化幀的截圖，而不是只留上面的字。」沒簽不擋錄，
  她會當場說明降級，只記字、一張截圖都不寫；簽了才准依設定保留變化幀。
- `azure-tts`：「我同意在設定裡開啟 Azure 新答案自動朗讀時，每份新答案完成後不再逐次詢問，就把該答案正文原文交給我在設定裡選擇區域的 Microsoft Azure 語音服務並自動播放。正文可能含姓名、電話與金額，不會先遮罩；
  不會送出截圖、來源連結、memory id、整份資料庫或其他文字。」沒有這一張，她一次
  都不會呼叫 Azure 語音服務；本機朗讀不受影響。

例如要准她在本機記字並保留截圖，要寫
`sister consent --grant local-recording --grant frame-storage`。三個介面——
`sister consent` 和使用者第一次開桌面姊妹時那一頁都從 core 取同一份條文與未簽後果；
`sister doctor` 讀同一個檔案，另外報告目前是否簽署及會發生什麼事。
前三張共同條文改版會讓前三張舊簽名失效；第四張另有自己的條文版本。檔案讀不到、
損壞或版本不符一律 fail closed。alpha.109 以前沒有 Azure 欄位的舊檔保留前三張、
第四張未簽；alpha.109 已簽的逐次點擊條文在 alpha.110 也會顯示為過期，只需重簽
第四張，不能從任何舊同意推定自動朗讀已獲授權。CLI
指定 `--data-dir` 時，同意書跟著那個資料夾走；桌面姊妹只讀預設資料夾，兩邊不一定是同一份。

**alpha.46 已在 Ted 的真 Windows、1920×1080、正常切換 Better Agent workspace
寫程式的 60 秒裡量到 CPU 平均 44.0%、RAM 峰值 73.7MB。** Ted 在 2026-08-23
選擇保留現在的觀察密度；CPU 仍然每場照實量，但不再是 Phase 0 的 blocker。
`<3%` 留作長期產品目標，不拿來否定這個已接受的 Phase 0 基準。RAM 已通過
`<400MB`。磁碟仍未結案，但 alpha.47 已經把缺口拆開：那 60 秒寫了 2.7MB
畫面，摘要所稱的「其他」2.9MB 裡有 2.8MB 是可重用的 SQLite WAL；SQLite
邏輯配置只增加 156KB。摘要當時印的 4.3GB/天（其中「其他」4.1GB/天）把
WAL 工作檔當成每天永久長大，所以**不拿來作 Phase 0 判決**；main 的計帳已改成
只拿 SQLite 邏輯配置與畫面外推。容量仍高於長期目標，但 Ted 已決定先完成產品
功能與體驗，再回來優化容量；它照實公開，不再擋目前的功能 milestone。
細節與下一步見 [`docs/WINDOWS-CHECKLIST.md`](docs/WINDOWS-CHECKLIST.md)。

最底下那一層會看畫面、讀字、抓出電話與金額之類的事實、存進 SQLite、搜得回來；
**這一層一個模型呼叫都沒有**，全部是程式在抄寫。L2/L3 的 Interpreter、Reviewer、
承諾表與 Gatekeeper，以及只做白名單動作的 hands 也已經接上；會把 OCR 原文交給模型的
部分仍須第二張同意書與使用者自己設定好的 CLI，沒有就維持純本機檢索。

從 alpha.106 起，Windows release contract 有一個預設入口與兩個免安裝／診斷
備用檔：

| | 做什麼 |
|---|---|
| `AI-Sister-Setup.exe` | **一般使用者優先下載這個。** current-user NSIS 會把 exact `sister.exe` sidecar 和 WebView2 offline installer 一起帶進去；設計為安裝時不需連網，代價是安裝包會顯著變大，實際 bytes 見該版 Release asset。正式 artifact 的斷網安裝仍待實測 |
| `sister.exe` | 錄製、搜尋、重播評測與資料管理；也包含 `interpret`／`review`／`watch`、Gatekeeper 的 `speak`，以及 `do`／`hands`／`url-policy` 的行動與稽核入口 |
| `sister-desktop.exe` | 桌面角落的姊妹：錄製狀態、搜尋與可點開的出處、時間軸與刪除；也顯示目前推測、Gatekeeper 與 hands 建議，並主動詢問無人值守網址政策 |

installer 沒有內建自動更新。升級時由使用者下載新 `AI-Sister-Setup.exe`，先自行結束
desktop 並停止 recorder，再原地安裝。alpha.115 使用 pinned tauri-bundler 2.9.4 custom
NSIS template；Setup 絕不巢狀執行已安裝的 NSIS uninstaller。同版 repair 或舊版升新版時，
它在 `.onInit` 取得 lifecycle mutex，綁定 current-user 產品鍵所記的 exact root，
並原地覆蓋。GUI、passive `/P` 與 silent `/S` 都會在 WebView2、程式檔或安裝登錄前重驗
`DisplayVersion`、root 與 quoted `UninstallString`；不相符就先停。若這份 Setup 比已安裝
版本舊，請關閉 Setup，再從 Windows「已安裝的應用程式」分開移除目前版本。

direct uninstaller 在確認頁之後、任何移除動作之前的 `PREUNINSTALL` 取得 mutex，接著重驗
exact version、root 與 uninstall string；使用者確認後若已有新版覆蓋，它會拒絕，不讓 stale
confirmation 刪除新版。本版產生的 Setup／uninstaller 仍覆寫 Tauri stock running-app macro；
current-user scanner **回報命中時**只拒絕，不提供或執行 kill。拒絕會先釋放 mutex，取消則由
process teardown 關閉。無關的第二份 Setup 不能同時進入 lifecycle。

alpha.113-aware desktop／CLI 在產品 log、DB、WebView、記憶或設定之前執行
mutex → product event → mutex 握手，並把 event 持有到行程結束；installer 取得 mutex 後
只有明確量到 event 不存在才繼續。舊 binary 沒有 event，只能靠沒有 Unknown 的 legacy
scanner best-effort 掃描；列舉、token 或 SID 查詢失敗會被它當成未找到。已經出貨或複製到
temp 的 alpha.114 uninstaller 不能被 alpha.115 retroactively 修改；新版 Setup 只保證不再
啟動它。
Windows loader 又在 Rust `main` 前映射 executable，最後一次 legacy scan 到 NSIS `File`
仍有窄窗；所以 installer late-start 與跨版本完整原子 lifecycle 仍未完成。現在也還沒有
code signing，Windows 可能顯示未簽章警告。

## 跑起來

**Windows 10 以上**——從
[Releases](https://github.com/teddashh/AI-Sister/releases) 優先下載
`AI-Sister-Setup.exe`；只想跑 CLI 或診斷 installer 問題時，仍可下載兩個免安裝
執行檔並放在同一個資料夾（桌面姊妹是去隔壁找 `sister.exe` 的）。她要先拿到第一張
同意書才會動：

```
sister consent --grant local-recording
sister doctor
sister record --duration 60
```

安裝版可直接開 AI-Sister；免安裝用法則開 `sister-desktop.exe`。然後問她剛剛那一分鐘
發生了什麼、在桌面問「她知道了什麼」看已整理的本機 L2，或者直接
`sister query 電話`。
`doctor` 排在錄之前是有意的：它會當場示範這台機器**現在**讀不讀得到網址、
OCR 有沒有裝、哪幾條排除規則其實不生效——比錄完 60 秒才發現什麼都沒進去好。

Windows 上第二次開 `sister-desktop.exe` 只會請原本的桌面姊妹顯示並取得焦點；它不會
再開一份，也不會因此停止或重啟 recorder。顯示／焦點本身仍列在下方真機人工確認。

alpha.107 的安裝版設定頁另有一個**立即生效、預設關閉**的「登入 Windows 後啟動」
開關。真相只在目前使用者的
`HKCU\Software\Microsoft\Windows\CurrentVersion\Run`：值不存在是 `disabled`，只有
`"<目前安裝的 sister-desktop.exe>" --ai-sister-login` 逐字相同才是 `enabled`；舊路徑、
少參數或非字串是 `mismatch`，讀不到是 `unreadable`，portable／非 exact 安裝副本是
`unsupported`。後三種都不會冒充關閉。只有 uninstaller metadata 的
`InstallLocation` 精確對應目前 exe 的 parent 時才准修改；portable 只顯示狀態，不會
碰安裝版的值。這裡只保證 Run value 已登錄，Windows 的 `StartupApproved` 仍可由
「啟動應用程式」另外停用。關掉開關只影響之後的登入，不會停止這一輪 recorder。

以 `--ai-sister-login` 進來時只留在系統匣，不顯示或聚焦桌面姊妹，也不彈同意書；同一個
desktop worker 只承接第一個 login intent，之後 delayed／secondary duplicate 都忽略，
也不 reveal。第一張本機記錄同意有效時，它會要求啟動 desktop 自己擁有的 recorder；
沒簽、讀不到或版本失效則不啟動。既有 pause 永遠保留，登入與重試都不能替使用者解除；
它們也不能清掉先前的 Stop 或 `consent-revoked` latch。

Login 的 transient preflight 每 500ms 從頭重查一次，最長五分鐘，而且不消耗
watchdog 的 1／5／30 秒 failure budget。只有 typed `desktop-quit` 可在這個期限內等上一輪
lease／heartbeat 退場，再走完整 barrier；marker 是 Absent 卻看見任何 owner 證據時，
那就是 External，哪怕它稍後停止也不由這份 login intent takeover／restart。一般 external
最後一拍變舊或暫時讀不到仍不算離場，只有 `stopped` 墓碑能完成離場證明；
`desktop-quit` 是上一輪正常退出留下的窄例外，不是把 mere staleness 普遍當空房。
五分鐘 deadline 在每次重試前與最後 commit 前都硬檢查，不能睡過期限再 late spawn。
真人 Stop／Quit／Explicit Start、`requested`／`consent-revoked`、無效或無法安全判讀的
consent／control 都會取消這份 Login intent，且失敗的 barrier 不清 marker。

正常退出另留較弱的 `desktop-quit`；下一次全新的 opt-in Windows login 只有在上述 bounded
handoff 與完整 consent／stop／lease／heartbeat barrier 後才可清這一種，讓正常 shutdown
不會把登入啟動永久關死。
撤回本機記錄同意時，程式會在同一個 exclusive `consent.lock` transaction 裡、寫
`consent.toml` **之前**先發布獨立的 `consent-revoke.barrier`。它不是可由 Start 清掉的
stop marker；即使同意檔 save 失敗，舊同意仍有效，這道 barrier 也會繼續擋住開始。
只有成功 commit 的本機記錄 regrant 能拿 generation ticket 清掉對應的舊 barrier；清前
若沒有其他 stop，會先留下 `consent-revoked` stop latch，而且既有 `requested`／
`desktop-quit` 不動，所以重簽只是重新授權，不會偷偷重啟或吃掉真人 Stop。

所有 recorder start 的鎖順序固定是 shared `consent.lock` → `stop.lock` →
`recording.lock`／舊 heartbeat barrier。CLI recorder 的同一份 shared guard 活過 explicit
clear 與第一拍；desktop parent 的 guard 活過 preflight／clear 到 `Command::spawn` 回來便
立刻放掉，不等待 child heartbeat，child 則自己 nonblocking 重拿一份 guard 並持到第一拍。
撤回 writer 先拿到 exclusive consent lock 時，這次 start 不清 marker、不 spawn、也不寫
heartbeat；start 先拿到 shared guard 時則明確排在撤回前，晚到的撤回仍由 barrier 與
recorder 檢查收工，不讓兩邊用鎖外舊 snapshot 無序穿過。

desktop-owned recorder 若非正常失敗，連續前三次分別等 1 秒、5 秒、30 秒再試，第四次
失敗後放棄。十分鐘健康帳只由**相對上一個已觀察樣本真的更新、仍新鮮的 `Recording`
heartbeat** 推進；反覆讀同一拍不算新證據，missing／stalled／unreadable／
`Thinking` 一旦被觀察到就把連續區間歸零。exit 0、手動停止、撤回第一張同意、
desktop quit 都取消重試。真人一按 Stop，worker 會在任何可能失敗的 durable write 前先取消
Login 與 watchdog 自動工作；寫不進去時會照實說現有 recorder 可能仍在跑，但之後也不會
自動復活，只有下一次真人 Explicit Start **成功 commit** 才解除這道記憶體 latch，失敗的
Start 不算。quit 也會先寫 durable stop；寫不進去時雖然仍立即取消
in-memory retry，desktop 卻不會退出，而是顯示錯誤等使用者修復，不留一個無法證明
會停的 recorder 在背後。占用或狀態未知時 fail closed，不開第二個；
外部自行啟動的 recorder 不會被接管，desktop 自己當掉也沒有 self-relaunch 服務。
而且所有 CLI／desktop recorder 在第一拍 heartbeat 前都必須 nonblocking 拿到
data-dir 空檔 `recording.lock` 的 OS whole-file exclusive lock，整場持有；同時開只會
有一個成功。process crash／handle drop 會讓 OS 釋放，但空檔本身可持久存在，
所以不能拿「看到檔案」當 occupied；symlink 或 non-regular path 則 fail closed。
真人從 desktop 開始時，parent 會先持 shared consent start guard，再在同一個 stop
transaction 內暫時取得這把 lease、檢查舊 heartbeat，commit 後才清舊 latch；child 仍以
supervised 模式重新取得正式 lease，
並在第一拍前重讀 consent／stop，所以稍後到達的 Stop／Quit 不會被 child 擦掉。
exact command／五態、watchdog、recorder lease 與 consent transaction 都有自動測試；
正式 alpha.107 安裝檔在真 Windows 登入與故障時序上的人工勾驗仍列在
[`docs/WINDOWS-CHECKLIST.md`](docs/WINDOWS-CHECKLIST.md)。

**目前可以在設定裡直接選四姊妹與 13 位閨密：ChatGPT、Claude、Gemini、Grok、
DeepSeek、Qwen、Mistral、Llama、Sakana、Perplexity、GLM、Kimi、Hunyuan、MiniMax、
Nemotron、Cohere、MiMo。** 每人的 workplace 分層 rig 都隨桌面程式離線提供；
17 rigs 共 402 張 PNG、35,140,885 bytes。畫面只解碼目前角色的 21–26 層，
全部成功後才切過去；完整 1280 canvas 以透明全身浮空呈現，不再裁成
相框裡的上半身。任一圖層失敗就繼續顯示同一位的透明全身 bundled WebP。
預設是 ChatGPT；沒有 Neutral，也沒有 S/T/C/G/X 字母 fallback。選材、大小、
SHA-256 與授權邊界在 `apps/desktop/ui/persona-reels/manifest.json` 與 `NOTICE.md`；
17 張透明全身 WebP 的同等資料在 `apps/desktop/ui/personas/`。圖像不納入
Apache-2.0 程式碼授權；由 ChatGPT preview 衍生的五個應用程式圖示另固定在
`apps/desktop/src-tauri/icons/{manifest.json,NOTICE.md}`，不借前一份 grant 擴張範圍。
可重現的 selector 與排除範圍見 [Persona Reel 選材](docs/PERSONA-REELS.md)。

只有你真的按下角色（或在原生按鈕上用 Enter／Space）才會顯示下一句，不叫模型、
不改答案，也不因開場、輪詢、錄製或記憶事件自己開口。聲音預設關閉；打開後，四姊妹
有已驗證固定錄音時優先播放，否則只接受 WebView 明確回報 `localService = true` 的繁中／
中文系統 voice。找不到就保持安靜，不會偷換 remote voice。答案清單底下的「用本機聲音
朗讀」也只在使用者親手按下後讀畫面文字，不經 Rust IPC 或遠端 TTS。

alpha.110 的 Azure 繁中朗讀仍然**可選而且預設關閉**；它不是本機 voice 的自動
fallback，本機找不到聲音時仍靜音，Azure 失敗時也不自動改走另一條。啟用 Azure、
選定 `eastasia`、`southeastasia` 或 `japaneast`、把 subscription key 存進 Windows
Credential Manager 的固定 target `ted-h/AI-Sister/AzureSpeech/v1`、簽第四張
`azure-tts` 後，每份最新新答案完成時會由 native Rust 對所選區域的固定
`https://<region>.tts.speech.microsoft.com/cognitiveservices/v1` 發出一個 HTTPS
`POST` 並自動播放一次；答案下方可停止或 trusted 手動重播，重播會再 POST。關掉
設定就不再開始新的自動朗讀。開機、設定／狀態重讀、舊答案重畫、demo、角色點擊、
錄製或記憶事件都不會補送答案。WebView 仍只走 IPC，不能傳 URL，也沒有 redirect、proxy 或 retry。

每次 POST 只含**當前答案正文原文**，可能含姓名、電話與金額且不先遮罩；不含截圖、
來源連結／出處 chip、memory id、資料庫或任何其他文字。subscription key 不進
`config.toml`、log、資料庫或 export；你輸入時它會短暫存在 password 欄位與 Tauri IPC，
保存後頁面立即清空，native 回條只帶 Present／Missing／Unreadable／Unsupported 狀態，
不會把 secret 讀回 renderer。程式不保存文字／MP3 cache；每份新答案與每次手動重播都可能是新請求。按停止
或切到別題會立刻讓這一代音訊失效，晚回來的 MP3 不播放、不快取；但已取得送出權的 blocking
POST **無法中途撤回**，仍可能跑到 45 秒 timeout，而且 Azure 可能已把它計入用量。
若 transport 已先取得送出權，原生取消、關閉開關、改區域／聲音、存刪 key 或修改第四張
同意可能等它結束或逾時才回覆；畫面上的停播仍先立即發生。任一操作回覆成功後，舊的
設定／同意快照不可能才開始另一個 POST。
Microsoft 目前公開列出的 Azure Speech F0 neural TTS 額度是每月 0.5 million characters；
是否可用、計費與額度仍以你的 Azure 帳號、resource、方案及 Microsoft 當下規則為準，
AI-Sister 不提供或保證免費額度。見 [Azure Speech 定價](https://azure.microsoft.com/en-us/pricing/details/speech/)。

四姊妹的八句預錄固定語音仍由舊 optional pack 提供。下載按鈕前會列出
`cdn.ted-h.com`、**73,261,088 bytes**，以及 CDN 會看見來源 IP、時間、TLS、固定
path／headers；只有你看完後明確按下，desktop 才可發至多一個 fixed GET。請求不含
目前選誰、角色狀態、OCR、畫面、問題、答案或記憶，也不 redirect／proxy／retry，
不帶 cookie／credentials／authorization、referrer、query 或 body。WebView 本身仍只
走 Tauri IPC，CSP 沒有開 CDN。

目前 desktop 不採用 pack 內的舊立繪，只選八段語音。pack 裡另有四段 `active` 聲音會說
「今天滿有活力」，但點角色這件事沒有量到這個事實，所以 app 不選、不存也不播放；
不拿一段有條件的聲音湊成第三句。

程式先用 compact embedded authority 驗 73,261,088-byte ZIP 的整包 SHA-256
`7d98e0d18c470f82818e8ada67208847c3cf4ff5c10cb5f99f9215191e981f30`、946 entries，
再逐檔驗真正會使用的四位角色 projection；不是把約 2.1 MB 的完整 11 人 manifest
塞進執行檔。全部成功才原子啟用額外錄音；任何失敗都保留所選的 bundled 角色圖，
不會背景重抓。cache 在預設資料目錄的 `persona-assets-v1/`，memory export、
`forget`、`prune` 不碰它；Persona 撤回才精準清除該 release。

**alpha.100 多問一個只關於無人值守網址的問題。** 設定尚未回答時，第一次開
`sister-desktop.exe` 會直接問：「有時候我讀到的東西裡會有一個網址。你不在的時候，
要我自己按下去嗎？」CLI 的同一個入口是 `sister url-policy`。兩個答案都合法：

- `only-on-my-press`：「等我在。」網址一律等你當場按，standing grant 帶不動。
- `when-you-can-name-the-origin`：「可以，但你要說得出它從哪來。」只有她在保留中的
  alpha.103 v2 真 Windows 錄製裡看過同一個 host，standing grant 才帶得動；只容許
  一層 `www.` 的差異，不把子網域當成同一站。alpha.100–102 的 v1 錄製仍可查、
  但不再是無人值守 URL 的來源票。

沒回答不是拒絕或預設選項；在回答前，無人值守網址會 fail-closed。這個設定只管
帶 `--use-grant --unattended` 的 `sister do`：你當場按的網址在兩個答案下都照常執行，
`--dry-run` 也不走這道閘門。v1／更舊錄製、import 與 replay 都不會被升格成來源票，
升級後要讓 v2 recorder 實際看過該站一次。**同 host 只是一筆來源紀錄，不證明網址
安全或由你主動開啟，也不證明 path、redirect 或站內內容可信。**

**從原始碼**——Linux/macOS 也跑得起來，只是還沒有產品擷取後端，所以第一次不能叫她
錄；改用 repo 裡那份腳本重播一遍（CI 每次 push 走的是同一條路）：

```
git clone https://github.com/teddashh/AI-Sister.git
cd AI-Sister
cargo build --release -p sister-cli
./target/release/sister --data-dir ./data replay scenarios/bill-lookup.json
./target/release/sister --data-dir ./data query 電話
```

最後那行會給你這個：

```
🔍 「電話」 2 筆答案、0 筆原文，0.3 ms

我最後看到的是：
  ★ +886800080123  「0800-080-123」
    ↳ phone · 2026-08-19 04:37:39 (剛剛) · chrome.exe · 中華電信 客戶服務 - 帳單查詢 · frame #1
  ★ +886912345678  「0912-345-678」
    ↳ phone · 2026-08-19 04:36:47 (1 分鐘前) · chrome.exe · 中華電信 客戶服務 - 帳單查詢 · frame #1
```

**clone 到第一個答案實測 33 秒**（乾淨的 `CARGO_HOME`：抓 108 MB 相依 + build 32 秒。
16 核開發機，GitHub 的 runner 大約是這裡的 2.1 倍）。需要 Rust 1.85 以上——這份
程式是 edition 2024；CI 會用 Rust 1.85.0 對根目錄與 desktop 兩個 workspace 做
`cargo check`。整條路上沒有 `sudo`、沒有服務、沒有帳號。

這一步**一個像素都沒讀你的螢幕**，所以它不用簽同意書：`replay` 讀的是 repo 裡那份
JSON 腳本，沒有任何東西可以同意。要看她在你自己的機器上會做什麼，得走上面那條
Windows 的路。

`sister replay scenarios/bill-lookup.json` 指令不變；scenario JSON 現在必須明寫
`privacy_context` 與 `system_state`（例如 `"clear"`／`"active"`）。這兩欄是測試語料
對安全前提的聲明，缺欄就拒絕執行，不會默認成「已知安全」。現在也能把自己記下的
一段真實工作日做成 replay 語料：

```bash
sister replay export --last 24h --to ./workday.sister-replay-draft.json
sister replay import ./workday.sister-replay-draft.json --dry-run
```

`export` 寫的一定是私有 **Draft**：時間改成相對值、文字先自動去敏，而且零截圖、
零來源資料或圖片路徑。但「自動去敏跑完」不等於「可以分享」：真實螢幕文字裡可能
還有程式不認得的人名、內部案號與對話，必須由人逐項看過，才把 JSON 的 `review`
從 `draft` 標成 `reviewed`。只有這種 **Reviewed** corpus 才能分享。
`import --dry-run` 可以在本機驗證 Draft，用去敏後的 L0 重建搜尋索引和 L1 事實，
不會因為它尚未 Reviewed 就禁止本機重播。兩個指令都不會上傳任何東西。

要把同一段時間裡真的問過她的話一起做成待標註題庫，在 export 多給一個輸出檔：

```bash
sister replay export --last 14d --to ./workday.sister-replay-draft.json \
  --questions-to ./workday.sister-questions-draft.json
```

題庫和 corpus 綁同一個指紋，時間只留相對毫秒，不帶資料庫 row id 或真實 epoch。
每題的 `expected` 都是 `null`：當時回 0 筆、點過出處或按過 ★ 都只算標註提示，
不會被猜成正解。題目保留你輸入的原話、沒有自動去敏，所以這份檔案是 private
Draft；現在不用手改 JSON，可以在終端把整份題庫走完：

```bash
sister replay questions status ./workday.sister-replay-draft.json ./workday.sister-questions-draft.json
sister replay questions annotate ./workday.sister-replay-draft.json ./workday.sister-questions-draft.json \
  --to ./workday-labeled.sister-questions-draft.json
sister replay questions review ./workday.sister-replay-draft.json \
  ./workday-labeled.sister-questions-draft.json --to ./workday.sister-questions.json \
  --confirm-private-text-reviewed
```

`annotate` 每題顯示產品真正的 `facts` 檢索候選，也可用 `f 文字` 搜 corpus、
`e EVENT` 看可作 evidence 的文字；只有人輸入 `a EVENT 答案` 或 `n` 才會落標籤。
輸出永遠寫到另一個新檔，不改來源、不覆寫既有檔案；進入互動前先確認目的地可寫，
每完成一題就同步一份仍然合法的 Draft，輸入 `q` 可帶著進度離開。`review` 只有在
全部標完、fingerprint 與 evidence 都有效，而且人明確確認未去敏的題目原話已審查
後才會產生 Reviewed 題庫。corpus 與題庫仍各自審查，任何一邊不會替另一邊通關。

Phase 2 的第一版 runner 也已經可以直接跑：

```bash
sister replay evaluate scenarios/recall-baseline.corpus.json scenarios/recall-baseline.questions.json --k 5 --runs 3
```

完整語法是 `sister replay evaluate <corpus> <questions> [--k K] [--runs N] [--json | --to FILE]`；
`--k` 預設 5、`--runs` 預設 3。`--json` 把完整報告印到
stdout，`--to` 寫進一個新檔且拒絕覆寫。repo 裡的
`scenarios/recall-baseline.corpus.json` 與 `scenarios/recall-baseline.questions.json`
是 3 個純合成事件、5 題 QA 的 Reviewed smoke fixture，不含真實工作日資料。

兩個配置走的都是真正產品檢索接線。`baseline_text` 是現有文字路徑：三份 FTS5
索引加上必要時的有界 LIKE fallback，不是只跑一個「純 FTS」查詢；`facts` 在同一
條文字路徑上加 L1 typed facts，並把 fact 結果排在文字結果前。下表由實際
`sister replay evaluate --json` 的穩定欄位自動生成；CI 會重跑同一份 fixture，
而腳本裡 checked-in 的 regression contract 會鎖住目前接受的分數。若有意接受一組
新的 baseline，要先查清變動、更新腳本的 `expected_scores`，再跑
`python3 scripts/check-recall-baseline.py --update-readme` 重生下表：

<!-- BEGIN GENERATED: recall-benchmark -->
<!-- 由 scripts/check-recall-baseline.py 生成；不要手改這一段。 -->
| 配置 | 找回率@5 | 答案正確率 | 出處正確率 | 模型呼叫 | 成本 |
|---|---:|---:|---:|---:|---:|
| `baseline_text` | 2/4（50.0%） | 3/5（60.0%） | 2/4（50.0%） | 沒跑腦 | 沒跑腦 |
| `facts` | 4/4（100.0%） | 5/5（100.0%） | 4/4（100.0%） | 沒跑腦 | 沒跑腦 |
| `facts_session` | 4/4（100.0%） | 5/5（100.0%） | 4/4（100.0%） | 沒跑腦 | 沒跑腦 |
<!-- END GENERATED: recall-benchmark -->

上表那 5 題**都沒有時間範圍**，所以 `facts_session` 和 `facts` 一模一樣——
章節在那種問法上什麼都不會多帶來，這一列照實登出來。章節幫得上忙的是
「我昨天下午在弄什麼」這種問法：問句裡沒有可以拿去比對螢幕的內容詞，
純文字檢索撈不到東西。下面這份是另一份 fixture（一個 115 分鐘的合成下午、
3 題），由同一支腳本以同樣方式生成與鎖定：

<!-- BEGIN GENERATED: recall-session-benchmark -->
<!-- 由 scripts/check-recall-baseline.py 生成；不要手改這一段。 -->
| 配置 | 找回率@5 | 答案正確率 | 出處正確率 | 模型呼叫 | 成本 |
|---|---:|---:|---:|---:|---:|
| `baseline_text` | 1/2（50.0%） | 2/3（66.7%） | 1/2（50.0%） | 沒跑腦 | 沒跑腦 |
| `facts` | 1/2（50.0%） | 2/3（66.7%） | 1/2（50.0%） | 沒跑腦 | 沒跑腦 |
| `facts_session` | 2/2（100.0%） | 3/3（100.0%） | 2/2（100.0%） | 沒跑腦 | 沒跑腦 |
<!-- END GENERATED: recall-session-benchmark -->

**這份的分母只有 2 題**，所以它證明的是「章節在時間範圍問句上撈得回東西，
而純文字和 facts 撈不回」這個機制，不是一個有統計份量的數字。要有份量得等
題庫長到 ≥ 100 題（Phase 2 的退場條件之一，還沒到）。

延遲會隨機器與 runner 浮動，不放進上面的 CI 比對。以下只是有日期、有環境的
快照：2026-08-23，在目前 Linux 開發機用 release build，先暖身 1 輪、再每題
計時 3 次：

| 配置 | 延遲 p50 / p95 |
|---|---:|
| `baseline_text` | 0.06 / 0.09 ms |
| `facts` | 0.15 / 0.19 ms |

題目來源是 query log 0、人工標註 3、腳本埋題 2。兩個配置都沒有模型路徑，所以
模型呼叫是 0、成本是 US$0/天；提醒誤報／漏報、斷句 F1、Reviewer 回查率、CPU、
RAM、電池與磁碟還沒量，JSON 報告裡是 `null`，不是 0。延遲只代表這台機器這一次
執行；這組 5 題合成 fixture 是 runner 的可重現 smoke test，不是 ≥100 題的公開
Phase 2 baseline，也不能代表真實工作日品質。完整報告會帶回傳文字；corpus 與
question set 各有自己的 Draft／Reviewed 狀態，任一輸入仍是 private Draft 時，
報告也仍是私有資料，人工審查前不要分享。

要在桌面看這份報告，先明確打開開發者入口。Windows 上桌面真正讀的是
`%APPDATA%` 底下的 `ted-h\AI-Sister\config\config.toml`。檔案已有 `[shell]` 時，只在那個
區塊加入或修改 `developer_mode`，不要再貼第二個 `[shell]`；區塊不存在時才新增
下面這一段。沒寫這項時等同 `false`，一般使用者的系統匣不會出現它：

```toml
[shell]
developer_mode = true
```

完整結束再重開 `sister-desktop.exe`，系統匣才會多一項「評測指標…」。先用 CLI
把報告寫成另一個新檔，再從頁面的原生選檔器打開：

```bat
.\sister.exe replay evaluate .\workday.sister-replay-draft.json .\workday.sister-questions.json --to .\report.json
```

選檔後，完整 report 文字會短暫進入這個本機 WebView，再由同一行程裡的 Rust
嚴格解析；頁面實際保存和顯示的是 Rust 回傳的數值 projection。它拿掉 report
裡全部自由文字，包括 corpus／題庫名稱、fingerprint、逐題原問句、回傳內容與
自由填寫的題目 id；失敗題改用 question set 的 1-based 題號定位。整條路不連網、
不上傳，頁面也不另存一份報告。不過磁碟上的
`report.json` 原檔仍含那些文字；任一輸入是 Draft 時，頁面會一直顯示 private
Draft 警告，不能因為畫面沒有逐字內容就把原檔拿去分享。這個入口已接線，但真
Windows 的系統匣、選檔器與三種載入狀態仍列在實機清單，沒有拿 Linux 測試冒充。

問她「**她知道了什麼**」不再把「知道」兩字拿去全文搜尋。桌面姊妹會直接列最近最多
三張 current L2 理解卡；每張都明講是可修正的假設，保留作者、信心來源與畫面出處按鈕。
有原始紀錄、但還沒整理出 L2 時，她會直接這樣說；最近候選目前沒有畫面出處時，也不拿
無出處的卡或 OCR 片段補位。這個窄 intent 只認完整問法，所以「妳知道客服電話嗎」仍照
「客服電話」搜尋。總覽本身不會叫 configured CLI，也不寫入既有 retrieval query log；
既有 L2 則可能是先前在第二張同意下，由使用者設定的 CLI 整理出來的。

問她「**剛剛發生什麼事**」會得到答案，而不是「我記得的東西裡沒有這件事」。那句話
問的是時間、不是關鍵字，所以她不會拿那七個字去比對——她直接把最後看到的幾件事列
出來，每一筆一樣掛著時間與出處，而且會先講一句「我把它當成時間問題了」，你才知道
答案為什麼跟你打的字對不上。判斷刻意做得很膽小：句子裡只要還剩下任何講得出內容的
詞（「剛剛那個電話號碼」），就照舊走搜尋——把你真正想問的東西弄丟，比多查一次糟
得多。

而走搜尋的時候，「剛剛」那兩個字**不會跟著進去比對**。中文沒有空白，整句話會被
當成一整串子字串去找，而沒有人的螢幕上寫著「剛剛那個優惠方案」——所以頭尾的時間
詞和虛字先剝掉，中間原樣留著。加了「剛剛」之後變成零筆，是這個產品最容易失去信任
的那種答案。一般檢索問法仍由 `sister query` 和桌面姊妹共用同一份規則；上面的 L2
總覽則是桌面問答外層的窄 intent，CLI 目前不把同一句改成總覽。

問「**電話**」的時候，答案是那串號碼本身（`★ +886800080123`，底下附著螢幕上原本
那行「客服專線 0800-080-123」和它被看到過幾次），不是一堆剛好提到電話的字。螢幕上
從來沒有出現過「電話」兩個字，全文比對永遠接不起來——但記下那串數字的時候，它已經
被標成一支電話號碼了。這一層本來只有終端機有，桌面姊妹只會做全文比對：同一句話，
`sister query` 答得出來、她說找不到。現在兩邊是同一份程式碼（`sister-core::answer`），
和上面那個「剛剛」的判斷同一條紀律。

**答不出來的時候她會講出查得到的理由，而不是猜。** 問「轉帳帳號」而她兩手空空，
以前得到的是「這件事我沒看到過」——一句斷言，而真正的答案往往是「你自己叫我不要看
那個網站」。她其實查得到：排除規則生效過幾段、暫停過幾次，都在資料庫裡躺著。
所以現在那句話底下會接上「不過你自己的排除規則擋掉過東西（excluded url 12 段、
excluded app: keepassxc 3 段）——在那裡面的我本來就不會知道」。查不到任何理由的
時候她就直說：她記的每一段裡都沒有這個字。那句話沒有安慰的成分，但它是真的。

連那句開場白都是講她自己的紀錄，不是講這個世界：「**我記得的東西裡沒有這件事**」，
不是「這件事我沒看到過」。東西可能就在螢幕上，只是被排除規則擋掉、被暫停跳過、
或者 OCR 沒讀出來——最後那一種她連數都數不出來，所以下面那幾行理由永遠不會是
完整的。和 ★ 上面那句「我最後看到的是：」同一條紀律。

桌面姊妹也會告訴你**現在到底有沒有人在錄**，而且那句話底下就是把她開起來的按鈕。
這兩件事以前混在一起：`sister record` 是另一個執行檔，沒有人把它跑起來的時候
暫停旗標是乾淨的，於是她顯示「在聽」——而她什麼都沒在看。現在沒人開她的時候
她是灰的，寫著「沒有人在記錄——從現在起發生的事，她不會知道」，並且長出一顆**開始記錄**
——那是整個灰掉的畫面上唯一有顏色的東西。和暫停長得不一樣，因為**下一步不一樣**：
一個要按繼續，一個要把 recorder 開起來（暫停中不會出現那顆按鈕，不然會變成
兩個 recorder 各錄一份）。判斷靠 recorder 每 5 秒蓋一次的時戳，不靠資料庫裡
那筆 session——recorder 當掉的時候那一筆會永遠停在「還沒結束」。

灰掉的時候她還會多講一句**上一次是什麼時候、為什麼停的**（「上一次 08-19 02:53
停的：你按了停止」）。少了這一句，早上打開電腦看到的那句灰字，既可能是你昨晚
自己按的停止，也可能是她半夜當掉、你一整天都沒被記錄——而只有後者需要你做什麼。
`sister doctor` 上有同一行。

按下去她起不來的話，`record.log` 的最後幾行會直接顯示在她身上（同意書沒簽、
找不到 `sister.exe`、已經有一個在跑）——那個檔案在 `%APPDATA%` 深處，而正在
看著一顆沒反應的按鈕的人不會去翻它。停止在系統匣選單裡（那一顆的字會跟著
現在的狀態換），也可以 `sister stop`。**結束桌面姊妹會連記錄一起停**，而正在錄
的時候那一項就直接寫成「結束（記錄也會停）」——不然他關掉的是唯一看得見的
那個視窗，而螢幕還在被記錄。停止走的是一個檔案而不是把行程砍掉：被砍死的
recorder 不會寫完 session、不會收掉心跳，於是接下來 16 秒她會宣稱自己還在錄。
**她還在開資料庫的那幾分鐘按下去也算數**——那個請求會等她開完，然後她一個字都
不記就直接收工（一顆存了一年的資料庫第一次開起來要重建索引，那段時間不短）。

停得掉是這個產品的前提，所以暫停在四個地方都按得到（全域熱鍵 `Ctrl+Alt+P`、
桌面姊妹的 `⏸`、系統匣選單、`sister pause`），而且**不會自己恢復**——會自己醒來的
暫停等於沒有暫停。熱鍵那條路不用先找到她，代價是全域熱鍵先搶先贏：設定頁上會
直說這一組現在搶到了沒，搶不到就用警告色寫出原因，而不是讓你按了沒反應。暫停期間
她整個人是灰的，`sister record` 每分鐘會講一次，進出各留一筆稽核紀錄，
`sister stats` 事後查得回來「那天到底有沒有停」。細節見
[docs/PRIVACY.md](docs/PRIVACY.md#停用不等於刪除)。

她也會記得**你問過她什麼**——只在這台機器上。那是整個資料庫裡唯一一張存著
你自己打進去的字的表（其他每一張都是她觀察到的東西），所以它有自己的開關、
自己的一節文件，而且「忘掉這一段」會把那段時間問過的話一起帶走。留著的理由
是下一階段的評測要用真實的用詞當題庫，而那種東西補建不回來——沒有人記得住
自己上禮拜是怎麼問的。**一筆都沒找到的那些題目是最有價值的**：找得回來的只
證明她現在能做什麼，找不回來的才是下一版要修的。`sister queries --empty` 就是
在問這件事。

那張表上還有一格不是她記的，是你按的：她答對一件你早就忘掉的事的那一刻，答案
底下那顆「這件事我本來已經忘了」——終端機上是 `sister mark`——把這一次記下來。
為什麼要為這件事新開一格：題庫的其他每一欄都答不出來——它們記得住你問了什麼、
她給了幾筆、你點開了哪一個出處，**記不住你當時知不知道那個答案**。它也是唯一補不回來的一格——那是你看到答案那一刻
腦袋裡的狀態，一個禮拜之後翻題庫翻不出來。按錯了再按一下就收回，
`sister queries --marked` 列出是哪幾題。

看得見才談得上信任，所以時間軸（拖曳條上的 `▤`）列出她有紀錄的每一天，而且
**每一段空白都會說明自己**：她被按了暫停，和你盯著同一份文件沒動過，在那條線上
是兩種不同的顏色和兩句不同的話。翻到不想留的那一段，底下那條就能把它忘掉——
兩下才會刪，第一下先把「會刪掉多少」擺給你看。終端機上是 `sister forget
--last 2h`（一樣兩段式，而且 `--last 30` 這種沒有單位的寫法直接拒絕：它看起來
像 30 分鐘，也一樣像 30 天）。詳見
[docs/PRIVACY.md](docs/PRIVACY.md#忘掉某一段時間)。

**帶得走才是你的。** `sister export --to <目錄>` 匯出的目的地就是一個資料
目錄，不是另一種格式——所以還原不需要任何工具，也不需要這個專案還活著：

```bash
sister export --to ~/sister-backup --with-frames
sister --data-dir ~/sister-backup query 電話      # 直接就問得到
```

不要自己去複製 `sister.db`：資料庫跑在 WAL 模式，她正在錄的時候最近那一段還
躺在旁邊的 `-wal` 檔裡，只複製主檔的備份會安靜地少掉最後那幾小時，而你會在
真的需要它的那天才發現。

17 位角色的 workplace rig 與 WebP 退路都隨 desktop 離線提供，拒絕下載或
舊 cache 壞掉時仍然可用；沒有用單一字母冒充角色的 fallback。選配 pack 只補
四姊妹的固定語音，而且仍須通過 public rights projection、整包與 selected-file
驗證才能播放。兩套 bundled 圖像另有自己的來源 manifest 與授權排除，
不能拿「已放進安裝包」替素材權利背書。

先跑 `sister doctor`——它不會宣稱任何東西，只會當場示範給你看：能不能讀到你現在的網址、
OCR 引擎讀不讀得出內建那張圖上的字、哪幾條隱私規則現在其實不生效。

已經量過的（GitHub Actions 的 windows-latest runner、release build、1024×768，
**不是**一般桌機的數字，主要拿來擋回歸）：

| 一次要多少 | 實測 |
|---|---|
| 讀一次螢幕（一個 tick 只讀一次） | 17–33 ms |
| OCR 一張 | 126–193 ms |
| 寫一張 PNG | 6 ms |
| 沒有人動鍵盤滑鼠的那些 tick | **0 ms**——根本不碰螢幕 |

第一列在你的機器上會是完全不同的數字，而它決定了其他所有事：一次讀螢幕的
成本是隨**來源像素**走的，1024×768 到 2560×1440 是 4.7 倍。實測過一台
2560×1440 是一次 127 ms。這件事不能用推的，所以有 `sister bench`——它把一次
擷取拆成建立 GDI 物件、`BitBlt`、`GetDIBits` 三段，一次只換一個變因，跑完就
結束，不寫資料庫也不留任何畫面。

為什麼要在意最後那一列以外的數字：省電閘門只管「沒人碰的時候別看」，可是就算
一整天沒有人碰，每 5 秒還是得睜一次眼（「沒有輸入」只是「畫面沒變」的猜測，
不是保證）。所以一天最少仍有 17,280 次抓圖；但「一次多少 ms」量的是牆上時間，
裡面包含等顯示驅動，不能直接換算成 CPU 百分比。錄製收尾會把抓圖時間與整段
CPU 分開照實列出；alpha.46 的 44.0% 是已接受的活躍寫程式基準，不再拿這個
抓圖地板替它猜原因。

查詢那一邊，在**一個半月份量**的資料庫上量的（3,110,400 行字，
開發機、release build。CI 每次 push 也會重跑同一份 benchmark）：

| 你打了什麼 | 走哪條路 | 實測 |
|---|---|---|
| `客服專線`（三個字以上） | trigram 索引 | 0.1 ms |
| `0800`（整個 token） | unicode61 索引 | 0.7 ms |
| `客服`（兩個字的中文） | bigram 索引 | 0.1 ms |
| 查不到的東西 | bigram 索引（確定沒有） | 0.1 ms |
| `工`（一個字的中文） | **沒有索引，只能掃 30 天** | 0.1 ms |

第三、四列以前是 224 ms 和 96.7 ms，而且只找得回最近 30 天。那是一個真的
缺口，不是還沒調校：trigram 比不了少於 3 個字，而 unicode61 把「客服專線」
整串當成**一個** token（不是逐字切），所以 `MATCH "客服"` 是 0 筆——而兩個
字正是中文裡最常見的詞長。剩下唯一找得到的辦法是掃過所有文字，成本跟你用了
多久成正比，只好夾在 30 天內。

補法是第三個索引：把中文切成**相鄰的雙字**存起來（「客服專線」→「客服
服專 專線」）。代價量過是 **+29%** 的資料庫大小（207,360 行字：97.9 MB →
126.0 MB），不是原型估的翻倍。時間界線跟著消失了。

還剩最後一列：**一個字**的查詢產不出雙字，所以它仍然是掃描、仍然只看得到
30 天。`sister doctor` 會直接告訴你，你自己的資料庫裡有多少行中文已經進了
索引。

（順帶一提，同一份 benchmark 在 runner 上大約是開發機的 2.1 倍。所以上面那
張表裡任何一個貼著門檻的數字，換一台機器就會翻面。這也是為什麼「掃描不會跟
著資料長大」那件事是用行為測的，不是用碼錶測的。）

第一張表的最後一條是目前最大的一筆。沒有人碰鍵盤滑鼠，畫面就多半沒變，那就
連讀都不要讀（最多每 5 秒仍會睜眼一次，換視窗也會，不然影片和進度條會整段消失）。
在 CI 上量到的差別：每 tick 40 ms → 13 ms、CPU 3.5% → 2.5%。

擷取的成本幾乎完全由**讀了多少來源像素**決定，跟你要縮到多小無關——
原生 1024×768 是 28.4 ms、縮到 256×192 是 24.2 ms，目的地像素差 12 倍。
所以這裡沒有「先用小圖探一下」這種東西：那只是把一張讀完的畫面丟掉，然後再讀一次。

Phase 0 的七天自我錄製與磁碟預算還沒達成；CPU／RAM 的真機基準與剩下的磁碟缺口記在首段。

## 文件地圖

| 文件 | 內容 |
|---|---|
| [docs/PRODUCT.md](docs/PRODUCT.md) | Final Product 定義：定位、信條、killer scenarios、競品、護城河、non-goals |
| [docs/SPEC.md](docs/SPEC.md) | Final Spec：四層真相模型、五個子系統、隱私架構、成本模型、技術選型、懸案判決 |
| [docs/PHASES.md](docs/PHASES.md) | Phase 0–8 里程碑：每階段退役一個致命風險，附可量測 exit criteria |
| [docs/PRIVACY.md](docs/PRIVACY.md) | 承諾、邊界，以及**我們做不到的事**（旁人同意、規則式排除的極限） |
| [docs/DATA_INVENTORY.md](docs/DATA_INVENTORY.md) | 逐欄位盤點她到底存了什麼，含已知缺口 |
| [docs/THREAT_MODEL.md](docs/THREAT_MODEL.md) | 資產、攻擊者、明確不防禦的項目，以及三個真實發生過的靜默失效 |
| [docs/WINDOWS-CHECKLIST.md](docs/WINDOWS-CHECKLIST.md) | 開發機上一行 Windows 擷取程式碼都沒被執行過——這是那些只有真機器答得出來的問題 |
| [research/landscape.md](research/landscape.md) | 競品與生態現況（2026-08 查證）：Recall、Rewind/Limitless、Screenpipe、Everywhere、各家桌面 AI、computer-use 專案 |
| [research/tech-stack.md](research/tech-stack.md) | 技術選型調查：per-OS capture、繁中 OCR、SQLite FTS5/向量、Tauri overlay、資源預算 |
| [research/cost-model.md](research/cost-model.md) | LLM 成本試算（2026-08 實價）：四種架構情境的月費 |

設計源頭是一場四模型（Claude / Gemini / Grok / ChatGPT）7 題 × 5 輪的 roundtable
辯論；其逐字萃取屬私下對話，未公開，但所有收斂結論與判決理由都寫進了上面三份設計文件
（特別是 SPEC §17 的懸案決策表）。

## 三個一句話

- **產品**：檔案櫃（L0/L1，零 LLM）+ 大腦（L2/L3，事件驅動、可推翻、可結案）+
  守門員（開口預算制）。
- **原則**：暴力用在保存，不用在生成；抄寫歸程式，意圖歸模型；敢開著比聰明重要。
- **路線**：「搜得到」與「想得對」已接上，現在在 Phase 6 把「接得了手」的授權與
  prompt-injection 邊界做完；之後才進 bounded takeover——每一步都用重播與 hostile
  fixture 的結果守門。
