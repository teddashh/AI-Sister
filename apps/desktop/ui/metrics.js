// 開發者指標頁。JSON 先交給 Rust 解析成固定 schema，再裁掉所有自由字串；
// 這裡只畫數值摘要與 1-based 失敗題號。有限數字與 null 的顯示仍在這裡
// 逐欄處理，不把壞值印成 NaN。

const invoke = globalThis.__TAURI__?.core?.invoke ?? null;

const el = {
  file: document.querySelector("[data-file]"),
  status: document.querySelector("[data-status]"),
  private: document.querySelector("[data-private]"),
  report: document.querySelector("[data-report]"),
  corpus: document.querySelector("[data-corpus]"),
  corpusMeta: document.querySelector("[data-corpus-meta]"),
  questions: document.querySelector("[data-questions]"),
  questionMeta: document.querySelector("[data-question-meta]"),
  parameters: document.querySelector("[data-parameters]"),
  version: document.querySelector("[data-version]"),
  profiles: document.querySelector("[data-profiles]"),
  failures: document.querySelector("[data-failures]"),
  failureList: document.querySelector("[data-failure-list]"),
  other: document.querySelector("[data-other-metrics]"),
};

// 連選兩份檔案時，只准最後一次選擇改畫面。完整 report 可能很大，A 比 B
// 晚 parse 完並不代表使用者又選回 A。
let loadGeneration = 0;

function clearReport(message, bad = false) {
  el.status.textContent = message;
  el.status.classList.toggle("bad", bad);
  el.private.hidden = true;
  el.report.hidden = true;
  el.corpus.textContent = "";
  el.corpusMeta.textContent = "";
  el.questions.textContent = "";
  el.questionMeta.textContent = "";
  el.parameters.textContent = "";
  el.version.textContent = "";
  el.profiles.replaceChildren();
  el.failureList.replaceChildren();
  el.failures.hidden = true;
  el.other.replaceChildren();
}

function review(value) {
  return value === "reviewed" ? "Reviewed" : "Draft";
}

function finite(value) {
  return typeof value === "number" && Number.isFinite(value);
}

function whole(value) {
  return finite(value) ? value.toLocaleString("zh-Hant-TW") : "未量到";
}

function measured(value, suffix = "", digits = 1) {
  if (value === null || !finite(value)) return "未量到";
  return `${value.toFixed(digits)}${suffix}`;
}

function rate(value) {
  if (value === null || !finite(value)) return "未量到";
  return `${(value * 100).toFixed(1)}%`;
}

function fraction(value) {
  if (value?.total === 0) return "不適用（0 題）";
  if (!finite(value?.passed) || !finite(value?.total) || value.rate === null || !finite(value.rate)) {
    return "未量到";
  }
  return `${whole(value.passed)}/${whole(value.total)}（${rate(value.rate)}）`;
}

function latency(value) {
  if (value === null || !finite(value?.samples) || value.samples === 0) return "未量到";
  return `${measured(value.p50_ms, " ms")} / ${measured(value.p95_ms, " ms")} / ${measured(value.max_ms, " ms")}`;
}

function bytes(value) {
  if (value === null || !finite(value)) return "未量到";
  if (value < 1024) return `${whole(value)} B`;
  if (value < 1024 * 1024) return `${(value / 1024).toFixed(1)} KB`;
  return `${(value / 1024 / 1024).toFixed(1)} MB`;
}

function cell(row, text) {
  const td = document.createElement("td");
  td.textContent = text;
  row.append(td);
}

function modelCell(model) {
  if (!model || model.kind === "not_on_path") return "沒跑腦";
  if (model.kind === "measured") {
    if (model.calls === 0) {
      return "0 calls／US$0.00/天（跑了，沒呼叫）";
    }
    return `${whole(model.calls)} calls／US$${measured(model.usd_per_day, "", 2)}/天`;
  }
  return "未量到";
}

function paintProfiles(configurations) {
  el.profiles.replaceChildren();
  el.other.replaceChildren();
  el.failureList.replaceChildren();
  let hasFailures = false;

  for (const config of configurations) {
    const row = document.createElement("tr");
    cell(row, config.name);
    cell(row, fraction(config.recall_at_k));
    cell(row, fraction(config.answer_accuracy));
    cell(row, fraction(config.citation_accuracy));
    cell(row, latency(config.latency));
    cell(row, modelCell(config.model));
    el.profiles.append(row);

    const other = document.createElement("tr");
    cell(other, config.name);
    cell(other, rate(config.reminder_false_positive_rate));
    cell(other, rate(config.reminder_miss_rate));
    cell(other, measured(config.segmentation_f1, "", 3));
    cell(other, rate(config.reviewer_lookup_rate));
    cell(other, measured(config.cpu_percent, "%"));
    cell(other, measured(config.ram_peak_mb, " MB"));
    cell(other, measured(config.battery_percent_per_hour, "%"));
    cell(other, bytes(config.disk_bytes));
    el.other.append(other);

    if (config.failed_question_numbers.length > 0) {
      hasFailures = true;
      const group = document.createElement("div");
      group.className = "failure-group";
      const name = document.createElement("strong");
      name.textContent = config.name;
      const numbers = document.createElement("code");
      numbers.textContent = config.failed_question_numbers.map((number) => `#${whole(number)}`).join("、");
      group.append(name, numbers);
      el.failureList.append(group);
    }
  }
  el.failures.hidden = !hasFailures;
}

function paint(view) {
  const privateDraft =
    view.private_draft || view.corpus.review === "draft" || view.question_set.review === "draft";
  el.private.hidden = !privateDraft;

  el.corpus.textContent = "Corpus";
  el.corpusMeta.textContent = `${review(view.corpus.review)}／${whole(view.corpus.events)} events／${measured(view.corpus.duration_ms / 1000, " 秒")}`;
  el.questions.textContent = "題庫";
  const sources = view.question_set.sources;
  el.questionMeta.textContent = `${review(view.question_set.review)}／${whole(view.question_set.questions)} 題（query log ${whole(sources.query_log)}、人工 ${whole(sources.hand_labeled)}、埋題 ${whole(sources.planted)}）`;
  el.parameters.textContent = `k=${whole(view.parameters.k)}／${whole(view.parameters.runs)} runs`;
  el.version.textContent = `report format v${whole(view.format_version)}／${whole(view.parameters.warmups)} 次暖身`;
  paintProfiles(view.configurations);

  el.status.textContent = "已載入評測報告";
  el.status.classList.remove("bad");
  el.report.hidden = false;
}

async function loadSelected() {
  const generation = ++loadGeneration;
  const file = el.file.files?.[0];
  if (!file) {
    clearReport("尚未載入");
    return;
  }

  // 先撤掉舊畫面。新檔若讀壞，不能讓上一份數字繼續冒充這次選的檔案。
  clearReport("正在讀取…");
  try {
    if (invoke === null) throw new Error("這個頁面不在 AI-Sister desktop 裡");
    // raw JSON 只活在這個區塊的區域變數；不放進 DOM 或 module state。
    const contents = await file.text();
    const view = await invoke("eval_report_view", { contents });
    if (generation === loadGeneration) paint(view);
  } catch (error) {
    if (generation === loadGeneration) {
      clearReport(`讀不進這份報告：${String(error)}`, true);
    }
  } finally {
    // 不讓 WebView 的 file input 繼續握著 raw report；畫面只保留 Rust 裁過的 view。
    if (generation === loadGeneration) el.file.value = "";
  }
}

el.file.addEventListener("change", () => void loadSelected());
clearReport("尚未載入");
