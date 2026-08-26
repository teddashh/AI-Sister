# 按下「停止」之後那兩分鐘

## 選了哪個方向、為什麼

選的是 **A.1 第一條：心跳在腦收工之前就不再說「在錄」**。

「在錄」在這個產品裡的意思是她正在記畫面。迴圈跳出來之後她一個畫面都不抓了，繼續蓋 Recording 心跳，字母人會說「在聽」——他照著那三個字去做想被記住的事，問「剛剛發生什麼事」拿到空白。那是模組開頭寫的、這個產品唯一不能說的謊。13013 那段註解已經選過一次：說她還在錄卻沒在錄，比說她走了但其實還在，更危險。

可是把墓碑提前蓋上去，`is_occupied` 會跟著變 false，第二個 recorder 穿得進去，而第一個還握著資料庫。所以不是把心跳收成墓碑再乾等，是 **第三種佔著：沒在錄、但目錄還佔著**。和開機那幾分鐘同一種拆法，方向相反：

| | 開機 | 收工想最後一段 |
|---|---|---|
| 在錄？ | 否 | 否 |
| 佔著？ | 是 | 是 |
| 畫面 | 「正在開資料庫」 | 「錄製已停，解釋層還在想最後一段」 |

沒有新增 `Phase` 變體（`settings_write` 那一支不准動，加變體編不過）。心跳檔寫 `{ts} thinking {until}`，`presence` 多一種 `Thinking`。`phase()` 對它回 `None`（不在錄），`is_occupied` 回 true（還佔著），上限寫在檔案裡，不靠 16 秒逾時。

牆上時間上限是 **240 秒**：腦執行緒可能正好卡在一輪 CLI（`SPAWN_TIMEOUT` = 120），看到停止旗標之後再跑一輪把開著的最後一段想完（再 120）。槽是並行的，不是 4 × 120。

沒有把「想完最後一段」拿掉。沒有改 `SPAWN_TIMEOUT`。

---

## 閘門原文

```
=== 1 cargo fmt --all -- --check ===
（無輸出，exit 0）

=== 2 cargo clippy --workspace --all-targets -- -D warnings ===
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 1.33s

=== 3 cargo test --workspace ===
test result: ok. 112 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.52s   # sister-capture
test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.09s     # sister-cli extras
test result: ok. 182 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.90s   # sister-cli
test result: ok. 447 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 6.30s   # sister-core
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 3.89s     # search_latency
test result: ok. 10 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s    # sister-shell
Doc-tests: 0 / 0 / 0

=== 4 cargo build -q -p sister-cli ===
（無輸出，exit 0）

=== python3 ./scripts/check-consent-copy.py ===
三張同意書的條文與未簽後果：consent.rs / onboarding.js 逐字一致；撤回週期的五份文案都與 ops.rs 的 CONSENT_EVERY=5 秒一致

=== ./scripts/check-no-network.sh ===
✓ 出貨的相依樹裡沒有 HTTP client 也沒有推論引擎，原始碼和畫面裡沒有連外的路

=== python3 ./scripts/check-brain-outbound.py ===
出境路徑：spawn_cli 要 CloudAllowed；憑證只在 consent.rs 鑄；brain_outbound 不含原文；自我檢查改壞三行都會紅

=== python3 ./scripts/check-no-keylogging.py ===
✓ `kb_proc` 只把那個指標交還給 CallNextHookEx，從來沒有讀過它
  （自我檢查：同一條規則抓得到 `mouse_proc` 的解參考）

=== node ./scripts/check-settings-say.mjs ===
✓ 設定頁：成功和失敗都說得出話，而失敗不會偷偷刪掉他的規則

=== node ./scripts/check-pet-says-why.mjs ===
✓ 那幾句「為什麼沒成」活得過輪詢，而且下一個動作蓋得掉
（含新增的 ㉘：thinking 不是「在聽」、也不是「沒有人在記錄」，開始鍵藏著）

=== node ./scripts/check-timeline-forget.mjs ===
✓ 「忘掉這一段」的兩段式，在成功和失敗之後都退得回去
✓ 外送紀錄面板把兩種空、跳過原因和原文沒遮講開了

=== node ./scripts/check-consent-sticks.mjs ===
✓ 勾勾上寫的東西就是檔案裡的東西，寫失敗的時候它會自己說

=== node ./scripts/check-frame-source.mjs ===
✓ 表頭上那兩格，只替真的看到的那張圖說話

=== node ./scripts/check-metrics-view.mjs ===
metrics frontend contract 全部通過。

=== node ./scripts/check-aux-window-threading.mjs ===
✓ 五扇 auxiliary window 都離開同步 handler，4 秒訊息只說已經量到的事

=== node ./scripts/check-moment-set-contract.mjs ===
✓ moments 沒接到執行路徑、未標與零分得開、提醒率仍是 null、通知類逐一列舉且全部不是通知

=== python3 ./scripts/check-combo-is-readable.py ===
✓ 沒有任何一句給人看的話會叫他去按 KeyP

=== python3 ./scripts/check-docs-point-somewhere.py ===
✓ 文件裡指出去的每一條路都指得到東西

=== python3 ./scripts/check-checklist-quotes-exist.py ===
✓ 清單裡那 29 句「產品會說 X」，X 在原始碼裡都找得到

=== ./scripts/check-erased-db.sh ===
要忘掉的是 2026-08-25 06:41:17 到 2026-08-26 06:41:17（24 小時）。
  刪掉了 4 列畫面紀錄、9 段文字、9 個事實、14 筆事件
  刪掉了 1 題你自己問過的話（題庫）
  刪掉了 1 場錄製的紀錄本身（那幾場已經一列都不剩了）
（exit 0）

=== SISTER=./target/debug/sister ./scripts/check-readme-quickstart.sh ===
✓ README 的 quickstart 跑得起來，答得出 ★ +886800080123，而且那塊範例輸出逐行對得上

=== SISTER=./target/debug/sister python3 ./scripts/check-recall-baseline.py ===
✓ README 的 recall 品質／成本表和本次 CLI JSON 一致
✓ README 的 recall-session 品質／成本表和本次 CLI JSON 一致
✓ 3 個事件、5 題、三個產品 profile 的結果都和 baseline 一致
✓ 活動級章節 corpus：7 個事件、3 題，分數已釘
✓ 延遲有樣本且都是有限非負數；沒有鎖毫秒門檻

=== SISTER=./target/debug/sister python3 ./scripts/check-moment-baseline.py ===
✓ review 不帶 --confirm-private-text-reviewed 會失敗
✓ draft 候選分佈由 fixture 決定；多一幀日期就 +1 DateTimeMention
✓ corpus 有多種 System 事件，notification 仍是 0
✓ unlabeled 在 Draft／Reviewed 上真的會變，且與 should_speak 分開
✓ review 不帶隱私確認旗標會失敗

=== ./scripts/check-windows.sh ===
✓ Windows 端編譯與 lint 都過了（行為仍需在 Windows 上驗證）
```

全部 exit 0。沒有改版本、沒有 commit、沒有 tag。

---

## 收工時新增／改動的每一句話（原文）

開始等之前（`record` 收工、有腦執行緒時印一次）：

```
錄製已停；解釋層還要把最後一段想完（最多 240 秒）。
```

`format_report` 新增兩句，接在既有解釋層／審閱層後面，不取代它們：

```
收工時看過還開著的最後一段，沒有值得理解的訊號可想。
```

```
收工時等到上限（240 秒），還開著的最後一段沒想完。
```

按「開始記錄」時，三種佔著三句（舊的「已經有一個 sister record 在跑了」只留給真的在錄）：

```
已經有一個 sister record 在跑了
```

```
已經有一個 sister record 正在起來（多半在開資料庫）。再等一下
```

```
錄製已經停了，解釋層還在想最後一段（最多還要 N 秒）。想完就會收工，這期間不要再開一個。
```

字母人（`recording_state === "thinking"`）：

```
錄製已停，解釋層還在想最後一段
```

`sister stop` 在 Thinking 時：

```
■ 錄製已經停了，解釋層還在想最後一段。停止的請求還在，想完就會收工。
```

`sister doctor`「現在有沒有在看」在 Thinking 時：

```
錄製已停，解釋層還在想最後一段（行程還在，不要再開一個）
```

既有三句空沒有改：「沒設定 CLI」「這一場一次都沒醒」「醒了沒東西可想」還是各一句。

---

## 證明「停止之後畫面說的和行程真實狀態一致」的測試

**`wakeup::tests::after_stop_the_beat_says_not_recording_while_the_process_still_thinks`**

用假 CLI（睡 2 秒，不是真模型）。斷言：

1. 停止之前心跳是 Recording（`is_recording`）。
2. `Handle::shutdown` 一開始，心跳變成 `Presence::Thinking`：`is_recording == false`，`is_occupied == true`。
3. `occupied_why` 含「想最後一段」和「秒」，不是「已經有一個 sister record 在跑了」。
4. join 之後蓋墓碑，`is_occupied == false`。

同模組還有：

- `silence_sentences_are_not_the_same`：一次都沒醒 ≠ 最後一段沒東西可想 ≠ 等到上限沒想完 ≠ 開始等之前那句。
- `shutdown_with_nothing_worth_does_not_print_timed_out`：沒東西可想不准印成「沒想完」，而且「一次都沒醒」那句還在。

heartbeat 側：

- `thinking_through_the_last_segment_occupies_but_does_not_record`
- `a_thinking_beat_expires_at_the_deadline_it_wrote`（到點不靠 16 秒逾時）
- `thinking_does_not_go_stale_at_the_recording_timeout`
- `the_three_ways_of_occupied_are_three_different_sentences`
- `a_tombstone_clears_thinking`

字母人閘門 ㉘ 守畫面那一句和開始鍵藏著。

沒有測到的：真的等到 120 秒 CLI 逾時那一條（常數不准改，測它要真等兩分鐘）。`LastSegment::TimedOut` 的句子是用構造出來的 `Report` 斷的，不是跑過 `SPAWN_TIMEOUT` 才斷的。

---

## 沒做完或沒把握

- **設定頁。** 不准碰 `settings.*` 和 `settings_write`。想最後一段時 `phase()` 是 `None`，設定頁仍會說「現在沒有人在錄，所以這一份要等你按下『開始記錄』才會生效」。按下去會拿到 `occupied_why` 那句（有上限）。設定頁自己那一行和心跳在這兩分鐘裡仍對打——這是這一輪不能修的。
- **`forget_range` / `session_shell_why`。** 它們還是吃 `Option<Phase>`。想最後一段時會走 `None` 那句「她當掉了」。doctor 的「現在有沒有在看」有特判，忘掉／空殼那幾句沒有。
- **`catch_up` 只往回看 `LOOKAROUND_MS`（5 分鐘）。** 逾時的痕跡在 `brain_outbound` 的 `timeout` 列；下次開機 5 分鐘內再開始錄，解釋層會再看到已關閉的那一段。隔了更久再開，那一筆 Timeout 還在外送紀錄裡，但這一次補不到。句子裡**沒有**寫「catch_up 一定會補」——那句話我查過，程式做不到「一定」。
- **舊版 `sister.exe`。** `{ts} thinking {until}` 的第二欄它認不得，會讀成 Recording。時戳是現在，16 秒後過期。那 16 秒裡舊版會說她在錄（佔著，第二個 recorder 進不去；指示燈是謊）。過了 16 秒舊版放行第二個 recorder，而新版還佔著直到 `until`。沒有用未來時戳去騙舊版的 16 秒窗。
- **`Handle::Drop` 不寫 thinking 心跳。** 只有 `shutdown()` 寫。正常收工走 `shutdown`；`?` 提早離開那條路還是舊行為（Recording 心跳 16 秒後過期）。
- **Windows 真機沒跑過。** `check-windows.sh` 只編過。thinking 心跳的寫讀在 Linux 單測裡驗過。
