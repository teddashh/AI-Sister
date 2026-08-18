//! 把 OCR 引擎吐出來的一串「詞」重新組回一行字。
//!
//! ## 為什麼不直接用引擎給的整行字串
//!
//! Windows 的 `OcrLine::Text()` 是把底下的 `OcrWord` **用空白接起來**的。
//! 對英文那是對的；對中文就不是。如果引擎把「本期應繳金額」切成六個詞，
//! 接回來會變成「本 期 應 繳 金 額」——於是使用者搜「應繳」永遠搜不到，
//! L1 也抽不出金額，因為正規表示式看到的是被空白切斷的字串。
//!
//! 這個錯誤有這個專案最怕的形狀：**看起來一切正常，而且沒有人會發現。**
//! 畫面有錄到、OCR 有跑、資料庫在長大、`stats` 一片綠，只是搜不到而已。
//! 對照 THREAT_MODEL 的「安靜地不生效」：少擋了不會被發現，記錯了也不會。
//!
//! ## 改用幾何判斷
//!
//! 所以這裡不採信引擎給的整行字串，改成自己看每個詞的**位置**：兩個詞之間
//! 的空隙小於字高的某個比例，就是連著的；夠大才是真的有一個空白。
//!
//! 這條規則對中文和英文一視同仁。它不需要先判斷這行是哪國語言，
//! 也就不會在語言判斷錯的時候跟著失效——少一個會安靜出錯的環節。

/// OCR 引擎回報的一個詞：文字加上它在畫面上的位置。
#[derive(Debug, Clone, PartialEq)]
pub struct Word {
    pub text: String,
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Word {
    pub fn new(text: &str, x: f32, y: f32, w: f32, h: f32) -> Self {
        Self {
            text: text.to_string(),
            x,
            y,
            w,
            h,
        }
    }

    fn right(&self) -> f32 {
        self.x + self.w
    }

    fn bottom(&self) -> f32 {
        self.y + self.h
    }
}

/// 組好的一行：文字，加上整行的外接矩形。
#[derive(Debug, Clone, PartialEq)]
pub struct Line {
    pub text: String,
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

/// 詞距要超過字高的這個比例，才算是一個真的空白。
///
/// 這個數字的來源是排版而不是實驗室：中文字之間的縫大約是字高的 5%～15%，
/// 一個真正的空白大約是 30%～50%。0.28 落在中間，而且兩邊都留了餘裕。
///
/// 偏大或偏小的後果不對稱，所以值得說清楚：
/// - **太小**（把字縫誤判成空白）→「應 繳」被切開 → 搜不到 → **安靜地壞掉**
/// - **太大**（把空白誤判成字縫）→「客服專線0800」黏在一起 → 全文檢索照樣
///   命中子字串，L1 的電話規則也還是抽得到
///
/// 所以有疑慮時要往大的方向靠，讓它黏起來，不要讓它斷開。
const SPACE_RATIO: f32 = 0.28;

/// 把一行的詞組回一個字串。
///
/// 假設 `words` 已經是引擎給的**閱讀順序**（左到右）。這裡刻意不重新排序：
/// 引擎比我們更知道閱讀順序，而按 x 排序會在直排或多欄的版面上排出更糟的
/// 結果。空的、或全是空白的輸入回傳 `None`。
pub fn assemble_line(words: &[Word]) -> Option<Line> {
    let words: Vec<&Word> = words.iter().filter(|w| !w.text.trim().is_empty()).collect();
    let (first, rest) = words.split_first()?;

    let mut text = first.text.trim().to_string();
    let (mut x0, mut y0) = (first.x, first.y);
    let (mut x1, mut y1) = (first.right(), first.bottom());

    let mut prev = *first;
    for word in rest {
        // 字高用兩邊的較大值：一個矮的標點不該讓它旁邊的空白判定變嚴。
        // 下限 1.0 是為了不讓退化的（高度 0）方框把門檻變成 0。
        let height = prev.h.max(word.h).max(1.0);
        let gap = word.x - prev.right();
        if gap > SPACE_RATIO * height {
            text.push(' ');
        }
        text.push_str(word.text.trim());

        x0 = x0.min(word.x);
        y0 = y0.min(word.y);
        x1 = x1.max(word.right());
        y1 = y1.max(word.bottom());
        prev = word;
    }

    let text = text.trim().to_string();
    if text.is_empty() {
        return None;
    }

    Some(Line {
        text,
        x: x0.round() as i32,
        y: y0.round() as i32,
        w: (x1 - x0).round().max(0.0) as i32,
        h: (y1 - y0).round().max(0.0) as i32,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 造一排等寬、間距固定的方框，模擬引擎逐字切出來的結果。
    fn run(texts: &[&str], char_w: f32, gap: f32) -> Vec<Word> {
        let mut x = 10.0;
        let mut out = Vec::new();
        for t in texts {
            out.push(Word::new(t, x, 100.0, char_w, 20.0));
            x += char_w + gap;
        }
        out
    }

    /// 這是整個模組存在的理由：中文被逐字切開時，不可以組出「本 期 應 繳」。
    #[test]
    fn chinese_split_into_single_characters_is_rejoined_without_spaces() {
        // 字高 20，字縫 2（= 10%），遠小於 0.28 的門檻
        let words = run(&["本", "期", "應", "繳", "金", "額"], 20.0, 2.0);
        let line = assemble_line(&words).expect("應該組得出一行");
        assert_eq!(line.text, "本期應繳金額");
    }

    /// 反面：英文的詞距是真的空白，不可以被黏成一坨。
    #[test]
    fn english_words_keep_their_spaces() {
        // 字高 20，詞距 10（= 50%），大於門檻
        let words = run(&["Total", "due", "today"], 40.0, 10.0);
        let line = assemble_line(&words).expect("應該組得出一行");
        assert_eq!(line.text, "Total due today");
    }

    /// 中英混排：中文那段要黏，中文和數字之間畫面上有空白就要留。
    #[test]
    fn mixed_chinese_and_latin_follows_the_pixels_not_the_script() {
        let mut words = run(&["客", "服", "專", "線"], 20.0, 2.0);
        // 標籤和號碼之間有一個明顯的空白（12 > 0.28 * 20 = 5.6）
        let after = words.last().unwrap().right() + 12.0;
        words.push(Word::new("0800-080-123", after, 100.0, 120.0, 20.0));

        let line = assemble_line(&words).expect("應該組得出一行");
        assert_eq!(line.text, "客服專線 0800-080-123");
    }

    /// 方框重疊（負的空隙）不能被當成空白。
    #[test]
    fn overlapping_boxes_do_not_produce_a_space() {
        let words = vec![
            Word::new("金", 10.0, 100.0, 20.0, 20.0),
            Word::new("額", 28.0, 100.0, 20.0, 20.0), // 往回疊了 2px
        ];
        assert_eq!(assemble_line(&words).unwrap().text, "金額");
    }

    /// 外接矩形是所有詞的聯集，不是第一個詞的。
    #[test]
    fn bounding_box_covers_every_word() {
        let words = vec![
            Word::new("a", 10.0, 100.0, 20.0, 20.0),
            Word::new("b", 50.0, 96.0, 30.0, 28.0),
        ];
        let line = assemble_line(&words).unwrap();
        assert_eq!((line.x, line.y), (10, 96));
        assert_eq!((line.w, line.h), (70, 28)); // 10..80, 96..124
    }

    #[test]
    fn empty_and_blank_input_produces_nothing() {
        assert!(assemble_line(&[]).is_none());
        assert!(assemble_line(&run(&[" ", ""], 20.0, 2.0)).is_none());
    }

    /// 退化的方框（高度 0）不可以 panic，也不該把每個字都拆開。
    #[test]
    fn degenerate_boxes_do_not_panic() {
        let words = vec![
            Word::new("本", 10.0, 100.0, 0.0, 0.0),
            Word::new("期", 10.0, 100.0, 0.0, 0.0),
        ];
        assert_eq!(assemble_line(&words).unwrap().text, "本期");
    }

    /// 真正要守住的東西不是字串相等，是**這行字還能被 L1 抽出事實**。
    ///
    /// 逐字切開的中文如果被空白黏斷，金額與電話的規則就全部失效——
    /// 而那正是使用者唯一會注意到的症狀（「她什麼都查不到」）。
    #[test]
    fn a_reassembled_line_still_yields_l1_facts() {
        let mut words = run(&["本", "期", "應", "繳"], 20.0, 2.0);
        let after = words.last().unwrap().right() + 12.0;
        words.push(Word::new("NT$13,450", after, 100.0, 90.0, 20.0));

        let line = assemble_line(&words).expect("應該組得出一行");
        let facts = sister_core::facts::extract(&line.text);
        assert!(
            facts
                .iter()
                .any(|f| f.kind == sister_core::facts::FactKind::Money),
            "組回來的「{}」抽不出金額，L1 等於失效",
            line.text
        );
    }
}
