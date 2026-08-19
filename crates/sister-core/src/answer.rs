//! 從使用者的說法直接回答，繞過全文比對。
//!
//! 螢幕上寫的是「客服**專線**」，使用者問的是「電話」——全文檢索永遠接不起
//! 這兩個詞，但 L1 早就把那串數字標成 `phone` 了。這裡就是把使用者的說法接到
//! 事實型別上，然後直接回答。純查表、零模型。
//!
//! **這一層放在 core，因為兩個介面都要用同一份。** 它本來只長在 `sister-cli`
//! 裡，於是 `sister query 電話` 答得出號碼、字母人問同一句話卻只做全文比對，
//! 兩邊得到完全不同的答案——而字母人正是他每天真的會用的那一個。同一個教訓
//! 已經在 [`crate::question::shape`] 和 [`crate::config::Config::db_path`]
//! 各發生過一次了。

use std::collections::HashMap;

use crate::db::{Db, FactRow};

/// 一個答案：正規化後的值，加上「最後一次在哪裡看到它」。
#[derive(Debug, Clone)]
pub struct Answer {
    pub latest: FactRow,
    /// 這個值一共被看見過幾次。3 次目擊是同一個答案，不是三個答案。
    pub sightings: usize,
}

/// 依使用者的問法查 L1 事實。認不出問的是哪一類就回空集合——不要亂猜一堆
/// 事實塞給他。
pub fn answers(db: &Db, query: &str, limit: usize) -> anyhow::Result<Vec<Answer>> {
    let mut rows = Vec::new();
    for kind in crate::facts::kinds_for_query(query) {
        rows.extend(db.facts_by_kind(kind.as_str(), limit * 4)?);
    }

    // 同一個號碼在三個畫面出現過，是同一個答案、三次目擊——不是三個答案。
    // 併成一筆並保留最近一次的出處，因為使用者要追的是「最後看到它的地方」。
    let mut order: Vec<String> = Vec::new();
    let mut merged: HashMap<String, Answer> = HashMap::new();
    for row in rows {
        match merged.get_mut(&row.normalized) {
            Some(a) => {
                a.sightings += 1;
                if row.ts > a.latest.ts {
                    a.latest = row;
                }
            }
            None => {
                order.push(row.normalized.clone());
                merged.insert(
                    row.normalized.clone(),
                    Answer {
                        latest: row,
                        sightings: 1,
                    },
                );
            }
        }
    }

    let mut out: Vec<Answer> = order
        .into_iter()
        .filter_map(|k| merged.remove(&k))
        .collect();
    out.sort_by_key(|a| std::cmp::Reverse(a.latest.ts));
    out.truncate(limit);
    Ok(out)
}

/// 一筆都沒找到的時候，她**查得到**的那幾個理由。
///
/// 原本那句話是「她可能當時沒在看，或那段被排除規則擋掉了」——兩個猜測、零個
/// 證據，而兩件事她其實都答得出來：排除稽核和暫停稽核都在資料庫裡，`sister
/// stats` 早就在印了。字母人那邊更糟，它講的是「這件事我沒看到過」，一句斷言
/// ——而正確答案可能是「你自己叫我不要看那個網站」。
///
/// 這裡只回**事實**，不回句子：終端機和字母人的講法不一樣，但根據要是同一份。
/// 同一條紀律見這個模組開頭。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BlindSpots {
    /// 她一共記過幾段文字。`0` = 她根本還沒開始記，那時候該講的是下一步。
    pub chunks: i64,
    /// 排除規則生效過的（理由, 段數）。**段不是張**——見
    /// [`Db::exclusion_audit`](crate::db::Db::exclusion_audit)。
    pub excluded: Vec<(String, i64)>,
    /// 暫停過幾段。
    pub paused_episodes: i64,
    /// 暫停一共多久。有一段配不起來的時候這個數字會偏小，
    /// 見 [`Db::pause_audit`](crate::db::Db::pause_audit)。
    pub paused_ms: i64,
}

impl BlindSpots {
    /// 有沒有任何一個查得到的理由。`false` = 她真的記了，而裡面就是沒有。
    pub fn any(&self) -> bool {
        self.chunks == 0 || !self.excluded.is_empty() || self.paused_episodes > 0
    }
}

/// 查出 [`BlindSpots`]。
///
/// 範圍是整顆資料庫而不是某個時間窗：她不知道使用者心裡想的是哪一段，而把
/// 「上禮拜二下午」猜錯之後給出的理由，比不給理由更糟。所以講的是「她記過的
/// 這段期間裡」，而每一條都附得出時間讓人自己去對。
pub fn blind_spots(db: &Db) -> anyhow::Result<BlindSpots> {
    let stats = db.stats()?;
    let pauses = db.pause_audit()?;
    Ok(BlindSpots {
        chunks: stats.chunks,
        excluded: db
            .exclusion_audit()?
            .into_iter()
            .map(|e| (e.reason, e.episodes))
            .collect(),
        paused_episodes: pauses.episodes,
        paused_ms: pauses.total_ms,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{SystemEvent, SystemKind};

    /// 一顆全新的資料庫上，「沒找到」只有一個理由，而它不是「我沒看過那件事」。
    #[test]
    fn a_database_that_never_recorded_says_so_instead_of_denying_it() {
        let db = Db::open_in_memory().expect("db");
        let b = blind_spots(&db).expect("blind");
        assert_eq!(b.chunks, 0);
        assert!(b.any(), "「我還沒開始記」本身就是一個查得到的理由");
    }

    /// 她記了，但那段時間他自己叫她別看。這才是那句「這件事我沒看到過」最
    /// 可能說錯話的場合——東西在，只是她不准看。
    #[test]
    fn the_rules_he_wrote_himself_are_a_reason_she_can_actually_point_at() {
        let mut db = Db::open_in_memory().expect("db");
        let s = db.start_session("test", "0.0.1").expect("session");
        for (kind, detail, ts) in [
            (SystemKind::Excluded, Some("excluded url"), 1_000),
            (SystemKind::Excluded, Some("excluded url"), 2_000),
            (SystemKind::Excluded, Some("excluded app: keepassxc"), 3_000),
            (SystemKind::CapturePaused, None, 4_000),
            (SystemKind::CaptureResumed, None, 9_000),
        ] {
            db.insert_system(
                s,
                &SystemEvent {
                    ts,
                    kind,
                    detail: detail.map(str::to_string),
                },
            )
            .expect("system event");
        }

        let b = blind_spots(&db).expect("blind");
        assert!(b.any());
        // 段數多的排前面——「12 段」比「1 段」更值得他去看
        assert_eq!(
            b.excluded,
            vec![
                ("excluded url".to_string(), 2),
                ("excluded app: keepassxc".to_string(), 1),
            ]
        );
        assert_eq!(b.paused_episodes, 1);
        assert_eq!(b.paused_ms, 5_000);
    }
}
