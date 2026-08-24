//! 可攜、可重現的 L0 replay 語料。
//!
//! 這不是 [`crate::db::Db`] 的備份格式。它只帶重建搜尋索引與 L1 事實需要的
//! 訊號，不帶 PNG、圖片 bytes、資料庫 row id 或原始圖片路徑。真實資料先經
//! [`Corpus::deidentify`] 變成 [`DraftCorpus`]；人工看過並明確標成
//! [`ReviewStatus::Reviewed`] 以前，不是可以分享的檔案。

use std::collections::BTreeMap;

use anyhow::{Result, bail, ensure};
use serde::{Deserialize, Serialize};

use crate::facts::{FactKind, extract};
use crate::model::{ClipboardKind, FocusKind, Millis, OcrBlock, SystemKind};
use crate::redact::looks_like_secret;

pub const FORMAT_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewStatus {
    /// 已自動去敏，但還沒有人逐項看過；只能留在本機。
    Draft,
    /// 人工確認沒有不該分享的內容。
    Reviewed,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RedactionSummary {
    pub money: usize,
    pub phone: usize,
    pub email: usize,
    pub id_like: usize,
    pub secrets: usize,
}

impl RedactionSummary {
    pub fn total(&self) -> usize {
        self.money + self.phone + self.email + self.id_like + self.secrets
    }
}

/// JSON 最外層。所有時間都是語料起點之後的相對毫秒。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Corpus {
    pub format_version: u32,
    pub name: String,
    pub duration_ms: Millis,
    pub review: ReviewStatus,
    pub redactions: RedactionSummary,
    pub events: Vec<Event>,
}

/// 編譯期分得出「自動掃過」和「人工看過」。序列化仍是同一份公開 JSON 格式。
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(transparent)]
pub struct DraftCorpus(Corpus);

/// 只有 JSON 裡已明確標成 `reviewed` 的 corpus 才能建出這個型別。
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(transparent)]
pub struct ReviewedCorpus(Corpus);

impl DraftCorpus {
    pub fn as_corpus(&self) -> &Corpus {
        &self.0
    }

    pub fn into_inner(self) -> Corpus {
        self.0
    }
}

impl ReviewedCorpus {
    pub fn as_corpus(&self) -> &Corpus {
        &self.0
    }

    pub fn into_inner(self) -> Corpus {
        self.0
    }
}

impl TryFrom<Corpus> for DraftCorpus {
    type Error = anyhow::Error;

    fn try_from(corpus: Corpus) -> Result<Self> {
        corpus.validate()?;
        ensure!(
            corpus.review == ReviewStatus::Draft,
            "corpus review 不是 draft"
        );
        Ok(Self(corpus))
    }
}

impl TryFrom<Corpus> for ReviewedCorpus {
    type Error = anyhow::Error;

    fn try_from(corpus: Corpus) -> Result<Self> {
        corpus.validate()?;
        ensure!(
            corpus.review == ReviewStatus::Reviewed,
            "這份 corpus 還是 Draft；人工逐項審查並標成 Reviewed 後才能分享"
        );
        Ok(Self(corpus))
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReplayFocus {
    pub app_id: Option<String>,
    pub app_name: Option<String>,
    pub window_title: Option<String>,
    pub url: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Event {
    Frame {
        at_ms: Millis,
        monitor: i32,
        width: u32,
        height: u32,
        dhash: u64,
        dup_run: u32,
        focus: ReplayFocus,
        ocr: Vec<OcrBlock>,
    },
    Focus {
        at_ms: Millis,
        kind: FocusKind,
        snapshot: ReplayFocus,
    },
    Clipboard {
        at_ms: Millis,
        kind: ClipboardKind,
        text: Option<String>,
        byte_len: i64,
        truncated: bool,
        secret_suspected: bool,
        source_app: Option<String>,
    },
    Input {
        at_ms: Millis,
        end_ms: Millis,
        keystrokes: i64,
        clicks: i64,
        mouse_px: i64,
        scroll_ticks: i64,
        window_switches: i64,
        idle_ms: i64,
        typing_bursts: i64,
    },
    System {
        at_ms: Millis,
        kind: SystemKind,
        detail: Option<String>,
    },
}

impl Event {
    pub fn at_ms(&self) -> Millis {
        match self {
            Self::Frame { at_ms, .. }
            | Self::Focus { at_ms, .. }
            | Self::Clipboard { at_ms, .. }
            | Self::Input { at_ms, .. }
            | Self::System { at_ms, .. } => *at_ms,
        }
    }
}

impl Corpus {
    /// 在寫入任何資料以前拒絕壞掉或不是這一版的 corpus。
    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.format_version == FORMAT_VERSION,
            "不支援 replay corpus format_version {}（這版只支援 {}）",
            self.format_version,
            FORMAT_VERSION
        );
        ensure!(!self.name.trim().is_empty(), "replay corpus 缺少名稱");
        ensure!(
            self.duration_ms >= 0,
            "replay corpus duration_ms 不可為負數"
        );

        let mut previous = None;
        for (index, event) in self.events.iter().enumerate() {
            let at = event.at_ms();
            ensure!(
                (0..=self.duration_ms).contains(&at),
                "replay event #{index} 的 at_ms={at} 不在 0..={} 內",
                self.duration_ms
            );
            if let Some(before) = previous {
                ensure!(
                    at >= before,
                    "replay event #{index} 的時間 {at} 早於前一筆 {before}"
                );
            }
            previous = Some(at);

            match event {
                Event::Frame {
                    width, height, ocr, ..
                } => {
                    ensure!(
                        *width > 0 && *height > 0,
                        "replay frame event #{index} 的尺寸必須大於 0"
                    );
                    ensure!(
                        ocr.iter().all(|block| block.confidence.is_finite()),
                        "replay frame event #{index} 有非有限 OCR confidence"
                    );
                }
                Event::Clipboard {
                    byte_len,
                    text,
                    secret_suspected,
                    ..
                } => {
                    ensure!(
                        *byte_len >= 0,
                        "replay clipboard event #{index} 的 byte_len 不可為負數"
                    );
                    ensure!(
                        !(text.is_some() && *secret_suspected),
                        "replay clipboard event #{index} 同時帶著 secret_suspected 和內容"
                    );
                }
                Event::Input {
                    end_ms,
                    keystrokes,
                    clicks,
                    mouse_px,
                    scroll_ticks,
                    window_switches,
                    idle_ms,
                    typing_bursts,
                    ..
                } => {
                    ensure!(
                        *end_ms >= at && *end_ms <= self.duration_ms,
                        "replay input event #{index} 的 end_ms={end_ms} 不在事件起點與語料終點之間"
                    );
                    ensure!(
                        [
                            *keystrokes,
                            *clicks,
                            *mouse_px,
                            *scroll_ticks,
                            *window_switches,
                            *idle_ms,
                            *typing_bursts,
                        ]
                        .into_iter()
                        .all(|value| value >= 0),
                        "replay input event #{index} 有負的計數"
                    );
                }
                Event::System { kind, .. } if kind.is_session_mark() => bail!(
                    "replay event #{index} 不可帶 {}；import 會自己建立 session 邊界",
                    kind.as_str()
                ),
                _ => {}
            }
        }
        Ok(())
    }

    /// 自動掃過 corpus 裡的每一個文字表面，輸出只能留在本機的 Draft。
    ///
    /// 相同原文在整份 corpus 裡會換成相同假值，讓 replay 還能測跨事件檢索；
    /// 金額、電話、email 與類 ID 保留可被 L1 規則辨認的形狀。自動規則不可能
    /// 認得所有人名與私人語意，所以這一步刻意不能產生 [`ReviewedCorpus`]。
    pub fn deidentify(mut self) -> Result<DraftCorpus> {
        self.validate()?;
        let mut scrubber = Scrubber::default();
        scrubber.text(&mut self.name);

        for event in &mut self.events {
            match event {
                Event::Frame { focus, ocr, .. } => {
                    scrubber.focus(focus);
                    for block in ocr {
                        scrubber.text(&mut block.text);
                    }
                }
                Event::Focus { snapshot, .. } => scrubber.focus(snapshot),
                Event::Clipboard {
                    text, source_app, ..
                } => {
                    scrubber.optional(text);
                    scrubber.optional(source_app);
                }
                Event::Input { .. } => {}
                Event::System { detail, .. } => scrubber.optional(detail),
            }
        }

        self.review = ReviewStatus::Draft;
        self.redactions = scrubber.summary;
        self.validate()?;
        DraftCorpus::try_from(self)
    }
}

#[derive(Default)]
struct Scrubber {
    replacements: BTreeMap<(String, String), String>,
    summary: RedactionSummary,
}

impl Scrubber {
    fn focus(&mut self, focus: &mut ReplayFocus) {
        self.optional(&mut focus.app_id);
        self.optional(&mut focus.app_name);
        self.optional(&mut focus.window_title);
        self.optional(&mut focus.url);
    }

    fn optional(&mut self, value: &mut Option<String>) {
        if let Some(value) = value {
            self.text(value);
        }
    }

    fn text(&mut self, text: &mut String) {
        self.secrets(text);

        // 從後往前換，ExtractedFact 的 byte spans 才不會被前一次替換推歪。
        let facts = extract(text);
        for fact in facts.into_iter().rev() {
            if !matches!(
                fact.kind,
                FactKind::Money | FactKind::Phone | FactKind::Email | FactKind::IdLike
            ) {
                continue;
            }
            let fake = self.fake_fact(fact.kind, &fact.raw);
            text.replace_range(fact.byte_start..fact.byte_end, &fake);
            match fact.kind {
                FactKind::Money => self.summary.money += 1,
                FactKind::Phone => self.summary.phone += 1,
                FactKind::Email => self.summary.email += 1,
                FactKind::IdLike => self.summary.id_like += 1,
                _ => unreachable!(),
            }
        }

        // 任何一次替換都不能自己造出一把看起來像秘密的字串。
        if looks_like_secret(text).is_some() {
            *text = self.fake_secret(text);
            self.summary.secrets += 1;
        }
    }

    fn secrets(&mut self, text: &mut String) {
        if looks_like_secret(text).is_none() {
            return;
        }

        let mut out = String::with_capacity(text.len());
        let mut found = 0usize;
        for (candidate, piece) in split_keep_secret_boundaries(text) {
            if candidate && looks_like_secret(piece).is_some() {
                out.push_str(&self.fake_secret(piece));
                found += 1;
            } else {
                out.push_str(piece);
            }
        }

        if found == 0 || looks_like_secret(&out).is_some() {
            out = self.fake_secret(text);
            found = 1;
        }
        *text = out;
        self.summary.secrets += found;
    }

    fn fake_secret(&mut self, raw: &str) -> String {
        self.replacement("secret", raw, |n| format!("replay-secret-{n:04}"))
    }

    fn fake_fact(&mut self, kind: FactKind, raw: &str) -> String {
        let key = kind.as_str();
        self.replacement(key, raw, |n| match kind {
            FactKind::Money => fake_digits(raw, n, true),
            FactKind::Phone => fake_phone(raw, n),
            FactKind::Email => format!("person{n}@example.invalid"),
            FactKind::IdLike => fake_id(raw, n),
            _ => unreachable!(),
        })
    }

    fn replacement(&mut self, kind: &str, raw: &str, make: impl FnOnce(usize) -> String) -> String {
        let key = (kind.to_string(), raw.to_string());
        if let Some(value) = self.replacements.get(&key) {
            return value.clone();
        }
        let n = self.replacements.len() + 1;
        let value = make(n);
        self.replacements.insert(key, value.clone());
        value
    }
}

fn split_keep_secret_boundaries(text: &str) -> Vec<(bool, &str)> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut candidate = text
        .chars()
        .next()
        .map(is_secret_token_char)
        .unwrap_or(false);
    for (index, ch) in text.char_indices() {
        let now = is_secret_token_char(ch);
        if now != candidate {
            out.push((candidate, &text[start..index]));
            start = index;
            candidate = now;
        }
    }
    if start < text.len() {
        out.push((candidate, &text[start..]));
    }
    out
}

fn is_secret_token_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '+' | '/' | '.')
}

fn fake_digits(raw: &str, seed: usize, keep_leading_zero: bool) -> String {
    let mut digit_index = 0usize;
    raw.chars()
        .map(|ch| {
            if !ch.is_ascii_digit() {
                return ch;
            }
            let replacement = if keep_leading_zero && digit_index == 0 && ch == '0' {
                '0'
            } else {
                let digit = ((seed * 7 + digit_index * 3 + 1) % 10) as u8;
                (b'0' + digit) as char
            };
            digit_index += 1;
            replacement
        })
        .collect()
}

fn fake_phone(raw: &str, seed: usize) -> String {
    let digits: Vec<_> = raw.match_indices(char::is_numeric).collect();
    let first_two = digits.iter().take(2).map(|(_, s)| *s).collect::<String>();
    let mut index = 0usize;
    raw.chars()
        .map(|ch| {
            if !ch.is_ascii_digit() {
                return ch;
            }
            let replacement = if index < 2 && (first_two == "09" || first_two == "08") {
                first_two.as_bytes()[index] as char
            } else if index == 0 && ch == '0' {
                '0'
            } else {
                let digit = ((seed * 3 + index * 7 + 5) % 10) as u8;
                (b'0' + digit) as char
            };
            index += 1;
            replacement
        })
        .collect()
}

fn fake_id(raw: &str, seed: usize) -> String {
    let mut index = 0usize;
    raw.chars()
        .map(|ch| {
            if ch.is_ascii_digit() {
                let digit = ((seed * 5 + index * 3 + 1) % 10) as u8;
                index += 1;
                (b'0' + digit) as char
            } else if ch.is_ascii_alphabetic() {
                let base = if ch.is_ascii_uppercase() { b'A' } else { b'a' };
                let letter = ((seed * 11 + index * 5) % 26) as u8;
                index += 1;
                (base + letter) as char
            } else {
                ch
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn corpus(texts: &[&str]) -> Corpus {
        Corpus {
            format_version: FORMAT_VERSION,
            name: "ordinary corpus".into(),
            duration_ms: 2_000,
            review: ReviewStatus::Reviewed,
            redactions: RedactionSummary::default(),
            events: texts
                .iter()
                .enumerate()
                .map(|(i, text)| Event::Frame {
                    at_ms: i as i64 * 1_000,
                    monitor: 0,
                    width: 1920,
                    height: 1080,
                    dhash: i as u64,
                    dup_run: 0,
                    focus: ReplayFocus {
                        app_id: Some("terminal.exe".into()),
                        window_title: Some("正常視窗標題".into()),
                        ..Default::default()
                    },
                    ocr: vec![OcrBlock {
                        text: (*text).into(),
                        x: 10,
                        y: 20,
                        w: 300,
                        h: 40,
                        confidence: 0.9,
                    }],
                })
                .collect(),
        }
    }

    #[test]
    fn serde_round_trip_and_version_are_explicit() {
        let original = corpus(&["今天在修 replay"]);
        let json = serde_json::to_string_pretty(&original).expect("serialize");
        assert!(json.contains("\"format_version\": 1"), "{json}");
        assert!(!json.contains("image_path"), "{json}");
        let decoded: Corpus = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(decoded, original);

        let mut future = decoded;
        future.format_version += 1;
        assert!(future.validate().is_err());
    }

    #[test]
    fn validation_rejects_time_travel_and_session_container_marks() {
        let mut bad = corpus(&["a", "b"]);
        if let Event::Frame { at_ms, .. } = &mut bad.events[1] {
            *at_ms = -1;
        }
        assert!(bad.validate().is_err());

        let mut bad = corpus(&[]);
        bad.events.push(Event::System {
            at_ms: 0,
            kind: SystemKind::SessionStart,
            detail: None,
        });
        assert!(bad.validate().is_err());
    }

    #[test]
    fn deidentifies_every_supported_surface_but_keeps_normal_text() {
        let api_key = "ghp_16CharsAtLeastHereOk123";
        let original = format!(
            "一般說明還在；電話 0912-345-678，信箱 ted@example.com，帳單 NT$13,450，單號 ABCD-123456；{api_key}"
        );
        let mut source = corpus(&[&original, "一般說明還在"]);
        if let Event::Frame { focus, .. } = &mut source.events[0] {
            focus.window_title = Some("ted@example.com 的帳單 NT$13,450".into());
            focus.url = Some(format!("https://example.test/?token={api_key}"));
        }
        source.events.push(Event::Clipboard {
            at_ms: 2_000,
            kind: ClipboardKind::Text,
            text: Some(original.clone()),
            byte_len: original.len() as i64,
            truncated: false,
            secret_suspected: false,
            source_app: Some("terminal.exe".into()),
        });

        let draft = source.deidentify().expect("deidentify");
        let json = serde_json::to_string(&draft).expect("json");
        for secret in [
            api_key,
            "0912-345-678",
            "ted@example.com",
            "NT$13,450",
            "ABCD-123456",
        ] {
            assert!(!json.contains(secret), "洩漏 {secret}: {json}");
        }
        assert!(json.contains("一般說明還在"), "{json}");
        assert!(json.contains("\"review\":\"draft\""), "{json}");
        assert!(draft.as_corpus().redactions.total() >= 5);
        assert!(looks_like_secret(&json).is_none(), "{json}");
    }

    #[test]
    fn replacements_are_stable_and_still_rebuild_typed_facts() {
        let draft = corpus(&[
            "客服 0912-345-678，帳單 NT$13,450",
            "再抄一次 0912-345-678，帳單 NT$13,450",
        ])
        .deidentify()
        .expect("deidentify");
        let texts: Vec<_> = draft
            .as_corpus()
            .events
            .iter()
            .filter_map(|event| match event {
                Event::Frame { ocr, .. } => Some(&ocr[0].text),
                _ => None,
            })
            .collect();
        let first = extract(texts[0]);
        let second = extract(texts[1]);
        for kind in [FactKind::Phone, FactKind::Money] {
            let a = first.iter().find(|f| f.kind == kind).expect("first fact");
            let b = second.iter().find(|f| f.kind == kind).expect("second fact");
            assert_eq!(a.raw, b.raw);
        }
    }

    #[test]
    fn reviewed_newtype_cannot_be_built_from_a_draft() {
        let draft = corpus(&["normal"])
            .deidentify()
            .expect("draft")
            .into_inner();
        assert!(ReviewedCorpus::try_from(draft.clone()).is_err());
        assert!(DraftCorpus::try_from(draft).is_ok());
    }
}
