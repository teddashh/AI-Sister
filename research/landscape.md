# 桌面 Ambient-AI / 螢幕記憶 競品地景調查(2026-08-17)

> 調查目的:為開源、local-first 的「螢幕歷史 + recall + 主動陪伴」產品(字母人概念)定位。
> 方法:2026-08-17 當日 web 查證(WebSearch + 直接抓取 GitHub repo / LICENSE / 官方公告),非訓練資料舊聞。所有連結為當日可驗證來源。

---

## 1. Microsoft Recall(Windows 11 / Copilot+ PC)

**現況(2026-08)**:2025 年 4 月正式對 Copilot+ PC 公開推出,**opt-in**(需主動開啟,非預設);快照每隔數秒擷取、本地儲存於 SQLite,宣稱以本地 NPU 處理並過濾敏感內容(密碼、卡號)。截至 2026 年 4 月,**只有不到 10% 的 Windows 11 機器具備啟用資格**(需 Copilot+ NPU 硬體),賓州大學資安辦公室甚至正式警告 Recall「帶來重大且不可接受的安全、法律與隱私挑戰」([GeekWire, 2026](https://www.geekwire.com/2026/one-year-after-its-rocky-launch-microsofts-windows-recall-still-raises-security-red-flags/))。

**安全信任持續破產**:2026 年 4 月,研究員 Alexander Hagenah 發布 **TotalRecall Reloaded**,證明惡意程式在**一般使用者權限(無需 admin、無需 kernel exploit)**下即可從記憶體中的程序完整撈出 Recall 快照(含密碼、銀行 session、六個月的 Slack 對話),且該工具已被實際武器化遠傳資料;Kevin Beaumont 的分析亦指出快照可被解出為 SQLite 檔、「100% 不需要實體接觸即可竊取」([GeekWire](https://www.geekwire.com/2026/one-year-after-its-rocky-launch-microsofts-windows-recall-still-raises-security-red-flags/)、[DoublePulsar](https://doublepulsar.com/microsoft-recall-on-copilot-pc-testing-the-security-and-privacy-implications-ddb296093b6c))。Microsoft 官方否認構成新漏洞([Thurrott](https://www.thurrott.com/windows/windows-11/335005/microsoft-denies-a-new-recall-security-vulnerability-claim))。

**Click to Do 與 AI 總開關民怨**:Click to Do(畫面上任意內容的 AI 動作層)仍隨系統綁定。2026 年 8 月更新(KB5101684,build 26100.8973 / 26200.8973)僅允許移除 Copilot+ PC 上的「Image Generation AI model」一個元件——**Windows 11 至今仍沒有一個能同時關閉 Copilot / Recall / Click to Do 的總開關**,關閉仍需 workaround([Tech Nerdiness](https://www.technerdiness.com/windows/windows-11-august-2026-update/)、[Windows Latest](https://www.windowslatest.com/2026/01/31/microsoft-reportedly-admits-windows-11-went-off-track-cuts-back-copilot-and-promises-real-fixes-in-2026/))。2026 年 1 月更有報導稱 Microsoft 內部承認 Windows 11 的 AI 推進「走偏了」,正在收縮 Copilot 布局並重新評估 Recall。

**小結**:OS 巨頭親自示範了這個 lane 的最大風險——**架構上宣稱 local,實作上失信**。Recall 沒死,但 adoption 停滯、名聲已成為「螢幕記憶」品類的負面代名詞;這對任何新產品既是阻力(用戶先入為主的恐懼),也是差異化機會(可驗證、可關閉、可審計)。

---

## 2. Rewind.ai → Limitless → Meta(已死,2025-12-19)

**完整時間線**([官方頁面現況](https://rewind.ai/what-happened-to-rewind/)、[9to5Mac](https://9to5mac.com/2025/12/05/rewind-limitless-meta-acquisition/)、[Winbuzzer](https://winbuzzer.com/2025/12/05/meta-acquires-ai-wearables-startup-limitless-kills-pendant-sales-and-sunsets-rewind-app-xcxwbn/)):

- **2020**:Dan Siroker 創立 Rewind,「perfect memory」概念;**2022-11** Mac app 上市(24/7 螢幕+音訊錄製、本地壓縮儲存)。
- **2023-03**:推出 Ask Rewind(「ChatGPT for me」,GPT-4 驅動),聲量高峰。
- **2024**:改名 **Limitless**,重心轉向 Pendant 穿戴硬體與雲端個人 AI;Rewind Mac app 進入維護模式(桌面螢幕記憶 lane 實質棄守)。
- **2025-12-05**:**Meta 宣布收購 Limitless**。Pendant 即日停售;EU、UK、巴西、中國、以色列、南韓、土耳其服務直接切斷,該地區用戶資料若不在 12/19 前匯出即被刪除([TechInformed](https://techinformed.com/meta-acquires-limitless-pendant-users-moved-to-free-unlimited-plan/))。
- **2025-12-19**:最後一次更新即為 **kill switch**——永久停用所有螢幕與音訊擷取;Rewind Mac app 正式關閉、下載移除。既有 Pendant 用戶轉為免費 Unlimited 方案,承諾「至少再支援一年」,之後併入 Meta 穿戴生態([Hedy 案例整理](https://www.hedy.ai/post/meta-acquires-limitless-ai-privacy/)、[Neowin](https://www.neowin.net/news/meta-acquires-ai-startup-limitless-ending-sales-of-its-popular-pendant-wearable/))。
- **2026 現況**:rewind.ai 網域已易手,現為無關聯的 AI 工具目錄站——產品死得徹底連網域都沒留下。

**用戶遷往何處**:各家「Rewind 替代品」整理指向三類——(1) **Littlebird**(雲端 full-context 助理,見 §5);(2) **Screenpipe**(local 開放原始碼路線);(3) **Hedy** / **LUCI**(會議與穿戴向)([Littlebird 整理](https://littlebird.ai/blog/rewind-alternatives)、[LUCI 整理](https://luci.memories.ai/blog/rewind-ai-shut-down-on-device-replacement))。

**死因剖析(重要)**:Rewind 之死**不是 privacy backlash**——它有一批死忠 power users,錄的是自己的機器、資料留在本地。它死於**商業路徑**:桌面訂閱撐不起 venture 估值 → pivot 硬體 Pendant → 被 Meta 收購後桌面產品被 kill switch。教訓有三:(1) **closed-source 個人記憶產品存在「收購即滅頂」的結構性風險**,用戶多年的記憶庫可以被一紙公告作廢——這是 open-source + 本地資料格式最強的信任論證;(2) 純「回憶搜尋」是低頻需求,Rewind 直到 Ask Rewind 才有黏性,證明**記憶必須接上主動使用它的 agent 才有 habit loop**;(3) 區域監管(EU/UK 直接砍)顯示這個品類的合規成本會被大公司用「乾脆退出」解決,獨立產品反而能以 local-only 繞開。

---

## 3. Screenpipe(mediar-ai/screenpipe)— 最接近的 OSS capture 層,**但已棄守 open-source**

Repo:[github.com/mediar-ai/screenpipe](https://github.com/mediar-ai/screenpipe)(現重導向至 `screenpipe` org)。**21.0k stars / 2.1k forks / 12,400+ commits**,YC **S26** 批次,2026-07 仍有活躍 release——**維護中,且拿了 YC 錢在衝商業化**。

**License(關鍵發現,已逐字查證)**:2026-06-10 官方公告「we updated our license to keep screenpipe sustainable」——**從 MIT 改為自訂的「Screenpipe Commercial License」(source-available,非 OSI)**([LICENSE.md 原文](https://raw.githubusercontent.com/mediar-ai/screenpipe/main/LICENSE.md))。條款要點:

- 免費僅限:「Personal, non-commercial use; Non-profit, educational, or research use」+ 組織 7 天評估期。
- 「Any Commercial Use ... requires a separate paid commercial license ... **regardless of company size, headcount, revenue, or funding**」。
- 明文禁止(無商業授權時):併入商業產品散布、作為 hosted/managed service、嵌入面向客戶的產品、**「building competing products or services」**。
- 非 copyleft;Licensor 保留一切權利。既有 lifetime license($400)仍有效但已停售新的([screenpipe.com](https://screenpipe.com/)、[MemX 比較](https://memx.app/blog/tried-every-ai-memory-app-2026/))。
- **但舊版本仍是 MIT**:Homebrew 至今打包的 v0.2.13 明載 `license "MIT"`([formula 原文](https://raw.githubusercontent.com/Homebrew/homebrew-core/master/Formula/s/screenpipe.rb))。License 變更不溯及既往——**2026-06-10 之前的 tag/commit 依 MIT 授權,可合法 fork**。

**架構(值得抄的部分)**([README](https://github.com/mediar-ai/screenpipe/blob/main/README.md)):

- **事件驅動擷取**而非固定間隔連拍:監聽 app 切換、點擊、打字停頓、捲動,「只在內容真的變了才截圖」——JPEG 儲存約 300 MB / 8 小時。這是它從早期版本演化出的省資源共識設計。
- 文字抽取以 **OS accessibility tree 優先、OCR 為 fallback**(macOS Apple Vision / Windows 原生 OCR / Linux Tesseract)。
- 本地 **SQLite + FTS5** 全文索引;音訊本地 Whisper 或雲端 Deepgram。
- **REST API 於 localhost:3030** + JS/TS SDK;插件「**pipes**」現為 markdown 檔 + YAML frontmatter 宣告資料權限的排程 AI agents。
- 生態定位已轉為「餵 context 給 agents」:README 直接列 OpenClaw、Hermes agent、Cursor、Claude Code 等整合。

**資源問題(有實據)**:官方 FAQ 自承約 **10% CPU、4 GB RAM、15 GB 儲存/月**,Apple Silicon 上多耗 5–10% 電池,且「調校不好時」更糟;歷史 issue 記錄過 CPU >100%、RAM >10 GB([Issue #183](https://github.com/mediar-ai/screenpipe/issues/183)、[官方 FAQ](https://docs.screenpipe.com/faq))。FAQ 的建議緩解包括「改用雲端轉錄」——對 local-first 訴求是個諷刺,也說明 24/7 本地 pipeline 的資源工程是真門檻。

**能否當 dependency?判定:不能;只能當參考架構,或從 MIT 時代 fork。**

- 現行 license 禁止嵌入商業產品**且禁止拿來做競品**——一個開源 ambient 助理正是它的競品,連「免費個人使用」的授權範圍都罩不住下游使用者的商業情境。
- 可行路徑:(a) 把它當**架構教科書**(事件驅動擷取、a11y-first、FTS5 schema、pipes 權限模型);(b) 必要時從 2026-06 之前的 MIT commit fork(但會揹上與上游分道的維護成本);(c) 互通面:使用者自己裝 screenpipe 屬其個人使用,第三方工具讀取其 localhost:3030 API 在法律上較灰,不宜作為官方支援路徑。

---

## 4. Everywhere(Sylinko/Everywhere)— BUSL 1.1 的「隨叫隨到」螢幕助理

Repo:[github.com/Sylinko/Everywhere](https://github.com/Sylinko/Everywhere)。**6.2k stars / 388 forks / ~1,900 commits**。.NET + Avalonia;Windows 10 (19041+) 與 macOS 12+,**Linux 仍在開發中**。多 LLM(OpenAI/Anthropic/Gemini/DeepSeek/Moonshot/Ollama)+ **MCP tools** + agent 系統(瀏覽器、檔案系統、terminal)。

**現況(2026-08)**:極活躍的 canary 節奏——最新 **v0.8.1-canary.20260814.19(2026-08-14)**,近期更新集中在 chat UI、context compression、per-assistant tool 審批策略等([Releases](https://github.com/Sylinko/Everywhere/releases))。**「Strategy Engine」(依情境自動給捷徑策略、免解釋需求)在 README 仍標示 work-in-progress,近月 release notes 完全未提及**——主動化這一步尚未出貨。

**互動模型**:hotkey 喚起、以 accessibility API + UI automation 抽取當前畫面結構化 context、就地行動——是「**on-demand 看螢幕**」而非「24/7 記憶」;**沒有持續錄製、沒有歷史 timeline、沒有 recall lane**([官方文件](https://everywhere.sylinko.com/en-US/docs/getting-started/introduction))。

**BUSL 1.1 條款(已逐字查證)**([LICENSE 原文](https://raw.githubusercontent.com/Sylinko/Everywhere/main/LICENSE)):Licensor 為 Sylinko Inc.;Additional Use Grant 為「**A Permitted Purpose is any purpose other than a Competing Use**」,Competing Use 定義為:作為替代品在商業上提供、以商業形式提供功能近似的服務、作為 hosted/managed/PaaS 提供;**Change Date 為「該版本首次公開發布後四年」→ 轉為 Apache 2.0**(即現行程式碼約 2030 年才自由)。

**對「想消費其訊號或互通」的專案的意涵**:BUSL 約束的是**對其程式碼(Licensed Work)的使用**,不約束「與它對話」——透過 MCP / API / IPC 與使用者自行安裝的 Everywhere 互通**不受限**;讀它的原始碼學招也沒問題;但 **fork 或抽取其程式碼進一個競品 ambient 助理 = Competing Use,明確禁止**。實務結論:當「潛在互通對象 + 設計參考」,不當 code 來源。

---

## 5. Highlight AI、Cluely 與 2025–2026 新進者:誰活著、誰有錢、誰死了

- **Highlight AI — 活,且剛拿大錢**:2026-03-24 完成 **$40M Series A(Khosla Ventures 領投**,General Catalyst 等跟投),累計 $50M;全桌面 app 的 context-aware 助理(寫作、會議轉錄、任務、chat),計畫年底前擴歐亞([Crunchbase](https://www.crunchbase.com/organization/highlight-5d80)、[分析](https://www.buildmvpfast.com/blog/highlight-40m-ai-tools-information-overload-coordination-tax-2026))。閉源、雲端成分重,非 local-first。
- **Cluely — 活,但信譽重傷**:2025-06 a16z 領投 $15M Series A(投後約 $120M),累計 $20.3M;即時聽音訊+看螢幕的 overlay「偷偷幫你」助理。**2026-03-05 創辦人 Roy Lee 公開承認先前對媒體宣稱的 $7M ARR 灌水約 35%(實際 ~$5.2M)**;2026-05 員工 73 人([Wikipedia](https://en.wikipedia.org/wiki/Cluely)、[PitchBook](https://pitchbook.com/profiles/company/802998-10)、[Getlatka](https://getlatka.com/companies/cluely))。它證明了「即時螢幕+音訊 context」有市場,也證明了這個品類的信任門檻極高——定位挑釁 + 誠信翻車的組合傷害極大。
- **Littlebird — Rewind 精神續作(雲端版),2026-03 出場**:$11M seed(Lotus Studio 領投;Lenny Rachitsky、Scott Belsky 等天使),創辦人 Alexander Green 與 Sentieo 創辦人 Alap/Naman Shah 兄弟;macOS 原生,「structured screenreading」讀所有 app 的螢幕文字 + 即時會議轉錄,主打「已經知道你在做什麼的 AI」;**雲端架構**,以 SOC 2 / GDPR / CCPA 合規背書([TechCrunch](https://techcrunch.com/2026/03/23/littlebird-raises-11m-to-capture-context-from-your-computer-so-you-can-query-your-data/)、[PR Newswire](https://www.prnewswire.com/news-releases/littlebird-raises-11-million-to-launch-the-only-ai-that-already-knows-what-youre-working-on-302721664.html))。是「full-context 桌面記憶」商業 lane 目前最正面的信號,也是 local-first 開源方案最直接的對照組。
- **Hedy / LUCI(Memories.ai)**:承接 Limitless 難民的會議向(Hedy,跨平台)與穿戴向(LUCI AI Pin,CES 2026,主打 on-device Large Visual Memory Model 2.0)([TechTimes](https://www.techtimes.com/articles/313851/20260107/ces-2026-memoriesai-wants-you-recall-past-conversations-through-luci-ai-pin.htm))。
- **OpenAI 的 ambient 硬體**:與 Jony Ive 合作的無螢幕語音優先裝置預計 2026 秋——巨頭把「ambient 陪伴」往**離開螢幕**的方向做([FinancialContent](https://www.financialcontent.com/article/tokenring-2026-1-5-openais-ambient-ambitions-the-screenless-ai-gadget-set-to-redefine-computing-in-fall-2026))。
- **OSS 小型 recall clones(活著但長不大)**:**OpenRecall**(AGPLv3,2.9k★,Python,跨平台,commit 量小)([repo](https://github.com/openrecall/openrecall));**Windrecorder**(GPL-2.0,3.9k★,Windows-only,3 秒截圖+15 分鐘轉影片+多 OCR 引擎,仍在維護)([repo](https://github.com/yuka-friends/Windrecorder));LiveRecall 等更小。共同點:**只有「錄+搜」,沒有 proactive 層、沒有 agent、沒有人格**,停在工具而非陪伴,社群規模也反映了純 recall 的需求上限。
- **已死名單**:Rewind/Limitless(§2)、ChatGPT Pulse(§6)、Atlas 瀏覽器(§6);更早的 Humane AI Pin 世代殘骸不再贅述。

---

## 6. 巨頭桌面布局:OpenAI / Anthropic / Google 已佔走哪些 lane

- **OpenAI**:macOS 桌面 app 的「**Work with Apps**」開放到所有方案(可讀寫 VS Code/Xcode/Terminal 等),對話中可**分享螢幕/截圖**取得視覺 context([alternativeto](https://alternativeto.net/news/2025/3/chatgpt-s-work-with-apps-feature-now-available-to-everyone)、[功能頁](https://chatgpt.com/features/desktop/))。**Pulse**(2025-09 推出的主動每日簡報)始終 mobile-only,**2026-08 已宣布日落、併入 scheduled tasks**([OpenAI release notes](https://help.openai.com/en/articles/6825453-chatgpt-release-notes));**Atlas 瀏覽器 2026-07 上線八個月即被砍**([TechTimes](https://www.techtimes.com/articles/320183/20260711/openai-kills-atlas-browser-after-8-months-what-replaces-it-what-users-must-do-now.htm))。結論:OpenAI 佔「on-demand 看螢幕」,但**主動式(proactive)嘗試已收攤**,賭注轉向 Ive 硬體。
- **Anthropic**:**Claude Cowork** 從桌面 app 一路擴張——2026-07-07 上 web 與 iOS/Android(雲端執行 session),**2026-08-12 Chrome 側欄升級為完整 Cowork session、可無縫接回桌面 app**;computer use 以高解析截圖(3.75 MP)讀螢幕、像人一樣操作桌面([9to5Mac](https://9to5mac.com/2026/08/12/claude-cowork-chrome/)、[DevOps.com](https://devops.com/claude-code-can-now-run-your-desktop/))。佔的是「**看著螢幕替你做事(agent)**」lane。
- **Google**:**Gemini Windows app** 全球推送——Alt+Space 系統級喚起、搜本機檔案/app/Drive、內建螢幕分享 + Lens「Select from screen」的螢幕感知([Windows Forum](https://windowsforum.com/threads/google-gemini-comes-to-windows-alt-space-desktop-search-ai-assistant.413786/)、[Engadget](https://www.engadget.com/apps/googles-new-windows-app-is-yet-another-way-to-access-gemini-214000564.html));ChromeOS 144(2026-01-27 起)把 Gemini 進 Chrome 設為預設能力;秋季「Googlebook」將以 Android+ChromeOS 融合平台出貨;Google Assistant 2026-09 EOL 全面讓位 Gemini([Chrome Unboxed](https://chromeunboxed.com/gemini-officially-arrives-in-chrome-with-the-chromeos-144-update/)、[Windows Forum](https://windowsforum.com/windows-news.4/googlebook-arrives-fall-2026-with-gemini-and-android-app-casting.440485/))。
- **總評**:到 2026-08,巨頭已完全佔據兩條 lane——「**當下畫面的 on-demand 理解**」與「**看著螢幕操作的 agent**」。但 **24/7 本地歷史記憶只有 Microsoft Recall 一家在做且做壞了名聲**;OpenAI 的 proactive(Pulse)已死、Anthropic/Google 均未做「從你的長期螢幕歷史主動出擊」。巨頭的包袱(隱私訴訟面、企業客戶、Recall 前車之鑑)讓它們短期不會碰「幫你留完整螢幕日記」這條線。

---

## 7. 「手」的元件盤點(grounding / 操作,非記憶 lane)

- **UI-TARS Desktop(ByteDance)**:[repo](https://github.com/bytedance/UI-TARS-desktop) 38.6k★,**Apache 2.0**。含 Agent TARS(多模態 agent 框架,CLI/Web UI,MCP 整合)與 UI-TARS Desktop(以 UI-TARS-1.5 / Seed-1.5-VL 模型做視覺 grounding 的原生 GUI agent),跨平台滑鼠鍵盤控制、hybrid browser control(GUI+DOM)。最近大版本 Agent TARS CLI v0.3.0(2025-11),2026 仍活躍。**可重用:開放權重的 grounding VLM + 桌面 operator 實作**,是目前開源「手」的最強預設。
- **Agent S(simular-ai)**:[repo](https://github.com/simular-ai/Agent-S) 12.2k★,**Apache 2.0**。Agent S3(2025-12)在 OSWorld 達 **72.6%、超越人類基線**,論文 2026-07 進 TMLR;架構為主 LLM + grounding 模型(UI-TARS-1.5-7B/72B)雙件式,`gui-agents` pip SDK 跨 Linux/macOS/Windows。活躍。**可重用:`gui-agents` 作為即插的「手」SDK**,並示範了「把 grounding 外包給開放模型」的正確切法。
- **Microsoft UFO / UFO² / UFO³**:[repo](https://github.com/microsoft/UFO) 9.5k★,**MIT**。UFO²「Desktop AgentOS」進入 LTS:Windows 深度整合(UIA/Win32/COM 的 hybrid GUI+API 行動、視覺+UIA 混合控件偵測、speculative 多動作批次省 51% LLM 呼叫);UFO³ Galaxy(2025-11,[arXiv:2511.11332](https://arxiv.org/abs/2511.11332))做多裝置 DAG 編排 + MCP。**可重用:Windows 上「能用 API 就不點 GUI」的混合執行層**,MIT 授權無負擔。
- **OpenAdapt(OpenAdaptAI)**:[repo](https://github.com/OpenAdaptAI/OpenAdapt) 1.7k★,**MIT**,Beta。已轉型為「demonstration-to-automation compiler」:錄下人類示範(DOM、a11y tree、視覺標記、OCR 證據),編譯成**確定性可重放**流程,健康路徑零模型呼叫、fail-closed(不確定就停)。社群較小但未棄坑(引擎在 openadapt-flow)。**可重用:示範→確定性重放的思路**,適合字母人「學會使用者重複動作」的場景;它同時證明錄製技術往 RPA 走是另一條已被佔的分岔。
- **OmniParser(Microsoft)**:[repo](https://github.com/microsoft/OmniParser) 25.3k★。螢幕截圖→結構化 UI 元素的純視覺解析器;V2(2025-02)+ OmniTool。**License 分層注意:repo 為 CC-BY-4.0;icon_detect_v3 權重 MIT(YOLOv9 系)、更早的 detector 權重 AGPL、caption 模型 MIT**。2025-03 之後無大動作,**半停滯**,但權重可獨立使用。**可重用:當 a11y tree 拿不到時的視覺 fallback parser**(選 MIT 的 v3 權重,避開 AGPL 舊權重)。

**結論**:「手」已是 commodity——Apache 2.0 / MIT 的 grounding 與執行層任選任組,**不構成護城河,也不需要自研**。

---

## 8. 綜合分析:2026-08 的開放空隙在哪

### 8.1 各 lane 佔位圖

| Lane | 佔位者 | 狀態 |
|---|---|---|
| OS 級 24/7 記憶 | Microsoft Recall | 信任破產、<10% 機器可用、adoption 停滯 |
| 商業雲端 full-context | Littlebird($11M)、Highlight($50M)、Cluely | 活躍且有錢,**全是雲端+閉源** |
| Source-available capture 基建 | Screenpipe(YC S26) | 技術最近、**2026-06 棄守 open-source** |
| On-demand 螢幕助理 | 巨頭桌面 app;Everywhere(BUSL) | 已被佔滿,無記憶 lane |
| OSS recall clones | OpenRecall(AGPL)、Windrecorder(GPL-2) | 只有錄+搜,無 proactive、無陪伴,長不大 |
| Agent「手」 | UI-TARS / Agent S / UFO / OmniParser | Apache/MIT commodity,隨取隨用 |
| Proactive 主動出擊 | (OpenAI Pulse 已死;Everywhere Strategy Engine 未出貨) | **實質無人佔位** |

### 8.2 死因分類(區分清楚,別學錯教訓)

- **死於收購/商業路徑,非隱私反彈:Rewind/Limitless。** 用戶愛它,是 pivot 硬體 + Meta 收購觸發 kill switch。教訓 = closed 個人記憶有「退出即滅頂」風險;**開源 + 本地開放資料格式是結構性信任優勢,不是意識形態**。
- **死於隱私/安全失信(adoption 死,產品未下架):Microsoft Recall。** 承諾 local 卻被一般權限撈庫;教訓 = 這個品類**必須以可驗證的加密、審計、總開關為第一級功能**,「宣稱本地」不夠。
- **死於成本 + habit 未養成:ChatGPT Pulse。** 雲端主動簡報推理成本高、黏性不足,八個月後折進 scheduled tasks;Atlas 同期被砍。教訓 = **proactive 必須便宜(本地觸發、小模型篩選)且長在使用者既有情境裡**,不能是另一個要點開的 feed。
- **傷於誠信/定位:Cluely。**「偷偷幫你」的敘事 + 造假營收,說明此品類的信任資本比技術更稀缺。
- **長不大於無價值閉環:OSS recall clones。** 純搜尋自己的過去是低頻需求;沒有 agent 消費記憶、沒有主動性、沒有人格,就沒有每日回訪理由。

### 8.3 空隙陳述

**到 2026-08,「真開源(OSI license)+ local-first + 隱私硬保證 + 24/7 螢幕記憶 + 主動出擊 + 人格化陪伴」這個組合沒有任何一個玩家同時佔據。** 最接近的 Screenpipe 在 2026-06-10 為了 YC 商業化放棄 MIT,親手讓出「品類裡唯一真開源 capture+memory」的位置;Everywhere 是 BUSL 且根本沒有記憶 lane,主動化引擎也還沒出貨;商業新貴(Littlebird/Highlight)全押雲端,把「隱私」做成合規證書而非架構保證;巨頭佔滿 on-demand 與 agent 兩條 lane,卻因 Recall 的前車之鑑不敢碰 24/7 本地日記;OSS clones 證明了純工具做不出黏性。

**對字母人的定位推論**:
1. **Capture 層是 commodity**:事件驅動擷取 + a11y-tree-first + SQLite/FTS5 已是公開共識架構(Screenpipe 驗證過);可自寫 thin Rust 層或從其 MIT 時代(≤2026-06-10,如 v0.2.x)fork,**不要依賴現行 Screenpipe Commercial License 版本**。
2. **「手」直接組裝**:Apache 2.0 的 UI-TARS / Agent S(`gui-agents`)+ MIT 的 UFO(Windows API 混合執行)+ MIT 的 OmniParser v3 權重(視覺 fallback),零授權負擔。
3. **護城河在四件事**:記憶 schema 與摘要/遺忘 pipeline;**便宜的本地 proactive 觸發引擎**(何時開口——全場無人做成);字母人 persona(把工具變成關係,補上所有死者缺的 habit loop);**可驗證的隱私硬化**(at-rest 加密、審計日誌、一鍵銷毀、全功能離線)——並把「我們有 Recall 沒有的總開關」直接做成賣點。
4. **互通而非依賴**:與 Everywhere 走 MCP/protocol 互通合法且無授權風險;與 screenpipe 僅止於使用者自主安裝層面的相容,不進 supply chain。

---

*調查與撰寫:2026-08-17。所有 star 數、版本號、license 條文均為當日抓取值。*
