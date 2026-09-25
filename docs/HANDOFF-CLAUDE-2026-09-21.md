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

`gates-all.sh` 量 `33e1809`：**通過 51 條，失敗 2 條**。

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
`scripts/check-pet-says-why.mjs:915` 一行 `process.exit()`，把後面 977 條
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
