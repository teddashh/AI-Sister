//! recorder 工作幀像素的 changed-region OCR gate。
//!
//! 第一次讀全畫面；之後先找 RGB 真的變過的區域，引擎看到的 crop 不再二次
//! 縮小。未變區沿用上一張**已成功寫進 DB**的文字，通過結構證據的局部結果
//! 再拼成這一幀的完整文字。dHash 已判成新畫面時，證據不足就退回全幅；dHash
//! 近似重複時只准成功 crop 推翻它，失敗維持重複，避免游標閃爍放大成全幅 OCR。
//! 結構檢查能擋住缺行、拆併與閱讀順序錯接，不能證明 raw OCR 對 crop 裡的每個
//! 字都辨識正確（全幅 OCR 也沒有這種證明）。Windows 工作幀本身另有 4096px
//! 安全上限；那是進 gate 前的事。

use anyhow::{Context, Result, anyhow, bail};
use sister_core::model::OcrBlock;
use std::num::NonZeroU64;
use std::time::{Duration, Instant};

use crate::traits::{DhashRecheck, Ocr, OcrAttempt, OcrOutcome, OcrWork, RawFrame, RecordingOcr};

const TILE: u32 = 64;
const REGION_PAD: u32 = 64;
const EDIT_CROSS_PAD: u32 = 4;
const CHANGE_COVER_PAD: u32 = 2;
const EDGE_GUARD: i32 = 8;
const MAX_REGIONS: usize = 4;
const FULL_AREA_NUMERATOR: u64 = 1;
const FULL_AREA_DENOMINATOR: u64 = 3;

/// Region 路徑失敗有兩種完全不同的意思：OCR/crop 本身沒有執行完，或是
/// 執行完後 stitch 的結構證據不足。近似重複幀只把後者列成「局部未採用」；
/// 前者必須進 OCR failure 計數，不能用正常拒絕把引擎故障藏起來。
enum RegionFailure {
    Execution(anyhow::Error),
    Rejected(anyhow::Error),
}

impl RegionFailure {
    fn into_error(self) -> anyhow::Error {
        match self {
            Self::Execution(error) | Self::Rejected(error) => error,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FrameKey {
    ts: i64,
    monitor: i32,
    width: u32,
    height: u32,
    dhash: u64,
}

impl From<&RawFrame> for FrameKey {
    fn from(frame: &RawFrame) -> Self {
        Self {
            ts: frame.ts,
            monitor: frame.monitor,
            width: frame.width,
            height: frame.height,
            dhash: frame.dhash,
        }
    }
}

#[derive(Debug, Clone)]
struct Fingerprint {
    monitor: i32,
    width: u32,
    height: u32,
    columns: u32,
    rows: u32,
    tiles: Vec<u64>,
}

impl Fingerprint {
    fn from_rgb(frame: &RawFrame, rgb: &[u8]) -> Self {
        let columns = frame.width.div_ceil(TILE);
        let rows = frame.height.div_ceil(TILE);
        let mut tiles = Vec::with_capacity((columns * rows) as usize);
        let stride = frame.width as usize * 3;
        for ty in 0..rows {
            let top = ty * TILE;
            let bottom = (top + TILE).min(frame.height);
            for tx in 0..columns {
                let left = tx * TILE;
                let right = (left + TILE).min(frame.width);
                // FNV-1a，吃每一個 RGB byte。任何小字像素變化都要打開精確
                // gate；這一層寧可多做局部判斷，不可用容忍門檻安靜漏掉候選。
                let mut hash = 0xcbf29ce484222325u64;
                for y in top..bottom {
                    let row = &rgb[y as usize * stride..(y as usize + 1) * stride];
                    for x in left..right {
                        let at = x as usize * 3;
                        for byte in &row[at..at + 3] {
                            hash ^= u64::from(*byte);
                            hash = hash.wrapping_mul(0x100000001b3);
                        }
                    }
                }
                tiles.push(hash);
            }
        }
        Self {
            monitor: frame.monitor,
            width: frame.width,
            height: frame.height,
            columns,
            rows,
            tiles,
        }
    }

    fn same_shape(&self, other: &Self) -> bool {
        self.monitor == other.monitor
            && self.width == other.width
            && self.height == other.height
            && self.columns == other.columns
            && self.rows == other.rows
    }
}

struct AnalyzedFrame {
    fingerprint: Fingerprint,
    rgb: Box<[u8]>,
}

impl AnalyzedFrame {
    fn from_frame(frame: &RawFrame) -> Result<Self> {
        let rgba = frame
            .rgba
            .as_deref()
            .ok_or_else(|| anyhow!("changed-region gate 收到沒有像素的幀"))?;
        let pixels = u64::from(frame.width)
            .checked_mul(u64::from(frame.height))
            .ok_or_else(|| anyhow!("畫面尺寸溢位：{}x{}", frame.width, frame.height))?;
        let needed = pixels
            .checked_mul(4)
            .and_then(|n| usize::try_from(n).ok())
            .ok_or_else(|| anyhow!("畫面尺寸溢位：{}x{}", frame.width, frame.height))?;
        if needed == 0 || rgba.len() < needed {
            bail!(
                "changed-region gate 的影像緩衝區和尺寸對不起來：{}x{} 需要 {needed} bytes，只有 {}",
                frame.width,
                frame.height,
                rgba.len()
            );
        }
        let rgb_len = pixels
            .checked_mul(3)
            .and_then(|n| usize::try_from(n).ok())
            .ok_or_else(|| anyhow!("RGB baseline 尺寸溢位：{}x{}", frame.width, frame.height))?;
        let mut rgb = Vec::with_capacity(rgb_len);
        for pixel in rgba[..needed].as_chunks::<4>().0 {
            rgb.extend_from_slice(&pixel[..3]);
        }
        let rgb = rgb.into_boxed_slice();
        let fingerprint = Fingerprint::from_rgb(frame, &rgb);
        Ok(Self { fingerprint, rgb })
    }
}

#[derive(Debug)]
struct Snapshot {
    fingerprint: Fingerprint,
    rgb: Box<[u8]>,
    blocks: Vec<OcrBlock>,
}

#[derive(Debug)]
struct Pending {
    key: FrameKey,
    snapshot: Snapshot,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Span {
    y: u32,
    x0: u32,
    x1: u32,
}

#[derive(Debug)]
struct OwnedChange {
    old: usize,
    spans: Vec<Span>,
    bbox: Region,
    side: AppendSide,
}

struct ChangeDraft {
    old: usize,
    spans: Vec<Span>,
    bbox: Region,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AppendSide {
    Left,
    Right,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct Region {
    left: u32,
    top: u32,
    right: u32,
    bottom: u32,
}

impl Region {
    fn from_tile(tx: u32, ty: u32, width: u32, height: u32) -> Self {
        Self {
            left: tx * TILE,
            top: ty * TILE,
            right: ((tx + 1) * TILE).min(width),
            bottom: ((ty + 1) * TILE).min(height),
        }
    }

    fn width(self) -> u32 {
        self.right - self.left
    }

    fn height(self) -> u32 {
        self.bottom - self.top
    }

    fn area(self) -> u64 {
        u64::from(self.width()) * u64::from(self.height())
    }

    fn expand(self, by: u32, width: u32, height: u32) -> Self {
        self.expand_xy(by, by, width, height)
    }

    fn expand_xy(self, x: u32, y: u32, width: u32, height: u32) -> Self {
        Self {
            left: self.left.saturating_sub(x),
            top: self.top.saturating_sub(y),
            right: self.right.saturating_add(x).min(width),
            bottom: self.bottom.saturating_add(y).min(height),
        }
    }

    fn union(self, other: Self) -> Self {
        Self {
            left: self.left.min(other.left),
            top: self.top.min(other.top),
            right: self.right.max(other.right),
            bottom: self.bottom.max(other.bottom),
        }
    }

    fn intersects(self, other: Self) -> bool {
        self.left < other.right
            && other.left < self.right
            && self.top < other.bottom
            && other.top < self.bottom
    }

    fn touches(self, other: Self) -> bool {
        self.left <= other.right
            && other.left <= self.right
            && self.top <= other.bottom
            && other.top <= self.bottom
    }

    fn contains_point(self, x: u32, y: u32) -> bool {
        self.left <= x && x < self.right && self.top <= y && y < self.bottom
    }

    fn intersects_span(self, span: Span) -> bool {
        self.top <= span.y && span.y < self.bottom && self.left < span.x1 && span.x0 < self.right
    }

    fn contains_span(self, span: Span) -> bool {
        self.top <= span.y && span.y < self.bottom && self.left <= span.x0 && span.x1 <= self.right
    }

    fn label(self) -> String {
        format!(
            "x={} y={} {}x{}",
            self.left,
            self.top,
            self.width(),
            self.height()
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FullReason {
    NoBaseline,
    ShapeChanged,
    UnsafeBaseline,
    TooMuchChange,
    UnownedPixel,
    AmbiguousOwner,
    UnsupportedEdit,
    CropRejected,
}

impl FullReason {
    fn is_fallback(self) -> bool {
        self != Self::NoBaseline
    }
}

enum Plan {
    Full(FullReason),
    Reuse,
    Regions {
        crops: Vec<Region>,
        changes: Vec<OwnedChange>,
    },
}

/// Windows recording backend 會包這一層；bench / doctor 直接使用裡面的 raw OCR。
pub struct ChangedRegionOcr<O> {
    inner: O,
    committed: Option<Snapshot>,
    pending: Option<Pending>,
}

impl<O> ChangedRegionOcr<O> {
    pub fn new(inner: O) -> Self {
        Self {
            inner,
            committed: None,
            pending: None,
        }
    }
}

#[derive(Default)]
struct WorkMeter {
    calls: u64,
    elapsed: Duration,
    input_pixels: u64,
}

impl WorkMeter {
    fn run<O: Ocr>(&mut self, inner: &mut O, frame: &RawFrame) -> Result<Vec<OcrBlock>> {
        let started = Instant::now();
        let result = inner.recognize(frame);
        self.elapsed += started.elapsed();
        self.calls += 1;
        self.input_pixels += u64::from(frame.width) * u64::from(frame.height);
        result
    }

    fn finish(self) -> Option<OcrWork> {
        NonZeroU64::new(self.calls)
            .map(|calls| OcrWork::new(calls, self.elapsed, self.input_pixels))
    }
}

impl<O: Ocr> ChangedRegionOcr<O> {
    fn plan(&self, current: &AnalyzedFrame) -> Plan {
        let Some(previous) = self.committed.as_ref() else {
            return Plan::Full(FullReason::NoBaseline);
        };
        if !previous.fingerprint.same_shape(&current.fingerprint) {
            return Plan::Full(FullReason::ShapeChanged);
        }

        let changed: Vec<bool> = previous
            .fingerprint
            .tiles
            .iter()
            .zip(&current.fingerprint.tiles)
            .enumerate()
            .map(|(index, (a, b))| a != b || tile_rgb_changed(previous, current, index as u32))
            .collect();
        if !changed.iter().any(|v| *v) {
            return Plan::Reuse;
        }

        let changed_tile_area: u64 = changed
            .iter()
            .enumerate()
            .filter(|(_, is_changed)| **is_changed)
            .map(|(index, _)| {
                Region::from_tile(
                    index as u32 % current.fingerprint.columns,
                    index as u32 / current.fingerprint.columns,
                    current.fingerprint.width,
                    current.fingerprint.height,
                )
                .area()
            })
            .sum();
        let full_area =
            u64::from(current.fingerprint.width) * u64::from(current.fingerprint.height);
        if changed_tile_area.saturating_mul(FULL_AREA_DENOMINATOR)
            >= full_area.saturating_mul(FULL_AREA_NUMERATOR)
        {
            return Plan::Full(FullReason::TooMuchChange);
        }

        let changes = match assign_changed_pixels(previous, current, &changed) {
            Ok(changes) => changes,
            Err(reason) => return Plan::Full(reason),
        };
        if changes.is_empty() {
            // changed tile 卻找不到不同 RGB pixel 代表內部不變量壞了。不要把
            // 「沒證據」當成可以沿用；全幅重建會往看得見的方向失敗。
            return Plan::Full(FullReason::UnsafeBaseline);
        }
        let mut crops = changes
            .iter()
            .map(|change| {
                let old = block_region(
                    &previous.blocks[change.old],
                    current.fingerprint.width,
                    current.fingerprint.height,
                )
                .expect("assign_changed_pixels 已驗過所有舊 block");
                old.union(change.bbox).expand(
                    REGION_PAD,
                    current.fingerprint.width,
                    current.fingerprint.height,
                )
            })
            .collect::<Vec<_>>();
        // Context padding 只做這一次。它可以讓兩個 crop 合併，但不准再拿 padding
        // 去遞迴招募下一行，否則 8px 行距會一路串成整頁。
        merge_regions(&mut crops);
        let total_area: u64 = crops.iter().map(|r| r.area()).sum();
        if crops.len() > MAX_REGIONS
            || total_area.saturating_mul(FULL_AREA_DENOMINATOR)
                >= full_area.saturating_mul(FULL_AREA_NUMERATOR)
        {
            Plan::Full(FullReason::TooMuchChange)
        } else {
            Plan::Regions { crops, changes }
        }
    }

    fn full(
        &mut self,
        frame: &RawFrame,
        reason: FullReason,
        meter: &mut WorkMeter,
    ) -> Result<OcrOutcome> {
        let blocks = meter.run(&mut self.inner, frame)?;
        Ok(OcrOutcome::Full {
            blocks,
            fallback: reason.is_fallback(),
        })
    }

    fn regions(
        &mut self,
        frame: &RawFrame,
        crops: Vec<Region>,
        changes: &[OwnedChange],
        meter: &mut WorkMeter,
    ) -> std::result::Result<OcrOutcome, RegionFailure> {
        let previous = self
            .committed
            .as_ref()
            .expect("Regions plan 一定有 committed snapshot");
        let mut reads = Vec::with_capacity(crops.len());
        for (index, region) in crops.iter().copied().enumerate() {
            let crop = crop_frame(frame, region)
                .with_context(|| {
                    format!(
                        "建立 OCR crop {}/{}（{}）",
                        index + 1,
                        crops.len(),
                        region.label()
                    )
                })
                .map_err(RegionFailure::Execution)?;
            let blocks = meter
                .run(&mut self.inner, &crop)
                .with_context(|| {
                    format!(
                        "局部 OCR {}/{} 失敗（{}）",
                        index + 1,
                        crops.len(),
                        region.label()
                    )
                })
                .map_err(RegionFailure::Execution)?;
            reads.push((region, blocks));
        }
        let blocks = stitch(previous, changes, reads, frame.width, frame.height)
            .map_err(RegionFailure::Rejected)?;
        let count = NonZeroU64::new(crops.len() as u64).expect("Regions 不會是空的");
        Ok(OcrOutcome::Regions {
            blocks,
            regions: count,
        })
    }

    fn fallback_full(
        &mut self,
        frame: &RawFrame,
        why: anyhow::Error,
        meter: &mut WorkMeter,
    ) -> Result<OcrOutcome> {
        self.full(frame, FullReason::CropRejected, meter)
            .map_err(|full| {
                full.context(format!(
                    "changed-region OCR 沒有把握，退回全幅仍失敗；局部原因：{why:#}"
                ))
            })
    }

    fn finish_attempt(
        &mut self,
        frame: &RawFrame,
        outcome: Result<OcrOutcome>,
        analyzed: Option<AnalyzedFrame>,
        total_started: Instant,
        meter: WorkMeter,
    ) -> OcrAttempt {
        if let (Ok(success), Some(analyzed)) = (&outcome, analyzed) {
            self.pending = Some(Pending {
                key: FrameKey::from(frame),
                snapshot: Snapshot {
                    fingerprint: analyzed.fingerprint,
                    rgb: analyzed.rgb,
                    blocks: success.blocks().to_vec(),
                },
            });
        }
        if outcome.is_err() {
            // 中間有一張沒有可靠文字；下一次真的走到 OCR 時強制全幅，不能把
            // 更舊的文字跨洞拼過來。相同畫面是否再進 OCR 仍由 recorder 的
            // dHash gate 決定，這裡不把一次引擎錯誤變成每 400ms 無限重試。
            self.committed = None;
            self.pending = None;
        }
        let engine_elapsed = meter.elapsed;
        let gate_elapsed = total_started.elapsed().saturating_sub(engine_elapsed);
        OcrAttempt::measured(outcome, gate_elapsed, meter.finish())
    }

    fn finish_dhash_duplicate(
        total_started: Instant,
        meter: WorkMeter,
        rejected_regions: Option<NonZeroU64>,
        error: Option<anyhow::Error>,
    ) -> DhashRecheck {
        let engine_elapsed = meter.elapsed;
        let gate_elapsed = total_started.elapsed().saturating_sub(engine_elapsed);
        DhashRecheck::Duplicate {
            gate_elapsed: Some(gate_elapsed),
            work: meter.finish(),
            rejected_regions,
            error,
        }
    }
}

impl<O: Ocr> RecordingOcr for ChangedRegionOcr<O> {
    fn recognize_frame(&mut self, frame: &RawFrame) -> OcrAttempt {
        self.pending = None;
        let total_started = Instant::now();
        let mut meter = WorkMeter::default();
        let analyzed = AnalyzedFrame::from_frame(frame);
        let (outcome, analyzed) = match analyzed {
            Err(error) => Err(error),
            Ok(analyzed) => {
                let outcome = match self.plan(&analyzed) {
                    Plan::Full(reason) => self.full(frame, reason, &mut meter),
                    Plan::Reuse => {
                        let blocks = self
                            .committed
                            .as_ref()
                            .expect("Reuse plan 一定有 committed snapshot")
                            .blocks
                            .clone();
                        Ok(OcrOutcome::Reused { blocks })
                    }
                    Plan::Regions { crops, changes } => {
                        match self.regions(frame, crops, &changes, &mut meter) {
                            Ok(outcome) => Ok(outcome),
                            Err(error) => {
                                self.pending = None;
                                self.fallback_full(frame, error.into_error(), &mut meter)
                            }
                        }
                    }
                };
                Ok((outcome, analyzed))
            }
        }
        .map_or_else(
            |error| (Err(error), None),
            |(outcome, analyzed)| (outcome, Some(analyzed)),
        );
        self.finish_attempt(frame, outcome, analyzed, total_started, meter)
    }

    fn recheck_dhash_duplicate(&mut self, frame: &RawFrame) -> DhashRecheck {
        self.pending = None;
        let total_started = Instant::now();
        let mut meter = WorkMeter::default();
        let analyzed = match AnalyzedFrame::from_frame(frame) {
            Ok(analyzed) => analyzed,
            Err(error) => {
                tracing::debug!(
                    error = %error,
                    "dHash 近似重複幀無法做 changed-region recheck，維持重複"
                );
                return Self::finish_dhash_duplicate(total_started, meter, None, Some(error));
            }
        };
        let Plan::Regions { crops, changes } = self.plan(&analyzed) else {
            return Self::finish_dhash_duplicate(total_started, meter, None, None);
        };
        let rejected_regions =
            NonZeroU64::new(crops.len() as u64).expect("Regions plan 不會是空的");
        match self.regions(frame, crops, &changes, &mut meter) {
            Ok(outcome) => DhashRecheck::Changed(self.finish_attempt(
                frame,
                Ok(outcome),
                Some(analyzed),
                total_started,
                meter,
            )),
            Err(RegionFailure::Rejected(error)) => {
                // dHash 本來就會吞掉游標閃爍等小變化。這裡只准用一個通過
                // stitch 的 crop 推翻它；局部沒把握時若退全幅，游標每閃一次
                // 就會重新跑最貴的那條路，CPU 最佳化會反過來變成放大器。
                tracing::debug!(
                    error = %error,
                    "dHash 近似重複幀的局部 OCR 證據不足，維持重複"
                );
                Self::finish_dhash_duplicate(total_started, meter, Some(rejected_regions), None)
            }
            Err(RegionFailure::Execution(error)) => {
                tracing::warn!(
                    error = %error,
                    "dHash 近似重複幀的局部 OCR 執行失敗，維持重複"
                );
                Self::finish_dhash_duplicate(total_started, meter, None, Some(error))
            }
        }
    }

    fn commit_frame(&mut self, frame: &RawFrame) {
        if self
            .pending
            .as_ref()
            .is_some_and(|pending| pending.key == FrameKey::from(frame))
        {
            self.committed = self.pending.take().map(|pending| pending.snapshot);
        }
    }

    fn discard_frame(&mut self, frame: &RawFrame) {
        if self
            .pending
            .as_ref()
            .is_some_and(|pending| pending.key == FrameKey::from(frame))
        {
            self.pending = None;
        }
    }

    fn reset(&mut self) {
        self.committed = None;
        self.pending = None;
    }
}

fn block_region(block: &OcrBlock, width: u32, height: u32) -> Result<Region> {
    if block.w <= 0 || block.h <= 0 {
        bail!("OCR block 幾何不是正數：{:?}", block);
    }
    let right = block
        .x
        .checked_add(block.w)
        .ok_or_else(|| anyhow!("OCR block x+w 溢位：{:?}", block))?;
    let bottom = block
        .y
        .checked_add(block.h)
        .ok_or_else(|| anyhow!("OCR block y+h 溢位：{:?}", block))?;
    let left = block.x.max(0) as u32;
    let top = block.y.max(0) as u32;
    let right = right.max(0) as u32;
    let bottom = bottom.max(0) as u32;
    if left >= right || top >= bottom || left >= width || top >= height {
        bail!("OCR block 在畫面外：{:?}，畫面 {width}x{height}", block);
    }
    Ok(Region {
        left,
        top,
        right: right.min(width),
        bottom: bottom.min(height),
    })
}

fn tile_rgb_changed(previous: &Snapshot, current: &AnalyzedFrame, tile_index: u32) -> bool {
    let width = current.fingerprint.width;
    let tile = Region::from_tile(
        tile_index % current.fingerprint.columns,
        tile_index / current.fingerprint.columns,
        width,
        current.fingerprint.height,
    );
    let stride = width as usize * 3;
    let row_bytes = tile.width() as usize * 3;
    (tile.top..tile.bottom).any(|y| {
        let start = y as usize * stride + tile.left as usize * 3;
        previous.rgb[start..start + row_bytes] != current.rgb[start..start + row_bytes]
    })
}

fn edit_envelope(rect: Region, width: u32, height: u32) -> Region {
    if rect.width() >= rect.height() {
        let along = rect.height().clamp(8, 32);
        rect.expand_xy(along, EDIT_CROSS_PAD, width, height)
    } else {
        let along = rect.width().clamp(8, 32);
        rect.expand_xy(EDIT_CROSS_PAD, along, width, height)
    }
}

fn assign_changed_pixels(
    previous: &Snapshot,
    current: &AnalyzedFrame,
    changed_tiles: &[bool],
) -> std::result::Result<Vec<OwnedChange>, FullReason> {
    let width = current.fingerprint.width;
    let height = current.fingerprint.height;
    if previous.rgb.len() != current.rgb.len() {
        return Err(FullReason::ShapeChanged);
    }
    let old_rects = previous
        .blocks
        .iter()
        .map(|block| block_region(block, width, height))
        .collect::<Result<Vec<_>>>()
        .map_err(|_| FullReason::UnsafeBaseline)?;
    let envelopes = old_rects
        .iter()
        .map(|rect| edit_envelope(*rect, width, height))
        .collect::<Vec<_>>();
    let mut owned = (0..old_rects.len())
        .map(|_| None::<ChangeDraft>)
        .collect::<Vec<_>>();
    let stride = width as usize * 3;

    for (tile_index, is_changed) in changed_tiles.iter().copied().enumerate() {
        if !is_changed {
            continue;
        }
        let tile = Region::from_tile(
            tile_index as u32 % current.fingerprint.columns,
            tile_index as u32 / current.fingerprint.columns,
            width,
            height,
        );
        let candidates = envelopes
            .iter()
            .enumerate()
            .filter_map(|(old, envelope)| tile.intersects(*envelope).then_some(old))
            .collect::<Vec<_>>();

        for y in tile.top..tile.bottom {
            let mut run_owner = None;
            let mut run_start = tile.left;
            for x in tile.left..tile.right {
                let at = y as usize * stride + x as usize * 3;
                let changed = previous.rgb[at..at + 3] != current.rgb[at..at + 3];
                let owner = if changed {
                    let mut found = None;
                    for old in candidates.iter().copied() {
                        if envelopes[old].contains_point(x, y) && found.replace(old).is_some() {
                            return Err(FullReason::AmbiguousOwner);
                        }
                    }
                    Some(found.ok_or(FullReason::UnownedPixel)?)
                } else {
                    None
                };

                if owner != run_owner {
                    if let Some(old) = run_owner {
                        append_span(
                            &mut owned,
                            old,
                            Span {
                                y,
                                x0: run_start,
                                x1: x,
                            },
                        );
                    }
                    if owner.is_some() {
                        run_start = x;
                    }
                    run_owner = owner;
                }
            }
            if let Some(old) = run_owner {
                append_span(
                    &mut owned,
                    old,
                    Span {
                        y,
                        x0: run_start,
                        x1: tile.right,
                    },
                );
            }
        }
    }

    owned
        .into_iter()
        .flatten()
        .map(|draft| {
            let side =
                append_side(old_rects[draft.old], &draft).ok_or(FullReason::UnsupportedEdit)?;
            Ok(OwnedChange {
                old: draft.old,
                spans: draft.spans,
                bbox: draft.bbox,
                side,
            })
        })
        .collect()
}

fn append_side(old: Region, change: &ChangeDraft) -> Option<AppendSide> {
    // OcrBlock 是 Windows OCR 組好的「一行」。只有橫排行首／行尾追加有足夠
    // 的語意不變量：fresh 文字可以用 prefix/suffix 證明舊內容仍在。行內編輯
    // 或直排若只看一個外接矩形，內部空白新增文字仍可能被 crop 漏掉。
    if old.width() < old.height() || horizontal_band_count(change) != 1 {
        return None;
    }
    let right = change.spans.iter().any(|span| span.x1 > old.right)
        && change
            .spans
            .iter()
            .all(|span| span.x0 >= old.right.saturating_sub(CHANGE_COVER_PAD));
    let left = change.spans.iter().any(|span| span.x0 < old.left)
        && change
            .spans
            .iter()
            .all(|span| span.x1 <= old.left.saturating_add(CHANGE_COVER_PAD));
    match (left, right) {
        (true, false) => Some(AppendSide::Left),
        (false, true) => Some(AppendSide::Right),
        _ => None,
    }
}

fn horizontal_band_count(change: &ChangeDraft) -> usize {
    let mut occupied = vec![false; change.bbox.width() as usize];
    for span in &change.spans {
        for x in span.x0..span.x1 {
            occupied[(x - change.bbox.left) as usize] = true;
        }
    }
    occupied
        .iter()
        .fold((0usize, false), |(bands, inside), occupied| {
            if *occupied && !inside {
                (bands + 1, true)
            } else {
                (bands, *occupied)
            }
        })
        .0
}

fn append_span(owned: &mut [Option<ChangeDraft>], old: usize, span: Span) {
    let span_rect = Region {
        left: span.x0,
        top: span.y,
        right: span.x1,
        bottom: span.y + 1,
    };
    match &mut owned[old] {
        Some(change) => {
            change.bbox = change.bbox.union(span_rect);
            change.spans.push(span);
        }
        slot @ None => {
            *slot = Some(ChangeDraft {
                old,
                spans: vec![span],
                bbox: span_rect,
            });
        }
    }
}

fn merge_regions(regions: &mut Vec<Region>) {
    loop {
        let mut merged = false;
        'outer: for i in 0..regions.len() {
            for j in i + 1..regions.len() {
                if regions[i].touches(regions[j]) {
                    regions[i] = regions[i].union(regions[j]);
                    regions.remove(j);
                    merged = true;
                    break 'outer;
                }
            }
        }
        if !merged {
            regions.sort();
            return;
        }
    }
}

fn crop_frame(frame: &RawFrame, region: Region) -> Result<RawFrame> {
    if region.left >= region.right
        || region.top >= region.bottom
        || region.right > frame.width
        || region.bottom > frame.height
    {
        bail!(
            "OCR crop 越界：{}，畫面 {}x{}",
            region.label(),
            frame.width,
            frame.height
        );
    }
    let rgba = frame
        .rgba
        .as_deref()
        .ok_or_else(|| anyhow!("OCR crop 的來源幀沒有像素"))?;
    let stride = frame.width as usize * 4;
    let row_bytes = region.width() as usize * 4;
    let mut out = Vec::with_capacity(row_bytes * region.height() as usize);
    for y in region.top..region.bottom {
        let start = y as usize * stride + region.left as usize * 4;
        let end = start + row_bytes;
        let row = rgba
            .get(start..end)
            .ok_or_else(|| anyhow!("OCR crop {} 對不到來源 buffer", region.label()))?;
        out.extend_from_slice(row);
    }
    Ok(RawFrame::from_rgba(
        frame.ts,
        frame.monitor,
        region.width(),
        region.height(),
        out,
    ))
}

fn stitch(
    previous: &Snapshot,
    changes: &[OwnedChange],
    reads: Vec<(Region, Vec<OcrBlock>)>,
    width: u32,
    height: u32,
) -> Result<Vec<OcrBlock>> {
    struct Fresh {
        block: OcrBlock,
        rect: Region,
        local_rect: Region,
        crop: Region,
    }

    let old_rects = previous
        .blocks
        .iter()
        .map(|block| block_region(block, width, height))
        .collect::<Result<Vec<_>>>()?;
    let mut fresh = Vec::new();

    for (region, local) in reads {
        for mut block in local {
            let local_rect = block_region(&block, region.width(), region.height())?;
            let dx = i32::try_from(region.left).context("OCR crop x 超過 i32")?;
            let dy = i32::try_from(region.top).context("OCR crop y 超過 i32")?;
            block.x = block.x.checked_add(dx).context("OCR block x 平移溢位")?;
            block.y = block.y.checked_add(dy).context("OCR block y 平移溢位")?;
            let rect = block_region(&block, width, height)?;
            fresh.push(Fresh {
                block,
                rect,
                local_rect,
                crop: region,
            });
        }
    }

    let mut candidates = (0..changes.len())
        .map(|_| Vec::<usize>::new())
        .collect::<Vec<_>>();
    for (fresh_index, block) in fresh.iter().enumerate() {
        let evidence = block.rect.expand(CHANGE_COVER_PAD, width, height);
        let owners = changes
            .iter()
            .enumerate()
            .filter_map(|(change_index, change)| {
                change
                    .spans
                    .iter()
                    .any(|span| evidence.intersects_span(*span))
                    .then_some(change_index)
            })
            .collect::<Vec<_>>();
        match owners.as_slice() {
            [] => {
                // 只是 context 裡被重讀到的未變行：沿用 full baseline，不能因
                // crop 的局部版面判讀不同就擅自改它。
            }
            [change_index] => {
                let old = changes[*change_index].old;
                if !block.rect.intersects(old_rects[old]) {
                    bail!("局部 OCR block 吃到變動像素，卻沒有和原本那一行相交");
                }
                candidates[*change_index].push(fresh_index);
            }
            _ => bail!("一個局部 OCR block 同時吃到多個舊行的變動像素"),
        }
    }

    let mut replacements = vec![None::<OcrBlock>; previous.blocks.len()];
    for (change_index, change) in changes.iter().enumerate() {
        let [fresh_index] = candidates[change_index].as_slice() else {
            bail!("受影響的舊 OCR 行沒有恰好一個局部結果（一對多、缺行或空讀）");
        };
        let fresh = &fresh[*fresh_index];
        let evidence = fresh.rect.expand(CHANGE_COVER_PAD, width, height);
        if change
            .spans
            .iter()
            .any(|span| !evidence.contains_span(*span))
        {
            // 典型例子：在行尾新增金額，但 crop 只重讀出舊 label。bbox 沒長到
            // 新像素上就不能把這次當成功，否則新增的字會安靜消失。
            bail!("局部 OCR block 沒有覆蓋這一行的全部變動像素");
        }
        let old = &previous.blocks[change.old];
        let old_rect = old_rects[change.old];
        let semantic_append = match change.side {
            AppendSide::Right => {
                !old.text.is_empty()
                    && fresh
                        .block
                        .text
                        .strip_prefix(&old.text)
                        .is_some_and(|suffix| suffix.chars().count() == 1)
                    && fresh.rect.left.abs_diff(old_rect.left) <= CHANGE_COVER_PAD
                    && fresh.rect.right > old_rect.right
            }
            AppendSide::Left => {
                !old.text.is_empty()
                    && fresh
                        .block
                        .text
                        .strip_suffix(&old.text)
                        .is_some_and(|prefix| prefix.chars().count() == 1)
                    && fresh.rect.right.abs_diff(old_rect.right) <= CHANGE_COVER_PAD
                    && fresh.rect.left < old_rect.left
            }
        };
        if !semantic_append {
            // 一個 line bbox 只包「第一個字到最後一個字」；中間大片空白新增 X，
            // crop 可以漏 X 卻仍用同一個 bbox 回別的文字。只有單一像素帶的
            // 行首／行尾追加，且 fresh 完整保留舊 prefix/suffix、只多一個字元，
            // 才有幾何以外的內容證據。
            bail!("局部 OCR 沒有證明舊行完整保留、只在行首或行尾多一個字元");
        }
        let touches_left = fresh.crop.left > 0 && fresh.local_rect.left <= EDGE_GUARD as u32;
        let touches_top = fresh.crop.top > 0 && fresh.local_rect.top <= EDGE_GUARD as u32;
        let touches_right = fresh.crop.right < width
            && fresh.crop.width().saturating_sub(fresh.local_rect.right) <= EDGE_GUARD as u32;
        let touches_bottom = fresh.crop.bottom < height
            && fresh.crop.height().saturating_sub(fresh.local_rect.bottom) <= EDGE_GUARD as u32;
        if touches_left || touches_top || touches_right || touches_bottom {
            bail!(
                "受影響的局部 OCR block 貼到 crop 邊緣，可能被截斷：{:?}",
                fresh.block
            );
        }
        if replacements[change.old].is_some() {
            bail!("同一個舊 OCR 行被兩個變動集合重複改寫");
        }
        replacements[change.old] = Some(fresh.block.clone());
    }

    let mut blocks = Vec::with_capacity(previous.blocks.len());
    for (index, old) in previous.blocks.iter().enumerate() {
        if let Some(fresh) = replacements[index].take() {
            // 每個 fresh 寫回自己的 full-OCR OldId；完全不採信 crop Vec 的順序。
            // 所以多欄或直排 crop 即使回成 [B', A']，整張仍是 [A', B']。
            blocks.push(fresh);
        } else {
            blocks.push(old.clone());
        }
    }
    Ok(blocks)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::collections::VecDeque;
    use std::rc::Rc;

    fn frame(ts: i64, w: u32, h: u32, changes: &[(u32, u32, [u8; 3])]) -> RawFrame {
        let mut rgba = vec![255u8; (w * h * 4) as usize];
        for (x, y, rgb) in changes {
            let at = ((*y * w + *x) * 4) as usize;
            rgba[at..at + 3].copy_from_slice(rgb);
        }
        RawFrame::from_rgba(ts, 0, w, h, rgba)
    }

    fn changed_rect(left: u32, top: u32, right: u32, bottom: u32) -> Vec<(u32, u32, [u8; 3])> {
        (top..bottom)
            .flat_map(|y| (left..right).map(move |x| (x, y, [0, 0, 0])))
            .collect()
    }

    fn block(text: &str, x: i32, y: i32, w: i32, h: i32) -> OcrBlock {
        OcrBlock {
            text: text.into(),
            x,
            y,
            w,
            h,
            confidence: -1.0,
        }
    }

    #[derive(Default)]
    struct Spy {
        seen: Rc<RefCell<Vec<(u32, u32)>>>,
        replies: VecDeque<Result<Vec<OcrBlock>>>,
    }

    impl Ocr for Spy {
        fn recognize(&mut self, frame: &RawFrame) -> Result<Vec<OcrBlock>> {
            self.seen.borrow_mut().push((frame.width, frame.height));
            self.replies.pop_front().unwrap_or_else(|| Ok(Vec::new()))
        }
    }

    fn outcome(attempt: OcrAttempt) -> OcrOutcome {
        attempt.outcome.expect("OCR outcome")
    }

    #[test]
    fn first_frame_is_full_and_commit_is_required_before_reuse() {
        let seen = Rc::new(RefCell::new(Vec::new()));
        let mut gate = ChangedRegionOcr::new(Spy {
            seen: seen.clone(),
            replies: VecDeque::from([Ok(vec![block("甲", 10, 10, 20, 20)]), Ok(vec![])]),
        });
        let a = frame(1, 256, 256, &[]);
        let first = gate.recognize_frame(&a);
        let first_work = first.work.expect("首張真的呼叫 OCR 實作");
        assert_eq!(first_work.calls().get(), 1);
        assert_eq!(first_work.input_pixels(), 256 * 256);
        assert!(first.gate_elapsed.is_some(), "gate 時間和 raw OCR 要分開");
        assert!(matches!(
            outcome(first),
            OcrOutcome::Full {
                fallback: false,
                ..
            }
        ));
        // 沒 commit：同一畫面仍然必須真的讀，不能認領上一個沒落 DB 的 pending。
        assert!(matches!(
            outcome(gate.recognize_frame(&a)),
            OcrOutcome::Full { .. }
        ));
        assert_eq!(*seen.borrow(), vec![(256, 256), (256, 256)]);

        gate.commit_frame(&a);
        let reused = gate.recognize_frame(&a);
        assert!(
            reused.work.is_none(),
            "Reuse 沒有呼叫 OCR 實作，不能記一筆 0ms"
        );
        assert!(matches!(outcome(reused), OcrOutcome::Reused { .. }));
        assert_eq!(seen.borrow().len(), 2, "Reuse 不准呼叫引擎");
    }

    #[test]
    fn one_changed_tile_keeps_working_pixels_and_rebases_coordinates() {
        let seen = Rc::new(RefCell::new(Vec::new()));
        let mut gate = ChangedRegionOcr::new(Spy {
            seen: seen.clone(),
            replies: VecDeque::from([
                Ok(vec![block("舊", 140, 140, 20, 20)]),
                Ok(vec![block("舊新", 64, 64, 21, 20)]),
            ]),
        });
        let a = frame(1, 384, 384, &[]);
        let _ = outcome(gate.recognize_frame(&a));
        gate.commit_frame(&a);
        let b = frame(2, 384, 384, &[(160, 150, [0, 0, 0])]);
        let attempt = gate.recognize_frame(&b);
        let work = attempt.work.expect("局部路徑真的呼叫 OCR 實作");
        assert_eq!(work.calls().get(), 1);
        assert_eq!(work.input_pixels(), 149 * 148);
        let result = outcome(attempt);
        let OcrOutcome::Regions { blocks, regions } = result else {
            panic!("應走局部 OCR");
        };
        assert_eq!(regions.get(), 1);
        assert_eq!(
            seen.borrow()[1],
            (149, 148),
            "crop 保留工作幀像素，沒有二次縮圖"
        );
        assert!(
            blocks
                .iter()
                .any(|b| b.text == "舊新" && b.x == 140 && b.y == 140)
        );
    }

    #[test]
    fn dense_lines_do_not_chain_through_context_padding() {
        let seen = Rc::new(RefCell::new(Vec::new()));
        let lines = (0..30)
            .map(|i| block(&format!("第 {i} 行"), 100, 100 + i * 28, 1600, 20))
            .collect::<Vec<_>>();
        let mut changed_lines = lines.clone();
        changed_lines[15].text.push('新');
        changed_lines[15].w += 1;
        let mut gate = ChangedRegionOcr::new(Spy {
            seen: seen.clone(),
            replies: VecDeque::from([Ok(lines), Ok(vec![block("第 15 行新", 64, 64, 1601, 20)])]),
        });
        let a = frame(1, 1920, 1080, &[]);
        let _ = outcome(gate.recognize_frame(&a));
        gate.commit_frame(&a);
        let b = frame(2, 1920, 1080, &[(1700, 530, [0, 0, 0])]);
        let result = outcome(gate.recognize_frame(&b));
        let OcrOutcome::Regions { blocks, .. } = result else {
            panic!("普通 8px 行距不該被 padding 遞迴串成整頁 full OCR");
        };
        assert_eq!(blocks, changed_lines);
        assert_eq!(seen.borrow()[1], (1729, 148));
        assert!(u64::from(1729u32 * 148) < u64::from(1920u32 * 1080) / 3);
    }

    #[test]
    fn a_recognized_line_end_append_is_regional() {
        let changes = changed_rect(200, 105, 210, 115);
        let mut gate = ChangedRegionOcr::new(Spy {
            seen: Rc::default(),
            replies: VecDeque::from([
                Ok(vec![block("ABC", 100, 100, 100, 20)]),
                Ok(vec![block("ABCD", 64, 64, 110, 20)]),
            ]),
        });
        let a = frame(1, 512, 512, &[]);
        let _ = outcome(gate.recognize_frame(&a));
        gate.commit_frame(&a);
        let b = frame(2, 512, 512, &changes);
        let OcrOutcome::Regions { blocks, .. } = outcome(gate.recognize_frame(&b)) else {
            panic!("行尾新字有進 fresh bbox，應可局部拼回");
        };
        assert_eq!(blocks, vec![block("ABCD", 100, 100, 110, 20)]);
    }

    #[test]
    fn a_dhash_near_duplicate_can_be_promoted_by_a_proven_append() {
        let changes = changed_rect(200, 105, 210, 115);
        let mut gate = ChangedRegionOcr::new(Spy {
            seen: Rc::default(),
            replies: VecDeque::from([
                Ok(vec![block("ABC", 100, 100, 100, 20)]),
                Ok(vec![block("ABCD", 64, 64, 110, 20)]),
            ]),
        });
        let a = frame(1, 512, 512, &[]);
        let _ = outcome(gate.recognize_frame(&a));
        gate.commit_frame(&a);
        let b = frame(2, 512, 512, &changes);
        let DhashRecheck::Changed(attempt) = gate.recheck_dhash_duplicate(&b) else {
            panic!("dHash 吞掉的小字追加應由精確 RGB + crop 證據升格");
        };
        assert_eq!(attempt.work.expect("一個 crop").calls().get(), 1);
        assert!(matches!(outcome(attempt), OcrOutcome::Regions { .. }));
    }

    #[test]
    fn a_rejected_near_duplicate_crop_does_not_expand_into_full_ocr() {
        let seen = Rc::new(RefCell::new(Vec::new()));
        let changes = changed_rect(200, 105, 210, 115);
        let mut gate = ChangedRegionOcr::new(Spy {
            seen: seen.clone(),
            replies: VecDeque::from([
                Ok(vec![block("ABC", 100, 100, 100, 20)]),
                // 這也是閃爍游標的形狀：像素在行尾變了，文字沒有新增。
                Ok(vec![block("ABC", 64, 64, 100, 20)]),
            ]),
        });
        let a = frame(1, 512, 512, &[]);
        let _ = outcome(gate.recognize_frame(&a));
        gate.commit_frame(&a);
        let b = frame(2, 512, 512, &changes);
        let DhashRecheck::Duplicate {
            work,
            rejected_regions,
            ..
        } = gate.recheck_dhash_duplicate(&b)
        else {
            panic!("沒有新字的近似重複幀應維持重複");
        };
        assert_eq!(work.expect("真的試過 crop").calls().get(), 1);
        assert_eq!(rejected_regions.expect("不能拿 0 冒充未量到").get(), 1);
        assert_eq!(seen.borrow().len(), 2, "不能再補跑一次全幅 OCR");
    }

    #[test]
    fn a_failed_near_duplicate_crop_is_an_ocr_error_not_a_structural_rejection() {
        let changes = changed_rect(200, 105, 210, 115);
        let mut gate = ChangedRegionOcr::new(Spy {
            seen: Rc::default(),
            replies: VecDeque::from([
                Ok(vec![block("ABC", 100, 100, 100, 20)]),
                Err(anyhow!("engine down")),
            ]),
        });
        let a = frame(1, 512, 512, &[]);
        let _ = outcome(gate.recognize_frame(&a));
        gate.commit_frame(&a);
        let b = frame(2, 512, 512, &changes);
        let DhashRecheck::Duplicate {
            work,
            rejected_regions,
            error,
            ..
        } = gate.recheck_dhash_duplicate(&b)
        else {
            panic!("OCR 執行失敗不能把近似重複幀升格");
        };
        assert_eq!(work.expect("失敗的 raw OCR 呼叫也要入帳").calls().get(), 1);
        assert!(
            rejected_regions.is_none(),
            "引擎沒跑完，不能把計畫中的 crop 數冒充已驗完的結構拒絕"
        );
        assert!(
            error
                .expect("OCR 執行錯誤不能保持沉默")
                .to_string()
                .contains("局部 OCR 1/1 失敗")
        );
    }

    #[test]
    fn a_recognized_line_start_append_is_regional() {
        let changes = changed_rect(90, 105, 100, 115);
        let mut gate = ChangedRegionOcr::new(Spy {
            seen: Rc::default(),
            replies: VecDeque::from([
                Ok(vec![block("ABC", 100, 100, 100, 20)]),
                Ok(vec![block("DABC", 64, 64, 110, 20)]),
            ]),
        });
        let a = frame(1, 512, 512, &[]);
        let _ = outcome(gate.recognize_frame(&a));
        gate.commit_frame(&a);
        let b = frame(2, 512, 512, &changes);
        let OcrOutcome::Regions { blocks, .. } = outcome(gate.recognize_frame(&b)) else {
            panic!("行首新字有進 fresh bbox，應可局部拼回");
        };
        assert_eq!(blocks, vec![block("DABC", 90, 100, 110, 20)]);
    }

    #[test]
    fn a_missed_line_end_append_falls_back_to_full() {
        let seen = Rc::new(RefCell::new(Vec::new()));
        let changes = changed_rect(200, 105, 210, 115);
        let mut gate = ChangedRegionOcr::new(Spy {
            seen: seen.clone(),
            replies: VecDeque::from([
                Ok(vec![block("ABC", 100, 100, 100, 20)]),
                // crop 只回原本那一行；bbox 沒長到新增像素，不能宣稱成功。
                Ok(vec![block("ABC", 64, 64, 100, 20)]),
                Ok(vec![block("ABCD", 100, 100, 110, 20)]),
            ]),
        });
        let a = frame(1, 512, 512, &[]);
        let _ = outcome(gate.recognize_frame(&a));
        gate.commit_frame(&a);
        let b = frame(2, 512, 512, &changes);
        let attempt = gate.recognize_frame(&b);
        assert_eq!(attempt.work.expect("crop + full").calls().get(), 2);
        let OcrOutcome::Full { blocks, fallback } = outcome(attempt) else {
            panic!("crop 漏掉 append 必須全幅確認");
        };
        assert!(fallback);
        assert_eq!(blocks, vec![block("ABCD", 100, 100, 110, 20)]);
        assert_eq!(seen.borrow().len(), 3);
    }

    #[test]
    fn a_crop_that_only_sees_the_new_suffix_cannot_replace_the_old_line() {
        let changes = changed_rect(200, 105, 210, 115);
        let mut gate = ChangedRegionOcr::new(Spy {
            seen: Rc::default(),
            replies: VecDeque::from([
                Ok(vec![block("ABC", 100, 100, 100, 20)]),
                // 全域 195..210：碰到舊框、也蓋住新增像素，但沒有保留 ABC。
                Ok(vec![block("D", 159, 64, 15, 20)]),
                Ok(vec![block("ABCD", 100, 100, 110, 20)]),
            ]),
        });
        let a = frame(1, 512, 512, &[]);
        let _ = outcome(gate.recognize_frame(&a));
        gate.commit_frame(&a);
        let b = frame(2, 512, 512, &changes);
        let attempt = gate.recognize_frame(&b);
        assert_eq!(attempt.work.expect("crop + full").calls().get(), 2);
        assert!(matches!(
            outcome(attempt),
            OcrOutcome::Full { fallback: true, .. }
        ));
    }

    #[test]
    fn an_internal_edit_never_uses_a_line_bounding_box_as_content_proof() {
        let seen = Rc::new(RefCell::new(Vec::new()));
        let mut gate = ChangedRegionOcr::new(Spy {
            seen: seen.clone(),
            replies: VecDeque::from([
                Ok(vec![block("A C", 100, 100, 200, 20)]),
                Ok(vec![block("B X C", 100, 100, 200, 20)]),
            ]),
        });
        let a = frame(1, 512, 512, &[]);
        let _ = outcome(gate.recognize_frame(&a));
        gate.commit_frame(&a);
        let b = frame(2, 512, 512, &[(110, 110, [0, 0, 0]), (200, 110, [0, 0, 0])]);
        let attempt = gate.recognize_frame(&b);
        assert_eq!(attempt.work.expect("direct full").calls().get(), 1);
        assert!(matches!(
            outcome(attempt),
            OcrOutcome::Full { fallback: true, .. }
        ));
        assert_eq!(*seen.borrow(), vec![(512, 512), (512, 512)]);
    }

    #[test]
    fn equal_tile_hashes_are_still_confirmed_against_exact_rgb() {
        let changes = changed_rect(200, 105, 210, 115);
        let mut gate = ChangedRegionOcr::new(Spy {
            seen: Rc::default(),
            replies: VecDeque::from([
                Ok(vec![block("ABC", 100, 100, 100, 20)]),
                Ok(vec![block("ABCD", 64, 64, 110, 20)]),
            ]),
        });
        let a = frame(1, 512, 512, &[]);
        let _ = outcome(gate.recognize_frame(&a));
        gate.commit_frame(&a);
        let b = frame(2, 512, 512, &changes);
        let current = AnalyzedFrame::from_frame(&b).expect("analyze current");
        let tile = (105 / TILE) * current.fingerprint.columns + 200 / TILE;
        gate.committed.as_mut().expect("baseline").fingerprint.tiles[tile as usize] =
            current.fingerprint.tiles[tile as usize];

        let attempt = gate.recognize_frame(&b);
        assert!(attempt.work.is_some(), "hash 相等也要用 exact RGB 抓到變動");
        assert!(matches!(outcome(attempt), OcrOutcome::Regions { .. }));
    }

    #[test]
    fn independent_new_text_goes_directly_to_full() {
        let seen = Rc::new(RefCell::new(Vec::new()));
        let mut gate = ChangedRegionOcr::new(Spy {
            seen: seen.clone(),
            replies: VecDeque::from([
                Ok(vec![block("舊", 100, 100, 40, 20)]),
                Ok(vec![
                    block("舊", 100, 100, 40, 20),
                    block("新增", 300, 300, 60, 20),
                ]),
            ]),
        });
        let a = frame(1, 512, 512, &[]);
        let _ = outcome(gate.recognize_frame(&a));
        gate.commit_frame(&a);
        let b = frame(2, 512, 512, &[(330, 310, [0, 0, 0])]);
        let attempt = gate.recognize_frame(&b);
        let work = attempt.work.expect("直接全幅");
        assert_eq!(work.calls().get(), 1, "無 owner 不要先浪費一次 crop");
        assert_eq!(work.input_pixels(), 512 * 512);
        assert!(matches!(
            outcome(attempt),
            OcrOutcome::Full { fallback: true, .. }
        ));
        assert_eq!(*seen.borrow(), vec![(512, 512), (512, 512)]);
    }

    #[test]
    fn unchanged_old_blocks_survive_and_empty_crop_is_confirmed_by_full_ocr() {
        let seen = Rc::new(RefCell::new(Vec::new()));
        let mut gate = ChangedRegionOcr::new(Spy {
            seen: seen.clone(),
            replies: VecDeque::from([
                Ok(vec![
                    block("左上", 10, 10, 30, 20),
                    block("右下", 300, 300, 30, 20),
                ]),
                Ok(vec![]),
                Ok(vec![block("左上", 10, 10, 30, 20)]),
            ]),
        });
        let a = frame(1, 512, 512, &[]);
        let _ = outcome(gate.recognize_frame(&a));
        gate.commit_frame(&a);
        let b = frame(2, 512, 512, &[(330, 310, [0, 0, 0])]);
        let OcrOutcome::Full { blocks, fallback } = outcome(gate.recognize_frame(&b)) else {
            panic!("空 crop 無法證明字真的刪掉，應退回全幅");
        };
        assert!(fallback);
        assert_eq!(blocks, vec![block("左上", 10, 10, 30, 20)]);
        assert_eq!(seen.borrow().len(), 3, "空 crop 後當場全幅確認");
    }

    #[test]
    fn reversed_crop_results_return_to_their_nonconsecutive_full_reading_order() {
        let mut gate = ChangedRegionOcr::new(Spy {
            seen: Rc::default(),
            replies: VecDeque::from([
                // 中間那行在遠處，A/B 在 full 閱讀順序中刻意不連續。
                Ok(vec![
                    block("A", 100, 100, 100, 20),
                    block("中間", 400, 20, 40, 20),
                    block("B", 100, 150, 100, 20),
                ]),
                // crop 刻意反著回 B+、A+；幾何各自唯一，不能照 Vec 順序拼。
                Ok(vec![
                    block("B+", 64, 114, 101, 20),
                    block("A+", 64, 64, 101, 20),
                ]),
            ]),
        });
        let a = frame(1, 512, 512, &[]);
        let _ = outcome(gate.recognize_frame(&a));
        gate.commit_frame(&a);
        let b = frame(2, 512, 512, &[(200, 110, [0, 0, 0]), (200, 160, [0, 0, 0])]);
        let OcrOutcome::Regions { blocks, .. } = outcome(gate.recognize_frame(&b)) else {
            panic!("兩行都有唯一像素證據，應走局部 OCR");
        };
        assert_eq!(
            blocks
                .iter()
                .map(|block| block.text.as_str())
                .collect::<Vec<_>>(),
            vec!["A+", "中間", "B+"],
            "crop Vec 順序不能打散 full OCR 知道的多欄閱讀順序"
        );
    }

    #[test]
    fn discarding_a_partial_result_keeps_the_last_committed_baseline() {
        let seen = Rc::new(RefCell::new(Vec::new()));
        let mut gate = ChangedRegionOcr::new(Spy {
            seen: seen.clone(),
            replies: VecDeque::from([
                Ok(vec![block("舊", 300, 300, 30, 20)]),
                Ok(vec![block("舊新", 64, 64, 31, 20)]),
                Ok(vec![block("舊新", 64, 64, 31, 20)]),
            ]),
        });
        let a = frame(1, 512, 512, &[]);
        let _ = outcome(gate.recognize_frame(&a));
        gate.commit_frame(&a);
        let b = frame(2, 512, 512, &[(330, 310, [0, 0, 0])]);

        assert!(matches!(
            outcome(gate.recognize_frame(&b)),
            OcrOutcome::Regions { .. }
        ));
        gate.discard_frame(&b);
        assert!(matches!(
            outcome(gate.recognize_frame(&b)),
            OcrOutcome::Regions { .. }
        ));
        assert_eq!(seen.borrow().len(), 3, "discard 後同一幀必須重新讀 crop");
    }

    #[test]
    fn dimension_change_and_large_change_fall_back_to_full() {
        let mut gate = ChangedRegionOcr::new(Spy::default());
        let a = frame(1, 384, 384, &[]);
        let _ = outcome(gate.recognize_frame(&a));
        gate.commit_frame(&a);
        let resized = frame(2, 512, 384, &[]);
        assert!(matches!(
            outcome(gate.recognize_frame(&resized)),
            OcrOutcome::Full { .. }
        ));

        gate.commit_frame(&resized);
        let changes = (0..512)
            .step_by(8)
            .flat_map(|x| (0..384).step_by(8).map(move |y| (x, y, [0, 0, 0])))
            .collect::<Vec<_>>();
        let busy = frame(3, 512, 384, &changes);
        assert!(matches!(
            outcome(gate.recognize_frame(&busy)),
            OcrOutcome::Full { .. }
        ));
    }

    #[test]
    fn crop_failure_retries_full_and_reset_forces_full_again() {
        let seen = Rc::new(RefCell::new(Vec::new()));
        let mut gate = ChangedRegionOcr::new(Spy {
            seen: seen.clone(),
            replies: VecDeque::from([
                Ok(vec![block("甲", 300, 300, 20, 20)]),
                Err(anyhow!("crop broke")),
                Ok(vec![block("全幅", 10, 10, 20, 20)]),
                Ok(vec![]),
            ]),
        });
        let a = frame(1, 384, 384, &[]);
        let _ = outcome(gate.recognize_frame(&a));
        gate.commit_frame(&a);
        let b = frame(2, 384, 384, &[(320, 310, [0, 0, 0])]);
        let retried = gate.recognize_frame(&b);
        let work = retried.work.expect("crop 失敗後全幅重試也要入帳");
        assert_eq!(work.calls().get(), 2);
        assert!(
            work.input_pixels() > u64::from(b.width) * u64::from(b.height),
            "失敗的 crop 和重試的全幅都要算，不能只留下好看的那筆"
        );
        assert!(matches!(
            outcome(retried),
            OcrOutcome::Full { fallback: true, .. }
        ));
        assert_eq!(seen.borrow().len(), 3, "crop 失敗後當場退回全幅");
        gate.commit_frame(&b);
        gate.reset();
        assert!(matches!(
            outcome(gate.recognize_frame(&b)),
            OcrOutcome::Full { .. }
        ));
    }

    #[test]
    fn crop_and_full_both_failing_clear_the_baseline_before_the_next_attempt() {
        let seen = Rc::new(RefCell::new(Vec::new()));
        let mut gate = ChangedRegionOcr::new(Spy {
            seen: seen.clone(),
            replies: VecDeque::from([
                Ok(vec![block("甲", 300, 300, 20, 20)]),
                Err(anyhow!("crop broke")),
                Err(anyhow!("full broke")),
                Ok(vec![block("恢復", 10, 10, 20, 20)]),
            ]),
        });
        let a = frame(1, 512, 512, &[]);
        let _ = outcome(gate.recognize_frame(&a));
        gate.commit_frame(&a);
        let b = frame(2, 512, 512, &[(320, 310, [0, 0, 0])]);

        let failed = gate.recognize_frame(&b);
        let work = failed.work.expect("crop 與 full 兩次嘗試都要入帳");
        assert_eq!(work.calls().get(), 2);
        assert!(work.input_pixels() > u64::from(b.width) * u64::from(b.height));
        assert!(failed.outcome.is_err());

        let recovered = gate.recognize_frame(&b);
        assert!(matches!(
            outcome(recovered),
            OcrOutcome::Full {
                fallback: false,
                ..
            }
        ));
        assert_eq!(seen.borrow().last(), Some(&(512, 512)));
    }

    #[test]
    fn native_crop_copies_rows_without_mixing_stride() {
        let mut rgba = Vec::new();
        for y in 0..3u8 {
            for x in 0..4u8 {
                rgba.extend_from_slice(&[x, y, x.wrapping_add(y), 255]);
            }
        }
        let full = RawFrame::from_rgba(1, 0, 4, 3, rgba);
        let crop = crop_frame(
            &full,
            Region {
                left: 1,
                top: 1,
                right: 3,
                bottom: 3,
            },
        )
        .unwrap();
        assert_eq!(crop.width, 2);
        assert_eq!(crop.height, 2);
        assert_eq!(
            crop.rgba.unwrap(),
            vec![1, 1, 2, 255, 2, 1, 3, 255, 1, 2, 3, 255, 2, 2, 4, 255]
        );
    }
}
