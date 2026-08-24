# AI-Sister（字母人）

> 一個站在桌面角落的姊妹。她一直都在，看得見你的一天，記得住細節，
> 95% 的時間安靜，該說話的時候才說話——說的每一句都能點開證據。
>
> An open-source, local-first desktop companion: a filing cabinet that never
> forgets, an event-driven brain that can admit it's wrong, and a letter-person
> who knows when to stay quiet. Raw screen data never leaves your machine.

**Status: Phase 0 收尾、Phase 1 開工，Windows alpha 可以下載來跑**
（[Releases](https://github.com/teddashh/AI-Sister/releases)）。

她開始看之前有**三張各自獨立、隨時撤得掉的同意書**，條文和效力就是：

- `local-recording`：「我同意在我的硬碟上記錄我的螢幕。」沒有這一張，`sister record`
  不會開始錄；錄到一半撤回，正在跑的 record 每 5 秒重讀同意書，最多再錄 5 秒加一拍；
  `capture.min_interval_ms` 超過 5 秒時，主要會等那一拍。沒簽時拒絕啟動、回非零；
  簽了才准在本機記錄。
- `cloud-reading`：「我同意把去識別化後的文字（永不含畫面）送到我指定的模型商做解讀。」
  **今天沒有東西會用到這張：這份程式裡沒有任何連外路徑，沒簽無事可擋，簽了也還沒有作用。**
  L2/L3 還沒開工；這張將來才准把文字交給使用者指定的模型商。
- `frame-storage`：「我同意保留變化幀的截圖，而不是只留上面的字。」沒簽不擋錄，
  她會當場說明降級，只記字、一張截圖都不寫；簽了才准依設定保留變化幀。

例如要准她在本機記字並保留截圖，要寫
`sister consent --grant local-recording --grant frame-storage`。三個介面——
`sister consent` 和第一次開字母人時那一頁都從 core 取同一份條文與未簽後果；
`sister doctor` 讀同一個檔案，另外報告目前是否簽署及會發生什麼事。
條文改版會讓舊簽名全部失效，檔案讀不到、損壞或版本不符也一律當成沒簽。CLI
指定 `--data-dir` 時，同意書跟著那個資料夾走；字母人只讀預設資料夾，兩邊不一定是同一份。

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

跑得起來的是最底下那一層：看畫面、讀字、抓出電話與金額之類的事實、存進 SQLite、搜得回來。
**這一層一個模型呼叫都沒有**，全部是程式在抄寫。L2/L3（會推論的那個腦）還沒開始。

Release 裡有**兩個**執行檔，分工是「一個記、一個問」：

| | 做什麼 |
|---|---|
| `sister.exe` | 記。`record` 錄、`doctor` 自我檢查、`query` 在終端機查、`pause` 叫她閉眼、`consent` 簽或撤同意書 |
| `sister-desktop.exe` | 問。桌面角落那個字母人：搜尋框 + 出處，點得開當時那張畫面；還有一條時間軸，可以翻、可以刪 |

## 跑起來

**Windows**——[Releases](https://github.com/teddashh/AI-Sister/releases) 下載那兩個
執行檔，放同一個資料夾（字母人是去隔壁找 `sister.exe` 的）。她要先拿到第一張
同意書才會動：

```
sister consent --grant local-recording
sister doctor
sister record --duration 60
```

然後開 `sister-desktop.exe` 問她剛剛那一分鐘發生了什麼，或者直接 `sister query 電話`。
`doctor` 排在錄之前是有意的：它會當場示範這台機器**現在**讀不讀得到網址、
OCR 有沒有裝、哪幾條排除規則其實不生效——比錄完 60 秒才發現什麼都沒進去好。

**從原始碼**——Linux/macOS 也跑得起來，只是還沒有擷取後端，所以第一次不能叫她
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
    ↳ phone · 2026-08-19 04:37:39 (剛剛) · slack.exe
  ★ +886912345678  「0912-345-678」
    ↳ phone · 2026-08-19 04:36:47 (1 分鐘前) · chrome.exe · 中華電信 客戶服務 - 帳單查詢 · frame #1
```

**clone 到第一個答案實測 33 秒**（乾淨的 `CARGO_HOME`：抓 108 MB 相依 + build 32 秒。
16 核開發機，GitHub 的 runner 大約是這裡的 2.1 倍）。需要 Rust 1.85 以上——這份
程式是 edition 2024。整條路上沒有 `sudo`、沒有服務、沒有帳號。

這一步**一個像素都沒讀你的螢幕**，所以它不用簽同意書：`replay` 讀的是 repo 裡那份
JSON 腳本，沒有任何東西可以同意。要看她在你自己的機器上會做什麼，得走上面那條
Windows 的路。

舊的 `sister replay scenarios/bill-lookup.json` 語法保留不變。現在也能把自己記下的
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
| `baseline_text` | 2/4（50.0%） | 3/5（60.0%） | 2/4（50.0%） | 0 | US$0/天 |
| `facts` | 4/4（100.0%） | 5/5（100.0%） | 4/4（100.0%） | 0 | US$0/天 |
<!-- END GENERATED: recall-benchmark -->

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

問她「**剛剛發生什麼事**」會得到答案，而不是「我記得的東西裡沒有這件事」。那句話
問的是時間、不是關鍵字，所以她不會拿那七個字去比對——她直接把最後看到的幾件事列
出來，每一筆一樣掛著時間與出處，而且會先講一句「我把它當成時間問題了」，你才知道
答案為什麼跟你打的字對不上。判斷刻意做得很膽小：句子裡只要還剩下任何講得出內容的
詞（「剛剛那個電話號碼」），就照舊走搜尋——把你真正想問的東西弄丟，比多查一次糟
得多。

而走搜尋的時候，「剛剛」那兩個字**不會跟著進去比對**。中文沒有空白，整句話會被
當成一整串子字串去找，而沒有人的螢幕上寫著「剛剛那個優惠方案」——所以頭尾的時間
詞和虛字先剝掉，中間原樣留著。加了「剛剛」之後變成零筆，是這個產品最容易失去信任
的那種答案。`sister query` 和字母人共用同一份規則，不會一個說有、一個說沒有。

問「**電話**」的時候，答案是那串號碼本身（`★ +886800080123`，底下附著螢幕上原本
那行「客服專線 0800-080-123」和它被看到過幾次），不是一堆剛好提到電話的字。螢幕上
從來沒有出現過「電話」兩個字，全文比對永遠接不起來——但記下那串數字的時候，它已經
被標成一支電話號碼了。這一層本來只有終端機有，字母人只會做全文比對：同一句話，
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

字母人也會告訴你**現在到底有沒有人在錄**，而且那句話底下就是把她開起來的按鈕。
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
現在的狀態換），也可以 `sister stop`。**結束字母人會連記錄一起停**，而正在錄
的時候那一項就直接寫成「結束（記錄也會停）」——不然他關掉的是唯一看得見的
那個視窗，而螢幕還在被記錄。停止走的是一個檔案而不是把行程砍掉：被砍死的
recorder 不會寫完 session、不會收掉心跳，於是接下來 16 秒她會宣稱自己還在錄。
**她還在開資料庫的那幾分鐘按下去也算數**——那個請求會等她開完，然後她一個字都
不記就直接收工（一顆存了一年的資料庫第一次開起來要重建索引，那段時間不短）。

停得掉是這個產品的前提，所以暫停在四個地方都按得到（全域熱鍵 `Ctrl+Alt+P`、
字母人的 `⏸`、系統匣選單、`sister pause`），而且**不會自己恢復**——會自己醒來的
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

字母人**沒有任何圖檔**——整個角色是一個字加幾條 CSS，連應用程式圖示都是把
同一份 CSS 渲染出來的。所以她離線、可縮放，而且沒有任何一張圖的授權需要解釋。

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
- **路線**：先做「搜得到」（Sister 1.0），再做「想得對」（大腦），最後做「接得了手」
  （hands）——每一步都用重播評測的數字守門。
