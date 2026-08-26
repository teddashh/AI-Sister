//! 「現在到底有沒有人在錄」——兩個行程之間的第二條線。
//!
//! [`crate::pause`] 回答的是「她有沒有被叫停」，而那是**另一個問題**。字母人
//! 上那顆暫停鍵沒被按下的時候，它顯示「在聽」；但 `sister record` 是另一個
//! 執行檔，如果根本沒有人把它跑起來，「在聽」就是一句謊話——而且是這個產品
//! 唯一不能說的那種謊。使用者會照著那三個字去相信「她記得住今天」，然後某天
//! 問她「剛剛發生什麼事」，得到一片空白。
//!
//! 判定不能只看 `sessions.ended_at IS NULL`：recorder 當掉的時候那一列會永遠
//! 停在 NULL，於是「她死了」和「她正在錄」長得一模一樣。也不能只看「資料庫
//! 最近有沒有新資料」：閒置閘門本來就會讓一台沒人動的機器好幾秒不寫東西，
//! 那樣會把「安靜」誤判成「死了」。
//!
//! 所以 recorder 每隔幾秒主動蓋一次時戳。**活著才蓋得動**——當掉、被 kill、
//! 整台關機，時戳就停在那裡，而停住的時戳自己會過期。
//!
//! 和暫停旗標刻意相反的一點：這裡**讀不到就當作沒有人在錄**。暫停那邊
//! 「不確定就是暫停」是為了少錄；這邊「不確定就是沒在錄」是為了少吹牛。
//! 兩者是同一個方向——不確定的時候，往「她做得比較少」那邊倒。

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

use crate::model::Millis;

/// 檔名。放在 data dir 裡，跟 `sister.db` 同一層。
const BEAT: &str = "recording.beat";

/// 多久蓋一次。
///
/// 比 tick 慢很多是刻意的：這個檔案的用途是「有沒有人活著」，不是「上一個
/// tick 是什麼時候」。每秒寫一次沒有多回答任何問題，只是多 86,400 次磁碟寫入。
pub const BEAT_EVERY_MS: i64 = 5_000;

/// 超過這麼久沒蓋，就當作沒有人在錄。
///
/// 抓 `BEAT_EVERY_MS` 的三倍多一點：一次寫入被 OS 排程延後、或者某個 tick
/// 剛好卡在一張很大的圖上，都不該讓字母人閃一下「沒有人在記錄」。**寧可晚
/// 一點說她死了，也不要在她活著的時候說她死了**——後者會讓使用者去重開一個
/// 已經在跑的 recorder。
pub const STALE_AFTER_MS: i64 = 16_000;

pub fn beat_path(data_dir: &Path) -> PathBuf {
    data_dir.join(BEAT)
}

/// 心跳的主人現在在做什麼。
///
/// 這個檔案要同時回答兩個**不一樣**的問題，而以前只有一個答案：
///
/// - 「這個資料目錄有人佔著嗎」——`record` 拿它擋第二個 recorder。開機那幾
///   分鐘（大顆資料庫的 migration 會跑很久）**算佔著**，不然第二下按鈕會穿
///   過去，兩個 recorder 各錄一份。
/// - 「她現在在錄嗎」——字母人那顆指示燈、系統匣、`doctor` 拿它講話。開機
///   那幾分鐘**不算在錄**：她一個字都還沒記，而使用者正照著那三個字相信
///   「她記得住今天」。
///
/// 一個述詞同時回答這兩題，就一定有一邊是錯的。而修好「有沒有人佔著」的那
/// 一版，剛好把「她在錄嗎」弄壞了——這正是模組開頭說的那種、這個產品唯一
/// 不能說的謊。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    /// 行程起來了，但還沒開始錄（多半卡在 `Db::open` 的 migration）。
    Booting,
    /// 主迴圈在跑，她真的在記東西。
    Recording,
}

/// 蓋一次時戳。recorder 每 [`BEAT_EVERY_MS`] 呼叫一次。
///
/// 先寫暫存檔再 rename，否則讀的人有機會讀到寫到一半的半行數字。那不會壞掉
/// （解析失敗就當成沒在錄），但會讓字母人無緣無故閃一下。
pub fn beat(data_dir: &Path, ts: Millis) -> Result<()> {
    write_beat(data_dir, ts, Phase::Recording)
}

/// 開機那一段蓋的心跳。見 [`Phase`]。
pub fn beat_booting(data_dir: &Path, ts: Millis) -> Result<()> {
    write_beat(data_dir, ts, Phase::Booting)
}

/// 錄製迴圈已經停了，解釋層還在把最後一段想完。
///
/// 這不是 [`Phase`] 的第三種：字母人和設定頁問的是「她在錄嗎」，答案是
/// 不在——她一個畫面都不再抓。但這個資料目錄**還有人佔著**（行程活著、握
/// 著資料庫、CLI 最多還要跑到 `until`），所以 [`is_occupied`] 是 true、
/// [`is_recording`] 是 false。和開機那幾分鐘同一種拆法，只是方向相反。
///
/// `until` 是這一次收工自己算好的上限，寫進檔案裡，不靠 [`STALE_AFTER_MS`]。
/// 逾時是「她沒回話」，收工想最後一段是「她說了還要多久」——兩件事不准
/// 共用一個 16 秒。
pub fn beat_thinking(data_dir: &Path, ts: Millis, until: Millis) -> Result<()> {
    write_raw(data_dir, &format!("{ts} thinking {until}"))
}

fn write_beat(data_dir: &Path, ts: Millis, phase: Phase) -> Result<()> {
    // 錄製中就寫一個裸數字，和舊版一模一樣——舊的心跳檔（和舊的 recorder）
    // 讀起來仍然是「在錄」，那也是對的：會寫這個檔案的舊版都在主迴圈裡。
    write_raw(
        data_dir,
        &match phase {
            Phase::Recording => ts.to_string(),
            Phase::Booting => format!("{ts} boot"),
        },
    )
}

/// 收工。**乾淨結束的時候要自己講一聲**，不要留給逾時去猜——那 16 秒裡字母人
/// 會說她還在錄，而她已經走了。
///
/// 以前這裡是 `remove_file`，於是「檔案不在」同時是三件事：她還沒起來、她正在
/// 乾淨收工、她好好的只是這一拍慢了。三件事的下一步不一樣，而其中一件曾經被
/// 拿去守一個破壞性動作（字母人按「結束」的時候 kill 掉 recorder，見
/// `apps/desktop/src-tauri/src/main.rs` 那段長註解）。留下一塊墓碑之後，**檔案
/// 不在只剩一個意思：這個資料目錄從來沒有人跑過 recorder。**
///
/// 墓碑的時戳寫 `0`，這是**故意的**，而且是這個改動唯一微妙的地方：舊版的
/// `read_beat` 認不得的第二欄一律當成 `Recording`（那條 forward-compat 本身是
/// 對的，見下面），所以一個舊的 `sister.exe` 讀到這塊墓碑會說「她在錄」——正是
/// 這個模組開頭寫的、這個產品唯一不能說的那種謊。時戳寫 0 之後，舊版走的是
/// 「過期」那條路，答案變回正確的「沒有人在錄」。真正的收工時間放在第三欄，
/// 新版讀得到，舊版看都不會看。
///
/// 寫不進去（磁碟滿、權限）的退路是**把檔案截成 0 位元組**，不是刪掉它。
///
/// 這一段第一版寫的是刪掉，理由是「比留下一個新鮮的心跳說她還在錄好」。那句
/// 話是對的，但刪掉不是唯一比它好的選項，而它剛好把這整個改動的目的拆掉：
/// 「檔案不在」就是 [`safe_to_kill_spawn`] 唯一放行的那一種。磁碟滿的時候
/// `stop` 走這條退路，字母人那一頭就讀到 `NeverStarted`、放行落刀，而她正卡在
/// `rec.finish()` 寫 `end_session`——**正是這個改動要防的那一刀，從錯誤路徑上
/// 走回來了**。而且磁碟滿對一個整天在錄螢幕的東西不是假想狀況。
///
/// 截成 0 位元組同時滿足兩邊：`read_record` 讀不到第一欄 → [`Presence::Unreadable`]
/// →「沒有人在錄」（不吹牛）而且「有人來過」（不落刀）。它也比寫入更可能成功
/// ——truncate 不需要配置新的區塊。真的連截都截不動才刪，那時候已經沒有第三條
/// 路了。
pub fn stop(data_dir: &Path, at: Millis) {
    if write_raw(data_dir, &format!("0 stopped {at}")).is_err()
        && std::fs::File::create(beat_path(data_dir)).is_err()
    {
        let _ = std::fs::remove_file(beat_path(data_dir));
    }
    // 寫成功的話 rename 已經把它吃掉了；寫失敗的話這裡收掉那半個檔。
    let _ = std::fs::remove_file(beat_path(data_dir).with_extension("beat.tmp"));
}

fn write_raw(data_dir: &Path, body: &str) -> Result<()> {
    let path = beat_path(data_dir);
    let tmp = path.with_extension("beat.tmp");
    std::fs::write(&tmp, body).with_context(|| format!("write {}", tmp.display()))?;
    // rename 失敗就把那個檔收掉。它沒有人讀（`beat_path` 指的是另一個名字），
    // 所以留著不會說謊；收掉是因為那時候它是**一份完整的、內容正確的心跳**，
    // 只是名字不對——把它留在資料目錄裡等著給下一個人誤會，不如當場清掉。
    //
    // **這裡不是磁碟滿那條路。** 磁碟滿的時候上面那行 `fs::write` 就先失敗
    // 了，根本走不到 rename；而且下一次 `fs::write` 會 truncate，留著的 tmp
    // 不會讓它寫不進去。走到這一行的是 rename 自己失敗——Windows 上多半是
    // 防毒或索引服務正壓著目的檔。
    if let Err(e) =
        std::fs::rename(&tmp, &path).with_context(|| format!("rename to {}", path.display()))
    {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    Ok(())
}

/// 檔案裡那一行說什麼——**還沒和「現在」比過**。
///
/// 拆成兩層是刻意的：「那一行寫了什麼」和「所以她現在是什麼狀態」是兩個問題，
/// 而過期只跟第二個有關。墓碑不會過期（她收工了就是收工了，過一年還是收工），
/// 心跳會。
enum Record {
    /// 一次心跳：那個時刻她活著，而且在這個階段。
    Beat(Millis, Phase),
    /// 錄製已停，解釋層還在想最後一段。`until` 是這一次自己寫下的上限。
    Thinking { at: Millis, until: Millis },
    /// 一塊墓碑：她收工了。`at` 是收工時間；`None` 代表寫墓碑的版本沒寫第三欄。
    Tombstone { at: Option<Millis> },
}

/// 那個檔案這一次讀出了什麼。**三種答案，全部來自同一次 syscall。**
///
/// 這裡不回 `Option<String>`，因為「沒讀到」有兩種，而它們在
/// [`safe_to_kill_spawn`] 上是**相反**的答案。
///
/// 第一版是回 `Option`，然後在 [`presence`] 裡補一句 `beat_path().exists()` 把
/// 兩種分開。那是**兩次獨立的查找**，而 `Path::exists()` 的文件寫得很清楚：它
/// 把任何 metadata 失敗都回成 `false`，不只是 `NotFound`。於是「權限拿不到」
/// ——那個資料目錄一時 stat 不動的時候，也就是**最說不準**的那一種——兩次查
/// 找會各自說一句真話（「我讀不到」「我 stat 不到」）而湊出一句假的：
/// `NeverStarted`，[`safe_to_kill_spawn`] 唯一放行的那一種。她錄了一整天，字母
/// 人按「結束」，刀就落下去了。
///
/// 從真的那一半推不夠，要從**同一個**真的那一半推：`read_to_string` 那一次
/// 已經知道答案了（`ErrorKind`），上一版只是把它丟掉。
enum Raw {
    /// 檔案真的不在（而且我們確定：`NotFound`）。
    Missing,
    /// 讀不到，原因不是「不在」——權限、路徑上有一段不是資料夾、I/O 錯誤。
    /// **說不準有沒有人來過。**
    Unreadable,
    Text(String),
}

fn read_raw(data_dir: &Path) -> Raw {
    match std::fs::read_to_string(beat_path(data_dir)) {
        Ok(s) => Raw::Text(s),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Raw::Missing,
        Err(_) => Raw::Unreadable,
    }
}

/// `None` 有兩種原因，而**這一層分不出來**——見 [`Presence`]，那裡分得出。
fn read_record(data_dir: &Path) -> Option<Record> {
    match read_raw(data_dir) {
        Raw::Text(s) => parse_record(&s),
        Raw::Missing | Raw::Unreadable => None,
    }
}

/// 那一行字是什麼意思。純函式：不碰磁碟，所以「讀不到」和「讀到了看不懂」
/// 是兩個問題，在兩個地方回答。
fn parse_record(raw: &str) -> Option<Record> {
    let mut fields = raw.split_whitespace();
    let ts: Millis = fields.next()?.parse().ok()?;
    // 認不得的第二欄當成「在錄」而不是丟掉整行：未來多寫一個欄位的版本，
    // 不該讓舊版讀成「沒有人在錄」然後放行第二個 recorder。
    //
    // `stopped` 是這條規則的第一個客人，而它**不靠**這一欄被舊版讀懂——舊版
    // 讀不懂它，只會讀成 `Recording`。靠的是時戳寫 0，見 [`stop`]。
    match fields.next() {
        Some("boot") => Some(Record::Beat(ts, Phase::Booting)),
        Some("thinking") => Some(Record::Thinking {
            at: ts,
            until: fields.next().and_then(|s| s.parse().ok())?,
        }),
        Some("stopped") => Some(Record::Tombstone {
            at: fields.next().and_then(|s| s.parse().ok()),
        }),
        _ => Some(Record::Beat(ts, Phase::Recording)),
    }
}

/// 這個資料目錄現在到底是什麼狀況。**六種，而「檔案不在」只剩一個意思。**
///
/// [`phase`] 把 Live 以外都壓成 `None`，那對「要不要對使用者說她在錄」是對的
/// 答案，而且是三十幾個呼叫端要的那一個。這裡分得比較細，給那些**下一步不一樣**
/// 的地方用——尤其是任何想動手殺行程的地方（見 `main.rs` 那段長註解：`none`
/// 曾經同時是三件事，而只有其中一件可以落刀）。收工想最後一段是第六種：
/// 她沒在錄，但行程還在，[`is_occupied`] 和 [`phase`] 的答案不一樣。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Presence {
    /// 檔案不在。這個資料目錄**從來沒有人跑過 recorder**——只有這一個意思。
    NeverStarted,
    /// **問不出答案。** 兩種：讀到了但看不懂（寫到一半斷電），或者連讀都讀不
    /// 到而原因不是「不在」（權限、路徑上有一段不是資料夾、I/O 錯誤）。
    ///
    /// 這兩種在「有沒有人來過」上**答案不一樣**——第一種是有，第二種是不知
    /// 道——所以這裡不講那句話，只講它們共同的那一件事：**說不準**。上一版
    /// 這行寫的是「有人來過」，那時候唯一的入口是 `exists() == true`；後來
    /// [`Raw::Unreadable`] 也走進來，那句話就有一半的輸入是假的。守著落刀的
    /// 那道閘門讀的正是這一格，而上一輪它出的事就是「文件寫著程式已經沒有的
    /// 性質」。
    ///
    /// 和 `NeverStarted` 分開，因為「確定沒有人來過」是唯一可以放心落刀的那
    /// 一種，而「說不準」不是。
    Unreadable,
    /// 心跳是新鮮的，她活著，而且在這個階段。
    Live(Phase),
    /// 錄製迴圈已經停了，解釋層還在把最後一段想完。見 [`beat_thinking`]。
    ///
    /// `until` 是檔案裡寫下的上限。過了這個時刻，[`presence`] 會把它讀成
    /// [`Stalled`]——她說了還要多久，到點沒收工，就和當掉同一種「沒回話」。
    Thinking { at: Millis, until: Millis },
    /// 她自己講了收工（[`stop`] 留下的墓碑）。不會過期。
    Stopped { at: Option<Millis> },
    /// 心跳停在那裡過期了——當掉、被 kill、整台關機，或者這一拍慢了 16 秒
    /// （六小時一輪的 `prune` 在刪成千上萬個檔，休眠喚醒也算）。
    /// **後面那一種還活著**，所以這不是「她死了」，是「她沒回話」。
    Stalled { at: Millis, phase: Phase },
}

/// 現在這個資料目錄的狀況。見 [`Presence`]。
///
/// `now` 由呼叫端給，因為時間在這個 crate 裡一律是參數（測試要能演「三分鐘
/// 前的心跳」，不能等三分鐘）。
pub fn presence(data_dir: &Path, now: Millis) -> Presence {
    // 一次查找，三種答案。**不要在這裡補第二次 stat 去分辨前兩種**——那一版
    // 把「權限拿不到」餵成了 `NeverStarted`，理由寫在 [`Raw`] 上面。
    let raw = match read_raw(data_dir) {
        Raw::Missing => return Presence::NeverStarted,
        Raw::Unreadable => return Presence::Unreadable,
        Raw::Text(s) => s,
    };
    match parse_record(&raw) {
        // 讀到了，但看不懂（寫到一半斷電）。有人來過。
        None => Presence::Unreadable,
        Some(Record::Tombstone { at }) => Presence::Stopped { at },
        Some(Record::Thinking { at, until }) if now < until => Presence::Thinking { at, until },
        // 她寫了上限、到點還沒改成墓碑：沒回話。phase 填 Recording 是因為
        // [`Phase`] 沒有第三種——這筆 stalled 不會被拿去講「她在錄」
        // （[`phase`] 只看 Live），只給 [`safe_to_kill_spawn`] 比時戳。
        Some(Record::Thinking { at, .. }) => Presence::Stalled {
            at,
            phase: Phase::Recording,
        },
        // 未來的時戳一樣算活的：使用者調過時鐘、或者兩個行程對時差了幾百毫秒，
        // 都不該被讀成「她死了」。
        Some(Record::Beat(ts, phase)) if now - ts < STALE_AFTER_MS => Presence::Live(phase),
        Some(Record::Beat(ts, phase)) => Presence::Stalled { at: ts, phase },
    }
}

/// 最後一次心跳的時戳。`None` = 檔案不在、讀不到、或者那是一塊墓碑。
///
/// 墓碑回 `None` 是對的：那不是一次心跳，而這支函式的呼叫端拿它去講「最後一次
/// 心跳是幾秒前」。收工時間要用 [`presence`] 拿。
pub fn last_beat(data_dir: &Path) -> Option<Millis> {
    match read_record(data_dir)? {
        Record::Beat(ts, _) | Record::Thinking { at: ts, .. } => Some(ts),
        Record::Tombstone { .. } => None,
    }
}

/// 現在這個資料目錄的狀態。`None` = 沒有人在。
///
/// **把 [`Presence`] 壓成「在不在錄」**：只有新鮮的 `Recording`／`Booting`
/// 心跳算「有人在」。收工了、過期了、讀不懂、從來沒跑過、**正在想最後一段**
/// ——對「要不要說她在錄」來說都是同一個答案，而且是安全的那個答案（少吹
/// 牛，見模組開頭）。想最後一段那一種佔不佔著，請用 [`is_occupied`]。
///
/// **沒有 `_`。** 下一種變體編不過，才不會再安靜地掉進「沒在錄」。
pub fn phase(data_dir: &Path, now: Millis) -> Option<Phase> {
    phase_of(presence(data_dir, now))
}

/// [`phase`] 的純函式那一半：同一顆 [`Presence`] 不要讀第二次磁碟。
pub fn phase_of(p: Presence) -> Option<Phase> {
    match p {
        Presence::Live(p) => Some(p),
        Presence::NeverStarted
        | Presence::Unreadable
        | Presence::Thinking { .. }
        | Presence::Stopped { .. }
        | Presence::Stalled { .. } => None,
    }
}

/// 我在 `spawned_at` 那一刻 spawn 出去的那個 recorder，**還沒碰到資料庫嗎**。
///
/// 這支函式只有一個呼叫端、只回答一個問題：字母人按「結束」的時候，那個剛被
/// spawn 出來、還沒蓋過任何心跳的 child，可不可以直接 kill 掉。
///
/// 它靠的不變式只有一條：**心跳蓋在 `Db::open` 之前**（`ops::BootBeat::start`
/// 是開機的第一段，早於任何開檔）。所以「這個資料目錄上沒有任何東西是在我
/// spawn 之後寫下的」蘊涵「我那個 child 還沒走到 `Db::open`」，而那正是落刀
/// 唯一安全的時刻。**改動 `BootBeat::start` 的位置就是在動這條不變式。**
///
/// 為什麼要比時戳、而不是「檔案在不在」：一台錄過東西的機器上那個檔案**永遠
/// 都在**（不是墓碑就是過期的心跳），所以「不在」在那台機器上根本不會發生。
/// 分得出「上一場留下的」和「我這個 child 剛寫的」的，只有時戳。
///
/// 每一種說不準的情況都回 `false`。這裡和模組開頭那條「不確定就往少的那邊倒」
/// **方向相反**，因為守的是一個破壞性動作：那邊少倒是少講一句話，這邊少倒是
/// 少砍一刀。同一句「不確定」，在這裡的安全方向是不要動手。
pub fn safe_to_kill_spawn(data_dir: &Path, spawned_at: Millis, now: Millis) -> bool {
    match presence(data_dir, now) {
        // 從來沒有人在這個資料目錄跑過 recorder。**只有這一種是乾淨的。**
        Presence::NeverStarted => true,
        // 檔案在但讀不懂——說不出那半行是誰寫的，也就說不出是不是我這個 child。
        Presence::Unreadable => false,
        // 她已經蓋過心跳了，那就是走過 `Db::open` 了。收工想最後一段也是——
        // 那一刀會讓 `end_session` 永遠是 NULL。
        Presence::Live(_) | Presence::Thinking { .. } => false,
        // 墓碑比我 spawn 還早 = 上一場留下的，跟我這個 child 無關。
        // 比我晚 = **她正在收尾**（`heartbeat::stop` 寫在 `rec.finish()` 之前，
        // 中間隔著一次能力報告的寫檔），這一刀會讓 `end_session` 永遠是 NULL。
        // 沒有第三欄的舊墓碑說不出是哪一種，所以不動手。
        Presence::Stopped { at } => at.is_some_and(|at| at < spawned_at),
        // 同理：比我早的過期心跳是上一場的屍體；比我晚的是我這個 child 蓋的，
        // 它只是這一拍慢了（prune 在刪幾萬個檔、休眠喚醒），而她開著資料庫。
        Presence::Stalled { at, .. } => at < spawned_at,
    }
}

/// 她現在**在錄嗎**。正在開機的 recorder 回 `false`：她一個字都還沒記。
///
/// 給那幾個會對使用者講一句話的地方用（字母人的指示燈、系統匣、`doctor`）。
/// 要擋第二個 recorder 請用 [`is_occupied`]。
pub fn is_recording(data_dir: &Path, now: Millis) -> bool {
    phase(data_dir, now) == Some(Phase::Recording)
}

/// 這個資料目錄**有人佔著嗎**。正在開機的也算，收工想最後一段的也算。
///
/// 給那幾個「要不要再開一個」的判斷用。開機那幾分鐘算佔著，否則第二下按鈕
/// 會穿過去，兩個 recorder 各錄一份，而使用者只會發現磁碟用得比講好的快
/// 一倍。收工那兩分鐘同樣：行程還握著資料庫，第二個 writer 打進來的症狀
/// 一樣是磁碟用兩倍、而且兩句話對打（畫面說沒在錄，按開始卻說已經有人）。
pub fn is_occupied(data_dir: &Path, now: Millis) -> bool {
    occupied_of(presence(data_dir, now))
}

/// [`is_occupied`] 的純函式那一半。沒有 `_`：下一種變體編不過。
pub fn occupied_of(p: Presence) -> bool {
    match p {
        Presence::Live(_) | Presence::Thinking { .. } => true,
        Presence::NeverStarted
        | Presence::Unreadable
        | Presence::Stopped { .. }
        | Presence::Stalled { .. } => false,
    }
}

/// 有人佔著的時候，按「開始記錄」該看到的那一句。
///
/// `None` = 沒人佔著，可以開。`Some` 裡的字指得出現在在等什麼、還要多久——
/// 三種佔著不准印成同一句「已經有一個 sister record 在跑了」，因為下一步
/// 不一樣：正在錄的是去按停止，正在起來的是再等一下，想最後一段的是等她
/// 想完（有上限，上限在句子裡）。
pub fn occupied_why(data_dir: &Path, now: Millis) -> Option<String> {
    occupied_why_of(presence(data_dir, now), now)
}

/// [`occupied_why`] 的純函式那一半。同一顆 [`Presence`] 不要讀第二次磁碟。
///
/// 沒有 `_`：下一種變體編不過，才不會再讓「佔著」的新狀態靜靜變成可以再開一個。
pub fn occupied_why_of(p: Presence, now: Millis) -> Option<String> {
    match p {
        Presence::Live(Phase::Recording) => Some("已經有一個 sister record 在跑了".into()),
        Presence::Live(Phase::Booting) => {
            Some("已經有一個 sister record 正在起來（多半在開資料庫）。再等一下".into())
        }
        Presence::Thinking { until, .. } => {
            let left_secs = (until.saturating_sub(now) / 1000).max(1);
            Some(format!(
                "錄製已經停了，解釋層還在想最後一段（最多還要 {left_secs} 秒）。\
                 想完就會收工，這期間不要再開一個。"
            ))
        }
        Presence::NeverStarted
        | Presence::Unreadable
        | Presence::Stopped { .. }
        | Presence::Stalled { .. } => None,
    }
}

/// 最新那一列 `sessions` **是她的嗎**——正在錄，或錄製已停、還在想最後一段。
///
/// 開機那幾分鐘不是：心跳在、列還沒 INSERT，`MAX(id)` 上坐的是上一場的殼。
/// 沒有 `_`：少列一種，下一次就會把別人的當機扣成她的、或把她的收尾算成當機。
pub fn session_row_is_hers(p: Presence) -> bool {
    match p {
        Presence::Live(Phase::Recording) | Presence::Thinking { .. } => true,
        Presence::Live(Phase::Booting)
        | Presence::NeverStarted
        | Presence::Unreadable
        | Presence::Stopped { .. }
        | Presence::Stalled { .. } => false,
    }
}

/// 系統匣那顆「開始／停止記錄」按下去該做什麼。
///
/// 收的是**佔不佔著**，不是「在不在錄」——但佔著有三種，按下去的下一步不一樣：
/// 正在錄／正在起來的，寫 `stop.request` 有人讀；想最後一段的，迴圈已經跳出
/// 去了，再寫一個檔沒有人會讀。一個布林湊不出這三種。
///
/// 沒有 `_`。
pub fn tray_record_action(p: Presence) -> TrayRecordAction {
    match p {
        Presence::Live(_) => TrayRecordAction::Stop,
        Presence::Thinking { .. } => TrayRecordAction::WaitForThinking,
        Presence::NeverStarted
        | Presence::Unreadable
        | Presence::Stopped { .. }
        | Presence::Stalled { .. } => TrayRecordAction::Start,
    }
}

/// 系統匣那顆「開始／停止記錄」該寫什麼字。見 [`tray_record_action`]。
pub fn tray_record_label(p: Presence) -> &'static str {
    match tray_record_action(p) {
        TrayRecordAction::Stop => "停止記錄",
        TrayRecordAction::WaitForThinking => "還在收尾",
        TrayRecordAction::Start => "開始記錄",
    }
}

/// 系統匣「結束」那一行。想最後一段時記錄已經停了，不要再說「記錄也會停」。
pub fn tray_quit_label(p: Presence) -> &'static str {
    match p {
        Presence::Live(_) => "結束（記錄也會停）",
        Presence::Thinking { .. } => "結束（還在收尾）",
        Presence::NeverStarted
        | Presence::Unreadable
        | Presence::Stopped { .. }
        | Presence::Stalled { .. } => "結束",
    }
}

/// 系統匣那顆記錄鍵按下去會發生的三件事。見 [`tray_record_action`]。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayRecordAction {
    /// 沒人佔著：開一個 recorder。
    Start,
    /// 正在錄或正在起來：寫 `stop.request`，迴圈會讀到。
    Stop,
    /// 錄製迴圈已經跳出，解釋層還在想最後一段。再寫 `stop.request` 沒人讀。
    WaitForThinking,
}

/// 設定頁存完、字母人指示燈、系統匣共用的那四個字。
///
/// `"thinking"` 不是 `"none"`：那兩分鐘裡叫他去按「開始記錄」會被擋。
/// 沒有 `_`。
pub fn watching_word(p: Presence) -> &'static str {
    match p {
        Presence::Live(Phase::Recording) => "recording",
        Presence::Live(Phase::Booting) => "booting",
        Presence::Thinking { .. } => "thinking",
        Presence::NeverStarted
        | Presence::Unreadable
        | Presence::Stopped { .. }
        | Presence::Stalled { .. } => "none",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Tmp(PathBuf);
    impl Tmp {
        fn new(name: &str) -> Self {
            let p = std::env::temp_dir().join(format!("sister-beat-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&p);
            std::fs::create_dir_all(&p).expect("mkdir");
            Self(p)
        }
    }
    impl Drop for Tmp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// 沒有檔案 = 沒有人在錄。這是**新裝好、還沒跑過 `sister record`** 的
    /// 那台機器，也就是字母人第一次被打開時的樣子。
    #[test]
    fn a_machine_where_nobody_ever_started_her_is_not_recording() {
        let t = Tmp::new("never");
        assert!(!is_recording(&t.0, 1_000_000));
        assert_eq!(last_beat(&t.0), None);
    }

    #[test]
    fn a_fresh_beat_means_she_is_alive() {
        let t = Tmp::new("fresh");
        beat(&t.0, 1_000_000).expect("beat");
        assert!(is_recording(&t.0, 1_000_000));
        assert!(is_recording(&t.0, 1_000_000 + STALE_AFTER_MS - 1));
    }

    /// recorder 被 kill 掉、或整台當機。時戳停在那裡，而停住的時戳會過期
    /// ——這正是 `sessions.ended_at IS NULL` 做不到的判斷。
    #[test]
    fn a_beat_that_stopped_expires_by_itself() {
        let t = Tmp::new("stale");
        beat(&t.0, 1_000_000).expect("beat");
        assert!(!is_recording(&t.0, 1_000_000 + STALE_AFTER_MS));
        assert!(!is_recording(&t.0, 1_000_000 + 3_600_000));
    }

    /// 乾淨結束不要留給逾時去猜：那 16 秒裡字母人會說她還在錄，而她已經走了。
    #[test]
    fn stopping_cleanly_takes_effect_immediately() {
        let t = Tmp::new("stop");
        beat(&t.0, 1_000_000).expect("beat");
        stop(&t.0, 1_000_001);
        assert!(!is_recording(&t.0, 1_000_000));
    }

    /// **收工之後她要能再開一次。** 墓碑是留給「檔案不在是什麼意思」看的，不是
    /// 一把鎖——如果它算「有人佔著」，那 `record` 從今天起再也起不來，而這條路
    /// 上唯一的症狀是使用者按下開始、什麼都沒發生。
    #[test]
    fn a_tombstone_does_not_hold_the_door_shut() {
        let t = Tmp::new("restart");
        beat(&t.0, 1_000_000).expect("beat");
        stop(&t.0, 1_000_001);
        assert!(!is_occupied(&t.0, 1_000_002), "墓碑不可以擋住下一次開始");
        beat_booting(&t.0, 2_000_000).expect("boot again");
        assert!(is_occupied(&t.0, 2_000_000), "蓋得掉墓碑才叫再開一次");
        beat(&t.0, 2_000_100).expect("beat again");
        assert!(is_recording(&t.0, 2_000_100));
    }

    /// 「檔案不在」現在**只剩一個意思**——這是 #66 的全部內容。四種狀況以前
    /// 有三種長成同一個 `None`，而其中一種曾經被拿去守一個 kill。
    #[test]
    fn the_four_ways_of_nobody_are_four_different_answers() {
        let never = Tmp::new("p-never");
        assert_eq!(presence(&never.0, 1_000_000), Presence::NeverStarted);

        let broken = Tmp::new("p-broken");
        std::fs::write(beat_path(&broken.0), "not a number").expect("write");
        assert_eq!(
            presence(&broken.0, 1_000_000),
            Presence::Unreadable,
            "有人來過但話沒說完，不等於沒有人來過"
        );

        let stopped = Tmp::new("p-stopped");
        beat(&stopped.0, 1_000_000).expect("beat");
        stop(&stopped.0, 1_000_500);
        assert_eq!(
            presence(&stopped.0, 1_000_600),
            Presence::Stopped {
                at: Some(1_000_500)
            },
            "收工時間要是真的那一個"
        );

        let dead = Tmp::new("p-dead");
        beat(&dead.0, 1_000_000).expect("beat");
        assert_eq!(
            presence(&dead.0, 1_000_000 + STALE_AFTER_MS),
            Presence::Stalled {
                at: 1_000_000,
                phase: Phase::Recording
            },
            "沒回話的時候要說得出她最後一次講話是什麼時候"
        );

        // 而這四種對「要不要跟使用者說她在錄」是同一個答案。
        for dir in [&never.0, &broken.0, &stopped.0, &dead.0] {
            assert!(!is_recording(dir, 1_000_000 + STALE_AFTER_MS));
        }
    }

    /// **「我 stat 不到那個資料夾」不是「沒有人來過」。**
    ///
    /// 上一版的 `presence` 在 `read_to_string` 回 `None` 之後補一句
    /// `beat_path().exists()` 去分辨兩種「沒讀到」。那句話在權限拿不到的時候
    /// 回 `false`（`Path::exists` 的文件就是這樣寫的），於是最說不準的那一種
    /// 掉進 `NeverStarted`——`safe_to_kill_spawn` 唯一放行的那一種。
    ///
    /// 這條測試**先量前提再斷言結論**：資料夾關掉之後，`read_to_string` 要真
    /// 的失敗、而且失敗的原因不是 `NotFound`。前提不成立（例如用 root 跑）的
    /// 話就直接紅，不要靜靜跳過——一條「在這台機器上什麼都沒驗到」的綠燈，比
    /// 沒有這條測試更糟。
    #[cfg(unix)]
    #[test]
    fn a_data_dir_we_cannot_even_stat_is_not_a_data_dir_nobody_has_used() {
        use std::os::unix::fs::PermissionsExt;

        let t = Tmp::new("p-locked");
        beat(&t.0, 1_000_000).expect("beat");

        let shut = |mode| {
            std::fs::set_permissions(&t.0, std::fs::Permissions::from_mode(mode)).expect("chmod");
        };
        shut(0o000);
        // 關著的時候把要問的都問完，再開回去——斷言失敗會 panic，而一個 0o000
        // 的資料夾會讓 `TempDir` 的 `Drop` 清不掉，測試失敗就變成留一地垃圾。
        let err = std::fs::read_to_string(beat_path(&t.0)).map(|_| ()).err();
        let seen = presence(&t.0, 1_000_000);
        let knife = safe_to_kill_spawn(&t.0, 999_000, 1_000_000);
        shut(0o755);

        let kind = err.map(|e| e.kind());
        assert!(
            kind.is_some_and(|k| k != std::io::ErrorKind::NotFound),
            "前提沒成立：資料夾關掉之後還讀得到（或說它不在）。\
             這條測試要非 root 才驗得到東西，實際讀到的是 {kind:?}"
        );
        assert_eq!(
            seen,
            Presence::Unreadable,
            "說不準的時候要說「說不準」，不是「沒有人來過」"
        );
        assert!(
            !knife,
            "她可能正在錄，而這一刀是照著「這裡從來沒有人跑過」下的"
        );
    }

    /// 墓碑**不會過期**。她收工了就是收工了，隔一年開機還是收工——過期是心跳
    /// 才有的性質。這一條掉了的話，`Stopped` 會在 16 秒後變成 `Stalled`
    /// （「她當掉了」），於是那個 kill 又拿回一個它不該拿到的綠燈。
    #[test]
    fn a_tombstone_never_goes_stale() {
        let t = Tmp::new("tomb-age");
        stop(&t.0, 1_000_000);
        assert_eq!(
            presence(&t.0, 1_000_000 + 365 * 24 * 3_600_000),
            Presence::Stopped {
                at: Some(1_000_000)
            }
        );
    }

    /// 墓碑不是一次心跳。這支函式的呼叫端拿它去講「最後一次心跳是幾秒前」，
    /// 而 `0` 會變成「五十六年前」。
    #[test]
    fn a_tombstone_is_not_a_beat() {
        let t = Tmp::new("tomb-beat");
        beat(&t.0, 1_000_000).expect("beat");
        assert_eq!(last_beat(&t.0), Some(1_000_000));
        stop(&t.0, 1_000_001);
        assert_eq!(last_beat(&t.0), None);
    }

    /// **一個舊的 `sister.exe` 讀到這塊墓碑，不可以說她在錄。**
    ///
    /// 這是整個改動唯一微妙的地方，所以這裡不寫「我相信舊版會怎樣」，而是把
    /// alpha.41 那版 `read_beat` 原樣抄過來跑一次。它認不得 `stopped`，會走
    /// 「不認得的第二欄 = Recording」那條 forward-compat；救回正確答案的是時
    /// 戳那個 `0`，它讓舊版走過期那條路。
    ///
    /// 這條紅掉的時候不要改這個測試——改 [`stop`] 寫出去的那一行。
    #[test]
    fn an_old_exe_reading_this_tombstone_still_says_nobody_is_recording() {
        // ↓↓↓ alpha.41 `heartbeat.rs` 的 `read_beat` + `phase`，一字不改。
        fn old_read_beat(data_dir: &Path) -> Option<(Millis, Phase)> {
            let raw = std::fs::read_to_string(beat_path(data_dir)).ok()?;
            let mut fields = raw.split_whitespace();
            let ts: Millis = fields.next()?.parse().ok()?;
            let phase = match fields.next() {
                Some("boot") => Phase::Booting,
                _ => Phase::Recording,
            };
            Some((ts, phase))
        }
        fn old_phase(data_dir: &Path, now: Millis) -> Option<Phase> {
            let (beat, phase) = old_read_beat(data_dir)?;
            (now - beat < STALE_AFTER_MS).then_some(phase)
        }
        // ↑↑↑

        let t = Tmp::new("old-reader");
        let now = crate::now_ms();
        stop(&t.0, now);

        // 前提：舊版真的讀得到這個檔、而且真的認不得 `stopped`——不然下面那句
        // 綠燈是「檔案根本沒被讀到」冒充的。
        let (ts, phase) = old_read_beat(&t.0).expect("舊版要讀得到這個檔");
        assert_eq!(phase, Phase::Recording, "前提：舊版把 `stopped` 讀成在錄");
        assert_eq!(ts, 0, "救回答案的是這個 0");

        assert_eq!(old_phase(&t.0, now), None, "舊版不可以說她在錄");

        // 這個手法的**立足點**寫成斷言，不寫成註解。舊版說「她在錄」的條件是
        // `now - 0 < STALE_AFTER_MS`，所以只要那台機器的時鐘走過 1970 年的第
        // 16 秒，它就說不出那句話。時鐘真的歸零的話沒有任何一種墓碑救得了它
        // ——往未來寫也不行，舊版把未來當活的。那一格記在這裡，不假裝它不存在。
        assert_eq!(old_phase(&t.0, STALE_AFTER_MS), None, "第一個安全的時刻");
        assert_eq!(
            old_phase(&t.0, STALE_AFTER_MS - 1),
            Some(Phase::Recording),
            "而這是它救不了的那一格：時鐘停在 1970-01-01T00:00:15Z"
        );
    }

    /// 寫不進去（磁碟滿、權限）就退回舊行為。「檔案不在」的意思會再度變糊，
    /// 但比留下一個新鮮的心跳說她還在錄好。
    #[test]
    fn a_tombstone_that_cannot_be_written_falls_back_to_deleting() {
        let t = Tmp::new("tomb-fail");
        beat(&t.0, 1_000_000).expect("beat");
        // 把 tmp 檔的位置佔成一個目錄：`std::fs::write` 一定失敗。
        std::fs::create_dir_all(beat_path(&t.0).with_extension("beat.tmp")).expect("mkdir");
        stop(&t.0, 1_000_001);
        // 斷言要挑一句**只有退路走得出來**的話。`!is_recording` 兩條路都成立
        // ——墓碑寫成功也是「沒在錄」——所以拿它當斷言等於沒有斷言。
        assert_eq!(
            presence(&t.0, 1_000_002),
            Presence::Unreadable,
            "退路要留一個「有人來過但說不出是誰」，不是「從來沒有人來過」"
        );
        // 而這才是重點：退路**不可以把刀遞出去**。第一版這裡是 `remove_file`，
        // 於是磁碟滿的時候字母人讀到 `NeverStarted` 就放行落刀，而她正卡在
        // `rec.finish()` 寫 `end_session`——這個改動要防的那一刀，從錯誤路徑
        // 上走回來了。
        assert!(
            !safe_to_kill_spawn(&t.0, 999_999, 1_000_002),
            "寫不出墓碑不等於沒有人來過，不可以因此放行落刀"
        );
        assert!(!is_recording(&t.0, 1_000_002), "但也不可以說她還在錄");
    }

    /// 使用者調了時鐘，或者兩個行程對時差了一點。往未來偏不可以被讀成
    /// 「她死了」——那會讓人去重開一個已經在跑的 recorder。
    #[test]
    fn a_beat_from_the_future_is_still_alive() {
        let t = Tmp::new("future");
        beat(&t.0, 2_000_000).expect("beat");
        assert!(is_recording(&t.0, 1_000_000));
    }

    /// 內容壞掉（寫到一半斷電）當作沒在錄，不是 panic、也不是「還在錄」。
    #[test]
    fn a_corrupt_beat_reads_as_nobody_recording() {
        let t = Tmp::new("corrupt");
        std::fs::write(beat_path(&t.0), "not a number").expect("write");
        assert!(!is_recording(&t.0, 1_000_000));
    }

    /// 落刀的閘門。**每一種說不準的都不可以放行**——這裡放行一次的代價是
    /// 一場錄製的 `end_session` 永遠是 NULL，然後 `doctor` 說「她當掉了」。
    #[test]
    fn the_only_moment_the_knife_is_safe_is_before_she_opened_the_db() {
        const SPAWN: Millis = 5_000_000;
        let now = SPAWN + 500;

        // 全新的機器：這個資料目錄從來沒有人跑過。這是唯一乾淨的那一種。
        let never = Tmp::new("k-never");
        assert!(safe_to_kill_spawn(&never.0, SPAWN, now));

        // 昨天錄過、乾淨收工過的機器。檔案**在**，而且是墓碑——`NeverStarted`
        // 在這台機器上永遠不會再出現，所以分得出來的只有時戳。
        let yesterday = Tmp::new("k-yesterday");
        stop(&yesterday.0, SPAWN - 86_400_000);
        assert!(
            safe_to_kill_spawn(&yesterday.0, SPAWN, now),
            "昨天那塊墓碑不可以擋住今天這一刀"
        );

        // 她**正在收尾**：墓碑比我 spawn 還新。`stop` 寫在 `rec.finish()` 之
        // 前，這一刀會讓 `end_session` 永遠是 NULL。這是最容易踩到的一種——
        // 「停止記錄」和「結束」在同一張選單上，中間隔 0.4 秒。
        let winding_down = Tmp::new("k-winding");
        stop(&winding_down.0, SPAWN + 400);
        assert!(!safe_to_kill_spawn(&winding_down.0, SPAWN, now));

        // 沒有第三欄的舊墓碑：說不出是上一種還是上上一種，所以不動手。
        let old_tomb = Tmp::new("k-oldtomb");
        std::fs::write(beat_path(&old_tomb.0), "0 stopped").expect("write");
        assert_eq!(presence(&old_tomb.0, now), Presence::Stopped { at: None });
        assert!(!safe_to_kill_spawn(&old_tomb.0, SPAWN, now));

        // 她蓋過心跳了 = 她走過 `Db::open` 了。開機那一拍也算。
        for (name, write) in [
            ("k-live", beat as fn(&Path, Millis) -> Result<()>),
            ("k-boot", beat_booting),
        ] {
            let t = Tmp::new(name);
            write(&t.0, SPAWN + 100).expect("beat");
            assert!(!safe_to_kill_spawn(&t.0, SPAWN, now), "{name}");
        }

        // 收工想最後一段：錄製迴圈停了，行程還握著資料庫。
        let thinking = Tmp::new("k-thinking");
        beat_thinking(&thinking.0, SPAWN + 100, SPAWN + 240_000).expect("thinking");
        assert!(!safe_to_kill_spawn(&thinking.0, SPAWN, now));

        // 我這個 child 蓋過一拍就卡住了（prune 在刪幾萬個檔、休眠喚醒）。
        // 她沒死，她開著資料庫。
        let mine_stalled = Tmp::new("k-mine-stalled");
        beat(&mine_stalled.0, SPAWN + 100).expect("beat");
        assert!(!safe_to_kill_spawn(
            &mine_stalled.0,
            SPAWN,
            SPAWN + 100 + STALE_AFTER_MS
        ));

        // 上一場的屍體（沒收工就當掉）。比我早，跟我這個 child 無關。
        let old_corpse = Tmp::new("k-corpse");
        beat(&old_corpse.0, SPAWN - 3_600_000).expect("beat");
        assert!(safe_to_kill_spawn(&old_corpse.0, SPAWN, now));

        // 寫到一半斷電。說不出那半行是誰寫的，就不要動手。
        let broken = Tmp::new("k-broken");
        std::fs::write(beat_path(&broken.0), "not a number").expect("write");
        assert!(!safe_to_kill_spawn(&broken.0, SPAWN, now));
    }

    /// 蓋第二次要蓋得掉第一次。少了這一條，心跳只會活 16 秒。
    #[test]
    fn beating_again_moves_it_forward() {
        let t = Tmp::new("again");
        beat(&t.0, 1_000_000).expect("beat");
        beat(&t.0, 1_000_000 + 5_000).expect("beat again");
        assert_eq!(last_beat(&t.0), Some(1_005_000));
        assert!(is_recording(&t.0, 1_000_000 + STALE_AFTER_MS));
    }

    /// 停止之後、腦收工之前：沒在錄，但目錄還佔著。
    ///
    /// 這是按下停止之後那兩分鐘的整句話。心跳說 Recording 是謊（她不抓畫面
    /// 了）；心跳說 Stopped 也是謊（行程還握著資料庫）。兩句分開講。
    #[test]
    fn thinking_through_the_last_segment_occupies_but_does_not_record() {
        let t = Tmp::new("thinking");
        beat(&t.0, 1_000_000).expect("beat");
        beat_thinking(&t.0, 1_000_100, 1_240_100).expect("thinking");
        let now = 1_000_200;
        assert!(!is_recording(&t.0, now), "她已經不抓畫面了，不可以說在錄");
        assert!(is_occupied(&t.0, now), "行程還在，第二個 recorder 不准進來");
        assert_eq!(
            presence(&t.0, now),
            Presence::Thinking {
                at: 1_000_100,
                until: 1_240_100
            }
        );
        assert_eq!(
            phase(&t.0, now),
            None,
            "phase 是「在不在錄」，想最後一段不是"
        );
        let why = occupied_why(&t.0, now).expect("佔著就要說為什麼");
        assert!(why.contains("想最後一段"), "{why}");
        assert!(why.contains("秒"), "{why}");
        assert!(!why.contains("已經有一個 sister record 在跑了"), "{why}");
        assert!(!why.contains("正在起來"), "{why}");
    }

    /// 上限到了還沒改成墓碑：沒回話，不再佔著——他可以再開一個。
    #[test]
    fn a_thinking_beat_expires_at_the_deadline_it_wrote() {
        let t = Tmp::new("thinking-exp");
        beat_thinking(&t.0, 1_000_000, 1_240_000).expect("thinking");
        assert!(is_occupied(&t.0, 1_239_999));
        assert!(
            !is_occupied(&t.0, 1_240_000),
            "到點就是到點，不靠 16 秒逾時"
        );
        assert!(!is_recording(&t.0, 1_240_000));
        // 過了上限仍然不可以落刀：她可能卡在 finish()，只是逾時了。
        assert!(!safe_to_kill_spawn(&t.0, 999_000, 1_240_000));
    }

    /// 想最後一段不走 16 秒逾時。那 16 秒是「她沒回話」；這裡她話寫在檔案裡。
    #[test]
    fn thinking_does_not_go_stale_at_the_recording_timeout() {
        let t = Tmp::new("thinking-fresh");
        beat_thinking(&t.0, 1_000_000, 1_000_000 + 240_000).expect("thinking");
        let after_stale = 1_000_000 + STALE_AFTER_MS + 1;
        assert!(
            is_occupied(&t.0, after_stale),
            "16 秒後還在上限內，不可以被讀成沒人佔著"
        );
        assert!(!is_recording(&t.0, after_stale));
    }

    /// 三種佔著三句話。印成同一句，按下去的人就不知道下一步是等、是停、還是
    /// 不要再開一個。
    #[test]
    fn the_three_ways_of_occupied_are_three_different_sentences() {
        let rec = Tmp::new("why-rec");
        beat(&rec.0, 1_000_000).expect("beat");
        let boot = Tmp::new("why-boot");
        beat_booting(&boot.0, 1_000_000).expect("boot");
        let think = Tmp::new("why-think");
        beat_thinking(&think.0, 1_000_000, 1_240_000).expect("thinking");
        let a = occupied_why(&rec.0, 1_000_000).expect("rec");
        let b = occupied_why(&boot.0, 1_000_000).expect("boot");
        let c = occupied_why(&think.0, 1_000_000).expect("think");
        assert_ne!(a, b, "在錄和正在起來印成同一句");
        assert_ne!(a, c, "在錄和想最後一段印成同一句");
        assert_ne!(b, c, "正在起來和想最後一段印成同一句");
        assert!(occupied_why(&Tmp::new("why-none").0, 1_000_000).is_none());
    }

    /// 墓碑蓋得掉「想最後一段」。否則收工之後那 240 秒裡他再開一次會被擋住。
    #[test]
    fn a_tombstone_clears_thinking() {
        let t = Tmp::new("think-then-stop");
        beat_thinking(&t.0, 1_000_000, 1_240_000).expect("thinking");
        stop(&t.0, 1_000_500);
        assert!(!is_occupied(&t.0, 1_000_600));
        assert!(!is_recording(&t.0, 1_000_600));
        assert_eq!(
            presence(&t.0, 1_000_600),
            Presence::Stopped {
                at: Some(1_000_500)
            }
        );
    }

    /// 系統匣那顆寫著「停止記錄」的時候，按下去要有人讀那個檔。想最後一段
    /// 沒有人讀——迴圈已經跳出。三種佔著三個下一步，一個布林湊不出來。
    #[test]
    fn the_tray_does_not_advertise_a_stop_nobody_will_read() {
        let rec = Tmp::new("tray-rec");
        beat(&rec.0, 1_000_000).expect("beat");
        let boot = Tmp::new("tray-boot");
        beat_booting(&boot.0, 1_000_000).expect("boot");
        let think = Tmp::new("tray-think");
        beat_thinking(&think.0, 1_000_000, 1_240_000).expect("thinking");
        let none = Tmp::new("tray-none");

        let rec_p = presence(&rec.0, 1_000_000);
        let boot_p = presence(&boot.0, 1_000_000);
        let think_p = presence(&think.0, 1_000_000);
        let none_p = presence(&none.0, 1_000_000);

        assert_eq!(tray_record_action(rec_p), TrayRecordAction::Stop);
        assert_eq!(tray_record_action(boot_p), TrayRecordAction::Stop);
        assert_eq!(
            tray_record_action(think_p),
            TrayRecordAction::WaitForThinking,
            "想最後一段再寫 stop.request，沒有人會讀"
        );
        assert_eq!(tray_record_action(none_p), TrayRecordAction::Start);

        assert_eq!(tray_record_label(rec_p), "停止記錄");
        assert_eq!(tray_record_label(boot_p), "停止記錄");
        assert_eq!(tray_record_label(think_p), "還在收尾");
        assert_eq!(tray_record_label(none_p), "開始記錄");
        assert_ne!(
            tray_record_label(think_p),
            "停止記錄",
            "寫著停止、按下去什麼都不發生"
        );
        assert_ne!(
            tray_record_label(think_p),
            "開始記錄",
            "寫著開始、按下去會被佔著閘門擋下來"
        );

        assert_eq!(tray_quit_label(rec_p), "結束（記錄也會停）");
        assert_eq!(tray_quit_label(think_p), "結束（還在收尾）");
        assert_eq!(tray_quit_label(none_p), "結束");

        assert_eq!(watching_word(rec_p), "recording");
        assert_eq!(watching_word(boot_p), "booting");
        assert_eq!(watching_word(think_p), "thinking");
        assert_eq!(watching_word(none_p), "none");
        assert_ne!(
            watching_word(think_p),
            "none",
            "想最後一段壓成 none，設定頁會叫他去按開始"
        );
    }

    /// 最新那一列是她的：正在錄，或正在想最後一段。開機不是。
    #[test]
    fn an_open_session_row_is_hers_while_recording_or_thinking() {
        assert!(session_row_is_hers(Presence::Live(Phase::Recording)));
        assert!(session_row_is_hers(Presence::Thinking { at: 1, until: 2 }));
        assert!(!session_row_is_hers(Presence::Live(Phase::Booting)));
        assert!(!session_row_is_hers(Presence::NeverStarted));
        assert!(!session_row_is_hers(Presence::Stopped { at: None }));
        assert!(!session_row_is_hers(Presence::Stalled {
            at: 1,
            phase: Phase::Recording
        }));
        assert!(!session_row_is_hers(Presence::Unreadable));
    }
}
