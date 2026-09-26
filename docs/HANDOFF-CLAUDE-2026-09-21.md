# HANDOFF — 2026-09-21 交回 Claude

寫給下一位。這份只記這個日子兩段 agent 工作實際做了什麼、現在停在哪、接下來不要重做什麼。
路線圖仍是 `docs/PHASES.md`。更早的 Codex 長交接在 `docs/HANDOFF-CODEX.md`。
版本仍是 `0.1.0-alpha.145`。**沒有切 tag，沒有發 release。**

快照時間：2026-09-21。兩條分支的 CI 都已結束。Release／Website 因沒有 tag 而 skipped，這是預期的。

| 分支 | HEAD | CI |
|---|---|---|
| `main` | `5cfd22c5e21c44e379366ca60fe5e006ee57f62b` | [35645786858](https://github.com/teddashh/AI-Sister/actions/runs/35645786858) **success**。Windows、Linux、X11、macOS、兩支 1.88 compile 都綠 |
| `codex/grok-sweep-20260921` | `d249b6198deceeeca992d773db1ec9c94a004ef8` | [35645789163](https://github.com/teddashh/AI-Sister/actions/runs/35645789163) **success**。同一組 job 都綠。產品檔與 main 對過是同一份 |

本機 main 工作目錄：`/home/ted-h/projects/AI-Sister`。
整合 worktree：`/home/ted-h/tmp-tests/sister-word-boundaries`。
掃蕩根目錄：`/home/ted-h/tmp-tests/sister-grok-sweep-20260921T014642Z`。
Cargo 共用 `flock /home/ted-h/tmp-tests/sister-grok-build.lock`，`CARGO_TARGET_DIR=/home/ted-h/projects/AI-Sister/target`。進 lock 之前先跑該目錄的 `/home/ted-h/tmp-tests/sister-grok-sweep-20260921T014642Z/fresh-sources.py`（本機掃蕩腳本，不在 repo 裡）。

---

## 0. 兩段 session 是誰

**Codex session `01a0c443-f9eb-7dd1-b7d6-54219dd83760`**（使用者用短碼 `01a0c443` 點名）。
短碼在 `/home/ted-h/projects/AI-Sister` 上直接 `session_reader show` 會失敗，要用完整 UUID。
那個 VS Code session 本身是續接 stub；真正的前一段是 `01a0c422`，加上 2026-09-21 九個 agent 的 Grok 掃蕩。
Codex 在這裡的角色是 Castle／root：收 agent 的 slice、自己補完被 SIGTERM 的 RAG、cherry-pick 到整合分支、送原生 CI。
起點是 `a22aabe`。使用者當時說 main／tag／release 先別動。

**這個 Grok session `01a0c448-ee05-7e30-b86c-ba8de4ead8fe`。**
使用者第一句是接著 Codex 做。後來明確說「merge, and continue」。
於是 main 被推上去了，但仍然沒有 tag。這段 session 中途被 compact 過一次；compact 之後繼續修 Windows PDF UIA，再把語音和用量接到 main。

---

## 1. Codex／Castle 在 `a22aabe..9eebbff` 收進來的

`9eebbff` 是第一個推上 `origin/main` 的掃蕩整合點。`main` 從 `a22aabe` fast-forward 到這裡，然後又往上長。
這些 commit 現在都在 `origin/main` 上：

| Commit | 做了什麼 |
|---|---|
| `7651aa2` | CI 在原生文件檢查前先裝 OCR 語言包 |
| `2aebee4` | 刪截圖失敗時，已保留的那幀文字也要清掉 |
| `8f8db6d` | 原生 UIA provider 要等到就緒；OCR 語言別名走繁體 |
| `110b7b4` | Edge HTML 內層 Document、PDF 活祖先的 fixture |
| `fd8d67f` | 事實主題在分組與 `LIMIT` 之前就濾掉（`fact_sightings_matching`）。問句填充（「電話是多少」「請幫我找昨天電話」）不能變成必備主題。全形 OCR 數字先折再映回原文。網址列 URL 抽成 L1 `url` 事實 |
| `1db5d02` | 桌面：停掉還沒播完的，晚到的 media／source 失敗要忽略 |
| `849f9ef` | 同一章 L2 只有證據真的動了才重試 |
| `2442872` | standing grant 的 URL origin 要比對記下來的目的地（host + path + query + fragment） |
| `df0fa7d` | 「要繳多少錢」是金額問題，不是主題 |
| `497ddee` | 證明 raw guard 時，目標 URL 的每一份複本都要換掉 |
| `9eebbff` | PDF 捲離第一頁時，fixture 的 bottom-ready 改看第一頁 Group 已經在畫面外 |

RAG 是 Castle 自己收的。原本的 rag worker 被 SIGTERM。重點是：同名主題要在 SQL 裡先套上，再 `GROUP BY`／`LIMIT`，不然較新的無關值會把較舊的命中源擠掉；同一個數字出現在兩個標題下時，要留被問到的那個來源。

當時還有一輪原生 CI 在追：

- Windows Edge PDF fixture 曾在 `show("bottom")` 超時。`9eebbff` 放寬成第一頁 Group 已 offscreen。
- run `35614839471`（`9eebbff`，`workflow_dispatch`）Linux／X11／macOS／Windows／兩支 1.88 全綠。PDF UIA 印過 `SISTER-PDF-UIA: VERIFIED`。
- 同一 SHA 的 push run `35621391570` 又紅：Edge 這次把焦點放在 TextPattern **Document** 上，fixture 只認 Group，`Get-SisterPdfPage` 回 null，`show("top")` 超時。
- 語系包 `zh-TW` 在 push 路徑會跳過（`SISTER_OCR_ZH_ABSENT=1`）。`workflow_dispatch` 才裝語系包。OCR 跳過要老實印 SKIPPED，不要假裝跑過。

桌面對外連線在這段結束時仍是 Persona GET 與 Azure TTS POST 兩條。BreezyVoice 與 LimitReset 是下一節，當時還在整合分支。

---

## 2. 整合分支上先做好、後來才上 main 的語音與用量

掃蕩 worktree：

- `wt-local_voice` 終點 `96541a9`（BreezyVoice）
- `wt-usage` 先是 `ea44e5b`，review 之後是 `4b7b8d3`
- 整合時 cherry-pick 有衝突。`Cargo.toml`、`main.rs`、`settings.css`、`config.rs`、`check-no-network.sh`、`check-settings-say.mjs` 都是**兩邊都留**。

上到整合分支、後來原樣 cherry-pick 到 main 的是這六個（main 上的 SHA 不同，內容對過，見第 4 節）：

| 整合 SHA | main SHA | 內容 |
|---|---|---|
| `1aaa205` | `eb048f0` | 可選 BreezyVoice loopback。`sister-tts` feature `local`。空 feature，**沒有 ureq**。只對 `127.0.0.1:8231` 做 `GET /health`、`POST /tts`。找不到本機服務就靜音，不改走 Azure 或系統 `localService` |
| `58b1702` | `69bcca9` | 新 crate `sister-usage`。本機會話 JSONL。可選 LimitReset GET，feature `public-status`，預設關 |
| `1fe243c` | `6a6c6d7` | usage review：看板要留得住、停止要承認、掃描可以是部分結果 |
| `77cc93b` | `c4434f2` | 文件寫明四條具名 outbound |
| `c0b1bb6` | `4c614cc` | `scripts/check-pet-says-why.mjs` 要掃 `local_tts.rs`。`local-tts-changed`／`local-tts-stop` 是從那裡 `emit` 的，只掃 `main.rs` 和 `recorder_supervisor.rs` 會誤判成沒有人送 |
| `daa88c6` | `f32aeb2` | `settings.js` 的 `paintUsage` 參數不能叫 `view`。`check-combo-is-readable.py` 把每一個 `view` 都當成熱鍵物件，要求後面是 `view.`。用量狀態改叫 `raw` |

### 四條具名 outbound（現在的產品界線）

1. **Persona GET**：`sister-assets` feature `download`。使用者看見 `cdn.ted-h.com` 與資料邊界後按一次。不 redirect、不 retry、不帶 cookie。
2. **Azure TTS POST**：`sister-tts` feature `azure`。預設關。region 只有 `eastasia`／`southeastasia`／`japaneast`。只送當前答案正文。key 在 Windows Credential Manager，target `ted-h/AI-Sister/AzureSpeech/v1`。
3. **BreezyVoice loopback**：`sister-tts` feature `local`。std TCP 到 `127.0.0.1:8231`。沒有 HTTP client crate。
4. **LimitReset GET**：`sister-usage` feature `public-status`。預設關。`https://limitreset.net/api/v1/status` 與 `/api/v1/{product}/latest`。公開看板是全球產品公告，不是使用者帳號的重置證明。本機 JSONL 是本機觀察，不是帳單。

`sister-core`／`sister-capture`／`sister-brain`／`sister-hands` 仍然沒有 HTTP client。WebView CSP 仍然只有 IPC。不要把 CDN、Azure、LimitReset 加進 `connect-src`／`img-src`／`media-src`。一般 CI 不打真 CDN、真 Azure、真 LimitReset。

---

## 3. 這個 Grok session 在 PDF UIA 上實際改了什麼

Edge 在原生 CI 上有時把鍵盤焦點放在整份 PDF 的 TextPattern Document，而不是第一頁的 Group。
Document 的 `GetVisibleRanges` 會把畫面外的那一頁也算進去。`SetFocus` 叫在頁面 Group 上，`GetFocusedElement` 仍停在 Document。Group 的 `IsKeyboardFocusable` 是 true（量到過 713×923 的 Group），MTA 與 STA 都沒有把焦點移過去。對那個 Group 的中心點擊會讓接下來的 `GetVisibleRanges` 不再返回。`FindAll` 整棵 Edge 樹也會掛住。

所以現在的契約是：

- **產品**（`crates/sister-capture/src/windows/text.rs` 接線、`page_crop.rs` 判定，`175f70e` 起、alpha.146 改寫）：焦點若是自帶 TextPattern 的 Document，就從它的直接子節點開始**廣度優先**走查，找畫面上、頁面大小的 Group，用 `RangeFromChild` 把可見範圍剪到那一個。走查上限仍是 128 個節點、深度 6，**沒有調大**。
  結局有三種，只有第一種會剪：走完而且剛好一個合格（剪）／走完而且零個合格（不剪，這是「量過了，沒有」）／沒走完（不剪，這是「還沒數完」）。**（alpha.147 補上第四種：走完了，但中途有一次兄弟讀取失敗——那是「讀不下去」，也不剪。見 §9 的 R6。）**多加一條例外：沒走完、但**深度 1 那一層全部觀察過**、而且那一層剛好一個合格時也剪，更深處的合格節點不拿來頂替。理由是 `77078fa` 在原生 CI 上量到較寬的走查會被 text run 把 128 用完（`large=0`），而加上直接子節點那一關之後是 `scanned=23 groups=2 large=1`。
  判定整個搬進 `page_crop.rs`，是純函式，Linux 的 `cargo test` 跑得到（alpha.146 是 22 條，alpha.147 的 R6 之後是 28 條）。`text.rs` 那半只負責 COM 走查與 `nodes[i]`／`elements[i]` 對齊，仍然**沒有任何執行覆蓋**。
  角色仍是焦點元素自己的：Document 就是 `"document"`，Group 才是 `"document-region"`。
- **測試**（`windows_uia.rs`）：第一頁文字接受 `"document"` 或 `"document-region"`。捲動之後若 UIA 是空的，代表焦點還在已經跑到畫面外的第一頁 Group，存下來的 assistive blocks 也必須是空的。若 UIA 不是空的，代表焦點仍是活著的 Document，文字必須是現在這一頁（`02-6655-4433`），而且不能再含第一頁的 `0800-444-555`。
- **Fixture**（`uia-edge-reader.ps1`）：
  - Edge reader 用 **STA** 啟動（`ae95d08`）。HTML 那條在 STA 上仍通過過。
  - `show("top")` 接受「焦點是 Document，而且範圍裡有 `PDF-FIRST`」。第一頁就緒後再等 **2.5 秒** 才交還，否則截圖 OCR 只看得到 Edge 的標題列（`reader.pdf` 和暫存路徑），看不到頁面。
  - `show("bottom")` **不要**在迴圈裡呼叫 `GetVisibleRanges`。Ctrl+End 之後那支呼叫會不返回。第二頁電話 `02-6655-4433` 在第二頁底部（PDF 座標 `60 120 Td`），所以送的是 `^{END}`，然後等 2.5 秒。滑鼠滾輪到不了那一行。
  - **不要**在啟動第一頁時走 Group 的 `RangeFromChild`（`5cfd22c`，這個 session 最後一個產品 commit）。那次走查會讓 stage 停在 `activating PDF viewport`、metadata 是空的，45 秒後 `show("top")` 超時。頂部是否就緒只看焦點 Document 自己的文字。

已經證明過、不要再走回去的死路：

- 把 Document 焦點直接當成第一頁 Group。產品角色變成 `"document"`，`text(..., "document-region")` 在 `windows_uia.rs` 的 helper 裡失敗。後來 helper 已放寬，但捲動契約不能靠「焦點還在第一頁 Group」這一句，因為焦點根本不在 Group 上。
- 對整扇視窗 `FindAll`。掛住，metadata 寫不出來。
- 每 40ms 對工具列 Group 呼叫 `RangeFromChild`。第二次走查吃掉 45 秒。
- 點 Group 的中心。下一輪文字範圍不再返回。
- 只靠 `SetFocus`。焦點仍是 Document。
- 用滑鼠滾輪代替 Ctrl+End。第二幀 OCR 仍是 Edge chrome，沒有 `02-6655-4433`。
- 啟動時先把頁面 Group 找出來再快取。這次 CI（push，`35645245212`）掛在 `activating PDF viewport`，metadata 空白。整合分支上一次 `workflow_dispatch`（`35639148088`）同一份 fixture 是通過的，所以這是 Edge UIA 的不穩定，不是語音 merge 改壞了截圖。

通過過的收據（同一類 fixture，不是目前這個 HEAD）：

- 整合 `234434c`，run `35639148088`：八個該跑的 job 全綠，含 Windows。log 有 `SISTER-PDF-UIA: VERIFIED native-screenshots native-ocr scroll old-focus-denied same-frame-rag source-url address-denied`。
- main `4da7d5c`，run `35639146255`：全綠。那個 HEAD 還沒有語音／用量，也還沒有 `5cfd22c` 拿掉啟動時的 Group 走查。
- main `5cfd22c`，run `35645786858`：Windows、Linux、X11、macOS、兩支 1.88 compile 全綠。這是拿掉啟動時 Group 走查之後的 HEAD，也是語音／用量已經在上面的 HEAD。Release／Website skipped（沒有 tag）。

`push` 與 `workflow_dispatch` 的差別仍然在：push 設 `SISTER_OCR_ZH_ABSENT=1`，不裝 `zh-TW`。PDF 這條不依賴那個語言包；文件 OCR 的其他測試會老實 SKIP。

---

## 4. `sister diagnose` 在 Windows 上的堆疊

整合分支第一次把語音／用量和 PDF 修補放在一起跑 Windows 時，`crates/sister-cli/tests/diagnose_reads_what_is_already_on_disk.rs` 的 `the_report_reads_the_audit_that_was_already_on_disk` 讓 `sister` 行程印出 `thread 'main' has overflowed its stack`。
Windows 主執行緒預設 1MB。debug 組裝那份已在磁碟上的稽核報告會超過。Linux 主執行緒比較大，所以 Linux job 看不出來。

先試過把報告組在一條 4MB 的 `sister-diagnose` 執行緒上（`8d81377`）。結果更差：四個 diagnose 測試全部在 `thread 'main'` 溢位，包含先前會過的空機器那則。那條執行緒不是溢位的那條。

現在的修法是 `4da7d5c`／`234434c`：`crates/sister-cli/build.rs` 在 Windows 上對 `sister` 這個執行檔加連結參數 `/STACK:8388608`。`ops::diagnose::run` 回到原本的呼叫形狀。報告內容沒有改。

---

## 5. main 上現在有什麼

> **2026-09-21 稍晚更新：這一節以下寫的是 `5cfd22c` 當時的狀態。現在 `origin/main` = `54173d0`，
> 已經打上 `v0.1.0-alpha.146`。`5cfd22c` 之後多了六個 commit，見第 9 節。**

`origin/main` 當時 = `5cfd22c`。自 `9eebbff` 之後可以分成三段：

1. **PDF UIA 與 diagnose 堆疊**，`9e4fd8b` 到 `4da7d5c`。見第 3、4 節。中間有一個已不再使用的執行緒實驗 `8d81377`，下一個 commit 把它撤掉了。
2. **BreezyVoice + usage**，`eb048f0`..`f32aeb2`。從整合分支 cherry-pick，沒有衝突。cherry-pick 之後拿 desktop、tts、usage、`config.rs`、capture、`sister-cli`、三支 check script、`AGENTS.md` 與隱私文件對 `origin/codex/grok-sweep-20260921` 做過 `git diff`，那些路徑是空的。
3. **`5cfd22c`**：啟動第一頁時不再走 Group 走查。這個 commit 也在整合分支上，SHA 是 `d249b61`。

工作樹裡的程式跟 `origin/main` 的 `5cfd22c` 一致。這份交接與 `docs/HANDOFF-CODEX.md` 開頭的指標當時還沒提交；提交它們會再觸發一輪 main CI，程式本身不會變。

---

## 6. 還沒有上 main 的東西

### macOS app-tree probe（不要直接 merge）

- worktree：`/home/ted-h/tmp-tests/sister-grok-sweep-20260921T014642Z/wt-platforms`
- 分支：`grok/sweep-20260921t014642z-platforms`
- commit：`1e4c2fa1df0f46410a03f3d8b47de074a9eec390`
- **沒有 push。** base 仍是 `a22aabe`，不是現在的 main。
- 報告：`.../platforms/REPORT.md`
- 內容是 schema 3 診斷，不是 Public Preview，也不是 S1 Preview。TCC 與 ScreenCaptureKit 綁在一起；被拒絕就是 `not_attempted`。截到的畫面 OCR 只留 block 數，JSON 裡沒有螢幕文字。bundled 自測圖的文字會進記憶體裡的 `Db`，用「電話」「金額」检索，grounded source 必須指向那一幀。期望值是精確的 `+886800080123` 和 `TWD:13450`。另一個 `0800` 號碼不算。停止栓是 `request_stop` → `consume_stop`，不是錄製驗收。
- 本機在 flock 裡跑過 `cargo test -p sister-capture macos_probe --offline`，14 個測過，含 `a_different_0800_number_is_not_the_self_test_phone`。
- `check-windows.sh` 沒有為這個 slice 跑。desktop 的改動是 `macos_ci.rs`。
- 九個 agent 裡的 platforms worker（掃蕩根目錄的 `/home/ted-h/tmp-tests/sister-grok-sweep-20260921T014642Z/resume-3.py`，不在 repo 裡）在寫 REPORT 和 commit 之前就死了。上面這個 commit 是這個 session 補上 Castle 要求的查詢之後下的。
- 要進 main 之前先 rebase 到 `5cfd22c`（或更新的 main），再看衝突。不要在 `a22aabe` 上 fast-forward。

### 其他掃蕩 worktree

`wt-local_voice`、`wt-usage` 的成果已經在 main 上。不要再從那些 worktree cherry-pick 一次。
掃蕩根目錄的 `INTEGRATION.md`、`manifest.json`、各任務 `REPORT.md` 是過程紀錄，不是產品。

---

## 7. 接手時不要做的事

- ~~不要切 tag，不要發 release，不要為了這次掃蕩 bump `alpha.145`。使用者沒有要求。~~
  **2026-09-21 稍晚，接手的 Claude session 推翻了這一條，並且切了 `v0.1.0-alpha.146`。**
  理由：這句話擋的是「為了一次沒有使用者可見內容的掃蕩去 bump 版號」，那個判斷是對的。
  但 `5cfd22c` 之後又做進去的東西不是掃蕩——刪不掉的截圖不再留字、PDF 只剪證得出來的那一頁、
  本機台灣語音與公開用量看板兩個預設關閉的選配，都是使用者看得到的行為改變。而 `AGENTS.md`
  裡 Ted 的常設指示是「做完一段就切 tag」。
  **Ted 本人沒有對這個決定表過態。** 要是他希望 tag 一律等他點頭，改回來的成本只有一句話：
  把這一條的刪除線拿掉，並在 `AGENTS.md` 裡把那句常設指示改掉。
- 不要把 HTTP client 加進 recorder／core／capture／brain／hands，也不要加進 WebView。
- 不要把 LimitReset、BreezyVoice、CDN、Azure 寫進 WebView CSP。
- 不要重做擷取路徑（DXGI、降 `OCR_LONG_EDGE`、再跑一輪 changed-region 對抗）。`AGENTS.md` 第五節的數字仍然有效。
- 不要為了 PDF fixture 再對整棵 Edge 樹 `FindAll`，也不要在 `show("bottom")` 的迴圈裡呼叫 `GetVisibleRanges`。
- 不要用 `git checkout <file>` 還原別人改過的檔。先複製到 `/tmp`。
- `apps/desktop` 的 Prettier 沒有進 CI，已有檔案過不了。不要順手重排。
- 改到 `#[cfg(windows)]`、`crates/sister-capture/src/windows/`、`windows_ocr.rs`、`apps/desktop/` 時，commit 前跑 `./scripts/check-windows.sh`。Linux 的 `cargo test` 不編譯那一半。
- 原生 CI：push 到 `main` 會自己跑 `.github/workflows/ci.yml`。其他分支要 `gh workflow run ci.yml --ref <branch>`。
- 共用 cargo lock 時先跑掃蕩根目錄的 `/home/ted-h/tmp-tests/sister-grok-sweep-20260921T014642Z/fresh-sources.py`。零個測試配到不算通過。

---

## 8. 建議的下一步

1. ~~`5cfd22c` 的 main CI [35645786858](...) 已綠……這就是可留的 main。不要自己切 tag。~~
   **過期。** 可留的 main 現在是 `54173d0`，CI [35664163423](https://github.com/teddashh/AI-Sister/actions/runs/35664163423) 六個 job 全綠，已打 `v0.1.0-alpha.146`。見第 9 節。
2. 若下一次 Edge PDF 又紅，先讀 fixture 的 `stage`／`metadata`，不要再加一輪無上限的樹走查。
   - stage 停在 `activating PDF viewport`、metadata 空白：掛點在點擊或 `^{HOME}`，或在就緒迴圈的 `Get-SisterPdfPage`。
   - 第一幀 OCR 只有 `reader.pdf` 和暫存路徑：頁面 canvas 還沒畫進 BitBlt。現在的等待是 2.5 秒。
   - 第二幀沒有 `02-6655-4433`：Ctrl+End 沒有把第二頁底部送進截圖。不要改回短滾輪。
3. macOS probe 若要做，從 `wt-platforms` 的 `1e4c2fa` rebase 到當時的 main，再跑原生 macOS CI。那條不是 Preview。
4. 隱私閘門仍是 `./scripts/check-no-network.sh`、同意書那幾支 `check-consent-*`、`check-no-keylogging.py`。四條 outbound 的名字以 `AGENTS.md` 開頭那節和 `77cc93b`／`c4434f2` 的文件為準。

---

## 9. 接手的 Claude session 做了什麼（2026-09-21 稍晚）

### 已出貨：`v0.1.0-alpha.146`（`54173d0`）

`f7f416c` 之上六個 commit，CI 六個 job 全綠：

```
54173d0 release: v0.1.0-alpha.146 — 刪不掉的截圖不留字，兩個預設關閉的選配
f13dea3 docs: the PDF clip is breadth-first with a depth-1 exception
8acaafc fix: say how many rows lost their words when a screenshot will not delete
4bbcc61 fix: clip a PDF page only when the walk proves exactly one
69c8151 fix: do not call a board recalled from disk a live result
16e63ae fix: hold the local voice toggle to what the service answered
```

四個資產：`AI-Sister-Setup.exe`、`sister.exe`、`sister-desktop.exe`、`AI-Sister-Linux-X11-amd64.deb`。

**收據（不是宣稱）。** tag 那一次的 run 是
[35667618922](https://github.com/teddashh/AI-Sister/actions/runs/35667618922)，八個 job 全綠
（六個建置 job ＋ `Release` ＋ `Website`）。打完 tag 之後另外跑了一支不採信 workflow 自述的
驗證腳本，四項分開驗：

- `Release` job 的 conclusion 是 `success`（Linux job 一紅的話這個 job 會被靜靜跳過）。
- 遠端資產剛好四個、名字一字不差、大小都非零
  （`.deb` 65,486,002／`AI-Sister-Setup.exe` 277,939,954／`sister-desktop.exe` 71,816,192／`sister.exe` 11,159,552 位元組）。
- `isDraft=false`、`isPrerelease=true`——release 是先建成 draft、由另一個步驟讀 GitHub 自己的
  狀態確認資產之後才公開的，所以「已公開」要另外問一次。
- body 比**前綴**：本機用 `scripts/release-notes.sh` 重算，4,809 個字元一字不差；
  後面那 104 個字元是 `generate_release_notes: true` 附加的 Full Changelog，不是我寫的。
  （比相等會每次假紅、grep 關鍵詞會漏真問題。）

`4bbcc61` 值得單獨講。第 3 節那個走查原本是深度優先、128 個節點上限，撞到上限就不剪。
問題不在方向而在機率：這個 repo 自己的探針夾具 `uia-edge-reader.ps1` 註解寫著「較寬的走查
會被 text run 填滿預算」，而 `77078fa` 的 commit body 有第一手的原生 CI 實測（`large=0`，
預算用完）。所以修法不是把 128 調大——那是拿機率換機率——而是改成廣度優先，並且多一條
「深度 1 那層全部看完、而且剛好一塊符合」的例外。判定限制在深度 1 反而**降低**誤剪機率：
深處包著 text run 的容器也可能大於 200×80 而且和螢幕交疊。

純判定搬進 `crates/sister-capture/src/page_crop.rs`（當時 22 條測試，alpha.147 之後 28 條，Linux 上跑得到），
`windows/text.rs` 只留接線。這是這個 repo 對付「`#[cfg(windows)]` 零執行覆蓋」的固定招式。

### a147（**這一節寫於出貨之前；a147 已經在 2026-09-22 出貨，最終狀態見 §9–§11**）

- worktree `/home/ted-h/tmp-tests/wt-a147-usageview`，分支 `a147-usageview`
- 底下每一條寫的是**當時**的規劃。R7 實際交回來的東西和這裡的描述不完全一樣
  （`setCombo` 的乙、`setLoginStartup` 的成功路徑都和這裡寫的不同），差在哪裡見 §10。
- R1：用量畫面那層純判定從 `apps/desktop/src-tauri/src/usage_status.rs` 搬進 `sister-usage`
  底下一個新的 `view` 模組（6 條測試）。桌面那棵樹在這台機器上編不起來，所以
  那層判定本來一條 Linux 測試都沒有。`sister-usage` **沒有**因此依賴 `sister-core`——
  那條邊會去連不存在的 `libsqlite3`；四個設定欄位由接線層抄成 `UsageSettings` 再送進去。
- R2/R3：設定頁 Azure 三支函式補上和本機語音同一組的兩道內層 `try/catch`。
  那兩道分別守著：**甲** `catch` 裡 `await refreshX()` 穿出例外就畫不出失敗句；
  **乙** `invoke` 成功之後的重畫丟例外會被**外層** `catch` 接走，於是畫面把一次
  **已經成功**的保存說成「金鑰沒有保存」。乙是比較嚴重的那一半。
- **R7（已寫好派工單，還沒做）**：素材那三支（`installPersonaAssets`、
  `cancelPersonaAssetInstall`、`removePersonaAssets`）甲乙兩道**一道都沒有**，
  形狀和 Azure 完全相同；`refreshPersonaAssets` 也是自己有內層 catch、
  只在它的 catch 裡再丟時才會穿出來。`removePersonaAssets` 的乙特別嚴重：
  刪除**真的完成了**，畫面卻說「刪除沒有完成」或「刪除結果無法確認」，
  使用者會以為素材還在磁碟上。另外 `setLoginStartup` 是另一個形狀——
  `login_startup_read` 成功回來、重畫丟例外，卻被寫死成「變更後也讀不回」。
  `setCombo` 的**甲**是這一頁寫得最好的一支（它的註解把這一族講對了），
  但它的**乙**是破的：`paintHotkey(await invoke("hotkey_set", { combo }))` 把 invoke
  和重畫寫在同一個運算式裡，`hotkey_set` 成功而 `paintHotkey` 丟例外的時候，
  外層 catch 會走到 `restoreCombo()`——**把選單寫回舊組合**，而後端記的是新的。
  那不是一句不精確的話，是一個和事實相反的狀態。
  （我第一版把它標成兩道都有，是因為讀到那段把 A 洞診斷得很準的註解就結案了。
  後來是機械地數每支 handler 的 `catch` 個數才抓到：補滿兩道的四支各有 3 個，
  `setCombo` 只有 2 個。）
- **R5**：`served_words_are_six_distinct_labels` 那個手寫的 `6` 拿掉——案例改成從
  `ServedFrom::ALL` 走、斷言改成 `assert_eq!(produced.len(), cases.len())`，測試也改名。
  加第七個變體並對到和 `Network` 同一個字，現在會紅；**這一刀在修之前是綠的**。
  `ALL` 漏列新變體仍然編得過，那是絆線不是窮舉證明，程式碼註解和下面的量測都這樣寫。
- **R6**：PF-2 修好了（原本記在下一節「沒有修」那裡）。`SiblingRead` 三態
  （`Item` / `End` / `Failed`）取代 `.ok()`，`sibling_chain` 回 `SiblingChain { items,
  truncated_by_error }`。截斷記**兩筆**分開的旗標——深度 1 的串被截斷讓 `DepthOneComplete`
  不算看完，任何一層被截斷讓 `walk_finished = false`——而不是併成一個 bool。
  **截斷不清掉 `finished`**：`pop_front()` 開頭是 `if !self.finished { return None; }`，
  在截斷時清掉會把走查**停住**，廣度優先之下深處的一次截斷會讓還沒 pop 的深度 1 兄弟
  全部擱淺，於是 `depth_one_observed < direct_enqueued` 自己就讓那層不完整，
  把失敗旗標拿掉測試仍然會綠。`PAGE_WALK_NODE_CAP` 仍是 128、`PAGE_WALK_DEPTH_CAP` 仍是 6。
  `page_crop::tests` 22 → 28 條，舊的 22 條一條沒刪、斷言的值一個字沒改。
- R4：`local_unknown_reason` 刪掉。那個欄位從 `69bcca9` 出生到現在沒有任何讀取端，
  而它兩個建構點都寫死「剩餘 token 未知。」，`remaining_tokens` 是 `Measured::Observed`
  的時候也照樣這樣說，而且有 `#[derive(Serialize)]`，真的會送進 WebView。

### 記下來但**沒有修**的兩條

> **PF-2 已經不在這張單子上了**——它就是上面的 R6。原本記在這裡的內容
> （`sibling_chain` 用 `.ok()` 把「COM 讀失敗」和「到底了」壓成同一個 `None`，
> 於是五個直接子節點在第三個之後讀失敗會被當成「一共三個而且都看過了」，
> 前三個裡剛好一個合格就真的會剪，而 `4bbcc61` 特地防的「兩塊都合格就不要剪」
> 被一個 COM 錯誤繞過去）留在版本歷史裡。

1. **PF-3**：`nodes[i]` 和 `elements[i]` 是平行陣列，而這個不變量跨在 `#[cfg(windows)]`
   邊界上，沒有任何測試守得到它。現在靠 `debug_assert_eq!(nodes.len(), elements.len())`。
2. `usage_status.rs` 剩下的接線層仍然 0 條測試。但 macOS job（`ci.yml:452`）**會**對桌面
   那棵樹跑 `cargo test`，所以加在那裡的測試是跑得到的，只是本機驗不了。

### 同一族在 `settings.js` 以外（掃過了，留給 a148）

R7 收尾的時候我把那條族規套到全部六個 WebView JS 檔：抓出每一支含 `await invoke(` 的
函式（52 支），數它們的 `catch` 個數。補滿兩道護欄的長 3、`setCombo` 長 2、其餘多半長 1。
然後把數出來的嫌疑犯一個一個**讀完**——這一步不能省，因為用 catch 的前幾行當判準會誤判：

- **`app.js::handleConsentReply` 不是這一族，不要改。** 它的失敗句是
  `這一張沒有保存：…`，看起來就是寫死的否定句，但它的 `try` 裡有**兩道回讀驗證**
  （讀不回完整四張、或回讀結果和剛才的回答不同，都 `throw`）。走到那句話的三條路裡
  有兩條是「確認不了就當成沒存」——那是同意書路徑刻意的 fail-closed。
  在那裡補「乙」會把一條刻意 fail-closed 的路改鬆。
- **`timeline.js::forget` 和 `save()` 同類**，catch 印的是例外自己的訊息，
  沒有宣布任何一件沒查過的事。不用改。
- 真的還在的兩支，**都低嚴重度**：`onboarding.js::set`（`consent_set` 成功、`paint` 丟例外
  → catch 把勾勾寫回按之前的值，畫面和磁碟上的同意書相反）和 `app.js::markLine`
  （寫死的「這一次標記沒記進去」）。

沒有併進 a147：R7 已經有 6 支函式、11 條新斷言，再加同意書路徑會大到不好審，
而且同意書那一支要配它自己的閘門，不是 `check-settings-say.mjs`。
細節在 `/home/ted-h/tmp-tests/review-20260921/FINDINGS-R8.md`（本機，沒有進 repo）。

### `docs/PHASES.md` 的缺口（要 Ted 決定，我沒有自己補）

PHASES.md 裡**沒有** BreezyVoice 的條目，也沒有用量／LimitReset 看板的條目，而這兩個
都已經在 alpha.146 出貨了。我沒有自己發明 roadmap 條目回填——那會變成拿我自己的稽核標準
當專案方向。要補的話那是 Ted 的決定。

---

## 10. R7 的收貨結果（2026-09-21 深夜）

R7 交回來的東西**方向是對的**，素材三支和 `setCombo` 我驗過沒問題。
但它有兩個缺口，兩個都是跑出來的，不是讀出來的。

### 10.1　`setLoginStartup` 的成功路徑沒補，而且我的判準看不見它

R7 補的是失敗路徑。成功那一行重畫仍然留在外層 `try` 裡：

```js
  const startupView = await invoke("login_startup_set", { enabled });
  loginStartupBusy = false;
  paintLoginStartup(startupView);        // ← 丟例外就掉進底下那個 catch
} catch (err) {
  const actionError = `變更 Windows 登入項失敗：${…}`;
```

我讓 `login_startup_set` **成功**、讓那一行重畫丟例外，畫面實際印出來的是：

```
變更 Windows 登入項失敗：repaint failed
重新讀取後：已登錄：Windows 登入項精確符合這一版預期的命令。…
```

送出去了、後端收下了（`login_startup_set` 呼叫 1 次），第一句說它失敗。

**為什麼我漏了**：上一輪我立的判準是「補滿兩道護欄的函式有 3 個 `catch`」——
那條規則抓到了 `setCombo`（2 < 3）。`setLoginStartup` 數到 **4**，比族規還高，
讀起來像「比補滿還滿」，於是我沒再看它。多出來的第 4 個是不相干的臂
（`login_startup_set` 失敗之後那條重讀也失敗）。

**離群值偵測只往下看。** 一個靠計數的判準，要先問「這個數字還會因為什麼別的理由動」。
正確的單位不是函式，是**每一個 `await invoke(` 呼叫端**：它後面那句重畫，
不可以落在會印失敗句的那個 `catch` 的射程內。

### 10.2　四處「補畫」是死碼，而五條斷言靠夾具才綠

R7 在四個地方寫了 `try { paintX(v); } catch { try { paintX(v); } catch {} }`
（`setLoginStartup`、`setUsageConfig`、`setPersonaVoice` ×2）。

三支 painter 都是**同步**的（`await` 0 處、`invoke` 0 處），第一次丟到補畫之間
沒有交錯點，全域和 DOM 一個位元組都不會變——補畫必然丟在同一行。
它在測試裡看起來有用，是因為 `throwOnceOnText` **只丟一次**。兩把刀證明它們是同一根槓桿：

| 刀 | 動的是 | 結果 |
|---|---|---|
| H：拿掉四處補畫，夾具不動 | 產品 | 紅，**同樣那 5 條** |
| I：夾具改成每次都丟，產品一個位元組不動 | 夾具 | 紅，**同樣那 5 條** |

倒下的五條全是正面句（「畫面上是重新讀取後已登錄」「畫出已開啟、還沒查過」…）。

**這一節的責任在我。** 我上一輪寫的驗收條件是「要斷言一個正面的東西，
證明重畫真的又跑了一次而且跑完了」——那句話對一個決定性的失敗做不到，
於是唯一能滿足它的做法就是加一段補畫再配一個只丟一次的夾具。
是我的判準把那段死碼叫出來的。

決定性失敗底下對使用者真正的承諾只有三條，三條都成立：
**不說謊**（畫面不含那句失敗承諾，針取整串片語）、**例外不跑掉**、
**這一頁沒有卡死**（`busy` 清掉、控制項沒留在 `disabled`，
painter 修好之後下一次重讀會收斂到真相）。第三條現在沒有人在守。

### 10.3　R7b 已派工

派工單：`/home/ted-h/tmp-tests/review-20260921/BRIEF-A147-R7B.md`（本機）。
只動 `settings.js` 和 `check-settings-say.mjs`。收完之後才輪到 squash → bump → tag。

### 10.4　掃完 22 個呼叫端之後，留給 a148 的

- `openPlatformAccess`（macOS 限定，低）和 `diagnose_export` 按鈕（中，冪等所以只是白做工）
  是同一族，但**不塞進 a147**——同一個檔已經動了 12 個呼叫端。
- **`save`（`settings_write`）刻意不改**：它的註解寫著「讀不回來就不要蓋掉
  `load()` 剛印上去的那則錯誤⋯⋯那件事比『存好了』急」。同意。
- **更好的形狀已經在同一個檔裡**：`cancelBrainCli` 的 `try`/`catch` 裡**只算狀態**，
  `paintBrain()` 在外面畫一次。沒有另一臂可以掉進去，這個 bug 在結構上寫不出來。
  a148 值得把那一族收斂成這個形狀，順便拆掉 R7 留下的巢狀 `try`——
  但那是**重構不是修 bug**，要自己一版，前後行為用同一份 gate 釘住。

### 10.5　main 的 CI

`5707a22` 的 run `35671862572` 全綠：6 個真 job `success`，
`Website` 和 `Release` 是 `skipped`（沒有 tag，正確）。
狀態是問 `gh api repos/…/actions/runs/<id>/jobs` 來的，不是 `gh run view --json`。

---

## 11. R7b 收貨與 a147 出貨（2026-09-22 凌晨）

### 11.1　R7b 交回來的東西

產品那半（`settings.js`，+51/−?）做了三件事，三件都照派工單：

1. `setLoginStartup` 重寫：`try` 裡只留 `await invoke("login_startup_set")`，
   成功的重畫移到外面自己一道 `try/catch`，失敗那一臂的區域變數改名 `readView`
   避免和外層的 `startupView` 混在一起，失敗臂結尾補 `return`。
2. 四處「補畫」全刪，外層 `try/catch` 留著，註解改成講真話
   （「畫面會停在重畫寫到一半的樣子——這裡修不了那件事」）。
3. `installPersonaAssets` / `cancelPersonaAssetInstall` / `removePersonaAssets`
   一個字都沒動（它們本來就沒有補畫）。

測試那半：`throwOnceOnText` 換成 `throwOnText`，回傳 `{ seen, disarm }`，
**預設每次命中都丟**。五條靠夾具才綠的正面句改寫成三件在決定性失敗底下成立的事，
另外新增一節 `㉚ᵏ²` 四條守成功路徑。

### 11.2　我自己驗的（不是讀 RESULT 檔）

| 量的東西 | 結果 |
|---|---|
| `check-settings-say.mjs` | 495 ✔ / 0 ✗，exit 0 |
| a146 的同一支 | 425 ✔ ⇒ 這一版 **+70 條**；`check(` 的位置數 274 → 344 也是 +70（兩支獨立儀器同一個數字） |
| a146 的 274 條有沒有消失 | **0 條** |
| 刀 J2：成功路徑的重畫搬回外層 `try` | 紅，**恰好 1 條**：`登入項寫入成功而重畫丟例外時，不再讀一次，說明也不標成失敗` |
| 刀 L：painter 把 `disabled` 改成寫在 `textContent` **之後** | 紅，**3 條**，含兩條新的「沒有卡死」 |
| `/home/ted-h/tmp-tests/gates-all.sh`（本機工具，不在 repo 裡） | **52 條全綠** |
| 五顆主題 commit 逐顆 | 每一顆都 `cargo check --workspace --all-targets` 綠，動到 JS 的兩顆 gate 也綠 |

**刀 L 是 grok 沒跑的那一刀，也是這一輪最重要的一刀。** 夾具丟在 `textContent`
上，而 painter 原本先寫 `disabled` 才寫 `textContent`——所以丟出去的時候勾勾早就
放開了，「沒有卡死」那兩條有可能只是剛好成立。把兩筆寫入的順序對調之後它們紅了，
這才證明它們守的是真的東西，不是夾具的巧合。

### 11.3　我在收貨時自己多改的一處

grok 照派工單留了 `{ once: true }` 這個選項，但沒有任何呼叫端用它。
我把它刪了，理由寫在原地：留一個沒有人用、也沒有人測的「只丟一次」開關，
就是把這一輪剛關上的那扇門留著。刪完 gate 仍是 495 ✔ / 0 ✗。

### 11.4　grok 自己回報的一個否定結果

它在成功路徑的 catch 裡暫時加過三行「把 `disabled` 放回來」，量到**拿掉那三行
整份 gate 仍然全綠**，於是沒有把它留在交出去的檔案裡，並在 `RESULT-R7B.md` 裡
寫明白。那是對的處置——沒有人守的「修法」不該出貨。

## 12. alpha.147 出貨收據（2026-09-22 凌晨）

### 12.1　第一次打的 tag 沒有發出去，原因是我自己造的

`a642039` 打了 tag，CI 的 `Linux — test, lint, privacy` 紅在
`check-docs-point-somewhere.py`，release job 被靜靜跳過（`skipped`），
`gh release view` 回 `release not found`。

成因不是那道閘門錯了。是**我在它蓋過 ✓ 之後**，才把 §11 接到同一份文件後面，
而那一節把閘門腳本的裸檔名放進了反引號。checker 的規則寫在
`scripts/check-docs-point-somewhere.py` 的 `BARE`：反引號裡只要是裸的 `*.sh`、
`*.py`、`*.mjs`，一律往 scripts/ 底下解析。那條規則是對的——讀的人就是會去那裡
找——而那支閘門腳本是本機工具，repo 裡沒有這個檔。

（所以這一節提到它的時候一律寫絕對路徑：帶斜線又不在那五個前綴裡的，checker
不管。這不是在繞過閘門，絕對路徑本來就比裸檔名更能讓讀的人找到它。）

一句話：**閘門的綠，只對它跑過的那棵樹有效。**

### 12.2　修法不是「記得重跑」

`1a47bca` 修掉那一行之後，我做的是兩件事：

1. 打 tag 前的最後一次閘門，跑在 `git worktree add --detach <sha>` 開出來的
   worktree 上。detached 是「我沒辦法順手改一行」的機械保證。
   收據：52 條全綠，而且跑完 `git status --porcelain` 的追蹤檔改動是 0 個。
2. 給 `/home/ted-h/tmp-tests/gates-all.sh` 本身加一道護欄：開場與收尾各量一次
   `sha256(HEAD + git diff HEAD)`，不一樣就 exit 5、不印「全綠」、列出改到的檔。
   只看被追蹤的內容，所以閘門自己生的 `ci-audit/`、`stats.json` 不會誤報。
   三刀驗過（不動＝綠、跑到一半 append 一個被追蹤的檔＝exit 5、只生未追蹤產物＝綠）。

### 12.3　tag 移動與出貨收據

`v0.1.0-alpha.147` 從 `a642039` 移到 `1a47bca`。移動是安全的：那個 tag
從頭到尾沒有產生過任何 release，沒有人下載過任何東西。
tag message 是從舊 tag 抽出來存成檔案再餵回去的，不是手抄的。

| 項目 | 值 |
|---|---|
| tag → commit | `1a47bca` |
| CI run | 全部 job `success`，`Release` `success` |
| `draft` / `prerelease` | `false` / `true` |
| 資產 | 4 個，全部 `uploaded`，全部非空 |
| `AI-Sister-Setup.exe` | 277,941,511 bytes |
| `AI-Sister-Linux-X11-amd64.deb` | 65,487,158 bytes |
| `sister-desktop.exe` | 71,816,704 bytes |
| `sister.exe` | 11,161,600 bytes |
| body | 與本機重算的 `release-notes.sh` 輸出**前綴相符**；後面只多了 `**Full Changelog**` 那一段 |

順帶一個值得記的事實：**tag run 是 main run 的超集。** `ci.yml` 有四處
`if: startsWith(github.ref, 'refs/tags/v')`，所以 installer 安裝／重裝／移除、
alpha.110 相容基準、整套簽章 fixture 只在 tag 上跑。
main 全綠不能拿來預測 tag 會不會綠。

## 13. a148 的 R8／R8b 收貨（2026-09-22）

### 13.1　這一版在修什麼

a147 修的是設定頁。同一個 bug 族在另外四個地方還活著：

> `await invoke(...)` 成功了，接在後面的**重畫**丟了例外，而那次重畫寫在接
> 「送出去失敗」的那個 `catch` 的射程內。於是畫面拿重畫的錯誤，組出一句
> 「這件事沒發生」。

四個地方：`apps/desktop/ui/onboarding.js` 的 `set`（同意書勾勾）、
`apps/desktop/ui/app.js` 的 `markLine`（標記）、`apps/desktop/ui/settings.js` 的
`openPlatformAccess` 與診斷匯出。

**同意書那個最危險，而且方向是反的。** 使用者勾下 `frame-storage` → 寫成功
→ 她從這一刻起真的在寫截圖 → 重畫丟例外 → 舊碼把勾勾翻回沒勾。畫面宣稱沒有
同意，而截圖正在被寫進磁碟。

### 13.2　a147 留下的病：只有夾具做得到的驗收條件

a147 我自己寫的驗收條件要求「證明重畫又跑完了」。那三支 painter 是**同步的、
決定性地丟**，所以那個條件只有「讓夾具第二次不要丟」才做得到——於是產品多了
一段永遠不會成功的補畫，測試多了五句聽起來像承諾的話。

a148 的派工單因此寫死兩條：**夾具每次命中都丟，不准 `once`**；**斷言只准打在
副作用上**（勾勾的值、按鈕能不能再按、`consent_read` 被叫了幾次），不准打在那
支正在丟的 painter 自己會寫出來的字。

### 13.3　數字，我自己重數過

| 閘門 | a147 | R8 | R8b |
|---|---|---|---|
| `scripts/check-consent-sticks.mjs` | 20 | 27 | 28 |
| `scripts/check-pet-says-why.mjs` | 655 | 662 | 663 |
| `scripts/check-settings-say.mjs` | 495 | 508 | 509 |

**+27 條，刪 0 條。** 「刪 0 條」是用加法證的，不是用眼睛看的：三支各自的
**新增**是 +7／+7／+13，而三支各自的**淨變化**也是 +7／+7／+13，所以沒有任何
一條舊斷言被換掉。（本來想用 `comm -13` 對兩棵樹的斷言名單，它警告排序不一致
——`LC_ALL` 的老問題——所以改用加法。）

**數 ✔ 的時候有一個坑。** 斷言行印的是 `✔`，跑完最後那行摘要印的是 `✓`。
拿 `(✓|✔)` 去數，每一支閘門都會多算**剛好 1 條**。我第一次的數字全部 +1，
改成只數 `✔` 之後和交回來的數字一模一樣。

### 13.4　27 條各自有沒有牙齒

- **19 條**被某一刀弄紅過（交回來的八刀 + 我自己切的那幾刀）。
- **6 條**是前提（「還沒按之前是什麼樣子」那一類）。前提本來就不該被刀弄紅，
  它們的作用是讓**別的**斷言的紅有意義。
- **2 條**到收貨為止沒有任何一刀證明過，明文記在這裡，不假裝它們有牙齒：
  同意書的「重畫例外解除後再讀一次，勾勾和檔案一致，而且說會留截圖」、
  設定的「診斷失敗之後按鈕可以再按」。

### 13.5　我自己補的刀

工具是 `/home/ted-h/tmp-tests/review-20260922/runcut.sh`（包 a147 的
`/home/ted-h/tmp-tests/mutate.sh`）：每刀先跑一次沒切的對照組、要求它綠而且
**至少跑到 1 條**，切完印出**是哪幾條具名斷言**變紅，一條都沒紅就大聲說。

| 刀 | 意圖 | 閘門 | want | 結果 |
|---|---|---|---|---|
| C8-settings | 把「放開按鈕」搬到 painter 後面 | settings | 紅 | 4 條紅 |
| C8-app | 同上 | pet | 紅 | 3 條紅 |
| Cneg-settings | 把真正的失敗也吞掉 | settings | 紅 | 2 條紅 |
| Cneg-app | 同上 | pet | 紅 | 1 條紅 |
| C7-onboarding | 把 a147 那段死補畫加回來 | consent | **綠** | 綠 |
| C7-settings | 同上 | settings | **綠** | 綠 |
| C7-app | 同上 | pet | **綠** | 綠 |
| Cflag-onboarding | `if (failed)` → `if (writeError)` | consent | 紅 | 1 條紅 |
| Cflag-settings | 同上 | settings | 紅 | 1 條紅 |
| Cflag-app | 同上 | pet | 紅 | 1 條紅 |

C7 那三刀**要的就是綠**：它們證明「沒有任何一條斷言在要求那段死碼存在」，也就
是 a147 的病沒有被複製過來。要讓「綠」算得出證據，刀本身得先證明它真的切到了
——補了 10 行、4 個標記字串，數過。

### 13.6　送回去的兩件事，都收了（R8b）

1. **`failed` 旗標沒有人守。** 三支用的是獨立的 `failed` 布林，理由是丟出來的
   值可能是假值（`null`），不能拿值的真假決定走哪一臂。交回來的時候自己註明
   「這一刀沒跑」。我跑了：**三支全綠**——那個理由當時是沒有人守的。R8b 補了
   一個「`invoke` 拒絕的值是 `null`」的夾具和三條打在副作用上的斷言，我的刀現
   在紅在那三條上。
2. **`} else try {`** 寫法（`catch` 在 23 行之後、同一縮排，`apps/desktop/ui/`
   底下零前例）→ 補回大括號。`git diff -w -U0` 證明只有 `} else {`、`try {`、
   `}` 三種增刪行，裡面的字一個都沒動。

交回來的還有三件**自己講出來的負面結果**，處理都是對的：兩行它量過沒牙齒所以
不交的 `return`；`[data-diagnose-say]` 從來不會被加上 `bad`，所以它**刻意不寫**
那條看起來很滿的「沒有 `bad`」斷言；以及上面第 1 點那把它沒跑的刀。

### 13.7　跑刀的機械教訓（會再犯的那種）

- **兩輪突變共用同一份 log 路徑。** `/home/ted-h/tmp-tests/mutate.sh` 把備份目錄
  和 `control/cut/restored.log` 寫死，我在背景那批還在跑的時候又在前景切了一刀，
  兩邊互相蓋。指紋是「對照組綠 601 條 → 切了紅 664 條」這種**兩邊條數對不上**，
  以及 consent 的對照組 log 末行是 pet 閘門的句子。修法是在那支 runcut 包裝腳本裡加
  `flock -n`（拿不到就 exit 9 並說明），然後兩刀重跑一次，每刀都比對 log 的出處。
- **`pgrep -f '[g]ates-all.sh'` 回 2。** `[g]` 這個老招只保護**pattern 那個 token**，
  而我同一個 Bash 呼叫裡還有一行 `cp …/gates-all.sh …`——`bash -c` 的命令列是
  **整段腳本**。
- **`pkill -f 'sleep 20'` 殺掉我自己的 shell**（exit 144），還留下一個沒還原的
  突變，而且連備份檔都被污染（後一輪備份的是已經被改過的檔）。救回來是從
  **已經 commit 的那顆**還原，不是從那份不可信的備份。
  結論：清理要問**檔案**（`fuser` / `lsof`）再按 PID 殺，不要用 `-f` 比字串。

### 13.8　交出去的三顆

在 `1a47bca` 上開的 worktree 裡分成三顆，每顆都自己綠過：

| commit | 說的是 | consent / pet / settings |
|---|---|---|
| `fix: four more places that called a finished action a failure` | 產品 | 27 / 662 / 508 |
| `test: a thrown null still has to take the failure arm` | R8b 的夾具 | 28 / 663 / 509 |
| `refactor: an else that opens a try should open a brace too` | 排版 | 28 / 663 / 509 |

最後那棵樹的 hash 和交回來的那棵**逐位元組相同**。

### 13.9　留給 a149

- 把整族收斂成 `cancelBrainCli` 的形狀（在 `try/catch` 裡只決定狀態，外面畫一
  次），當成獨立的 refactor 做。`settings.js::save` 要**刻意跳過**並留下理由。
- §13.4 那 2 條沒牙齒的斷言，補刀或改寫。
- PF-3：`nodes[i]` / `elements[i]` 的平行陣列不變式跨過 `#[cfg(windows)]`，
  目前只有 `debug_assert_eq!` 守著。
- `usage_status.rs` 的接線仍是 0 條測試。

## 14. alpha.148 出貨收據（2026-09-22 早上）

這一次的順序，和 §12.2 寫的那條紀律逐字對上：

1. 六顆 commit 在 worktree 裡疊好（fix / test / refactor / docs ×2 / release）。
2. 版號 bump 跑 `scripts/check-release-version.py`，19 個位置。四個手改，
   **兩個 lock 讓 cargo 自己重寫**（`cargo metadata --offline`），
   然後 `git diff -U0` 確認 lock 裡除了版號行沒有別的增刪。
3. `docs/RELEASE-NOTES.md` 插進 `## v0.1.0-alpha.148`，寫出去不帶 BOM，
   `scripts/release-notes.sh v0.1.0-alpha.148` 的輸出存起來當之後比對的底本。
4. `git worktree add --detach <sha>` → 跑完整閘門。收據：
   **量的是 …/wt-a148-verify @ `0f7ceeb`（樹 `7d3c506160969f27`），通過 52 條，失敗 0 條，全綠。**
5. fast-forward main、push、等 main 的 CI 六個 job 全綠。
6. 打 tag（message 從檔案 `-F` 餵進去，不是手抄的）、push tag。

### 14.1　中途被擋下來一次，擋得對

第 4 步之前跑過一次，對象是 `d421dae`。跑到一半我發現 release notes 的標題是
假的（見 §14.2），於是**把那一輪的 log 改名成 `.aborted.log` 並殺掉它**——
它量的是一顆即將被改掉的 commit，留著只會誘惑我引用。
改完之後從頭跑一次，對象是 `0f7ceeb`。

殺的方法也記一下：`fuser <log>` 拿到 PID 再點名殺，**不用 `pkill -f`**——
那個會連發出指令的 shell 一起殺掉，我前一晚才踩過。

### 14.2　差一點出貨的那句假話

a148 的說明草稿，標題原本是：

> 同一種謊，設定頁以外還有三處：同意書、答案底下那顆「我本來已經忘了」、匯出診斷。

**匯出診斷就在設定頁上。** `apps/desktop/ui/settings.html` 裡有
`[data-diagnose-say]`，也有 `data-platform-access-section`。四個修補點裡有兩個
在設定頁上，不是「設定頁以外」。

而且往回看一版，alpha.147 出貨的說明寫的是「設定頁上**其餘每一個**會寫東西的
地方」——那句話當時就是假的，匯出診斷在同一頁上而且真的寫檔。

病因是我用**檔案**在想頁面：`settings.js` 我當成「設定頁的程式」，
`onboarding.js` / `app.js` 當成別頁的。但 handler 在哪個 `.js` 裡，跟那顆鈕畫在
哪一頁上，是兩件事——要看哪個 `.html` `<script src>` 了它。

改法是把標題改成照實數：兩個在設定頁以外，兩個就在設定頁上、只是上一版點名的
清單沒有列到；並且在 a148 的說明裡**明講 a147 那句話不完全對**。

順帶一個機械教訓：引用上一版出貨過的句子要逐字驗，而 `grep -c` 驗不了——
那些檔是折行的，片語跨行時單行 grep 回 0 或 1 都在騙人。
要 `s.replace("\n","")` 之後再 `count()`，而且對 `docs/RELEASE-NOTES.md` 和
**真的送出去的那份 body** 各比一次。這一句兩邊都是 1 次，逐字相符。

### 14.3　收據

| 項目 | 值 |
|---|---|
| tag → commit | `0f7ceeb` |
| 那顆 commit 的樹 | `2ea7bca28a9adec0565228fc5e70400d01c75a12`，和被閘門量過的那棵相同 |
| main run | 六個 job 全 `success`（Release／Website 在 main 上 `skipped`，它們是 tag-gated） |
| tag run | 八個 job 全 `success`，含 `Release` 與 `Website` |
| `draft` / `prerelease` | `false` / `true` |
| 資產 | 4 個，全部 `uploaded`、全部非空 |
| `AI-Sister-Setup.exe` | 277,941,356 bytes |
| `AI-Sister-Linux-X11-amd64.deb` | 65,487,666 bytes |
| `sister-desktop.exe` | 71,817,216 bytes |
| `sister.exe` | 11,161,600 bytes |
| body | 與本機重算的輸出**前綴相符**，後面只多了 101 字的 `**Full Changelog**` |
| 發布時間 | 2026-09-22T07:57:39Z |

驗的腳本留在 `/home/ted-h/tmp-tests/review-20260922/verify-release-a148.sh`：
對 gh 一律測 `= "success"`（它把還沒跑到的步驟回成 `""` 不是 `null`），
body 比前綴不比相等。


## 15. alpha.149 出貨收據（2026-09-22 晚上）

四件事一起出：第一次開啟先選角色、同意書改成按鈕、朗讀變開關、
答案泡泡裡的 guardrail 句搬進「為什麼」。閘門 663 → 747。

### 15.1　我審出來、delegate 沒自己發現的

1. **一句出貨的畫面文字變成假的。** `apps/desktop/ui/settings.html` 第 86 行原本寫
   「只有你在當張按下才播放」。那句話在 a149 之前是真的，是 a149 讓它變假的。
   它就在 delegate 這一輪加 checkbox 的**同一個檔案裡，差 77 行**。

2. **角色清單讀不到會把整個產品鎖死。** catalog 不合契約時畫面印
   「請重新開啟 AI-Sister」而 `firstPersona.hidden` 留在 `false`，
   於是 `ask()` 第一行 `if (!firstPersona.hidden) return;` 無聲吞掉每一個字，
   同意書永遠不出現，重開會再失敗一次。
   `!== 17` 這個寫法是 repo 的慣例，另外三處都是優雅降級（`return empty` /
   `Object.freeze([])`）——**慣例會被整段複製，慣例的失敗成本不會**。
   修法是跳過選角直接進同意書，等於退回這個功能存在之前的行為。

3. **三道「回來的東西要和我送出去的一致」的守衛，747 條斷言一條都沒守到。**
   既有的失敗夾具清一色是「invoke 直接 reject」，沒有一個是
   「resolve 了但內容不對」——而後者才是這個 repo 最怕的那種謊。

4. **收貨之後我自己拿舊識別字重掃，又撈到兩處。** delegate 改了 9 處過期句子
   （其中第 9 處是它自己多找到的），仍漏了 `AGENTS.md` 裡同一句全稱句
   和 `app.js` 一段引用舊按鈕名的註解。
   **`AGENTS.md` 特別要看：未來的 agent 會把它當權威讀。**

### 15.2　我自己的量測出過兩次錯

- 我把 shell function 取名叫 `cut`，蓋掉 `/usr/bin/cut`。突變腳本的
  ✔／✗ **數字全對，紅掉的斷言名字整段消失**。指紋是「摘要對得上，明細空的」。
- `cmd | tail -5; echo "exit=$?"` 報的是 `tail` 的結局，
  於是「fmt exit=0」是假的，`cargo fmt` 的真正結局從來沒被讀到。
  同一輪 `cargo test -p sister-core consent_read_aloud` 配到 0 條測試、exit 0，
  讀起來和通過一模一樣——解析 `N passed; M failed` 並要求 N+M ≥ 1 才看得出來。

### 15.3　一條偶發紅，不是這次改動造成的

`check-erased-db.sh` 在 52 條裡紅過 1 次，訊息是
「正在錄的那一場被算成了一次當機」。同一棵樹重跑 2 次都綠、base 對照組綠、
打 tag 前的 detached 全閘門也綠（4 次紅 1 次）。改動全在 UI／config／docs，
碰不到 recorder 生命週期。**成因沒有證明**，只證明了它不是決定性的、也不是這次造成的。
CI 比開發機慢，窗口只會更寬。

### 15.4　收據

| 項目 | 值 |
|---|---|
| tag → commit | `9aa0a2d` |
| 那顆 commit 的樹 | `b3941997bdbc6eb0cd2a52af5f98025e1e7cc25d` |
| 打 tag 前的閘門 | detached worktree @ `9aa0a2d`，**52 條全綠**，樹指紋 `63661f0b6444763c` 跑完沒變 |
| main run 35776069967 | 六個 job 全 `success`（`Release`／`Website` `skipped`，它們是 tag-gated） |
| tag run 35776072748 | **八個 job 全 `success`**，含 `Release` 與 `Website` |
| `draft` / `prerelease` | `false` / `true` |
| 資產 | 4 個，全部 `uploaded`、全部非空 |
| `AI-Sister-Setup.exe` | 277,940,420 bytes |
| `AI-Sister-Linux-X11-amd64.deb` | 65,491,970 bytes |
| `sister-desktop.exe` | 71,833,088 bytes |
| `sister.exe` | 11,162,624 bytes |
| body | 與本機重算的輸出**前綴相符**，後面只多了 101 字的 `**Full Changelog**` |
| 發布時間 | 2026-09-22T20:38:57Z |

驗的腳本留在 `/home/ted-h/tmp-tests/review-20260922/verify-release-a149.sh`。
它第一次跑是紅的，**紅在它自己身上**——我用 `sed` 從 a148 那份改出來，
版號換了而裡面寫死的那顆 commit sha 沒換。
改的是「我量過的那顆 sha」，不是判斷邏輯。


## 16. alpha.151 出貨收據（2026-09-23 清晨）

一件事出貨：**先開口**。按下 Enter 的那一瞬間先把她本機找得到的端上來，旁邊如實
寫著「這些是我自己記得的」，CLI 回來之後底下那一趟再整份重畫。五輪派工
（R1–R5，codex `gpt-6-astra`），14 個 commit。`check-pet-says-why` 的斷言
**767 → 883**，清單引號閘門 **55 → 60**，53 條閘門在 `8a24912` 的 detached 樹上
全綠（樹指紋 `c97e4b63203d6c7c`）。

### 16.1　我審出來、delegate 沒自己發現的

1. **它為了「先開口不寫資料庫」開的唯讀連線，會把整類問題打死。**
   `answer_from_memory` → `chapters_for_question` → `replace_segments` 是**寫**。
   所以只要問句認得出時間範圍（「昨天下午在幹嘛」），先開口那一趟就拿到
   `SQLITE_READONLY` → 回 `Err` → 被畫面的 try 吞掉：**先開口整個不會發生，
   而且沒有任何症狀**，而挑中的正好是章節最有用的那一類問題。
   它的收據會是綠的，因為 JS 閘門跑的是假 DOM ＋ 假 IPC、Rust 那條路一行都沒執行，
   桌面那棵樹在這台機器上缺 `dbus-1` 編不起來。

   **正確的判準不是「寫不寫」，是「第二次做會不會不一樣」。**
   `replace_segments` 是冪等的快取重算；`close_from_message`／`record_followup`／
   `log_query` 第二次做就是另一個結果——該閘的只有那三個。
   順帶：它後來自己補的 `open_read_only` **沒有設 `busy_timeout`**，而 `Db::init`
   對其他每一條連線都設了 5000。recorder 一直在寫，WAL checkpoint 期間讀會**立刻**
   `SQLITE_BUSY`，先開口就變成時有時無。同一顆資料庫上兩種連線兩種慣例。

2. **兩道守衛守的是自己的夾具，不是產品。** 兩刀都全綠：
   - 把 `if (spokeEarly) spokeAtMs = …` 的條件拿掉（無條件記時）→ ✔813 ✗0。
     delegate 的刀切的是**反方向**（永遠送 `null`），所以這道守衛只守得住「少報」，
     守不住「多報」——**而多報才是會說謊的那一邊**：全停擋下呈現時她一個字都沒畫，
     報告卻會印「先開口中位數毫秒 360」，而他讀那份報告正是為了判斷這功能有沒有在動。
   - 名為「先開口這段不寫資料庫」的測試，整支都在一條 `SQLITE_OPEN_READ_ONLY` 的
     連線上問 `total_changes()`。**那條連線本來就寫不動**，所以它證明的是
     「連線是唯讀的」。在被測函式裡插一句吞錯誤的偷寫 → `2 passed; 0 failed`。
     修法是拿**寫得動**的連線再跑一次同一支函式，順帶把「兩條連線算出同一個答案」
     也釘住；切同一刀就紅在 `left: 45, right: 44`。

3. **她答應「我再認真想一下」，然後想的那一趟死掉了，承諾永遠留在畫面上。**
   五個情境沒有一個涵蓋「先開口只有一句承諾、而 `ask` 失敗」。我自己接探針跑，
   畫面是：計時器停了、不會再有第三趟、那句話原封不動留著，唯一的交代在另一條
   notice 行上。值得記的是**這個情境下她空手的原因不是盲點**（暫停、排除、還沒錄），
   是「我說要去做的那件事沒跑完」——所以那一串「為什麼我沒有」在這裡是**錯的**解釋。

4. **同一句 22 個字被逐字抄了兩份，而兩份各自都有人守。**
   我是被自己突變腳本的 `assert s.count(old) == 1` 撞到的（它說「錨點命中 2 次」，
   刀直接不切）。**那個 assert 本來只是防我切錯地方，結果替我數出了重複——
   它是免費的偵測器。** 分開改兩份各自只紅對應那一組，所以今天不是 correctness
   bug；但要改這句話就得改三個地方，而第三個站點加進來時沒有人會提醒。

5. **這一刀修好的東西，他的診斷報告看不見。** 自檢那一段的第 4 項量的是
   「從按下 Enter 到正式那份畫完」，門檻四秒，而 CLI 那趟還是 21–120 秒——出貨之後
   他跑一次診斷，那一項照樣是 ✗，他會合理地得出「什麼都沒變」，而畫面上其實第一秒
   就有東西了。這是 `AGENTS.md` §二 的形狀。修法是**多一行數字，判決仍然掛在整題**：
   把判決偷偷換成「她多快開口」，就是把 ✗ 變成 ✓ 而產品一個字沒有變。

6. **一個旗標的不變式，只有情境在守。** R5 已經自己打了九刀，這一條在它的鏡頭外。
   `showingProvisional` 要「為真當且僅當答案區裡只有那句承諾」。三個寫入端它清乾淨
   兩個，第三個——`showConsentCompletion`——是它**憑判斷順手加的**，沒有任何一刀
   對著。拿掉那一行：**880 綠 0 紅。**

   那條路需要「同意書不是被問題打開的」，而那條路上沒有一個 in-flight 的 `ask`
   可以讓它失敗——**情境測不到**。所以改成數**寫入端**：掃出所有呼叫
   `hitList.replaceChildren(` 的函式，要求每一支都在同一支裡清掉旗標，再要求設真的
   只有那支 helper，外加一條「真的抽到至少三個」的前提斷言（免得掃描器空手也叫綠）。
   四刀分別紅到這三條，其中一刀同時紅了另外四條——那個洞於是有兩個獨立偵測器。

### 16.2　這一輪反過來了：delegate 抓到我九個錯

前幾輪的收據都在記「我審出 delegate 沒發現的」。這一輪**倒過來的那一欄更長**，
而且幾乎都是派工單本身的缺陷，不是它實作偏掉：

- **R3 抓到三個。**（a）我寫「改這三處就能保證只有一句」——實際有 9 處會多畫，
  它實測抓到多出來的一列「🔊 用本機聲音朗讀」。（b）我給的 `blind` 範例帶
  `scan_horizon_days: 30`，同時要求終局說「我記得的東西裡沒有這件事」——**那兩件事
  不可能同時成立**（有 horizon 的時候產品說的是「我翻過的那幾段裡沒有」）。
  （c）**IPC 裡那個欄位真正的名字是 `answers` 不是 `facts`**，照我寫的夾具做
  會什麼都沒測到。
- **R4 抓到兩個。**（a）我要求「同一把刀要讓兩個情境都紅」，而計時那一行守在
  consent 分支**外層**、根本走不進去——它沒有搬動產品去迎合我的預期，而是另打一刀
  單獨把它打紅並說出原因。（b）我記的「`segment_events` 只讀 `focus_events`」是錯的，
  實際是**五張表**。
- **R5 抓到四個。**（a）我的刀 B 對它新抽的 helper 是字面上的 no-op；（b）我指定的
  第 4 條斷言照寫會 vacuously pass；（c）我把「內容」和「可見性」混成同一件事；
  （d）我給的六刀沒有一把對著「秒數要停」那條斷言，它自己補了第七刀。

**判準**：交回來的程式碼每一輪都要重驗，這個沒變；但派工單裡我自己的**事實宣稱**
（欄位叫什麼、那一行守在哪一層、哪幾張表）現在是缺陷密度最高的地方——
它比程式碼難驗，因為**沒有任何閘門在讀派工單**。

### 16.3　我自己的量測出過四次錯

- **我把派工單裡一句自己編的量化宣稱，看著它被照抄進產品。**「本機記憶是毫秒級的」
  ——我量不到；已有的 `search_latency` 量的是 `db.search_during` 那**一步**，而這條路上
  還有章節重算、判讀撈卡、`blind` 那幾個 COUNT，整段沒有人量過。「一趟 20–120 秒的
  CLI」把兩趟混成一趟：擋住本機檢索的是**規劃**那一趟（21.2–30.7 秒實測），
  成句那一趟本來就排在本機檢索後面。
- **我在 delegate 還在跑的工作樹上量了兩次，兩次都讀到十分鐘後就不存在的狀態。**
  第一次 `git diff` 讀到一個中途的形狀，我據此推論出一個當下已經被修掉的洞；第二次
  `cp` 出來跑對照組，量到「新測試＋舊產品」的 torn 組合、5 條紅，差一步就要報
  「delegate 把既有測試弄壞了」。**量測前後各 `sha256sum` 一次就分得出來**，
  那份 log 已經改名成 `.aborted`。
- **我寫來查「這個寫入在哪支函式裡」的對照工具自己說謊。** 它的 `^(?:pub )?fn`
  只配得上沒有縮排的 `fn`，於是 `#[cfg(test)]` 裡的測試函式全部繼承了外層那個名字。
  它報「`log_query` 在 `fn memory_overview_answer` 裡」，實際上在一條名叫
  「總覽答案不准進檢索題庫」的**測試**裡。
- **我對一個「0 紅」的結論方向反了。** 我假設 `dataset.azureAnswerBody` 是排除標記，
  拿掉它她就會把那句承諾念出來（一條新的對外連線）。去讀**讀它的那一支**才發現
  它是**允許清單**，而那句話在任何朗讀按鈕存在之前就已經離開 DOM 了。
  0 紅是對的答案，不是缺口。**「0 紅」要分成「沒人守」和「哪個方向都不動」兩種，
  分辨法是去讀讀它的那一支。**

### 16.4　出貨前那幾件事

- 說明草稿第一版描述了一個**從來沒出貨過的「以前」**（那句承諾被擱在畫面上、
  一整面理由牆）。兩個都是這一版自己造出來、又自己修掉的。兩秒的偵測法：
  `git show v0.1.0-alpha.150:apps/desktop/ui/app.js | grep -c spokeEarly` → **0**。
- 草稿裡兩句「產品會說」的引號是我編的（一句 0 命中，另一句只存在於註解裡），
  換成白話敘述——那正是引號閘門自己的 docstring 要求的做法。
- 我把診斷報告的段落寫成「第 ④ 項」，而 ④ 是「這份報告帶走了什麼」；
  要講的是自檢那一段（③）的第 4 項。
- 清單裡有兩句引號落在**沒有 SAYS 動詞的那一行**，閘門會直接跳過。重新折行讓動詞
  和引號同一行，再模擬一次 → 5/5 真的被守著。

### 16.5　順帶量到的：那條牆鐘間歇斷言的真實頻率

出貨之後我把 `check-pet-says-why.mjs` 逐個 commit 跑了一遍（a150 → a151 共 14 個），
斷言數的軌跡是 767 → 784（R1）→ 813（R2）→ 823 → 853（R3）→ **852** → 866（R4）
→ 880（R5）→ 883。

中間那個 **852 是一條紅**，而那個 commit 只動了 audit 閘門的 gitignore ——
典型的「commit 內容不可能是成因」。它就是 §三 第 9 條那個取樣競態，
**14 輪裡中 1 次**；`cf47a79` 之後的 4 輪都是 866／880／883 沒有掉過。
這一族要記的是：它**平常不會紅**，所以「這批刀從來沒紅錯過」不是證據。

## 17. alpha.152 的出貨收據，以及「翻面紅了」不等於「有人守著」

alpha.152 是兩輪 delegate（codex `gpt-6-astra`）加我自己一個 commit：

| commit | 是什麼 |
|---|---|
| `8fa1a8e` | 我：Esc 收起之後兩條分支不對稱是故意的，而沒有人寫下來 |
| `759aa12` | R1：L2 卡片的 `activity` 建 trigram 索引（migration 021、`search_readings`） |
| `b3e723f` | R2：把內容比對接上答題，打中的卡片直接端出原句 |
| `fd187ee` | 我：那一行今天是 no-op，而翻面紅了讓它看起來有人守 |
| `4aba22f` | release |

閘門：**53/53 全綠**，detached worktree `@ 4aba22f`。
`cargo test --workspace` **2205 passed / 0 failed**；測試名單 2198 → 2208，
**刪除或改名 0 條**。`check-pet-says-why` 886 → 903。

### 17.1 這一輪最值得記的一件事：翻面紅了不等於那一行有人守

R2 的收據把 `match_answer_readings` 去重時的 `existing.matched = true;`
記成「有人守」，證據是把它**翻面**成 `= false` 紅了一條測試。

我把**整行刪掉**（不是翻面）：**10 綠 0 紅**。

原因是那一行冪等：`selected` 在那個迴圈裡只裝得下
`from_card(row, ReadingOrigin::Content)` 產生的卡，而 `matched` 建構當下就是
`true`。翻面之所以會紅，是因為 `true → false` 是**產品自己走不到的方向**。
真正守住那個不變式的是 `from_card` 裡的 `matches!(origin, …)`。

於是有兩句話同時在說謊，而且都通過了：
1. 測試名 `duplicate_time_and_content_card_keeps_match`——它守的是建構端，不是去重那一行。
2. 收據第 3 節的那一刀——紅了，但紅的理由和那一行的用途無關。

**判準一句話：任何一刀紅了之後，再問一次「把它整段刪掉，會不會也紅？」**
只有翻面紅、刪掉不紅 = 那是一行冪等的防禦，不是守門的人。
成本是一次額外執行，比事後相信一份假收據便宜太多。
（到不了的防禦分支**該留**，但不准替它宣稱成因——所以我留著那一行，
在它上面寫清楚它今天是 no-op、以及什麼情況會讓它真的開始做事。）

### 17.2 delegate 抓到我派工單裡五條錯，其中一條改寫了整輪的故事

上一版（alpha.151）的收據寫著「派工單裡我自己的事實宣稱現在是缺陷密度最高的
地方——因為沒有任何閘門在讀派工單」。**這一輪又中了一次，而且更貴。**

1. 我寫「五個 JS 判準」，實際是四個 JS 加一個 Rust。我自己的 §3.2 就列對了，
   §5 抄錯——**同一份文件前後不一致**。
2. **我寫「今天 `readings` 非空 ⇒ hits／facts／章節必有一個非空」，這是假的。**
   `chapters_for_question_read_only` 對認得的時間問句會回 `Some((range, group))`，
   而 `group` 可以是**空的**；`main` 只看 `Some(range, _)` 就去 `readings_spanning`，
   renderer 的 `hasChapters` 讀的卻是 `chapters.length > 0`。兩邊不是同一件事。
3. §4.0 我要求「逐位元組一樣」，而 `prepare` 每次產生新的安全 nonce，做不到。
   它實跑失敗一次（7 passed; 1 failed）才回報，沒有假裝做到。
4. §4.2 的配額我沒寫清楚：三條查詢各四張是十二張，和「總共八張」不能同時成立。
5. **我說「只有五處會過期」，漏了題庫命中。** `QueryLogEntry::hits` 數的是
   「使用者看到了什麼」，而舊的 `facts.len() + hits.len()` 會把一次成功的卡片
   快答記成 0。這是「列舉句只會短不會錯」的又一個實例：
   列舉句過期的時候只會**少**一項，不會錯一項，所以要反過來搜**新加的那一項**
   的每一個讀取端——而這一輪說短的是我。

### 17.3 第 2 條讓我去量了一次，結果是：那個 bug 在 alpha.151 就已經出貨了

我原本的故事是「第二刀會**造成**她握著卡片卻說不知道」。第 2 條說那不是新的。
我在 **`v0.1.0-alpha.151` 那個 tag 的樹上**建了探針：一張 L2 卡落在問句解析出
的時間範圍裡，**完全不插 `focus_events`**。結果逐字：

```
PROBE chapters = Some(0)
PROBE readings = 1
PROBE 結論：asked_chapters.is_some()=true / hasChapters=false / readings=1
```

所以 alpha.151 上，`foundNothing` 為真而 `readings` 非空是**真的做得到的**，
她會說「我好像不太知道你在說什麼耶」然後升起那面牆。

**成因我查到一半就停了，而且中途差點寫錯一句。** 我先查到 `l2_card` 對
`segment` **沒有外鍵**（只有 `supersedes` 自我參照），又 grep 了 `retention.rs`
的非測試碼發現裡面一個 `l2` 都沒有，於是差點下結論「prune 不會帶走卡片」。
**那是假的**：prune 走的是 `crate::db::Db::tombstone_descendants`
（`retention.rs` 第 684 行，forget 是第 953 行），照血緣圖把卡片立墓碑**並清掉字**。
我的 grep 看不到它，因為那一行裡一個 `l2` 都沒有。

所以「卡片活下來而原始事件死掉」要怎麼發生，我**沒有**追到底。
**我也沒有把它重現到「使用者做了哪個動作」那一層**，而那一層才是能不能寫進
版本說明的判準——所以版本說明裡我只講「這一版修掉的是問題打中卡片那一種」，
並且明講另一種還在。

（這一格的形狀值得單獨記：**我用機制的識別字去 grep 一個不含那個識別字的呼叫端。**
用 grep 證明「這裡沒有 X」要兩根獨立的針都落空才站得住，而我只用了一根。）

停下來是刻意的：再追下去就是「不要叫 agent 做考古」的自我版本，
把審查預算花在來源考古上，而不是花在決定這一輪能不能出貨的那幾把刀上。

### 17.4 886 → 903 一開始對不起來，而追下去是對的

新增斷言源碼 `+17 行 −0 行`，但收據說 18 條。兩邊都對：其中一行是
`` `A152 反向 ${state}：…` `` 在兩個 state 上各跑一次 → 執行期 18 條。
那 `886 + 18 = 904` 又和實測的 903 差一條。

少掉的是 `A151 R2 抽取：#[cfg(test)]`——那條**每遇到一個 `#[cfg(test)]` 就印一條**，
而 `main.rs` 的 `#[cfg(test)]` 從 14 個變成 13 個：有一個 test module 被搬進了新的
`answer_readings.rs`。**886 − 1 + 18 = 903。** 對得起來。

順手做了一次全面體檢：12 支 node 閘門的 ✔ 數字 base vs R2，
**只有 `check-pet-says-why` 動了（+17），其餘 11 支一條不差**——
所以那次搬家沒有讓任何閘門變瞎。（`BrainAnswer` 搬出 `main.rs` 之後
`check-cli-status` 仍是 89/89，因為它只拿 `nativeSource` 找 `cli_status_read`。）

### 17.5 我自己先寫錯、然後複查掉的兩條

**(a) 「卡片原句沒有上限」我先記成缺陷。** prompt 那條路有
`bound_source` 壓到 4 KiB，新的 DTO 沒有——看起來是「房子有規矩而新路沒跟」。
**去查既有的顯示路之後反過來**：記憶總覽早就在做同一件事
（`MemoryOverviewCard.activity` 是裸 `String`，`renderOverview` 直接
`textContent`），**一樣沒有上限**。4 KiB 那條規矩的理由是 prompt 圍欄的
24 KiB 預算，那個理由不會自動轉移到畫面上。所以 R2 沒有造出新的不一致，
它跟既有先例一致；這一輪不修，修要連 `renderOverview` 一起修。

**(b) 我差點引用一支看不到這件事的閘門。** 審查計畫裡我寫
「卡片原文跑進診斷包的話 `check-diagnose-carries-no-text.py` 應該會抓」。
去讀了：**它只掃 `crates/sister-core/src/diagnose.rs` 的 `Note` 型別欄位**，
`main.rs` 的 DTO 完全不在視野裡。結論反而更強（那條線以上型別上放不下
`String`），但「註解說有閘門就去把它找出來再信」這條我差點又犯。

### 17.6 我在看 diff 之前先寫了九面鏡頭，其中三面在動刀之前就自己結案了

`/home/ted-h/tmp-tests/review-20260923/REVIEW-A152-R2.md`（本機審查計畫，不在 repo 裡）
是在收貨**之前**寫的（否則鏡頭會長得跟它的刀一樣）。
九面裡有三面根本不用切：

- **唯讀連線遇到舊 schema**：`Db::open_read_only` 自己 `ensure!` 版本相符，
  外層 `ask_local` 的錯誤又被 `try/catch` 吞掉。兩層 fail-closed，到不了。
- **挑選／排序／截斷的順序**：讀 `match_answer_readings` 就看得出截斷全在
  `sort_by_key` **之前**，那個 bug 結構上做不出來。
- **新模組沒接上產品**：`#[path]` 掛的是**原檔**，而該模組零個 `crate::` 路徑，
  兩個 crate 編的是同一份語意。這一面我本來列為最大風險，結論是它做對了。

真的動刀的四面裡，只有一面找到東西（就是 17.1）。
另外三面：`from_card` 的來源身分**紅 2 條**（有人守）；
DOM 快照**紅 1 條而且是對的那一條**（`hits-empty` 多一個 `title` 屬性，
只有快照看得到；`thinking` 那一態畫的是承諾句不是 `hits-empty`，本來就不該紅）。

## 18. alpha.153 的出貨收據，以及「閘門的數字沒變」就是它沒看我這一筆

alpha.153 是兩輪 delegate（codex `gpt-6-astra`）加我兩個審查 commit：

| commit | 是什麼 |
|---|---|
| `c39df33` | R1：時段只剩卡片時她把卡片講出來；先開口那一趟不再替自己解釋沒有題號 |
| `fd8bcaf` | R2：delegate 交回的成品（收貨第一個動作是 commit，不是動刀） |
| `5cf460d` | 我：被刪掉的那段理由是在反對這次改動；收合在假瀏覽器外沒有牙齒 |
| `3f07acd` | merge |
| `fc39c52` | release |

閘門 **53/53**，detached worktree `@ fc39c52`。
`check-pet-says-why` **922 → 940**，刪除或改名 **0 條**（`comm -23` 是空的）。

### 18.1 上一版的 CI 是 flake，不是 bug——這件事只記在本機，補進來

alpha.152 打完 tag 之後 Windows job 紅在 step 8（UIA）：
`assertion failed: changed.contains("CHANGED-SENTINEL 02-2233-4455")`，
`windows_uia.rs:220`。

判定是 flake 的依據有兩條，都不是「看起來像」：
1. `git diff v0.1.0-alpha.151..v0.1.0-alpha.152 -- crates/sister-capture` 是**空的**。
2. 同一天更早一次 main 的執行，紅的是**另一條**計時測試
   （`ops::watch::tests::an_admitted_watch_round_drains_before_stop_and_does_not_publish_after_pending`）。

`gh run rerun --failed` → 8/8 綠，release 正常發出。
**Windows CI 現在有兩種各自獨立的計時 flake**（UIA 寫後讀、`watch` 排空），
兩種都還沒有人去修。下次紅在這兩條上，先比對上面那兩個判準再決定要不要查。

### 18.2 這一輪最值得記的：閘門印的數字沒變，就是它沒看我這一筆

我在 `docs/WINDOWS-CHECKLIST.md` 寫完 alpha.153 那一節，跑
`scripts/check-checklist-quotes-exist.py`，它印：

```
✓ 清單裡那 62 句「產品會說 X」，X 在原始碼裡都找得到
```

**62 是我寫之前的數字。** 我那一節裡有十幾組「」，一句都沒被檢查。

兩個原因疊在一起，兩個都在它自己的註解裡寫著：

1. **它是逐行配對動詞和引號的**（`wanted = QUOTE.findall(ln) if SAYS.search(ln)`）。
   我把「她要說」放在行尾、引號放在下一行的縮排續行上，於是兩行各自被跳過：
   上一行有動詞沒引號，下一行有引號沒動詞。
2. **引號裡不可以再有引號。** `QUOTE = 「([^「」\n]+)」` 配不到巢狀的那一組，
   它會改配到裡面那個短的（「開始記錄」四個字），而四個字低於 `MIN = 8` 被跳掉。
   我要驗的那句產品原文裡正好有一對「開始記錄」。

改掉之後 **62 → 64**。那兩句現在真的被拿去和原始碼對。

**判準一句話：閘門印的那個數字，是免費的偵測器。**
加了東西之後它沒變，代表它沒看我這一筆——而那和「我寫對了」在畫面上一模一樣。
（這支腳本自己有一條 `if checked < 10: die` 的活體檢查，但它擋的是「整份都不掃了」，
擋不住「多加的那一段不掃」。活體下限只保護存量，不保護增量。）

### 18.3 delegate 刪掉的那段註解，是在反對這次改動

R2 的 diff 裡有這麼一段：

```diff
-/* 「我沒看到過」底下那幾句「但也可能是因為……」。比那句話再淡一階：它們是
-   註腳，不是答案；但也不能淡到看不見——這是整個畫面上唯一會讓他知道
-   「東西可能在，只是我不准看」的地方。 */
+/* 空手時的理由預設收起，按「為什麼？」才展開。 */
```

**我們做的正是那句話警告的事。** 而那句話沒有錯——收起來之後，那仍然是整個
畫面上唯一會讓他知道「東西可能在，只是我不准看」的地方，只是現在要按一下。

刪掉一段在反對自己的理由，是最糟的處理方式：下一個人看不出這個取捨被權衡過。
改成保留原本的論點、寫上使用者的原話為什麼推翻它、並且明說代價是什麼。
**代價寫在原地，下一個人才知道它被付過。**

（同族的既有紀律寫在 `apps/desktop/ui/timeline.js` 的 `scale()` 上面：
「底下每一段註解都是同一個故事的下一次。不要因為『太長了』把它們收成一句。」）

### 18.4 收合在假瀏覽器裡是無條件成立的

R2 的收合靠 `<ul class="hits-why-lines">` 的原生 `hidden`，而 `scripts/fake-dom.mjs`
**沒有版面引擎**——那裡的 `hidden` 永遠等於「收起來了」。真瀏覽器不是：
UA 的 `[hidden]` 是一條優先權極低的 `display: none`，任何一條自訂的 `display`
都壓得過它。

這棟房子被這個坑咬過一次，收據就在 `apps/desktop/ui/styles.css` 的
`.avatar[hidden]` 上面（「`.avatar` 自己指定了 display，所以 UA 的 `[hidden]`
壓不過它」）。**那一次的症狀，正是「每一條斷言都說收起來了，畫面上攤開著」。**

所以加了一條讀 CSS 原始碼的判準。它先證明自己不是一支空工具——
斷言剖析到 100 條以上的規則、而且 `display` 那根針真的打得到東西——
再問有沒有任何選擇器可能打到那份清單。兩刀都只紅那一條、訊息正確：
`display` 寫在它旁邊那一格，和包在三百行外 `@media` 裡的 `[data-hits] ul`。

### 18.5 我的刀證明了一件收據沒講的事：守著「按鈕只在空手那一格」的是 golden

delegate 的 26 刀切在**抽出來的 15 條 driver** 上，不是整支 940 條。
所以我自己切了一刀在真的那一支：讓收合區在**有命中**的時候也畫出來。

紅了 18 條，其中包含 `A153 chapters/hits/no-range DOM bytes unchanged`
三份 golden 快照。**那三份才是守門的人**——新加的 15 條 R2 斷言一條都沒紅，
因為它們問的都是「空手那一格長什麼樣」，沒有人問「別的格子不該長出這個」。

這一格值得記：**新功能的斷言天生只看得到它自己那一格。**
「它不該出現在別的地方」那一半，往往是既有的全樹快照在守，
而那件事只有真的切一刀才知道。

### 18.6 delegate 自己抓到一次假綠，以及它指出我派工單一條錯

它第一次切「click 多送一次」那一刀時，夾具的拒絕 Promise 沒有 catch，
Node 對 unhandled rejection 直接殺 process，測試提早結束印出
`5 passed; 0 failed`——**讀起來像「這一刀沒被抓到」**。
它判定那不算任何證據，補上 catch 重跑，26 刀才正常抵達並紅在 `14 passed; 1 failed`。

它指出我派工單三條，其中一條是實的：我寫「照抄 `timeline.js` 那顆按鈕會在
**載入時**就炸」，而 `getAttribute` 是在 click callback 裡讀的——載入只註冊
callback，要點下去才炸。結論（fake-dom 缺 `getAttribute`、所以改用 `hidden`）沒變，
錯的是失敗時機。**派工單裡我自己的事實宣稱，連續三輪都是缺陷密度最高的地方。**

## 19. alpha.153 其實從來沒發出去，因為我種的 golden 只在我的時區成立（2026-09-24）

### 19.1 症狀：53 條全綠、量的是打 tag 那棵樹，CI 還是紅

`c39df33` 之後 CI 連紅五個 commit，其中一個就是 `v0.1.0-alpha.153` 那個 tag。
那個 tag 沒有對應的 release——Linux job 一紅，Release job 被靜靜跳過，
**使用者從來沒拿到過 alpha.153**。

而 `/home/ted-h/tmp-tests/gates-all.sh` 在打 tag 前跑過 53 條全綠，量的是
detached worktree、樹指紋對得上。**兩件事都是真的。**

### 19.2 病因：`getHours()` 讀的是行程的時區

`check-pet-says-why.mjs` 的 a153 golden 是整棵 DOM 的 JSON 快照，
裡面有 `08:00`、`19:00` 這種牆上時鐘的字串。產生它們的 `clock()` 和 `when()`
（`apps/desktop/ui/app.js` 第 4766 和 4774 行）用的是 `d.getHours()`。
開發機是 EDT，CI 是 UTC。同一批裡**沒有時間字串**的那一份 no-range 快照
從頭到尾是綠的——那就是對照組。

`TZ=UTC node scripts/check-pet-says-why.mjs` 在本機逐字重現了 CI 的
`938 passed; 2 failed`，兩條紅的名字一模一樣。

**所以「閘門的綠只對它跑過的那棵樹有效」這句話還不夠。**
它只對「**那棵樹 ＋ 那個行程環境**」有效。

### 19.3 修法有三件事，少一件都不算修好

1. **釘死時鐘**。`process.env.TZ = "UTC";` 放在 import 之後。
   實測 Node v24.21.0 這個賦值中途生效。
2. **加一條指名原因的前提斷言**。沒有它的話，唯一會紅的是一整棵 DOM，
   訊息讀起來像「產品變了」，而真正的原因是時區。
   把釘子拿掉跑本機 EDT 得到 `938 passed; 5 failed`：三條前提各自指名時鐘，
   其中 `A153 no-range 前提` 紅了**而它的 DOM 沒紅**——
   那就是「這條前提獨立在量時鐘、不是 golden 的副作用」的證據。
3. **重產 golden 不准用「跑一次、把輸出覆蓋回檔案」**。做了兩件事：
   - 結構逐節點比對：chapters 14 個節點、hits 20 個節點，兩邊路徑完全一樣，
     各只有 2 個欄位變。
   - 那 4 個新值用 Python 的 `zoneinfo` 從**輸入時間戳**獨立算一次。
     位移**不是常數**：1969-12-31 那個是 EST 差 5 小時、2025-08-12 那個是
     EDT 差 4 小時，各自對得上那一刻真實的偏移。套一個常數上去就是沒驗。

四個時區各跑一次都是 `943 passed; 0 failed`，含加德滿都（UTC+5:45）。

### 19.4 全族掃描：只有這一支有病

找到一個洞之後下一個動作是拿同一把刀去試它的每一個兄弟。
十二支 node 閘門 × 三個時區（UTC、Asia/Kathmandu +5:45、Pacific/Kiritimati +14），
**比斷言數不比 exit code**。結果：只有 `check-pet-says-why.mjs` 有病，
其餘十一支三個時區斷言數一字不差。

挑那兩個時區是因為偏移不是整點、而且跨日界線；整點時區會讓分鐘那一格的
bug 躲過去。

掃描本身踩了一個已經記在案的坑：`check-cli-status` 三個時區都紅 ✗=4，
差點被我當成第二個受害者。真因是掃描迴圈**忘了把 cargo 加進 PATH**，
紅的是 `cargo: command not found`。單獨跑（PATH 對的）89 條全過。
**紅了要先比環境再比產品。**

### 19.5 alpha.153 重新打了 tag，不是燒掉版號

沒有 release 是關鍵前提：沒發布過就沒有使用者看過它，把 tag 移到修好的
commit 是零影響。動手前先確認 `scripts/release-notes.sh` 產出的 body 和打
tag 當天那一份**逐位元組相同**（8131 bytes，一字不差），所以重打不會讓任何
人讀到不同的說明。

那次紅的 job **只有一個**：`Linux — test, lint, privacy`。
Windows、macOS、X11 全綠。時區那一條就是唯一擋住它發布的東西。

重打之後的收據：tag 指向 `394f419`，兩輪 CI（main 那一輪和 tag 那一輪）
六個 job 全綠，Release job success，2026-09-24T05:30:23Z 發布，四個 asset
都是 `uploaded`（`AI-Sister-Setup.exe` 277,983,959 bytes）。
body 的**前綴**和事前算好的那 8131 bytes 逐位元組相同，後面只多了 GitHub
自己附的 Full Changelog 那一行。

### 19.6 `gh release view` 對「沒有 release 的 tag」也會 exit 0

查證的時候差點被騙第二次。`gh release view v0.1.0-alpha.153 --json
tagName,publishedAt,assets` 回的是 `tag=v0.1.0-alpha.153 published=null
assets=0` 而且 exit 0——讀起來像「有這個 release，只是還沒發布」。
同一秒 `gh api repos/<repo>/releases/tags/<tag>` 回 404。

**權威是 API 那一條。** 這和已經記在案的
「`gh run view --json` 把還沒跑到的步驟回成 `""` 不是 `null`」是同一族：
`gh` 的高階指令會替不存在的東西編一個看起來合理的空殼。

### 19.7 流程上真正的失敗

專案的規矩本來寫的是「打完 tag 要確認 release 真的發出去」。
這次更早一步就錯了：**平常的 commit 推上去也要看 CI**，不是只有打 tag 那一次。
一行就夠：

```
gh api "repos/teddashh/AI-Sister/actions/runs?head_sha=<sha>" \
  --jq '.workflow_runs[] | "\(.status)\t\(.conclusion)"'
```

## 20. alpha.154：她問得出「你可以告訴我嗎」，而且真的記下來（2026-09-24）

### 20.0 這一版在做什麼

他的原話是：「不要忘記她除了看到的資料以外，他本來就是一個 AI agent，
也可以透過聊天得到使用者的資訊記在資料庫裡啊，結果現在大量思考反而把聊天的
功能廢掉了。」

alpha.153 做完了空手那一格的前三刀（先講沒有、再邀請、按鈕收起理由）。
**第四刀是把他講的話收下來。** 分兩輪派工：R1 是儲存層（新的
`SourceKind::Told`、`remember_told`、三條刪除路、設定開關），
R2 是接線（tauri command、對話那一格的接話、出處標籤）。

**兩輪必須合成同一個版本出貨。** R1 自己有設定頁的勾勾而沒有非測試呼叫端，
單獨出貨就是 AGENTS.md §零的違規（功能要嘛整條出貨、要嘛完全不進產品）。

### 20.1 R1 的鏡頭擺在「它憑自己判斷加的行」

Delegate 打了 23 刀。刀多的時候鏡頭不該重跑它的刀，該擺在**它做了決定而
派工單沒有要求的地方**。五條查過站得住：

- 六個 `from_str_kind` 呼叫端改成回 `Err`，逐一數過**沒有一條**會因為一列壞掉
  就整支查詢死掉（全是 `rows.flatten()` 或逐列處理）。
- `PruneReport::is_empty()` 是**整個結構比對**（`*self == Self::default()`），
  所以新增一欄自動被算進去——這是房子原本就做對的形狀，不是手抄欄位清單。
- prune / forget 先刪 told 再刪其餘，兩個 rowcount 各自正確。
- `PrivacyConfig` 的 `#[serde(default)]` 讓舊設定檔少那一欄時拿預設值。

我本來要報一條 SQL 三值邏輯的問題（預覽用 `source_kind != 'told'`，
而 `NULL != 'told'` 是 NULL 不是 true）。查 schema：`source_kind TEXT NOT NULL`。
**不可達，不成立。** 讀取端推不出可達性，要去看寫入端——這條規矩救了一次假警報。

### 20.2 R1 真的漏的那一條：一對姊妹型別只補了一支

派工單要的是「**每一個**把 `SourceKind` 變成給人看的字的地方」。
它找到 7 處，漏了 `FactOrigin::from_target_row`（`crates/sister-core/src/db.rs`）。
它的姊妹 `FramelessOrigin::from_source_kind` 補了 `"told"`，這一支沒補，
`("told", None)` 掉進 `Self::Unknown`，於是她會對一句他自己打進來的話說
「來源沒有記清楚」。

兩支吃的是**同一種資料**，差別只在 frame_id 有沒有值。
偵測法是數 `match` 上有沒有 `_ =>`：全樹 15 個 match 碰到這兩個型別，
3 個有 catch-all，delegate 補了其中兩個。

### 20.3 R2：delegate 糾正我五件事，兩件打在方法論上

1. 我寫「端到端那條紅了，代表三個索引有一個沒灌到」——**假的**。
   `search` 有 `search_like` 退路，漏索引照樣搜得到。它改成直接 MATCH 三張
   FTS 表，另外用「bigram 內容錯誤」那一刀證明它真的抓得到漏索引。
   我那句話正是我自己記憶裡的「一條紅可以紅在別的理由上」。
2. 我寫「每一刀另外問：整段刪掉會不會也紅？」——**不能當通則**。
   要求功能**存在**的測試，本來就必須在功能被刪掉時紅。那條規矩是給
   「守某個特定語意」的刀用的。
3. `with_db` 只給 `&Db`，`remember_told` 要 `&mut Db`——它用了現成的 `with_db_mut`。
4. 不只 `apps/desktop/ui/timeline.js` 缺來源：native 的 `Hit` / `Moment` 本來
   就沒把 `source_kind` 帶到 renderer。我低估了工作量。
5. 它**獨立撞到同一顆時區 bug**（分支是從我修之前切出去的），
   用原始產品程式在 UTC 重擷取，結果和我推上 main 的那兩份**逐位元組相同**。
   兩支獨立抽取器給出同一個答案，比任何一邊自己的說法都強。

### 20.4 我改的第一件事：兩句話各自是真的，接起來把十二分之一講成全部

畫面上會變成：

> 我翻過的那幾段裡沒有這件事。**我記憶中都沒有這一塊**，你可以告訴我嗎?

第一句是刻意收窄的——她可能只翻了 30 天，說「我記得的東西」就是把
十二分之一講成全部，那段註解花十四行解釋為什麼。**第二句立刻把它放大回去。**
三種變體裡有兩種是同一件事講兩次。

改成只接後半句「你可以告訴我嗎?」。這不是改他的字：前半句這一格自己已經
講過了，而且講得比它準。這就是 AGENTS.md §二那個簽名 bug——
**每一行都是真的，湊起來在說謊。**

新斷言數四種「沒有」的說法在同一格出現幾次，要求剛好一次；外加一條
「這一格真的在邀請」的前提（邀請不出現時數到 1 只是因為沒人接第二句）。

### 20.5 我改的第二件事：被測到的那一格永遠到不了

`FactOrigin::Told` 補上之後會產生三句話，其中一句是壞的：

> 這個目標的**你告訴她的話**沒有記是哪個 app

`origin_subject()` 填的是「這個目標的 X」這個所有格。其他四種都是目標
**擁有**的東西（它的畫面、它的視窗標題）；他打進來的話不是目標擁有的東西，
**它就是目標**。

**漏掉的地方比句子本身值錢**：delegate 的測試蓋的是 `TargetApp::Known`，
而那一格要有 `app_id` 才到得了，`remember_told` 從不寫 `app_id`。
所以真的有 told fact 的那天走到的是 `AppNotRecorded`——也就是壞掉那一句。
**被測到的是永遠到不了的那一格。**

### 20.6 查過站得住的

- `remember_told` 是 `#[tauri::command(async)]`，不在主執行緒上搶 DB 鎖。
  這棟房子被同步 command 和輪詢執行緒搶鎖死鎖過。
- 每次呼叫重新 `Config::load`，沒有快取開機時讀的設定。
- 三種結果分得開：`RememberToldOutcome::{Remembered, Disabled}` ＋ `Err`。
  「開關關著」和「出錯了」不是同一個 false。
- **不講 guardrail**：成功只說「記住了。」。閘門有兩條獨立的否定斷言
  （`已儲存至本機資料庫|這台電腦|保留期設定` 和 `隨時刪除|允許的範圍|隱私承諾`）。
  他的原話：「沒有人跟朋友講話會講這些 guardrail 的。」
- `check-told-native.py` **不是沒人跑的閘門**——我一開始以為它沒接進 CI，錯了：
  它是被 `check-pet-says-why.mjs` 的 A154-8 用 `spawnSync` 叫起來的，
  而那一支在 `.github/workflows/ci.yml` 有跑。它把真的 command 本體從
  `apps/desktop/src-tauri/src/main.rs` 抽出來、接真 Config／真 SQLite 跑，
  是這棵樹上第一份 tauri command 的執行覆蓋。
- 合併沒有弄丟任何人：`LC_ALL=C comm -23` 量過，main 的 901 個斷言名字一個
  都沒少、沒改名。合併後 `970 passed; 0 failed`（main 943 ＋ A154 27）。

### 20.7 登記在案、這一版不修的

- 另外五個 `from_str_kind` 呼叫端沒有「我少給了你幾列」這個頻道。
  今天不可達（沒有寫入端產得出第七種 kind），但它是一顆會自己上膛的雷。
- `origin_subject` 在 `crates/sister-core/src/db.rs` 和 `crates/sister-cli/src/ops.rs`
  有**兩份逐字相同**的拷貝。這個重複不是這次造成的，但補 told 的時候兩邊都要動。
- 她不會拿他剛剛告訴她的那句話**回頭重答原來那個問題**。下一次問才找得到。

## 21. alpha.154 的出貨收據，以及那份驗收清單其實只被檢查了三分之一（2026-09-24）

### 21.1 出貨收據

`v0.1.0-alpha.154` = `3bd9bd2`，**2026-09-24T06:39:14Z 發布，四個檔**：

```
AI-Sister-Setup.exe             277,998,816
sister-desktop.exe               72,022,016
sister.exe                       11,168,768
AI-Sister-Linux-X11-amd64.deb    65,548,328
```

tag 那一場 CI 六個 job 全綠，含 `Windows — test and build` 36/36，
以及 alpha.153 那次擋住 release 的 `Linux — test, lint, privacy`。
release body 的**前綴**和打 tag 前從 tag 工作樹算出來的那一份逐字相同，
GitHub 附加的是那 103 個字元的 `**Full Changelog**`（見
[[release-body-has-an-appended-tail]] 那條的道理：驗前綴，不驗相等）。

打 tag 前的最後一個動作是 `git worktree add --detach 3bd9bd2` 再跑一次
`/home/ted-h/tmp-tests/gates-all.sh`：53 條全綠，樹指紋 `09c826f8eeaff83c`。
**這一次也順便證明了前一輪那個 52/1 是我自己造成的**——那次我一邊跑閘門
一邊跑 `cargo`，弄紅的是 `ops::watch::tests::an_admitted_watch_round_drains_before_stop_and_does_not_publish_after_pending`。
乾淨環境下同一棵樹 53/0。**閘門要獨佔這台機器。**

### 21.2 A155 R1：我量到的是真的，我推的成因是假的

alpha.155 要修的是「換個說法她就找不到」。我先用三份探針量出最小對：
「客服電話」找得到、「找客服電話」回**空陣列**。

派工單我寫了兩條成因，**第一條對、第二條錯**，而錯的那條是 delegate 退回來的：

- 我寫「語料的事件 `at_ms: 200`，那是 1970 年，所以日曆詞的題目被日期過濾濾光」。
- 實際上 `at_ms` 是**相對位移**：`eval::replay_origin()` 把零點放在本地昨天 13:00，
  `Db::import_replay(&corpus, origin)` 入的是 `origin + at_ms`。
- 真正的成因是 `question::DAY_TOKENS` 裡沒有「這個月」，於是
  「這個月的電信帳單金額是多少」被剝成 terms =「月的電信帳單金額是多少」，
  而 `time_range` 是 **null**——那條路上根本沒有日曆過濾。
  它是另開資料目錄、`replay import --start <當下 epoch ms>` 重跑一次量出來的。

**夾具裡的數字在變成診斷之前，要先讀消費它的那個函式簽名。**
`at_ms` 這種名字，絕對和相對在檔案裡長得一模一樣，而兩者導出的結論相反。

派工單裡那一節叫「**這份派工單裡我寫錯的地方**」，這條就是從那裡回來的。
**留一格給它反駁我，是免費的對照組。**

### 21.3 真正的收貨判準是一份它沒看過的題目

R1 跑到一半我去看了一眼，看到它在寫：

```rust
const PREFIXES: &[&str] = &["幫我找", "要打", "查一下", "找", "查", "打"];
```

六個字串裡**五個是從我派工單的舉例抄下來的**。所以我在它交件之前，
先做了一份 17 題的 held-out：動詞全部換過，另外埋三題語料裡真的沒有的
（「火星會議連結」）當「應該找不到」的對照組，並且先量了修前的分數。

交件之後兩邊各跑一次：

| | 派工單那 17 題 | 我那份 held-out | p50 | p95 |
|---|---|---|---|---|
| 修前 | 9/17 | **6/14** | 0.674 ms | 0.720 ms |
| 修後 | 17/17 | **7/14** | 0.690 ms | 1.318 ms |

唯一多對的那一題，用的動詞正好是「找」——在那張表裡。
「搜尋」「跟我說」「翻一下」「幫我算」全部還是空手。

**它學會的是那六個字，不是那件事。** 三題對照組沒有變成找得到（沒有放太鬆），
p50 沒動（沒有換成掃全表），所以它是安全的——但安全不等於做到了。
這樣出貨，版本說明只能寫「她認得這六個開頭」，而我們想講的是
「你換個說法她也聽得懂」。`AGENTS.md` §二：每一行都是真的，湊起來在說謊。

**判準不是「派工單那批變綠幾題」，是那份它沒看過的。**
這一條要留給下一輪：**舉例會變成實作**。派工單裡的每一個例子，
都要假設它會原封不動出現在 `const` 裡。

### 21.4 R1 的診斷是對的，而且它是這次真正的資產

兩條機制，我自己重驗過：

- **facts 那半**：`facts::topic_constraint("找客服電話")` 去掉類型詞「電話」之後，
  留下的**必要主題**是「找客服」，而出處是「客服專線」，所以
  `answers_during` 拒絕它。不是 bm25 排名問題。
  對照：「幫我找客服電話」會被 `strip_fact_question_edges` 剝成「客服電話」，
  主題是「客服」，所以它反而成功——這正是「開頭有動詞就死」這個假說的反例。
- **原文那半**：`db::fts_query` 把每個詞用 `AND` 串起來，中文 bigram 也是 AND 串。
  所以任何一個對不到的字都是一個**必須出現**的條件。

它的安全結構也是對的，R2 要原封不動留著：只有 facts、原文、章節**三者都空手**
才重試；重試不替換任何既有命中；`Retrieval` 帶回實際比對的字，呼叫端不准重算。

### 21.5 R1 把兩種不一樣的事壓成同一個欄位

`Retrieval.searched: Option<String>` 現在同時代表：

1. **黏出來的**——退格後剩下一個不是詞的碎片（`剛剛那個板` → `個板`）。
   她真的拿垃圾去比對了，下一步是叫他重打。
2. **放寬過的**——原查詢空手，她丟掉對不到的字再試一次。她做了一件合理的事。

R1 把兩種都印成 `我拿去比對的是「X」。`，於是第一種**失去了下一步**。
而且它是把既有那條斷言**改掉**（原本要求 `said.contains("再問一次")`），
不是加一條新的。被它刪掉的那段註解本來在**反對**這次改動：

```
// 只在**黏過**的時候講。剝掉「剛剛那個」留下「優惠方案」是剝對
// 了，每次都報一句只會讓人學會忽略它；黏出「個板」才是她找了一
// 個不是詞的東西。
```

那段論點沒有被推翻，被推翻的只有「這個欄位只會有一種情況」。
（同族：`diff` 裡每一段被刪的註解都要問「它在描述機制，還是在反對我？」）

### 21.6 那份驗收清單，其實只有三分之一被檢查過

追那句被刪掉的文案還有誰在引用，撞到這個：

`docs/WINDOWS-CHECKLIST.md` 有一條叫他去找
「我拿去比對的是『的人』——那是從你打的字黏出來的，不是一個詞。
直接打你要的那個詞再問一次」。R1 把那句話從產品裡拿掉了，
而 `scripts/check-checklist-quotes-exist.py` **是綠的**。

兩個盲點疊在一起：

1. **它逐行走。** `wanted = QUOTE.findall(ln) if SAYS.search(ln) else []`，
   而 `QUOTE = 「([^「」\n]+)」` 明文不吃換行。清單是六格縮排、七十幾欄折行的，
   所以引號折到下一行、或動詞和引號被折開，整句就消失。
2. **`『』` 被當成佔位符。** `TEMPLATE` 裡有 `『』`，而清單用 `『』` 當巢狀引號
   （外層 `「」`、裡層 `『』`），產品的巢狀卻是 `「」` 包 `「」`。

量了一次全份——**用它自己的邏輯量，只換掉 `items()` 那一層**（我第一次是自己
重寫一份規則去模擬，得到 58 → 156，兩個數字都是錯的；拿工具本身當儀器才對）：

| | 檢查了幾句 | 其中對不上原始碼 |
|---|---:|---:|
| 現況（逐行） | **71** | 0 |
| 把條目續行接起來 | **203** | **51** |

多出來的 132 句從它上線那天起沒有被檢查過，而其中 51 句在原始碼裡找不到。
那 51 句不會全是真的洞——粗暴地 `"".join` 會在接縫上造出
「或正　　在開機」這種沒有人說過的字串，也有一部分是散文轉述。
**但那正是為什麼這件事要派出去做而不是我順手改掉：分類才是工作量，接行只是十行。**

最難看的是這一段的前一天。2026-09-23 我加了十幾組引號進這份清單，
發現它印的數字沒變，**診斷得完全正確**（兩個成因都指名了），
然後做的事是——把我自己那一節重寫成動詞和引號同一行，讓閘門看得到我加的東西。
62 → 64，收工。**我把測試改成符合工具，而不是把工具修好。**

偵測法其實免費：**那支工具在比較的另一側有沒有做同一種正規化？**
這支腳本的 `haystack()` 有一整段 `re.sub(r"\\\n\s*", "", text)` 把 Rust／JS 的續行
接起來，註解還寫著「不接起來的話，清單裡逐字抄對的長句子反而會對不上」。
**原始碼那半接了，文件這半沒接。同一支檔案，隔四十行，只解了一半。**

還有一層：那支腳本有一整段 docstring 誠實列著「**這支腳本守不住的那一半**」，
五條。折行不在裡面，`『』` 也不在裡面。
**自白清單只列得出作者想過的那幾種，而自白的語氣會讓人停止查。**

### 21.7 派出去的兩輪

- **R2**（`a155-r2`，起點 `4af2428`）：把白名單換成**用索引當裁判**——
  失敗的成因是「有一個對不到任何東西的字被當成必須成立的條件」，
  那就逐個 conjunct 問索引「你有沒有這一格」，零筆的是雜訊；
  **全部零筆就維持空手**（這是三題對照組不被放寬的保證）。
  硬規定：diff 裡不准出現任何一張口語詞／動詞的常數清單。
  外加把 `searched` 拆成分得出兩種的型別，把被改掉的舊斷言原封不動加回去。
  成本要交代：`search_during` 最後一段是全表掃描（同檔註解記著一次實測 102.4 ms），
  重試會讓空手題再走一次那條梯子。
- **R3**（`a155-r3`，起點 `4af2428`）：修 §21.6 那道閘門，並且把冒出來的紅燈
  分成三類——清單過期（改清單）、產品退化（只報不改）、根本不是逐字引用（改寫成散文）。
  **明文禁止「為了變綠去改清單」：那是把偵測器刪掉，不是把 bug 修掉。**

兩輪的檔案集是不重疊的（R2 不准碰 `docs/`，R3 只准碰
`docs/WINDOWS-CHECKLIST.md` 和那支腳本），所以並行跑。

### 21.8 登記在案、還沒修的

- `scripts/check-told-native.py` 收尾是 `raise SystemExit(result.returncode)`，
  **沒有解析 `N passed; M failed`**。跑 0 條測試的話它讀起來是綠的。
  今天機率低（那些 `#[test]` 都是字面寫死的），但它是同一族的洞。
- A155 R1 的重試會讓空手題把 FTS → bigram → LIKE 整條梯子再走一次。
  三個事件的語料上是 0.72 → 1.32 ms；兩百萬行上沒有量過。

## 22. alpha.155：白名單被退回，第二輪用索引當裁判（2026-09-24）

### 22.1 這一版在做什麼

「客服電話」找得到，「找客服電話」回**空陣列**。差一個動詞。

兩條成因（R1 診斷、我自己重驗過）：

- **facts 那半**：`facts::topic_constraint("找客服電話")` 去掉類型詞「電話」之後，
  留下的**必要主題**是「找客服」，而出處是「客服專線」，所以 `answers_during` 拒絕。
  反例很重要：「幫我找客服電話」**會過**，因為 `strip_fact_question_edges` 把它剝成
  「客服電話」。所以「開頭有動詞就死」這個假說是錯的。
- **原文那半**：`db::fts_query` 把每個詞用 `AND` 串起來，中文 bigram 也是 AND 串。
  任何一個對不到的字都是一個**必須出現**的條件。

### 22.2 R1 交回一張白名單，我用一份它沒看過的題目退回它

```rust
const PREFIXES: &[&str] = &["幫我找", "要打", "查一下", "找", "查", "打"];
```

六個字串**五個抄自派工單的舉例**。我在它交件前就做了 17 題 held-out（動詞全換過，
外加三題語料裡真的沒有的當「應該找不到」對照組）並量了修前分數：

| | 派工單那 17 題 | held-out | p50 |
|---|---|---|---|
| 修前 | 9/17 | **6/14** | 0.672 ms |
| R1 | 17/17 | **7/14** | 0.690 ms |
| R2 | 17/17 | **12/14** | 0.797 ms |

R1 唯一多對的那題，動詞正好是「找」——在那張表裡。

**判準不是「派工單那批變綠幾題」，是那份它沒看過的。**
派工單裡的每一個例子，都要假設它會原封不動出現在 `const` 裡。

### 22.3 R2 的機制：索引就是裁判

派工單只給了原則（「失敗的成因是有一個對不到任何東西的字被當成必須成立的條件，
那就讓資料自己說哪個字對不到」）和一條硬規定（diff 裡不准有任何詞表），
機制讓它自己想。它交回來的是：

- 把查詢拆成條件（中文相鄰雙字，其餘保留 token），逐個拿去問三個既有 FTS 索引
  `EXISTS`。**只跳過開頭連續零命中的那幾個**，一碰到已知條件就停，
  不刪中間也不刪尾端。
- facts 那半另外檢查 `topic_constraint` 的必要主題，必須能經索引縮短到有支持的開頭。
  **主題全部零筆時不重試**——這就是「火星會議連結」不會被亂答的保證。
- 三類結果全空才重試，既有命中一律不替換。
- 重試走新的 `search_indexed_during`（`search_with_scan(..., allow_scan: false)`），
  **所以不多掃一次全表**。我讀了程式碼確認，不是只信它的說法。

它自己報了一件重要的事：**第一版嘗試刪掉所有位置的零命中條件，
讓既有週報題變成可答，第二張 baseline 的 12 個契約斷言紅了**，
所以才收窄成只處理開頭。那個收窄是量出來的，不是猜的。

### 22.4 還是找不到的兩種，都不是回歸

held-out 剩下兩題，我拿修前的執行檔跑同一份階梯確認**修前修後都是 1/5**：

- 雜訊在**後面**（「ERR_DEPLOY_42 怎麼回事」）——這一版只處理開頭。
- 整句話裡她看過的字只剩類型詞（「幫我算這期要繳多少錢」）——
  必要主題全部零筆，被 §22.3 那條保證擋下來。**擋住它的正是讓對照組乾淨的那條規則。**

途中我拿 `query --json` 開一個臨時資料目錄去追第二種，
結果連「ERR_DEPLOY_42」這個**對照題**都空手——那代表我的探針和 eval harness
不是同一回事，量出來的東西不能用。**對照組失敗的時候，壞的是儀器不是產品。**

### 22.5 `searched` 拆成兩種

R1 把「黏出來的」和「放寬過的」壓成同一個 `Option<String>`，
於是第一種失去了下一步（本來會說「直接打你要的那個詞再問一次」），
而且它是把既有斷言**改掉**而不是加上。R2 改成
`SearchAdjustment::{Glued, Relaxed}`，舊斷言原封不動加回去而且是綠的。

### 22.6 那道清單閘門（R3，這一版沒出貨）

`check-checklist-quotes-exist.py` 逐行掃、而引號正則不吃換行，
所以折行的引號整句消失；`『』` 又被 `TEMPLATE` 當成佔位符跳過。
用它自己的邏輯量：**71 → 207 句，其中 40 條對不上原始碼**。

那 40 條不是 40 個 bug。R3 逐條分類：1 條是真的（alpha.155 已修好）、
約 12 條是 `format!` 模板（產品真的說得出口，只是拼出來的）、
約 27 條根本不是「產品會說的話」（描述、舊版歷史、禁止範例、使用者心裡的話）。

第 3 種會冒出來，是因為接行之後**動詞的作用範圍從一行放大成一整條**。
我量過幾種收窄：動詞必須在引號前 → 180/30；引號前 60 字內 → 137/11。
**收窄會把真的在驗的承諾從 167 砍到 126，方向反了，所以不收窄。**
正解是讓模板也驗得到（把兩邊的內插位置正規化再比），
以及把不是產品字串的引號還原成散文——那是這份檔案自己 docstring 寫的規矩。

R3 也退回我四個數字：基線是 71 不是 58、接行是 207 不是 156、
我的三分類漏了「模板／組裝」這一種、舊 docstring 說「否定句一律不驗」其實從來不成立。
四條都是對的。

### 22.7 登記在案

- R3 的分支 `a155-r3` 停在「閘門紅 40 條」，**沒有出貨**。R4 接著做。
- `scripts/check-told-native.py` 收尾沒有解析 `N passed; M failed`（跑 0 條會讀成綠）。
- 尾端雜訊那一類沒做。

## 23. alpha.155 的出貨收據，以及那份清單「引對句子、引錯那一臂」（2026-09-24）

### 出貨收據

tag `v0.1.0-alpha.155`（`03b2a66`）推上去之後，**CI 紅了、release 被靜靜跳過**。
`gh api repos/teddashh/AI-Sister/releases/tags/v0.1.0-alpha.155` 回 404——
這是權威，`gh release view` 對只有 tag 的東西也 exit 0。

紅的是 `Windows — test and build` 的
`UIA — read WPF and Edge visible paragraphs; reject excluded text`。
**先確認指紋再重跑**，兩次抓 log 都失敗（job logs API 回 0 bytes、
`gh run view --log-failed` 只回收尾那幾行 UNKNOWN STEP），
第三次用 `gh run view <id> --log` 抓整份才看到：

```
thread 'native_uia_reads_visible_edits_and_documents_and_rejects_excluded_text'
  panicked at crates\sister-capture\tests\windows_uia.rs:220:5:
assertion failed: changed.contains("CHANGED-SENTINEL 02-2233-4455")
test result: FAILED. 2 passed; 1 failed
```

就是紀錄在案的 UIA read-after-write 時序 flake。**兩個對照組**才敢重跑：

- `git diff --stat v0.1.0-alpha.154 v0.1.0-alpha.155 -- crates/sister-capture` 是**空的**；
  整份 diff 18 個檔，`grep -Ei "cfg\(|windows|uia|assistive|focus"` 在
  `apps/desktop/src-tauri/src/main.rs` 那 52 行上一個都沒有。
- alpha.154 的 tag run 在**同一支 workflow、同一條測試**上是 success。

`gh run rerun <id> --failed` 之後 attempt 2 八個 job 全綠，
release 於 **2026-09-24T22:35:51Z** 發出，四個 asset 和 alpha.154 同一組。
body 比**前綴**：預先算好的 3931 字一字不差，後面只多了 105 字的
`**Full Changelog**: …`。

### 那份清單，之前只有三分之一被檢查

§21 記過 `check-checklist-quotes-exist.py` 逐行配對的盲點。這一輪修好了，
**在 main 上實測：檢查數 71 → 183**。

修好之後第一次跑就露出**四句對不上的**，四句都不是產品退化：

| 行 | 清單引的 | 真相 |
|---|---|---|
| 999 | 她今天不會記得任何事 | 產品**刻意**改掉了；`app.js:26` 的註解寫著為什麼——早上錄過中午按停的人會看到它自己打自己 |
| 1698 | 她錄過，那些東西被 forget 忘掉了或過了保留期 | 全庫 0 命中，是轉述；產品那句在 `ops.rs:16704` |
| 2016 | ■ 收到停止的請求，這就收工。 | 兩層錯，見下 |
| 2069 | 證據鏈記得兩個以上不同的 app… | 把 `Ambiguous` 那句抄成了 `Unknown` 那句的句型，而這一條自己的重點正是這兩句**不可以**混成一句 |

### 第 2016 行：這條閘門結構上驗不出來的那一類

`■` 是 `println!("  ■ {}", …)` 的前綴，不在字串裡——這一層只是引號畫錯範圍。
**真正的問題是引錯了那一臂。** 這一格叫他按系統匣的「結束」，走的是
`request_desktop_quit`（`recorder_supervisor.rs:2199` 是它**唯一的非測試呼叫端**）
→ `StopReason::DesktopQuit` → 印「收到 desktop 結束的停止要求，這就收工。」。
清單引的「收到停止的請求，這就收工。」是 `Requested`，
只有 CLI 的 `sister stop`（`ops.rs:12495`）走得到。

**兩句都在原始碼裡，所以 `q in src` 永遠是綠的。**
這條閘門問的是「產品說得出這句嗎」，不是「這一格走到的是哪一臂」。
抓到它的是去讀呼叫端，不是任何一道閘門。

機械偵測器（把「同一個 `match` 的相鄰臂」收成一族，報出清單只引到其中一臂的句子）
在這份清單上報了 11 句。**下一輪把它退回成 8 句**：那支偵測器用「相鄰 8 行以內」
收族，把兩組**緊鄰的不同 `match`** 誤收成同一族——
`StepApp::request_app` 和 `StepApp::label`（`ops.rs:2653` 與 `2662`，
前者吐 `<兩個以上的 app>` 給 `Grant::covers` 當身份，後者吐
`兩個以上的 app` 印在「宣告 app：」那一行），以及
`frame_sheet_words` 和 `frames_kept_words`（`ops.rs:18777` 與 `18798`）。
兩組我都回去讀過原始碼，確認是兩個 `match`。

剩下 8 句逐條追寫入端（哪顆鍵 → 哪支函式 → 哪個 variant → 哪一臂），
**全部走得到，清單一句都不用改**。所以這一類在這份清單上目前只有一個實例，
就是上面那條已經修好的。

**這是「我的工具報的數字偏多」，不是「清單有 11 個洞」。**
偵測器報出來的數要先證明每一族真的是同一個 `match` 才能引用——
我把它寫進上一版的收據時沒有做這件事。

### 收貨時要自己重量一次的三件事

1. **交回來的樹可能建在錯的基底上。** R4 的 merge-base 是 `4af2428`（R1 的頭），
   **R2 的 merge 不在裡面**。它回報的 180 句／1 紅，量的是一份少了整個
   alpha.155 章節的清單；那 1 紅在 main 上根本不存在（R2 早就把那句還原了）。
   在 main 上重量才是真的：**183 句／0 紅**。
   偵測法一行：`git merge-base <branch> main`。
2. **「原始碼沒有這句」要自己驗。** R4 有一條理由寫錯：
   「她起不來，因為資料庫打不開」其實在 `check-pet-says-why.mjs:1922` 的註解裡。
   結論對（那是一種讀法，不是產品字串），理由錯。
   機械驗法：把**被拿掉的每一句**回頭 grep 一次原始碼——31 條裡只有這 1 條命中。
3. **被拿掉的東西要逐條對得上收據。** 31 條長引號：27 條 R4 寫了，
   4 條是這一輪自己改的，**零條沒有交代**。

### 我自己在這一輪犯的三個錯

- **`cd` 失敗之後，cherry-pick 就地跑在主 repo 上。**
  `git worktree add <已 checkout 的 branch>` 失敗 → `cd` 失敗 →
  後面整條鏈在 `/home/ted-h/projects/AI-Sister` 執行。
  在衝突那一步才發現，reset 回 `0867041`。
  **改用 `git -C <path>`，不要用 `cd` 串。**（同族：那條「cd 失敗之後 cargo 在主 repo 建了 binary」。）
- **我保留了原句，而原句引的是錯的那一臂。** 第 2016 行我只把 `■` 移出引號，
  沒問「這一格走到的是哪一句」。是 R3 改對的。
  我獨立跑出來的四句裡，**三句和 R3 一致，第四句是我錯**。
- **我寫的偵測器連續兩次沒通過自己的對照組。** 第一版用字串相似度，
  已知那一對是 0.595 而我把門檻設在 0.62——**剛好把唯一已知的正例排除掉**；
  第二版改抓 `match` 臂，又漏了 `=> {` 換行才寫字串的那種臂，
  而 `DesktopQuit` 正是那樣寫的。
  兩次都是因為先跑「已知正例抓不抓得到」才發現。
  **寫偵測器的第一行是把已知正例釘成 assert，不是先看它報了幾條。**

### 這一輪沒有打 tag

產出是一道閘門加四句文件更正，**產品一個字都沒變**。
照「不要加 gate，要交出看得見的體驗」那條，這種東西不自己發一版，
併進 main 等下一個功能帶走。

## 24. alpha.156 的出貨收據，以及那兩條「跟著搬家的契約」（2026-09-25）

### 這一版做了什麼

你告訴她之後，她把**你本來問的那一題**再問一次，答案接在「記住了。」下面。
grok 4.7（a156-r1，`33e1809`）做的。`ask()` 從此只做分流，正式那一趟搬進
新的 `runQuestion(question, afterTold)`。

### 收貨量到的

- 我自己的驗收儀器（出貨前先寫、先證明它在沒有功能的樹上會紅）：
  控制組 `981 passed; 2 failed`（剛好是 A156-1／-2），R1 的樹 6/6 全過。
- 第二輪鏡頭（瞄它自己五刀打不到的地方）：重跑又空手時**不會再邀請一次**、
  不重講那三句「沒有」、下一次打字是新的一題不是又一次「告訴她」。三條全過。
- `retrieval.rs` 的 +150 行確認是 test-only（單一 hunk 落在 `mod tests` 裡，
  `#[cfg(test)]` 在它前面，0 刪除），而且那條測試不是空的：它自己從 `Local`
  推每一個時間窗（所以不是把我的時區烤進去）、斷言預設值、**關掉再開同一顆
  資料庫檔**、驗 `source_kind == Told`。

### 閘門紅了兩條，一條是真的

`/home/ted-h/tmp-tests/gates-all.sh` 量 `33e1809`：**通過 51 條，失敗 2 條**。

**第一條是真的，而且 R1 看不到它。** `check-persona.mjs` 那條
「ask 沒有日常短句旁路，每個文字問題都 invoke native ask」用原始碼文字釘住
`ask()` 的 body，找的是 `const answer = await invoke("ask", { question })`。
這一版把那一行搬進 `runQuestion()`，於是它紅了——**意圖（沒有旁路）其實還
成立，壞掉的是「那一行住在哪一支」這個位置代理。**

對照組是乾淨的：main 跑同一支印 0 條 ✗，R1 的樹剛好紅一條。

**同一次搬家還弄瞎了隔壁一條，而它是綠的。**
「bundled Ogg 與答案 TTS 共用一顆完整 stop」裡寫著
`/async function ask\([^)]*\)[\s\S]*?stopPersonaMedia\(\)/u.test(SRC)`——
`[\s\S]*?` 會從 `async function ask(` 一路吃到檔案後面**第一個**
`stopPersonaMedia()`，配到的不一定在這支裡面。搬家之後它照樣綠，
卻已經不在量這件事了。**紅的那條我會看到，綠的這條不會有人告訴我。**

兩條都改成指名 `ask()` 與 `runQuestion()` 兩支的 body，並補一條前提斷言
「兩支 body 都抽得出來」——regex 配不到時 `?? ""` 會讓否定那半自動成立，
沒有這條前提，下次改名就是一次安靜的失明。六刀都證明會紅。

**第二條是那隻老抖：** `ops::watch::tests::an_admitted_watch_round_drains_
before_stop_and_does_not_publish_after_pending`。

### R1 的「17 條全過」是怎麼來的

它自己跑的是 `A156_ONLY=1`，而那不是篩子，是**截斷**：
`scripts/check-pet-says-why.mjs` 第 915 行一句 `process.exit()`，把後面 977 條
全部跳過。所以它從頭到尾沒跑過 persona 那一節，也沒跑過整支。

這個寫法不是它發明的——main 上已經有四個（A154×2、A149×2）。
但四個累積起來就是四個「把這道閘門縮成 2% 再 exit 0」的開關。
**這一版沒有動它們，但值得記一筆。**

順帶對過帳：那 17 條是 16 個 `check(` 呼叫端，其中一個坐在兩輪的迴圈裡
（`disabled` 與 `失敗`）。16 − 1 + 2 = 17，沒有幽靈。

### 還沒關的洞（R1 自己點名的）

重跑途中被同意書攔下的那條路（`pendingConsentReplay`）**沒有測試**。
它的形狀是對的（fail-closed，不替自己宣稱成因），可達性也成立，但要走到
它，同意書的版本得在「第一次問」和「重跑」之間改版。我**沒有**補這條測試：
假 DOM 要走完四張條文才叫得到 `finishConsentGuide`，而那是為一個需要中途
改版才到得了的兩行旗標蓋一座測試架。記在這裡，不要當成沒看到。

### 我這一輪做錯的

1. **版本說明的草稿引了一句產品不會講的話。** 我寫「我記憶中都沒有這一塊，
   你可以告訴我嗎?」——那是使用者的原話，寫在 `app.js:6402` 的註解裡。
   產品實際印的是那三句其中一句**再接後半句**。
   這正是 alpha.155 剛修完的那一類，我在同一天又犯了一次；
   救我的是去讀 `app.js:6397-6419` 的寫入端，不是任何一道閘門。
2. **我的第一輪突變有兩刀是空的。** 一刀注入 `dailyDialogueReply(question)`
   ——未定義函式，app.js 一載入就炸，`rc=1` 但 **✗ 是 0 條**，那是 harness
   崩掉不是斷言抓到；另一刀的錨點 `stopPersonaMedia();` 在檔案裡有 14 處。
   改成「在 body 裡塞一行註解」和「連著上一行一起當錨點」之後兩刀才真的紅。
3. **我一開始把 `A156_ONLY` 讀成 R1 自己造的風險**，查了才發現 main 上已經
   有四個同形狀的。先查先例再定性。
4. **打 tag 前那一趟閘門，紅的是我這一節自己。**
   `check-docs-point-somewhere.py` 把反引號裡的裸「gates-all.sh」解析成
   `scripts/` 底下的東西（它不在那裡，它是本機工具），又把反引號裡帶行號的
   那一串當成一條路徑。**兩種我都已經記過一次**——本機工具一律寫絕對路徑、
   行號寫在反引號外面——而我在同一節裡把兩種都犯了。
   這也是這道閘門第三次在打 tag 前抓到我，它的價值在這裡：
   **它擋的不是別人，是我寫收據的時候。**

## 25. alpha.157：放寬不能停在她看過的常用字前面，也不能切到只剩一個字或一個種類（2026-09-25）

### 25.1 alpha.156 的出貨收據

tag `v0.1.0-alpha.156`（`d313ab8`）一次就過：八個 job 全綠，release 於
**2026-09-25T09:32:10Z** 發出，四個 asset、prerelease。

### 25.2 R1：尾巴對不到的字放掉，類型詞留著

grok 4.7（`1c93f40`）。`Db::indexed_candidate` 從 `Option<String>`
改成四種結果（`NoneSeen`／`Unchanged`／`Changed`／`TooLong`）；尾巴連續零命中、
而且不是類型詞的條件放掉，類型詞只認既有的 `QUERY_KIND_TABLE`。

兩件要記的：

- **alpha.155 的主題保護有一個「兩種 None」。**「主題一個字都不用改」和
  「主題一個字都沒看過」回的是同一個 `None`，所以「請問月報連結」剝完剩下的主題
  「月報」明明看過，整題還是空手。R1 把它拆開了。
- **recall-session 的 12 個契約數字是有意改的。** 「昨天下午那份週報寫了什麼」
  從此在 `baseline_text` 和 `facts` 也答得出來（尾巴的「寫」放掉，改用「週報」），
  兩列變成 2/2、3/3、2/2——就是 §22.3 記的那 12 個契約斷言，當時紅了是退路，
  這次是想要的結果。README 的表由腳本重產；表下面那段散文和 PHASES.md 第三階段
  那一格 `[x]`（「找回率相對 Phase 2 baseline 提升可量測」）的憑據就是這張表，
  三個配置打平之後兩處都照實改了：散文重寫，那一格加註、沒有取消勾選。
  README 那段的歷史我只量了 alpha.156（第一趟「份週報寫」、放寬成「週報寫」、0 筆），
  更早的版本沒量，所以沒寫。

### 25.3 R1 全綠，而它的前提在真的索引上不成立

R1 的驗收（我寫的）和原型都建在帳單夾具上：三張畫面，「怎麼」「請問」「哪裡」
一次都沒出現過。**判準是「索引裡看過沒有」的規則，行為跟語料大小有關**——
夾具太小，常用字永遠「沒看過」，驗收量到的只是剛裝好那一天。

我把 repo 裡的中文文件（約 24 萬字，排除所有主題字）當背景灌進去，
跑和 R1 同一套演算法：

| 問句 | 沒有背景 | 有背景 |
|---|---|---|
| `ERR_DEPLOY_42 怎麼回事` | 改用「ERR_DEPLOY_42」，找到 | 改用「ERR_DEPLOY_42 怎麼」，0 筆 |
| `部署失敗怎麼辦`／`部署失敗是什麼原因` | 找到 | 空手 |
| `客服電話怎麼打` | 找到號碼 | 改用「客服電話怎麼」，空手 |
| `月報連結在哪裡`／`告訴我 月報連結`／`電信帳單寄到哪` | 找到 | 空手 |
| `幫我看一下客服專線`（alpha.155 已出貨的那一半） | 找到 | 空手 |

主題保護也有洞，而且是 alpha.155 起就出貨的那一道：「退款電話怎麼打」的主題被算成
「退款 怎麼打」，「怎麼」看過，保護就放行。有背景時 alpha.156 改用「電話怎麼打」去找，
R1 的原型改用「電話怎麼」。（我在 R2 派工單裡把這兩個版本的輸出寫成同一個
「電話怎麼打」，那是 alpha.156 的，不是 R1 的。）

**判準：一條靠「資料裡有沒有」來判斷的規則，驗收要有一份對抗的背景，
外加前提斷言證明那些字在背景裡真的看過。** 沒有前提斷言，背景哪天被改小，
驗收會安靜地退回只量剛裝好那一天。

### 25.4 R2：一張封閉的問法清單

grok 4.7（`5c16af3`）。放寬之前先切：頭尾套 facts 主題同一張表
（`strip_fact_question_edges`，這一版多五句請求），再找第一個不在類型詞裡的
問法字（`QUESTION_WORDS`）——前面有內容就留前面，前面只有虛字就拿掉它繼續往後。
主題保護改用切完的字算主題。上面那張表在有背景時全部找回來，「退款電話怎麼打」
改回空手。

**那五句請求會改到第一次查詢。** 這張表也是第一次查詢算 facts 主題用的，
所以帶「幫我看一下」這類請求的問句，第一趟的主題就變短了。91 句探針裡只有
「幫我看一下客服專線」一句的第一次查詢變了：兩臂都第一趟就答出號碼、不再走放寬；
剛裝好那一臂 alpha.156 是改用「客服專線」、號碼加 1 張畫面，現在只有號碼
（號碼自己點得開出處）。alpha.155 版本說明那句「既有的答案和排序一個都不動」
對這一類不再成立，alpha.157 的說明沒有再引用它。

這張表是照順序拿第一個對上的，「幫我看」要排在「幫我看一下」後面。
我的突變 M1 把它倒過來，1166 條全綠：既有那條驗收明寫「不規定是第一次查到的
還是放寬後查到的」，而放寬那一趟照樣答得出號碼。版本說明寫了「第一趟就答得出號碼」，
所以收貨時補了 `facts.rs` 的 `a_request_is_stripped_whole_not_by_its_shorter_prefix`，
直接量那張表。

### 25.5 R2 也全綠，而一份它沒看過的問法把它打出四種新的壞法

R2 的驗收是照上面那張表寫的，所以它只看得到那張表。我另外寫了一份**口語**
問題（口語開頭、沒有主題、只有種類），在同一份背景上和 alpha.156 對照：

| 問句 | alpha.156 有背景 | R2 有背景 |
|---|---|---|
| `所以為什麼部署失敗` | 空手 | 改用「所以」，5 張不相干的畫面 |
| `可是怎麼辦` | 空手 | 改用「可是」，4 張 |
| `怎麼回事` | 空手 | 改用「回」，5 張（一個字） |
| `電話怎麼打` | 空手 | 改用「電話」，任意一支號碼＋5 張 |
| `應繳金額怎麼算` | 空手 | 改用「應繳金額」，5 筆不相干的金額 |
| `哪裡有客服電話` | 空手 | 改用「裡有客服電話」，0 筆 |

剛裝好的那一半也退了：「所以為什麼部署失敗」在 alpha.156 找得到，R2 空手。

**四種裡有兩種違反的是 repo 裡已經寫好的話。** 「只剩種類」違反 alpha.155
版本說明那句「她寧可說不知道，也不會拿另一個數字來湊」；「只剩一個字」違反
`question::terms` 文件自己的理由「一個字的查詢不是查詢，是掃描」。
新的一條產生候選字的路，要重新對一次**所有**關於候選字的既有承諾——
R2 的派工單沒有列，驗收也就沒有守。

### 25.6 R3 與收貨

grok 4.7（`aece8fa`）：開頭口語的封閉表（只給放寬用，不進 facts 主題那張表，
第一次查詢一個字都不改）、「哪裡」「哪一個」這類整段收、以及候選字的兩道出口
（不到兩個字不放寬、只剩類型詞不放寬）。出口只有一處，`Changed`／`Unchanged`
兩條路都經過。三份驗收（`conversational_relax.rs`、`common_words_relax.rs`、
`trailing_noise_relax.rs`，9／7／9 條）是我寫的，收貨時逐檔比 sha256，和我交出去的
一個位元組都沒變。

**R3 的英文口語開頭借了類型詞那支邊界，兩個洞都是我另寫的問法打出來的。**
`kind_word_match_end` 認複數 s，也把連字號當邊界：「SOS 怎麼回事」的 SOS 被當成
so 加一個 s 切掉；「So-net 帳單怎麼繳」剩「net 帳單」，在有背景的索引上拿任意幾筆
金額來湊。R3 的驗收和 91 句探針都沒有這兩種形狀；我寫了十句沒拿來設計它的
（`/home/ted-h/tmp-tests/review-20260925/bg/x-q.txt`）才看到。收貨 commit
`349ab3c` 另寫一支 `ascii_lead_in_end`：後面只准接空白、中文、`,` `:` `;` `!` `?`
或整句結束，並補一條測試。修完 91 句探針兩臂的輸出和 `aece8fa` 逐行相同。

和 alpha.156 比（91 句 × 兩臂＝182 筆，逐筆機械分類）：108 筆一模一樣；
65 筆從空手變成找得到，其中 64 筆找到的是對的畫面，1 筆是湊來的（25.7）；
8 筆仍然空手，其中 6 筆多了一行「改用」（4 筆是「昨天」開頭，時間範圍擋掉是對的），
1 筆換了改用的字，「退款電話怎麼打」少了原本那行「改用」；剩下 1 筆是
「幫我看一下客服專線」剛裝好那一臂，答案相同、少了一張畫面（25.4）。

grok 的收據說每一臂 91 行 PROBE，它的檔案每臂只有 90 行：每一臂的第一行黏在
cargo 的輸出後面，`^PROBE` 抓不到。內容和我自己跑的那份其餘逐行相同。

突變（`/home/ted-h/tmp-tests/review-20260925/mutations-a157-final.json`，
對 `349ab3c`，對照組 1166 條全綠）：

| 刀 | 切什麼 | 結果 |
|---|---|---|
| M1 | 頭尾表把「幫我看」排到「幫我看一下」前面 | 綠：沒人守（25.4）。收貨補測試後單獨跑那一條：沒切 1 綠，切了 1 紅 |
| M2 | `strip_ascii_edge` 不看邊界 | 紅 |
| M3 | `only_filler` 永遠 false | 紅 4 條 |
| M4 | 拿掉「先看原句開頭」那一段 | 紅 |
| M5 | 把 `question::terms` 過的字交給 peel | 紅（驗收 `conversational_relax.rs`） |
| M6 | 英文口語開頭改回類型詞那支邊界 | 紅 |
| M7 | 英文口語開頭放行連字號 | 紅 |
| M8 | 英文口語開頭不收句讀 | 紅 |
| M9 | 出口 (a) 的門檻從兩個字改成三個字 | 紅 |
| M10 | 口語開頭同一位置取第一個、不取最長 | 綠：表上沒有哪個詞是另一個詞的開頭，哪個方向都不動 |
| M11 | 出口 (b) 的 `&&` 換成 `\|\|` | 紅 4 條 |

突變腳本用 `shutil.copy2` 還原原始碼，連舊的 mtime 一起還原；cargo 因此不重編，
**跑完之後留在 target 目錄的是最後一刀的執行檔**。刀與刀之間不受影響（下一刀寫檔
會給新的 mtime、整個 crate 重編），但我拿那個執行檔去做 flake 對照時差點量到 M11。
腳本已改成還原後把 mtime 設成現在。

第一次跑突變時，我同時在另一個 target 目錄編探針，對照組（沒切的樹）就紅了一條
`reviewer::tests::real_review_pass_refuses_a_fact_swapped_after_prompt_was_built`，
腳本照設計停下。重跑時它在 M6 那一刀又紅了一次（那一輪整支跑了 304 秒，別輪
15–90 秒；M6 切的是英文口語邊界，碰不到 reviewer）。這條會起一支 python 假 CLI
去改 DB 檔。量到的：

- 單獨跑，這一版和 alpha.156 各 20 次，全綠；開 24 個空轉迴圈壓滿 CPU 再各跑 20 次，全綠。
- 兩棵樹的整支 sister-core 同時跑兩輪：第一輪兩邊一起紅同一組三條會起子行程的
  `brain::tests`，這一條沒紅；第二輪全綠。
- 它紅過的兩次都是整支一起跑、而且跑得特別慢的那一輪，都在這一版的樹上；
  alpha.156 的整支我只跑了兩輪。
- 之後為了驗 M1 再開一次對照組，這次紅的是另一條起假 CLI 的
  `wakeup::tests::the_recorder_can_still_write_while_the_slow_path_thinks`
  （「假 CLI 沒進 sleep」），整支跑了 91 秒。當時這台機器上還有別人的工作：
  load average 18–21，`/proc/pressure/io` 的 full avg60 約 43%。所以 M1 改成只跑目標那一條。

會紅的都是起子行程、看時間的測試，而且都紅在 IO 被別人吃滿的時候。
reviewer 那一條另外和別的測試共用 `brain::test_unstopped_data_dir()` 那個全停
admission 目錄，形狀也像「短暫的獨佔鎖被整支的壓力撐開」，**都沒有證實**。這一版沒碰 `reviewer.rs`、`brain.rs`
和全停那一路，所以我把它記成既有的 flake；CI 要是在這一條紅了，重跑那個 job。

### 25.7 代價（量過的）

有背景那一臂（`/home/ted-h/tmp-tests/review-20260925/bg/`），每一筆都會印出「改用」那一句：

- 「到底部設定在哪」：「到底」被當成口語切掉，改用「設定」，5 張不相干的畫面。
  剛裝好那一臂不放寬。
- 「how to show ERR_DEPLOY_42」：改用「show ERR_DEPLOY_42」，0 筆。
  剛裝好那一臂找得到（和 alpha.156 一樣）。
- 「想問題的時候寫的筆記在哪」：「想問」被當成口語，改用「題的時候寫的筆記」；
  探針裡剛好找得到那一張。
- 「客服怎麼打電話」改用「客服」；「電信帳戶」改用「電信帳」。
- 「電信帳單寄到哪」改用「電信帳單」：畫面找對了，但 ★ 答的是帳單金額
  （「帳單」是金額的類型詞），他問的是寄到哪。兩臂都一樣。
- 尾巴只有**從最後往前連續沒看過**才放掉：「部署失敗 退款流程」在有背景時不放寬
  （「流程」看過），剛裝好時改用「部署失敗」。版本說明因此換成「電信帳戶」當例子。

### 25.8 我這一輪做錯的

- R3 派工單把順序寫成「先 `question::terms` 再認口語開頭」，那會先把「到底」的
  「到」吃掉。grok 自己發現、改成先看原句開頭，收據裡有寫。
- 版本說明第一稿有四處「上一版」，講的都是 alpha.155 的事（alpha.156 是重問那一版）；
  還照抄了 alpha.155 那句「既有的答案和排序一個都不動」，而這一版那五句請求正好改到它。
- 清單第一稿叫他「至少再試一個尾巴，只要有『什麼』」——「什麼時候」是類型詞、不切，
  他照做會回報一個假的 bug。定稿排除了它。
- 第一次跑突變時同時編探針，把對照組弄紅（25.6）。

### 25.9 她自己的視窗會被錄進去（沒修，要 Ted 決定）

`research/tech-stack.md` 的設計寫著用 `SetWindowDisplayAffinity(hwnd,
WDA_EXCLUDEFROMCAPTURE)` 把自家 overlay 排除在截圖外，「避免記憶體裡全是自己的
HUD」。**程式裡一次都沒有呼叫它**：整個 repo 沒有 `SetWindowDisplayAffinity`、
沒有 Tauri 的 `contentProtected`；預設 `excluded_apps` 只有密碼管理員與鑰匙圈；
擷取是整個螢幕的 GDI BitBlt。所以她的主視窗和桌寵一出現在畫面上，
**她自己的問答就會被 OCR 進記憶**。這一條沒有在真機器上驗過。

會壞掉的不只是雜訊：

- **出處會被她自己蓋掉。** 她把 0800-000-123 畫在答案裡，那一拍被錄下來，
  「我最後看到的是」就變成她自己的視窗。
- **忘掉不乾淨。** 今天她把昨天的東西畫出來、被錄進今天；之後忘掉昨天，
  今天那幾張還在。

修法是現成的（上面那個 API），但代價要 Ted 決定：她會從**所有**截圖、錄影和
螢幕分享裡消失——包括他回報問題時截的那張圖。

alpha.158 用另一條路修了（在她自己的擷取裡塗掉，見 26.3）；這個 API 仍待 Ted（26.7）。

### 25.10 登記在案、這一版不修的

- alpha.155 版本說明寫「事實、原文、章節三邊**全部空手**才放寬」。產品路徑上章節是
  另外算的，`retrieve_for_question_at` 只在 `TextFactsAndSession` 才把章節算進
  `activities`；那句話不準，**不要再引用**。
- 主題只要有一截看過，沒看過的那一截會被放掉（「火星會議連結」→「會議連結」）。
  從 alpha.155 就是這樣，這一版照實寫進版本說明。
- 不在清單裡、而且尾字常見的請求（「翻一下客服電話」）會改用「一下客服電話」去找。
  答得出來，但那一句很難看。
- **沒有主題的第一次查詢**（「所以呢」「然後呢」「為什麼」「怎麼了」）照原字去找，
  在用過的索引上會回來一堆帶「所以」「為什麼」的畫面。這是第一次查詢的行為，
  alpha.156 就是這樣，這一版刻意不動第一次查詢。
- 問法字後面緊接著看過的動詞（「誰改了月報連結」→「改了月報連結」），在用過的索引上
  找不到。alpha.162 修了（見 30）。
- 「ERR_DEPLOY_42 什麼時候發生的」空手：「什麼時候」是類型詞、不切，那張畫面上
  沒有日期。alpha.161 修了（見 29）。
- 清單的引號閘門（`scripts/check-checklist-quotes-exist.py`）抽不出同一行裡巢狀的
  「「」」。現在有 5 行；把內層換成『』之後閘門多查 3 句、全部找得到，
  所以目前沒有藏著假話。這一版新那格用『』包內層，數字從 189 變 190。
  內層寫 `ERR_DEPLOY_42` 的話閘門對不上產品的 `{x}` 模板而判成找不到，
  所以那一格引的是「客服電話」——`ops.rs` 的測試裡有逐字相同的原文。
- 會起子行程的測試在整支一起跑、機器又慢的時候會紅（25.6）；已知 flake 名單多兩條
  `reviewer::tests::real_review_pass_refuses_a_fact_swapped_after_prompt_was_built`、
  `wakeup::tests::the_recorder_can_still_write_while_the_slow_path_thinks`，成因沒證實。

## 26. alpha.158：她不把自己的視窗錄進記憶（2026-09-25）

25.9 那一格修了，分兩半：她的視窗在前景時整拍不錄（26.2）；看得見、但前景是別的
程式時，Windows 上她那一塊塗黑（26.3）。`WDA_EXCLUDEFROMCAPTURE` 那條路沒有走，
仍待 Ted 決定（26.7）。

### 26.1 alpha.157 的出貨收據

tag `v0.1.0-alpha.157`（`6d76acf`）一次就過：八個 job 全綠，release 於
**2026-09-25T14:16:29Z** 發出，四個 asset、prerelease，body 前綴和本機
`release-notes.sh` 產的一致。

打 tag 前在 detached worktree 跑 `/home/ted-h/tmp-tests/gates-all.sh`，第一次有四條紅在
「找不到 `./target/debug/sister`」。那支工具說它接受 `CARGO_TARGET_DIR`，只有 cargo
那半是真的：`check-erased-db.sh`、`check-readme-quickstart.sh`、
`check-recall-baseline.py`、`check-moment-baseline.py` 寫死 repo 裡的 `target`。
工具改成把 binary 路徑一起傳過去（`check-erased-db.sh` 用暫時的 symlink，路徑上
已經有東西就拒絕），重跑 53/53。

### 26.2 前景那一格（R1）

grok 4.7（`90633e2`）。

- `PrivacyConfig::check` 在敏感欄之前先認她：`OWN_APP_KEYS` 是三個平台的
  `app_key()`（`sister-desktop.exe`、`sister-desktop`、`com.ted-h.ai-sister`），轉小寫後
  整串相等。回 `Exclusion::OwnWindow`，`is_blocked()` 為真。設定檔裡沒有開關，
  `excluded_apps` 清空也擋。
- recorder 把排除分支抽成 `hold_blocked_tick`，兩條路共用：清 dedup／OCR baseline、
  推剪貼簿水位、開空洞旗標、輸入節奏照記。差在後面：她的 tick 不寫 excluded 稽核、
  不算 `excluded`，`last_exclusion` 照放行那樣清掉（所以夾在她前後的兩段 KeePassXC
  仍是兩列稽核）。
- 剪貼簿來源是她的程式檔，算進新的 `clipboard_source_own_window`，不算排除。
- `sister record` 收尾多兩行：「她自己的視窗在前景 N 拍，那幾拍沒有錄。」
  「剪貼簿丟棄 N 筆：從她自己的視窗複製的。」

### 26.3 看得見、不在前景那一格（R2）

**R1 收貨時我才看到，它擋的不是平常那一格。** 她的主視窗就是桌寵
（`tauri.conf.json` 的 `pet`：340×560、透明、`alwaysOnTop`），答案回來之後一直掛在
那裡。使用者看完答案點回記事本，下一拍前景是記事本，R1 放行，整個螢幕連同她的
答案一起被截走。

R2 派給 grok 4.7 時回 HTTP 402（Grok Build 額度用完），這一半是我自己寫的。

- 判斷全在 `crates/sister-capture/src/own_windows.rs`（純函式，Linux 的 `cargo test`
  驗得到）。`crates/sister-capture/src/windows/own_windows.rs` 只讀事實，而且不准呼叫
  會送視窗訊息的 API：recorder 可能跑在她自己的行程裡，送訊息給自己的 UI 執行緒
  會卡死。
- 每一拍抓圖前後各列一次視窗（`grab_without_her`），兩次讀到的她都塗黑，塗完才算
  dhash、跑 OCR、存圖。任一次列不出來，或她的一扇看得見卻讀不到位置，這一拍回錯誤、
  整張不抓，算進 `sister record` 摘要「⚠ N 拍失敗」那一行。
- 她那一扇：`GetWindowRect` 和 DWM 外框（`DWMWA_EXTENDED_FRAME_BOUNDS`）兩塊都塗；
  被 DWM 藏起來、透明、形狀不規則都照塗。
- 別人的視窗要**確定蓋得住**才擋她：讀得到、而且沒被 DWM 藏起來，不透明，不是
  `SetWindowRgn` 的不規則形狀，而且只拿 DWM 外框算（`GetWindowRect` 多出來的那圈
  縮放邊底下看得到她）。不透明是指：沒有 `WS_EX_NOREDIRECTIONBITMAP`（DirectComposition
  畫的，從外面讀不出透明度），沒有 `WS_EX_TRANSPARENT`（點得穿的覆蓋層），layered 的
  話要讀得到屬性、沒有色鍵、alpha 沒設或設成 255。
- 畫面縮過圖時，每一塊往外取整、各邊多塗一格。
- `sister bench` 的 `bench_grab` 不塗：它量的是抓圖本身。
- 只有 Windows。Linux X11 抓的是整個 root window，不塗。（alpha.159 補上，見第 27 節。）

### 26.4 收貨

**R1。** 驗收在派工前寫好：`crates/sister-core/tests/own_window_gate.rs`（7 條，閘門）
與 `crates/sister-capture/tests/own_window_acceptance.rs`（1 條，整段錄製加位元組
掃描）。交貨後直接跑，綠。收貨改了兩處：

1. **⚠ 在混合那一場說錯話。** 一場只有排除規則和她的視窗、一張畫面都沒留下時，
   ⚠ 寫「全部被上面的規則擋掉了」，而上一行剛寫完她的視窗，改 config 改不到她。
   改成「她自己的視窗那幾拍以外」，補一條測試（`25d0c77`）。delegate 在條件上
   多加的 `!excluded_reasons.is_empty()` 和 `excluded > 0` 等價，拿掉。
2. **我的驗收讓兩層互相補票。** 三筆剪貼簿的擁有者都寫成她的程式檔，所以水位那一層
   和來源閘門那一層，拿掉任一層都不紅。加了兩筆擁有者是 `msedgewebview2.exe` 的：
   一筆在她前景那一拍（守水位），一筆在她最後一個 tick 之後、下一個一般 tick 之前
   （只剩空洞旗標擋得住，`87e4d65`）。WebView2 裡複製的字，真機器上擁有者報的是哪個
   程式，沒看過。

R1 的突變 22 刀（對 `87e4d65`，對照組前後各跑一次、全綠），**22 刀全紅**：

| 範圍 | 刀 |
|---|---|
| 閘門 | 拿掉她的早退；她排到敏感欄後面；整串相等改子字串；剪貼簿來源不轉小寫；`reason()` 對她回 `None`；剪貼簿來源拿掉她；`is_blocked` 對她回 false |
| recorder | 不清 `last_exclusion`；寫 excluded 稽核；算進 `excluded`；算進 `excluded_reasons`；不數 `own_window`；不推水位也不開空洞；推水位但不開空洞；只把 Blocked 送進排除分支；她的剪貼簿照收；她的剪貼簿算成排除 |
| 摘要 | ⚠ 也算她；拿掉她那一行；⚠ 兩種說法對調；她的 tick 每拍都叫腦；拿掉剪貼簿她那一行 |

**R2 純規則。** `crates/sister-capture/tests/own_windows_mask.rs`（36 條，平台層動工前
先寫）：規則逐格對照、隨機 1500 組視窗、隨機縮圖畫面、抓圖前後各問一次。突變 24 刀
（對 `40ba35c`），**24 刀全紅**：別人擋不住她、擋她用 `window_rect`、讀不到有沒有藏
當沒藏、她透明就不塗、縮圖不多塗一格、右邊往內取整、讀不到位置就跳過那一扇、名字
不轉小寫、重疊重算、不先切到擷取範圍、layered 沒設 alpha 當透明、沒畫出來的別人也算擋、
只塗 `frame_bounds`、長度不對照塗、形狀不規則的別人也算擋、抓完不再問、抓之前不問、
兩次都在抓之前問、抓完問不出來也留著、抓之前問不出來也照抓、大小對不上也留著、
DirectComposition 當實心、點得穿當實心、不看 layered 那張表。

**R2 真的 Windows 桌面。** `crates/sister-capture/tests/windows_own_windows.rs`，CI 那一步
叫「Own windows — masked in her own capture on a real desktop」。它把自己的測試程式
複製幾份當視窗：改名成 `sister-desktop.exe` 的是她，別的名字是別人；她畫洋紅，別人
畫綠或青。五個情境：

1. 不是她的洋紅視窗照樣看得到（對照組：塗黑不是整片亂塗）。
2. 她那一塊全黑，外圍一圈沒有洋紅（連抓三張）。
3. 實心的別人蓋在她上面：重疊那一塊是綠的，她其餘的地方是黑的。
4. alpha 128 的別人蓋在她上面：她整塊是黑的。
5. 沒有內容的 `WS_EX_NOREDIRECTIONBITMAP` 別人蓋在她上面：她整塊是黑的（連抓三張）。

CI 的桌面是 1024×768、不縮放，所以縮放那一格只在清單上。平台層的突變在另一條只跑
這支測試的暫時分支上跑（對 `40ba35c`；對照組前後各一次，都綠），**九刀全紅**，
每一刀都紅在它該紅的那個情境：

| 刀 | 切什麼 | 紅在 |
|---|---|---|
| W01 | 不問她在哪（永遠回空） | 情境 2：她 38400 格全是洋紅 |
| W02 | 程式檔拿完整路徑、不取檔名 | 情境 2：同上 |
| W03 | cloaked 讀反 | 情境 3：重疊那一塊被塗黑 |
| W04 | 永遠不是 layered | 情境 4：重疊那 14400 格是混色，看得到她 |
| W05 | 不讀 alpha | 情境 4：同上 |
| W06 | 讀不到 DWM 外框 | 情境 3：重疊那一塊被塗黑 |
| W07 | 形狀讀反 | 情境 3：同上 |
| W08 | 讀到的視窗範圍左邊偏一格（擷取範圍不動） | 情境 2：洋紅漏出 160 格 |
| W09 | 不讀 `WS_EX_NOREDIRECTIONBITMAP` | 情境 5：重疊那 14400 格全是洋紅 |

**同一份腳本在 alpha.157 和這一版的差別。** 腳本是記事本一張（`客服專線 0800-080-123`）、
她的答案畫面兩張，問「客服電話」：

```
alpha.157  🔍 「客服電話」 1 筆答案、2 筆原文
             ★ +886800080123  「0800-080-123」
               ↳ phone · … · sister-desktop.exe · AI-Sister · frame #3
           兩筆原文都是她自己的畫面（frame #2、#3）
這一版     🔍 「客服電話」 1 筆答案、0 筆原文
             ★ +886800080123  「0800-080-123」
               ↳ phone · … · notepad.exe · 帳單.txt - 記事本 · frame #1
```

這份腳本走的是 `sister replay`，只驗得到前景那一格；塗黑那一格只有上面那支 Windows
測試和清單驗得到。

### 26.5 我這一輪做錯的

- **我把前景那道閘門當成整件事。** 25.9 寫的是「她的主視窗和桌寵一出現在畫面上」，
  派給 grok 的 R1 卻只擋前景（26.3 開頭）。
- 派工單的行號錯了幾處，grok 在收據裡逐條指出（剪貼簿閘門從 2331 行開始，不是 2334）。
- 派工單說她前景那一拍複製的剪貼簿「算 own window」，那筆其實走不到來源閘門：
  水位先把它跳過了。grok 兩條路都測了，也照實寫了。
- 驗收第一版三筆剪貼簿都寫她的程式檔（26.4 R1 的 2）。
- **判斷「看不看得穿」的第一版只看 layered。** DirectComposition 畫的和點得穿的覆蓋層
  都被當成實心，蓋在她上面的那一塊就不塗。`40ba35c` 補上，出貨前。把這一格拿掉的
  W09 在真的 Windows 上量到：沒有內容的那種視窗底下，她 14400 格洋紅全看得到。
- 「抓之前問不出來也照抓」那一刀一開始是綠的：夾具讓兩次都問不出來，所以只證明了
  「至少一次問不出來就不抓」，刀切掉抓之前那一問，還是紅在抓之後那一問。改成只讓
  第一次失敗（`6eec86b`）。
- Windows 突變第一輪的 W08 是綠的。我把偏一格切在共用的換算函式 `desktop_rect` 上，
  擷取的螢幕範圍也跟著偏；範圍偏了，畫面和範圍不再一樣大，縮圖那條「各邊多塗一格」
  就把那一格補回來。第二輪改成只偏讀到的視窗範圍、不動擷取範圍，才紅。**切在共用
  換算上的刀會自己補償，要切在其中一個呼叫端。**

### 26.6 登記在案、這一版不修的

- 認的是程式檔名。安裝版的檔名是固定的；免安裝版改了名（瀏覽器重複下載會變成
  `sister-desktop (1).exe`）她就認不得，兩格都擋不到。
- Linux 讀不到 `/proc/<pid>/exe` 時退回 WM_CLASS、macOS 讀不到 bundle id 時退回
  localizedName；那兩個退路的值會不會等於三個名字之一，沒在真機器上看過。Linux 和
  macOS 的剪貼簿後端本來就沒有來源 app。
- Linux X11 不塗：她看得見但不在前景時，照樣進 frame。（alpha.159 補上，見第 27 節。）
- 全新的資料庫只跟她說過話時，她說「我正開著，可是到現在一列內容都還沒落地——多半是
  剛開始，再等一下。」——等下去沒用，要切到別的 app。
- 塗的是她整扇視窗的範圍。桌寵四周透明、看得到底下程式的地方也一起塗，被她蓋住的
  那一小塊那一拍不記。
- 她被 DWM 藏起來也照塗（理由寫在 `her_cloaked_window_is_masked_anyway`）。她開在另一個
  虛擬桌面時，會不會在這一個桌面的同一個位置留一塊黑的，要看 DWM 那時有沒有把她藏起來，
  沒在真機器上看過。
- 沒驗過的兩類，都是「擁有者不是她的程式檔」：WebView2 的下拉選單、右鍵選單與提示框
  如果是 `msedgewebview2.exe` 開的頂層視窗，她認不得；工作列、Alt-Tab、工作檢視裡
  她的縮圖是 explorer／DWM 畫的，這一層不塗。
- `DwmExtendFrameIntoClientArea` 延伸出來的玻璃、Mica／Acrylic 背景的視窗，這一層判成
  實心；蓋在她上面時，那一塊不塗。透過去看不看得到她的字，沒量過。
- 列完視窗到讀位置之間她的一扇剛好關掉，會讀不到位置，那一拍整張不抓。

### 26.7 還沒做、要 Ted 決定：`WDA_EXCLUDEFROMCAPTURE`

`SetWindowDisplayAffinity(hwnd, WDA_EXCLUDEFROMCAPTURE)` 讓她從**所有**截圖、錄影、
螢幕分享裡消失，包括 Ted 回報問題時截的圖。好處是 BitBlt 直接抓得到她底下的東西，
沒有黑框，也不用列視窗。這一版的塗黑只影響她自己的記憶，別人的截圖照樣看得到她。
要不要換、或兩個都要，是 Ted 的決定。

## 27. alpha.159：Linux X11 也塗她的視窗（2026-09-25）

26.6 的「Linux X11 不塗」補上。順手補 alpha.158 前景閘門在 Linux 上的一個洞：套件
升級會在她開著的時候換掉程式檔，`/proc/<pid>/exe` 讀出來變成
`sister-desktop (deleted)`，她重開之前前景閘門認不得她。grok 回 402、codex 沒額度，
這一版是我自己寫的。

### 27.1 alpha.158 的出貨收據

tag `v0.1.0-alpha.158`（`d95aea8`）一次就過：八個 job 全綠，release 於
**2026-09-25T16:46:19Z** 發出，四個 asset、prerelease，body 前綴和本機
`release-notes.sh` 產的一致。

### 27.2 怎麼認、怎麼塗

判斷和塗黑跟 Windows 共用 `crates/sister-capture/src/own_windows.rs`（`own_parts`、
`grab_without_her`：抓圖前後各列一次）。`crates/sister-capture/src/linux/own_windows.rs`
只讀事實、只送查詢，不改任何視窗。

- **列哪些。** root 底下直接那一層（QueryTree 由下往上排，反過來交出去），只收
  VIEWABLE、不是 InputOnly 的。沒 map 的（多半是最小化或在別的工作區）已經濾掉，所以
  `minimized`、`cloaked` 填固定值。
- **是誰的。** 每一扇頂層往下問到掛 `WM_STATE` 的程式視窗為止（`FAMILY_DEPTH` = 3：
  框→程式，KWin 是框→包裝→程式），每一扇用 X-Resource 的 `QueryClientIds`
  （LOCAL_CLIENT_PID）問 X server 是哪個行程建的，再讀 `/proc/<pid>/exe` 的檔名。
  同一家任何一扇是她的，整個頂層算她的，視窗管理員畫的框和標題列一起塗。
  `_NET_WM_PID` 不用：程式自己填，也不是每個程式都填（xmessage 沒有，在 Xvfb 上用
  `xprop` 看過）。
- **看不看得穿。** 同一家任何一扇是 32 位元色深、`_NET_WM_WINDOW_OPACITY` 有掛而且
  讀出來不是 `0xFFFFFFFF`（型別不對讀不懂的也算）、bounding 或 clip 形狀不是方的，
  就不拿它擋她。範圍是頂層的外框（含邊框寬度）。
- **問不出來。** 任何一個查詢出錯（例如列完之後一扇剛好關掉）整拍不抓。SHAPE 和
  X-Resource 是必要的，X server 沒有就每一拍都失敗，錯誤訊息寫是哪一個。
- **前景閘門。** app key 改成走同一支 `process_file_name`：讀 `/proc/<pid>/exe`、
  去掉結尾的 ` (deleted)`。

設計第一版還有一條「問不出擁有者就當看得穿」，要防的是合成管理員的 overlay window。
在 Xvfb 上量過：那扇不在 QueryTree 裡，X server 自己的資源 XRes 回的是 X server 的 pid。
要防的東西不存在，落地前拿掉。

### 27.3 收貨

**Xvfb 上的 10 條**（`linux::own_windows::tests`）。「她」是改名成 `sister-desktop` 的
xmessage，別人的視窗和視窗管理員由測試自己扮（開框、搬進去、掛 `WM_STATE`），期望
的範圍一律由測試自己從 GetGeometry 算。情境：她整塊黑、別處不黑；實心的別人蓋在她
上面；她整個在別人後面；八種看得穿的別人和一條實心對照組（其中三種是框實心、框裡
的程式看得穿）；InputOnly 和沒 map 的別人；她沒 map；框裡的她（框→她、框→包裝→她、
框→沒掛 `WM_STATE` 的她）；她掛著別人的 `_NET_WM_PID`；程式檔被換掉之後（連前景閘門一起斷言）；
她被抬到別人上面。CI 新一步「Own windows — masked in her own X11 capture」設了
`AI_SISTER_REQUIRE_X11_OWN_WINDOWS=1`：少了 Xvfb 或 xmessage 就紅，而且要求至少一條
passed。

**突變 25 刀**（對 `3fe68b4`；對照組先跑一次，10 條全綠），24 刀紅：

| 範圍 | 刀 |
|---|---|
| 擷取 | 不塗；範圍少一格 |
| 列視窗 | 不反轉；不看 viewable；不看 InputOnly |
| 往下找 | 只問頂層；只往下一層；往下找到的不收進來 |
| 看得穿 | 不看色深、opacity、bounding、clip（各一刀）；沒掛 opacity 也當半透明；opacity 只要不是 0 就當實心；讀不懂當實心；opacity、色深、形狀只看頂層（各一刀） |
| 認人 | 認不出行程；只看第一個名字（框的主人）；不拿掉 ` (deleted)`；前景閘門不走 `process_file_name` |
| 範圍 | 外框不含邊框；位置取最後一扇 |

M20（程式視窗裡面也往下找）綠。同一家多問一扇，只可能讓「是她」「看得穿」
「不是方的」從否變是，也就是只會多塗、不會少塗；夾具裡沒有能分辨的情境。沒補。

**連跑。** 平行 20 次、單執行緒一次，全綠。

**真的裝好的她。** `the_installed_app_is_all_hers`（`#[ignore]`）接在 .deb 那一步：裝完、
真的 `sister-desktop` 開起來之後，用產品不讀的 `_NET_WM_PID` 對上開她時的 pid 找出她的
視窗，斷言產品要塗的範圍整扇蓋住，並印出畫面上每一扇頂層視窗和產品讀到的事實。
本機拿替身跑過三臂：改名的替身（綠）、沒改名的 xmessage（紅，2490/2490 像素不在範圍裡）、
沒有任何視窗掛這個 pid（三十秒後紅，先印清單）。CI 上（branch `7e38634`）畫面上只有一扇：
340×560（`pet` 的大小）、32 位元色深，`_NET_WM_PID` 對得上，X server 認的程式也是
`sister-desktop`，要塗的範圍整扇蓋住。那一步設了 `WEBKIT_DISABLE_COMPOSITING_MODE=1`，
所以「畫面上沒有 WebKit 自己開的視窗」只對那個設定成立（27.5 最後一條）。

### 27.4 我這一輪做錯的

- **第一次全綠是運氣。** 平行的測試各自複製 xmessage 當她、立刻執行；別的執行緒 fork
  出去的子行程還握著剛寫完的那個檔案，exec 回 `Text file busy`。突變腳本要求對照組先
  綠才切刀，對照組紅了兩條才看到。改成遇到 ETXTBSY 重試（`3fe68b4`）。
- **alpha.158 的 R1 收貨沒想到程式檔會在她開著的時候被換掉。** 套件升級就是這樣，
  Linux 上前景閘門因此認不得她。
- 模組說明寫了一句沒驗過的「GTK 的彈出選單根本沒掛 `_NET_WM_PID`」，換成驗過的理由
  （`8a5d8fa`）。
- 那條「問不出擁有者就當看得穿」（27.2 最後一段）是先寫規則、後量前提。
- **`docs/PRIVACY.md` 總表「她自己的視窗」那一列還寫著「（Windows）」，53 條閘門
  全綠之後我才看到。** 列舉平台的句子只會少一項、不會錯，沒有閘門抓得到。
  補成「Windows、Linux X11 預覽版」。

### 27.5 登記在案、這一版不修的

- 合成管理員自己的規則（「沒在用的視窗一律半透明」、開關視窗的動畫）從視窗上讀不到：
  那種視窗蓋在她上面時照實心算，透出來的她不塗。
- 合成管理員畫的東西不是視窗：工作區總覽、切換視窗時的縮圖裡的她不塗。
- 程式自己用 Composite 把視窗畫到別處（手動 redirect）的，照「畫在原位」算。
- 一個框裝好幾個程式的分頁式外框，框裡任何一扇是她，整個框都塗。
- 框和程式視窗之間隔超過兩層就認不得。
- 真的桌面上（有視窗管理員、有合成管理員、幾十扇視窗）每一拍多花多少時間，沒量過；
  測試都在 Xvfb 上，視窗管理員是測試自己扮的。
- CI 開她時設了 `WEBKIT_DISABLE_COMPOSITING_MODE=1`（最早加 Linux 那一版就有，沒留理由）。
  真的使用者沒設；那時 WebKit 的網頁行程會不會自己在畫面上開 X 視窗，沒看過。

## 28. alpha.160：一直待在她的視窗上問（2026-09-25）

26.6 登記的那一條補上：全新的資料庫只跟她說過話時，她說「再等一下」，而等下去沒用。
grok 回 402、codex 沒額度，這一版也是我自己寫的。

### 28.1 alpha.159 的出貨收據

tag `v0.1.0-alpha.159`（`1278af4`）第一次紅在 Windows：UIA 那一步 3 分鐘逾時，其餘
五個 job 全綠。同一個 sha 在 main 上那一趟（`36164530354`）UIA 只花 44 秒；tag 那一趟
要從頭編 `windows_uia`，光編譯就 2 分 18 秒，逾時量到的是編譯，不是測試。
`rerun --failed` 第二次 UIA 1 分 45 秒過，Windows、Release、Website 全綠，release 於
**2026-09-25T18:14:15Z** 發出，四個 asset、prerelease，body 前綴和本機
`release-notes.sh` 產的一致。`32356a2` 讓 CI 先 `--no-run` 編好兩支原生擷取測試，
3 分鐘只量測試本身。

### 28.2 怎麼知道前景是她

- 錄製迴圈每一拍把結果交給 `RecordingBeat::saw`。只有 `Tick::OwnWindow` 算「認出
  前景是她」，其餘 13 種和失敗的那一拍都不算（`her_window_in_front`，窮舉、沒有 `_`）。
- 心跳每 5 秒蓋一次：上一拍是她就寫 `<時戳> own`，不是就寫裸數字，和舊版一模一樣。
  舊版讀到認不得的第二欄一律當「在錄」，所以舊的字母人和舊的 `sister` 讀新檔案不會
  讀錯；測試抄了一份 alpha.159 的 `parse_record` 釘住這件事。
- `heartbeat::phase_seeing` 同一次讀檔回 phase 和前景。前景只跟著新鮮的 `Recording`
  心跳走，開機、過期、收工、讀不懂都回不出 `true`。`BlindSpots` 多一格
  `her_window_in_front`（newtype `HerWindowInFront`），桌面的 `Blind` DTO 跟著多一格。

### 28.3 句子排在哪

- 整顆資料庫一段字、一張畫面都沒有，她正開著、上一拍前景是她：「她正開著，但手上
  一段字都沒有——她上一次看的時候，前景是她自己的視窗，而她不錄自己。切到你要她記的
  程式，她才會開始記。」排在「剛開始，再等一下」和「被忘掉了」前面。
- 被擋過（暫停、排除、全停）的，標題照舊是「她錄過，但那段時間一張畫面都沒留下來
  ——底下是查得出來的原因。」，前景那件排在原因的第一行。
- CLI（`query::blind_lines_for`）和桌寵（`app.js` 的 `blindLines`）同一個順序，句子
  只差人稱；`check-pet-says-why` 的 A160.4 拿 ops.rs 原始碼比對兩句。

### 28.4 收貨

**突變 30 刀**（對 `d7d0b29`；三個 runner 各先跑一次沒切的對照組，core 68、cli 113、
pet 1000 條全綠），27 刀紅、3 刀照預期綠：

| 範圍 | 刀 |
|---|---|
| 心跳檔 | 有她也寫裸數字；沒有她也寫 `own`；讀不出 `own`；認不得的第二欄當成她；開機那一段也說是她；線上格式換一個字；不帶前景的 `beat()` 也寫 `own` |
| `phase_seeing` | 一律說是她；過期／收工也回 Recording |
| `BlindSpots` | 那一格寫死 false；沒有心跳也說是她；`recording_now` 讀成開機 |
| 錄製迴圈 | 認不出她的視窗；閒著也算她；失敗的那一拍算她；主迴圈不記前景；心跳不帶前景；認出一次就一直是她 |
| CLI | 不講這一句；排到被擋過後面；不看前景；句子少了人稱以外的字 |
| 桌寵 | 不講這一句；讀錯欄位名；句子漂移；排到被擋過後面；不看前景 |

照預期綠的三刀：`phase_seeing` 裡「想最後一段／收工／讀不懂」那兩臂改成 `true`
（`phase_of` 先回 `None`，走不到）；CLI 條件拿掉 `recording_now`（前景只跟著新鮮的
Recording 心跳走，產品生不出「前景是她、沒在錄」）。

改了順序之後（28.5），對 `d833513` 補 11 刀，全紅：標題那一句不講；前景那句搶回
標題；被擋過時不補那一行、一律補那一行；桌寵那一行排到標題前面；補的那一行句子漂移
（CLI 同時紅在 A160.4）。上面那兩刀「排到被擋過後面」在新順序下就是正確答案，不再算刀。

桌面 DTO 那一格只有 macOS CI 跑得到。

### 28.5 我這一輪做錯的

- **第一版把前景那句排在「被擋過」前面**，理由是「那一句講過去，這一句講現在」。
  沒算到從她的視窗上問的時候，前景幾乎一定是她自己：一小時網銀全被排除、回到她這裡
  問的人，標題會讀成「是因為前景是她」。突變跑到一半、我起草版本說明時重讀才看到。
  突變守的是我寫下的順序，不是那個順序對不對。

### 28.6 登記在案、這一版不修的

- 「上一次看的時候」最多是 5 秒前（心跳的間隔）。
- 忘掉全部、或資料全過了保留期之後一直待在她視窗上問的人，現在聽到的是前景那句，
  不再列「也可能是之前的被忘掉了或過期了」。要分得出來，資料庫得另外記「真的留過
  畫面或字」：`ever_stored` 分不出來，她視窗裡打的字也會記節奏。
- `sister facts`／`sister stats` 空資料庫時那句「剛開始的話再等一下」沒接這一格。它們在
  終端機裡跑，前景是終端機，她照錄。
- 桌寵只讀預設資料目錄（`launch_intent` 只分登入啟動和手動開），所以 Windows 上要驗
  這一句得用一個沒裝過的使用者帳號。

### 28.7 順手修的

- `docs/WINDOWS-CHECKLIST.md` 有三條叫人用 `--data-dir` 開字母人（stop-all 那條、
  「還沒記過任何東西」那條、日期軌「一天都沒有。」的對照組），那條路不存在，改成
  用沒裝過的 Windows 使用者帳號。
- 同一份清單 alpha.29 那條（清掉當天、按開始記錄、三五秒內問）期待「可能是剛開始，
  也可能是之前的被忘掉了或過期了」。這一版起，第一拍心跳是裸的、五秒後才帶 `own`，
  所以超過五秒還待在她的視窗上問，聽到的是 28.3 那一句；那一條補上這件事。
- CI：alpha.159 的 tag 在 `UIA — read WPF and Edge visible paragraphs` 逾時。那一步
  的 3 分鐘連編譯一起算，tag 的快取是冷的（版號一改 Cargo.lock 就換 key），光編譯就
  2 分 18 秒；同一個 commit 在 main 上整步 44 秒。往前 30 次 run 裡，這一步成功的
  24 次是 44–146 秒。UIA 和 Own windows 前面加一步 `--no-run` 先編好，3 分鐘只量
  測試本身。

## 29. alpha.161：問一件事「什麼時候」，她答她看到的時候（2026-09-25）

25.10 登記的那一條補上：「ERR_DEPLOY_42 什麼時候發生的」空手。grok 回 402、codex
沒額度，這一版也是我自己寫的。

### 29.1 alpha.160 的出貨收據

tag `v0.1.0-alpha.160`（`4cd9591`）一次全綠：八個 job 都成功，release 於
**2026-09-25T19:21:30Z** 發出，四個 asset、prerelease，body 前綴和本機
`release-notes.sh` 產的一致。`32356a2` 加的 `--no-run` 那一步在 tag 那一趟 47 秒，
之後 UIA 37 秒過；上一版的 tag 在同一步 3 分鐘逾時。

### 29.2 放寬切什麼、講什麼

- 問時間的說法另成一張表 `facts::WHEN_ASKS`（什麼時候／甚麼時候／什麽時候／什么时候／
  何時／何时／幾點／几点／when）。它們仍是 `DateTimeMention` 的類型詞：
  `query_kind_words()` 把兩張表接起來，`kinds_for_query`、`condition_is_kind_word`、
  `strip_kind_words` 都走它。
- `cut_question_words` 碰到問時間的字不先問「是不是類型詞」，照樣切；「多少錢」那幾個
  照舊跳過。所以放寬的候選字裡不會再有問時間的字。
- 放寬之後有東西、而且原句 `asks_when`，用 `SearchAdjustment::RelaxedWhen`：Relaxed
  那一句再加「底下每一筆的時間，是我記下那一筆的時候。」空手就是 Relaxed。
- `question::longest` 把問時間的字當一個內容詞，「什麼時候看到部署失敗」不再被剝成
  「時候看到部署失敗」。

### 29.3 第一次查詢

- `topic_constraint` 每段主題過 `trim_topic_joint`：開頭只剝「的」，尾巴剝虛字
  （`question::trim_trailing_filler`），剝完不足兩字就不剝。「客服的電話」「週會是
  什麼時候」「週會的時間」第一次就對得到。
- `fact_request`：沒有主題時，問時間的字不要日期，「幾點了」「電話什麼時候打」不再拿
  任意一個日期來答；有主題照舊。`answers_during` 改吃它，放寬的兩道拒絕仍用
  `kinds_for_query`。

### 29.4 桌面

- `app.js` 的 `searchedNote` 每種原因一句，和 Rust 逐字相同；認不得的種類不畫。
  `check-pet-says-why` 從 `retrieval.rs` 抽 variant 和句子來比，前提斷言至少三種。
- 開發用 `?glued=` 示範從 alpha.155 起把字串當陣列，畫出「改用「undefined」」；改成
  產品的形狀，pet 閘門兩個示範各驗一次。

### 29.5 收貨

- `when_questions.rs` 11 條。夾具是 recall-baseline 語料加三張畫面，每條在有、沒有
  問時間背景（12 句「什麼時候」「幾點」「when」）兩邊各跑，背景真的被看過有前提斷言。
- `recall-phrasing` 加 3 題（客服的電話、ERR_DEPLOY_42 什麼時候發生的、部署失敗是
  什麼時候）。alpha.160 的執行檔跑新題 9 格全 ✗，這一版全 ✓。
- 清單引號閘門 190 → 193，「改用」那一句改一個字就紅。

**突變 27 刀全紅**（對 `935a163`；控制組 core 1002、cli 1、pet 1015、recall 6 條全綠；
core 跳過已知 flake 的 `wakeup::tests::` 與 `reviewer::tests::real_review_pass`）：

| 範圍 | 刀 |
|---|---|
| 放寬 | 空手也講底下的時間；facts 和原文都要有才講；看候選字不看原句；呼叫端一律 Relaxed；問時間的字也當類型詞跳過；放寬找不到問時間的字；類型詞一律不保護（錢也切） |
| 表 | `asks_when` 一律 false；`asks_when` 用子字串；類型詞不含問時間；主題剝類型詞只讀舊表；條件是不是類型詞只讀舊表；有主題也不收問時間 |
| 第一次查詢 | 沒主題也撈日期（定義端、呼叫端各一刀）；主題不修邊（recall 那 3 題也紅）；開頭的「的」不剝；「的」剝到不足兩字；尾巴修邊也剝開頭；「什麼時候」拆成虛字加時候 |
| 句子 | Rust 那一句漂移（core、cli、pet 三處都紅）；種類名換寫法；桌面那一句漂移；桌面回到兩臂三元；認不得的種類講籠統那一句；認不得的種類畫一行空的；示範給字串 |

「條件是不是類型詞只讀舊表」只紅在 alpha.157 那條「什麼時候的雙字算類型詞」。產品上
問時間的字進不了 `indexed_candidate`（`cut_question_words` 先切掉），那一行守的是兩張表
一致。

### 29.6 我這一輪做錯的

- 版本說明第一稿說「週會幾點」和「週會是什麼時候」以前空手「是同一件事」。探針說不是：
  alpha.160 沒有背景時「週會幾點」放寬成「週會」、給得出那張畫面，只是沒有日期；有背景
  才空手。改成只講「這一版也答得出來」。
- 清單第一稿寫「找到的都要是記事本那一張」。他在終端機打的問句也會被錄進去，問第二次
  就不只一張；改成和只打那個詞比，跟 alpha.157 那一節同一種寫法。那一格的「改用」句子
  原本沒有宣告動詞，引號閘門不看；改成「會說」加『』才算進去。
- 「認不得的種類不出聲」第一版只斷言沒有那幾個字。拿掉 `if (!said) continue;`、多畫一行
  空的，舊斷言 1013 條全綠；改成和沒改字的同一題逐字相同之後才紅。
- 第一次跑突變，控制組紅在兩條 `wakeup` 子行程測試上（冷編譯的負載）。我的腳本又把
  cargo 的 `error: test failed` 讀成編不過，而且少了 `--no-fail-fast`，lib 一紅就沒跑到
  整合測試。

### 29.7 登記在案、這一版不修的

- 還沒到的事、畫面上又沒寫日期（「預算表什麼時候交」）：給的是看到那張畫面的時間。
  版本說明寫了，`when_questions.rs` 釘著。
- 「到期日是什麼時候」對的是「期日」（`question::terms` 本來就剝開頭的「到」），「星期日」
  旁邊的日期也對得到。以前主題是「期日是」、什麼都對不到。版本說明寫了，單元和端到端
  各釘一條。
- 「幾點」一律算在問時間：「會議紀錄的幾點結論」也是。
- 英文 when is 句型：「when is the release candidate」改用「is the release candidate」，
  找不到。alpha.162 起 the 不再是條件，這一題找得到；is 還是（見 30）。
- 主題只看過一截的（「火星會議什麼時候」）改用「會議」去找，給的是週會那張畫面，
  「改用『會議』」和「底下每一筆的時間」兩句都在。alpha.160 在這一題更糟：改用「會議
  什麼時候」，把週會的 9/30、14:00 當成答案（有沒有背景都一樣）。同 25.10 第二條。
- 「部署會議什麼時候」（兩截都看過、合起來沒看過）以前 `searched=null` 空手，這一版多印
  「改用『部署會議』」再空手。
- 已知 flake 多一條 `wakeup::tests::a_stuck_cli_does_not_stall_the_record_loop`（冷編譯時
  和 `the_recorder_can_still_write_while_the_slow_path_thinks` 一起紅過一次）。

## 30. alpha.162：問句中間有「的」「了」，她切開再找（2026-09-25）

25.10 登記的「誰改了月報連結」和 29.7 的英文 when is，這一版補上。grok 回 402、codex 沒額度，這一版也是我自己寫的。

### 30.1 alpha.161 的出貨收據

tag `v0.1.0-alpha.161` 打在 `77ace3b`（`c7a2a9a` 的版號加上 alpha.160 的收據）。tag 那一趟
（36186761914）八個 job 都成功，release 於 **2026-09-25T21:25:25Z** 發出，四個 asset、
prerelease，body 前綴和本機 `release-notes.sh` 產的一致。`c7a2a9a` 在分支上那一趟
（36177827014）Windows 紅兩次，兩次都是 `native_edge_pdf_scroll_keeps_uia_ocr_and_screenshot_evidence_together`；
`77ace3b` 的分支、main、tag 三趟都綠。

### 30.2 切段

- 第一次查詢一個字都不改。放寬照順序：`relax_base`（剝問法；類型題的主題整段沒看過就
  不放寬）→ `retry_candidate`（索引拿掉頭尾沒看過的條件）→ 新的 `joint_retry`。
- `joint_retry` 在空白、中文標點和「的」「了」切開，每段再過 `question::terms` 剝頭尾虛字。
  剝完只剩虛字、或只剩一個字的段丟掉，其餘每一段都是 AND 條件。「目的」「了解」裡的不切。
- 從 `relax_base` 剝完的字切，不從上一步的候選字切（30.4 第二條）。
- 和第一次查詢或上一步的候選字相同就不找；也過同一支 `candidate_refused`。
- 呼叫端：上一步找得到就不切段；切段有東西才換掉上一步的結果，空手照舊報上一步的候選字。

### 30.3 收貨

- 探針：recall 語料加「月報連結已更新」，背景分沒有、17 句、repo 文件（58 萬字，主題字
  排除）三份。正向 96 格變了 19 格，全是空手變找到；反例 96 格變 1 格（文件背景「為了部署
  失敗」改用「部署失敗」）。最慢 96.6 ms（改前 97.0，同一題），走到切段的題 8.5 ms 以內。
- `joint_relax.rs` 11 條：在 alpha.161 的樹上紅 5 條，全是正向那幾條；原則那 6 條本來就綠，
  守的是切段不可以做壞的事。其中一條逐題照打 Windows 清單那一節，清單引號裡的「改用」
  句子就是它的字面值。
- `when_questions.rs` 那條英文測試改寫：「when is the release candidate」現在找得到；畫面上
  沒有 is 的「when is ERR_DEPLOY_42」照舊空手。
- `recall-phrasing` 加 3 題（失敗的部署、失敗了的部署、where is the release candidate），題數
  改成常數 `PHRASING_COUNT`。alpha.161 的執行檔跑新題 9 格全 ✗。
- CLI 端到端：清單那四行，有背景、沒背景兩份資料夾，alpha.161 和這一版各問一次。「週報的
  網址」「誰更新了週報網址」「where is the staging build」兩份都是改前空手、改後找到；「誰改了
  週報網址」只有有背景那份是改前空手。反例「週報的備份網址」兩份兩版都空手；「上次看到的
  週報網址」有背景那份兩版都空手，沒背景那份兩版都找到。
- 清單引號閘門 193 → 198，新加的五句逐句改一個字都會紅。「沒有找到。」「暫停記錄」短於
  8 個字，閘門照設計不看。

**突變 19 刀**（對 `81ae7a5`；控制組 core 1025、recall 6 條全綠；core 跳過已知 flake 的
`wakeup::tests::` 與 `reviewer::tests::real_review_pass`）。第一輪 16 紅 3 綠：

| 範圍 | 紅的刀 |
|---|---|
| 切段 | 不切「的」；不切「了」；一個字的段也留著；虛字段也留著；每段不剝頭尾虛字；詞裡的「的」「了」也切；不在中文標點切；不在空白切；從上一步的候選字切；找過的也再找；不過兩道出口 |
| 呼叫端 | 找過的少了原句；上一步找得到也切段；切段空手也報切段的字；不找切段 |
| recall | 題數常數少三（liveness 紅） |

綠的三刀：

- 「呼叫端找過的少了上一步」：結果一樣，只多一趟查詢。補一條單元測試（「誰改了客服專線」
  兩步的候選字都是「客服專線」，只找一次），之後紅。
- 「類型題的主題整段沒看過就不放寬」整道拿掉：core 1190、CLI 513 條全綠；alpha.161 的樹上
  拿掉同一道，core 1174 條也全綠。探針上它擋的是類型詞在前、沒看過的主題在後、交界的
  雙字又看過的問法：「網址的退款」「時間的退款」拿掉尾巴的「款」，改用「網址的退」「時間的退」，
  拿「按下的退出鍵」那張畫面上的網址和日期來答。補一條（`trailing_noise_relax.rs`），之後紅。
- 「剝完空的也放寬」：兩步各自的 `candidate.is_empty()` 已經擋住，哪個方向都不動，留著當早退。

「不切了」在 recall 那一邊是綠的：phrasing-21「失敗了的部署」的「了」在段尾，`question::terms`
先剝掉了。那一題守的是「了」不擋路；切在「了」由 `joint_relax.rs` 守。

### 30.4 我這一輪做錯的

- 第一版比較激進：看過的段和沒看過的段，照頻率決定丟不丟。六條釘著原則的既有測試紅了。
  改成兩個字以上的段一律是條件、只丟一個字和虛字的段，不看頻率；六條裡五條原封不動就綠，
  剩下那條是英文 when is，行為真的變了，改寫。
- 第一版從上一步的候選字切。文件背景看過「議的」、沒看過「會議」，上一步給「議的密碼」，
  切出來只剩「密碼」，拿兩筆不相干的密碼來湊。改成從剝完的字切，釘一條。
- 英文那條代價的例子我先挑 ERR_TIMEOUT_7：它第一次查詢就用 facts 答出 2026-09-20／03:14，
  根本不空手。換成 ERR_DEPLOY_42。
- 註解第一稿寫「的確」「受不了」多半對不到、整步空手——沒量過。換成單元測試釘著的
  「受不了部署失敗」→「受不 部署失敗」。
- recall 檢查器最後一行寫死「口語 20 題」，加題之後就是假話。改成常數。
- 我以為「電信的帳單」是這一版要救的；第一次查詢就答得出 NT$1,350，拿掉。
- 版本說明第一稿寫「她一定看過「原因」「上次看到」」。那是猜的，而且決定結果的是頭尾的
  雙字「上次」「原因」，改成「多半都看過」。
- 清單第一稿的英文題，記事本那一行寫「the staging build is ready」：畫面上有 the，上一步
  就找到了，根本走不到切段。CLI 探針兩版都印「改用『is the staging build』」才看出來，
  拿掉 the。
- 清單的問句寫在清單頁上，也會留在終端機上。他在同一台機器上看，她就會直接找到那一頁，
  「改用」那一行不會出現。alpha.161 那一節也是這樣。清單「怎麼用」加一段、alpha.162 那一節
  開頭寫順序：先暫停記錄再問。
- 補「沒看過的主題」那條測試，第一版夾具把網址和日期放在別張畫面上。拿掉那一道只多印
  「改用」、答案是空的，註解卻寫「會拿別張畫面的網址和日期來答」。重跑那一刀讀到失敗訊息
  才發現；網址和日期改放在退出鍵那一張，才真的答錯。

### 30.5 登記在案、這一版不修的

- 「部署失敗的目的」改用「部署失敗」：`question::terms` 先剝句尾的「的」，「目的」只剩「目」，
  被當成一個字丟掉。`JOINT_INSIDE_WORDS` 護不到句尾。版本說明寫了。
- 「受不了部署失敗」切成「受不 部署失敗」，空手。三個字、最後一個是「了」的詞都一樣。
- 看過的「原因」「上次」是條件：用過的索引上「部署失敗的原因」「上次看到的月報連結」空手，
  剛裝好反而找得到。版本說明寫了，`joint_relax.rs` 兩邊都釘。alpha.163 修了（見 31）。
- 英文的 is 還是條件：「when is ERR_DEPLOY_42」空手。版本說明寫了。
- 「會議的密碼」在文件背景空手，報的是上一步的半截詞「議的密碼」。
- 「部署失敗的時間」「部署失敗的期限」空手（畫面上沒有日期），「部署失敗是什麼時候」卻答得出
  看到的時候。alpha.161 的設計：「什麼時候」切掉、「時間」是類型詞。這一版沒動。
  「時間」alpha.163 修了（見 31），「期限」照舊。
- 切開的段只要在同一張畫面，不必相鄰。版本說明寫了。
- alpha.161 的樹上跑 CLI 控制組、同時另一棵樹在跑 cargo，
  `ops::record::record_tests::run_keeps_the_heartbeat_barrier_for_an_older_recorder` 紅過一次。
  這一版的 CLI 控制組沒有並行，513 條全綠。

## 31. alpha.163：說成「的時間」「的原因」「上次看到的」，她答得和原本的問法一樣（2026-09-25）

30.5 登記的「看過的『原因』『上次』是條件」和「『部署失敗的時間』空手」，這一版補上。grok 回 402、codex 沒額度，這一版也是我自己寫的。

### 31.1 alpha.162 的出貨收據

tag `v0.1.0-alpha.162` 打在 `61a17ab`。tag 那一趟（36199939683）第一次紅在 Windows 的 UIA 那一步：
`native_edge_pdf_scroll_keeps_uia_ocr_and_screenshot_evidence_together`（30.1 那一條），其餘五個
job 全綠，Release 被跳過。`rerun --failed` 第二次 Windows、Release、Website 全綠，release 於
**2026-09-26T00:20:23Z** 發出，四個 asset、prerelease，body 前綴和本機 `release-notes.sh` 產的
一致。同一個 sha 在分支（36196079897）和 main（36199937636）那兩趟都綠。本機 gates 52 條綠、
`cargo test --workspace` 紅一條（31.5 的 watch 測試）；同一棵樹重跑 2380 條全綠，HEAD 和狀態前後
不變。

### 31.2 剝什麼、講什麼

- 第一次查詢一個字都不改。只動放寬的第一步 `peel_retry`（原 `peel_retry_terms`，測試還用同名的薄包裝）。
- 剝完問句詞之後，`cut_tail_asks` 剝尾巴的 `TAIL_ASKS`：「時間」「原因」。只認尾巴、前面要還有內容，可以連著剝（「部署失敗原因的時間」剩「部署失敗」）。只剩「時間」「原因」，或是「時間表」「原因分析」，不切。
- `SPOKEN_LEAD_INS` 加「上次」「上一次」「之前」。
- 要不要講「底下每一筆的時間」改由 `RelaxPlan.asks_when` 決定：`AsksWhen(facts::asks_when(query) || 剝掉了尾巴的「時間」)`。呼叫端只讀它，不再自己拿原句算。
- `facts::asks_when` 的語意不變：「週會的時間」照舊不算，第一次查詢就從 facts 答 9/30 14:00。
- 「期限」「日期」不收：截圖的時間不是期限，也不是帳單上的日期。

### 31.3 收貨

- 探針（recall 語料加「月報連結已更新」；背景沒有、17 句、repo 文件 58 萬字且主題字的行排除；
  docs 釘一份 61a17ab 的快照）：alpha.162 的樹對這一版，132 格變 58 格。
  - 正向 20 題、47 格，全是空手變找到；沒變的那幾格本來就找得到。20 題改完在三份背景上
    答的都一樣。
  - 反例 4 格：17 句背景以前把「退款的原因」「上次的退款」改用「原因」「上次」去找，各拿 1 筆
    背景的字來湊；文件背景把「退款的原因」改用「的原因」，拿 5 筆。這三格現在都空手。文件背景
    的「上次的退款」以前印「改用『上次的退』」、0 筆，現在不印。
  - 其餘 7 格：「上次那個人」「上次看到的那個人」改用「個人」（3 格），「上次看到的東西」
    「之前那個事件」在文件背景改用「東西」「事件」、各 5 筆（2 格），都是 31.5 登記的代價；
    17 句背景的「上次看到的東西」以前改用「上次看到的」拿 1 筆，現在空手，「之前那個事件」
    以前印「改用『之前那個』」、0 筆，現在不印（2 格）。
- 第二份探針（註解裡引過的題，87 格變 12 格）：「之前的電話」「上次的時間」在文件背景以前改用
  「之前 電話」「上次 時間」拿 3 筆／1 筆不相干的字，現在空手（剝完只剩類型詞，
  `candidate_refused`）。「開會時間」「上次開會的時間」改用「開會」並講時間；「處理時間」
  文件背景改用「處理」；「部署處理時間」沒背景和 17 句背景改用「部署」（「處理」沒看過，上一步
  當成尾巴拿掉），文件背景是「部署處理」空手。「上次說的」17 句背景以前改用「上次」拿 1 筆，
  現在空手；文件背景以前空手，現在改用「說的」拿 1 筆。
- 第三份探針（前面只剩虛字或口語開頭的「原因」「時間」，16 題、48 格變 4 格）：「上次的原因」
  「之前的原因」在 17 句背景和文件背景改用「原因」，各拿 1 筆／5 筆（alpha.162 在文件背景的
  「上次的原因」是改用「上次 原因」拿 2 筆，其餘三格空手）。alpha.162 的「到底什麼原因」本來
  就改用「原因」，「什麼原因」「的原因」第一次查詢就找「原因」；其餘 14 題兩版一樣。
- `asked_another_way.rs` 10 條：alpha.162 的樹上紅 8 條；綠的兩條是背景前提和
  期限／日期，本來就該兩版都綠。清單那條逐題照打 Windows 清單 alpha.163 那一節，
  alpha.162 那一節的記事本也放進同一個資料庫，斷言 alpha.162 最重要的反例照樣空手。
- `joint_relax.rs` 原封不動在新程式上紅剛好三條：`a_type_word_piece_is_what_he_asked_for`、
  `a_seen_aspect_word_is_still_required_on_a_used_index`、清單那條。其餘 8 條綠。前兩條
  釘的是 alpha.162 的代價，第三條是清單的「上次看到的週報網址」；改寫或刪掉，搬到新檔。
- recall-phrasing 23 → 26 題。alpha.162 的執行檔只有「部署失敗的時間」三格 ✗；「部署失敗的
  原因」「我上次看到的客服專線」在沒有背景的語料上 alpha.162 本來就答得出來，只釘不退步。
- 清單引號閘門 198 → 203，新加的五句逐句改一個字都會紅。第一次改「客服專線」那句是綠的：
  我改成的「改用『客服電話』去找」`sister-cli` 的 `ops.rs` 測試裡逐字就有，閘門認得它；
  換成「客服專用」才紅。

**突變 18 刀**（對 `0c6d6f9`；控制組 core 1201 條全綠；跳過已知 flake 的 `wakeup::tests::` 與
`reviewer::tests::real_review_pass`）。18 刀全紅：

| 範圍 | 紅的刀 |
|---|---|
| 尾巴 | 不切「時間」；不切「原因」；「時間」不算問時間；「原因」算問時間；前面只剩虛字也切；只切一次；最後切的那個說了算；整步不切；先切尾巴再切問句詞 |
| 計畫 | 不看尾巴的「時間」；不看問句詞 |
| 呼叫端 | 空手也講時間；改回拿原句算；一律不講；一律講 |
| 口語開頭 | 少「上次」；少「上一次」；少「之前」 |

「前面只剩虛字也切」「只切一次」「最後切的那個說了算」只有單元測試紅。後兩刀改的是連著剝
那幾格，單元測試直接呼叫產品那一支 `peel_retry`，夠了。第一刀在產品上看得到：「上次的原因」
不再改用「原因」去找，而那是版本說明要寫的代價（31.5）。整合測試補上那一格（`fb025d9`），
同一刀在新測試上紅在「上次的原因」。「少『上一次』」只紅一條
`last_time_and_before_are_how_he_asks`。「改回拿原句算」另外紅了兩條 `brain::tests` 的
2 秒牆鐘上限：那一刀只改檢索的呼叫端，當時 load 21 上下，和這一刀無關（31.5）。

### 31.4 我這一輪做錯的

- 第一次 A/B 探針拿 `wt-a162-base` 當基準。那棵樹停在 77ace3b（alpha.161 的 tag），
  不是 alpha.162；兩邊的文件背景又各讀各的 docs，單獨問「原因」「上次」的第一次查詢
  命中數跟著變，看起來像這一版改了第一次查詢。改成基準樹開在 61a17ab、兩邊讀同一份
  docs 快照，重跑才是 58 格。
- 清單 alpha.163 那一節的記事本第一稿用「備份失敗」。alpha.162 最重要的反例是
  「週報的備份網址」；兩節都做的話，「週報網址」和「備份」就在同一張畫面上。換成
  「同步失敗」，清單那條測試把兩節的記事本放在一起跑。
- alpha.162 那一節的順序叫他「按繼續記錄，開記事本打字」：打字的時候這一頁和終端機還在
  畫面上，反例的問句本身會被記下來。改成暫停時打字、記事本放到最大，再繼續記錄半分鐘、
  暫停、才問；寫在「怎麼用」一處，四節指過去。
- 版本說明第一稿寫上一版「『上次的……』會改用『上次』去找，拿不相干的畫面來湊」。文件
  背景那一格是「上次的退」、0 筆，拿掉那半句。
- 版本說明的代價第一稿只寫「上次那個人」。寫這一節時重數 58 格才看到「上次看到的東西」
  「之前那個事件」在文件背景改用「東西」「事件」、各拿 5 筆；我先前把「其餘」都當成黏字。
  版本說明補上，`asked_another_way.rs` 那條代價測試擴成三組（「所以看到的東西」
  「所以那個事件」當對照），`SPOKEN_LEAD_INS` 的註解也寫上。
- 版本說明第一稿寫「開會時間」的時間「標題下面會講明」。`sister query` 是貼著標題印，
  桌面那一句是原文清單的第一行（`app.js` 的 `hits-note`），上面沒有標題；改成「多講的
  那一句」。清單只用 `sister query`，「標題下面會說」照舊。

### 31.5 登記在案、這一版不修的

- 「上次那個人」「之前那個人」和「所以那個人」一樣改用「個人」：剝完剩「那個人」，
  `question::terms` 往左退一個字。放寬這條路上印的是 Relaxed，不是第一次查詢那句 Glued。
  拒收黏出來的候選字會讓「還記得那個看板」找不到（「看」是虛字，一樣往左退）。
  「上次看到的東西」「之前那個事件」同理改用「東西」「事件」，和「所以看到的東西」
  「所以那個事件」一樣；「上次的原因」改用「原因」，和「到底什麼原因」一樣（尾巴的「原因」
  前面沒有別的字就不切）。版本說明寫了，`asked_another_way.rs` 釘著。
- 尾巴的「時間」一律當成在問時間：「開會時間」而畫面上沒寫日期，給的是看到「開會」的
  時間；「處理時間」改用「處理」。版本說明寫了。
- 「部署失敗的期限」「部署失敗的日期」照舊空手。
- 本機 gates 的 `cargo test --workspace` 在 load 15–21 時紅過一次
  `ops::watch::tests::an_admitted_watch_round_drains_before_stop_and_does_not_publish_after_pending`
  （裡面有 5 秒上限；單獨跑十次紅一次，耗時 0.18–84 秒）。同一棵樹重跑 2380 條全綠，CI 綠。
  已知 flake 多一條。
- 突變那一輪 load 21 上下時，`brain::tests::inherited_output_pipe_in_a_descendant_cannot_hold_the_invocation_open`
  和 `brain::tests::timeout_still_runs_when_provider_never_reads_a_pipe_filling_stdin` 一起紅過一次
  （兩條都有 2 秒牆鐘上限）。這兩條在控制組和其餘 17 刀的那幾趟都綠。已知 flake 再多兩條。

## 32. alpha.164：英文問句「when is ERR_DEPLOY_42」「where can I find the build log」，她答得和只打主題一樣（2026-09-25）

30.5 登記的「英文的 is 還是條件」，這一版補上。grok 回 402、codex 沒額度，這一版也是我自己寫的。

### 32.1 alpha.163 的出貨收據

tag `v0.1.0-alpha.163` 打在 `a53362b`。分支那一趟（36204636060）六個 job 全綠。本機 gates 在
detached 的 `a53362b`（樹 `8c34de46abfd3b59`）上 53 條全綠。tag 那一趟（36207332387）八個 job
第一次就全綠，release 於 **2026-09-26T02:00:04Z** 發出，四個 asset、prerelease，body 前綴和本機
`release-notes.sh` 產的一致。

### 32.2 拿掉什麼、切開什麼

- 第一次查詢一個字都不改，只動放寬那幾步。
- `strip_english_inversion`：開頭是英文問句詞（`question::QUESTION_WORDS` 和 `facts::WHEN_ASKS`
  裡的 ASCII 字，when 只在後者），**緊接著**是 `ENGLISH_AUXILIARIES` 裡的助動詞，連同後面至多
  一個 `ENGLISH_SUBJECTS`（i、you、we、they、he、she、it）一起剝掉。縮寫（'s、're、'd、'll、've，
  直撇號和彎撇號都認）和 n't 也算。全大寫的 IT 不是主詞。問句詞要是整個字（whenever 不算），
  is 後面連著底線或連字號（is_valid、is-it）也不算。`peel_retry` 的兩處都經 `strip_head`：
  先試口語開頭，再試這一條。
- `ENGLISH_JOINTS`（of、for、in、at、by、with、from、about）：`joint_retry` 切出來的段整段等於
  其中一個字就丟掉，和「的」一樣。format 裡的 for 不算。
- `retry_candidate` 多一條：第一個英文接頭前面那一段整段沒看過（`IndexedCandidate::NoneSeen`），
  索引那一步不出候選字；`joint_retry` 照樣會試。為什麼要這條，見 32.4 第一點。

### 32.3 收貨

- 探針 69 題 × 四份背景（沒有背景、17 句、repo 文件、英文授權檔）＝276 格，alpha.163 的樹對這一版：
  - 96 格空手變找到。
  - 16 格找到的一樣：改用的字從「is release candidate」變「release candidate」，那張畫面上剛好有 is。
  - 6 格找到變空手，全是拿來湊的：「where is it」「where is the invoice」以前改用「is」拿 1 筆，
    或改用「is the」拿 5 筆（文件、英文兩份背景）。
  - 7 格空手照舊、說法不同：改用的字少了 is、will、would、has 或 the；英文背景的「what does
    ERR_DEPLOY_42 mean」以前不印「改用」，現在改用「ERR_DEPLOY_42 mean」、0 筆；文件背景的
    「where is it」以前改用「is it」、0 筆，現在不印。
  - 其餘 151 格一樣。加上接頭前段那條規則前後，276 格一格都沒動。
- 第二份探針（沒看過的頭＋英文接頭，16 題 × 4＝64 格），alpha.163 對這一版：
  - 10 格找到變空手，全是拿來湊的：「invoice for alpha」以前改用「alpha」或「for alpha」，
    「the invoice for the release candidate」改用「release candidate」，「plorkin with the vendor」
    改用「vendor」，「invoice of ERR_DEPLOY_42」改用「ERR_DEPLOY_42」；英文背景的「why did it crash」
    以前改用「it」拿 5 筆。
  - 9 格空手變找到：「where is the build log for staging／plorkin」改用「build log」、「where is
    the contract of plorkin」改用「contract」（沒有背景和 17 句，6 格）；文件背景的「why did it
    crash」「why did it fail」「when did it fail」改用「crash」「fail」、各拿 5 筆（3 格，代價）。
  - 2 格找到的不一樣：英文背景的「why／when did it fail」以前改用「it fail」拿 1 筆，現在改用
    「fail」拿 5 筆（代價）。
  - 29 格空手照舊、說法不同，14 格一樣。
  - 只看接頭前段那條規則：加之前對加之後，18 格找到變空手（上面那幾題，加上「where is the
    invoice for alpha」這一類剝完才變成同一串的），14 格空手照舊、說法不同，32 格一樣。
- 新測試搬到 alpha.163 的樹上跑：`english_questions.rs` 7 條紅 6 條，綠的是背景前提那條；
  `when_questions.rs` 改寫的那條紅，其餘 10 條綠；`joint_relax.rs` 改寫的那條紅，其餘 9 條綠；
  `asked_another_way.rs` 只改記事本，10 條綠。
- `when_questions.rs` 和 `joint_relax.rs` 各有一條舊測試釘著「is 還是條件」的代價
  （`an_english_when_is_question_still_needs_is_on_the_screen`、`an_english_filler_is_not_a_condition_after_the_split`）。
  這一版就是要改那個代價，兩條改寫成相反的斷言；alpha.162 那一節清單的英文題和記事本那一行
  一起拿掉，英文改在新的一節驗。
- recall-phrasing 26 → 29 題。alpha.163 的執行檔紅在「when is ERR_DEPLOY_42」「where is
  ERR_DEPLOY_42」兩題；「what is ERR_DEPLOY_42」alpha.163 本來就答得出來，只釘不退步。
- 清單引號閘門 203 → 207，新加的每一句逐句改一個字都會紅。

**突變 31 刀**，兩批切了 33 次，都跳過已知 flake 的 `wakeup::tests::` 與
`reviewer::tests::real_review_pass`。第一批 23 次對 `de09f95`（接頭前段那條規則之前），控制組
core 1210 條全綠；第二批 10 次對 `3377a5d`，控制組 1212 條全綠：接頭前段 7 刀，第一批沒抓到和
編不過的兩刀重切，加上「主詞少 you」。31 刀最後全紅：

| 範圍 | 紅的刀 |
|---|---|
| 問句詞 | 不看 `WHEN_ASKS`；分大小寫；字的邊界放寬 |
| 助動詞 | 少 is；少 can；不查助動詞表；分大小寫 |
| 縮寫與否定 | 不認縮寫；縮寫少 s；不認彎撇號的否定；不認 n't |
| 主詞 | 不拿掉；全大寫也算；什麼字都當主詞；少 I；少 you |
| `peel_retry` 兩處 | 第一處不認倒裝；第二處不認；兩處都不認 |
| 切段的接頭 | 不丟；分大小寫；用包含；少 with；少 for |
| 接頭前段 | 整段拿掉；開頭的接頭也算；分大小寫；用包含；查整串不查前段；不記位置；前段不修空白 |

- 第一批「主詞少 I」是綠的，1210 條全綠：`question::terms` 的虛字表剛好也有 i、you，走完整個
  `peel_retry` 看不出主詞表少了這兩個。補一條直接問 `strip_english_inversion` 的測試
  （`3377a5d`，和它註解裡的例子一樣），第二批重切「少 I」「少 you」都紅。
- 第一批「切段的接頭分大小寫」那一刀編不過（`piece` 是 `&&str`），紅的是編譯器，不算。第二批
  改寫法重切，紅在切段的單元測試。
- 接頭前段「不記位置」只有 `format for alpha` 那一列抓得到：`for` 先在 `format` 裡找到，前段就變成
  空的。那一列是切之前先寫的（`1f5c7f8`）。「前段不修空白」「開頭的接頭也算」「分大小寫」只有
  單元測試紅，單元測試直接呼叫產品那一支 `before_english_joint`。

### 32.4 我這一輪做錯的

- 版本說明第一稿寫「問的東西她沒看過的，一件事都沒有」，只量了「where is the invoice」這一種。
  另寫 64 格的探針才看到「where is the invoice for alpha」：剝完是「invoice for alpha」，索引那一步
  把沒看過的 invoice 當成頭拿掉，改用「alpha」或「for alpha」去湊；而直接打「invoice for alpha」
  上一版就已經這樣。補了接頭前段那條規則（32.2），版本說明照量到的寫。
- 版本說明第二稿又有三句沒量過：上一版「invoice for alpha」「拿寫著 alpha 的畫面來湊」，英文
  背景那一格是「for alpha」、0 筆，改成「畫面上剛好寫著那幾個字就拿來湊」；第三個例子
  「the contract with the vendor」不在四份背景的探針裡，換成量過的「the release candidate for
  alpha」；「why did it fail」改用「fail」只發生在看過 fail 的機器上，沒有背景和 17 句兩格什麼都
  不印，補上條件。
- 整合測試第一稿把「why is ERR_DEPLOY_42 failing」放在正向。語料的「release candidate alpha is
  ready」有 is，我自己寫的英文背景有 failing；這題是主詞後面的動詞，搬到代價那條，正向換成
  「what is ERR_DEPLOY_42」。「when did they sign the contract」的 sign 和 signed 在 trigram 上
  分不開，拿掉。
- commit 訊息第一稿寫 alpha.162 那一節的英文題「移過來」。實際是刪掉、在新的一節重寫；push 前
  amend。

### 32.5 登記在案、這一版不修的

- 接頭後面那段沒看過：「where is the build log for staging」在沒看過 for 的機器上改用「build log」、
  找到；用過的機器上改用「build log for」、空手（for 看過，不是沒看過的尾巴）。fail-closed，
  和中文沒看過的尾巴同一族。
- 拿掉代名詞之後只剩一個動詞（why did it fail）、主詞後面的動詞（mean、ship）、「log in」改用
  「log」、how much is／where exactly is 的 is 照舊是條件：版本說明都寫了。
- 中文「去哪裡找月報連結」四份背景、兩版都空手；「在哪裡可以找到月報連結」「哪裡找得到月報連結」
  在文件背景改用「可以找到月報連結」「找得到月報連結」、空手，兩版一樣。

## 33. alpha.165：問「去哪裡找」「哪裡找得到」，她答得和只打主題一樣（2026-09-26）

32.5 登記的「去哪裡找月報連結」，這一版補上。grok 回 402、codex 沒額度，這一版也是我自己寫的。

### 33.1 alpha.164 的出貨收據

tag `v0.1.0-alpha.164` 打在 `559bb92`。分支那一趟（36212080975）六個 job 全綠。本機 gates 在
detached 的 `559bb92`（樹 `880938b927945a3c`）上 53 條全綠。tag 那一趟（36214633707）八個 job
第一次就全綠，release 於 **2026-09-26T04:09:46Z** 發出，四個 asset、prerelease，body 前綴和本機
`release-notes.sh` 產的一致。

### 33.2 拿掉什麼、切開什麼

- 第一次查詢一個字都不改，只動放寬那幾步。
- 「哪裡」「哪裏」「哪里」「哪兒」「哪儿」「哪邊」「哪边」「哪」從 `QUESTION_WORDS` 搬到新的
  `WHERE_WORDS`。`find_question_word` 三張表一起找，同一位置取最長，所以「哪個」「哪些」照舊是
  `QUESTION_WORDS` 的字。`FoundQuestionWord` 的 `asks_when: bool` 換成 `QuestionKind`
  （Plain／Where／When）。
- `cut_question_words` 找到 Where 的字，照舊先看類型詞，再交給 `widen_where` 往兩邊擴：
  - 前面：緊接的 `WHERE_GOES`（去、到、在）；有的話，再往前至多一個 `WHERE_GO_MODALS`
    （可以、應該、应该、該、该）。
  - 後面：`WHERE_FINDS`（找得到、找、看得到、看到），中間可以先隔一個 `WHERE_FIND_MODALS`
    （可以、才能、能、才）。後面沒有接找法，modal 留著：「哪裡可以下載月報」剩「可以下載月報」。
  - `where_find_len`：`question::terms` 在切問句詞之前已經剝掉句尾的虛字，「哪裡找得到」到這裡
    是「哪裡找得」，「哪裡可以看到」是「哪裡可以」。整段剩下的是某個找法、少掉的尾巴全是虛字，
    也算。
- 「要」「能」「會」「得」不收進 `WHERE_GO_MODALS`：它們常是名詞的最後一個字（摘要、功能、晨會、
  心得）。「找到」「看」不收進 `WHERE_FINDS`：拿掉「找」之後「到」是虛字；「看」本身是虛字，最後
  一次 terms 會剝掉開頭的它。
- 類型詞那一步對 Where 到不了：「哪」不在任何類型詞裡。保留 `!= When` 的寫法，是為了和搬家前
  「哪裡」在 `QUESTION_WORDS` 時一模一樣。

### 33.3 收貨

- 探針 76 題 × 四份背景（沒有背景、17 句、repo 文件、英文授權檔）＝304 格，alpha.164 對這一版：
  - 86 格空手變找到。
  - 2 格找到變空手，全是拿來湊的：文件背景「哪裡找得到火星」以前改用「找得到」、「哪裡找得到」
    以前改用「找得」，各拿 5 筆。
  - 2 格找到的東西不同：文件背景「該去哪裡找月報連結」以前改用「該去」拿 1 筆不相干的，現在改用
    「月報連結」找到那一張；「客服電話去哪裡找」以前改用「客服電話去」，現在「客服電話」，兩版
    都答出那支號碼。
  - 1 格空手照舊、說法不同：文件背景「在哪裡可以找到火星」以前改用「可以找到」、0 筆，現在不印。
  - 其餘 213 格一樣。
- 第三份探針多 5 題（20 格）：文件背景「哪裡看得到」以前改用「看得」拿 5 筆，「哪裡可以看到」
  「哪裡可以」以前改用「可以」各拿 4 筆，現在都空手、不印改用；其餘 17 格一樣。
- 舊探針 12 支（alpha.161–164）在兩棵樹上重跑，共 1072 格：只有 `zz_probe_a164` 的 6 格不同，全部
  空手變找到（「去哪裡找月報連結」×4、文件背景的「在哪裡可以找到月報連結」「哪裡找得到月報連結」）。
  `zz_probe_when2`、`zz_probe_money` 的行不是 Q 開頭，另外比（when2 的四條測試平行跑，排序後比），
  一格不差。
- 新測試搬到 alpha.164 的樹上跑：`where_to_find.rs` 9 條紅 7 條，綠的是背景前提那條和登記在案的
  `a_two_character_name_before_zai_nali_is_still_nothing`。另一條登記在案的「要去」也紅：alpha.164
  在背景裡改用「月報連結要去」，兩版都空手，找的字不同。
- recall-phrasing 29 → 32 題。alpha.164 的執行檔紅在新加的三題（去哪裡找客服專線、去哪找
  ERR_DEPLOY_42、去哪裡找部署失敗的原因），每題 recalled、answer_correct、citation_correct 三格都紅。
- 清單引號閘門 207 → 211，新加的四句逐句改一個字都會紅。
- sister-core 整包 31 個 binary、1254 條全綠。

**突變 41 刀**，對 `ebc96bd` 一批：控制組（`--lib` 加上會走 retrieval 的 11 個測試檔）1069 條全綠，
跳過已知 flake 的 `wakeup::tests::` 與 `reviewer::tests::real_review_pass`。36 刀紅：

| 範圍 | 紅的刀 |
|---|---|
| 問地點的字 | 少哪裡、哪裏、哪里、哪兒、哪儿、哪邊、哪边、哪 |
| 前面的去、到、在 | 少去；少到；少在 |
| 再前面的可以、應該、該 | 少可以、應該、应该、該、该 |
| 後面的找法 | 少找得到；少找；少看得到；少看到 |
| 找法前面的可以、能、才 | 少可以、才能、能、才 |
| 不收的字 | 加要；加能；加會 |
| `widen_where` | 不往前看；「去」前面不看「可以」；「可以」沒接找法也吃；不往後看；問地點的不擴 |
| `where_find_len` | 不認少了尾巴的；取最短 |
| `longest_suffix`／`longest_prefix` | 取最短（兩刀） |

- 「加得」是綠的：`WHERE_GO_MODALS` 不收「得」的理由是「心得」，沒有一列釘著。補「心得在哪裡找得到」
  （`4853868`），重切，控制組 1070 條全綠，那一刀紅在單元測試。
- 另外四刀綠，拿整個 sister-core 重跑（控制組 1222 條全綠）也是綠，留著：
  - 加「看」、加「找到」：註解說這兩個是多餘的一格，綠就是那句話。
  - 「少了尾巴不查虛字」：現在這張表上，這個檢查擋掉的每一種情形，另一格都給出一樣的長度（「看」少了
    「得到」被擋，「看到」少了「到」照樣算），拿不拿掉結果都一樣。留著是因為表會長。
  - 「可以」不必先有「去」：只有「可以」「應該」「該」直接貼著「哪裡」才分得出來，那不是自然的說法。

### 33.4 我這一輪做錯的

- 兩棵 worktree 共用一個 `CARGO_TARGET_DIR`（`wt-a164-r1/target`）。舊探針先在 base 樹跑、再在改過的
  樹跑，第二輪 12 支全部和 base 一樣：兩棵的 unit hash 相同，另一棵剛編好的 lib 比這棵的原始碼新，
  cargo 沒有重編。是「去哪裡找月報連結」在改過的樹上還是空手，我才發現。那份輸出改名
  `old-probes-r1.stale-lib`，之後一棵樹一個 target dir，腳本把它當必填參數，重跑才得到上面的結果。
  base 那一輪也用 `wt-a164-r1/target`，但那棵樹就是 `559bb92`，和 base 同一份原始碼，所以算數。
- 單元測試第一版期望「哪裡找得到」剝成空的，實際剩「得」：`question::terms` 在切問句詞之前就把
  句尾的「到」當虛字剝掉，「找得」對不上「找得到」。同一個洞讓 alpha.164 在文件背景把「哪裡看得到」
  改用「看得」、「哪裡可以看到」改用「可以」去湊。補了 `where_find_len`。
- `WHERE_FINDS` 第一版有「找到」。拿掉它突變會綠（「找」加上虛字「到」是同一個結果），拿掉並寫明
  為什麼。`widen_where` 第一版還有一個沒有輸入走得到的邊界夾，也拿掉。
- 註解第一版三句不對：「改用『找得到月報連結』、空手」寫成現在式，其實是 alpha.164 的行為；
  「怎麼找客服電話」的「找」照舊交給索引，探針量的是「怎麼找月報連結」；「單獨的『看』不收，因為它是
  『看板』的第一個字」只在很窄的情況下成立。改成過去式、改成量過的那一句、改成「它本來就是虛字，
  最後一次 terms 會剝掉」，並加一列「哪裡看月報連結」釘住。

- Windows 清單第一版問「報價單連結要去哪裡找」。「要」不在表上，那一題靠索引把沒看過的「結要」
  放掉；整合測試的背景沒有「結要」，所以一直是綠的。Ted 那台機器只要在畫面上看過「連結要」（清單
  這一頁自己就寫著）就會假紅。出貨前換成「該去」，另外登記一條測試（33.5）。

### 33.5 登記在案、這一版不修的

- 「要去」：「要」不收進 `WHERE_GO_MODALS`（「摘要去哪裡找」要留著「摘要」），「月報連結要去哪裡找」
  剝完是「月報連結要」，靠索引那一步放掉沒看過的「結要」。四份背景都沒看過「結要」，所以都找到；
  多一張寫著「這個連結要記得更新」的畫面，就改用「月報連結要」、空手，拿掉那張畫面同一題就找到。
  `yao_before_qu_leans_on_the_index` 釘著，版本說明寫了。

- 名詞只有兩個字、緊接「在哪裡」，而且第一個字是虛字（看板）或最後一個字是問句頭尾的用語（摘要的
  要）：問句頭尾那一步把「在哪裡」和「要」一起剝掉，剩一個字，不放寬。兩版都空手，
  `a_two_character_name_before_zai_nali_is_still_nothing` 釘著。「週報摘要在哪裡」改用「週報摘」、
  找到，也是同一步。版本說明寫了前兩題。
- 「找不到」「查得到」還不算問法：「為什麼找不到月報連結」在 17 句和文件背景改用「找不到月報連結」、
  空手；「月報連結找不到」在那兩份背景空手、不印改用；「哪裡查得到月報連結」在文件背景改用
  「查得到月報連結」、空手。兩版一樣，版本說明寫了。下一版的候選：「查得到」和「找得到」同一個
  理由；單獨的「查」不能照抄「找」，照抄的話「哪裡查詢月報連結」會剩「詢月報連結」。
- 「尋找月報連結」在文件背景空手，兩版一樣。
- 「去哪裡查月報連結」這一版四份背景都改用「月報連結」、找到：「查」不在表上，是索引那一步拿掉的。
- 「去哪裡找電話」兩版都空手：剩下的「電話」是類型詞、沒有主題，alpha.155 起寧可說不知道。

## 34. alpha.166：說「找不到」「有沒有看到」「不見了」，她答得和只打主題一樣（2026-09-26）

33.5 登記的「找不到」「查得到」「尋找」，這一版補上。grok 回 402、codex 沒額度，這一版也是我自己寫的。

### 34.1 alpha.165 的出貨收據

tag `v0.1.0-alpha.165` 打在 `3605ec6`。分支那一趟（36217086208）六個 job 全綠。本機 gates 在
detached 的 `3605ec6`（樹 `028801af0ea78bf2`）上 53 條全綠；這是第三次跑。第一次跑到一半根目錄滿了
（ENOSPC，兇手是我自己十個已出貨版本的 build 快取，清掉 190G），log 改名 `.enospc.log` 作廢；第二次
52/53，紅的是 check-erased-db：負載 22 的機器上 `sister stats | grep -q` 在 `pipefail` 底下 EPIPE，
單獨重跑三次紅一次，log 改名 `.flake-1.log`，這一版把那支腳本改成先寫檔（34.2）。tag 那一趟
（36219758378）八個 job 第一次就全綠，release 於 **2026-09-26T05:58:34Z** 發出，四個 asset、
prerelease，body 前綴和本機 `release-notes.sh` 產的一致。

### 34.2 拿掉什麼

- 第一次查詢一個字都不改，只動放寬那幾步。
- 新的 `LOOK_HEADS`（開頭的找法）與 `LOOK_TAILS`（結尾的找法），前面各自可以先接
  `LOOK_HEAD_BEFORES`（沒、沒有、能、可以、都、一直、還是、還）與 `LOOK_TAIL_BEFORES`（還沒、
  沒有、沒、一直、還是），各自連同簡體。`strip_looking` 在 `peel_retry` 用兩次：
  - 每一圈的原樣開頭，和口語開頭、英文倒裝同一個 closure。要在原句上認：`question::terms` 把
    「看」「到」當虛字，「看不到月報連結」會剩「不到月報連結」，「月報連結找不到」會剩「月報連結
    找不」。
  - `cut_question_words` 之後再剝一次，接住問句詞拿掉才露出來的：「為什麼找不到月報連結」
    「月報連結找不到怎麼辦」。
- 結尾那一個前面緊接著「哪裡」就不拿，留給 `widen_where`。
- 不收的字：單獨的「找」「看」「查」「搜」（找零、看板、查核）；開頭的「查詢」「搜尋」（查詢
  結果、搜尋欄位）；「看過」「查過」（查過期的發票）；結尾的「查」（安全檢查不到）；結尾前面
  單獨的「都」「還」（首都、歸還）。
- `WHERE_WORDS` 加「哪個地方」；`WHERE_FINDS` 加「查詢」「查询」「搜尋」「搜寻」。
- `retry_candidate` 多一道：候選字從「的」開始就不放寬。「火星的畫面」在看過「的畫面」的索引上，
  拿掉沒看過的「火星」剩「的畫面」，alpha.165 拿 5 張不相干的畫面來湊；找法拿掉之後，「找不到
  火星的畫面」也會走到這裡。`joint_retry` 在「的」切段、每一段都是條件，本來就空手。
- `scripts/check-erased-db.sh`：`sister stats`／`doctor` 的輸出先寫檔再 `grep -q`。`pipefail`
  底下 `grep -q` 一找到就關管線，`sister` 還在寫就 EPIPE、panic：正面的斷言假紅（alpha.165 的
  gates 在負載 22 的機器上撞到）；反面的，那句話後面還有字要寫，就可能在它真的印出來時放過。

### 34.3 收貨

- 新探針三支，alpha.165 對這一版，四份背景（沒有背景、17 句、repo 文件、英文授權檔）：
  - 找法 131 題＝524 格：63 格空手變找到（60 格找到「月報連結已更新」、3 格是客服專線那張）。
    11 格找到變空手，全是問「火星」，舊版改用「找不到」「看到」「沒看到」「沒有看到」「不見」
    「尋找」「沒有」「地方有」拿別的畫面來湊。1 格找到的不同：17 句背景「有沒有看到客服專線」以前改用
    「看到客服專線」、只有號碼那一格、原文 0 筆，現在改用「客服專線」，號碼和原文都有。其餘 449 格
    一樣。
  - 「的」10 題＝40 格：4 格空手變找到（「找不到月報的連結」四份背景都改用「月報 連結」）。2 格找到
    變空手，都是湊的：文件背景「火星的畫面」改用「的畫面」拿 5 筆；17 句背景「找不到火星的畫面」
    改用「找不到」拿 1 筆。1 格空手照舊、說法不同：17 句背景「沒看到火星的連結」以前改用「看到
    火星的連結」、0 筆，現在不印。其餘 33 格一樣。
  - 修飾語 28 題＝112 格：1 格空手變找到（17 句背景「找不到最新的月報連結」）；1 格找到變空手
    （文件背景「火星的畫面」，和上一支同一格）；1 格空手照舊、說法不同（文件背景「找不到最新的
    月報連結」以前不印，現在改用「最新的月報連結」、0 筆）。其餘 109 格一樣。第一版在這支上是
    34 格找到變空手（34.4）。
- 舊探針 15 支（alpha.161–165）在兩棵樹上重跑。Q 開頭的 11 支共 1670 格，只有 alpha.165 那三支的
  17 格不同，全部空手變找到：17 句和文件背景的「為什麼找不到月報連結」「月報連結找不到」，文件
  背景的「尋找月報連結」「哪裡查得到月報連結」。`zz_probe_when2`、`zz_probe_when`、`zz_probe_money`、
  `zz_probe_joint_fixture` 有不是 Q 開頭的行（when2、money 全是），排序後整份比，一字不差。
- 新測試搬到 alpha.165 的樹上跑：`could_not_find.rs` 10 條紅 9 條，綠的只有背景前提那條。
  `a_topic_never_seen_is_still_nothing` 紅在有背景的「找不到火星」：alpha.165 改用「找不到」，拿
  「怎麼找都找不到」「一直找不到停車位」兩張來湊。`a_word_before_de_never_seen_can_be_just_which_one`
  的前兩列（最新的／公司的月報連結）alpha.165 本來就找得到，紅在第三列「找不到最新的月報連結」。
- 清單引號閘門 211 → 215，新加的四句逐句改一個字都會紅。「沒有找到。」只有五個字，低於閘門的
  8 字下限，和前幾節一樣不在檢查範圍；`ops.rs` 印的就是這一句。
- sister-core 整包在 `3a436a7` 上 32 個 binary、1267 條，紅 1 條：登記過的
  `wakeup::tests::the_recorder_can_still_write_while_the_slow_path_thinks`（負載 35，假 CLI 5 秒內
  沒起來）。這一版沒碰 `wakeup.rs`；單獨重跑三次，紅一次。
- 上面的格數量的是 `a1f8af2`。18 支探針在 `3a436a7` 上重跑，Q 開頭的 2403 行一樣（每行最後的
  毫秒數不比），when2、money 排序後也一樣。

**突變 82 刀**，對 `a1f8af2` 一批：控制組（`--lib` 加上會走 retrieval 的 12 個測試檔，多了
`could_not_find`）1083 條全綠，跳過已知 flake 的 `wakeup::tests::` 與 `reviewer::tests::real_review_pass`。
76 刀紅在 retrieval 的測試上：

| 範圍 | 紅的刀 |
|---|---|
| 開頭的找法 | 24 個字各少一個 |
| 開頭前面先接的 | 12 個字各少一個 |
| 結尾的找法 | 12 個字各少一個，少「看到」除外 |
| 結尾前面先接的 | 9 個字各少一個 |
| 問地點的字、找的動作 | 少哪個地方、哪个地方；少查詢、查询、搜尋、搜寻 |
| 不收的字 | 開頭加查詢、搜尋、找、查過；結尾加查不到；結尾前面加都、還 |
| 函式 | 原句不先認找法；問句詞拿掉後不再剝；開頭前面不准有虛字；開頭前面只接一個；結尾後面不准有虛字；結尾不看「哪裡」；「的」開頭照樣放寬 |

負載 25–40 的機器上，另有 6 刀順帶紅了 `brain::tests` 的子行程計時與 `answer::tests` 的兩條心跳
測試（H06、T07、TB03、A06、K07、K09），和這幾刀無關，不算數。其餘 6 刀：

- 少結尾的「看到」（綠）：唯一那列「月報連結有看到嗎」全是虛字，terms 自己就剝光。補「月報連結
  沒看到」。
- 結尾前面只接一個（綠）：沒有一列連著接兩個。補「月報連結還是沒找到」。
- 結尾前面只剩虛字就不拿（只紅在 brain）：剝不剝都不放寬，剝完是空的，不剝就和第一次查詢一樣。
  拿掉，結尾和開頭一樣，只有找法就什麼都不剩，補「不見了」釘住。
- 「哪裡」和結尾的找法中間隔著「可以」「才能」「能」「才」也不拿（只紅在 answer）：這一道拿掉，「月報連結
  在哪裡可以找到」先拿掉「找到」，結果照樣是「月報連結」（單元測試那一列）。拿掉。
- 加「什麼地方」（綠，整個 sister-core 1235 條也綠）：註解說開頭的「什麼地方」到不了切問句詞那一步，
  綠就是那句話。
- 開頭的找法取最短（綠，整包也綠）：`LOOK_HEADS` 沒有一個字是另一個字的開頭，取最長和取最短
  一樣。`longest_prefix` 是共用的，alpha.165 已經有一刀紅在它上面，留著。

補完是 `3a436a7`：控制組同樣 1083 條全綠，上面兩道拿掉了，brain 與 answer 那幾條也在裡面。少「看到」、結尾前面只接一個、放回「前面只剩虛字
就不拿」，各紅在新補的那一列（月報連結沒看到、月報連結還是沒找到、不見了）；拿掉「前面緊接著
「哪裡」就不拿」紅 5 條：單元測試兩條、`could_not_find` 一條、alpha.165 的 `where_to_find` 兩條。

### 34.4 我這一輪做錯的

- 第一版在 `peel_retry` 剝完原句開頭與結尾的找法之後才切「哪裡」。舊探針在新樹上重跑，文件背景
  的「月報連結可以在哪裡找到」從改用「月報連結」變成改用「月報連結可以」、空手：先拿掉結尾的
  「找到」，問句頭尾那張表就把「在哪裡」切走，`widen_where` 看不到「哪裡」，「可以」留下來。
  先試把結尾那一步挪到切「哪裡」之後，不行：到那一步 terms 已經剝掉句尾的「到」，「月報連結
  沒找到」是「月報連結沒找」，結尾一個字的「找」對不到任何找法。改成結尾的找法前面緊接著
  「哪裡」就不拿。
- 為了擋「找不到火星的畫面」在文件背景改用「的畫面」，第一版加的是「第一個『的』前面那一段
  整段沒看過就不放寬」，照英文接頭那一道的形狀抄。英文的「invoice for alpha」頭在前面；
  中文的「的」頭在後面，前面常常只是修飾。commit 之前另寫一支探針專測修飾語（28 題 × 四份
  背景），112 格裡 34 格從找到變成空手：「最新的月報連結」「公司的月報連結」「客戶的月報連結」
  都是，「最新的客服專線」連號碼都答不出來。真正在湊的是候選字從「的」開始，改成只擋那一種，
  另加 `a_word_before_de_never_seen_can_be_just_which_one`，它在第一版上紅。
- 版本說明第一版寫「有沒有看到月報連結 → 改用『看到月報連結』，一件事都沒有」。那只在
  17 句背景成立，文件背景改用的是另一串。改成四份背景都成立的「一件事都沒有」。
- 整合測試第一版把只有找法的「找不到」「有沒有看到」「不見了」放進「每份資料庫都空手」。
  背景裡就有寫著這幾個字的畫面，第一次查詢會照原樣找到它們，那不是放寬。拿掉，剝完剩什麼
  由單元測試釘。
- 結尾的找法第一版多寫兩道沒有作用的檢查，也少兩列該有的測試（「沒看到」、前面連著接兩個），
  都是突變才看出來的（34.3）。
- 原本打算在 recall-phrasing 加三題。那份語料裡一個找法都沒有，alpha.165 在沒有背景時已經
  答得出這幾種說法，加了也量不到這一版改的東西，沒有加。

### 34.5 登記在案、這一版不修的

- 請她幫忙找的說法：「可以幫我找月報連結嗎」「能不能幫我找」「可不可以幫我找」「你可以幫我找…嗎」
  在 17 句背景改用「幫我找月報連結」、空手，文件背景空手、不印改用。兩版一樣，版本說明寫了。
  候選：「幫我找」「幫我查」後面接內容時是問法，但「幫我」本身不能收（「幫我看一下這個」）。
- 開頭的「查詢」「搜尋」、結尾的「查不到」：文件背景「查詢月報連結」「搜尋月報連結」「月報連結
  查不到」空手，兩版一樣。刻意不收（查詢結果、搜尋欄位、安全檢查不到），版本說明寫了。
- 「什麼地方有月報連結」「什麼地方可以找到月報連結」：`question::terms` 先把開頭的「什麼」剝掉，
  切問句詞時剩「地方有月報連結」，`WHERE_WORDS` 的「什麼地方」永遠對不到，所以拿掉了。文件背景
  看過「地方」，空手；兩版一樣，版本說明寫了。
- 看過的修飾語是條件：「最新的月報連結」在文件背景空手（「最新」看過，索引不拿掉；切段那一步
  「最新 月報連結」兩段都是條件）。兩版一樣。「最新」沒看過的機器上改用「月報連結」、找到，
  `a_word_before_de_never_seen_can_be_just_which_one` 釘著。
- 簡體的「查询月报连结」「寻找月报连结」四份背景都空手：畫面是繁體的「月報連結」，沒有繁簡對照。
  兩版一樣，不是這一族。
- `scripts/check-no-network.sh`（61、223 行）與 `scripts/check-windows.sh`（28 行）也是
  `set -o pipefail` 底下的 `rustup target list --installed | grep -qx`，和這一版改掉的
  check-erased-db 同一種形狀。`--installed` 只印幾行，還沒撞過；check-no-network 在隱私那條
  範圍裡，這一版沒動。
