# DATA_INVENTORY — 她到底存了什麼

> 這份文件描述 **schema v19**（`sister-core` 的 `MIGRATION_001`…`019`）。
> v8 加了 `segment`（斷句結果，打開時間軸才算，不在錄製熱路徑上）。
> v9 加了 `segment_edit`（使用者合併／切開的訓練訊號）和 `stuck_signal`
> （卡住偵測 v0，只記錄不開口）。兩張都不是錄製熱路徑。
> v10 加了 `l2_card` / `brain_outbound` / `brain_skip`（解釋層）。
> v11 拿掉 `brain_outbound.redaction_json`（出境改為原文）。
> v12 加了 L3（`commitments` / `entities` / `entity_mentions` / `day_summaries` /
> `preferences`）、全域血緣圖 `provenance`、L2 墓碑欄，以及審閱層的
> `reviewer_run` / `reviewer_recheck` / `reviewer_divergence`。
> v13 加了 `utterance`（守門員實際說了、或決定先不說的候選與理由）。
> v14 把 `commitments.allowed_next_step_fact` 綁回一筆 L1 fact；v15 加
> `commitments.agreed_evidence_json`，只留雙 pass 共同引用的證據。舊列在這兩欄
> 可以是 `NULL`：那是當時沒量，不是量到空集合。
> v16 把 `reviewer_run` 裡的說明搬到 `notes`，和真的拒絕 `detail` 分開；v17 加
> nullable 的 `answers_got`，把「試著叫了幾次 CLI」和「真的拿到幾份答案」分開。
> v18 加查詢索引；v19 補齊 `input_health` 表與索引，讓升級安裝和全新安裝一致。
> 刪一段 L0 時沿 provenance 把衍生 L2/L3 **tombstone**：列還在、標成被忘掉，
> 但**那一列說了什麼會一起清掉**（活動、承諾原文、人名、日摘要敘事）。
> 墓碑留的是「這裡曾經有東西」，不是內容——鐵律 2 說任何 L2/L3 不得成為
> 證據的唯一載體，而 L0 已經刪掉了。
>
> 規則：**動到訊號面的 PR 必須同步改這份文件**（PHASES.md §工作紀律 3）。
> 如果程式碼多存了一個欄位而這裡沒寫，那是 bug，不是文件落後。

她正在使用的**記憶**全都在一個 `<資料目錄>/sister.db` 檔案裡；畫面檔在旁邊的
`frames/YYYY/MM/DD/`；你按過「要我幫你打開嗎」那顆按鈕的話，還有一個
`action-log.jsonl`（alpha.69 起，單獨列在下面）。備份、加密、刪除這份活的
記憶，是這三個對象。
你自己跑 `sister replay export` 時會明確多做一份私有草稿；它是匯出副本，
不是活資料庫的一部分，要另外刪除。

17 張 current 角色 WebP 與 17 套 workplace 分層 rig 都是程式本身的靜態資產，
分別位於 `apps/desktop/ui/personas/` 與 `apps/desktop/ui/persona-reels/`。17 張
640×640 透明全身 WebP 合計 1,031,124 bytes；Reel 只選 402 張 PNG、
35,140,885 bytes，畫面只建當前人的
21–26 層。兩組都不在資料目錄、不是 cache，也不受 memory export／
forget／prune／Persona cache 撤回影響。來源、大小、SHA-256 與圖像授權
排除分別固定在同目錄的 manifest/NOTICE。Reel manifest 不含 raw receipt、
產生時間或本機絕對 path。

source tree 另有由 exact ChatGPT WebP 裁出的五個平台圖示輸入，共 464,143 bytes；
它們不是記憶或 cache，也不受上述刪除操作影響。Windows artifact 取其中
`icon.ico` 編進 executable／installer，macOS config 取 `icon.png`，不是把五個來源檔
原樣裝到使用者資料夾。圖示的來源、recipe、逐檔 hash 與獨立 owner-grant 範圍固定在
`apps/desktop/src-tauri/icons/{manifest.json,NOTICE.md}`。

另有一個物理上位於預設資料目錄、但**不是記憶**的舊固定錄音素材 cache：
`Config::default_data_dir()/persona-assets-v1/`。安裝時會短暫有 `.staging-*`；完成後是
`ai-sister-media-11-voice55-2026.07.23/objects/` 底下四張 selected WebP、八條 selected
WAV，以及 `installed-v1.txt`。receipt 只有 schema、release ID、canonical manifest
digest 與 pack digest，沒有下載時間；73,261,088-byte omnibus ZIP 與其餘 934 個未選
object 不留在 cache。這裡不得放 OCR、畫面、問題、答案、記憶 ID、目前 persona 或
request-derived IP／response header。`--data-dir` 不搬它；memory export、`sister
forget`、`sister prune` 都不讀、不複製、不刪它。Persona 撤回才停用並精準刪除該
release；直接刪掉整個預設資料目錄當然也會連素材一起刪掉。既有 staging 或 cache
損毀時會落到 `RepairNeeded` 並停用額外固定錄音；全新下載在建立 staging 前驗證失敗
則仍是 `Available`，並顯示當次錯誤。兩者都保留 bundled 角色圖，也不會自動連網修復。

cache 旁邊另有四種不跟 release 目錄一起刪的協定檔：`persona-assets-v1.lock-v1`
是空的跨行程鎖；`persona-assets-v1.revocations-v1/` 在第一次安裝嘗試或撤回時建立一個
空的 `epoch-<128-bit OS-random>`，之後每次撤回會追加一個內容只有 schema／release ID、
檔名為 `revoke-<128-bit OS-random>` 的 marker；
`persona-assets-v1.revocations-authorized-v1` 在任何一次完整安裝（包括第一次安裝、
修復或重裝）成功後保存當下 epoch＋ticket 名稱集合的 digest；
`persona-assets-v1.revocations-settled-v1` 保存最後一次 blocking remove 已結束處理的
同一種集合 digest。新 ticket 會立刻讓 settled digest 失配，下載／修復要等 remove
結束才可重新由人開始；即使精準刪除回錯，結束點仍會寫下，讓下一次明確修復可以處理
留下的 `RepairNeeded`，但不會因此重新授權舊素材。這些檔案不含角色、網路 metadata、
記憶、PID、撤回次數或程式自行寫入的操作時間；檔案系統本身仍會像任何檔案一樣留下
建立／修改時間，marker 數量也看得出撤回 ticket 的累積數。這組檔案讓另一個行程正在
下載或程式在撤回途中當掉時，舊 release 不會重新被當成可用；所以 memory export／
`forget`／`prune` 也不碰它們。

alpha.110 的 Azure TTS **不新增答案或 MP3 cache**。四道 gate 齊全時，每份最新新答案
自動朗讀一次；trusted 手動重播會再請求一次。該答案正文、產生的 SSML、從 Credential Manager 短暫讀出的 subscription key，以及
回傳的 MP3 只在 desktop／TTS 行程 RAM 裡活到該次 request／播放結束；沒有磁碟 cache，
也沒有可供下一次重播的記憶體 cache。停止或換題會讓舊 playback generation 失效，
晚回的 MP3 不播放、不保存；已取得送出權的 blocking POST 仍可能留在 RAM 並跑到 45 秒
timeout，相關設定／key／consent mutation 或 native cancel 可能等它結束，但回覆成功後
舊 request 不會才送。OS paging／crash dump 與同使用者權限程式仍可能觀察行程記憶體，不能把「不
持久化」寫成「那些 bytes 從未進 RAM」。

同一個資料夾裡還有 `replay-drafts/`（它有資料，單獨列在下面），以及幾個
小檔案。底下這幾個小檔案**都不含你的任何資料**：

| 檔案 | 是什麼 | 刪掉會怎樣 |
|---|---|---|
| `paused.flag` | 她現在有沒有被暫停的相容旗標。新版內容是格式版本、pause generation 與按下暫停的時戳；舊版純時戳仍讀得懂 | **不保證解除暫停**：`pause.state` 仍可能維持同一代 pause；請用 `sister resume` |
| `pause.state` | 最近一代 pause 的 generation、是否仍暫停，以及 pause／resume request 時戳；解除後仍保留，防止一整段 pause→resume 在兩次 recorder 探測之間消失 | 執行中不可刪。所有 AI-Sister 行程關閉後，損毀時才可刪除；會失去這份控制歷史，下次操作會重建 |
| `pause.lock` | 空的跨行程 read/write transaction 鎖；檔案留著，真正的鎖由作業系統 handle 持有 | 執行中不可刪，否則不同 process 可能各鎖到不同檔案；所有 AI-Sister 行程關閉後才可修復／重建 |
| `hands.stop` | 她的手現在是不是被拔掉。內容只有第一次拔手的毫秒時戳 | 等於安靜地把手接回去，所以任何 forget、prune、export 都刻意不動它 |
| `master.stop` | 三層全停（capture／brain／hands）現在是不是開著。內容只有第一次按下全停的毫秒時戳 | 等於安靜地把三層一起放回去，所以任何 forget、prune、export 都刻意不動它 |
| `master.stop.pending` | `stop-all` 已在線性化閘門內發佈、正在等舊活動排乾的停止意圖；正常完成 engage 或 release 後會移除 | 不可手動刪；可能讓已經開始的 capture／CLI／OS call 排乾前，新活動誤以為可以進場 |
| `master.stop.lock` | 空的永久 activity drain 鎖。capture tick、CLI spawn/stdin、reviewer product mutation、hands OS call、doctor/bench live probe 都持 shared handle；engage/release 取 exclusive | 永遠不靠 unlink 解除。執行中不可刪，否則不同 process 可能鎖到不同 inode；所有 AI-Sister 行程關閉後才可修復／重建 |
| `master.stop.turnstile` | 空的永久 admission／不可逆邊界鎖。新活動和最後一次 persistence/spawn/OS call 在 shared lock 內重驗 latch/pending；engage 在 exclusive lock 內發佈 pending | 永遠不靠 unlink 解除。執行中不可刪，否則 admission 與 engage 可能落在不同 inode；所有 AI-Sister 行程關閉後才可修復／重建 |
| `consent.toml` | 四張同意書各自是**何時**簽的；前三張有共同條文版本，第四張 `azure-tts` 另有獨立 terms version | 等於四張都沒簽；`sister record` 拒絕啟動，Azure TTS 一次都不呼叫 |
| `consent.lock` | 空的跨行程同意 transaction 鎖。CLI／desktop 的 grant／revoke 在 OS whole-file exclusive lock 內重讀最新 `consent.toml`、套當次變更再 atomic save；recorder start 先持 shared guard。CLI 那份跨到第一拍；desktop parent 那份只跨到 `Command::spawn` 回來便立即放掉，child 自己 nonblocking 重拿並跨到第一拍。symlink／non-regular path 拒絕 | Windows 的 live handle 會拒絕刪除；Unix Preview 的 advisory lock 擋不住 unlink／replace。執行中不可刪，否則不同 process 可能鎖到不同 inode；所有 AI-Sister 行程關閉後才可修復／重建 |
| `consent-revoke.barrier` | 第一張同意撤回在 atomic save **之前**發布的獨立 durable barrier；內容是 `v1:` 加 fresh 256-bit generation。Recorder 把它視為 `consent-revoked` 停止條件，但不消費；Start 無權清。只有成功 commit 的 local-recording regrant 可用相符 generation ticket 清理，並先確保另有 durable stop intent | 刪掉可能讓失敗的 consent save 留著舊有效同意時重新開錄，也可能讓同一拍內的快速 revoke→regrant 漏掉收工。不要手動刪；損毀時 fail closed，須先關閉所有 AI-Sister 行程再修復 |
| `pet-window.json` | 字母人視窗的位置與置頂狀態 | 下次開在右下角 |
| `recording.beat` | `sister record` 每 5 秒蓋一次的時戳，用來告訴字母人「現在真的有人在錄」。收工的時候**不刪檔**，改寫成一塊墓碑（`0 stopped <收工時間>`） | 字母人一樣顯示「沒有人在記錄」；但「這台機器從來沒跑過 recorder」和「她剛剛才收工」會變回同一句話 |
| `recording.lock` | 空的 recorder 單一擁有者協定檔。每個 CLI／desktop recorder 在第一拍 heartbeat 前 nonblocking 取得 OS whole-file exclusive lock，整場持有；process crash／handle drop 由 OS 釋放。symlink／non-regular path 拒絕 | Windows 的 live handle 會拒絕刪除；Unix Preview 的 advisory lock 擋不住 unlink／replace。檔案本來就可持久留著，刪掉不是「解除占用」；執行中刪除反而可讓不同 process 鎖到不同 inode。所有 AI-Sister 行程關閉後才可修復／重建 |
| `stop.request` | 尚未交給 recorder 的 durable stop latch；內容是 `desktop-quit`、`requested` 或 `consent-revoked`，舊版 `stop` 仍讀成 `requested`。獨立 revoke barrier 活著時先顯示撤回；成功 regrant 清 barrier 前若原本沒有 stop，會在這裡留下 `consent-revoked`，既有 marker 則原封保留 | 可能讓已排重試在不知使用者已停／已撤回的情況下又啟動；不要手動刪 |
| `stop.consumed` | recorder 已收到的同一種 durable stop latch。recorder 把 pending 原子搬到這裡，不刪除意圖；內容與強度同上 | 可能讓 child 在 DB finalize 失敗、非零退出後被 watchdog 當 crash 復活；真人顯式 start 可清三種，下一個全新 opt-in login 只可在五分鐘 bounded handoff 與完整 barrier 後清 `desktop-quit`，不要手動刪 |
| `stop.lock` | 空的跨行程 stop transaction 鎖。request、consume 與 explicit clear 在 OS whole-file exclusive lock 內更新兩顆 marker；probe 只取 shared lock。檔案永久保留，存在不代表有人要求停止 | Windows 的 live handle 會拒絕刪除；Unix Preview 的 advisory lock 擋不住 unlink／replace。執行中不可刪，否則 request 與 clear 可能各鎖到不同 inode、讓停止意圖消失；所有 AI-Sister 行程關閉後才可修復／重建 |
| `desktop.log` / `desktop.log.1` | 字母人這一輪（與上一輪）自己發生了什麼事 | 沒有影響，下次開會重寫 |
| `record.log` / `record.log.1` | 從字母人按「開始記錄」跑起來的那個 `record`，它印在終端機上的東西 | 沒有影響，下次開會重寫 |
| `capabilities.json` | 上一份能力快照：這台機器對每一項是已知可用、已知不可用，還是沒量到；錄製中每分鐘更新 URL 實測證據 | 設定頁改說「還不知道這幾條會不會生效」 |

`paused.flag`、`pause.state`、`pause.lock` 是同一份跨行程控制協定，不是三個
可以各自切換的開關。Desktop 的 toggle 會在 `pause.lock` 的 exclusive lock 裡配置
新 generation 並更新前兩個檔；recorder 每一道內容持久化邊界持有 shared lock，讓
「這次寫入」和「這次暫停」有唯一先後。`pause.lock` 本身不會因 owner crash 變成
stale lock，作業系統會釋放 handle；檔案留在原位是刻意的。

三者任一讀不到、格式互相矛盾，或 `pause.lock` 不是一般檔案時，都會 **fail closed**
成暫停。恢復時先關閉 recorder、字母人及其他 AI-Sister 行程，再修好資料目錄權限；
`pause.lock` 必須是一般檔案，損毀的 `pause.state` 可在所有行程關閉後刪除。最後跑
`sister --data-dir <同一個資料目錄> resume` 明確解除。不要在任何行程仍執行時刪
`pause.lock`，也不要只刪 `paused.flag`：新版 `pause.state` 仍可能正確地維持暫停。

`consent.toml` 存時戳而不是 `true`／`false`，因為「你什麼時候同意的」是一個
你有權利問、而我們答得出來的問題。讀不到的時候一律倒向「她做得比較少」那一邊：
暫停控制狀態讀不到當作暫停中，同意書讀不到當作沒簽；心跳讀不到則明講狀態未知，
不宣稱正在錄，也不把它折成「沒有人在錄」來開放 Start／retry。
第四張 `azure-tts` 只鑄出 Azure TTS permit，不借用 `cloud-reading`、Persona 下載點擊
或 Persona 的本機聲音開關。alpha.109 讀到沒有 Azure 欄位的同版本舊 consent 時，
原三張時戳原樣保留，第四張是 `None`；alpha.110 讀到 click-only 第四張時保留它的歷史
時戳，但獨立 terms version 不符，所以不能鑄出 permit，重簽第四張才生效。未知 sheet／
欄位或無法解析的值仍 fail closed。
改同意時不可先在鎖外讀完整份、最後拿舊 snapshot 寫回；那會讓同時在 CLI
和 desktop 改不同 sheet 時後寫者無聲把先寫者蓋掉。每個 mutation 要先持有
`consent.lock`，在鎖內重讀、套這次 grant／revoke、atomic save。Recorder start 不寫
consent，卻必須先拿同一把鎖的 shared guard，在鎖內重讀有效 snapshot。CLI recorder
把同一份 guard 帶過 stop clear 與第一拍；desktop parent 帶過 preflight／clear 到
`Command::spawn` 回來便立刻放掉，child 自己 nonblocking 重拿並帶過第一拍，不會讓
parent 無期限等 heartbeat。atomic replace 保證每一份 guard 看到舊檔或新檔，不是半份
TOML。lock path 若是
symlink／non-regular file 或無法取得，mutation fail closed，不跟著鎖到資料目錄外。

`hands.stop` 不在任何刪除路徑上。它沒有使用者打的字，只有一個毫秒時戳；刪掉它
換不到隱私，卻會把一道阻止執行的牆拿掉。「忘掉」若順手刪它，就等於沒有明講地
把手接回去。這和刪 `grant.json` 不衝突：grant 裡有 `--task` 原文，而且是一張
仍能拿去執行的票；`hands.stop` 是擋住執行的牆。

`master.stop` 是同一條規則，理由再強一級：這個 latch 與三顆協定檔同時擋著三層。拿掉 latch 不只
是把手接回去，還讓 capture 重新開始擷取、讓解釋層重新把字送給雲端模型——而使用者
按下「全部停止」的時候，要的正是那三件事一起停。所以 `sister forget`、`sister prune`
和匯出一樣刻意不碰它。匯出也不會把它複製到匯出目錄：那個目錄本身就是一個資料
目錄，帶一個全停 latch 過去，會讓那份備份看起來像壞掉了。

它不是 `paused.flag` 的另一個名字，兩邊各自保留：`sister resume` 解除不了全停，
`sister stop-all --off` 也不會順手解除你原本自己按的暫停或拔手。全停沒有 pause 那種
四檔協定；唯一受支援的解除方式是
`sister --data-dir <你的資料夾> stop-all --off`（**不是裸的 `sister stop-all --off`**，
那會去解除預設資料夾的全停，不是正在擋你的那一份）。不要直接刪
<你的資料夾>/master.stop；那會繞過 activity drain 與 turnstile。讀不到 latch／pending／lock
本身（權限不足、路徑壞掉、不是一般檔案或是 symlink／reparse），或資料目錄
讀不到、根本不是一個目錄的時候，一律當成全停中；資料目錄**整個不存在**則不算全停——
那是還沒開始用，不是被停下來。一條指向不存在目標的 `master.stop` symlink 也算全停：
判斷走的是 `symlink_metadata`，目錄項在就算在。

`recording.beat` 存在的理由是「暫停」和「根本沒有人開她」是兩件不同的事，
而字母人以前只分得出前者——暫停旗標乾淨的時候它就顯示「在聽」，即使沒有任何
人把 `sister record` 跑起來。判斷不能只看資料庫裡的 `sessions.ended_at`：
recorder 當掉的時候那一列會永遠停在 NULL，「她死了」和「她正在錄」長得一模
一樣。活著才蓋得動時戳，而停住的時戳自己會過期（16 秒）。
但 heartbeat 是可觀察狀態，不是原子單一擁有權：兩個 process 可能在第一顆心跳前
同時看見空房。真正阻止這個 TOCTOU 的是 `recording.lock` 當下由誰持有的 OS lock，
不是 lock 檔案存不存在。

收工的時候留墓碑而不是刪檔，是因為**「檔案不在」曾經同時是三件事**：她還沒
起來、她正在乾淨收工、她好好的只是這一拍慢了 16 秒。三件事的下一步不一樣，而
其中一件曾經被拿去守一個破壞性動作（字母人按「結束」的時候 kill 掉 recorder）
——砍在另外兩件上的代價是那一場的 `ended_at` 永遠是 NULL，然後 `doctor` 說
「她當掉了」。留下墓碑之後，「檔案不在」只剩一個意思：**這個資料目錄從來沒有
人跑過 recorder**。

墓碑的第一欄故意寫 `0`。舊版的讀法是「認不得的第二欄一律當成在錄」（那條
forward-compat 本身是對的：多寫一個欄位的新版不該讓舊版放行第二個 recorder），
所以一個舊的 `sister.exe` 讀到 `stopped` 會說「她在錄」——這個產品唯一不能說
的謊。時戳寫 0 之後，舊版走的是「過期」那條路，答案變回正確的「沒有人在錄」。
真正的收工時間放在第三欄，新版讀得到，舊版看都不會看。

`stop.request`／`stop.consumed` 和 pause 三檔是兩件事，刻意沒有合併：
暫停是「先別看，但留在這裡」，停止是「今天到此為止」——那個行程會結束。
用檔案而不是 `TerminateProcess`，是因為被砍死的 recorder 不會寫完 session、
不會收掉心跳，還可能留半張截圖；乾淨收工只有 recorder 自己做得到。

新協定不再把「收到」當「意圖可以刪掉」。recorder 只把 pending `stop.request`
搬成 `stop.consumed`，因此同一輪不會再處理第二次，desktop watchdog 卻仍看得見。
若 DB finalize 後來失敗、child 非零退出，這顆 consumed latch 會讓 retry 取消，
不把人手停止當成 crash。獨立 revoke barrier 活著時，觀察結果先呈現
`consent-revoked`，底下既有的 `requested`／`desktop-quit` marker 仍原封保留。只有一個成功 commit 完整 barrier 的新顯式 start 可處理錯誤後
清掉 pending 與 consumed；supervised automatic retry 永遠無權清。正常 desktop quit 使用較弱且獨立的
`desktop-quit`，下一個全新 opt-in Windows login 只可在五分鐘 bounded handoff 與完整
start barrier 後清這一種；
人工 `requested` 與 `consent-revoked` 都保留。`consent-revoked` 記的是撤回同意的停止
條件；不能被文案拿來反推 `consent.toml` 已成功撤回。

撤回 `local-recording` 的 CLI 與 desktop 會在同一個 exclusive `consent.lock`
transaction 裡、寫 `consent.toml` 前先 atomic publish `consent-revoke.barrier`，並 exact
readback。這道 barrier 不由 recorder 消費，也不准任何 Start 清除；因此 consent save
在 replace 前失敗時，舊 consent 即使仍有效，等待中的 Explicit Start 也只能在 writer
放鎖後讀到 barrier 並失敗。只有成功 commit 的 local-recording regrant 可用先前捕捉的
generation ticket 清掉同一代；若較新的 revoke 已換代，舊票回 `Superseded` 而不刪。
清 barrier 前若沒有其他 stop，會先 atomic 留下 `consent-revoked` stop latch；若已有
`requested`／`desktop-quit`，則一個位元也不改。這讓快速 revoke→regrant 仍先停止目前
recorder，而且重簽本身不是自動 restart。

任一檔讀不清時 automatic／supervised start fail closed。舊版把一顆無類型 `stop` 在
開機時直接刪掉的說明已不再是現行行為。
每個 start 的鎖順序固定是 shared `consent.lock` → `stop.lock` → `recording.lock`／
舊 heartbeat barrier。revoke writer 先拿 exclusive consent lock 時，start 不可 clear、spawn
或寫第一拍；start 先拿 shared guard 時則線性排在 revoke 前。Desktop 真人 Start 的
parent 在 shared consent guard 下進同一個 stop transaction、暫時取得 recorder lease，
通過 heartbeat 並 commit 後才清；它只持 guard 到 `Command::spawn` 回來便立刻放掉，不等
heartbeat。child 仍是 supervised，自己 nonblocking 重拿 consent guard 與整場 lease，並在
第一拍前重讀兩道狀態，所以晚到的 Stop／Quit 不會再被 child 清除。

`desktop.log` 裡**沒有任何一個字來自螢幕**——只有這個殼自己的事：熱鍵搶到了
沒、視窗開不開得起來、資料庫花了幾毫秒打開。它存在的理由是出貨的
`sister-desktop.exe` 沒有主控台，所以所有「講得出原因」的那幾句話原本都是講給
空氣聽的。上一輪的留成 `.log.1`，因為要看的多半正是當掉那一輪，而找記錄檔之前
一定得先把她重開。她卡住或行為怪怪的時候，這兩個檔案就是可以直接貼給我的東西。

`record.log` 是同一個理由的另一半。從字母人按「開始記錄」跑起來的那個 `record`
是**沒有主控台視窗**的（不然每次按下去都會彈出一個黑框，而那個黑框被關掉就等於
她被殺掉），所以它印出來的每一句話都得有地方去。**這個檔案裡會有螢幕上的東西的
統計，但沒有內容**：tick 數、保留／重複／排除的筆數、讀到幾行字、CPU 與 RAM ——
和你直接在終端機裡跑 `sister record` 看到的完全一樣。她起不來的時候，最後幾行
會直接顯示在字母人身上，不必去翻這個檔案。

`capabilities.json` 是把上一段那個問題修掉的東西。**排除規則最糟的失效方式不是
漏寫，是寫了一條自以為有效的**——而「這台機器讀不到瀏覽器網址，所以你那幾條
`excluded_urls` 一條都不生效」這句話，以前只印在 `record.log` 裡。你是在設定頁上
打那些規則的，那句話該出現在那一頁。能力探測只有 recorder 做得到（設定頁在另一個
行程裡，它沒有 UIA），所以 recorder 開機時留一份，錄製中再每分鐘更新
只有那一場看得到的證據。

裡面**存的是原始能力，不是結論**：上一場錄製開始的時候你可能一條規則都還沒寫，
那時候的結論會是「沒問題」，而你正是現在才在打第一條。結論每次都拿眼前這一刻的
規則清單重算。也刻意不放進資料庫——`sister forget` 會清掉 `system_events`，而這
不是一段記憶，是一台機器的事實；被 `forget` 帶走的話，設定頁會安靜地變回
「看起來沒問題」。

能力不用 `true` / `false` 兼差「沒量到」。新報告寫 `available` / `unavailable` /
`unknown`；舊報告沒有記下輸入 hook 的正向成功證據，所以舊的
`input_hook_failed = false` 升級後是 `unknown`，不會自動補成「已驗證可用」。

    Windows   %APPDATA%\ted-h\AI-Sister\data\
    Linux     ~/.local/share/ai-sister/

（實際路徑由 `directories` 依平台慣例決定，`sister doctor` 的「資料目錄」
那一行印的永遠是真的那一個。）

設定檔不在上面的資料目錄裡。Windows 預設在 `%APPDATA%` 底下的
`ted-h\AI-Sister\config\config.toml`；它是這台機器的設定，不是某一段記憶。
`--data-dir` 不會搬動它，`sister forget` 不會刪它，記憶匯出也不會帶走它；
CLI 可以用全域 `--config <FILE>` 明確改讀另一份，字母人則讀預設位置。
同目錄的 `config.toml.write-lock` 是內容為空、會保留在磁碟上的跨行程寫鎖；它讓
設定頁、語音開關、網址政策與熱鍵各自更新一格時不會把別人剛寫好的欄位蓋回舊值。
它不含設定值；刪掉會在下次寫入重建，但正在有行程持鎖時手動 unlink 會破壞鎖語意。
`[hands] url_open` 也住在這裡：沒有這一欄是**還沒問過**，不是某個預設答案；
答過之後只會是 `only-on-my-press` 或 `when-you-can-name-the-origin`。所以刪掉記憶
不會順便改變這個選擇，刪掉或移走設定檔才會讓它回到「還沒問過」。

`[shell.persona]` 也住在 `config.toml`，保存角色顯示開關、穩定 ID、動態效果、
tap-lines 與聲音偏好。這些值會改本機呈現，**不會進 asset request**；17 位角色與
所有狀態的 method／URL／headers／body 必須相同。Reel/WebP 都是 bundled 本機
檔，切人只讀所選角色的圖層，不產生網路 request。刪記憶不會重設它們，刪設定檔才會
回到 ChatGPT／預設值。舊設定裡的 exact `neutral` 會遷移成 ChatGPT 並關閉聲音；
其他未知 ID 仍拒絕。素材 cache 本身也不另存一份「目前選誰」。

`[shell.azure_tts]` 也在 `config.toml`，但只存**非機密**設定：`enabled`、`region`、
`voice`。預設是 `enabled = false`、沒有 region、voice 是
`zh-TW-HsiaoChenNeural`；它不會因 Persona 的 `voice_enabled` 或找不到本機 voice
自動打開。使用者明確打開且其餘三道 gate 齊全後，每份最新新答案完成時會自動朗讀
一次；關閉操作成功回覆後便不再開始新的請求。若 transport 已取得送出權，操作可能
等待完成或最長 45 秒逾時，見本節前述 fence 邊界。region 只接受 `eastasia`、`southeastasia`、`japaneast`；voice 只接受
`zh-TW-HsiaoChenNeural`、`zh-TW-HsiaoYuNeural`、`zh-TW-YunJheNeural`。這些 typed
值決定 fixed endpoint／SSML voice；未知值或多餘欄位讓設定讀取失敗，不猜預設、不接受
自訂 URL。舊設定缺整段時遷移成上述 disabled／unconfigured 預設。

Azure subscription key **不在 `config.toml` 或資料目錄**。它存在目前 Windows
使用者的 Credential Manager generic credential，fixed target
`ted-h/AI-Sister/AzureSpeech/v1`、username `Azure Speech subscription key`；設定頁
只能看到 Present／Missing／Unreadable／Unsupported，不能讀回 key。`--data-dir`、
memory export、`forget`、`prune`、Persona 撤回與刪除 `config.toml` 都不搬、不複製、
不刪這筆 credential；要由 Azure 設定裡的刪除動作明確移除。這也表示只刪資料目錄或
設定檔**不等於**刪掉 Azure key。

Windows 的「登入後啟動」**不在 `config.toml`**。它是目前使用者 registry 的
`HKCU\Software\Microsoft\Windows\CurrentVersion\Run` 下一個名為 `AI-Sister` 的
`REG_SZ`；唯一有效內容是 `"<目前安裝的 sister-desktop.exe>" --ai-sister-login`。
它不含 OCR、畫面、問題、資料目錄或記憶 ID，但會暴露目前使用者的安裝路徑。
Run 缺值才是 `disabled`；exact value 是 `enabled`；其他可讀值／型別是
`mismatch`；無法讀是 `unreadable`；目前副本無法用下述安裝 metadata 精確
證明時是 `unsupported`。這五種不壓成 bool，也不由「設定檔剛好讀壞」
影響。

同一位使用者的
`HKCU\Software\Microsoft\Windows\CurrentVersion\Uninstall\AI-Sister\InstallLocation`
是 installer 留下的安裝 metadata；desktop 只讀它，而且只有它的 quoted directory 精確
對應 current exe parent 才准修改 Run value。portable／診斷副本不寫這兩處。
真正 uninstall 會清掉 `AI-Sister` Run value；update／reinstall 不應把使用者的
on／off 選擇清成預設。AI-Sister 不讀寫 Windows 另一份 `StartupApproved`
metadata；Windows 「啟動應用程式」對它的狀態不是 app 這五態所稱已儲存的資料。

recorder supervisor 的 generation、desktop-owned child handle、連續失敗次數、
1／5／30 秒 retry deadline、10 分鐘 `Recording` 健康區間、Login preflight 的 500ms
retry／五分鐘 deadline、已觀察 owner、同一 worker one-shot、取消／逾時 terminal 文案，
以及 manual Stop 的 automatic-start cancellation／durable-delivery pending-or-failure overlay，
全在 **desktop 行程 RAM**。這些欄位不是「已成功寫入 stop」或「recorder 已停」的磁碟
證據；supervisor 自己不把這些 counter／timer 寫進檔案或 DB table。alpha.107 仍新增
三個空的跨行程協定檔：`recording.lock`、`consent.lock` 與 `stop.lock`，語意與刪除邊界
逐一列在上表；它們不是 supervisor counter 的持久化。desktop
結束就沒有 supervisor RAM 狀態；系統也不會因此自行重開 desktop。

Login preflight 每 500ms 完整重查，最長五分鐘且不吃 watchdog failure budget。只有
磁碟上 typed `desktop-quit` 可做上一輪 shutdown 的 bounded handoff；Absent 下曾看見
lease／heartbeat owner 就轉 External，同一份 intent 不 takeover。一般 External 的 stale／
missing／unreadable 仍只由 `stopped` 墓碑完成離場；DesktopQuit 是窄例外。真人
Stop／Quit／Explicit Start、`requested`／`consent-revoked`、invalid／unknown
consent／control 會清掉 RAM 裡 pending preflight；同一 worker 之後的 duplicate Login
仍忽略。deadline 在重試前和 commit 前都檢查，所以 expired timer 不能 late spawn。

十分鐘區間只由新鮮且相對上一個已觀察 `(timestamp, phase)` 樣本真的更新的
`Recording` heartbeat 推進；worker 在五秒間反覆讀到同一顆不算新證據。一旦觀察到
missing／stalled／unreadable／`Thinking`，區間就歸零，下一顆 Recording 從頭起算。
supervisor 讀已列在上面的 `consent.toml`、`stop.request`／`stop.consumed` 與
`recording.beat` 來決定能否啟動／重試；任一關鍵狀態不明時不開第二個
recorder。spawn 後仍由 recorder 自己在每道寫入邊界強制上面的 pause 三檔；
supervisor 沒有 resume 能力。重試的 child 繼續把 stdout／stderr 寫進既有
`record.log`／`record.log.1`，沒有新的螢幕內容副本或網路請求。

desktop quit 會先寫 durable `desktop-quit` latch，才可以真正退出。這一寫失敗時，
supervisor 的記憶體狀態仍轉成 `Quitting`、清掉 retry deadline，但 desktop 本身留下、
顯示錯誤，不接新 start；它不會在無法證明 recorder 會停時只把 UI 關掉。
Manual Stop 也在 durable write 前先清 Login／retry 並設 automatic-start cancellation；
write 失敗時 failure overlay 明講 recorder 可能仍在跑，使用者可再 Stop，但 background
不得重開。只有後來真人 Explicit Start 成功 commit 完整 barrier 才清 cancellation／failure；
invalid／busy／timeout Start 不會。

沒有遙測、沒有產品帳號。`sister.exe` 與 recorder／core／capture／brain／hands 沒有
HTTP client；desktop 只有兩條內建 outbound：使用者看完揭露並明確按下後，經
`sister-assets/download` 對固定 Persona pack 發至多一次 GET；以及 Azure 設定、
Credential Manager key 與現行第四張 consent 都成立後，經 `sister-tts/azure` 只替最新
新答案或 trusted 手動重播，對 `eastasia`／`southeastasia`／`japaneast` 其中一個 fixed
endpoint 發一個 POST。
後者唯一的使用者內容是當前答案正文原文，可能含姓名、電話與金額且不遮罩；不含截圖、
來源連結、memory id、DB 或其他文字。它不做 cache，cancel 只阻止 late audio 播放，
無法 abort 已開始、最長 45 秒的 blocking POST。簽了第二張同意書且
設定了 `[brain] command` 之後，螢幕文字原文會交給那支本機 CLI；外送紀錄在
`brain_outbound`（結構與計數，不含原文；`role` 分解釋層／審閱層／盯梢層——
`interpreter`／`reviewer`／`watcher`，最後一個是 alpha.71 的 `sister watch`；
送出去的是原文），假設卡片在 `l2_card`（append-only 版本鏈，`author` 是 interpreter／reviewer／user，刪 L0 時 tombstone 而不是實刪——列留著，
卡片上的字清掉）。桌面時間軸的「外送」頁讀這兩張表和 `meta.ever_brain_outbound`。

Microsoft 目前公開列 Azure Speech F0 neural TTS 每月 0.5 million characters；這是
帳號／resource／方案層的 provider 額度，不是本機資料，也不寫進 DB。能否使用與費用
仍以使用者帳號、方案及 Microsoft 當下規則為準，AI-Sister 不保存或保證剩餘額度。

L3 只由 Reviewer 寫入：`commitments`（承諾表，status 為 open／done／dead／snoozed／archived，`due_source` 分螢幕上寫的和她猜的）、`entities` 與 `entity_mentions`、`day_summaries`、`preferences`（例如哪一類被「其他一切」降權）。承諾的 `allowed_next_step_fact` 是下一步所引用的 L1 fact id；`agreed_evidence_json` 是兩個 reviewer pass 共同引用的 evidence refs。血緣在 `provenance(child_ref, parent_ref)`。審閱層有沒有跑過、回查了幾次，記在 `reviewer_run`／`reviewer_recheck`——`calls_used` 是嘗試呼叫數，nullable 的 `answers_got` 才是實際取得答案數；`detail` 是拒絕，`notes` 是其他說明。雙 pass 對不上的那幾筆在 `reviewer_divergence`，分歧不寫入 L3。

守門員每一個「要不要現在開口」的候選都在 `utterance`：候選文字、evidence refs、
四個評分輸入與總分、最後是 `spoke` 還是 `held`、呈現形式／點數或壓住的理由，
以及使用者之後的反應。忘掉來源 L0 時，衍生列會 tombstone，文字、證據與反應一起清掉。

---

## `replay-drafts/` — 真實工作日的私有重播草稿

`sister replay export --last 24h` 把一段真實記錄寫成 replay corpus JSON。
沒有指定 `--to` 時，私有草稿放在 `<資料目錄>/replay-drafts/`；檔名會帶
`.sister-replay-draft.json`，repo 也明確忽略這個副檔名和目錄。它是使用者的資料，
不是上面那組可以隨手貼出來的狀態檔。

匯出後只有下列東西：

- 相對時間與重播管線所需的 L0 事件；真實 epoch 時間不進語料。
- 自動去敏後的螢幕文字、視窗脈絡、剪貼簿文字與輸入計數。
- **零圖片**：不複製 `frames/`，不寫截圖位元組，也不寫 `image_path`。
- **零來源路徑**：corpus schema 沒有來源資料目錄、資料庫或圖片路徑欄位。OCR
  文字仍可能含程式規則不認得的路徑或代號，所以人工審查不能省。

Draft 的「自動去敏完成」只表示已跑過程式能辨認的規則，**不表示安全、
也不表示可分享**。人名、內部案號、公司用語和對話都可能留在看似普通的文字裡。
匯出時 JSON 頂層的 `review` 是 `draft`；只有人工逐項審查後把它標成 `reviewed`
的 **Reviewed** corpus 才可分享。

Draft 是純文字 JSON，沒有額外加密；它放到哪裡，就只被那個磁碟與目錄的權限
保護到哪裡。

`sister replay import <corpus> --dry-run` 可以把 Draft 匯入記憶體資料庫做本機驗證；
未 Reviewed 不影響本機重播，但仍不可分享。import 從 corpus 裡去敏後的 L0
重建 `text_chunks` / FTS 索引與 `facts` L1，不把匯出當時的衍生表當作不可質疑的答案。
原本的 `sister replay scenarios/bill-lookup.json` 指令繼續存在，和 corpus import
同樣都是本機、零連網。scenario JSON 必須明寫 `privacy_context` 與 `system_state`
安全前提；缺欄會拒絕執行，不能默認成「已知安全」。

### replay 題庫與評測報告

`sister replay export --last 14d --to <corpus> --questions-to <questions>` 會在匯出
corpus 的同時，讀本機 `queries`、`query_clicks` 計數與 `query_marks`，把同一個
`[from, to)` 時間窗的問法依 `(ts, id)` 舊到新排成 private Draft。重複問句仍是
不同實例；portable id 是 `query-0001` 這種檔內流水號，`asked_at_ms` 是相對時間，
不帶 SQLite row id 或真實 epoch。輸出檔拒絕覆寫，在 Unix 是 0600；
`*.sister-questions-draft.json` 也被 gitignore 擋住。

每題的 `observed` 只照實保留當時的 shape、產品回傳筆數、介面、點開出處數與 ★
標記。這些都不是 ground truth：尤其回 0 筆不能自動變成 NoAnswer。所以匯出時
`expected` 固定是 `null`，人工要填成 `answer`（含 corpus `event_index`）或
`no_answer`；沒填完不能 evaluate，逐題審查後才把題庫 `review` 改成 `reviewed`。

`sister replay questions status <corpus> <questions>` 只讀兩份 JSON、核對 fingerprint，
並印 answer／no_answer／未標的三個實數，不印題目原話。`annotate` 才會在明確的
互動流程裡顯示原問句、observed 提示與產品 `facts` 檢索候選；`f` 搜尋和 `e` 打開的
是 validator 同一套 evidence surfaces。它不從候選或 observed 自動套標，只接受人
輸入的 `a EVENT 答案`／`n`。結果用 `--to` 寫進另一個 create-new、Unix 0600 的
Draft，來源檔不變，目的地存在就會在互動前拒絕；每完成一題就同步完整 Draft，
中途 `q` 或之後的終端錯誤仍留得住上一個完成的 checkpoint。零變更正常離開不留檔。

`sister replay questions review` 重新驗完整題庫、corpus fingerprint 與每個 evidence；
缺任何標註就不建立輸出。它還要求 `--confirm-private-text-reviewed`，由執行者明確
確認題目原話與答案已人工審查，才另存 `review: reviewed`。這個狀態仍只屬於題庫，
不會改 corpus 的審查狀態。

`sister replay evaluate <corpus> <questions> [--k K] [--runs N] [--json | --to FILE]`
讀這份 question-set JSON。題庫自己也有 `review: draft | reviewed`，不能借用
corpus 的 Reviewed 狀態；每題明列 `id`、問題文字、來源（`query_log`、
`hand_labeled` 或 `planted`），以及 `answer` 的可接受字串與 corpus `event_index`
出處，或明列 `no_answer`。因此用真實 query log 做成的題庫本身也是使用者資料，
要和 corpus 分開人工審查。匯出器保留問題原話、**不自動去敏**；
`privacy.query_log = false` 只阻止之後新增，不刪舊題，而已被 `forget`／保留期
拿掉的題目也不可能由匯出器補回來。

repo 目前簽入的 `scenarios/recall-baseline.corpus.json` 與
`scenarios/recall-baseline.questions.json` 是純合成 Reviewed fixture：3 個事件、
5 題，其中 query log 0 題、人工標註 3 題、腳本埋題 2 題。它只守 runner 接線，
不是使用者資料，也不是 Phase 2 的 ≥100 題公開 baseline。

沒有 `--json` 或 `--to` 時只印人讀摘要，不自動保存完整報告。`--json` 把完整
report 寫到 stdout；`--to` 寫一個新檔，目的地已存在就拒絕覆寫。完整 report 包含：

- evaluator／輸入格式版本、corpus 與題庫名稱、兩者各自的 review 狀態、參數和兩份輸入指紋。
- 每題的問題文字、來源、判分、延遲，以及每個回傳項目的 channel、相對時間、
  source kind、值與 corpus `event_index`。fact 會帶 raw／normalized 值，文字結果
  會帶完整的去敏後文字。
- `baseline_text` 和 `facts` 各自的找回率@k、答案／出處正確率、延遲分布。
  沒有模型路徑的配置 `model.kind = not_on_path`（不是量到 0 次呼叫）。
  開了 `--ab` 的 `interpreter_reviewer` 才從 `brain_outbound` 數呼叫、
  用 SPEC §13／`research/cost-model.md` 的 Haiku 4.5 單價換算金額。
- 尚未量到的提醒誤報／漏報、斷句 F1、Reviewer 回查率、CPU、RAM、電池與磁碟是
  `null`，不是 0；沒有適用題目的比例也因為沒有分母而是 `null`。

報告會重複題目和檢索回來的文字，所以不因為叫「report」就變成低敏資料。輸入是
private Draft 時，報告會保留 Draft 狀態並在 CLI 顯示警告；這份 report 一樣只能
留在本機，corpus、題庫與報告都人工審查完成前不要分享。輸入是 Reviewed 也只代表
corpus 已審過，不能替另一份題庫或新產生的報告自動背書。

桌面開發者模式的「評測指標…」不新增任何常駐檔案。它預設隱藏；設定
`[shell] developer_mode = true` 並重開桌面後，人可以從原生選檔器挑一份既有 report。
完整 JSON 會短暫進入本機 WebView，再經同一行程 IPC 交給 Rust 依 report schema
嚴格解析；前端接著保存和渲染的數值 projection 只有：

- report format、corpus／題庫各自的 review、事件／題目數、來源數與 duration，
  以及 k、warmup、runs。
- 每個配置的三個 fraction、aggregate latency、模型呼叫／成本、尚未量到仍為
  `null` 的 footprint／提醒／斷句／回查指標，以及任一 QA 指標未通過的 1-based 題號。

projection **沒有任何來自 report 的自由字串**：corpus／題庫名稱、fingerprint、
ranking、題目 id、每題 question 與 returned values 都不過這道邊界。這不會改寫
原 report：頁面不上傳，也不另存磁碟副本，關閉後沒有多一個要清理的檔案。原檔
依然含上面列出的逐字內容；Draft report 載入後會一直顯示 private Draft 警告，不能
拿 projection 比較乾淨這件事替原檔通過人工審查。

---

## 快速回答

| 問題 | 答案 |
|---|---|
| 有存我按了什麼鍵嗎？ | **沒有。** 只存計數與節奏，見 `input_metrics` |
| 有存螢幕截圖嗎？ | 有，降採樣後的 PNG。Windows 抓的是前景所在的整個 monitor，可見背景視窗也可能入幀；可用 `store_images = false` 關掉 PNG，但 OCR 仍會讀工作幀 |
| 有存我複製的東西嗎？ | 有，但疑似秘密者只存「發生過」，不存內容 |
| 有存密碼嗎？ | 前置檢查確認焦點在敏感欄時不讀內容；所有前景 app 都問，問不出來也不放行。密碼管理員另由 app 規則整段排除；可見背景視窗與換窗 race 見下方「已知缺口」 |
| 網銀畫面呢？ | **前景**網址命中 blocklist 時不讀內容——但背景視窗與 browser clipboard 的邊界見下方「已知缺口」 |
| 有存我**問過她**什麼嗎？ | 有——`queries`，只在這台機器上。可用 `privacy.query_log = false` 關掉 |
| 資料會離開這台機器嗎？ | 畫面 pixel 不會。簽 `cloud-reading` 後，OCR 文字原文會交給你設定的本機 CLI；那支 CLI 是否送給 provider，由它自己的設定與行為決定。另有 Persona fixed GET；以及預設關閉的 Azure TTS：只有設定、Credential Manager key 與現行第四張同意都成立時，才把每份最新新答案的正文原文自動 POST 一次到你選的三個固定 Azure region 之一；trusted 手動重播會再 POST。正文可能含姓名、電話與金額且不遮罩，不含截圖、來源、memory id、DB 或其他文字 |

---

## 表一覽

### `frames` — 保留下來的畫面

| 欄位 | 內容 | 敏感度 |
|---|---|---|
| `ts` | 毫秒時間戳 | 低 |
| `monitor` / `width` / `height` | 哪一台螢幕、多大 | 低 |
| `dhash` | 感知雜湊（去重用，無法還原畫面） | 低 |
| `image_path` | PNG 相對路徑；`NULL` = 這一幀沒有圖（見下） | — | — |
| `image_bytes` | 檔案大小 | 低 |
| `dup_run` | 這張畫面連續重複了幾次 | 低 |
| `app_id` / `window_title` / `url` | 當下的脈絡 | **中**：標題與網址常含人名、案號、單號 |

畫面檔本身是**最敏感的東西**：螢幕上有什麼，它就有什麼。預設保留 30 天，
到期後 PNG 真的會從磁碟上消失，但這一列與它的文字留到 `text_days`（預設
365 天）——截圖和「三個月前那通客服電話」不是同一件東西，不該綁在一起刪。
清理在每次錄製開始時自動跑，之後每 6 小時再跑一次，也可以隨時
`sister prune`（`--dry-run` 先看）。

> **兩個保留期不一樣，所以「每天用多少磁碟」有兩個分母。** 用滿一年之後，
> `image_bytes` 只涵蓋最近 30 天，而 `text_chunks` 的時間範圍是整整 365 天
> ——拿前者除以後者，是三十天的分子配一年的分母，`sister stats` 的
> 「每天約」會印成實際的十二分之一。那句話是 Phase 0 退出條件（< 300 MB/天）
> 的判決，而錯的方向剛好是「看起來過了」。所以兩半各除以自己的跨度再相加，
> 而畫面那一半不到半天就整句不外推（`DbStats::image_first_ts`）。

> **`sister prune` 說「刪掉了 N 個畫面檔」，數的是 `remove_file` 成功幾次。**
> 資料庫說有幾列帶圖是另一回事：手動清空過 `frames/`、或從一份沒帶
> `--with-frames` 的備份還原之後，那個差額會單獨印成「另外 N 列說自己有圖，
> 但那個檔已經不在磁碟上了」。不是 ⚠——東西確實不在了，隱私上沒有缺口——
> 但它和「順利釋放了 1.2 GB」是兩件事。

> **`0` 天會被拒絕，兩道門都是。** logrotate、journald、docker 那邊 `0` 是
> 「不限制」；在這裡它的意思正好相反——下一次整理就把那些東西全部刪掉，而
> 整理在每次錄製開始時自動跑。也就是說照那個習慣寫下去的人，會在**她開始錄
> 的那一刻**失去全部記憶，而且畫面上不會有任何一行字提到這件事。兩種讀法都
> 合理，所以 `Config::load` 和 `Config::save` 都擋（`RetentionConfig::check`），
> 設定頁那兩格也擋。想留久一點請寫一個大的數字：`36500` 大約是 100 年。

**不是每一幀都有圖。** `image_path` 是 `NULL` 的原因有三個：

1. `store_images = false`（text-only 模式，第三張同意書關掉的那個）
2. **畫面檔節流**：距離上一張圖不到 `capture.image_min_interval_ms`
   （預設 5 秒），或今天的畫面額度 `capture.max_image_mb_per_day`
   （預設 250MB）已經用完。文字、事實、脈絡照常全部寫入，跳過的只有 PNG。
   額度以 UTC 天計，而且**啟動時會從資料庫接回今天已經用掉的量**——
   否則關掉再開就重拿一份，那個上限就形同不存在。
3. 這一幀的圖已經過了 `frames_days` 被清掉了

第 2 點是磁碟預算的主要手段，也是一個刻意的取捨：實測不節流時是
**11.4 GB/天**，而預算是 300MB/天。少存圖不會讓你少搜到任何一句話——
搜尋打的是文字索引——但**點進某些結果時會沒有圖可看**。這是設計，不是壞掉。
`sister record` 的摘要會直接講「另外 N 張只留了字」。

**`text_chunks.frame_id` 不等於「有圖」。** 它講的是「這段字抄自哪一幀」，
是出處，一直都在；上面那三種情況下 `image_path` 是 `NULL`，但 `frame_id`
照樣有值。第 1 種情況下**每一筆**都是這樣。所以字母人在把答案送上畫面之前
會先問一次 `frames_with_image`，沒有圖的就不給「點開看當時的畫面」——
`sister query --json` 給的 `frame_id` 則是原本的意思（出處），因為終端機上
沒有可以點的東西。抄這個欄位去做 UI 的人要知道這條分界（alpha.21）。

### `ocr_blocks` — 畫面上的文字（含位置）

`text` + `x/y/w/h` + `confidence`。**敏感度最高**，因為這是螢幕內容的可搜尋副本。
外鍵 `ON DELETE CASCADE`：刪掉一張 frame，它的文字跟著消失。

一列是**一行字**，不是一個詞。Windows 的 OCR 給的是逐詞的方框，我們用詞與詞
之間的間距把它們組回一行——不能用引擎給的整行字串，因為那是用空白接的，
中文會變成「本 期 應 繳」。詳見 `crates/sister-capture/src/ocr_layout.rs`。

> `confidence` 目前一律是 **`-1`**，代表「這個引擎不回報信心度」。
> 用 -1 而不是 1.0 是刻意的：萬一以後有人寫了 `WHERE confidence > 0.5`，
> -1 會讓所有文字一次全部消失（立刻被發現），1.0 則會是一個永遠沒有人
> 發現的謊。

### `text_chunks` + `text_fts` / `text_fts_uni` — 搜尋索引

一張 frame 的所有 OCR 文字接成一段（`\n` 相接）存成一列，這樣「本期應繳金額」
不會因為被切成兩個區塊而搜不到。兩個 FTS5 external-content 索引跟著同步：
trigram 給中日韓、unicode61 給英文。

> **注意**：FTS5 索引是文字的**另一份副本**。刪除必須經過 `text_chunks`
> 的觸發器，直接 `DELETE FROM text_chunks` 以外的路徑會留下孤兒索引。

### `focus_events` — 換到哪個視窗

`kind`（focus / title_change / url_change）+ `app_id` / `app_name` /
`window_title` / `url` / `pid`。脈絡變了才寫一列，不是每秒一列。

> **alpha.100 起，這張表也會替無人值守的 URL 回答「她說不說得出來源」；
> alpha.103 起只信 v2。** 不是每一列 `url` 都有這個資格：查詢會 join `sessions`，
> 只信 exact `platform = 'windows/windows-gdi-uia-focused-url-v2'` 的保留中真 Windows
> recorder session。歷史 `windows/windows-gdi-uia-focused-url-v1` 列仍可讀、可顯示，
> 但不再能當來源票；舊 `windows/windows-gdi`、corpus import、scenario replay 也不行。
> 比對只到 host，最多把一層 `www.` 視為同站；它不證明網站安全或是你主動開的，
> 也不證明 path、redirect 或站內內容。

### `clipboard_events` — 複製了什麼

| 欄位 | 內容 |
|---|---|
| `kind` | text / image / files |
| `text` | 內容。**疑似秘密時為 `NULL`** |
| `byte_len` | 長度（即使內容沒存也記，因為長度本身無害而有用） |
| `truncated` | 是否超過 64 KB 被截斷 |
| `secret_suspected` | 1 = 偵測到疑似秘密，內容**沒有**落地 |

> 這句承諾**當場查得出來**。`sister stats` 不是去數 `secret_suspected` 這面
> 旗子插了幾次——旗子是我們寫入時的自我宣稱——而是去問資料庫「插了旗子的那
> 幾列，`text` 到底還在不在」。旗子插了但字還在，是這句話唯一的失敗方式，
> 而且那種失敗不會報錯：錄製摘要照樣會印「內容未落地」。
>
> 這條查證路徑同樣到 alpha.8 才接上。在那之前，讀 `secret_suspected` 的只有
> 一個單元測試。
| `source_app` | 從哪個程式複製的 |

只存文字類內容。圖片與檔案只記「發生過」與來源，不存本體。

`source_app` 是 clipboard owner 的程式，不是下一拍前景 app；事件沒有一個可持久化的
來源 URL／視窗標題欄位，也不拿當下前景補值。因而使用者在瀏覽器複製後立刻切到
編輯器時，資料庫無法證明來源頁面。alpha.103 在 `excluded_urls` 非空、來源是瀏覽器、
但沒有 origin URL proof 時，整筆內容在 insert 前丟棄；不寫一列假來源來填這個空洞。

### `input_metrics` — 打字節奏（**不含內容**）

`keystrokes` / `clicks` / `mouse_px` / `scroll_ticks` / `window_switches` /
`idle_ms` / `typing_bursts`，預設每 10 秒一列。

`ts_end` 索引的磁碟成本已用合成資料實測：3,000,000 列
（預設每 10 秒一列 × 保留 365 天的上限）建立索引花 **0.36 秒**，
資料庫 **130 MB → 174 MB（+45 MB，每列 15 bytes）**。
`brain_outbound` 的新索引成本可忽略：這張表只有 `sister forget`
會刪，一年只有幾萬列。

這是「事後補不回來」的訊號：卡住、專注、焦慮都藏在這些數字裡。
Windows 的鍵盤 hook **從未解參考 `KBDLLHOOKSTRUCT`**——按鍵碼從來沒有進入
過這個程序的記憶體。這比「我們有記得過濾」強，因為它不需要你相信任何人。

> 話雖如此，這句話一直到 alpha.9 之後才真的不需要相信任何人：在
> `scripts/check-no-keylogging.py` 之前，守著它的東西是**零**——沒有測試、
> 沒有 lint、沒有 CI 檢查。加一行讀 `vkCode` 的程式碼，整套測試照樣全綠。
> 一句「不需要你相信任何人」的承諾，如果只能靠寫的人記得，那它要求的
> 信任比它宣稱的還多。

### `input_health` — 安靜的那一段，她聽不聽得見

`ts_start` / `ts_end` / `session_id` / `state`，只在**一個視窗內四個計數器
全都是 0** 的時候寫一列（有人在動就寫 `input_metrics`，不寫這張）。
所以它記的是「沒有輸入」這件事，不是輸入本身——這張表裡沒有任何按鍵、
座標或內容。

`state` 三種：

| 值 | 意思 |
|---|---|
| `idle_confirmed` | hook 正常，而且作業系統（`GetLastInputInfo`）說整個視窗都沒人碰 |
| `not_listening` | hook 沒裝起來／裝失敗，或是作業系統說那段有人動過而我們一個事件都沒收到（hook 被系統靜默拆掉的指紋） |
| `unknown` | 問不出作業系統的閒置時間 |

**為什麼要多一張表**：alpha.97 對「沒有那一列」講的是最謙虛的那一句
（「可能是沒人動，也可能是那一小段沒被記進來，我分不出是哪一種」）——那是
誠實的，但也表示**有把握的那一句在真實錄製上永遠印不出來**：它要的是一列
四個計數器全 0 的紀錄，而 Windows 那邊從來不寫那種列。

而讓它印得出來的**便宜作法會製造一句更糟的謊**：把「沒有那一列」直接當成
「沒人動」。因為那個沉默有兩種——真的沒人碰，和她自己沒在聽。
`LowLevelHooksTimeout` 會讓 Windows **無聲地**把 hook 拆掉：沒有錯誤、
沒有事件，`HookState::Active` 只代表「當初裝上去了」。所以這張表存的不是
「沒有輸入」而已，是「沒有輸入，而且我當時聽得見／聽不見」。

**隱私面**：一串連續的 `idle_confirmed` 等於「這段時間他不在電腦前」。
這是關於他的資訊，所以它跟 `retention.text_days` 一起過期、`sister forget`
會照重疊區間刪掉。

它帶著 `session_id`，但**不算**在「這一場錄製還剩不剩東西」裡（見
`retention::content_only`）：一場只剩下這幾拍錄製日誌的錄製要清得掉，而那幾列
跟著那一場一起走。沒有這一條的話它會反過來卡住——外鍵是 `ON` 的，那一列
`sessions` 就永遠刪不掉，而 `stats` 會對一場乾淨收尾的錄製印
「工作階段 1（空殼：⋯她當掉了）」。
它**不算**「她記下來的東西」（不在 `CONTENT_TABLES` 裡）：那是錄製自己的
日誌，一顆只剩這張表的資料庫應該說「從來沒存過」，不是「存過但被忘掉了」。

### `facts` — L1 抽取出來的事實

`kind`（money / phone / url / email / file_path / error_code / id_like /
datetime）、`raw`（螢幕原文）、`normalized`，以及回指
`chunk_id` / `frame_id` / `app_id` / `window_title` / `url` 的出處。

**純 regex，零模型。** 敏感度等同來源文字：螢幕上有電話，這裡就有電話。

> **schema v2 拿掉了 `confidence`。** 這一欄以前每條規則手寫一個 0.85～0.97
> 的數字，寫進每一列、印在畫面上（「信心 0.93」）、出現在 JSON 裡——然後
> 沒有任何一行程式讀它。連它自己的註解宣稱的用途（規則搶同一段文字時決定
> 誰贏）都是假的：去重讀的是 `kind` 的優先序。把所有值改成 0.5，99 個測試
> 全過。
>
> 一個沒有來源、沒人讀、卻長得像機率的數字，比沒有這個數字更糟。等 Phase 1
> 有了重播評測集、真的量得出每條規則抽對的比例，它再帶著一個有來源的值回來。
> （升級是自動的：`sister` 一開資料庫就跑 migration 002。）

### `system_events` — 錄製邊界與原生系統轉換

`kind`（session_start/end、lock、unlock、sleep、wake、capture_paused/resumed、
**excluded**）+ `detail`。平台來源的型別只能送 lock／unlock／sleep／wake；session、
pause 與 excluded 只能由 recorder 自己寫。Windows 目前只從連續、可觀測的 WTS
**相鄰 polling 樣本**產生 lock／unlock；第一個樣本或 Unknown 空洞後只建立狀態
基準，不捏造精確事件時刻。兩個樣本之間發生又恢復的轉換可能觀察不到，所以這張表
不是每個原生 WTS 事件的完整 event log。Release 1.0 最低 Windows 10；只在 session
同時為 active + unlocked 時讀內容，鎖定與電源正交，wake 不會自己變成 unlock。
sleep／wake 要等真正的 power notification，不能拿時鐘空洞猜。

同一次 observation 的多筆 transition 由一個 SQLite transaction 全寫或全不寫。
若 DB 寫入失敗，recorder 把完整 observation 留在**行程 RAM** 的 pending，下一拍在
新 poll 或讀內容前重試；狀態、timestamp 與 sequence watermark 等 transaction 成功
才一起推進。行程在成功前 crash 時 pending 仍會消失，因此這是 process-lifetime
best effort，不是 crash-safe exactly-once。

`excluded` 這一列是**稽核用的**：它記錄「這段時間因為某規則沒有擷取」，
理由字串裡含 app 名稱、以及**命中的那一條規則**（`excluded url: *password*`）。
這是刻意的——沒有它，使用者無法驗證排除真的生效了。

寫規則、不寫被擋的內容，這條界線是硬的：規則是你自己設定檔裡的那一行，網址
和標題是螢幕上的東西。稽核紀錄躲得過所有排除規則（它就是排除本身寫的），所以
它是整個資料庫裡唯一有機會夾帶內容出來的地方，不能開這個口。

> alpha.20 以前 app 那兩條寫得出名字，網址和標題只寫 `excluded url`。於是
> `sister stats` 事後答得出「excluded url 擋了 812 段」，答不出那裡面有 780 段
> 是預設的 `*password*` 吃掉的技術文件——而那正是使用者要採取行動時唯一需要
> 的那一格。一份說得出「有東西被擋了」卻說不出「被誰擋的」的稽核紀錄，只證明
> 得了系統有在動，證明不了它做對了事。

`capture_paused` / `capture_resumed` 是同一個道理，對象換成使用者自己按的
暫停：一筆進、一筆出，只在**狀態轉換**時寫（暫停三小時是兩筆，不是上萬筆）。
沒有這兩筆的話，資料裡的三小時空洞，和「那三小時什麼都沒發生」，事後完全
分不出來。

`session_end` 的 `detail` 是**這一場錄製為什麼結束**：`duration`（時間到）、
`requested`（你按了停止）、`desktop-quit`（AI-Sister 正常退出時收工）、
`interrupted`（Ctrl-C）、`consent-revoked`（本機記錄同意的停止條件生效；不單憑此
token 宣稱 consent save 已 commit）。沒有
「當掉」這個值——當掉的那一場寫不了任何東西，它的樣子是 `sessions.ended_at`
留在 NULL。字母人那句「沒有人在記錄」底下的第二行、`sister doctor` 的
「上一次錄製」都是從這裡讀的。alpha.17 以前這一欄一律是空的，於是「你自己
按了停止」和「她半夜當掉了」在磁碟上長得一模一樣——而只有後者需要你做什麼。

> 這兩個 kind 從 schema v1 就寫在這份文件裡，但一直到 alpha.12 才有程式碼
> 真的寫得出它們——在那之前它們是這份文件裡的一句空話。列在這裡是為了不讓
> 同一件事再發生一次：**文件寫了、程式沒做**，比文件沒寫更難發現。

> 用 `sister stats` 讀回來（依理由分組，附段數與第一段／最後一段的時間）。
> 這條路徑到 alpha.8 才接上：在那之前這張表是**只寫不讀**的，只有錄製當下
> 的即時統計印得出理由，終端機一關就再也查不回來。一份叫不回來的稽核紀錄，
> 對使用者而言跟沒有稽核紀錄是同一件事。
>
> 存的是**段**不是張數：踏進被排除的 app 待十分鐘只寫一列，出來再進去才寫
> 第二列。所以它答得出「這條規則生效過嗎、什麼時候」，答不出「總共擋掉幾張
> 畫面」——後者只有那一次錄製的即時統計知道，沒有進資料庫。段的**結束**時間
> 也沒有記，所以也算不出被擋掉的總時長。

> **稽核紀錄也會過期。** 這張表跟著 `retention.text_days`（預設 365 天）走，
> 到期整列消失——所以「這條規則去年擋過東西」是問不回來的。這一條寫在這裡是
> 因為它會讓人意外：上面才說了「沒有它，使用者無法驗證排除真的生效了」，
> 而它自己也是關於你的資料，給它一個永久的例外等於在保留期上開一個洞。
>
> 順帶把整份表的規矩講清楚，這份文件以前只說了存什麼、沒說存多久：
> **除了畫面檔（`retention.frames_days`，預設 30 天，到期只丟圖、字留著）
> 以外，其餘每一張表都跟著 `retention.text_days`。** 包含這張、`queries`
> 題庫、焦點／剪貼簿／輸入那四張訊號表（`input_health` 也在內）、
> 以及 `segment`、`segment_edit`、
> `stuck_signal`。`sister prune --dry-run` 會
> 當場把「現在會刪掉什麼」印出來，一個位元組都不動。

### `segment` — 一天切成哪幾段（schema v8）

從 L0 事件算出來的邊界假設（SPEC §4.1），不是她錄下來的原件。
打開時間軸才批次重算，**錄製那一拍不算**。升級不回填：舊資料庫升上來時這張
表是空的，等打開時間軸那天再算。

| 欄位 | 內容 |
|---|---|
| `started_at` / `ended_at` | 顯示範圍，含前後 5 秒重疊 margin |
| `core_started_at` / `core_ended_at` | 不含 margin 的核心；重算某一天時用核心起點判斷這一段算哪一天的 |
| `app_id` / `window_title` / `url_host` | 這一段待最久的那個前景。沒有就 `NULL`，不拿別的字來充數 |
| `cut_kinds` | 打開這一段的切刀（`app_change,host_change` 這種）。第一段是 `NULL`——沒有打開它的切刀，不是空字串 |
| `confidence` | 那道邊界的信心。第一段是 `NULL`。不是校準過的機率，是「幾個切刀同時成立」算出來的數 |
| `event_ids` | JSON：這一段用到哪些 focus/system/clipboard/input 的 id |
| `computed_at` | 這一次重算的時間 |

同一段時間重算是先刪舊列再插入，不 UPDATE。`sister forget` 和保留期會刪掉
重疊到的列；下一次打開時間軸從還在的事件再算。

使用者在時間軸上合併／切開的結果**不寫這張表**——寫進去下次打開就沒了。
那份動作在 `segment_edit`。

### `segment_edit` — 你怎麼改她切的段落（schema v9）

時間軸上合併兩段、或在一段中間切開，每一次都追加一列。重算 `segment`
不會動這張表；打開時間軸時先算演算法的切法，再依 id 把還沒撤銷的編輯
套上去。所以改過的段落重開還在。

同時是 SPEC §4.3 的訓練訊號：日後調切刀用的是「演算法原本怎麼切、人改
成怎樣」，不是畫面。

| 欄位 | 內容 |
|---|---|
| `ts` | 你按下的時間 |
| `kind` | `merge` / `split` / `undo`。撤銷是多一列，不改舊列 |
| `at_ms` | 合併：被拿掉的那道邊界。切開：切點 |
| `from_ms` / `to_ms` | 這次動作碰到的核心範圍。`forget` 用它判斷重疊 |
| `algo_cut_kinds` | 當時演算法在 `at_ms` 的切刀。演算法沒切就是 `NULL`，不是空字串 |
| `algo_confidence` | 當時那道邊界的信心。沒有就是 `NULL`，不是 0 |
| `target_id` | 只有 `undo`：指向被撤的那一列 |

不記畫面文字、不記你打的字、不記 OCR。`sister forget` 清掉重疊到的事件
時，對應的編輯跟著走；保留期整段核心都過了才刪。升級不回填：舊資料庫
升上來這張表是空的。

### `stuck_signal` — 卡住偵測 v0（schema v9）

打開時間軸時從當天的段落算出來，**只記錄、不開口、不提醒**。一列代表
一次「停留夠長 + 切換夠多次 + 同窗口有 error_code 事實」三個都量到且
都過門檻。缺任何一個成分就不寫列——沒有 input_metrics 覆蓋的時候不會
出現 `switch_count = 0` 的一列來假裝量過。

| 欄位 | 內容 |
|---|---|
| `started_at` / `ended_at` | 那段活動的核心範圍 |
| `app_id` / `window_title` | 那一段待最久的前景。沒有就 `NULL` |
| `dwell_ms` | 停留時長。寫進來的列都有量到 |
| `switch_count` | 這段期間 `input_metrics.window_switches` 加總。寫進來的列都有量到 |
| `error_fact_count` | 同窗口、同時段的 `error_code` 事實數。寫進來的列都 ≥ 1 |
| `computed_at` | 這一次重算的時間 |

門檻（停留 ≥ 3 分鐘、切換 ≥ 6 次）是實作選擇，不是規格常數。重算先刪
再插，跟 `segment` 同一條路。`forget` / 保留期跟著事件走。這一版時間軸
上不顯示——它還不是給人看的判斷，只是給下一層用的訊號。

### `queries` / `query_clicks` / `query_marks` — 你問過她什麼

**這是整份清單裡唯一一張存著「你自己打進去的字」的表。** 其他每一張都是她
觀察到的東西；這一張是你主動輸入的，所以它單獨列在這裡，也單獨有一個開關
（設定頁的「你問過她什麼」，或設定檔的 `privacy.query_log`）。

存的是：原話、走了哪條路（比對字／問時間）、她給了你幾筆東西、花了幾毫秒、
從哪裡問的（`sister query` 還是字母人）。`query_clicks` 再記你點開了哪一筆
出處、它排在第幾個。

`query_marks` 是**你自己按下去的那個位元**：題號 + 按下去的時間，沒有別的。
它記的是「這一題我本來已經忘了」——按鈕在答案底下（字母人）和 `sister mark`
（終端機），按第二次就收回。整份清單裡只有這一格不是她觀察到的東西，是你主動
給的一句話。為什麼要它：PHASES.md Phase 1 的第一條退場條件量的就是它，而題庫
的其他每一欄都答不出來——它們記得住你問了什麼、她給了幾筆、你點開了哪個出處，
記不住你當時知不知道那個答案。（點開出處尤其不是它：那件事最常發生在她答錯、
或你在查核的時候。）

「幾筆」數的是**你看到了什麼**，★ 答案和原文一起算。這一欄第一版只數了全文
比對的結果，於是問「電話」得到號碼的那次——這個產品最典型的一次成功——被記
成「一筆都沒找到」，因為螢幕上根本沒有出現過「電話」兩個字。

為什麼留著：PHASES.md Phase 2 的退場條件直接吃它的產物（「題庫 ≥ 100 題 recall
QA，其中 ≥ 30 題來自真實 query log」），而這種東西**補建不回來**——沒有人記得住
自己上禮拜是用什麼字問的，偏偏真實的用詞正是它唯一的價值。**一筆都沒找到的那些
題目是這裡面最有價值的**：找得回來的只證明她現在能做什麼，找不回來的才是下一版
要修的東西。

看得到：`sister queries`（`--empty` 只看她答不出來的那些，`--marked` 只看你標記
過的那幾次，`--json` 給腳本）。
刪得掉：時間軸上的「忘掉這一段」會一併帶走，`sister prune` 依 `text_days` 過期。
題目走了，掛在它上面的點擊和標記跟著走（外鍵 CASCADE，兩條測試在證明它真的生效）。
CASCADE 帶走的那幾列**不會出現在 `execute()` 的回傳值裡**，所以 `forget` / `prune`
是先數再刪的——不然那一行永遠印 0，而它偏偏是唯一一個補不回來的數字。刪之前的
預覽和真的刪那次報同一個數字（一條測試釘著這件事）。留下來的痕跡是 `meta` 裡的
`ever_marked`（下一節），它讓「一次都沒剩」和「從來沒按過」在畫面上分得開。
關得掉：關掉之後不再累積，已經有的仍然留著，直到你忘掉那段時間或它過期。

> **這一欄的誠實話**：四張同意書分別管本機記錄、CLI 讀字、截圖保存與 Azure
> 朗讀；沒有一張拿來授權保存你打進搜尋框的字。這一張表記的是你自己輸入的 query，
> 不是第四張 Azure 同意所授權的 outbound。理由講在這裡，
> 開關放在設定頁第一屏看得到的地方，預設是開的。如果你覺得這個決定不對，
> 那個勾就在那裡。

### `sessions` / `meta`

程式版本、擷取後端 identity、起訖時間；schema 版本。`sessions.platform` 不只是
顯示用的 OS 名稱：alpha.103 的 URL 來源查詢只接受
`windows/windows-gdi-uia-focused-url-v2` 這個 exact identity；換 backend 時會先
fail-closed，不能靠字首或版本字串大小猜成可信。歷史 v1 identity 仍留在原列供查詢
與顯示，但不再授權無人值守 URL。

> **`sessions` 也會過期，跟著它自己那幾列走。** 一場錄製的每一列都被
> `prune` 或 `forget` 帶走之後，那一列 `sessions` 本身也刪掉——不然「我那天
> 下午的東西全刪了」之後，資料庫裡還留著一列寫著你那天幾點開機、用什麼平台、
> 錄到幾點。判斷的形式是「沒有任何一張表指著它」，不是「它落在那段時間裡」：
> 一場橫跨整個下午、但只有一半被刪掉的錄製，那一列要留著。
>
> 正在錄的那一場不算——它一列都還沒寫是正常的，那是**還沒**，不是空的。
> `sister prune --dry-run` 和字母人上刪除前的那份預覽都會把「連錄製本身一起
> 刪掉幾場」印出來。
>
> **而當掉的那一場，和正在錄的那一場分不出來。** 兩者在資料庫裡都是「還沒
> 收尾的最新一列」，所以那道守衛會連當掉的那一列一起放過：全刪之後，`sessions`
> 上會剩下一列空殼，帶著開始時間、程式版本、平台。這裡選擇不刪（刪掉一場活著
> 的錄製會讓接下來每一筆紀錄指向一個不存在的東西），改成講出來——`sister
> forget` 刪完會多印一行說那一列留著了，`sister stats` 的「工作階段」旁邊會標
> 「空殼」。那兩個地方問 `heartbeat::is_occupied`，所以講得出是**當掉**還是
> **她此刻正在錄**——分得出來就不印「或」。
>
> 它什麼時候會走，兩種原因的答案剛好相反：**當掉的**那一列要等她再開始錄之後
> 才不再是最新的一列，接下來任何一次清理都會帶走它（**開錄那一次不算**——
> `record` 的清理跑在 `start_session` 之前，那一刻它還是最新的一列，所以整場
> 錄製期間 `sessions` 都會多一列；要馬上清掉就在開始錄之後跑一次
> `sister prune`）。**正在錄的**那一列則是等她**收工**：`end_session` 會掃它
> 自己那一場，一列都不剩就跟著走。
>
> **「一列都不剩」不算那一場自己的 `session_start` / `session_end`。**
> 那兩列是容器上的標籤，不是她記下來的東西——和 `sessions` 那一列是同一種東
> 西，所以會跟著那一場一起被刪掉。少了這個區分，上一段那句「等她收工」是假
> 的：`Recorder::finish` **先**寫 `session_end` **再**呼叫清掃，於是它自己剛剛
> 寫的那一列讓那一場「不空」，那道清掃在產品裡從來沒有刪掉過任何一列。同一個
> 區分也讓 `sister stats` 在你清空之後**又開始錄**的那一刻不會改口說「還沒錄
> 過」——那時候整顆資料庫只有一列 `session_start`。

> **`meta` 裡有一顆過不了期的旗標：`ever_recorded`。** 第一次開始錄的時候
> 寫下去，`forget` 和 `prune` 都不碰它。它是一個位元，內容是「這台機器上有人
> 錄過」——沒有時間、沒有次數、沒有錄了什麼。
>
> 留著它是因為「她從來沒錄過」和「你把東西全刪了」不是同一件事，而那兩件事
> 以前在畫面上長得一模一樣：全刪之後打開時間軸，她會說「還沒有東西，去按開始
> 錄」——把你剛剛做的那個決定講成一個你還沒做過的動作。這一個位元的代價，
> 換的是那一頁能改口說「這段時間裡沒有東西了」。
>
> 這是一份寫著「全部刪掉」的功能刻意留下的殘渣，所以它必須寫在這裡。要連它
> 一起清掉的話，只有刪掉 `sister.db` 這條路。

> **同一個地方還有第二顆：`ever_stored`（alpha.33 新的）。** 一樣是一個位元、
> 一樣過不了期、一樣只有刪掉 `sister.db` 才會消失。它答的是「這台機器上有沒有
> 真的存下來過一列內容」。
>
> 上面那一顆答不出這一題：它在 `start_session` 就翻成 1，**第一張畫面之前**。
> 於是一台 `capture.enabled = false` 的機器——她開場、跑完、收工、一個字都沒
> 記到，而 `sister forget` 從來沒有被執行過——在 `stats`、`facts`、`doctor`、
> `query` 四個地方被告知「被 `sister forget` 忘掉了，或是過了保留期」。那是
> **指控一件沒發生的事**，而且把下一步指到相反的方向：該看的是 `capture.enabled`，
> 不是保留期。
>
> 它是靠 SQLite 觸發器按下去的，不是靠程式碼在每個 insert 呼叫端記得寫一次
> ——那六個呼叫端漏掉任何一個，都是漏掉一種內容。內容進來的第一列按下去，
> 之後每一次 INSERT 只剩那道 `WHEN` 要付：量出來是 20 萬列純 INSERT 的
> microbenchmark 上 +40~45%，約 0.26 µs/列。她一秒寫個位數列，所以絕對值是
> 零——但別把它當成免費的。`session_start` / `session_end` 那兩列不算，理由
> 和上一段同一個：它們是容器上的標籤。
>
> **alpha.33 以前就已經被清空的資料庫答不出這一題**：升級那一刻現貨是空的，
> 而「它到底存過沒有」在那個檔案裡沒有任何一個位元分得出來。那一顆拿到的不是
> 答案，是一張標籤——`ever_stored = 'assumed-at-upgrade'`。它讀起來和「存過」
> 一樣，也就是**升級之後那台機器繼續說 alpha.32 說過的那句話**。
>
> 為什麼不留空：留空會讀成「一列都沒存進來過，先看 `capture.enabled`」，而他
> 昨天才刪掉一整天。那不是多餘，那是換了一個診斷、換了一個下一步，而那個下一
> 步指向一個他機器上根本沒問題的設定。**升級不可以改寫一句關於他的資料的舊
> 話。** 代價是相反那一種（升級前就已經是 `capture.enabled = false` 的機器）
> 繼續讀到那句舊的假話——但那是 alpha.32 本來就在說的，不是這一版新造的。
>
> 標籤會自己過期：第一列真的落地時，觸發器把它覆蓋成量到的 `'1'`。
>
> **而「沒存過」自己又是三種。** 一台跑完收工、一列都沒落地的機器（去看
> `capture.enabled`），一台**此刻正在錄、還沒落地第一列**的機器（再等一下），
> 和一台**recorder 正在起來、還在開資料庫**的機器（也是再等一下，但她連第一拍
> 都還沒跑）——下一步不一樣。分它們的不是資料庫裡的位元，是 `recording.beat`
> 那個心跳檔，而且要問到 `heartbeat::Phase` 那一層：`is_occupied` 只分得出兩
> 種，第三種會被併進「她此刻正在錄」，於是同一份 doctor 上半說她在錄、下半說她
> 還沒開始。所以問「這是哪一種空」的那幾頁都要拿著 `data_dir`，光有 `Db` 答不
> 出來。
>
> 修「兩種 0 長得一樣」的手段是多問一個位元，而多問的那個位元自己又會蓋住一
> 組——`ever_stored` 蓋掉的就是這一組。縮一個集合、加一個旗標之前先問：做完
> 之後，哪兩種處境會變成同一句話？**而一個位元只有兩面**：狀態機被壓成布林的
> 時候，被壓掉的幾乎永遠是「東西正在來、還沒好」那一面。

> **第三組（alpha.34 新的）：`sessions_started` / `sessions_ended`。** 前面兩顆
> 是位元，這兩顆是**數字**——所以它們寫在這裡的理由比前面兩顆重一級。
>
> 它們過不了期，`forget` 和保留期都不碰，只有刪掉 `sister.db` 才會消失。內容是
> 「她被開起來過幾次」和「她好好收尾過幾次」。**沒有時間、沒有長度、沒有版本、
> 沒有哪一次是哪一次**——重建不出任何一場錄製，也答不出你哪一天坐在電腦前。
> 差額就是沒有回來的那幾場。
>
> 為什麼一個寫著「全部刪掉」的功能要留下一個會長大的數字：因為
> `retention::delete_empty_sessions` 會刪 `sessions` 那張表自己的列（#52 要求
> 的，那一列帶著起訖時間，是「他那天下午 13:02 到 17:44 在電腦前」的證明），
> 而「零當機」以前就是數那張表。於是**最該被算進去的那一種當機，剛好就是會把
> 自己的證據刪掉的那一種**：開起來、還沒讀到第一張畫面就死掉——沒有內容，下一
> 次 `prune` 連紀錄一起掃走，分子和分母同時少一。實測（六場：一場正常＋五場開
> 機即死）掃之前是「6 段裡有 5 段沒回來」，掃之後是「2 段裡有 1 段」。訊號是反
> 的：**她死得越早，那一格讀起來越乾淨**，而 Phase 0 的退場條件寫著「連續 7 天
> 自我錄製、零當機」。
>
> 留著那幾列不是選項（那正是 #52 剛拿掉的東西），所以數字要撐過那幾列。這是這
> 份清單上第一次拿「一個會長大的數字被留下」去換一個診斷，寫在這裡讓你自己決定
> 值不值得。
>
> 和前兩顆一樣是 SQLite 觸發器按的，不是呼叫端。一場錄製才跑一次（不是一列內容
> 一次），所以 `ever_stored` 那 0.26 µs/列的帳在這裡不存在。
>
> **時間刻意不留。** 「她那天幾點幾分當過」正是那一列被刪掉要拿掉的東西，所以
> `doctor` 報得出時間的只有還留著紀錄的那幾場，而那句話會自己講出這個界線
> （「還留著紀錄的最後一次 ⋯，另外 N 段連紀錄都沒留下」）。數字撐過清空可以，
> 時間不行。
>
> **升級那天的數字是一個下限。** 回填只數得到還在的列，一顆跑了三個月、被
> `prune` 掃過幾十次的資料庫，升上來的那一刻真實數字已經不可考。所以回填的同時
> 按下第三顆 `session_counts_floor`，而那一顆在的時候句子不准說「全部」——它會
> 補一句「這裡的數字是升上來那天數到的；升級之前如果有錄製被清掉，不在裡面」。
> **回填出來的數字是一個猜測穿著數字的衣服**：要嘛講清楚，要嘛不要講。
>
> 那句補充寫成**條件句**是刻意的。第一版寫的是「升上來之前被清掉的那幾場不在這
> 個數字裡」，而那台機器可能一場都沒被清過——那句話替它宣告了一場沒發生的刪除。
> migration 本來就不可能知道有沒有被刪過，所以它只講自己知道的那一半：這個數字
> 是那天數到的。
>
> **而「現在正在錄的那一場」要從每一個數字裡扣掉，扣除算在產生數字的地方。**
> 那一場沒有 `ended_at`，在磁碟上和一次當機逐字相同——分得出來的只有心跳檔。所以
> `crash_audit` 和 `signal_audit` 收的是 `heartbeat::Phase` 本人，而不是一個
> 「有沒有人佔著」的布林：`Booting`（她起來了，但 `Db::open` 還在跑，她那一列
> 還沒 INSERT）那幾分鐘裡，「有人佔著這個目錄」是真的、「最新那一列是她的」是
> 假的，兩件事同時成立。一個布林湊不出三種答案，於是那段時間裡上一次當機留下來
> 的殼會被說成「她現在還在跑」，而分母會扣掉一場還不存在的錄製。
>
> 扣除**不可以**算在印字的地方：一個新位元只餵給一起印出來的其中一個數字，就會
> 生出 N 個描述 N 個不同集合的數字。實際犯過兩次——一次是「零當機」扣了當機數
> 卻沒扣分母和時間，一次是三列訊號稽核照樣把被扣掉的那一場叫成「上一場」（問
> 「那 2 段裡哪一場是上一場」，答案是都不是）。現在那三列在她還在錄的時候印的是
> 「這一場（⋯ 起，還在錄）」，`stats --json` 那一份也多一個 `scope_is_live`。

> **第四顆（alpha.43 新的）：`ever_marked`。** 一樣是一個位元、一樣過不了期、
> 一樣只有刪掉 `sister.db` 才會消失。內容是「這台機器上有人按過『這一題我本來
> 已經忘了』」——沒有時間、沒有次數、沒有是哪一題。
>
> 它守的零在 `query_marks`（上面那一節）。那張表清空之後 `sister queries` 的
> 「★ 魔法時刻」是 0，而 0 有兩種：**他從來沒按過**（還沒開始量），和**他按過、
> 現在一格都不剩**（量過的東西掉了）。後者又有兩條路——他自己 `mark --undo`
> 收回的，和那幾題被 `forget`／保留期帶走的。前一種是還沒開始，後一種裡有一種
> 是資料真的沒了，而 Phase 1 第一條退場條件量的就是那個數字：它掉了而沒被說
> 出來，那七天要重來。
>
> **這一顆分得出前兩種，分不出後兩種，而那條線寫在句子裡。** 它只答得出「他按
> 過」，答不出「後來是誰拿掉的」——`--undo` 和 `forget` 在這張表上長得逐字相同。
> 所以那一格印的是「你按過，但現在一個都不剩」，然後把兩種可能都攤開，一種都
> 不替他選。真的被 `forget` 帶走的那一次，`forget` 自己當場會說（「刪掉了 N 次
> 『★ 我本來已經忘了』——這一格補不回來」）；那是唯一知道答案的地方，也就只有
> 那裡講。
>
> **但題目整批被帶走的時候走的是另一句話。** 上面那一格是題庫表頭的一行，而
> 表頭只在題庫還有東西的時候印得出來——`forget`／保留期把整份題庫掃掉之後，
> `sister queries` 走的是「題庫是空的」那條路，★ 那一行根本輪不到。而那正是這
> 個旗標最要緊的一種空：退場條件的證據整批沒了。所以那句話自己也吃這一顆——
> 它翻成 1 就代表他問過（標記只掛得上一題真的問過的題目），於是「可能是還沒問
> 過她任何問題」當場被砍掉，剩下的話直接講「你問過，也按過，現在都不在了」。
> 少了這一步的話，一個剛把七天證據刪掉的人，收到的第一個可能性是「你還沒開始
> 問」。
>
> 第一版不是這樣寫的：它直接說「跟著那幾題一起被忘掉了」。而驗收清單上就有一步
> 是叫他按一次再收回來，那一步走完會當場拿到那句話，然後被告知他的資料被刪掉了
> ——**修一個「兩種零」的時候造出第三種**，是這個 repo 犯過最多次的一件事。
>
> 沒有回填，也不需要：標記這件事從來沒有出現在任何一個發出去的版本裡，所以
> 「升級之前按過」是一個不存在的處境。上面 `ever_stored` 那張 `'assumed-at-upgrade'`
> 標籤在這裡沒有對應物，這是它比前三顆便宜的地方。

> **第五顆：`ever_brain_outbound`。** 一樣單調、一樣過不了期。內容是「這台機器
> 上曾經把字送出過程式」。`brain_outbound` 的列只會被 `sister forget` 依外送時間
> （問出去那一刻）清掉；保留期不碰這張表。所以
> 「一列外送紀錄都沒有」有兩種意思：從來沒送過，和送過、被清掉了。外送紀錄面板
> 靠這一顆把兩句話分開。跳過（`brain_skip`）不算送出。列還在的時候沒有這個 key
> 也算送過——不然升級上來、旗標還沒按下的那幾列會被說成從來沒送。

---

## `action-log.jsonl` — 她動過的手（alpha.69 起）

**這個檔案有你的資料**，所以它不在上面那張「不含任何資料」的小檔案表裡。

一行一個事件的 JSONL。

**alpha.70 起，「有沒有這個檔案」的門檻比 alpha.69 低很多。** 只要你跑過一次
`sister do`，即使**一步都沒答應**，這個檔案就會存在，而且裡面已經有你的資料：

- 一列 `granted`，裡面有你打在命令列上的 `--task` 原文；
- 每一個被端到你面前的步驟一列 `proposed`，**含完整網址或檔案路徑**——
  這一列是在問你之前就寫下去的，不是你答應之後。

換句話說：**「她提議過什麼」和「他准了什麼」都會留在磁碟上，即使他全部說不要。**
這是刻意的（不然「她提過但我拒絕了」這件事就沒有任何紀錄），但它的意思是
alpha.69 那句「沒按過就沒有這個檔案」在 alpha.70 之後是假的。

只有既沒跑過 `sister do`、也沒按過字母人「要我幫你打開嗎」那顆按鈕，才是真的
**沒有這個檔案**（不是空檔案）。`sister do` 只要跑過一次就會寫，即使一步都沒答應；
字母人則是你真的按下那顆按鈕、讓她執行時才寫。

| 欄位 | 內容 |
|---|---|
| `event` | `granted` / `proposed` / `approved` / `executed` / `refused` / `step_finished` / `aborted` / `concluded` |
| `at_ms` | 這件事發生的時戳 |
| `grant` | 只在 `granted` 那一列：**你打的 `--task` 原文**、授權的 app 清單、動作種類、有效毫秒數、步數上限 |
| `action` | 完整的動作與目標——**含完整網址或檔案路徑** |
| `result` / `reason` | 成功細節、失敗訊息，或她為什麼沒有動手 |
| `evidence` | 只在 `step_finished` 那一列：這一步做完時她在 `frames` 裡查到什麼。**存的是 frame 的 id 和時戳，不是圖、也不是螢幕上的字**——要看圖還是得回資料庫。查不到的時候存的是查不到的**理由**（沒在錄／才剛起來／狀態檔讀不懂…），不是一個空值。`null` 只有一種意思：那一列是 alpha.72 以前寫的，當時根本沒有去查 |

每一列都重複完整的動作與時間，所以單獨一列就讀得懂（SPEC §9.3）。代價是
目標字串會在檔案裡出現很多次。

**它會跟著「忘掉」一起走。** `sister forget --last 7d --yes` 和字母人上的
「忘掉這一整天」都會把落在那段時間裡的列從檔案裡刪掉——刪的是字本身，不是
蓋一個旗標。讀不懂的列問不出時間，所以一併刪掉，而且會分開報數字。

**它也會跟著 `sister export` 一起走。** 那個指令自稱全量匯出（SPEC §11.8），
而這個檔案是記憶、不是這台機器的設定，所以它跟 `sister.db` 一起被帶走，
不用另外加開關。只有既沒跑過 `sister do`、也沒按過字母人「要我幫你打開嗎」
那顆按鈕，匯出的目錄裡才**不會有**這個檔案（不是一個空檔案）。`sister do`
只要跑過一次就會寫，即使一步都沒答應；字母人則是你真的按下那顆按鈕、讓她執行時
才寫。檔案一旦存在就會跟著匯出走，包含其中的完整網址或檔案路徑；`sister do`
還會留下你打的 `--task` 原文和她提議過的每一串網址。

**但它不受保留期管**（alpha.69 的狀態，見〈已知缺口〉第 8 條）。`sister prune`
會清掉過期的畫面與文字，不會碰這個檔案——三年前你按過的那顆按鈕，那串網址
今天還在裡面。要它消失只有兩條路：`sister forget` 涵蓋那段時間，或直接刪掉
這個檔案。

> 這一段是 alpha.69 補的，而那顆按鈕也是 alpha.69 才第一次真的出得來。
> 補之前那個檔案不在任何一條刪除路徑上：`sister.db` 清乾淨了，這裡的網址
> 還躺著，而畫面上寫的是「已經忘掉了」。

---

## `grant.json` — 跨行程重用的授權範圍

這個 JSON 檔只存 `Grant` 的五個範圍：任務原文、允許的 app、允許的動作種類、
發出時刻與相對期限、步數上限。它不存 `StepApproval` 或 `GrantPermit`：互動模式的
每一步仍要人在當場按「好」；只有明確使用 `sister do --use-grant --unattended`
才不等按鍵。無人值守時也不是把整張 grant 直接交給 executor——每個 exact action
通過 scope／期限／步數、目標畫面與 URL 政策後，才現鑄一張綁定該 action 的 permit。
`[hands] url_open` 是上面那份全域設定，不存在 `grant.json` 裡。

**`sister forget --yes` 會刪掉整個檔案，而且不看 `--last`。** 授權書含 `--task`
原文，又不是能按畫面時間切片的事件；只刪一部分會假裝剩下的 scope 沒有包含那段字。
不看區間是刻意的：授權書不是一段記憶，是一張還能拿去跑的票，留著等於「忘掉」之後
磁碟上還有一個上好膛的範圍配著他打的原文。**這是資料目錄裡唯一一個站在 `--last`
區間外面也會被刪掉的東西，所以 `--yes` 前的預覽會先講這件事。**

同一刀也會帶走 `grant.json.tmp`——那是寫到一半斷電留下的半成品，裡面是**整份**
授權書。它不算暫存垃圾，它跟本體一樣含 `--task` 原文。

**字母人時間軸上的「忘掉這一段」做同一件事**，預覽也講同一句話。兩邊呼叫的是
同一支函式（`sister_hands::semi_action::forget_saved_grant`）——各寫一份的話，
改天多一個檔案只會補到其中一邊，而兩邊畫面上都照樣寫著「已經忘掉了」。
`action-log.jsonl` 就是這樣漏了好幾版。

**`sister export` 會帶走它**，連同 `.tmp`。全量匯出的目錄若來源有，目的地也有；
來源從未存過時不會憑空建立空檔。

**過期就會被清掉，不必他動手。** `sister prune` 手打會清，而**錄製迴圈自己也會**
——開錄前一次、之後每 6 小時一次，跟資料庫的保留期同一支（`prune::sweep`）。
所以過期授權書裡的 `--task` 原文不會因為他從來不打 `sister prune` 就永遠留著。
`--dry-run` 只報告；檔案讀不懂或時鐘倒退時不猜、不刪。清掉之後那個畫面**不會**
再說「什麼都沒動」或「沒有東西可以清」——剛動過一個檔案還這樣講就是假話。

讀回使用時一律拿**現在**的時刻重驗，不是存下去那一刻；時鐘倒退與過期都拒絕。
`sister do --show-grant` 除了範圍之外還會印發出時刻、到期時刻和還剩多久——
只給一個相對毫秒數的話，「還剩 2 秒」和「還剩 59 分鐘」在畫面上長得一模一樣。

---

## 不存在的東西

以下**沒有任何表、任何欄位**承接：

- 按鍵內容、輸入法組字內容
- 麥克風、攝影機、系統音訊或使用者音訊（Persona cache 可有公開的預錄固定 WAV；
  Azure 回傳的合成 MP3 只作當次播放，兩者都不是從這台機器錄來的聲音）
- 網路流量、DNS、封包內容（Persona GET 與 Azure TTS POST 會真的發生，但資料庫不
  擷取或保存那些流量；Persona 素材 cache 與 Azure 的 transient RAM 邊界另列在文件開頭）
- 檔案內容（除非它顯示在螢幕上被 OCR 讀到）
- 位置、聯絡人、行事曆
- 任何形式的識別碼上傳、遙測、崩潰回報

---

## 已知缺口（誠實聲明）

**這一節比上面所有內容都重要。** 一條寫得好看但不生效的規則，
比沒有規則更危險——因為使用者會依賴它。

1. **讀回來的網址是縮寫過的。** Windows 上唯一讀得到網址的地方是瀏覽器
   的位址列，而 Chromium 顯示的是給人看的版本：`https://www.example.com/a`
   會變成 `example.com/a`。路徑與查詢字串留著，scheme 和 `www.` 沒了。
   所以 `frames.url` 存的是**位址列上那串字**，不是真正的 URL。
   規則是子字串比對，因此照樣命中；但寫成 `https://*bank*` 的規則
   永遠不會生效——`sister doctor` 會把這種規則挑出來。
   使用者正在位址列打字時我們**不讀**，因為那時候上面是散文不是網址。
   要講清楚的是：擋住這件事的是「位址列有沒有鍵盤焦點」那一道閘門，
   不是後面的字串檢查。alpha.100 起，連 `CurrentHasKeyboardFocus()` 自己報錯
   都會拒收這一個候選；「問不出來」不再被當成「沒有焦點」。修正後的 recorder
   當時用了 v1 session identity。alpha.103 又發現全域 focused element 沒綁回原本
   exact HWND，且 cache 可能重用舊 URL 字串；因此 v1 歷史列雖仍可讀，已不能替無人
   值守 URL 背書。只有每拍把 focused element 綁回原 HWND、重讀 live value 並通過
   前後 exact HWND／PID 重驗的 v2 session 才有資格。升級不改寫舊 session，也不能
   拿版本字串大小猜；要讓新版 recorder 實際觀察該站一次。
   v2 仍不把 host provenance 叫成安全判斷：一串沒有空白又帶點的字——email、
   內網 IP、未送出的半截網域——仍可能看起來像網址。
   那道焦點閘門要一個活的 COM 元素才驗得到，所以它沒有單元測試，只有
   `plausible_url_is_not_a_substitute_for_the_keyboard_focus_gate` 這條
   把它的「不可取代」釘住，以及 Windows 上的實機驗證。

2. **敏感欄偵測會問每一個前景 app。** UIA 的密碼屬性查詢每拍都做；只有昂貴的
   位址列樹遍歷仍限瀏覽器。UIA 回報焦點在敏感欄、回報不知道，或整個 privacy
   context 讀不到時，recorder 都在剪貼簿與螢幕之前停下。連續失敗五次會另外把
   能力缺口顯示給使用者，但不會把 fail-closed 保護關掉。若使用者按「顯示密碼」
   使控制項不再帶密碼屬性，仍可能被記下；app／標題排除規則仍是第二道防線。
   前置檢查後仍可能換窗：Windows pixels 可能短暫進工作 RAM，但 permit／system
   post-check 不通過就不進 dedup、OCR、DB 或 PNG。這是防持久化，不是「RAM 從未碰過」。

3. **旁人的畫面。** 會議 app 前景時自動暫停（Zoom/Teams/Meet 等），
   但 Slack 與 Discord **不在**該清單——它們平時是主要工作場所，
   光憑 app 名稱分不出「正在分享畫面」與「正在聊天」。在 Slack 裡開啟
   螢幕分享時，對方的畫面可能被記錄。Windows GDI 抓的是前景所在的整個 monitor；
   前景很普通時，同一個 monitor 上仍可見的背景敏感視窗也可能一起入幀，foreground
   app／URL／title／敏感欄規則不保護它。

4. **排除是規則式的，不是語意式的。** 沒有列進 blocklist 的敏感網站
   會被完整記錄。預設清單只涵蓋常見的台灣銀行與幾個登入頁。剪貼簿又沒有可靠的
   來源 URL／標題證明：快速 browser copy → switch 不能拿下一拍前景補來源。
   alpha.103 在有 URL rules 時保守丟棄這種 browser clipboard；代價是她也記不住
   本來可安全保留的瀏覽器複製內容。

5. **沒有人動的時候，她最多五秒沒在看。** 這道閘門原本為了降低 CPU，從上次
   看螢幕到現在沒有任何鍵盤滑鼠輸入的話，那個 tick 完全不碰螢幕。這是一個
   猜測——沒有人動 ⇒ 畫面沒變——而影片、進度條、跑動的 log、別人傳進來的
   訊息都不需要你動任何東西。上限是 5 秒（`MAX_BLIND_MS`），超過一定睜眼。
   實際的缺口長這樣：**一段在五秒內出現又消失、而你完全沒碰電腦的畫面，
   可能一個字都沒被記到**，而且資料庫裡不會有任何一列說明那一刻發生過事情。
   錄製摘要會印 `省下：N 次沒碰螢幕（M%）`，那個百分比就是這個缺口的大小。

6. **她只讀得懂你裝了語言包的語言。** Windows 的 OCR 是逐語言安裝的。
   沒裝中文時引擎**不會報錯**，它會挑一個裝了的（通常是英文），然後把
   滿螢幕的中文讀成空白——錄製正常、資料庫在長大，只有搜尋永遠是空的。
   `sister doctor` 的「讀字」那一段會印出**實際挑中的**語言，
   不是設定檔裡的偏好清單。

7. **抽出來的事實沒有分對錯，也沒有分把握。** `facts` 裡的每一列權重都
   一樣。`8/17` 會被當成日期抽出來，但它也可能是分數、比例或版本號；
   規則裡曾經有一個「把這種的信心壓到 0.65」的動作，而那個數字沒有人讀，
   所以實際上 `8/17` 一直是以日期的身分照樣進資料庫的。刪掉那欄之後這件事
   從「假裝處理過」變成「明講沒處理」。L2／Reviewer 現在已經會推論與回查，
   但它們不會倒過來把每一列 L1 regex 命中改成真；重播評測也只能量出這類錯誤，
   不能把未標註的列自動升格成 ground truth。搜尋結果裡仍可能出現不像日期的日期。

8. ~~**兩個字的中文詞沒有索引，而且只找得回最近 30 天。**~~ **schema 3 補起來了。**
   原本的缺口：`text_fts` 用 trigram，比不了少於 3 個字的東西；`text_fts_uni`
   用 unicode61，而它把「客服專線」整串當成**一個** token（不是逐字切），所以
   `MATCH "客服"` 是 0 筆。剩下唯一找得到的辦法是掃過所有文字，而那個成本跟你
   用了多久成正比（45 天語料 224 ms），只好夾在 30 天內（`LIKE_SCAN_DAYS`），
   代價是兩個字的中文查詢找不回 30 天以前的東西——而文字保留期是 **365 天**。
   現在 `text_fts_bi` 存的是切好的**相鄰雙字**（「客服專線」→「客服 服專
   專線」），兩個字的查詢因此有了索引：45 天語料 0.1 ms，時間界線消失。
   代價實測 **+29%**（207,360 行字：97.9 MB → 126.0 MB）。
   **還剩下的**：`LIKE_SCAN_DAYS` 這條路沒有拆掉，因為**一個字**的中文查詢
   產不出雙字（「工」沒有相鄰字），那條查詢仍然是掃描、仍然只看得到 30 天。
   bigram 是**粗篩**：三個字以上會被拆成重疊雙字，所以「客服中心的服部先生」
   會同時命中「客服」和「服部」——`search_bigram` 拿真字串再篩一次，那層篩
   掉的是索引答不了的部分，不是可以省的。

9. **有 reader 不等於訊號正確。** `focus_events` 現在供 segmenter 與 alpha.100
   的 URL 來源查詢使用，`input_metrics` 也進斷句／卡住訊號，`ocr_blocks` 的位置
   有 doctor 的自相矛盾檢查；所以它們已經不是「整張沒人讀」。但目前能驗的主要是
   **明顯自相矛盾**：焦點事件全無 app、輸入列四個計數器全 0、一整張畫面的文字框
   全在同一高度。錯但看起來合理的 app、host、計數或座標仍可能被下游採信。
   alpha.100 讀 `sessions.platform` 只解決 URL 來源的 backend allowlist，也不替
   每一列內容背書；「有人 SELECT」不能被寫成「有人驗過是真的」。
10. **`action-log.jsonl` 不受保留期管。**（alpha.69）`sister prune` 清得掉
   過期的畫面和文字，碰不到這個檔案——三年前你按下的那顆按鈕，那串網址今天
   還在裡面。`sister forget` 和 `sister export` 兩條路這一版都補上了，保留期
   這條沒有。
   沒有一起補的理由是它需要先回答一個沒人答過的問題：**這個檔案到底是記憶，
   還是稽核紀錄？** 是記憶，那就該跟文字一樣 365 天到期；是稽核紀錄，那自動
   刪掉她做過什麼正是稽核最不該做的事。這一版把它當成記憶處理（跟著 forget
   走、跟著 export 走），但沒有替「自動過期」那一題挑答案——在這裡寫一個沒
   想清楚的預設值，比先說出來更糟。
   實務上它只有在你按下那顆按鈕時才長一列，所以不會變大；想清掉就跑一次
   涵蓋那段時間的 `sister forget`，或直接刪掉這個檔案。

---

## 怎麼自己查證

```bash
sister stats               # 記了多少、佔多少空間、哪條排除規則真的擋過東西
sister doctor              # 排除規則、失效的保護、schema 版本、現在有多少已過期
sister query <關鍵字>      # 每一筆都附出處
sister facts --kind phone
SISTER_BENCH_DAYS=45 cargo test -p sister-core --release \
  --test search_latency -- --nocapture   # 查詢延遲，附語料規模
sister prune --dry-run     # 保留期現在會刪掉什麼（一個位元組都不動）
```

驗證排除真的生效：

```bash
cargo test -p sister-capture --test privacy
```

那個測試會跑一段踩滿地雷的腳本，然後把**整個資料目錄當成位元組**掃過，
確認不該存在的字串一個都不在。它不依賴任何人記得要檢查哪個欄位。
