#!/usr/bin/env python3
"""用真正的 CLI 跑 checked-in moment baseline。

單元測試守得到候選規則，守不到 clap 接線、fixture 路徑和 `status --json`
仍然接在同一條路上。這支腳本故意呼叫已建好的 `sister`。

釘住的四件事：
1. `moments draft` 對 baseline corpus 產出的候選數與各 candidate 分佈。
2. corpus 裡明明有多種 System 事件，`notification` 候選數是 0
   （L0 沒有通知訊號；Lock 的 detail 寫著「通知」也不算）。
3. 兩種 0 分得開：Draft 的 unlabeled == total，Reviewed 的 unlabeled == 0；
   同一個欄位真的會變，不是和 should_speak 印成同一個數字。
4. `review` 不帶 `--confirm-private-text-reviewed` 會失敗。

另外至少一條「改了輸入、輸出跟著動」：多一幀含日期的畫面，
DateTimeMention 候選必須 +1——不是斷在自己寫死的常數上。
"""

import json
import os
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
SISTER = os.environ.get("SISTER", str(ROOT / "target/debug/sister"))
CORPUS = ROOT / "scenarios/moment-baseline.corpus.json"
MOMENTS = ROOT / "scenarios/moment-baseline.moments.json"

# replay corpus 不准帶 SessionStart／SessionEnd（import 會自己建立）。
# 其餘每種 SystemKind 都要在場，才能證明「它是 System 也不算通知」。
REQUIRED_SYSTEM_KINDS = {
    "Lock",
    "Unlock",
    "Sleep",
    "Wake",
    "CapturePaused",
    "CaptureResumed",
    "Excluded",
}
FORBIDDEN_SESSION_MARKS = {"SessionStart", "SessionEnd"}

failed = []


def die(message):
    failed.append(message)
    print(f"✗ {message}")


def at(value, *path):
    current = value
    walked = []
    for part in path:
        walked.append(str(part))
        try:
            current = current[part]
        except (KeyError, IndexError, TypeError):
            die(f"JSON 少了 {'.'.join(walked)}")
            return None
    return current


def expect(value, expected, label):
    if value != expected:
        die(f"{label} 應該是 {expected!r}，實際是 {value!r}")


def run_sister(args, **kwargs):
    command = [SISTER, *args]
    try:
        return subprocess.run(
            command,
            cwd=ROOT,
            capture_output=True,
            text=True,
            check=False,
            **kwargs,
        )
    except OSError as error:
        die(f"跑不起 sister：{error}（先 cargo build -p sister-cli）")
        return None


def load_status_json(completed, label):
    if completed is None:
        return None
    if completed.returncode != 0:
        die(f"{label} 回傳 {completed.returncode}")
        if completed.stderr.strip():
            print(completed.stderr.rstrip())
        return None
    try:
        return json.loads(completed.stdout)
    except json.JSONDecodeError as error:
        die(f"{label} 沒有輸出完整 JSON：{error}")
        if completed.stdout.strip():
            print(completed.stdout.rstrip())
        return None


def status_json(corpus, moments, label):
    return load_status_json(
        run_sister(["replay", "moments", "status", str(corpus), str(moments), "--json"]),
        label,
    )


def private_text_leaked(blob, samples, label):
    for sample in samples:
        if sample and sample in blob:
            die(f"{label} 把未去敏的原文放進 JSON：{sample!r}")


if len(sys.argv) > 1:
    print(f"usage: {Path(sys.argv[0]).name}", file=sys.stderr)
    raise SystemExit(2)

print("▶ 用真正的 sister CLI 跑 synthetic moment baseline")

corpus_doc = json.loads(CORPUS.read_text(encoding="utf-8"))
system_kinds = {
    event["kind"]
    for event in corpus_doc.get("events", [])
    if event.get("type") == "system"
}
missing = REQUIRED_SYSTEM_KINDS - system_kinds
if missing:
    die(f"baseline corpus 必須含這些 SystemKind 才能證明它們不是通知：缺 {sorted(missing)}")
leaked_marks = system_kinds & FORBIDDEN_SESSION_MARKS
if leaked_marks:
    die(f"replay corpus 不可帶 session 邊界：{sorted(leaked_marks)}")

with tempfile.TemporaryDirectory(prefix="moment-baseline-") as tmp:
    tmp_path = Path(tmp)
    draft_path = tmp_path / "draft.moments.json"
    draft_status = None
    drafted = run_sister(
        [
            "replay",
            "moments",
            "draft",
            str(CORPUS),
            "--to",
            str(draft_path),
        ]
    )
    if drafted is None:
        pass
    elif drafted.returncode != 0:
        die(f"sister replay moments draft 回傳 {drafted.returncode}")
        if drafted.stderr.strip():
            print(drafted.stderr.rstrip())
    elif not draft_path.is_file():
        die("moments draft 沒有寫出目的檔")
    else:
        draft_status = status_json(CORPUS, draft_path, "draft status --json")
        if draft_status is not None:
            expect(at(draft_status, "format_version"), 1, "draft.format_version")
            expect(at(draft_status, "review"), "draft", "draft.review")
            expect(at(draft_status, "corpus", "review"), "reviewed", "draft.corpus.review")
            expect(at(draft_status, "corpus", "name"), "synthetic-moment-baseline", "draft.corpus.name")
            expect(at(draft_status, "counts", "total"), 3, "draft.counts.total")
            expect(at(draft_status, "counts", "unlabeled"), 3, "draft.counts.unlabeled")
            expect(at(draft_status, "counts", "commitments"), 0, "draft.counts.commitments")
            expect(at(draft_status, "counts", "should_speak"), 0, "draft.counts.should_speak")
            expect(at(draft_status, "counts", "should_stay_quiet"), 0, "draft.counts.should_stay_quiet")
            expect(at(draft_status, "candidates", "datetime_mention"), 2, "draft.candidates.datetime_mention")
            expect(at(draft_status, "candidates", "long_dwell"), 1, "draft.candidates.long_dwell")
            expect(
                at(draft_status, "candidates", "notification"),
                0,
                "draft.candidates.notification（corpus 有多種 System 事件，L0 沒有通知訊號）",
            )
            expect(at(draft_status, "candidates", "hand_picked"), 0, "draft.candidates.hand_picked")
            private_text_leaked(
                json.dumps(draft_status, ensure_ascii=False),
                (
                    "下午5點接她",
                    "明天 17:00 開會",
                    "LINE 通知：會計師來訊",
                    "synthetic editor still open after five minutes",
                ),
                "draft status --json",
            )

            extra_path = tmp_path / "extra.corpus.json"
            extra = json.loads(json.dumps(corpus_doc))
            extra["events"].append(
                {
                    "type": "frame",
                    "at_ms": extra["duration_ms"],
                    "monitor": 0,
                    "width": 1280,
                    "height": 720,
                    "dhash": 99,
                    "dup_run": 0,
                    "focus": {
                        "app_id": "notes.exe",
                        "app_name": "Synthetic Notes",
                        "window_title": "deadline",
                        "url": None,
                    },
                    "ocr": [
                        {
                            "text": "2026-08-25 截止",
                            "x": 20,
                            "y": 30,
                            "w": 300,
                            "h": 20,
                            "confidence": 1.0,
                        }
                    ],
                }
            )
            extra_path.write_text(json.dumps(extra), encoding="utf-8")
            extra_draft = tmp_path / "extra.moments.json"
            extra_run = run_sister(
                [
                    "replay",
                    "moments",
                    "draft",
                    str(extra_path),
                    "--to",
                    str(extra_draft),
                ]
            )
            extra_status = None
            if extra_run is None:
                pass
            elif extra_run.returncode != 0:
                die(f"加了一幀含日期之後 draft 回傳 {extra_run.returncode}")
                if extra_run.stderr.strip():
                    print(extra_run.stderr.rstrip())
            else:
                extra_status = status_json(extra_path, extra_draft, "extra datetime status --json")
            if extra_status is not None:
                expect(
                    at(extra_status, "candidates", "datetime_mention"),
                    3,
                    "多一幀「2026-08-25 截止」，datetime_mention 必須從 2 變成 3",
                )
                expect(
                    at(extra_status, "candidates", "long_dwell"),
                    1,
                    "多一幀日期不該動到 long_dwell",
                )
                expect(
                    at(extra_status, "candidates", "notification"),
                    0,
                    "多一幀日期不該冒出 notification 候選",
                )

            short_path = tmp_path / "short.corpus.json"
            short = json.loads(json.dumps(corpus_doc))
            short["events"] = [
                event
                for event in short["events"]
                if "synthetic editor still open after five minutes"
                not in json.dumps(event, ensure_ascii=False)
            ]
            short_path.write_text(json.dumps(short), encoding="utf-8")
            short_draft = tmp_path / "short.moments.json"
            short_run = run_sister(
                [
                    "replay",
                    "moments",
                    "draft",
                    str(short_path),
                    "--to",
                    str(short_draft),
                ]
            )
            short_status = None
            if short_run is None:
                pass
            elif short_run.returncode != 0:
                die(f"拿掉跨過五分鐘那一幀之後 draft 回傳 {short_run.returncode}")
                if short_run.stderr.strip():
                    print(short_run.stderr.rstrip())
            else:
                short_status = status_json(short_path, short_draft, "short dwell status --json")
            if short_status is not None:
                expect(
                    at(short_status, "candidates", "long_dwell"),
                    0,
                    "拿掉跨過五分鐘的那一幀，long_dwell 必須從 1 變成 0",
                )
                expect(
                    at(short_status, "candidates", "datetime_mention"),
                    2,
                    "拿掉停留幀不該動到 datetime_mention",
                )

    reviewed_status = status_json(CORPUS, MOMENTS, "reviewed status --json")
    if reviewed_status is not None:
        expect(at(reviewed_status, "review"), "reviewed", "reviewed.review")
        expect(at(reviewed_status, "counts", "total"), 4, "reviewed.counts.total")
        expect(at(reviewed_status, "counts", "unlabeled"), 0, "reviewed.counts.unlabeled")
        expect(at(reviewed_status, "counts", "commitments"), 2, "reviewed.counts.commitments")
        expect(
            at(reviewed_status, "counts", "commitments_with_due"),
            1,
            "reviewed.counts.commitments_with_due",
        )
        expect(at(reviewed_status, "counts", "should_speak"), 1, "reviewed.counts.should_speak")
        expect(
            at(reviewed_status, "counts", "should_stay_quiet"),
            1,
            "reviewed.counts.should_stay_quiet",
        )
        expect(
            at(reviewed_status, "candidates", "datetime_mention"),
            2,
            "reviewed.candidates.datetime_mention",
        )
        expect(at(reviewed_status, "candidates", "long_dwell"), 1, "reviewed.candidates.long_dwell")
        expect(
            at(reviewed_status, "candidates", "notification"),
            0,
            "reviewed.candidates.notification",
        )
        expect(at(reviewed_status, "candidates", "hand_picked"), 1, "reviewed.candidates.hand_picked")
        private_text_leaked(
            json.dumps(reviewed_status, ensure_ascii=False),
            (
                "五點接她",
                "正在寫合成範例，畫面沒有任何該開口的訊號",
                "同一個合成檔停留超過五分鐘",
                "開會",
            ),
            "reviewed status --json",
        )

    if draft_status is not None and reviewed_status is not None:
        draft_unlabeled = at(draft_status, "counts", "unlabeled")
        reviewed_unlabeled = at(reviewed_status, "counts", "unlabeled")
        if draft_unlabeled is not None and reviewed_unlabeled is not None:
            if draft_unlabeled == reviewed_unlabeled:
                die(
                    "unlabeled 在 Draft 與 Reviewed 上是同一個數字："
                    f"{draft_unlabeled}。這格必須真的會變，不能和「沒量到」長得一樣。"
                )
        draft_speak = at(draft_status, "counts", "should_speak")
        reviewed_speak = at(reviewed_status, "counts", "should_speak")
        if (
            draft_unlabeled is not None
            and reviewed_unlabeled is not None
            and draft_speak is not None
            and reviewed_speak is not None
        ):
            if draft_unlabeled == draft_speak and reviewed_unlabeled == reviewed_speak:
                die(
                    "unlabeled 與 should_speak 在兩份 fixture 上都是同一對數字；"
                    "兩個欄位沒有被證明是獨立的"
                )

    reviewed_out = tmp_path / "should-not-write.moments.json"
    review_without_flag = run_sister(
        [
            "replay",
            "moments",
            "review",
            str(CORPUS),
            str(MOMENTS),
            "--to",
            str(reviewed_out),
        ]
    )
    if review_without_flag is None:
        pass
    elif review_without_flag.returncode == 0:
        die("review 不帶 --confirm-private-text-reviewed 應該失敗")
    else:
        text = f"{review_without_flag.stderr}\n{review_without_flag.stdout}"
        if "--confirm-private-text-reviewed" not in text and "沒有自動去敏" not in text:
            die(
                "review 失敗了，但原因不是隱私確認旗標："
                + text.strip()[:500]
            )
        elif reviewed_out.exists():
            die("review 拒絕之後仍寫出了目的檔")
        else:
            print("✓ review 不帶 --confirm-private-text-reviewed 會失敗")

if failed:
    print(f"\n{len(failed)} 個 moment baseline 契約壞了。", file=sys.stderr)
    raise SystemExit(1)

print("✓ draft 候選分佈由 fixture 決定；多一幀日期就 +1 DateTimeMention")
print("✓ corpus 有多種 System 事件，notification 仍是 0")
print("✓ unlabeled 在 Draft／Reviewed 上真的會變，且與 should_speak 分開")
print("✓ review 不帶隱私確認旗標會失敗")
