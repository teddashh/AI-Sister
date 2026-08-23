//! 錄製收尾的磁碟歸因。
//!
//! SQLite 的邏輯配置、main/WAL/SHM 實體檔案，以及資料目錄旁檔是三種不同
//! 口徑。這裡把它們並排，不把可以各自為真的數字加成一句假結論。

use sister_core::db::{DbDiskDelta, DbDiskSnapshot, FileDelta, SqliteFileKind};
use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsStr;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SidecarSnapshot {
    files: BTreeMap<PathBuf, u64>,
    /// 第一層裡刻意沒往下掃的項目。`frames/` 與 SQLite 三個檔案不在這裡，
    /// 因為它們已經有自己的量測口徑。
    unscanned: Vec<PathBuf>,
}

/// 量資料目錄第一層的一般檔案；不遞迴，也不把 SQLite／frames 再算一次。
pub(crate) fn sidecar_snapshot(data_dir: &Path) -> Result<SidecarSnapshot, String> {
    let entries = std::fs::read_dir(data_dir)
        .map_err(|error| format!("讀取資料目錄 {} 失敗：{error}", data_dir.display()))?;
    let mut files = BTreeMap::new();
    let mut unscanned = Vec::new();

    for entry in entries {
        let entry = entry
            .map_err(|error| format!("讀取資料目錄 {} 的項目失敗：{error}", data_dir.display()))?;
        let name = entry.file_name();
        if is_sqlite_file(&name) || name == OsStr::new("frames") {
            continue;
        }
        let relative = PathBuf::from(&name);
        let kind = entry
            .file_type()
            .map_err(|error| format!("判斷 {} 的檔案類型失敗：{error}", entry.path().display()))?;
        if kind.is_file() {
            let bytes = entry
                .metadata()
                .map_err(|error| format!("讀取 {} 的大小失敗：{error}", entry.path().display()))?
                .len();
            files.insert(relative, bytes);
        } else {
            // 不跟 symlink 走，也不遞迴未知子目錄；兩者都明講沒有量。
            unscanned.push(relative);
        }
    }
    unscanned.sort();
    Ok(SidecarSnapshot { files, unscanned })
}

fn is_sqlite_file(name: &OsStr) -> bool {
    ["sister.db", "sister.db-wal", "sister.db-shm"]
        .into_iter()
        .any(|candidate| name == OsStr::new(candidate))
}

#[derive(Debug)]
pub(crate) struct AttributionInput {
    pub(crate) db_before: Result<DbDiskSnapshot, String>,
    pub(crate) db_after: Result<DbDiskSnapshot, String>,
    pub(crate) sidecars_before: Result<SidecarSnapshot, String>,
    pub(crate) sidecars_after: Result<SidecarSnapshot, String>,
    pub(crate) session_image_bytes: u64,
}

impl AttributionInput {
    fn db_pair(&self) -> Result<(&DbDiskSnapshot, &DbDiskSnapshot), String> {
        match (&self.db_before, &self.db_after) {
            (Ok(before), Ok(after)) => Ok((before, after)),
            (Err(before), Err(after)) => Err(format!(
                "開場 SQLite 快照失敗：{before}；收尾 SQLite 快照也失敗：{after}"
            )),
            (Err(error), _) => Err(format!("開場 SQLite 快照失敗：{error}")),
            (_, Err(error)) => Err(format!("收尾 SQLite 快照失敗：{error}")),
        }
    }

    fn db_delta(&self) -> Result<DbDiskDelta, String> {
        let (before, after) = self.db_pair()?;
        DbDiskDelta::between(before, after)
            .map_err(|error| format!("SQLite 快照無法相減：{error:#}"))
    }

    /// 現行足跡摘要的總量口徑：SQLite 邏輯配置 + WAL 檔長 + 目錄中登記的畫面。
    ///
    /// 這是為了讓新歸因能和既有「這段實際長了」逐位元組對帳，不代表 WAL
    /// 檔長適合外推每天；詳細輸出會把那個限制就地講清楚。
    pub(crate) fn total_delta_bytes(&self) -> Result<i64, String> {
        let delta = self.db_delta()?;
        let wal = sqlite_file_delta(&delta, SqliteFileKind::Wal)?;
        checked_sum(
            [
                delta.logical_allocated_bytes,
                wal.delta_bytes,
                delta.catalogued_image_bytes,
            ],
            "現行磁碟總量",
        )
    }
}

/// 排成可以直接接在 `record` 收尾摘要後面的行。
pub(crate) fn render(input: &AttributionInput) -> Vec<String> {
    let mut lines = vec!["  磁碟歸因（這段實測）：".to_string()];
    match input.db_delta() {
        Err(error) => lines.push(format!("        SQLite：量不到（{error}）")),
        Ok(delta) => render_sqlite(input, &delta, &mut lines),
    }
    render_sidecars(input, &mut lines);
    lines
}

fn render_sqlite(input: &AttributionInput, delta: &DbDiskDelta, lines: &mut Vec<String>) {
    let wal = match sqlite_file_delta(delta, SqliteFileKind::Wal) {
        Ok(wal) => *wal,
        Err(error) => {
            lines.push(format!("        SQLite 實體檔案：量不到（{error}）"));
            return;
        }
    };
    let written = match i64::try_from(input.session_image_bytes) {
        Ok(bytes) => bytes,
        Err(_) => {
            lines.push("        現行「其他」：量不到（這場寫圖量超過 i64）".to_string());
            return;
        }
    };
    let image_account = match delta.catalogued_image_bytes.checked_sub(written) {
        Some(bytes) => bytes,
        None => {
            lines.push("        現行「其他」：量不到（畫面帳差溢位）".to_string());
            return;
        }
    };
    match checked_sum(
        [
            delta.logical_allocated_bytes,
            wal.delta_bytes,
            image_account,
        ],
        "現行「其他」",
    ) {
        Ok(other) => lines.push(format!(
            "        現行「其他」 {} = SQLite 邏輯配置 {} + WAL 檔長 {} + 畫面帳差 {}",
            signed_bytes(other),
            signed_bytes(delta.logical_allocated_bytes),
            signed_bytes(wal.delta_bytes),
            signed_bytes(image_account),
        )),
        Err(error) => lines.push(format!("        現行「其他」：量不到（{error}）")),
    }

    lines.push(format!(
        "        SQLite 邏輯配置：總共 {}；空白頁 {}；SQLite 其他配置頁 {}",
        signed_bytes(delta.logical_allocated_bytes),
        signed_bytes(delta.free_bytes),
        signed_bytes(delta.residual_bytes),
    ));
    render_object_deltas(&delta.objects, lines);
    render_sqlite_files(input, delta, lines);
}

fn render_object_deltas(objects: &BTreeMap<String, i64>, lines: &mut Vec<String>) {
    let mut changed: Vec<(&str, i64)> = objects
        .iter()
        .filter_map(|(name, bytes)| (*bytes != 0).then_some((name.as_str(), *bytes)))
        .collect();
    changed.sort_by(|(name_a, bytes_a), (name_b, bytes_b)| {
        bytes_b
            .unsigned_abs()
            .cmp(&bytes_a.unsigned_abs())
            .then_with(|| name_a.cmp(name_b))
    });
    if changed.is_empty() {
        lines.push("              物件：0 B（沒有 table／index 配置淨變化）".to_string());
        return;
    }
    for (name, bytes) in changed.iter().take(8) {
        lines.push(format!("              {name} {}", signed_bytes(*bytes)));
    }
    if changed.len() > 8 {
        let rest = changed[8..]
            .iter()
            .try_fold(0i64, |sum, (_, bytes)| sum.checked_add(*bytes));
        lines.push(match rest {
            Some(bytes) => format!(
                "              其餘 {} 個物件合計 {}",
                changed.len() - 8,
                signed_bytes(bytes)
            ),
            None => format!(
                "              其餘 {} 個物件合計量不到（溢位）",
                changed.len() - 8
            ),
        });
    }
}

fn render_sqlite_files(input: &AttributionInput, delta: &DbDiskDelta, lines: &mut Vec<String>) {
    lines.push("        實體檔案（另一個口徑，不和邏輯配置相加）：".to_string());
    let Some(files) = delta.files.as_ref() else {
        lines.push("              量不到（這不是實體 SQLite 資料庫）".to_string());
        return;
    };
    for (kind, filename) in [
        (SqliteFileKind::Main, "sister.db"),
        (SqliteFileKind::Wal, "sister.db-wal"),
        (SqliteFileKind::Shm, "sister.db-shm"),
    ] {
        match files.get(&kind) {
            Some(file) => lines.push(format!(
                "              {filename} {}（{}）",
                signed_bytes(file.delta_bytes),
                end_size(file.end_bytes)
            )),
            None => lines.push(format!("              {filename} 量不到（快照缺少這一項）")),
        }
    }
    if let Ok((_, after)) = input.db_pair() {
        lines.push(format!(
            "              journal_mode={}；WAL 自動 checkpoint 門檻 {} 頁",
            after.journal_mode, after.wal_autocheckpoint_pages
        ));
    }
    lines.push(
        "              WAL 是可重用工作檔；檔長淨變化不換算每日增長，checkpoint 門檻也不是硬上限。"
            .to_string(),
    );
}

fn render_sidecars(input: &AttributionInput, lines: &mut Vec<String>) {
    let pair = match (&input.sidecars_before, &input.sidecars_after) {
        (Ok(before), Ok(after)) => Ok((before, after)),
        (Err(before), Err(after)) => Err(format!(
            "開場旁檔快照失敗：{before}；收尾旁檔快照也失敗：{after}"
        )),
        (Err(error), _) => Err(format!("開場旁檔快照失敗：{error}")),
        (_, Err(error)) => Err(format!("收尾旁檔快照失敗：{error}")),
    };
    let (before, after) = match pair {
        Ok(pair) => pair,
        Err(error) => {
            lines.push(format!("        資料目錄旁檔：量不到（{error}）"));
            return;
        }
    };

    lines.push(
        "        資料目錄旁檔（不含 DB／frames；收尾摘要寫入前，其他行程也可能在寫）：".to_string(),
    );
    match sidecar_deltas(before, after) {
        Err(error) => lines.push(format!("              量不到（{error}）")),
        Ok(mut deltas) => {
            deltas.retain(|_, bytes| *bytes != 0);
            if deltas.is_empty() {
                lines.push("              0 B（沒有一般檔案淨變化）".to_string());
            } else {
                let mut deltas: Vec<_> = deltas.into_iter().collect();
                deltas.sort_by(|(path_a, bytes_a), (path_b, bytes_b)| {
                    bytes_b
                        .unsigned_abs()
                        .cmp(&bytes_a.unsigned_abs())
                        .then_with(|| path_a.cmp(path_b))
                });
                for (path, bytes) in deltas {
                    lines.push(format!(
                        "              {} {}",
                        path.display(),
                        signed_bytes(bytes)
                    ));
                }
            }
        }
    }

    let unscanned: BTreeSet<&PathBuf> = before
        .unscanned
        .iter()
        .chain(after.unscanned.iter())
        .collect();
    if !unscanned.is_empty() {
        lines.push(format!(
            "              未掃項目：{}（只量第一層一般檔案）",
            unscanned
                .into_iter()
                .map(|path| path.display().to_string())
                .collect::<Vec<_>>()
                .join("、")
        ));
    }
}

fn sidecar_deltas(
    before: &SidecarSnapshot,
    after: &SidecarSnapshot,
) -> Result<BTreeMap<PathBuf, i64>, String> {
    let names: BTreeSet<&PathBuf> = before.files.keys().chain(after.files.keys()).collect();
    names
        .into_iter()
        .map(|name| {
            let before = before.files.get(name).copied().unwrap_or(0);
            let after = after.files.get(name).copied().unwrap_or(0);
            signed_delta(before, after, &name.display().to_string())
                .map(|delta| (name.clone(), delta))
        })
        .collect()
}

fn sqlite_file_delta(delta: &DbDiskDelta, kind: SqliteFileKind) -> Result<&FileDelta, String> {
    delta
        .files
        .as_ref()
        .ok_or_else(|| "這不是實體 SQLite 資料庫，沒有 main/WAL/SHM 檔案量測".to_string())?
        .get(&kind)
        .ok_or_else(|| format!("SQLite 快照缺少 {kind:?} 檔案量測"))
}

fn checked_sum<const N: usize>(values: [i64; N], label: &str) -> Result<i64, String> {
    values.into_iter().try_fold(0i64, |sum, value| {
        sum.checked_add(value)
            .ok_or_else(|| format!("{label}相加時溢位"))
    })
}

fn signed_delta(before: u64, after: u64, label: &str) -> Result<i64, String> {
    if after >= before {
        i64::try_from(after - before).map_err(|_| format!("{label} 的成長量超過 i64"))
    } else {
        let magnitude =
            i64::try_from(before - after).map_err(|_| format!("{label} 的縮減量超過 i64"))?;
        Ok(-magnitude)
    }
}

fn signed_bytes(bytes: i64) -> String {
    match bytes.cmp(&0) {
        std::cmp::Ordering::Greater => format!("+{}", human_bytes(bytes.unsigned_abs())),
        std::cmp::Ordering::Less => format!("-{}", human_bytes(bytes.unsigned_abs())),
        std::cmp::Ordering::Equal => "0 B".to_string(),
    }
}

fn human_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    format!("{value:.1} {}", UNITS[unit])
}

fn end_size(bytes: Option<u64>) -> String {
    bytes.map_or_else(
        || "收尾不存在".to_string(),
        |bytes| format!("收尾共 {}", human_bytes(bytes)),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn db_snapshot(
        logical: u64,
        objects: BTreeMap<String, u64>,
        free: u64,
        residual: u64,
        images: u64,
        files: (Option<u64>, Option<u64>, Option<u64>),
    ) -> DbDiskSnapshot {
        assert_eq!(
            objects.values().sum::<u64>() + free + residual,
            logical,
            "fixture 自己要先對得起來"
        );
        DbDiskSnapshot {
            logical_allocated_bytes: logical,
            objects,
            free_bytes: free,
            residual_bytes: residual,
            catalogued_image_bytes: images,
            files: Some(BTreeMap::from([
                (SqliteFileKind::Main, files.0),
                (SqliteFileKind::Wal, files.1),
                (SqliteFileKind::Shm, files.2),
            ])),
            journal_mode: "wal".to_string(),
            wal_autocheckpoint_pages: 1_000,
        }
    }

    fn sidecars(files: &[(&str, u64)]) -> SidecarSnapshot {
        SidecarSnapshot {
            files: files
                .iter()
                .map(|(name, bytes)| (PathBuf::from(name), *bytes))
                .collect(),
            unscanned: Vec::new(),
        }
    }

    fn measured_input() -> AttributionInput {
        AttributionInput {
            db_before: Ok(db_snapshot(
                1_000,
                BTreeMap::from([("alpha".into(), 700)]),
                200,
                100,
                500,
                (Some(1_000), Some(200), Some(300)),
            )),
            db_after: Ok(db_snapshot(
                1_600,
                BTreeMap::from([("alpha".into(), 800), ("beta".into(), 400)]),
                250,
                150,
                650,
                (Some(1_100), Some(500), Some(300)),
            )),
            sidecars_before: Ok(sidecars(&[("record.log", 100)])),
            sidecars_after: Ok(sidecars(&[("record.log", 120)])),
            session_image_bytes: 120,
        }
    }

    #[test]
    fn current_total_and_other_use_the_exact_three_named_terms() {
        let input = measured_input();
        assert_eq!(input.total_delta_bytes(), Ok(1_050));
        let out = render(&input).join("\n");
        assert!(
            out.contains(
                "現行「其他」 +930 B = SQLite 邏輯配置 +600 B + WAL 檔長 +300 B + 畫面帳差 +30 B"
            ),
            "{out}"
        );
        assert!(out.contains("sister.db +100 B（收尾共 1.1 KB）"), "{out}");
        assert!(
            out.contains("sister.db-wal +300 B（收尾共 500 B）"),
            "{out}"
        );
        assert!(out.contains("sister.db-shm 0 B（收尾共 300 B）"), "{out}");
        assert!(!out.contains("/天"), "WAL 歸因不准外推每天：{out}");
    }

    #[test]
    fn object_growth_and_shrinkage_keep_their_names_and_signs() {
        let mut input = measured_input();
        input.db_before.as_mut().unwrap().objects = BTreeMap::from([
            ("grew".into(), 100),
            ("shrunk".into(), 400),
            ("gone".into(), 200),
        ]);
        input.db_before.as_mut().unwrap().free_bytes = 200;
        input.db_before.as_mut().unwrap().residual_bytes = 100;
        input.db_before.as_mut().unwrap().logical_allocated_bytes = 1_000;
        input.db_after.as_mut().unwrap().objects =
            BTreeMap::from([("grew".into(), 500), ("shrunk".into(), 300)]);
        input.db_after.as_mut().unwrap().free_bytes = 250;
        input.db_after.as_mut().unwrap().residual_bytes = 150;
        input.db_after.as_mut().unwrap().logical_allocated_bytes = 1_200;
        let out = render(&input).join("\n");
        assert!(out.contains("grew +400 B"), "{out}");
        assert!(out.contains("shrunk -100 B"), "{out}");
        assert!(out.contains("gone -200 B"), "{out}");
        assert!(out.contains("空白頁 +50 B"), "{out}");
        assert!(out.contains("SQLite 其他配置頁 +50 B"), "{out}");
    }

    #[test]
    fn sidecar_scanner_excludes_db_and_frames_but_names_unknown_entries() {
        let tmp = crate::ops::tmp::Tmp::new("disk-sidecars");
        for (name, body) in [
            ("sister.db", "db"),
            ("sister.db-wal", "wal"),
            ("sister.db-shm", "shm"),
            ("record.log", "record"),
            ("capabilities.json", "{}"),
            ("leftover.tmp", "tmp"),
        ] {
            std::fs::write(tmp.0.join(name), body).expect("write fixture");
        }
        std::fs::create_dir_all(tmp.0.join("frames")).expect("frames");
        std::fs::create_dir_all(tmp.0.join("unknown-cache")).expect("unknown");

        let snapshot = sidecar_snapshot(&tmp.0).expect("snapshot");
        for wanted in ["record.log", "capabilities.json", "leftover.tmp"] {
            assert!(
                snapshot.files.contains_key(Path::new(wanted)),
                "少了 {wanted}"
            );
        }
        for excluded in ["sister.db", "sister.db-wal", "sister.db-shm", "frames"] {
            assert!(
                !snapshot.files.contains_key(Path::new(excluded)),
                "重算了 {excluded}"
            );
        }
        assert_eq!(snapshot.unscanned, vec![PathBuf::from("unknown-cache")]);
    }

    #[test]
    fn failed_measurement_and_a_measured_zero_are_different_sentences() {
        let mut failed = measured_input();
        failed.db_before = Err("permission denied".to_string());
        failed.sidecars_after = Err("directory vanished".to_string());
        assert!(failed.total_delta_bytes().is_err());
        let failed = render(&failed).join("\n");
        assert!(failed.contains("SQLite：量不到"), "{failed}");
        assert!(failed.contains("資料目錄旁檔：量不到"), "{failed}");

        let before = db_snapshot(
            1_000,
            BTreeMap::from([("same".into(), 700)]),
            200,
            100,
            500,
            (Some(1_000), None, Some(300)),
        );
        let zero = AttributionInput {
            db_before: Ok(before.clone()),
            db_after: Ok(before),
            sidecars_before: Ok(sidecars(&[])),
            sidecars_after: Ok(sidecars(&[])),
            session_image_bytes: 0,
        };
        assert_eq!(zero.total_delta_bytes(), Ok(0));
        let zero = render(&zero).join("\n");
        assert!(zero.contains("現行「其他」 0 B"), "{zero}");
        assert!(zero.contains("0 B（沒有一般檔案淨變化）"), "{zero}");
        assert!(!zero.contains("量不到"), "{zero}");
    }
}
