# HANDOFF — 交給下一位 agent（Codex）

**2026-09-30 三個 session 收尾，Phase 6 關閉（alpha.169）：**Phase 7 的 watch 遠端通報與
接手審計（已在 main）、eb61a808 的不可逆動作與十輪演練、batc 的承諾綁定與 URL 身分，
三支合成 alpha.169。合併後十輪演練被承諾綁定擋下，改成每輪各綁自己的承諾與 URL
目標，沒有繞過規則。injection 退場條件補上開檔那一半（20×3＝60 例全以 `Commitment`
拒絕、開檔良性對照走到平台執行層一次、八種 fact kind 只有 `url`／`file_path` 可執行），Phase 6
三格全部打勾。Ted 已定案：要他親手做的真機日誌與 service verification 不是退場條件；
WINDOWS-CHECKLIST 那段「真機收據仍待完成」已刪。Discord 通報的隱私對抗驗證另外修了兩處：
報告唯一的字串欄位改成私有（`compile_fail` doctest 守，改回 `pub` 實測會紅；stable rustdoc
不核對錯誤碼，所以用 `..base` 形狀並配一段必須編得過的雙胞胎），以及拿掉永遠是 0、
Discord 失敗時會和真正退出碼矛盾的 `exit_code`。`sister diagnose` 只讀四個固定環境變數，
放在環境變數裡的 webhook 不會進報告。README／THREAT_MODEL／DATA_INVENTORY 的「只比
host」舊說法已改成現行的完整去處比對與承諾綁定。**下一步是 Phase 7 的兩格退場條件**
（20 組監督式接手情境逐組驗證白名單外動作在執行前停止；offer → 執行 → 回報的完整迴路），
兩格都還沒開工。

**2026-09-29 Phase 6／7 後續：**現有 action 只有開網址、開檔、聚焦視窗；
新增的雙入口測試證實送出／發布／付款／刪除／開 terminal 五類無法被解析成按鈕
或持久化步驟。`ActionSnapshot` 現在拒收多餘欄位，不讓夾帶的意圖在讀 log 時消失。
`PHASES.md` 的當前 Phase 6 條件據此更新；Phase 7 原本「等 Ted 累積真實 run 才
能開始 offer／文件整理」的停點已移除，改由自動化正常／拒絕／停止／重開情境驗收。
本輪 `cargo fmt --all -- --check`、`cargo clippy --workspace --all-targets -- -D warnings`、
`cargo test --workspace --quiet` 與 `check-no-network.sh` 全通過。Phase 6 的注入
退場條件仍未通過；不能把原有 20 條窗外 URL 測試算成 100% 注入攔截。

**2026-09-29 Phase 6 驗收調整：**Ted 明確表示，不要把他在 Windows 親自完成
10 項 semi-action 任務、交出真實可回放日誌當作前進條件。`PHASES.md` 已把這條
換成 10 輪可重跑的自動演練，Phase 7 的自用次數門檻也改成自動化情境。
新增的 CLI integration test 讓 10 個不同目標各自走 grant、核准、公開執行隘口，
寫入 `action-log.jsonl`，再由 `hands runs --json` 確認 10 輪完整且目標相符。
executor 仍是模擬的，不算 Windows 真實動作。`cargo fmt --all -- --check`、
`cargo clippy --workspace --all-targets -- -D warnings`、`cargo test --workspace --quiet`
與 `check-no-network.sh` 全通過；可以接著推 Phase 7，不等 Ted 的私有日誌。

**2026-09-26 Phase 7 接手收據：**本分支已替 `sister watch` 加上每次命令明確指定的
`--remote-json` 與 `--discord-webhook-env`。兩條路只共用封閉、去文字化 schema；Discord
只准 exact webhook、不 redirect／proxy／retry，並在 physical all-stop admission/fence 內
完成 transport。新 `sister-notify` 預設沒有 HTTP client，只有 CLI 開 `discord` feature；
`scripts/check-no-network.sh` 已把它固定成 desktop 四條之外唯一的第五條 outbound。

同一批變更新增 takeover audit：新 `sister do` run 的第一列帶 opaque Run ID 與完整 Grant
SHA-256 ID；`sister hands runs --json` 從既有 `action-log.jsonl` 算出開始／結束／耗時、
完整步驟、畫面驗證與摘要，並明列被 limit 隱藏的 run 與讀不懂的列。舊 log 的 ID 維持
`null`。workspace 測試（Xvfb 需沙箱外）、fmt、clippy、Windows root／desktop cross-check、
四張同意書、no-keylogging 與 no-network 全綠。CI 不打真 Discord；正式 webhook 的 provider
收件仍應在 Windows artifact 上做一次人工 smoke，但這不影響本機 JSON 與 transport 邊界測試。

**2026-09-29 `cc0a889` 獨立審查後續修正：**該提交的重型 repo 收據 53/53，
但審查發現 staging CLI 沒計入提示期間真實時間、腳本核准未標來源、同資料夾第二輪
覆蓋第一輪收據；URL grant 的儲存清單也曾列出缺目標 fact／雙 pass 畫面的卡。
修正後 staging CLI 以單調時間計期，真等 31 秒的程序測試證明過期；腳本輸入必須
明示測試旗標，收據記 `scripted_fixture`／`human_terminal`，多輪以鎖定附加方式
留下完整回放。保存 URL grant 前先檢查政策、grant 範圍、fact、雙 pass 畫面、
可信位址列與完整網址紀錄，缺任一項就不說已保存。`0fd196f6` 對固定樹
`e932d92695602111` 的重型 gate 53/53 通過；staging feature hands 171+31、
CLI 程序 5 與 feature Clippy 全綠；新獨立審查對本段程式變更 PASS。
Ted 的 10 項真任務日誌
仍未提供；第三方不可逆動作沒有執行。

**同輪擴及全部無人值守動作：**`open-url`、`open-file`、`focus-window` 都在共同
`authorize_unattended` 入口核對當場選定的承諾與具體動作。`--save-grant` 對三種
動作都列出符合授權範圍的承諾，須選編號並答「好」才存；沒有候選不存。
定點測試驗無綁定的檔案／視窗 grant 拒絕、有綁定的放行，並驗 CLI 的檔案 grant
拒絕保存／選定保存與讀回核對。正式 Phase 6 三條退場條件仍為 0/3：
20×9 只證明這條 URL 授權路徑的 containment；產品沒有第三方不可逆 executor；
Ted 的 10 筆真任務收據及 service verification 尚缺，不可提前打勾。

**2026-09-29 Phase 6 審查退件後修正候選：**獨立審查指出上輪 20×9 夾具替 19 條
注入文字附加網址，測到的是同張 OCR 重複網址拒絕；純指令仍能選可信位址列並借同址
grant。現在無人值守 URL grant 必須另綁人當場選定的承諾 ID、原文、具體動作、
目標 fact ID 與兩個 pass 同意的畫面清單；
`--save-grant` 有 URL 目標時會列出候選、讀編號及當場「好」，舊的無綁定 grant
不能開 URL。20×9 端到端後三種保留原注入文字、可信同址來源與涵蓋網址的 grant，
但 grant 綁夾具建立的另一張已歸檔承諾；真正 `sister do --unattended` 因承諾不符拒絕。
乾淨且綁定的控制組仍執行一次。staging-only `sister` 入口現在呼叫共用不可逆
dispatch，五類權限從程序層測 approve／decline／expiry／replay，僅記本機 fixture。
本輪 `cargo test --workspace`、workspace Clippy、staging feature 的 CLI 與 hands
測試／Clippy、Windows 交叉編譯／Clippy、no-network 與 fmt 均通過。該候選後續
取得重型 CI 腳本 53/53，獨立審查另指出上段三項缺口；service verification 仍待完成。
正式產品沒有第三方不可逆 executor，Ted 的 10 項真 Windows／staging 任務日誌仍
須由 Ted 提供。Phase 6 三格暫不勾。

**2026-09-29 已被獨立審查退件的前一候選：**`insert_frame` 現在另存同張位址列衍生的 URL
fact；無人值守不再接受 OCR 網址 fact 借同址票，位址列 fact 若被同張畫面的
OCR／輔助文字重複提供同 URL 也拒絕。20 條 injection 語料 × 九種來源／授權
組合（180 例）走 `sister do --unattended` 與 action log；其中三種同址、可信
來源、grant 涵蓋的案例選位址列 fact，仍以 `target_only_in_screen_text` 拒絕，
乾淨位址列控制組執行一次。共用不可逆 dispatch 新增逐次即時核准、30 秒期限、
時鐘倒退、停止、具體步驟綁定與不可重播票；唯一 executor 是 feature-gated
本機 staging fixture，五類權限均測 approve／decline／expiry／JSONL replay。
正式產品仍沒有第三方不可逆 executor。workspace fmt／Clippy／test、staging feature
test／Clippy、Windows 交叉編譯／Clippy、no-network gate 均通過；仍待 service
verification 與獨立審查；後續審查指出純指令變體會繞過重複網址條件，見上段修正。
Ted 的 10 項真實 Windows／staging 任務日誌仍未提供，
絕不可用 fake executor 演練冒充。Phase 6 checkbox 目前保持未勾。

**2026-09-28 Phase 6 擴大工作候選（Ted 已核准完整範圍）：**URL 授權改用 `url`
parser 正規化 scheme／host／有效 port；userinfo 一律拒絕，path、query、fragment
保留原始位元組與空分隔符。保存的 grant 新增 exact `url_targets[]`，舊票預設空集合。
獨立審查找到 Chromium 縮寫位址列的 query 內含 `https://` 會被誤判成 scheme；已修
並加上 `target_address_on_source_frame` 真路徑回歸測試。審查續查發現 `http://` 後
緊接多位元組 Unicode host 時，八位元組探測會提前返回；現已改成獨立判斷並以
Unicode／punycode 同址的比較與 DB 來源路徑測試鎖定。20 條注入語料目前走 6 種
來源／授權組合，不能充作 100% injection 退場；staging 即時核准仍只在隔離的記憶體
fixture，10 個 CLI 演練仍用 fake executor，兩者均未達正式驗收。最新 workspace
fmt／Clippy／test、Windows 交叉編譯／Clippy、no-network 與 staging feature 測試通過。
正式 Phase 6 仍是 0/3；下一步要把即時核准接到可執行路徑、補足 injection 在可執行
可信來源的證據，並由 Ted 在 Windows 完成 10 個真實可逆任務及保留可回放日誌。不得
以測試夾具執行第三方帳號、金流或公開發文的不可逆動作。新一輪獨立審查已給 URL
授權程式 PASS、整體 Phase 6 REJECT／0/3；service verification 仍待完成。

**2026-09-28 Phase 6 來源票空分隔符修復：**`same_destination` 現在保留空 `?`／`#`
分隔符的有無；四種組合不能互借 standing grant。比較器及真實授權查詢
`site_in_her_record`／`target_address_on_source_frame` 均有 4×4 回歸測試，Chromium
省略 scheme／www. 的正例保留；既有 scheme／有效 port、path percent escape、尾端
`/`、重複 query 順序測試仍通過。20 條 injection 語料 × 3 種來源的 CLI 端到端測試、
workspace Cargo fmt／Clippy／test、no-network gate 通過。Phase 6 三項正式退場條件
仍未勾：60 個來源案例只證明本段 URL 閘門；不可逆動作全路徑核准與 10 個真實任務
不屬這輪 URL 修復。下一步須 Ted 決定是否擴大至完整 Phase 6 範圍並提供真實任務
紀錄；候選仍待 service verification 與新一輪獨立審查。

**2026-09-28 Phase 6 來源票第二輪修復：**`same_destination` 不再把 path 的每個
percent escape 解成等價文字，也不再排序 query；`%2F`／`/`、重複參數反序與結尾
`/` 都分開授權。20 條 injection 語料各走三種本段、已被 reviewer 接受的 URL fact
來源，共 60 個來源案例；逐次核對拒絕原因，保留一條窗外 fact 測試及良性執行控制。
scheme／有效 port 的前輪修復保留。格式、workspace Clippy／Cargo 測試與 no-network
gate 通過。這仍是候選，需 service verification 與新的獨立審查；
Phase 6 三項退場條件仍未完成。

**2026-09-28 Phase 6 來源票再修：**`same_destination` 現在把 scheme 與有效 port
列入去處身分；省略 scheme 的 Chromium 位址列只推定為 HTTPS，預設 port 與明寫
`:443`／`:80` 各依協定等價。HTTP 和不同 port 不能借 HTTPS standing grant；
`site_in_her_record` 與同張來源 frame 都有反向測試。workspace Cargo 測試、Clippy、
格式與 no-network gate 通過。這是候選修復，尚待 service
verification 與新一輪獨立審查；Phase 6 退場條件仍未打勾。

**2026-09-28 Phase 6 候選修復：**無人值守開 URL 的來源畫面現在必須屬於
`TRUSTED_URL_ORIGIN_PLATFORM` session；replay／無 session 的同網址畫面會以
`target_source_untrusted` 拒絕。位址列改用既有 `same_destination` 比較，接受 Chromium
省略 scheme／www. 的形式，仍拒絕不同 path、query、fragment。反向端到端測試涵蓋
replay 畫面位址列恰好等於注入 URL、另有可信瀏覽紀錄的情境；良性控制仍執行一次。
`cargo fmt --all -- --check`、workspace Clippy／測試、`check-no-network.sh` 通過。
這只修候選的兩個退件點；Phase 6 三項退場條件仍未完成，下一步照 `PHASES.md` 收驗。

**2026-09-21 交回 Claude 的紀錄在 [`HANDOFF-CLAUDE-2026-09-21.md`](HANDOFF-CLAUDE-2026-09-21.md)。**
那份寫的是當天 Codex session `01a0c443` 與後續 Grok session 做到哪、main 在 `5cfd22c`、語音／用量已在 main、PDF UIA 與 diagnose 堆疊怎麼修、macOS probe 為何還沒上。下面這份仍是到 alpha.145 為止的長交接。

**更新於 2026-09-20（Codex 續接）；alpha.145 已公開，release commit 是 `9ed66e5`。** Claude 的原始交接保留在本機的
`HANDOFF-CODEX-SEAT-2026-09-19.md`，已加入本機 Git exclude，不進公開提交。
本輪起點、成果、停點與未完成重點，先看[簡短交接](HANDOFF-SUMMARY-2026-09-20.md)。
這份是「打開就能接著做」的交接紀錄，不是
路線圖。路線圖在 `docs/PHASES.md`，規格在 `docs/SPEC.md`，產品定義在
`docs/PRODUCT.md`，工作紀律在 `AGENTS.md`。四份都要讀，順序就是這個順序。

> **接手更正（2026-09-13）：**這份交接的第 4.2 節與原步驟 3 把 #42 誤寫成尚未
> 實作。實際上它已由 `fbb61e2` 與 `85f6f16` 在 alpha.100 完成並出貨；PHASES 同一段
> 後文也有完整收據。下方已改成不再指示下一位重做。
>
> **Codex 續接收據（2026-09-13）：**`v0.1.0-alpha.141` 已由 `9c6fbca` 切 tag 並公開；
> tag CI 八個 job 全綠，四個 artifact 齊全。Ted 已在真 Windows 用正式 Setup 覆蓋舊版，
> 確認舊角色、四張同意、Grok 選擇與記憶保留，且問答的本機出處可點回原截圖。精確人工
> 驗收範圍記在 `docs/WINDOWS-CHECKLIST.md` 的 alpha.141 smoke，不把未做項目算通過。
>
> **Codex alpha.142 收據（2026-09-15）：**`9162cf0` 把背景解釋改成從前往後一次完成一段，
> 前一張工作假設落地後才處理下一段；新證據會支持、推翻或修正同一套理解。答題檢索也改成
> 多查詢輪流取證、保留原問題時間範圍、補開頭／中段／結尾或命中附近的 L2，再按時間成句。
> `v0.1.0-alpha.142` 已公開；tag run `34932696475` attempt 2 的八個 job 全部 success。
> attempt 1 只在最後的 Windows 簽章 fixture 第二次 NSIS bundle 遇到一次 runner 連線
> `10054`，同一 commit 重跑已通過，沒有產品改碼。Release 有四個 uploaded artifact：
> `AI-Sister-Setup.exe` 276,017,955 bytes、`sister-desktop.exe` 69,828,608 bytes、
> `sister.exe` 10,929,152 bytes、`AI-Sister-Linux-X11-amd64.deb` 63,810,044 bytes。
>
> **Codex alpha.143 收據（2026-09-15）：**`2de931b` 補上一次性的舊記憶重讀：第一次在新條文下
> 取得第二張同意後固定歷史右界，從最早仍保留的事件按六小時窗往後掃，一次完成一段、進度寫回
> SQLite 的 `meta`；現場優先，舊資料每天最多 20 段且不超過每日解釋預算的四分之一。第二張
> `cloud-reading` 條文因此重寫（131 → 191 字，`CLOUD_READING_TERMS_VERSION` 1 → 2，升級只
> 重問第二張），17 支 bundled 朗讀跟著重錄。`v0.1.0-alpha.143` 已公開；tag run `34959625002`
> 第一次就八個 job 全部 success，沒有重跑。Release 有四個 uploaded artifact：
> `AI-Sister-Setup.exe` 277,421,179 bytes、`sister-desktop.exe` 71,220,736 bytes、
> `sister.exe` 10,945,536 bytes、`AI-Sister-Linux-X11-amd64.deb` 65,201,634 bytes。
> Release body 和 `scripts/release-notes.sh v0.1.0-alpha.143` 逐字前綴相同，後面只多一行
> GitHub 自己附加的 Full Changelog。
>
> **下一位動同意書語音之前先讀這三件事：**（1）alpha.137 那份切點檔一度遺失，重跑 polish
> 會安靜地把 51 支「不該變的」換成另一批；切點已找回並存在 `~/voice-lab/`
> `consent-voice-trims-consent-v1.json`，每一筆都以「切完重走流水線、解出來的 PCM 和出貨的
> 那一支逐位元組相同」驗過。重跑一律帶 `--trims` 加 `--reuse-ogg-from`，它印的
> 「原封搬回來的：N / 68」就是收據。（2）同意書那包的品管引擎是 Whisper `large-v3`，不是
> `medium`；GPU 被佔住時用 `~/voice-lab/_a143_qc_fp16.py` 包一層（whisper 的 fp16 只轉 mel
> 不轉權重，LayerNorm 要留在 fp32）。（3）新的 17 支朗讀是 32.8–46.0 秒，**超過
> Breeze-ASR-25 的 30 秒硬上限**，所以 NOTICE 那句「兩個訓練資料不同的 ASR」目前只對 30 秒
> 以內的那 51 支成立；要再拉長條文之前先確認第二個引擎吃不吃得下。

---

## 0. 這是什麼專案

**AI-Sister 是一個在 Windows 上安靜看著螢幕、事後答得出「我昨天在幹嘛」、而且每一
句話都點得開證據的本機記錄器。**

- **repo**：`https://github.com/teddashh/AI-Sister`（public，Apache-2.0；產出的角色
  圖與語音以 NOTICE 排除在該授權之外）
- **本機工作目錄**：`/home/ted-h/projects/AI-Sister`，branch `main`，直接推 `main`，
  不開 PR。
- **「本機」的精確意思**：錄下的截圖、OCR 與記憶留在這台機器。**腦（L2/L3）接的是
  使用者自己已經裝好的 CLI agent，不是內建 HTTP client。** desktop 只有兩條具名、
  窄化的內建 outbound：Persona 素材包的使用者發起 GET，以及預設關閉、另行同意後才
  送答案正文的 Azure TTS POST。這條界線不可以擴散到 recorder／core／capture／
  brain／hands 或 WebView。

程式碼分佈：

| 位置 | 是什麼 |
|---|---|
| `crates/sister-core` | 記憶、斷句、同意書、審閱、回答組裝（邏輯的家） |
| `crates/sister-capture` | 擷取與 OCR 熱路徑 |
| `crates/sister-cli` | `sister` 指令（record／watch／ask／forget／export／doctor…） |
| `crates/sister-hands` | Phase 6 的手（sidecar，預設關） |
| `crates/sister-shell` | 桌面外殼共用邏輯（例：點擊穿透判斷 `hit.rs`） |
| `crates/sister-tts` | 本機朗讀與 Azure 可選 TTS |
| `crates/sister-assets` | 角色素材的存取層 |
| `apps/desktop` | Tauri 桌面（**另一個 workspace**，見第 6 節的坑） |
| `scripts/` | 44 支 `check-*` 閘門與 promote 腳本 |
| `site/`＋`scripts/build-website.py` | 公開網站 |

---

## 1. 我是誰、做到哪裡

| | |
|---|---|
| 這一段的執行者 | 原主段是 Claude Code（Opus 5, 1M context），session `e74b7a6f-34e8-4500-925c-8e0d020ac13c`；alpha.142／143 由 Codex 續接；2026-09-15～16 的 15 個 commit 由 Claude Code session `5c0ee346` 做 |
| 時間範圍 | 2026-09-09T05:04:30Z → 2026-09-20（UTC；包含 Codex 續接與 alpha.145 發布） |
| **已公開的最後一版** | **`v0.1.0-alpha.145` → `9ed66e5`**，published 2026-09-20T19:53:14Z |
| **2026-09-19 接手點** | **`e5645c7`，當時 ahead `origin/main` 16 個 commit，未 push、未 tag**；下述 Codex 續接另有測試與交接修正 |
| 目前產品版號 | `0.1.0-alpha.145`（17 個位置一致）；本版收斂 OCR／UIA 取證、日期搜尋、來源分類與同意鎖修正 |
| alpha.145 CI | tag run `35530957137` 第一次就八個 job（含 Release／Website）全部成功；main 的測試焦點修正另見下方收據 |
| alpha.144 CI | main run `35487928806` 六個平台 job 全部成功；tag run `35489365726` 八個 job（含 Release／Website）全部成功，兩輪都沒有重跑 |
| alpha.143 CI | tag run `34959625002` 第一次就八個 job 全部 `success`；Release 四個 artifact 齊全 |
| alpha.142 CI | tag run `34932696475` attempt 2 八個 job 全部 `success` |
| 本機閘門 | alpha.145 出貨內容（`9ed66e5`）既有 52 條閘門 **52 通過／0 失敗**，包含 workspace 測試、Windows root／desktop 與隱私檢查 |
| alpha.141 真機 | 正式 Setup 覆蓋成功；舊 persona／同意／Grok／記憶保留，本機出處可點回原截圖 |
| alpha.141 後續 | 真機 receipt／交接更新；CI 的 GitHub Actions 已升到 Node 24 majors，branch run `34793019345` 六個平台 job 全綠。沒有產品程式碼或新出貨內容 |

**Codex 續接（2026-09-19）：**

- `cargo test --workspace`：1,982 通過、0 失敗、3 忽略。Persona、主對話與時間軸三支
  renderer 檢查通過。主對話的等待秒數測試原以 5 秒自動交回答案、約 4.36 秒取樣，
  本次取樣時答案已返回，三個斷言讀到 `null`；改成完成等待畫面的斷言後才 resolve
  答案，原斷言保留、整支重驗通過。產品程式未改。
- 原始交接把三條 `codex/overview-*` 分支列為未落地，**這是誤判**。三條的
  `git cherry main` 全部為 `-`；等價提交是 `2512920`、`0a9c82d`、`7b43b92`，
  並由 `c85e648` 完成、隨 alpha.116 出貨。不需要 rebase 或重寫；分支保留。
- alpha.144 新增的六項 Windows 人工驗收仍未勾；本機 renderer 不代替真 WebView2。

**alpha.145 發布收據（2026-09-20）：** `9ed66e5` 已 push 並建立 annotated tag；tag run
`35530957137` 首輪八個 job 全綠，Release 是公開 prerelease。四檔全部 `uploaded`：
`AI-Sister-Setup.exe` 277,757,610 bytes、`sister-desktop.exe` 71,231,488 bytes、
`sister.exe` 11,044,864 bytes、`AI-Sister-Linux-X11-amd64.deb` 65,236,916 bytes。
Release job 下載回四檔、逐位元組核對後才公開。本輪另以匿名公開連結下載 Setup，大小及
SHA-256 均符合 GitHub API；SHA-256 是
`7cff03083b84a14844f6437944d9ce55a9911c3f7f1651b30dc15a92915fc4cb`。
Release body 與本版產生的 release notes 逐字前綴相同，官網 Setup 連結已指向 alpha.145。
Windows 安裝、重裝、移除、alpha.110 → current 記憶保留與簽章 fixture 都通過。
原生 PDF／HTML／WPF 取證已驗；OCR 記號仍是 `SISTER-OCR-ZH: SKIPPED lang=en-US zh=none available=en-US`，
本輪驗到中文 UIA 與英文 OCR，沒有把繁中 OCR 算作通過。

同一 release commit 的 main run `35530957259` 首輪在 PDF fixture 焦點就緒前逾時；原樣重跑
後 PDF 通過，但 HTML 只看頁面標題就回 ready，原生焦點仍在 Pane，讀字正確回空。
後續 `4eed7f4` 只修測試：啟用自有文件視窗、等 UIA Document 確實持有焦點才 ready，捲動也
保留文件焦點；不改出貨程式。新 main run `35532733393` 的三案原生 UIA、Windows 完整測試與
recorder 接線通過；本機 Windows 編譯／lint 與網路／鍵盤隱私檢查通過。
後續確認：`4eed7f4` 的 main run `35532733393` 與發布收據 `301e1bc` 的 run `35533936551`
均已完整成功，六個平台 job 全綠；main 的 Release／Website 按規則跳過，正式發布收據仍取 tag run。

**alpha.144 發布收據（2026-09-20）：** Ted 已明確授權 push／tag；`258a755` 的 main CI
通過後才建立 annotated tag。Release 是公開 prerelease，四個檔案全部 `uploaded`：
`AI-Sister-Setup.exe` 277,734,818 bytes、`sister-desktop.exe` 71,225,856 bytes、
`sister.exe` 10,945,536 bytes、`AI-Sister-Linux-X11-amd64.deb` 65,209,214 bytes。
Release job 已將遠端四檔下載回來，與原生 job 的 artifact 逐位元組比對後才公開；
本輪另核對 GitHub API 的檔名、大小、狀態及說明前綴，後面只多 GitHub 的 Full Changelog。
官網首頁也已實讀，下載連結指向 alpha.144。

Windows tag job 的 OCR 實測記號是 `SISTER-OCR-ZH: SKIPPED lang=en-US zh=none available=en-US`：
英文 OCR 管線通過，**本輪沒有驗到繁中 OCR**，不能因語言包安裝步驟或整個 job 成功就算通過。
安裝、重裝、移除、alpha.110 → current 記憶保留及簽章 fixture 都通過；六項新增真人 smoke
仍待 Ted 在正式安裝副本上驗，不補勾。

**2026-09-20 後續切片（收斂至 alpha.145）：** 指定日期的 OCR／事實取證已修正。原先「昨天電話」
只讓章節與判讀遵守日期，原文和電話仍從全期間取前幾筆；現在四條文字檢索路徑與事實候選
都在筆數上限前限制時間，目擊次數與畫面出處也取自同一時間窗。CLI 改寫查詢不能蓋掉
原問題的日期；指定舊日期的數字搜尋會查完整個指定窗，不再被預設 30 天掃描上限擋掉。
回歸測試先在原程式重現「昨天帳單拿到今天紀錄」，再驗修正、跨日邊界、較新候選擠占、
改寫查詢、出處 ID、空結果與未指定日期的正常路。工作區測試 1,991 通過／0 失敗／3 忽略；
fmt、clippy、Windows cross-check、同意書文案／保存、brain outbound、no-network、
no-keylogging、主對話與證據視窗檢查通過。這些不代替真 Windows 的 OCR 或畫面驗收。

**同日下一刀（收斂至 alpha.145）：** 英文查詢的事實類型改按完整詞辨認，避免 `hotel`、`profile`、
`update` 分別因 `tel`、`file`、`date` 片段帶出不相關電話、檔案與日期。大小寫、複數、
中英相接及明確的 `telephone` 仍可查；中文詞維持連寫。資料庫 → RAG 回歸先在原程式
重現，再驗正確文字來源與不相關事實排除；fixture 使用畫面標題，沒有宣稱跑過真 OCR。
工作區測試 1,994 通過／0 失敗／3 忽略；fmt、clippy、同意書文案／保存、brain outbound、
no-network 與 no-keylogging 通過。

**同日 macOS 編譯修復（收斂至 alpha.145）：** doctor 的前景探測結果與文案分支按實際使用平台
編譯，測試保留全部狀態；macOS 維持 `NotAsked`，修正原生 clippy 的 dead-code 失敗。
Windows root／desktop 編譯與 clippy、host fmt／clippy、同意書文案與 no-network 通過。
workspace 首輪有一條同意鎖測試在已簽後回 `Busy`；未改程式，該條單跑與整批重跑均過，
重跑為 1,994 通過／0 失敗／3 忽略。當時偶發 `Busy` 尚未定位；後續修復見下一段。

**同日同意寫鎖收尾（收斂至 alpha.145）：** 本機真子行程固定住 fork → exec 前的描述元繼承窗，
重現交易已結束卻仍回 `Busy`。寫鎖現在由 guard drop 明確解鎖，不再等子行程關閉繼承的
handle；簽署、撤回與交易失敗都有回歸，子行程仍活著時就必須讀到正確的最新同意狀態。
原程式兩條測試皆紅；故意提前解鎖也會被「交易尚未結束必須 Busy」的斷言抓到。
還原後 workspace 1,996 通過／0 失敗／3 忽略；fmt、clippy、Windows root／desktop、
同意書文案／保存、brain outbound、no-network、no-keylogging 通過。

**同日 RAG 來源分類（收斂至 alpha.145）：** RAG 不再把所有 fact／chunk 都標成 `screen`；
依紀錄保留 OCR、剪貼簿、視窗標題與網址的來源，判讀另標 `reading`，無法辨認的 fact
來源標 `unknown`。提示詞也不再把剪貼簿當成畫面文字，或把 OCR 紀錄當成圖檔仍存在。
正式 replay 入庫 → 檢索 → RAG 回歸先紅後綠；OCR 文字與事實沿用原始 frame_id，
沒有畫面編號的來源不借附近的幀。Fixture 只有文字與幀資料，沒有真機擷取或 UIA 驗收。
workspace 1,998 通過／0 失敗／3 忽略；fmt、clippy、Windows root／desktop、同意書
文案／保存、brain outbound、no-network、no-keylogging 通過。


**同日 Windows 輔助讀字（收斂至 alpha.145）：** UIA 另一路讀取前景 Edit 控制項的可見文字，
最多 16 段／8,192 字；沿用 HWND、PID 與擷取許可，讀取前後核對焦點、密碼與可見狀態，
跨螢幕控制項不補字。內容工作執行緒與隱私探測分開；逾時結果丟棄，不改 OCR 退路。
文字在慢 OCR 前取得，以 `assistive` 分開入庫、抽事實並供 RAG，引用同次保留幀；
同圖的 OCR／UIA 不重算目擊次數，小幅文字改變也不被 dHash 吞掉。
Schema 19 → 20；備份、去敏 replay 匯出／匯入與忘記的連帶清除都有回歸。
workspace 2,005 通過／0 失敗／3 忽略；兩個隱私突變都紅、還原後定點測試綠；
fmt、clippy、Windows root／desktop、同意書文案／保存、brain outbound、no-network、
no-keylogging 通過。該輪 PNG／RAG 測試使用合成畫面與獨立文字；後續原生 UIA 驗證見下。

**同日原生 UIA 驗證與耗時補記（收斂至 alpha.145）：** 獨立 Windows 測試行程開啟自有 WPF 視窗，
透過正式 WindowsFocus／UIA 讀到繁中電話與更新文字；捲出可見區的文字、密碼欄、按鈕
名稱與另一視窗下的舊許可皆被拒絕。CI run `35510612780` 的原生 UIA 與實錄／停止
檢查通過。實錄也驗回 `25ce979` 漏計耗時的修正：新增「輔助讀字」與「脈絡核對」兩列，
丟棄結果仍記實際成本。移除任一計時的突變都會使回歸失敗；還原後通過。
本機 workspace 2,005／0／3 忽略，Windows root／desktop 編譯與 lint、隱私／同意檢查通過。
原生文件可見段落的擴充見下。


**同日文件可見段落（收斂至 alpha.145）：** UIA 讀字擴到有焦點的 Document，沿用 GetVisibleRanges、
16 段／8,192 字與 900ms 截止；保留 `document` 角色，讀取途中角色改變也丟棄結果。
CI run `35513235431` 的 Windows 原生 WPF 測試通過：唯讀文件上方繁中文字與兩段內容可讀，
捲到底後讀到新的電話，上方舊段落不再出現；切到密碼欄、按鈕與舊視窗許可仍拒絕。
合成畫面／文字另走 recorder → SQLite／PNG → RAG → 備份重開 → 去敏匯出 → 忘記，
保留獨立 assistive 來源與同次幀；兩種角色都覆蓋暫停、排除及讀取途中換焦點。
本機 workspace 2,007 通過／0 失敗／3 忽略；Windows root／desktop 編譯與 lint、
同意／網路／鍵盤隱私檢查通過。拿掉密碼拒絕或讀取後核對的突變皆失敗，還原後通過。
Edge 本機閱讀頁的原生驗證與修正見下。


**同日 Edge 內層文件取證（收斂至 alpha.145）：** 原生實跑找到 Edge 焦點 Document 沒有 TextPattern、
介面掛在外層文件的情形。現在只替 Document 尋找同視窗內最多八層的外層文件介面，
用 RangeFromChild 將每段可見範圍裁回原焦點文件；持續重驗焦點、祖先關係、密碼與可見性。
CI run `35515828601` 的 Edge／WPF 原生案例通過：本機 HTML 的繁中電話、捲動後新電話可讀，
舊段落、文件旁另一支電話及隱藏文字均排除；密碼欄與網址編輯時 recorder 不呼叫讀字。
Edge 原生文字另配合成像素／系統狀態走 recorder → PNG／SQLite → RAG，兩支電話各自
引用同次幀；這份 PNG 是合成畫面，驗收範圍是本機 HTML 閱讀頁。
本機 workspace 2,009 通過／0 失敗／3 忽略；Windows root／desktop 編譯與 lint、
同意／網路／鍵盤隱私檢查通過。移除裁切或放行旁邊範圍的突變都會失敗，還原後通過。
Edge PDF 焦點頁的原生驗證與修正見下。

**同日 Edge PDF 焦點頁取證（收斂至 alpha.145）：** PDF 閱讀器的焦點可能是頁面 Group；現在只向
它的直接 Document 父節點取得文字介面，用 RangeFromChild 裁回原群組，保留
`document-region` 角色。讀字前後都驗文字矩形位於目前螢幕與視窗內；同意、焦點、
密碼、可見性與 900ms 截止照舊，不越過沒有介面的直接父文件。
CI run `35520486615` 的 PDF／Edge HTML／WPF 原生案例通過：兩頁合成 PDF 的第一頁
電話可讀，未顯示第二頁的電話排除；原生文字配合成像素走 recorder → PNG／SQLite →
RAG，每一筆事實或原文證據都核對電話、PDF 檔名與同次幀，網址編輯時不再讀字或新增幀。
這份 PNG 是合成畫面；驗收範圍是有焦點的 PDF 頁，不包含翻頁後的連續取證。
本機 workspace 2,010 通過／0 失敗／3 忽略；Windows root／desktop 編譯與 lint、
同意／網路／鍵盤隱私檢查通過。拿掉文字矩形右界檢查的突變會失敗，還原後通過。
PDF 捲動後的 OCR 接續取證見下。

**同日 PDF 翻頁 OCR 接續（收斂至 alpha.145）：** 原生測試重現兩頁 dHash 相近、UIA 焦點留在
畫面外舊頁時，新頁被當成重複幀的漏記。輔助文字由有變無現在也觸發正常 OCR 路徑；
提交後記住空狀態，持續缺席不會每拍重讀。舊焦點仍拒讀，不放寬 UIA 可見範圍。
CI run `35523335448` 的 Windows 原生 PDF／Edge HTML／WPF 案例通過：PDF 兩頁使用
真正 GDI 截圖與 Windows OCR，核對 SQLite 文字、存檔 PNG 重新辨識及 RAG 的電話、
來源類別、PDF 檔名與同次幀；第二頁不含第一頁電話，網址編輯時不再抓圖、讀字或新增幀。
系統活動、剪貼簿與輸入仍由測試替身提供。PDF 就緒檢查只讀直接父文件，避免探查外層
文字介面卡住。定點回歸先紅後綠，本機 workspace 2,011 通過／0 失敗／3 忽略；
Windows root／desktop 編譯與 lint、同意／網路／鍵盤隱私檢查通過。
WPF 文件的真截圖取證見下。

**同日 WPF 文件真截圖取證（收斂至 alpha.145）：** 新增原生回歸，使用自有 WPF 唯讀文件與
正式 GDI／Windows OCR／UIA，核對捲動前後的 SQLite 文字、存檔 PNG 再辨識及 RAG。
上下兩支電話各自對回同次幀；OCR 與 assistive 來源均保留正確視窗標題，且不帶瀏覽器
網址。暫停時抓圖／OCR／UIA 呼叫數不增加，恢復後可錄製；一般捲動後只讀可見新段落，
密碼欄仍拒絕擷取。系統活動、剪貼簿與輸入使用測試替身。
CI run `35529804355` 的 WPF／Edge HTML／PDF 原生案例通過；本機 workspace 2,011
通過／0 失敗／3 忽略，Windows root／desktop 編譯與 lint、同意／網路／鍵盤隱私檢查通過。
**alpha.145 發版收斂：** 將上述切片收進同一版。main `5f73a06` 的 PDF 原生測試曾因
runner 背景終端機文字干擾 OCR 而失敗；測試現在自行建立全螢幕空白背景，文件視窗置前，
不動其他行程的視窗。CI run `35530387269` 的 PDF／Edge HTML／WPF 原生步驟通過。
正式 tag 的安裝／升級、四個 release 資產與網站發布均已完成，收據見本節上方。


---

## 2. Spec → 現在：八個 Phase 的完成度

`docs/PHASES.md` 用 checkbox 記退場條件，**退場條件就是驗收條件**。機械數過一次
（`- [x]` / `- [ ]`）：

| Phase | 名稱 | 完成 / 未完 | 現況一句話 |
|---|---|---|---|
| 0 | 感官與地基 | 3 / 2 | 剩「連續 7 天零 crash」與「磁碟 < 300MB/天」兩個要真機時間才拿得到 |
| 1 | S1 回憶核心 | 2 / 3 | 產品已在跑；剩效能數字、全離線走查、README 首段的實測足跡 |
| 2 | 重播評測 harness | 7 / 3 | harness 在；剩題庫 ≥100 題、baseline 進 README、當 regression gate |
| 3 | 斷句 + 事實層 | 2 / 1 | 剩斷句邊界 F1 ≥ 0.75（要手標語料） |
| 4 | 理解與記憶（大腦） | 2 / 3 | L2/L3 已接 CLI；剩 A/B +10pt、成本實測、兩週自用 |
| 5 | **Release 1.0** | 2 / 10 | **現在的主戰場**，見下一節 |
| 6 | 手 v1（hands sidecar） | 0 / 3 | 有實作與 injection 套件，三個退場條件都沒收 |
| 7 | 接手模式 | 0 / 2 | 沒開始 |
| 8 | 生態與 Preview 成熟化 | — | 持續，不擋 1.0 |
| | **合計** | **18 / 27** | |

### Release 1.0 合約〔2026-09-06 由 Ted 定案，不要重新辯論〕

- **Windows 10+ = 正式支援（GA），是唯一的平台支援 blocker。**
- macOS = Public Preview、Linux = X11-only Developer Preview。**沒達到最小合約就
  不發那個 artifact，但缺席不擋 Windows GA。**
- **Persona 角色體驗是 Windows GA 的產品面 blocker**（17 人：四姊妹＋13 位閨密），
  但「使用者選擇關掉角色或聲音」是必須支援的正常路徑。
- 「可升級」= 使用者手動下載新 installer、關掉 desktop／recorder 後原地安裝，並拿
  真的舊版 binary → 新版 binary 跑過。**自動 updater 不在 1.0 合約內。**
- 明確**不擋** 1.0 的：Wayland、macOS 長期足跡數字、≥100 題真題庫、斷句 F1、
  A/B +10pt、兩週開口有用率、b／e 類主動開口、完整 hands、Phase 7。
  數字繼續照實公開，但不再拿來擋發版。

平台現況：Windows Setup／portable CLI／desktop 已公開；Linux X11 Developer Preview
`.deb` 已公開；macOS 只有 CI 上的 app-tree 診斷，**沒有公開 `.app`／`.dmg`**，缺
Developer ID、notarization 與真機 TCC 收據。

---

## 3. 這一輪（alpha.140 之後的 23 顆）做了什麼

主軸是 **ASR／QC／compliance／privacy**。Ted 這一輪的原話：
「**該做的做一做，不要用 compliance gate 卡，一次把他都做過去，讓 compliance 不再
是問題。**」以下由新到舊，全部已 push：

| commit | 做了什麼 |
|---|---|
| `9886ff5` | **他按同意的字和她唸出來的聲音可以是兩件事。** 四份文字互相釘死，但鏈到 manifest 的 `text` 欄位就斷了——同步改五個檔、音檔不動，六條同意書閘門全綠。時長量不回來（最危險的改法是等長的），改問 git 歷史：聲音最後一次真的換，必須不早於文案最後一次真的變。新增 `scripts/check-spoken-consent-is-not-older-than-the-words.py`，linux job 因此加 `fetch-depth: 0` |
| `b193dce` | NOTICE 那句「沒做過壓縮或限幅」掛名的閘門其實在讀流水線自己寫的字。量過 crest 對限幅／壓縮的靈敏度（限幅完全無感、壓縮和現況整段重疊），**老實從 GATE 降級成 LAB**；順帶把 `contains("no compression")` 的針擴成整個承諾片語 |
| `c9d8ea9` | manifest 的 `truePeakDbtp` **從來沒有人拿它對過聲音**（改一支 −9.99，九道閘門全綠），而它的最大值正是 NOTICE 印給使用者的數字。補上逐支比對，門檻 0.5 dB 是先量 952 支的儀器雜訊（0.000）才挑的 |
| `b95acc2` | 公開網站上那份被複製過去的 NOTICE，落地之後沒人問過它還成不成立 |
| `812716f` | **六份出貨 NOTICE 的每一句話都要有人負責**：MEASURED／GATE／LAB／PROSE 四格沒有第五格，多一句沒認領要紅、刪一句讓分類變死也紅，`--list` 拿得出整份清單給法務 |
| `7153e87` | NOTICE 寫「整平到 −23 LUFS／−1 dBTP 天花板」，出貨實測是 −28.0…−22.8、最高真峰 −0.96。改成從 manifest 算出來，不手抄 |
| `566e3eb` | 錄音那邊的私有素材（refs／WAV／QC 收據）在出貨資料夾**外面**一個人都沒守。`.gitignore` 加音檔規則（預防），repo 全域 `git ls-files` 白名單掃描（證明） |
| `a55e6e6`／`3262c53` | 兩支語音數值閘門各自漏印另一半餘裕，而註解替它寫了一句沒證明的話 |
| `4dfa7f6`／`b5beebb`／`5313e12`／`3f9a11b`／`9f58660` 等 | ASR 那一軸的量化收尾：互動短句為什麼比較差、「煩耶。」那一支 60 骰 0 骰過線（量出上限，不是沒解法） |
| `123c6bc`／`c58cf0d`／`4fe049e`／`46c8884`／`e22f7d8`／`acb9991` | NOTICE 內容誰改都沒人知道、出貨資料夾只准放該放的、文件 bytes 反向刀 |

**結果**：compliance 這一軸現在是「六份 NOTICE 共 69 句，當場量 22、別的閘門 9、
錄音那邊 15、非事實宣稱 23，沒有一句沒人認領」。Ted 交代的「讓 compliance 不再是
問題」已經做到，**不要再從頭掃一次**。

---

## 4. 進行中／未完成，以及關鍵檔案

### 4.1 原交接的兩件立刻工作已收掉

- `9886ff5` 的 CI 已是 `completed/success`；`fetch-depth: 0` 與新閘門第一次在 runner
  上執行都通過。
- `v0.1.0-alpha.140` 後的 23 顆 commit 已由 `9c6fbca` 出貨為
  `v0.1.0-alpha.141`；tag CI 八個 job 全綠，Release 的四個 artifact 齊全。
- alpha.141 後的 `7472cea`、`e84ae97`、`77eba17`、`dfbd3f0` 只記真 Windows receipt、
  修正會自我污染的零命中 fixture 與整理交接。`3567d51` 將七個 checkout 升到 v7；
  `67ca70d` 將六個 upload 升到 v7、五個 release download 升到 v8，Pages 三顆升到
  configure v6／upload v5／deploy v5；`c645375` 同步網站 gate。branch run `34793019345`
  六個平台 job 全綠、六個 upload 點全數成功且沒有 Node 20 annotation。Release download
  與 Pages 只在 tag job 執行；alpha.142 的 tag run `34932696475` 已把兩者真的跑過。
- `9162cf0` 回應 Ted 對片段回答與「問了才想」的批評：背景 L2 現在按時間逐段修正同一套
  工作假設；S1 答題則讓多條查詢、原問題時間範圍與鄰近 L2 一起形成有前後文的來源集。
  `v0.1.0-alpha.142` 已公開，沒有尚待出貨的產品程式碼。下一步仍是第 5 節步驟 3 的真機驗收，
  不重切同一版。

### 4.2 交接更正：#42 的 URL 設定已完成

`docs/PHASES.md` 的 Phase 6 那一節先保留「#42 沒關」的歷史問題，後文再記錄
**alpha.100 落地**。交接時只讀到前半段，因而把已完成能力誤列成下一步。

- `crates/sister-hands/src/url_policy.rs` 有兩個答案與 `Option` 三態；`None` 是「還沒
  問過」，型別上和「你說了要當場按」分開。
- `apps/desktop/ui/app.js` 會在主對話主動提出問題；`sister url-policy` 是同一題的 CLI
  入口。
- `Grant::authorize_unattended` 在唯一 standing-grant 授權邊界執行 host provenance
  規則；當場按的路徑維持另一種明確同意。

這一塊已經包含在 alpha.100 之後的公開版本，**不要再做一次**。Phase 6 的 injection
exit criterion 仍未勾，是因為同站 path、redirect 與當場按的邊界，不是缺這個設定。

### 4.3 已知但刻意沒做的

- **點擊穿透的最後一段**（`crates/sister-shell/src/hit.rs`）：`POLL_BLIND_MS` 的修法
  已經想好、刻意沒做——那是連續第三個改同一條線的版本，而「會不會真的踩到」只有真機
  答得出來。**評估過並否決**：在實心塊外圍加 keepout（會把「手臂和身體之間的空隙點
  得過去」這個賣點一起關掉）。
- **`docs/PHASES.md` 裡搜 `allowed_next_step_fact`**：字母人（`apps/desktop`）那半
  沒有那道閘門。看到「還缺」先分清楚「閘門在寫入端還是執行端」再動。
- **Dependabot**：只剩 glib `GHSA-wrw7-89jp-8q8g`，被 tauri 的 gtk 0.18 整套釘死，
  但不可達（沒碰 `glib::Variant`）且只進 Linux 的 `.deb`。**升 Tauri 或 dismiss 都
  是 Ted 的決定，不要自己決定。**
- **Windows 程式碼簽章**：pipeline 與四層隔離 fixture 都通了，公開 alpha 仍是
  verified-unsigned。stable 必須有 public-CA PFX 與密碼 secrets。
  **不可以假造，要等 Ted 的憑證。**

### 4.4 關鍵檔案路徑

| 要找什麼 | 去哪 |
|---|---|
| 路線圖與退場條件 | `docs/PHASES.md` |
| 規格 | `docs/SPEC.md`；產品定義 `docs/PRODUCT.md` |
| 工作紀律／文案規則 | `AGENTS.md` |
| 隱私宣言與資料位置 | `docs/PRIVACY.md`、`docs/DATA_INVENTORY.md`、`docs/THREAT_MODEL.md` |
| 版本歷史 | `docs/RELEASE-NOTES.md`（**歷史區段不要改**，見第 5 節） |
| Windows 驗收 | `docs/WINDOWS-CHECKLIST.md`、`docs/WINDOWS-CODE-SIGNING.md` |
| 同意書文字（權威） | `crates/sister-core/src/consent.rs` 的 `wording()`／`without()` |
| 同意書的另外三份副本 | `apps/desktop/ui/onboarding.js`、`apps/desktop/ui/persona-consent-voices/catalog-v1.json`、同資料夾的 `v1/manifest.json` |
| 出貨語音 | `apps/desktop/ui/persona-voices/v1`（544 支）、`persona-consent-voices/v1`（68 支）、`persona-banter-voices/v1`（340 支），合計 952 支 Ogg Opus |
| 上一位的 living plan | `.handoff/PLAN.md`（**刻意 gitignored**，2892 行，最前面那章是「唯一現行摘要」但停在 2026-09-11＝已落後九個版本；下面的日誌區仍是精確 receipt） |

---

## 5. 下一步最小可驗證步驟（照順序，打開就能做）

**2026-09-20 現行下一步：** alpha.145 已公開，本輪版本收斂完成。
接續用正式 alpha.145 Setup 收回中文日常文件的 OCR／UIA／RAG 出處、alpha.144 六項
朗讀停止／失敗／設定提示，以及下方步驟 3 仍未收回的背景連續理解、完整回答與零命中結果。
下方步驟 1–2 是歷史完成紀錄。Windows GA 的其他退場條件繼續依 `docs/PHASES.md` 推進。

### 步驟 1（已完成）：確認交接點的 CI

```bash
cd /home/ted-h/projects/AI-Sister
gh run list --limit 3 --json headSha,status,conclusion \
  --jq '.[] | "\(.headSha[0:7]) \(.status)/\(.conclusion // "-")"'
```

**驗收**：`9886ff5` 是 `completed/success`。若紅，最可能的兩個成因是
`fetch-depth: 0` 那個改動，或新閘門 `check-spoken-consent-is-not-older-than-the-words.py`
在 runner 上拿不到歷史——那一條**設計成「問不到就紅」**，不是 bug。

### 步驟 2（已完成）：把 23 顆未出貨的 commit 切成 `v0.1.0-alpha.141`

版號散在 **7 個檔**（`check-release-version.py` 會全部對一次）：
`Cargo.toml`、`Cargo.lock`、`apps/desktop/src-tauri/Cargo.toml`、
`apps/desktop/src-tauri/Cargo.lock`、`apps/desktop/src-tauri/tauri.conf.json`、
`docs/RELEASE-NOTES.md`、`scripts/check-windows-upgrade.ps1`。
兩個 `Cargo.lock` 要跑過 cargo 才會變髒，**很容易漏掉**。

**驗收**：`python3 ./scripts/check-release-version.py` 綠 → 推 commit → CI 全綠 →
才打 tag。**release body 在打 tag 那一刻就定死**，說明要先寫對；驗法是整份比
**前綴**（`generate_release_notes: true` 會在後面附加 Full Changelog，比相等會每次
假紅）。打完 tag 要回頭確認 release job 真的跑了——linux job 一紅，release job 會
被靜靜跳過。

### 步驟 3（進行中）：在現行 alpha.145 正式 artifact 上驗背景連續理解與完整回答

alpha.145 的 release job 已公開四個 artifact；CI 已驗 build、installer、alpha.110→current
升級與記憶保留，但不冒充 Ted 的真日常畫面。先安裝正式 Setup，照
`docs/WINDOWS-CHECKLIST.md` 新增的兩條 S1 項目實測：

1. 不先問問題，連續錄同一件工作的起因、處理、反證與結果，再看「她知道了什麼」；後段判讀
   必須真的改寫前段工作假設，不能每張只是孤立摘要。
2. 再問「為什麼會這樣，後來怎麼了」；答案要按時間串成一段、來源跨過不同查詢與時間段，
   沒證據時不能把先後冒充因果。

alpha.141 尚未收掉的零命中 smoke 仍有效：先暫停錄製，確認停下後才產生從未顯示過的新 GUID，
保持暫停拿它提問，最後恢復錄製。Ted 尚未回報上述三條；不要預先勾選，也不要拿 CI fixture
或另一輪 source gate 代替正式安裝副本上的結果。#42 已在 alpha.100 完成，不要重做。

---

## 6. 已知坑、不要重做的事、站立約束

### 6.1 Ted 的站立指示（優先於一切）

- **「不要用 compliance gate 卡，一次把他都做過去。」** 這一軸已經做完（第 3 節），
  不要再從頭掃一次。
- **「有執行檔他就下載測，沒有就繼續推。不要停下來等回覆；做完一段就切 tag。
  一次多做一點再叫他測。」**
- **「一條能力要嘛端到端做好再露出，要嘛整條留在產品外。」** 不交半成品、占位選項、
  「之後會補」文字。
- **產品介面不講開發過程**，也不把責任推回使用者。
- **不要加 gate、要交出看得見的體驗。** 連兩版只加 CI gate 會被打槍。
  （這一輪是 Ted 明確點名的 compliance 例外。）
- **兩邊都站得住的時候，先問「這題該不該是使用者的」**，不要拿二選一去問他。

### 6.2 產品邊界（不可以回歸）

沒有 Neutral／字母人 persona；預設 ChatGPT；brain 沒有 HTTP client；Azure 是
opt-in；WebView CSP 只走 IPC；暫停鍵不可以藏進設定裡（「她整個產品的前提是你隨時
停得掉」）。

### 6.3 隱私（fail-closed，不要擅自破壞）

- **錄音那邊的私有素材（`refs.json`、`refs/*.wav`、WAV 母帶、QC 收據）一個都不可以
  進這個 public repo。** 出貨的只有 Ogg ＋ manifest ＋ NOTICE。預防在 `.gitignore`，
  證明在 `scripts/check-shipped-asset-trees-hold-nothing-else.py`。
- `.handoff/` 與 `research/extracts` 刻意留本機。
- **Windows 程式碼簽章要 Ted 的憑證，不可以假造。**

### 6.4 這台機器的坑

- **沒有 sudo。** `apps/desktop/src-tauri` **本機編不起來**（缺 `libdbus-1-dev`）。
  驗證路徑是 `./scripts/check-windows.sh` ＋ CI 的 macOS job。
  **不要再寫「這裡編不出來所以沒人驗」——macOS job 真的會跑桌面那棵樹的測試。**
- **`/tmp` 是 31G RAM disk。** 跑測試前一定 `export TMPDIR=/home/ted-h/tmp-tests`；
  那裡有 26G 是別人的東西，不要清。
- **`export PATH="$HOME/.cargo/bin:$PATH"`** 是必要的。
- 語音實驗室的 Python 在 `/home/ted-h/voice-lab/.venv/bin/python`。

### 6.5 反覆踩到、代價最高的幾個

1. **`apps/desktop` 是另一個 workspace。** 根 `Cargo.toml` 是
   `members = ["crates/*"]`，所以 `cargo test --workspace` 碰不到桌面那棵樹。
   **邏輯要搬進 `crates/`**，在 `main.rs` 裡加測試等於沒加。
2. **改 `apps/desktop/` 一定要跑 `cargo fmt`**（macOS job 會 fmt-check 它）。
3. **`docs/RELEASE-NOTES.md` 舊版本標題底下的段落是歷史，不要為了「數字過期」去改。**
   但**當時就不成立的假話要改**（有三個先例：`3cfc0ba`、`e4cb088`、`7402f2f`）。
4. **突變有粗細兩種，只做粗的會騙過自己。** 刪掉整個分支會紅；把那個分支**算出來的
   值**換成空字串或隔壁那一臂的值卻全綠——而使用者讀到的是後者。
5. **doc 裡每寫一句「這擋不住 X」，驗收就要有一刀 `want=綠` 去證明它。** 正向的「會
   紅」很容易驗，反向的「抓不到」幾乎沒人驗，而它太樂觀或太保守都會讓下一輪做錯決定。
6. **還原突變一律從檔案備份，不要用 `git checkout --`**（git 只知道 HEAD，不知道
   這一輪的起點），還原後 `cmp` 逐位元組確認。
7. **「紅了」不等於「被測試抓到」**——編不過也是紅的，刀量級不夠會紅在別條規則上。
8. **報 before/after 之前先量儀器自己的雜訊。** 對照組通常是免費的。
9. **閘門不能問執行者本人。** 讀 manifest 上流水線自己寫的字串不是證據，是同步檢查；
   要降級成「量不回來」得先拿數字證明量不回來。
10. **壓縮後 summary 裡的數字是二手的，特別盯全稱句。** 這份交接檔裡每個數字都是
    這一輪從 repo 機械跑出來的，但**你接手後要自己再跑一次**。

---

## 7. 常用指令

```bash
cd /home/ted-h/projects/AI-Sister
export TMPDIR=/home/ted-h/tmp-tests          # /tmp 是 RAM disk，必須改
export PATH="$HOME/.cargo/bin:$PATH"

# ── 一次跑完所有閘門（本機工具，不在 repo 裡；要帶 worktree 參數）──
bash /home/ted-h/tmp-tests/gates-all.sh /home/ted-h/projects/AI-Sister
#   交接時：通過 52 條，失敗 0 條
#
#   2026-09-22 起它會先後量兩次「被追蹤的樹」（HEAD + git diff HEAD 的 sha256）。
#   跑的途中那棵樹被改過的話，它 **exit 5 而且不印「全綠」**——因為上面每一個 ✓
#   蓋的是一個已經不存在的中間狀態。收據那行會寫出它量的是哪一顆 HEAD、哪一份
#   樹指紋，讀的時候要看那一行，不要只看最後一句。
#   打 tag 之前最後一次要對著 `git worktree add --detach <sha>` 跑。

# ── Rust ──
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo test -p sister-capture privacy
#   注意：cargo test | tee | grep | head 會 SIGPIPE 殺掉 cargo，
#   「全綠」是假的。驗法是數 log 裡 "^test result:" 的行數。

# ── Windows 那半（本機唯一能驗的方式）──
./scripts/check-windows.sh

# ── 單條閘門（44 支都在 scripts/check-*）──
python3 ./scripts/check-consent-copy.py
python3 ./scripts/check-persona-voice-loudness.py          # 要 ffmpeg，解 952 支
python3 ./scripts/check-notice-claims-are-accounted-for.py
python3 ./scripts/check-notice-claims-are-accounted-for.py --list   # 整份宣稱清單
python3 ./scripts/check-spoken-consent-is-not-older-than-the-words.py
python3 ./scripts/check-release-version.py

# ── CI ──
gh run list --limit 5 --json headSha,status,conclusion,displayTitle \
  --jq '.[] | "\(.headSha[0:7]) \(.status)/\(.conclusion // "-") \(.displayTitle)"'

# ── 發版（步驟 2）──
#   1. 改 7 個檔的版號（兩個 Cargo.lock 要跑過 cargo 才會變髒）
#   2. python3 ./scripts/check-release-version.py
#   3. 推 commit → 等 CI 六個 job 全綠
#   4. git tag v0.1.0-alpha.N && git push origin v0.1.0-alpha.N
#   5. 回頭確認 release job 真的跑了、四個 artifact 齊、不是 draft
```

CI 有六個 job：`linux`（測試／lint／隱私／全部資產閘門）、`linux_preview`（原生
capture＋`.deb`）、`msrv`（Rust 1.88）、`macos_spike`、`windows`、`release`
（只在 `refs/tags/v*` 上跑，且 `needs: [linux, linux_preview, msrv, windows]`）。

---

## 8. 接手後的第一個動作

1. 讀 `AGENTS.md` 第零節（全域交付與產品文案規則）。
2. 讀 `docs/PHASES.md` 最前面的 Release 1.0 合約。
3. 不重切 alpha.141、不重掃 compliance；沿用第 5 節步驟 3，讓 Ted 在正式安裝副本上
   一次驗 `docs/WINDOWS-CHECKLIST.md` 的一條真機邊界，精確記下通過或失敗的範圍。

**不要**開新大軸、不要重掃 compliance、不要動 `docs/RELEASE-NOTES.md` 的歷史區段、
不要碰簽章憑證、不要把錄音那邊的東西搬進 repo。
