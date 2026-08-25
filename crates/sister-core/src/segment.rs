//! Segmenter v1：把 L0 事件流切成帶信心值的段落。純程式，零 LLM。
//!
//! 切刀與黏合**完全照 SPEC §4.1**，這裡不發明新訊號。規格沒寫死的數字
//! （工作集窗口、共現次數、大段剪貼簿）各自標成實作選擇，不要跟 §4.1
//! 的常數搞混。

use std::collections::{BTreeMap, HashMap, HashSet};

use serde::{Deserialize, Serialize};

use crate::model::{Millis, SystemKind};

/// SPEC §4.1：idle > 90s 後恢復才切。
pub const IDLE_RESUME_MS: Millis = 90_000;
/// SPEC §4.1：切換後 < 30s 返回原視窗則黏合。
pub const BOUNCE_BACK_MS: Millis = 30_000;
/// SPEC §4.1：強制時間上限 10 min。
pub const TIME_CAP_MS: Millis = 10 * 60 * 1_000;
/// SPEC §4.1〔定案〕：每段前後保留 5s 重疊 margin。
pub const OVERLAP_MARGIN_MS: Millis = 5_000;

/// 工作集的「短窗口」。SPEC §4.1 只說「短窗口內反覆共現」，沒給毫秒數。
/// 5 分鐘是實作選擇，不是規格常數。
pub const WORKSET_WINDOW_MS: Millis = 5 * 60 * 1_000;
/// 同一短窗口內至少出現幾次才算「反覆」。實作選擇，不是規格常數。
pub const WORKSET_MIN_APPEARANCES: u32 = 2;
/// 「大段」剪貼簿。SPEC 沒給位元組數；512 是實作選擇。
pub const LARGE_CLIPBOARD_BYTES: i64 = 512;

/// 開時間軸時往前後多讀這麼久，好讓黏合／閒置恢復看得到窗外的事件。
/// 取工作集窗口：它比 30s 折返和 90s 閒置都長。
pub const LOOKAROUND_MS: Millis = WORKSET_WINDOW_MS;

/// 觸發切刀的那一種訊號。字串是存進資料庫的值，改一個就要連測試一起改。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CutKind {
    AppChange,
    HostChange,
    IdleResume,
    Lock,
    Unlock,
    ClipboardPaste,
    TimeCap,
}

impl CutKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::AppChange => "app_change",
            Self::HostChange => "host_change",
            Self::IdleResume => "idle_resume",
            Self::Lock => "lock",
            Self::Unlock => "unlock",
            Self::ClipboardPaste => "clipboard_paste",
            Self::TimeCap => "time_cap",
        }
    }

    /// 講給人聽的那一句。每一句都對得上程式真的檢查過的那條規則。
    pub fn describe(self) -> &'static str {
        match self {
            Self::AppChange => "前景 app 變更",
            Self::HostChange => "瀏覽器 host 變更",
            Self::IdleResume => "idle 超過 90 秒後恢復",
            Self::Lock => "螢幕鎖定",
            Self::Unlock => "螢幕解鎖",
            Self::ClipboardPaste => "剪貼簿大段複製後切到另一個 app",
            Self::TimeCap => "滿 10 分鐘",
        }
    }

    pub fn from_str_kind(s: &str) -> Option<Self> {
        match s {
            "app_change" => Some(Self::AppChange),
            "host_change" => Some(Self::HostChange),
            "idle_resume" => Some(Self::IdleResume),
            "lock" => Some(Self::Lock),
            "unlock" => Some(Self::Unlock),
            "clipboard_paste" => Some(Self::ClipboardPaste),
            "time_cap" => Some(Self::TimeCap),
            _ => None,
        }
    }
}

/// 這一段是從哪些事件算出來的。缺的那種是空陣列，不是把欄位拿掉。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventRefs {
    #[serde(default)]
    pub focus: Vec<i64>,
    #[serde(default)]
    pub system: Vec<i64>,
    #[serde(default)]
    pub clipboard: Vec<i64>,
    #[serde(default)]
    pub input: Vec<i64>,
}

/// 斷出來的一段。`started_at`／`ended_at` 已含 5s 重疊 margin。
#[derive(Debug, Clone, PartialEq)]
pub struct Segment {
    pub started_at: Millis,
    pub ended_at: Millis,
    /// 不含 margin 的核心起點。重算某一天時用它判斷這一段算哪一天的。
    pub core_started_at: Millis,
    pub core_ended_at: Millis,
    pub app: Option<String>,
    pub title: Option<String>,
    pub host: Option<String>,
    /// 打開這一段的切刀。第一段沒有打開它的切刀，是 `None`。
    pub cut_kinds: Vec<CutKind>,
    /// 打開這一段那道邊界的信心。第一段沒有邊界可算，是 `None`。
    pub confidence: Option<f32>,
    pub event_ids: EventRefs,
    /// 套用過使用者編輯之後才有。演算法自己切的是 `None`。
    pub last_edit: Option<crate::segment_edit::AppliedEdit>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FocusPoint {
    pub id: i64,
    pub ts: Millis,
    pub app_id: Option<String>,
    pub app_name: Option<String>,
    pub window_title: Option<String>,
    pub url: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SystemPoint {
    pub id: i64,
    pub ts: Millis,
    pub kind: SystemKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClipboardPoint {
    pub id: i64,
    pub ts: Millis,
    pub byte_len: i64,
    pub source_app: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InputPoint {
    pub id: i64,
    pub ts_start: Millis,
    pub ts_end: Millis,
    pub idle_ms: i64,
}

/// Segmenter 的輸入。L1 事實不在這裡：§4.1 的 v1 切刀全是 L0 訊號。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EventStream {
    pub focus: Vec<FocusPoint>,
    pub system: Vec<SystemPoint>,
    pub clipboard: Vec<ClipboardPoint>,
    pub input: Vec<InputPoint>,
}

impl EventStream {
    pub fn is_empty(&self) -> bool {
        self.focus.is_empty()
            && self.system.is_empty()
            && self.clipboard.is_empty()
            && self.input.is_empty()
    }
}

/// 從 URL 抽出 host。沒有 host 可抽就回 `None`，不拿路徑或標題來充數。
pub fn url_host(url: &str) -> Option<String> {
    let s = url.trim();
    if s.is_empty() {
        return None;
    }
    let rest = match s.find("://") {
        Some(i) => s.get(i + 3..)?,
        None => s,
    };
    let hostport = rest
        .split(['/', '?', '#'])
        .next()
        .unwrap_or("")
        .rsplit('@')
        .next()
        .unwrap_or("");
    let host = if let Some(inner) = hostport.strip_prefix('[') {
        inner.split(']').next().unwrap_or("")
    } else {
        hostport.split(':').next().unwrap_or("")
    };
    let host = host.trim().trim_matches('.').to_ascii_lowercase();
    if !looks_like_host(&host) {
        return None;
    }
    Some(host)
}

fn looks_like_host(host: &str) -> bool {
    if host == "localhost" {
        return true;
    }
    if host.is_empty() || !host.contains('.') {
        return false;
    }
    host.bytes()
        .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-')
}

/// 把事件流切成段落。沒有事件就回空向量，不是一段假的全天。
pub fn segment(stream: &EventStream) -> Vec<Segment> {
    if stream.is_empty() {
        return Vec::new();
    }

    let mut focus = stream.focus.clone();
    focus.sort_by_key(|p| (p.ts, p.id));
    let mut system = stream.system.clone();
    system.sort_by_key(|p| (p.ts, p.id));
    let mut clipboard = stream.clipboard.clone();
    clipboard.sort_by_key(|p| (p.ts, p.id));
    let mut input = stream.input.clone();
    input.sort_by_key(|p| (p.ts_start, p.id));

    let stream_start = [
        focus.first().map(|p| p.ts),
        system.first().map(|p| p.ts),
        clipboard.first().map(|p| p.ts),
        input.first().map(|p| p.ts_start),
    ]
    .into_iter()
    .flatten()
    .min();
    let stream_end = [
        focus.last().map(|p| p.ts),
        system.last().map(|p| p.ts),
        clipboard.last().map(|p| p.ts),
        input.last().map(|p| p.ts_end),
    ]
    .into_iter()
    .flatten()
    .max();
    let (Some(stream_start), Some(stream_end)) = (stream_start, stream_end) else {
        return Vec::new();
    };
    if stream_end < stream_start {
        return Vec::new();
    }

    let windows: Vec<WindowId> = focus.iter().map(WindowId::from_focus).collect();
    let workset = workset_pairs(&focus, &windows);
    let bounce_suppressed = bounce_suppressed_cuts(&focus, &windows);

    let mut raw: BTreeMap<Millis, RawCut> = BTreeMap::new();
    let mut suppressed_at: HashMap<Millis, u32> = HashMap::new();

    for i in 1..focus.len() {
        if windows[i] == windows[i - 1] {
            continue;
        }
        let ts = focus[i].ts;
        let mut kinds = Vec::new();
        if app_changed(&windows[i - 1], &windows[i]) {
            kinds.push(CutKind::AppChange);
        }
        if host_changed(&windows[i - 1], &windows[i]) {
            kinds.push(CutKind::HostChange);
        }
        if kinds.is_empty() {
            continue;
        }
        let bounce = bounce_suppressed.contains(&i);
        let glued = bounce
            || workset.contains(&canon_work(
                windows[i - 1].work_key(),
                windows[i].work_key(),
            ));
        if glued {
            *suppressed_at.entry(ts).or_default() += 1;
            continue;
        }
        raw.entry(ts).or_default().kinds.extend(kinds);
    }

    for ts in idle_resumes(&input) {
        raw.entry(ts).or_default().kinds.push(CutKind::IdleResume);
    }

    for ev in &system {
        let kind = match ev.kind {
            SystemKind::Lock => CutKind::Lock,
            SystemKind::Unlock => CutKind::Unlock,
            _ => continue,
        };
        raw.entry(ev.ts).or_default().kinds.push(kind);
    }

    for clip in clipboard
        .iter()
        .filter(|c| c.byte_len >= LARGE_CLIPBOARD_BYTES)
    {
        let Some(next) = next_other_app(&focus, clip) else {
            continue;
        };
        let i = next;
        let bounce = bounce_suppressed.contains(&i);
        let glued = bounce
            || workset.contains(&canon_work(
                windows[i.saturating_sub(1)].work_key(),
                windows[i].work_key(),
            ));
        if glued {
            *suppressed_at.entry(focus[i].ts).or_default() += 1;
            continue;
        }
        raw.entry(focus[i].ts)
            .or_default()
            .kinds
            .push(CutKind::ClipboardPaste);
    }

    for cut in raw.values_mut() {
        cut.kinds.sort();
        cut.kinds.dedup();
    }

    let natural: Vec<(Millis, Vec<CutKind>)> = raw
        .into_iter()
        .filter(|(ts, cut)| *ts > stream_start && *ts < stream_end && !cut.kinds.is_empty())
        .map(|(ts, cut)| (ts, cut.kinds))
        .collect();

    let cuts = insert_time_caps(stream_start, stream_end, natural);

    let mut boundaries: Vec<Boundary> = Vec::new();
    for (ts, kinds) in cuts {
        let suppressed = suppressed_at.get(&ts).copied().unwrap_or(0);
        let confidence = boundary_confidence(&kinds, suppressed);
        boundaries.push(Boundary {
            ts,
            kinds,
            confidence,
        });
    }

    let mut cores: Vec<(Millis, Millis, Option<Boundary>)> = Vec::new();
    let mut cursor = stream_start;
    let mut opener: Option<Boundary> = None;
    for b in boundaries {
        if b.ts > cursor {
            cores.push((cursor, b.ts, opener.take()));
            cursor = b.ts;
        }
        opener = Some(b);
    }
    if stream_end > cursor {
        cores.push((cursor, stream_end, opener));
    }

    cores
        .into_iter()
        .filter(|(start, end, _)| *end > *start)
        .map(|(core_start, core_end, opener)| {
            let started_at = core_start.saturating_sub(OVERLAP_MARGIN_MS);
            let ended_at = core_end.saturating_add(OVERLAP_MARGIN_MS);
            let (app, title, host) = representative(&focus, core_start, core_end);
            let event_ids = refs_in(&focus, &system, &clipboard, &input, started_at, ended_at);
            Segment {
                started_at,
                ended_at,
                core_started_at: core_start,
                core_ended_at: core_end,
                app,
                title,
                host,
                cut_kinds: opener.as_ref().map(|b| b.kinds.clone()).unwrap_or_default(),
                confidence: opener.and_then(|b| b.confidence),
                event_ids,
                last_edit: None,
            }
        })
        .collect()
}

#[derive(Default)]
struct RawCut {
    kinds: Vec<CutKind>,
}

struct Boundary {
    ts: Millis,
    kinds: Vec<CutKind>,
    confidence: Option<f32>,
}

/// 邊界信心：幾個切刀同時成立、有沒有黏合訊號在同一時刻被壓掉。
///
/// - 1 個切刀 → 0.50；每多一個切刀往 1 靠近一截（1 − 0.5^n），到不了 1.0。
/// - 只有強制 10 分鐘上限 → 0.35（它不是活動變了，是時間到了）。
/// - 同一時刻有黏合把別的切刀壓掉 → 再乘 0.85。
///
/// 算不出來的情況不走這裡，呼叫端給 `None`。
fn boundary_confidence(kinds: &[CutKind], suppressed: u32) -> Option<f32> {
    if kinds.is_empty() {
        return None;
    }
    let only_cap = kinds.len() == 1 && kinds[0] == CutKind::TimeCap;
    let n = kinds.len() as i32;
    let mut value = if only_cap {
        0.35
    } else {
        1.0 - 0.5_f32.powi(n)
    };
    if suppressed > 0 {
        value *= 0.85;
    }
    Some(value)
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct WindowId {
    app: String,
    title: String,
    host: String,
}

impl WindowId {
    fn from_focus(p: &FocusPoint) -> Self {
        Self {
            app: app_key(p),
            title: p.window_title.clone().unwrap_or_default(),
            host: p.url.as_deref().and_then(url_host).unwrap_or_default(),
        }
    }

    fn work_key(&self) -> WorkKey {
        WorkKey {
            app: self.app.clone(),
            host: self.host.clone(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
struct WorkKey {
    app: String,
    host: String,
}

fn app_key(p: &FocusPoint) -> String {
    p.app_id
        .as_deref()
        .or(p.app_name.as_deref())
        .unwrap_or("")
        .to_ascii_lowercase()
}

fn app_changed(a: &WindowId, b: &WindowId) -> bool {
    !a.app.is_empty() && !b.app.is_empty() && a.app != b.app
}

fn host_changed(a: &WindowId, b: &WindowId) -> bool {
    !a.host.is_empty() && !b.host.is_empty() && a.host != b.host
}

fn canon_work(a: WorkKey, b: WorkKey) -> (WorkKey, WorkKey) {
    if a <= b { (a, b) } else { (b, a) }
}

/// 短窗口內兩個工作鍵都反覆出現過，就算同一工作集。
fn workset_pairs(focus: &[FocusPoint], windows: &[WindowId]) -> HashSet<(WorkKey, WorkKey)> {
    let mut pairs = HashSet::new();
    if focus.len() < 2 {
        return pairs;
    }
    for p in focus {
        let from = p.ts.saturating_sub(WORKSET_WINDOW_MS);
        let to = p.ts.saturating_add(WORKSET_WINDOW_MS);
        let mut counts: HashMap<WorkKey, u32> = HashMap::new();
        for (j, q) in focus.iter().enumerate() {
            if q.ts < from || q.ts > to {
                continue;
            }
            let key = windows[j].work_key();
            if key.app.is_empty() {
                continue;
            }
            *counts.entry(key).or_default() += 1;
        }
        let members: Vec<WorkKey> = counts
            .into_iter()
            .filter(|(_, n)| *n >= WORKSET_MIN_APPEARANCES)
            .map(|(k, _)| k)
            .collect();
        for a in 0..members.len() {
            for b in (a + 1)..members.len() {
                if members[a].app == members[b].app && members[a].host == members[b].host {
                    continue;
                }
                pairs.insert(canon_work(members[a].clone(), members[b].clone()));
            }
        }
    }
    pairs
}

/// 切換後在 30s 內回到原視窗：出程與回程那幾刀都壓掉。
fn bounce_suppressed_cuts(focus: &[FocusPoint], windows: &[WindowId]) -> HashSet<usize> {
    let mut suppressed = HashSet::new();
    for i in 1..focus.len() {
        if windows[i] == windows[i - 1] {
            continue;
        }
        let origin = &windows[i - 1];
        let depart = focus[i].ts;
        for (k, later) in focus.iter().enumerate().skip(i + 1) {
            if later.ts - depart > BOUNCE_BACK_MS {
                break;
            }
            if windows[k] == *origin {
                for j in i..=k {
                    suppressed.insert(j);
                }
                break;
            }
        }
    }
    suppressed
}

fn idle_resumes(input: &[InputPoint]) -> Vec<Millis> {
    let mut out = Vec::new();
    let mut idle_run: i64 = 0;
    for m in input {
        let span = (m.ts_end - m.ts_start).max(0);
        if span == 0 {
            continue;
        }
        let fully_idle = m.idle_ms >= span.saturating_sub(100);
        if fully_idle {
            idle_run = idle_run.saturating_add(m.idle_ms.max(span));
            continue;
        }
        if idle_run > IDLE_RESUME_MS {
            out.push(m.ts_start);
        }
        idle_run = 0;
        if m.idle_ms > IDLE_RESUME_MS {
            // 這一格自己就閒超過 90s，格尾恢復。
            out.push(m.ts_end);
        }
    }
    out
}

fn next_other_app(focus: &[FocusPoint], clip: &ClipboardPoint) -> Option<usize> {
    let src = clip
        .source_app
        .as_deref()
        .map(|s| s.to_ascii_lowercase())
        .filter(|s| !s.is_empty());
    for (i, p) in focus.iter().enumerate() {
        if p.ts < clip.ts {
            continue;
        }
        if p.ts - clip.ts > WORKSET_WINDOW_MS {
            break;
        }
        let dst = app_key(p);
        if dst.is_empty() {
            continue;
        }
        if let Some(src) = &src {
            if dst != *src {
                return Some(i);
            }
        } else if i > 0 {
            let prev = app_key(&focus[i - 1]);
            if !prev.is_empty() && prev != dst {
                return Some(i);
            }
        }
    }
    None
}

fn insert_time_caps(
    stream_start: Millis,
    stream_end: Millis,
    natural: Vec<(Millis, Vec<CutKind>)>,
) -> Vec<(Millis, Vec<CutKind>)> {
    let mut out: Vec<(Millis, Vec<CutKind>)> = Vec::new();
    let mut cursor = stream_start;
    for (ts, kinds) in natural {
        while ts.saturating_sub(cursor) > TIME_CAP_MS {
            let cap_at = cursor.saturating_add(TIME_CAP_MS);
            if cap_at >= stream_end {
                break;
            }
            push_cut(&mut out, cap_at, vec![CutKind::TimeCap]);
            cursor = cap_at;
        }
        if ts > cursor && ts < stream_end {
            push_cut(&mut out, ts, kinds);
            cursor = ts;
        }
    }
    while stream_end.saturating_sub(cursor) > TIME_CAP_MS {
        let cap_at = cursor.saturating_add(TIME_CAP_MS);
        if cap_at >= stream_end {
            break;
        }
        push_cut(&mut out, cap_at, vec![CutKind::TimeCap]);
        cursor = cap_at;
    }
    out
}

fn push_cut(out: &mut Vec<(Millis, Vec<CutKind>)>, ts: Millis, kinds: Vec<CutKind>) {
    if let Some((last_ts, last_kinds)) = out.last_mut()
        && *last_ts == ts
    {
        last_kinds.extend(kinds);
        last_kinds.sort();
        last_kinds.dedup();
        return;
    }
    out.push((ts, kinds));
}

fn representative(
    focus: &[FocusPoint],
    core_start: Millis,
    core_end: Millis,
) -> (Option<String>, Option<String>, Option<String>) {
    if focus.is_empty() {
        return (None, None, None);
    }
    // 核心開始時已經在前景的那一個也算——否則切刀當下那一格會被漏掉。
    let mut dwell: HashMap<WindowId, Millis> = HashMap::new();
    let mut current: Option<(WindowId, Millis)> = None;
    for p in focus {
        if p.ts > core_end {
            break;
        }
        let id = WindowId::from_focus(p);
        if p.ts <= core_start {
            current = Some((id, core_start));
            continue;
        }
        if let Some((prev, since)) = current.take() {
            let end = p.ts.min(core_end);
            if end > since {
                *dwell.entry(prev).or_default() += end - since;
            }
        }
        if p.ts < core_end {
            current = Some((id, p.ts));
        }
    }
    if let Some((prev, since)) = current
        && core_end > since
    {
        *dwell.entry(prev).or_default() += core_end - since;
    }
    let best = dwell.into_iter().max_by_key(|(_, ms)| *ms).map(|(w, _)| w);
    match best {
        Some(w) if !w.app.is_empty() || !w.title.is_empty() || !w.host.is_empty() => (
            if w.app.is_empty() { None } else { Some(w.app) },
            if w.title.is_empty() {
                None
            } else {
                Some(w.title)
            },
            if w.host.is_empty() {
                None
            } else {
                Some(w.host)
            },
        ),
        _ => (None, None, None),
    }
}

fn refs_in(
    focus: &[FocusPoint],
    system: &[SystemPoint],
    clipboard: &[ClipboardPoint],
    input: &[InputPoint],
    from: Millis,
    to: Millis,
) -> EventRefs {
    EventRefs {
        focus: focus
            .iter()
            .filter(|p| p.ts >= from && p.ts < to)
            .map(|p| p.id)
            .collect(),
        system: system
            .iter()
            .filter(|p| p.ts >= from && p.ts < to)
            .map(|p| p.id)
            .collect(),
        clipboard: clipboard
            .iter()
            .filter(|p| p.ts >= from && p.ts < to)
            .map(|p| p.id)
            .collect(),
        input: input
            .iter()
            .filter(|p| p.ts_end > from && p.ts_start < to)
            .map(|p| p.id)
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn focus(id: i64, ts: Millis, app: &str, title: &str, url: Option<&str>) -> FocusPoint {
        FocusPoint {
            id,
            ts,
            app_id: Some(app.into()),
            app_name: None,
            window_title: Some(title.into()),
            url: url.map(|u| u.into()),
        }
    }

    fn sys(id: i64, ts: Millis, kind: SystemKind) -> SystemPoint {
        SystemPoint { id, ts, kind }
    }

    fn clip(id: i64, ts: Millis, bytes: i64, app: &str) -> ClipboardPoint {
        ClipboardPoint {
            id,
            ts,
            byte_len: bytes,
            source_app: Some(app.into()),
        }
    }

    fn idle_bucket(id: i64, start: Millis, end: Millis, idle: i64) -> InputPoint {
        InputPoint {
            id,
            ts_start: start,
            ts_end: end,
            idle_ms: idle,
        }
    }

    fn apps_of(segs: &[Segment]) -> Vec<Option<&str>> {
        segs.iter().map(|s| s.app.as_deref()).collect()
    }

    fn kinds_of(segs: &[Segment]) -> Vec<Vec<CutKind>> {
        segs.iter().map(|s| s.cut_kinds.clone()).collect()
    }

    #[test]
    fn empty_stream_is_no_segments() {
        assert!(segment(&EventStream::default()).is_empty());
    }

    #[test]
    fn app_change_cuts() {
        let stream = EventStream {
            focus: vec![
                focus(1, 0, "code.exe", "db.rs", None),
                focus(
                    2,
                    60_000,
                    "chrome.exe",
                    "mail",
                    Some("https://mail.example/"),
                ),
                focus(
                    3,
                    90_000,
                    "chrome.exe",
                    "mail",
                    Some("https://mail.example/"),
                ),
            ],
            ..EventStream::default()
        };
        let segs = segment(&stream);
        assert_eq!(segs.len(), 2, "{:?}", kinds_of(&segs));
        assert_eq!(apps_of(&segs), vec![Some("code.exe"), Some("chrome.exe")]);
        assert_eq!(segs[0].cut_kinds, Vec::new());
        assert_eq!(segs[1].cut_kinds, vec![CutKind::AppChange]);
        assert!(
            segs[0].confidence.is_none(),
            "第一段沒有打開它的切刀，不該有信心值"
        );
        assert_eq!(segs[1].confidence, Some(0.5), "單一前景 app 變更是一個切刀");
    }

    #[test]
    fn bounce_back_under_30s_glues() {
        let stream = EventStream {
            focus: vec![
                focus(1, 0, "code.exe", "db.rs", None),
                focus(2, 5_000, "chrome.exe", "docs", Some("https://docs.rs/foo")),
                focus(3, 20_000, "code.exe", "db.rs", None),
                focus(4, 80_000, "code.exe", "db.rs", None),
            ],
            ..EventStream::default()
        };
        let segs = segment(&stream);
        assert_eq!(segs.len(), 1, "折返該黏成一段，得到 {:?}", kinds_of(&segs));
        assert_eq!(segs[0].app.as_deref(), Some("code.exe"));
    }

    #[test]
    fn bounce_back_over_30s_still_cuts() {
        let stream = EventStream {
            focus: vec![
                focus(1, 0, "code.exe", "db.rs", None),
                focus(2, 5_000, "chrome.exe", "docs", Some("https://docs.rs/foo")),
                focus(3, 40_000, "code.exe", "db.rs", None),
            ],
            ..EventStream::default()
        };
        let segs = segment(&stream);
        assert!(
            segs.len() >= 2,
            "超過 30s 不該黏：{} 段 {:?}",
            segs.len(),
            kinds_of(&segs)
        );
    }

    #[test]
    fn host_change_cuts_same_app() {
        let stream = EventStream {
            focus: vec![
                focus(1, 0, "chrome.exe", "a", Some("https://github.com/x")),
                focus(2, 60_000, "chrome.exe", "b", Some("https://nhi.gov.tw/y")),
                focus(3, 90_000, "chrome.exe", "b", Some("https://nhi.gov.tw/y")),
            ],
            ..EventStream::default()
        };
        let segs = segment(&stream);
        assert_eq!(segs.len(), 2);
        assert_eq!(segs[1].cut_kinds, vec![CutKind::HostChange]);
        assert_eq!(segs[0].host.as_deref(), Some("github.com"));
        assert_eq!(segs[1].host.as_deref(), Some("nhi.gov.tw"));
    }

    #[test]
    fn idle_over_90s_then_resume_cuts() {
        let mut input = Vec::new();
        // 十格、每格 10s 全閒 = 100s。
        for i in 0..10 {
            let s = i * 10_000;
            input.push(idle_bucket(i + 1, s, s + 10_000, 10_000));
        }
        // 恢復。
        input.push(idle_bucket(11, 100_000, 110_000, 0));
        let stream = EventStream {
            focus: vec![
                focus(1, 0, "code.exe", "db.rs", None),
                focus(2, 110_000, "code.exe", "db.rs", None),
            ],
            input,
            ..EventStream::default()
        };
        let segs = segment(&stream);
        assert!(
            segs.iter()
                .any(|s| s.cut_kinds.contains(&CutKind::IdleResume)),
            "該有 idle 恢復的切刀，得到 {kinds_of:?}",
            kinds_of = kinds_of(&segs)
        );
    }

    #[test]
    fn idle_under_90s_does_not_cut() {
        let mut input = Vec::new();
        for i in 0..5 {
            let s = i * 10_000;
            input.push(idle_bucket(i + 1, s, s + 10_000, 10_000));
        }
        input.push(idle_bucket(6, 50_000, 60_000, 0));
        let stream = EventStream {
            focus: vec![
                focus(1, 0, "code.exe", "db.rs", None),
                focus(2, 60_000, "code.exe", "db.rs", None),
            ],
            input,
            ..EventStream::default()
        };
        let segs = segment(&stream);
        assert!(
            segs.iter()
                .all(|s| !s.cut_kinds.contains(&CutKind::IdleResume)),
            "50s 不該切：{kinds_of:?}",
            kinds_of = kinds_of(&segs)
        );
    }

    #[test]
    fn lock_and_unlock_cut() {
        let stream = EventStream {
            focus: vec![
                focus(1, 0, "code.exe", "db.rs", None),
                focus(2, 200_000, "code.exe", "db.rs", None),
            ],
            system: vec![
                sys(1, 60_000, SystemKind::Lock),
                sys(2, 120_000, SystemKind::Unlock),
            ],
            ..EventStream::default()
        };
        let segs = segment(&stream);
        let kinds = kinds_of(&segs);
        assert!(
            kinds.iter().any(|k| k.contains(&CutKind::Lock)),
            "{kinds:?}"
        );
        assert!(
            kinds.iter().any(|k| k.contains(&CutKind::Unlock)),
            "{kinds:?}"
        );
    }

    #[test]
    fn large_clipboard_then_other_app_cuts() {
        let stream = EventStream {
            focus: vec![
                focus(1, 0, "chrome.exe", "docs", Some("https://docs.rs/x")),
                focus(2, 8_000, "code.exe", "main.rs", None),
                focus(3, 20_000, "code.exe", "main.rs", None),
            ],
            clipboard: vec![clip(1, 5_000, 4_000, "chrome.exe")],
            ..EventStream::default()
        };
        let segs = segment(&stream);
        assert!(
            segs.iter()
                .any(|s| s.cut_kinds.contains(&CutKind::ClipboardPaste)),
            "{kinds_of:?}",
            kinds_of = kinds_of(&segs)
        );
        let paste = segs
            .iter()
            .find(|s| s.cut_kinds.contains(&CutKind::ClipboardPaste))
            .expect("paste cut");
        assert!(
            paste.cut_kinds.contains(&CutKind::AppChange),
            "同時成立的切刀都該記：{:?}",
            paste.cut_kinds
        );
        assert!(
            paste.confidence.expect("有切刀就該有信心") > 0.5,
            "兩個切刀的信心該比單一切刀高，得到 {:?}",
            paste.confidence
        );
    }

    #[test]
    fn small_clipboard_is_not_a_clipboard_cut() {
        let stream = EventStream {
            focus: vec![
                focus(1, 0, "chrome.exe", "docs", Some("https://docs.rs/x")),
                focus(2, 8_000, "code.exe", "main.rs", None),
            ],
            clipboard: vec![clip(1, 5_000, 12, "chrome.exe")],
            ..EventStream::default()
        };
        let segs = segment(&stream);
        assert!(
            segs.iter()
                .all(|s| !s.cut_kinds.contains(&CutKind::ClipboardPaste)),
            "{kinds_of:?}",
            kinds_of = kinds_of(&segs)
        );
    }

    #[test]
    fn time_cap_splits_a_long_stay() {
        let stream = EventStream {
            focus: vec![
                focus(1, 0, "code.exe", "db.rs", None),
                focus(2, 25 * 60_000, "code.exe", "db.rs", None),
            ],
            ..EventStream::default()
        };
        let segs = segment(&stream);
        assert!(
            segs.len() >= 3,
            "25 分鐘該被 10 分鐘上限切成至少 3 段，得到 {}",
            segs.len()
        );
        assert!(
            segs.iter().any(|s| s.cut_kinds == vec![CutKind::TimeCap]),
            "{kinds_of:?}",
            kinds_of = kinds_of(&segs)
        );
        let cap = segs
            .iter()
            .find(|s| s.cut_kinds == vec![CutKind::TimeCap])
            .unwrap();
        assert_eq!(cap.confidence, Some(0.35));
    }

    #[test]
    fn glue_does_not_suppress_time_cap() {
        // 同一工作集裡反覆切換，黏合壓得掉 app 變更，壓不掉 10 分鐘上限。
        let mut focus_pts = Vec::new();
        let mut ts = 0;
        let mut id = 1;
        while ts <= 12 * 60_000 {
            focus_pts.push(focus(id, ts, "code.exe", "db.rs", None));
            id += 1;
            ts += 20_000;
            focus_pts.push(focus(id, ts, "wt.exe", "pwsh", None));
            id += 1;
            ts += 20_000;
        }
        let stream = EventStream {
            focus: focus_pts,
            ..EventStream::default()
        };
        let segs = segment(&stream);
        assert!(
            segs.iter().any(|s| s.cut_kinds.contains(&CutKind::TimeCap)),
            "工作集黏合不該把 10 分鐘上限吃掉：{kinds_of:?}",
            kinds_of = kinds_of(&segs)
        );
        assert!(
            segs.iter()
                .all(|s| !s.cut_kinds.contains(&CutKind::AppChange)),
            "工作集內的 app 切換該被黏住：{kinds_of:?}",
            kinds_of = kinds_of(&segs)
        );
    }

    #[test]
    fn workset_is_not_the_same_as_same_app() {
        // 兩個不同站，各只出現一次：這是 host 變更，不是工作集。
        let stream = EventStream {
            focus: vec![
                focus(1, 0, "chrome.exe", "gh", Some("https://github.com/x")),
                focus(
                    2,
                    60_000,
                    "chrome.exe",
                    "mail",
                    Some("https://mail.example/inbox"),
                ),
                focus(
                    3,
                    90_000,
                    "chrome.exe",
                    "mail",
                    Some("https://mail.example/inbox"),
                ),
            ],
            ..EventStream::default()
        };
        let segs = segment(&stream);
        assert_eq!(segs.len(), 2, "單次 host 變更不該被「同 app」黏住");
        assert_eq!(segs[1].cut_kinds, vec![CutKind::HostChange]);
    }

    #[test]
    fn repeated_co_occurrence_glues_a_workset() {
        let stream = EventStream {
            focus: vec![
                focus(1, 0, "code.exe", "db.rs", None),
                focus(2, 10_000, "wt.exe", "pwsh", None),
                focus(3, 20_000, "code.exe", "db.rs", None),
                focus(4, 30_000, "wt.exe", "pwsh", None),
                focus(5, 120_000, "code.exe", "lib.rs", None),
            ],
            ..EventStream::default()
        };
        let segs = segment(&stream);
        assert_eq!(
            segs.len(),
            1,
            "短窗口內 code↔terminal 反覆共現該是同一段，得到 {} 段 {kinds_of:?}",
            segs.len(),
            kinds_of = kinds_of(&segs)
        );
    }

    #[test]
    fn overlap_margin_is_five_seconds() {
        let stream = EventStream {
            focus: vec![
                focus(1, 10_000, "code.exe", "db.rs", None),
                focus(
                    2,
                    70_000,
                    "chrome.exe",
                    "mail",
                    Some("https://mail.example/"),
                ),
                focus(
                    3,
                    80_000,
                    "chrome.exe",
                    "mail",
                    Some("https://mail.example/"),
                ),
            ],
            ..EventStream::default()
        };
        let segs = segment(&stream);
        assert_eq!(segs.len(), 2);
        assert_eq!(
            segs[0].core_ended_at, segs[1].core_started_at,
            "核心邊界該相接"
        );
        assert_eq!(segs[0].ended_at - segs[0].core_ended_at, OVERLAP_MARGIN_MS);
        assert_eq!(
            segs[1].core_started_at - segs[1].started_at,
            OVERLAP_MARGIN_MS
        );
        assert!(
            segs[0].ended_at > segs[1].started_at,
            "margin 之後兩段該重疊"
        );
    }

    #[test]
    fn confidence_is_computed_not_hardcoded() {
        let one = EventStream {
            focus: vec![
                focus(1, 0, "a.exe", "a", None),
                focus(2, 60_000, "b.exe", "b", None),
                focus(3, 90_000, "b.exe", "b", None),
            ],
            ..EventStream::default()
        };
        let two = EventStream {
            focus: vec![
                focus(1, 0, "a.exe", "a", None),
                focus(2, 60_000, "b.exe", "b", Some("https://b.example/")),
                focus(3, 90_000, "b.exe", "b", Some("https://b.example/")),
            ],
            clipboard: vec![clip(1, 50_000, 4_000, "a.exe")],
            ..EventStream::default()
        };
        let c1 = segment(&one)[1].confidence.expect("one");
        let c2 = segment(&two)[1].confidence.expect("two");
        assert_ne!(c1, 1.0);
        assert_ne!(c2, 1.0);
        assert!(c2 > c1, "兩個切刀 {c2} 該比一個 {c1} 高");
    }

    #[test]
    fn url_host_parses_common_shapes() {
        assert_eq!(
            url_host("https://github.com/ted-h/AI-Sister").as_deref(),
            Some("github.com")
        );
        assert_eq!(
            url_host("nhi.gov.tw/records").as_deref(),
            Some("nhi.gov.tw")
        );
        assert_eq!(url_host("SPEC.md — AI-Sister"), None);
        assert_eq!(url_host(""), None);
    }

    #[test]
    fn event_refs_cover_the_overlap_window() {
        let stream = EventStream {
            focus: vec![
                focus(1, 0, "code.exe", "db.rs", None),
                focus(
                    2,
                    60_000,
                    "chrome.exe",
                    "mail",
                    Some("https://mail.example/"),
                ),
            ],
            system: vec![sys(9, 59_000, SystemKind::Lock)],
            ..EventStream::default()
        };
        let segs = segment(&stream);
        assert!(
            segs[1].event_ids.focus.contains(&2),
            "打開第二段的 focus 該被算進去 {:?}",
            segs[1].event_ids
        );
        // lock 在 59s，第二段核心從 60s 起、margin 往前 5s，所以 59s 落在第二段裡。
        assert!(
            segs[1].event_ids.system.contains(&9),
            "{:?}",
            segs[1].event_ids
        );
    }
}
