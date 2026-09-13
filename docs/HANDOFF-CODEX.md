# HANDOFF — 交給下一位 agent（Codex）

**寫於 2026-09-13，交接點 `9886ff5`。** 這份是「打開就能接著做」的交接紀錄，不是
路線圖。路線圖在 `docs/PHASES.md`，規格在 `docs/SPEC.md`，產品定義在
`docs/PRODUCT.md`，工作紀律在 `AGENTS.md`。四份都要讀，順序就是這個順序。

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
| 這一段的執行者 | Claude Code（Opus 5, 1M context），session `e74b7a6f-34e8-4500-925c-8e0d020ac13c` |
| 時間範圍 | 2026-09-09T05:04:30Z → 2026-09-13T05:16:53Z（UTC，約四天，中途壓縮二十餘次） |
| 交接時的 HEAD | `9886ff5`，**已 push**，`main == origin/main`，working tree 乾淨 |
| 交接時的 CI | `9886ff5` 的 run **還在跑**（接手第一件事就是看它）。前四顆 `b193dce`／`c9d8ea9`／`b95acc2`／`812716f` 都是 `success` |
| 本機閘門 | 本機的 gates-all.sh（見第 7 節，**不在 repo 裡**）報 **通過 52 條，失敗 0 條** |
| 已公開的最後一版 | `v0.1.0-alpha.140`，published 2026-09-12T20:46:04Z |
| **未出貨的量** | `v0.1.0-alpha.140..HEAD` = **23 顆 commit**，全部還沒進任何 tag |

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

### 4.1 立刻的（無人認領，接手就該處理）

- **`9886ff5` 的 CI 還沒收工。** 它動到 `.github/workflows/ci.yml` 的 checkout
  （加 `fetch-depth: 0`），是這批唯一改 CI 設定的一顆，也是新閘門第一次在 runner
  上跑。**先看它。**
- **23 顆 commit 沒出貨。** `v0.1.0-alpha.140` 之後累積的東西全部只在 `main` 上。
  Ted 的節奏是「有執行檔他就下載測，沒有就繼續推；做完一段就切 tag」。

### 4.2 產品面最大的一塊：#42 的 URL 設定

在 `docs/PHASES.md` 的 Phase 6 那一節，搜 `#42 沒關`。

Ted 已經定案設計，**但一行都還沒做**：

- 「螢幕上被埋的 URL 指過去會執行」這件事本身還在。目標來自別的 app 時授權書會擋，
  但被埋的 URL 如果就在已授權那個 app 的畫面上，照樣會執行。
- **〔Ted 定案 2026-09-06〕不由產品替他選，做成使用者選的**，而且**由她開口問**，
  不是躺在設定頁裡等人發現。她問的那一句、兩個答案的精確語意、以及「第三種狀態是
  『還沒問過』不是預設值」都已經逐字寫在 PHASES 那一段。照抄，不要重新設計。

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

### 步驟 1（最小）：確認交接點的 CI

```bash
cd /home/ted-h/projects/AI-Sister
gh run list --limit 3 --json headSha,status,conclusion \
  --jq '.[] | "\(.headSha[0:7]) \(.status)/\(.conclusion // "-")"'
```

**驗收**：`9886ff5` 是 `completed/success`。若紅，最可能的兩個成因是
`fetch-depth: 0` 那個改動，或新閘門 `check-spoken-consent-is-not-older-than-the-words.py`
在 runner 上拿不到歷史——那一條**設計成「問不到就紅」**，不是 bug。

### 步驟 2：把 23 顆未出貨的 commit 切成 `v0.1.0-alpha.141`

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

### 步驟 3：接 #42 的 URL 設定第一刀

從**她問的那一句**開始，不要從設定頁開始。文字逐字抄 `docs/PHASES.md` 裡搜 `#42 沒關`
那一段（Ted 定案過的）。第三種狀態是「還沒問過」，型別上要和「你說了不要」分得開。

**驗收**：新增的邏輯放在 `crates/`（不是 `apps/desktop/src-tauri/src/main.rs`，理由
見第 6 節），`cargo test --workspace` 蓋得到，而且對新分支各打**兩種**突變（整條刪
掉、以及把它算出來的值換成隔壁那一臂的值）。

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

1. 跑 `bash /home/ted-h/tmp-tests/gates-all.sh /home/ted-h/projects/AI-Sister`，
   確認本機是 52/0。
2. 看 `9886ff5` 的 CI。
3. 讀 `AGENTS.md` 第零節（全域交付與產品文案規則）。
4. 讀 `docs/PHASES.md` 最前面的 Release 1.0 合約。
5. 然後照第 5 節的步驟 1 → 2 → 3 做下去。

**不要**開新大軸、不要重掃 compliance、不要動 `docs/RELEASE-NOTES.md` 的歷史區段、
不要碰簽章憑證、不要把錄音那邊的東西搬進 repo。
