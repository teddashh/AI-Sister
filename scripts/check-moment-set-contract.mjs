#!/usr/bin/env node
/*
 * 承諾集／開口判斷集守的是四句寫在文件裡的話：
 *
 * 1. `docs/PHASES.md` 說這是評測資料，產品仍然不主動開口——所以 `moments` 不可以
 *    被錄製或回答路徑引用。
 * 2. `AGENTS.md` 第二節說「兩種零長得一樣」是這個 repo 落地 40 次的錯——所以
 *    「還沒標」必須是 `label: Option`，而且 status 要把未標單獨印出來。
 * 3. `docs/PHASES.md:226` 說提醒誤報／漏報要等真的量測來源——所以沒有守門員以前
 *    不准把那兩格填成數字。
 * 4. commit 訊息與 PHASES.md 都說「通知類提不出候選，因為 L0 沒有通知事件」——
 *    所以那個判斷必須逐一列出每個 SystemKind，不可以用萬用臂把未來的新 kind
 *    悄悄吞成 false，也不可以改用 OCR 關鍵字冒充通知訊號。
 */

import { readFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const read = (p) => readFileSync(join(ROOT, p), "utf8");

const MOMENTS = read("crates/sister-core/src/moments.rs");
const OPS = read("crates/sister-cli/src/ops.rs");
const RECORDER = read("crates/sister-capture/src/recorder.rs");
const ANSWER = read("crates/sister-core/src/answer.rs");
const MODEL = read("crates/sister-core/src/model.rs");

const problems = [];
const check = (ok, msg) => {
  if (!ok) problems.push(msg);
};

// 1. 評測資料不可以接到會執行的路徑。
for (const [name, src] of [
  ["recorder.rs", RECORDER],
  ["answer.rs", ANSWER],
]) {
  check(
    !/\bmoments::|\bMomentSet\b|\bLabeledMoment\b/.test(src),
    `${name} 引用了 moments：這是評測資料，不可以接到錄製或回答路徑（產品仍然不主動開口）`,
  );
}

// 2. 「還沒標」必須是 None，不可以是某個變體兼差。
check(
  /label:\s*Option<MomentLabel>/.test(MOMENTS),
  "LabeledMoment.label 不再是 Option<MomentLabel>：未標會跟某個真標籤長成同一個東西",
);
check(
  !/\bUnlabeled\b|\bNotLabeled\b|\bUnknown\b/.test(MOMENTS),
  "MomentLabel 出現了「未標」變體：未標只能用 None 表示",
);
check(
  /pub\s+unlabeled:\s*usize/.test(MOMENTS),
  "MomentSetCounts 少了 unlabeled：「標完了但沒有該講的」跟「還沒標」會印成同一個 0",
);
// status 必須把未標跟已標該講印在不同行，否則兩種 0 又混在一起。
const statusFn = OPS.match(/fn render_moment_status[\s\S]*?\n    \}/);
check(statusFn !== null, "ops.rs 找不到 render_moment_status");
if (statusFn) {
  check(
    /未標/.test(statusFn[0]) && /該講/.test(statusFn[0]),
    "moment status 沒有同時印出「未標」和「該講」：兩種 0 又分不出來了",
  );
}

// 3. 沒有守門員以前，提醒誤報／漏報只能是 null。
for (const [name, src] of [
  ["moments.rs", MOMENTS],
  ["ops.rs", OPS],
]) {
  check(
    !/reminder_(false_positive|miss)_rate\s*:\s*(Some\(|[0-9])/.test(src),
    `${name} 把 reminder_*_rate 填成了數字：沒有守門員可量，只能是 null`,
  );
}

// 4. 通知判斷必須逐一列出 SystemKind，且現行每一種都不是通知。
const notifFn = MOMENTS.match(
  /fn system_kind_is_notification[\s\S]*?\n\}/,
);
check(notifFn !== null, "moments.rs 找不到 system_kind_is_notification");
if (notifFn) {
  const body = notifFn[0];
  check(
    !/_\s*=>/.test(body),
    "system_kind_is_notification 用了萬用臂：將來新增的 SystemKind 會被悄悄吞成「不是通知」，" +
      "而不是編不過逼人決定",
  );
  check(
    !/\btrue\b/.test(body),
    "system_kind_is_notification 有分支回 true：現行 L0 沒有通知事件，回 true 就是在說謊",
  );
  // 每一個 SystemKind 都要被列到，少列一種就等於萬用臂。
  const kinds = [
    ...MODEL.match(/pub enum SystemKind\s*\{[\s\S]*?\n\}/)[0].matchAll(
      /^\s{4}([A-Z]\w+),/gm,
    ),
  ].map((m) => m[1]);
  check(kinds.length > 0, "抓不到 SystemKind 的變體清單");
  for (const kind of kinds) {
    check(
      body.includes(`SystemKind::${kind}`),
      `system_kind_is_notification 沒有列到 SystemKind::${kind}`,
    );
  }
}
// 不可以改用 OCR／detail 關鍵字冒充通知訊號（recorder 的 detail 只寫得出
// SessionEnd 與 Excluded 的理由字串，比對關鍵字就是宣布一件沒查過的事）。
check(
  !/(contains|starts_with|ends_with)\(\s*"(notification|notify|toast|banner|通知)"/i.test(
    MOMENTS.replace(/#\[cfg\(test\)\][\s\S]*$/, ""),
  ),
  "moments.rs 拿字串比對冒充通知訊號：detail 只有 SessionEnd／Excluded 的理由字串，" +
    "比對關鍵字等於宣布一件程式沒查過的事",
);

if (problems.length > 0) {
  console.error("✗ 承諾集／開口判斷集的契約破了：");
  for (const p of problems) console.error(`  - ${p}`);
  process.exit(1);
}
console.log(
  "✓ moments 沒接到執行路徑、未標與零分得開、提醒率仍是 null、通知類逐一列舉且全部不是通知",
);
