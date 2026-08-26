#!/usr/bin/env python3
"""用真正的 CLI 跑 checked-in replay baseline，守住 README 那組答案。

單元測試可以守住 evaluator 裡的算式，卻守不住 clap 接線、fixture 路徑和 JSON
輸出仍然接在同一條路上。這支腳本故意呼叫已建好的 `sister`，再只斷言由兩份
fixture 決定的結果。延遲會隨 runner 浮動，所以只驗它真的量了五題，而且每個值
都是有限的非負數；不拿任何毫秒數當 gate。README 的品質／成本表也由同一份
JSON 生成；傳 `--update-readme` 可重生，預設則檢查它沒有漂走。有意接受新分數時
先更新下面的 regression contract，再重生 README；更新指令不會繞過那份契約。
"""

import json
import math
import os
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
SISTER = os.environ.get("SISTER", str(ROOT / "target/debug/sister"))
CORPUS = ROOT / "scenarios/recall-baseline.corpus.json"
QUESTIONS = ROOT / "scenarios/recall-baseline.questions.json"
SESSION_CORPUS = ROOT / "scenarios/recall-session.corpus.json"
SESSION_QUESTIONS = ROOT / "scenarios/recall-session.questions.json"
README = ROOT / "README.md"
README_START = "<!-- BEGIN GENERATED: recall-benchmark -->"
README_END = "<!-- END GENERATED: recall-benchmark -->"
# 第二張表。第一張的 5 題都沒有時間範圍，`facts_session` 在上面和 `facts`
# 一模一樣——只登第一張的話，README 會讓人以為章節什麼都沒帶來。章節幫得上
# 忙的是「昨天下午」這種問法，證據要跟著登出來，否則 PHASES.md 那條
# 「找回率提升可量測」是一句沒有公開憑據的話。
SESSION_README_START = "<!-- BEGIN GENERATED: recall-session-benchmark -->"
SESSION_README_END = "<!-- END GENERATED: recall-session-benchmark -->"

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


def expect_fraction(metrics, key, passed, total, label):
    fraction = at(metrics, key)
    if fraction is None:
        return
    expect(at(fraction, "passed"), passed, f"{label}.passed")
    expect(at(fraction, "total"), total, f"{label}.total")
    expect(at(fraction, "rate"), passed / total, f"{label}.rate")


def expect_finite_nonnegative(value, label):
    if (
        isinstance(value, bool)
        or not isinstance(value, (int, float))
        or not math.isfinite(value)
        or value < 0
    ):
        die(f"{label} 應該是有限的非負數，實際是 {value!r}")


def format_fraction(value):
    passed = at(value, "passed")
    total = at(value, "total")
    rate = at(value, "rate")
    if passed is None or total is None:
        return "資料不完整"
    if rate is None:
        return "沒有這類題"
    return f"{passed}/{total}（{rate * 100:.1f}%）"


def format_cost(value):
    if isinstance(value, bool) or not isinstance(value, (int, float)):
        return str(value)
    return f"US${value:g}/天"


def format_model_calls(model):
    if not isinstance(model, dict):
        return "資料不完整"
    kind = model.get("kind")
    if kind == "not_on_path":
        return "沒跑腦"
    if kind == "measured":
        return str(model.get("calls"))
    return "資料不完整"


def format_model_cost(model):
    if not isinstance(model, dict):
        return "資料不完整"
    kind = model.get("kind")
    if kind == "not_on_path":
        return "沒跑腦"
    if kind == "measured":
        calls = model.get("calls")
        usd = model.get("usd_per_day")
        if calls == 0:
            return "US$0/天（跑了，沒呼叫）"
        return format_cost(usd)
    return "資料不完整"


def readme_block(report, start=README_START, end=README_END):
    lines = [
        start,
        "<!-- 由 scripts/check-recall-baseline.py 生成；不要手改這一段。 -->",
        "| 配置 | 找回率@5 | 答案正確率 | 出處正確率 | 模型呼叫 | 成本 |",
        "|---|---:|---:|---:|---:|---:|",
    ]
    configurations = at(report, "configurations")
    if configurations is None:
        configurations = []
    for config in configurations:
        name = at(config, "name")
        metrics = at(config, "metrics")
        if name is None or metrics is None:
            continue
        lines.append(
            "| `{}` | {} | {} | {} | {} | {} |".format(
                name,
                format_fraction(at(metrics, "recall_at_k")),
                format_fraction(at(metrics, "answer_accuracy")),
                format_fraction(at(metrics, "citation_accuracy")),
                format_model_calls(at(metrics, "model")),
                format_model_cost(at(metrics, "model")),
            )
        )
    lines.append(end)
    return "\n".join(lines)


def sync_readme(report, update, start=README_START, end=README_END, label="recall"):
    source = README.read_bytes()
    start_marker = start.encode("utf-8")
    end_marker = end.encode("utf-8")
    if source.count(start_marker) != 1 or source.count(end_marker) != 1:
        die(f"README 應恰有一組 {label} benchmark 生成標記：{start} / {end}")
        return

    begin_at = source.index(start_marker)
    end_at = source.index(end_marker)
    if end_at < begin_at:
        die(f"README 的 {label} benchmark 生成標記順序反了：{start} 必須在 {end} 前面")
        return

    after_start = begin_at + len(start_marker)
    if source[after_start : after_start + 2] == b"\r\n":
        newline = "\r\n"
    elif source[after_start : after_start + 1] == b"\n":
        newline = "\n"
    else:
        die(f"README 的 {start} 後面必須直接換行")
        return

    stop_at = end_at + len(end_marker)
    current = source[begin_at:stop_at]
    expected = readme_block(report, start, end).replace("\n", newline).encode("utf-8")
    if current == expected:
        print(f"✓ README 的 {label} 品質／成本表和本次 CLI JSON 一致")
        return

    if update:
        README.write_bytes(source[:begin_at] + expected + source[stop_at:])
        print(f"✓ 已用本次 CLI JSON 重生 README 的 {label} 品質／成本表")
        return

    die(
        f"README 的 {label} 品質／成本表和本次 CLI JSON 不一致；請跑 "
        "`SISTER=./target/debug/sister python3 "
        "./scripts/check-recall-baseline.py --update-readme` 重生。"
        "這只同步 README，不會改 regression contract；若 CLI 分數是有意接受的新版，"
        "要先審查變動並更新腳本裡的 expected_scores，再跑更新指令"
    )


def run(corpus, questions):
    command = [
        SISTER,
        "replay",
        "evaluate",
        str(corpus),
        str(questions),
        "--k",
        "5",
        "--runs",
        "1",
        "--json",
    ]
    try:
        completed = subprocess.run(
            command,
            cwd=ROOT,
            capture_output=True,
            text=True,
            check=False,
        )
    except OSError as error:
        die(f"跑不起 sister：{error}（先 cargo build -p sister-cli）")
        return None

    if completed.returncode != 0:
        die(f"sister replay evaluate 回傳 {completed.returncode}")
        if completed.stderr.strip():
            print(completed.stderr.rstrip())
        return None

    try:
        return json.loads(completed.stdout)
    except json.JSONDecodeError as error:
        die(f"sister --json 沒有輸出完整 JSON：{error}")
        if completed.stdout.strip():
            print(completed.stdout.rstrip())
        return None


if len(sys.argv) > 2 or (len(sys.argv) == 2 and sys.argv[1] != "--update-readme"):
    print(
        f"usage: {Path(sys.argv[0]).name} [--update-readme]",
        file=sys.stderr,
    )
    raise SystemExit(2)
update_readme = len(sys.argv) == 2

UNMEASURED = (
    "reminder_false_positive_rate",
    "reminder_miss_rate",
    "segmentation_f1",
    "reviewer_lookup_rate",
    "cpu_percent",
    "ram_peak_mb",
    "battery_percent_per_hour",
    "disk_bytes",
)


def check_configurations(report, expected_names, expected_scores, samples, label):
    configurations = at(report, "configurations")
    if configurations is None:
        return
    expect(
        [at(config, "name") for config in configurations],
        expected_names,
        f"{label} configurations 的順序與名稱",
    )
    for config in configurations:
        name = at(config, "name")
        if name not in expected_scores:
            continue
        metrics = at(config, "metrics")
        rows = at(config, "questions")
        if metrics is None or rows is None:
            continue

        for metric, (passed, total) in expected_scores[name].items():
            expect_fraction(metrics, metric, passed, total, f"{label}.{name}.{metric}")

        # 不跑腦那一路仍然是「沒呼叫模型」。型別上是 not_on_path，
        # 不是量到 0 次——那是「跑了腦但沒花錢」才用的數字。
        expect(at(metrics, "model", "kind"), "not_on_path", f"{label}.{name}.model.kind")
        if isinstance(metrics, dict) and "model_calls" in metrics:
            die(
                f"{label}.{name} 還有 model_calls={metrics['model_calls']!r}——"
                "不跑腦不該再印成一個和「跑了沒花錢」分不開的 0"
            )
        for field in UNMEASURED:
            expect(at(metrics, field), None, f"{label}.{name}.{field}")

        latency = at(metrics, "latency")
        if latency is not None:
            expect(at(latency, "samples"), samples, f"{label}.{name}.latency.samples")
            for field in ("p50_ms", "p95_ms", "max_ms"):
                expect_finite_nonnegative(
                    at(latency, field), f"{label}.{name}.latency.{field}"
                )

        known_miss = next((row for row in rows if at(row, "id") == "known-miss"), None)
        if known_miss is None:
            die(f"{label}.{name} 少了 known-miss 這題")
        else:
            expect(at(known_miss, "recalled"), None, f"{label}.{name}.known-miss.recalled")
            expect(
                at(known_miss, "citation_correct"),
                None,
                f"{label}.{name}.known-miss.citation_correct",
            )
            expect(at(known_miss, "returned"), [], f"{label}.{name}.known-miss.returned")

        for row in rows:
            question_id = at(row, "id")
            expect_finite_nonnegative(
                at(row, "latency_median_ms"),
                f"{label}.{name}.{question_id}.latency_median_ms",
            )


print("▶ 用真正的 sister CLI 跑 synthetic recall baseline")
report = run(CORPUS, QUESTIONS)

if report is not None:
    corpus = at(report, "corpus")
    questions = at(report, "question_set")

    if corpus is not None:
        expect(at(corpus, "review"), "reviewed", "corpus.review")
        expect(at(corpus, "events"), 3, "corpus.events")

    if questions is not None:
        expect(at(questions, "review"), "reviewed", "question_set.review")
        expect(at(questions, "questions"), 5, "question_set.questions")
        sources = at(questions, "sources")
        if sources is not None:
            expect(
                sources,
                {"query_log": 0, "hand_labeled": 3, "planted": 2},
                "question_set.sources",
            )

    # Phase 2 那 5 題沒有時間範圍，+session 不該偷偷改變分數。
    expected_scores = {
        "baseline_text": {
            "recall_at_k": (2, 4),
            "answer_accuracy": (3, 5),
            "citation_accuracy": (2, 4),
        },
        "facts": {
            "recall_at_k": (4, 4),
            "answer_accuracy": (5, 5),
            "citation_accuracy": (4, 4),
        },
        "facts_session": {
            "recall_at_k": (4, 4),
            "answer_accuracy": (5, 5),
            "citation_accuracy": (4, 4),
        },
    }
    check_configurations(
        report,
        ["baseline_text", "facts", "facts_session"],
        expected_scores,
        5,
        "recall-baseline",
    )

print("▶ 用真正的 sister CLI 跑活動級章節 corpus")
session_report = run(SESSION_CORPUS, SESSION_QUESTIONS)

if session_report is not None:
    corpus = at(session_report, "corpus")
    questions = at(session_report, "question_set")
    if corpus is not None:
        expect(at(corpus, "review"), "reviewed", "session.corpus.review")
        expect(at(corpus, "events"), 7, "session.corpus.events")
        expect(at(corpus, "duration_ms"), 6900000, "session.corpus.duration_ms")
    if questions is not None:
        expect(at(questions, "review"), "reviewed", "session.question_set.review")
        expect(at(questions, "questions"), 3, "session.question_set.questions")

    # 先釘這組；分數若和預期不同，照腳本自己寫的流程改 expected_scores，
    # 不准為了讓 +session 贏去動 fixture 或評分邏輯。
    session_scores = {
        "baseline_text": {
            "recall_at_k": (1, 2),
            "answer_accuracy": (2, 3),
            "citation_accuracy": (1, 2),
        },
        "facts": {
            "recall_at_k": (1, 2),
            "answer_accuracy": (2, 3),
            "citation_accuracy": (1, 2),
        },
        "facts_session": {
            "recall_at_k": (2, 2),
            "answer_accuracy": (3, 3),
            "citation_accuracy": (2, 2),
        },
    }
    check_configurations(
        session_report,
        ["baseline_text", "facts", "facts_session"],
        session_scores,
        3,
        "recall-session",
    )

if report is not None and not failed:
    sync_readme(report, update_readme)

if session_report is not None and not failed:
    sync_readme(
        session_report,
        update_readme,
        SESSION_README_START,
        SESSION_README_END,
        "recall-session",
    )

if failed:
    print(f"\n{len(failed)} 個 replay baseline 契約壞了。", file=sys.stderr)
    print(
        "若這是有意接受的新 baseline：先審查差異並更新本腳本的 expected_scores，"
        "再用 --update-readme 重生 README；更新指令不會跳過 regression contract。",
        file=sys.stderr,
    )
    raise SystemExit(1)

print("✓ 3 個事件、5 題、三個產品 profile 的結果都和 baseline 一致")
print("✓ 活動級章節 corpus：7 個事件、3 題，分數已釘")
print("✓ 延遲有樣本且都是有限非負數；沒有鎖毫秒門檻")
