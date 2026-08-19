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
