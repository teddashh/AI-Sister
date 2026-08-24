#!/usr/bin/env node
/*
 * Developer metrics 的前端契約：檔案由瀏覽器 File API 讀成字串，Rust 的
 * `eval_report_view` 負責 strict schema／format version 並裁掉 raw 欄位；有限數字
 * 的顯示由前端守。這支直接載產品 JS，守住「上一份不能殘留」、「Draft 警告
 * 不能消失」和「完整 report 的自由字串不能被畫出來」。
 */

import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { domOf, fakeDocument, loader, read, watchNonsense } from "./fake-dom.mjs";

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const UI = join(ROOT, "apps/desktop/ui");
const HTML = read(join(UI, "metrics.html"));
const SOURCE = read(join(UI, "metrics.js"));
const MAIN = read(join(ROOT, "apps/desktop/src-tauri/src/main.rs"));
const CONFIG = read(join(ROOT, "crates/sister-core/src/config.rs"));
const CAPABILITY = JSON.parse(read(join(ROOT, "apps/desktop/src-tauri/capabilities/aux-windows.json")));
const boot = loader(SOURCE);
const SECRET = "RAW-QUESTION-SHOULD-NEVER-RENDER";

function report(over = {}) {
  return {
    format_version: 1,
    evaluator_version: SECRET,
    private_draft: false,
    corpus: {
      name: SECRET,
      review: "reviewed",
      duration_ms: 60000,
      events: 100,
      fingerprint: SECRET,
    },
    question_set: {
      name: SECRET,
      review: "reviewed",
      questions: 30,
      sources: { query_log: 20, hand_labeled: 8, planted: 2 },
      fingerprint: SECRET,
      questions_raw: [{ question: SECRET }],
    },
    parameters: { k: 5, warmups: 1, runs: 3, ranking: SECRET },
    configurations: [
      {
        name: "baseline_text",
        recall_at_k: { passed: 0, total: 0, rate: null },
        answer_accuracy: { passed: 0, total: 30, rate: 0 },
        citation_accuracy: { passed: 10, total: 20, rate: 0.5 },
        latency: { samples: 90, p50_ms: 2.1, p95_ms: 5.2, max_ms: 8.7 },
        model_calls: 0,
        model_usd_per_day: 0,
        reminder_false_positive_rate: null,
        reminder_miss_rate: null,
        segmentation_f1: null,
        reviewer_lookup_rate: null,
        cpu_percent: null,
        ram_peak_mb: null,
        battery_percent_per_hour: null,
        disk_bytes: null,
        failed_question_numbers: [7],
        returned: [{ values: [SECRET] }],
      },
    ],
    ...over,
  };
}

const tick = () => new Promise((resolveTick) => setTimeout(resolveTick, 10));

function deferred() {
  let resolvePromise;
  let rejectPromise;
  const promise = new Promise((resolve, reject) => {
    resolvePromise = resolve;
    rejectPromise = reject;
  });
  return { promise, resolve: resolvePromise, reject: rejectPromise };
}

async function open(view = report()) {
  const node = domOf(HTML);
  globalThis.document = fakeDocument(node);
  const calls = [];
  globalThis.__TAURI__ = {
    core: {
      invoke: async (command, args) => {
        calls.push({ command, args });
        if (view instanceof Error) throw view;
        if (typeof view === "function") return view(command, args);
        return view;
      },
    },
  };
  const nonsense = watchNonsense();
  await boot();
  const file = node("[data-file]");
  return {
    node,
    calls,
    nonsense,
    select(contents = "the exact file contents", name = "report.json") {
      file.files = [{ name, text: async () => contents }];
      // 瀏覽器選完檔會把這格變成非空；不先造出反面，底下「清掉了」的
      // 斷言就算 production 少了 finally 也會永遠是綠的。
      file.value = `C:\\fakepath\\${name}`;
      for (const handler of file.handlers.change ?? []) handler();
    },
    async load(contents = "the exact file contents", name = "report.json") {
      this.select(contents, name);
      await tick();
    },
    text() {
      return [
        node("[data-status]").textContent,
        node("[data-private]").textContent,
        node("[data-corpus]").textContent,
        node("[data-corpus-meta]").textContent,
        node("[data-questions]").textContent,
        node("[data-question-meta]").textContent,
        node("[data-parameters]").textContent,
        node("[data-version]").textContent,
        node("[data-profiles]").textContent,
        node("[data-failure-list]").textContent,
        node("[data-other-metrics]").textContent,
      ].join("\n");
    },
  };
}

let failed = 0;
function check(name, ok, detail) {
  console.log(`  ${ok ? "✔" : "✗"} ${name}`);
  if (!ok) {
    failed++;
    if (detail !== undefined) console.log(`      實際：${JSON.stringify(detail)}`);
  }
}

console.log("① source wiring");
check("HTML 使用原生 file input", /<input\b[^>]*data-file[^>]*type="file"/.test(HTML), undefined);
check("前端透過 File.text() 讀檔", /await\s+file\.text\(\)/.test(SOURCE), undefined);
check("整份字串只交給 eval_report_view", /invoke\("eval_report_view",\s*\{\s*contents\s*\}\)/.test(SOURCE), undefined);
check("metrics window 在 auxiliary capability", CAPABILITY.windows.includes("metrics"), CAPABILITY.windows);
check("前端不讀 question/returned/value 自由文字欄位", !/\.(?:question|returned|values)\b/.test(SOURCE), undefined);
check("developer mode 預設關閉", /developer_mode:\s*false/.test(CONFIG), undefined);
check(
  "系統匣只在 developer mode 建立評測入口",
  /if\s+developer_mode\s*\{[\s\S]*?MenuItem::with_id\([\s\S]*?"metrics"[\s\S]*?"評測指標…"/.test(MAIN),
  undefined,
);
check(
  "評測入口真的開 metrics.html",
  /fn\s+open_metrics\b[\s\S]*?WebviewUrl::App\("metrics\.html"\.into\(\)\)/.test(MAIN),
  undefined,
);
check(
  "developer mode 的 item 真的放進 menu",
  /let\s+menu\s*=\s*match\s*&metrics_item\s*\{[\s\S]*?Some\(metrics_item\)[\s\S]*?Menu::with_items\([\s\S]*?metrics_item,/.test(MAIN),
  undefined,
);
check(
  "系統匣事件真的呼叫 open_metrics",
  /"metrics"\s*=>\s*\{[\s\S]*?open_metrics\(app\.clone\(\)\)/.test(MAIN),
  undefined,
);
check(
  "eval_report_view 有註冊進 Tauri handler",
  /generate_handler!\[[\s\S]*?\beval_report_view\b[\s\S]*?\]/.test(MAIN),
  undefined,
);
check(
  "解析失敗不把 report 原值抄進錯誤訊息",
  /fn\s+eval_report_view\b[\s\S]*?map_err\(\|_\|\s*"JSON 格式或 eval report 版本不符合這一版 sister"/.test(MAIN),
  undefined,
);

console.log("② 初始、成功與未量到");
{
  const page = await open();
  check("初始明講尚未載入", page.node("[data-status]").textContent === "尚未載入", page.text());
  check("初始沒有空白報告", page.node("[data-report]").hidden, page.text());
  await page.load("EXACT-CONTENTS", SECRET);
  check("真的呼叫 eval_report_view", page.calls[0]?.command === "eval_report_view", page.calls);
  check("File.text 的原字串送進 Rust", page.calls[0]?.args?.contents === "EXACT-CONTENTS", page.calls);
  check("invoke 後 file input 不再握著 raw report", page.node("[data-file]").value === "", page.node("[data-file]").value);
  check("成功後才顯示報告", !page.node("[data-report]").hidden, page.text());
  const profileCells = page.node("[data-profiles]").children[0]?.children ?? [];
  const otherCells = page.node("[data-other-metrics]").children[0]?.children ?? [];
  check("0 分母不是 0%", profileCells[1]?.textContent === "不適用（0 題）", profileCells.map((cell) => cell.textContent));
  check("量到的 0 仍是 0/30", profileCells[2]?.textContent === "0/30（0.0%）", profileCells.map((cell) => cell.textContent));
  check("模型的實測 0 沒變成未量到", profileCells[5]?.textContent === "0 calls／US$0.00/天", profileCells.map((cell) => cell.textContent));
  check(
    "八個 null 各自明講未量到",
    otherCells.length === 9 && otherCells.slice(1).every((cell) => cell.textContent === "未量到"),
    otherCells.map((cell) => cell.textContent),
  );
  check("畫面沒有 report 自由字串或檔名", !page.text().includes(SECRET), page.text());
  check("畫面沒有 NaN / undefined", page.nonsense().length === 0, page.nonsense());
}

console.log("③ Draft 警告與錯誤清場");
{
  const draft = report({
    private_draft: true,
    corpus: { ...report().corpus, review: "draft" },
  });
  const page = await open(draft);
  await page.load();
  check(
    "Draft 警告常駐而且直說不要分享",
    !page.node("[data-private]").hidden && /data-private[\s\S]*?不要分享/.test(HTML),
    page.text(),
  );
}
{
  const page = await open(new Error("schema 不對"));
  // 先放一份假的舊畫面，證明錯誤路徑真的清掉，不是因為開場本來就空。
  page.node("[data-report]").hidden = false;
  const old = document.createElement("tr");
  old.textContent = "OLD REPORT";
  page.node("[data-profiles]").append(old);
  page.node("[data-private]").hidden = false;
  await page.load();
  check("錯誤時舊報告整塊撤掉", page.node("[data-report]").hidden && !page.text().includes("OLD REPORT"), page.text());
  check("錯誤時舊 Draft 警告也不殘留", page.node("[data-private]").hidden, page.text());
  check("錯誤不是尚未載入或空白", page.node("[data-status]").textContent.includes("schema 不對"), page.text());
}

console.log("④ 連選兩份只認最後一份");
{
  const pending = new Map();
  const page = await open((_command, args) => {
    const request = deferred();
    pending.set(args.contents, request);
    return request.promise;
  });
  page.select("OLDER", "older.json");
  await tick();
  page.select("LATEST", "latest.json");
  await tick();

  pending.get("LATEST").resolve(
    report({ corpus: { ...report().corpus, events: 222 } }),
  );
  await tick();
  check("較新的報告先完成就顯示它", page.node("[data-corpus-meta]").textContent.includes("222 events"), page.text());

  pending.get("OLDER").resolve(
    report({ corpus: { ...report().corpus, events: 111 } }),
  );
  await tick();
  check(
    "較舊的成功不能蓋掉新報告",
    !page.node("[data-report]").hidden &&
      page.node("[data-corpus-meta]").textContent.includes("222 events") &&
      !page.node("[data-corpus-meta]").textContent.includes("111 events"),
    page.text(),
  );
}
{
  const pending = new Map();
  const page = await open((_command, args) => {
    const request = deferred();
    pending.set(args.contents, request);
    return request.promise;
  });
  page.select("OLDER", "older.json");
  await tick();
  page.select("LATEST", "latest.json");
  await tick();

  pending.get("LATEST").resolve(
    report({ corpus: { ...report().corpus, events: 222 } }),
  );
  await tick();
  pending.get("OLDER").reject(new Error("舊請求晚到的錯誤"));
  await tick();
  check(
    "較舊的錯誤不能清掉新報告",
    !page.node("[data-report]").hidden &&
      page.node("[data-corpus-meta]").textContent.includes("222 events") &&
      !page.node("[data-status]").textContent.includes("舊請求"),
    page.text(),
  );
}

if (failed > 0) {
  console.error(`\n${failed} 項失敗。`);
  process.exit(1);
}
console.log("\nmetrics frontend contract 全部通過。");
