# r28 後半：卡片那一格也要照章節的**範圍**去找

## 現況（已經在這個 branch 上，不要重做）

r28 前半已經完成：`Db::retained_interpreter_attempts_for_segment` 從「起點嚴格相等」
換成半開區間 `[core_started_at, core_ended_at)`，所以使用者把兩個章節**合併**之後，
「她試著問過 N 次」不會再少算。驗收測試在
`crates/sister-core/tests/chapter_merge_keeps_the_asks.rs`，六條全綠。

## 病灶：只改了一半，於是同一句話自相矛盾

決定「有沒有卡片」的那半還停在**起點嚴格相等**上。合併「左半問了 3 次沒答案」＋
「右半問了 2 次、最後一次成功並寫出卡片」之後，畫面印的是：

```text
最新一段值得理解，她試著問過 5 次，最近一次是成功（success），現在手上沒有卡片。
```

「最近一次是成功」和「現在手上沒有卡片」互相打臉，而那張卡就躺在 `l2_card` 裡，
鍵是右半的起點 —— 一列都沒被刪。

**r28 只出前半的話，是拿一個少算換一句自相矛盾。這一單要把後半補上。**

---

## 要做的事

### 1. 加一支資料層函式（`crates/sister-core/src/db.rs`）

```rust
pub fn l2_versions_for_chapter(
    &self,
    core_started_at: Millis,
    core_ended_at: Millis,
) -> Result<Vec<L2CardRow>>
```

語意三句，缺一不可：

1. 在 `segment_core_start ∈ [起點, 結束)` 且 `tombstoned_at IS NULL` 的卡片裡，
   取 `segment_core_start` **最大**的那一個當作脈絡的鍵。
2. 回傳**那一個鍵**的全部未墓碑版本，由舊到新（`ORDER BY version, id`）——
   和 `l2_versions_for_segment` 完全同一個形狀，好讓 `latest_with_previous`
   原封不動繼續用。
3. 範圍裡一張活著的卡都沒有 → 回空的 `Vec`。

⚠ **第 2 句是最容易做錯的一句。** 把 `WHERE segment_core_start = ?1` 直接換成
`>= ?1 AND < ?2` 就會把**兩個章節的卡片串成同一條版本史**，於是
`latest_with_previous` 拿左半的卡當成右半那張卡的「原版：⋯」——畫面上會出現一張卡
說自己的前身是**另一個章節**的內容。那是把一句自相矛盾換成一句更難發現的假話。
正確做法是**先取鍵、再取那個鍵的版本史**（可以是一句 SQL 的子查詢，也可以兩句，
但取鍵那一步必須排除墓碑）。

### 2. 兩個「章節作用域」的消費端改走這支函式

| 位置 | 現在 | 改成 |
|---|---|---|
| `apps/desktop/src-tauri/src/main.rs:1800`（`memory_current_guess`） | `l2_versions_for_segment(seg.core_started_at)` | `l2_versions_for_chapter(seg.core_started_at, seg.core_ended_at)` |
| `crates/sister-core/src/brain.rs:1015`（`collect_jobs`） | `latest_l2_for_segment(seg.core_started_at)?.is_some()` → `continue` | 用同一支函式判斷「這一章有沒有活著的卡」→ 有就 `continue` |

`collect_jobs` 那一格是同一個 bug 的第二個消費端：合併之後她會把**已經有卡的章節
再問一次**，白花預算又寫出第二張卡。兩處走同一支函式，不要各寫各的判斷。

### 3. 兩個「脈絡作用域」的呼叫端**不准動**

| 位置 | 它在問什麼 | 為什麼要維持相等 |
|---|---|---|
| `crates/sister-core/src/db.rs:3791`（`insert_l2_card` 算 `supersedes`） | 這條脈絡的前一版是誰 | 改成範圍會讓新卡接到別章的卡上，版本史當場錯亂 |
| `crates/sister-core/src/reviewer.rs:1859`（`correct_l2`） | 要修訂的是**哪一張**卡（UI 傳 `card.segment_ref` 進來） | 使用者指名了那一張，範圍會讓它改到別張 |

`brain.rs` 的其他 `latest_l2_for_segment` 呼叫都在 `mod tests` 裡，照舊。

### 4. `collect_jobs` 要有自己的測試

我的驗收測試是**整合測試**，呼叫不到私有的 `collect_jobs`。請在 `brain.rs` 的
`mod tests` 裡加一條：合併之後（`brain_outbound`／`l2_card` 的鍵在章節**中間**，
不在起點上），已經有卡的那一章**不會**再被排進 job。既有的
`interpret_writes_a_card_through_a_fake_cli` 是可以照抄的骨架。

### 5. 閘門腳本要一起改

`scripts/check-current-guess-wiring.py` 的閘門 A 現在**要求** main.rs 那一格是
`l2_versions_for_segment(...)` 配 `seg.core_started_at`。正確的修法會讓它變紅。
請把它改成要求新的函式名＋**兩個**引數 `seg.core_started_at, seg.core_ended_at`。

⚠ 那個檔案的**檔頭 docstring（約 36-45 行）**和**成功訊息（約 523 行）**都用文字
描述閘門在檢查什麼。程式碼改了、那兩處沒改的話，檔案就會自己和自己打架。
三處要一起。

---

## 驗收

`crates/sister-core/tests/a_merged_chapter_keeps_its_card.rs` 已經在樹上，七條。

**⚠ 那個檔案一個字都不准改。** 它是在你動手之前寫好的考題。現在它編不過
（`l2_versions_for_chapter` 還不存在，實測只有這一個 E0599，其餘 API 全部對得上）。
你交貨之後它必須**原封不動**全綠。如果你認為其中哪一條的期望是錯的，
**不要改它**——在 RESULT 裡寫下來，我自己判。

七條分別是：

1. `a_merged_chapter_shows_the_card_written_for_its_right_half` —— bug 本身
2. `an_unedited_chapter_returns_exactly_what_it_did_before` —— 迴歸（對照
   `l2_versions_for_segment`，不手抄期望值）
3. `the_right_edge_belongs_to_the_next_chapter` —— 右界是開的
4. `two_lineages_in_one_range_are_not_spliced_into_one_history` —— **上面那把刀**
5. `a_forgotten_right_half_falls_back_to_the_live_card_on_the_left` —— 取鍵要排除墓碑
6. `a_chapter_whose_only_card_was_forgotten_has_no_card_at_all`
7. `the_count_and_the_card_answer_the_same_question` —— 兩半擺在一起

其他必須全綠的：

```bash
export PATH="$HOME/.cargo/bin:$PATH"
export TMPDIR=/home/ted-h/tmp-tests/.tmp-r28
cargo test --workspace --no-fail-fast     # 應該是 25 個 binary（本來 24，你不會刪掉任何一個）
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all --check
python3 scripts/check-current-guess-wiring.py
```

⚠ `cargo fmt` **不要**在 `apps/desktop` 底下跑（那半邊沒有人 fmt-check，排版一動
`check-settings-say.mjs` 就紅——它比對 `main.rs` 的原始文字）。

⚠ 這台機器上 `cargo check` 對 `apps/desktop/src-tauri` 會 panic（libdbus-sys 的
build script），**那不是你弄壞的**。桌面那半編不出來是正常的，改完照樣要改對——
它由 CI 和 `check-current-guess-wiring.py` 把關。

---

## RESULT.md（必填欄位）

寫到 `/home/ted-h/tmp-tests/wt-r28/RESULT-r28b.md`：

1. **改了哪些檔案、每個檔案為什麼**
2. **`l2_versions_for_chapter` 的 SQL 原文**，以及「先取鍵、再取版本史」是怎麼做的
3. **七條驗收測試的實際輸出**（貼 `test result:` 那幾行）
4. **`collect_jobs` 那條新測試**：名字、它斷言什麼、以及你把哪一行改壞可以讓它紅
   （真的改壞跑一次，貼輸出——不要用推的）
5. **我這張派工單哪裡寫錯了。** 我前幾次錯過的地方：把測試夾具當成產品行為、
   替一個到不了的分支宣稱成因、列的輸入根本走不到我指定的那一格、
   把一個成員的性質寫成整個集合的性質。如果這張單子的前提有假的，直接說。
   **不要為了讓我高興而編一條。** 真的沒有就寫「沒有」。
6. **你覺得這個修法還有哪裡沒守住**（例如：範圍裡有兩張活卡的時候，畫面只顯示
   一張，而沒有任何地方告訴使用者另一張存在——這算不算問題？）

commit 由我來下，你不要 commit、不要 push、不要開 branch。
