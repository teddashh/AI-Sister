#!/usr/bin/env node
/*
 * Azure 朗讀跨四層，任何一層各自正確都不代表湊起來是真的：
 *
 *   trusted answer click → native config/consent/credential gates
 *   → 唯一 typed helper → sister-tts fixed POST → bounded MP3
 *
 * renderer 行為（正文 allowlist、無 autoplay/fallback、取消丟晚回應）由
 * check-pet-says-why.mjs 直接載入 app.js 驗；設定控制與 key DOM 壽命由
 * check-settings-say.mjs 直接載入 settings.js 驗。這支補兩份 JS 無法執行的
 * Windows/Tauri 接線：typed permit、gate 順序、單一 in-flight、secret projection。
 */

import { readFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const read = (path) => readFileSync(join(ROOT, path), "utf8");

const MAIN = read("apps/desktop/src-tauri/src/main.rs");
const CREDENTIAL = read("apps/desktop/src-tauri/src/azure_credential.rs");
const CONFIG = read("crates/sister-core/src/config.rs");
const CONSENT = read("crates/sister-core/src/consent.rs");
const APP = read("apps/desktop/ui/app.js");
const TTS = read("crates/sister-tts/src/lib.rs");
const NATIVE = read("crates/sister-tts/src/native.rs");
const TTS_MANIFEST = read("crates/sister-tts/Cargo.toml");

let failed = 0;
function check(name, ok, detail) {
  console.log(`  ${ok ? "✔" : "✗"} ${name}`);
  if (!ok) {
    failed += 1;
    if (detail !== undefined) console.log(`      實際：${JSON.stringify(detail)}`);
  }
}

function section(source, startNeedle, endNeedle) {
  const start = source.indexOf(startNeedle);
  const end = source.indexOf(endNeedle, start + startNeedle.length);
  return start >= 0 && end > start ? source.slice(start, end) : "";
}

function ordered(source, needles) {
  let after = -1;
  const positions = [];
  for (const needle of needles) {
    const at = source.indexOf(needle, after + 1);
    positions.push([needle, at]);
    if (at < 0) return { ok: false, positions };
    after = at;
  }
  return { ok: true, positions };
}

function structFields(source, name) {
  const body = section(source, `struct ${name} {`, "\n}");
  return [...body.matchAll(/^\s*(?:pub\s+)?([a-z][a-z0-9_]*)\s*:/gm)].map((match) => match[1]);
}

function exactFields(source, name, wanted) {
  const actual = structFields(source, name);
  return { ok: JSON.stringify(actual) === JSON.stringify(wanted), actual };
}

console.log("① default graph 沒有 HTTP；azure 才能編入唯一 ureq transport");
{
  const defaultFeature = TTS_MANIFEST.match(/^default\s*=\s*(\[[^\n]*\])$/m)?.[1];
  const azureFeature = TTS_MANIFEST.match(/^azure\s*=\s*(\[[^\n]*\])$/m)?.[1];
  check("default feature 精確為空", defaultFeature === "[]", defaultFeature);
  check("azure feature 只打開 ureq", azureFeature === '["dep:ureq"]', azureFeature);
  check(
    "ureq optional 且繼承 workspace pin",
    /^ureq\s*=\s*\{ workspace = true, optional = true \}$/m.test(TTS_MANIFEST),
  );
  check(
    "native request 只能 POST typed endpoint",
    NATIVE.includes("ureq::http::Request::post(request.endpoint())") &&
      !/Request::(?:get|put|patch|delete)\s*\(/.test(NATIVE),
  );
  check(
    "native agent 關 redirect/proxy 且所有 timeout 有界",
    NATIVE.includes(".https_only(true)") &&
      NATIVE.includes(".max_redirects(0)") &&
      NATIVE.includes(".proxy(None)") &&
      [
        "timeout_resolve",
        "timeout_connect",
        "timeout_send_request",
        "timeout_send_body",
        "timeout_recv_response",
        "timeout_recv_body",
        "timeout_global",
      ].every((name) => NATIVE.includes(`.${name}(Some(`)),
  );
  check(
    "transport 不藏 retry，status/body 交回共同 validator",
    (NATIVE.match(/\.agent\s*\n?\s*\.run\(/g) ?? []).length === 1 &&
      !/\bretry\s*\(/i.test(NATIVE) &&
      TTS.includes("validate_response(response, request.endpoint(), MAX_AUDIO_BYTES)"),
  );
}

console.log("② 唯一 outbound command 重讀完整 gate snapshot，再進 shared-consent typed helper");
{
  const speak = section(
    MAIN,
    "async fn azure_tts_speak(",
    "#[cfg(test)]\nmod azure_tts_mapping_tests",
  );
  const signature = speak.slice(0, speak.indexOf(") ->"));
  check(
    "renderer 只提交 text 與 typed expected snapshot，不能提交 key/permit/host",
    /text:\s*String/.test(signature) &&
      /expected:\s*AzureTtsExpected/.test(signature) &&
      /shell:\s*tauri::State/.test(signature) &&
      !/key|permit|host/i.test(signature),
    signature,
  );
  const gates = ordered(speak, [
    "if shell.azure_tts_generation.load(Ordering::Acquire) != expected.generation",
    "let _admission = azure_tts_admission(&shell)",
    "if shell.azure_tts_generation.load(Ordering::Acquire) != expected.generation",
    "sister_core::config::Config::load",
    "expected.enabled != azure.enabled",
    "if !azure.enabled",
    ".region",
    "sister_tts::build_ssml",
    "sister_core::consent::begin_azure_tts_admission",
    "expected.consent_at != Some(guard.signed_at())",
    "azure_credential::read()",
    "expected.credential_present != credential_present",
    "let request_generation = next_azure_tts_generation(expected.generation)",
    "let _transition = azure_tts_transition(&shell)",
    ".azure_tts_in_flight",
    ".compare_exchange(false, true",
    ".azure_tts_active_generation",
    ".store(expected.generation, Ordering::Release)",
    ".azure_tts_generation",
    ".compare_exchange(",
    "expected.generation,",
    "request_generation,",
    "spawn_blocking",
  ]);
  check(
    "generation 先比對、lock 後再比；config/consent/key/single-flight 全在 POST 前",
    gates.ok,
    gates.positions,
  );
  check(
    "native 逐格比較 renderer 看見的 config、consent timestamp 與 credential state",
    speak.includes("expected.enabled != azure.enabled") &&
      speak.includes("expected.region != azure.region") &&
      speak.includes("expected.voice != azure.voice") &&
      speak.includes("expected.consent_at != Some(guard.signed_at())") &&
      speak.includes("expected.credential_present != credential_present"),
  );

  const closure = section(speak, "spawn_blocking(move || {", "let bytes = task");
  const beforePost = section(closure, "spawn_blocking(move || {", "let bytes = synthesize_azure(");
  const afterPost = section(closure, "let bytes = synthesize_azure(", "Ok(bytes)");
  check(
    "blocking closure 在 POST 前仍比較 generation",
    beforePost.includes("generation_state.load(Ordering::Acquire) != request_generation"),
  );
  check(
    "POST 後再比較一次，取消的晚 response 不會回 renderer",
    afterPost.includes("generation_state.load(Ordering::Acquire) != request_generation") &&
      afterPost.includes("回應已丟掉、沒有播放"),
  );

  const helper = section(MAIN, "fn synthesize_azure(", "struct AzureTtsInFlightGuard");
  check(
    "唯一 helper by-value 吃 shared-lock Azure guard，不收 bool/Copy permit",
    /^fn synthesize_azure\(\s*consent_guard:\s*sister_core::consent::AzureTtsAdmissionGuard,/m.test(
      helper,
    ),
    helper.slice(0, helper.indexOf(") ->")),
  );
  check(
    "helper 是唯一 AzureClient 呼叫點，guard 活過完整 synthesize 才 drop",
    (MAIN.match(/sister_tts::AzureClient::new\(\)/g) ?? []).length === 1 &&
      ordered(helper, [
        "let _permit = consent_guard.permit()",
        "sister_tts::AzureClient::new()",
        ".synthesize(",
        "drop(consent_guard)",
      ]).ok,
  );
  check(
    "command 不再用鎖外 consent::load/Copy permit 越過 CLI revoke",
    speak.includes("begin_azure_tts_admission(data_dir)") &&
      !speak.includes("sister_core::consent::load") &&
      !speak.includes(".azure_tts_permit()"),
  );
  const permitBody = section(
    CONSENT,
    "fn azure_tts_permit(&self)",
    "/// 這份設定跑起來",
  );
  const consentWithoutTypeDeclaration = CONSENT.replace("pub struct AzureTtsAllowed(());", "");
  check(
    "private AzureTtsAllowed 只在第四張 permit body 鑄造一次",
    (permitBody.match(/AzureTtsAllowed\(\(\)\)/g) ?? []).length === 1 &&
      permitBody.includes("self.allows_azure_tts().then_some(AzureTtsAllowed(()))") &&
      (consentWithoutTypeDeclaration.match(/AzureTtsAllowed\(\(\)\)/g) ?? []).length === 1 &&
      !CONSENT.includes("pub fn azure_tts_permit") &&
      !MAIN.includes("AzureTtsAllowed(())"),
    permitBody,
  );
  check(
    "Azure marker 不能 Clone/Copy，guard 只借出 permit",
    /#\[derive\(Debug\)\]\s*pub struct AzureTtsAllowed\(\(\)\);/.test(CONSENT) &&
      /pub const fn permit\(&self\) -> &AzureTtsAllowed/.test(CONSENT) &&
      !/#\[derive\([^\]]*(?:Clone|Copy)[^\]]*\)\]\s*pub struct AzureTtsAllowed/.test(CONSENT),
  );
}

console.log("③ 一次只准一個 POST；cancel/關閉/換區域/刪 key/撤回都推進 generation");
{
  const backend = section(
    MAIN,
    "// ---------- 可選 Azure 雲端朗讀 ----------",
    "/// 設定頁上看得到、改得動的那幾項。",
  );
  const guard = section(backend, "struct AzureTtsInFlightGuard", "/// 唯一 outbound command");
  const cancel = section(backend, "fn azure_tts_cancel(", "fn transport_region(");
  const stopIntent = section(backend, "fn stop_azure_tts_intent(", "#[tauri::command]\nfn azure_tts_read");
  check(
    "single-flight guard 持有 transition，Drop 在同一把鎖內先清 identity 再釋放",
    backend.includes(".compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)") &&
      ordered(guard, [
        "transition: Arc<Mutex<()>>",
        "let _transition = self",
        ".transition",
        ".lock()",
        ".store(AZURE_TTS_NO_ACTIVE_GENERATION, Ordering::Release)",
        "self.in_flight.store(false, Ordering::Release)",
      ]).ok,
  );
  check(
    "cancel 先取 transition，再 CAS 未 admission token/active baseline；延遲 A 不誤殺 B",
    /expected_generation:\s*u64/.test(cancel) &&
      ordered(cancel, [
        "let _transition = azure_tts_transition(&shell)",
        "cancel_azure_tts_generation(",
        "fn cancel_azure_tts_generation(",
        "let next = next_azure_tts_generation(expected_generation)",
        "expected_generation,",
        "next,",
        "active_generation.load(Ordering::Acquire) != expected_generation",
        "let after_cancel = next_azure_tts_generation(next)",
        "next,",
        "after_cancel,",
      ]).ok &&
      !cancel.includes("fetch_add") &&
      !/abort|terminate|kill/.test(cancel),
    cancel,
  );
  check(
    "設定／金鑰變更共用 stop intent 並通知 renderer",
    ordered(stopIntent, [
      "let _transition = azure_tts_transition(shell)",
      ".azure_tts_generation",
      ".fetch_update(",
    ]).ok &&
      stopIntent.includes("next_azure_tts_generation(current)") &&
      stopIntent.includes('emit("azure-tts-stop"') &&
      (backend.match(/stop_azure_tts_intent\(&app, &shell\)/g) ?? []).length >= 3,
  );
  const configSet = section(backend, "fn azure_tts_config_set(", "fn azure_tts_key_set(");
  const keySet = section(backend, "fn azure_tts_key_set(", "fn azure_tts_key_delete(");
  const keyDelete = section(backend, "fn azure_tts_key_delete(", "fn azure_tts_cancel(");
  check(
    "三種 persistent mutation 都在共同 admission lock 內先 bump，再落地",
    [
      [configSet, "sister_core::config::Config::update"],
      [keySet, "azure_credential::write"],
      [keyDelete, "azure_credential::delete"],
    ].every(([body, persist]) =>
      ordered(body, [
        "azure_tts_admission(&shell)",
        "stop_azure_tts_intent(&app, &shell)",
        persist,
      ]).ok,
    ),
  );
  const consentSet = section(MAIN, "fn consent_set(", "fn open_onboarding_window");
  const azureConsentChange = consentSet.match(
    /let\s+([a-z][a-z0-9_]*)\s*=\s*sheet\s*==\s*sister_core::consent::Sheet::AzureTts\s*;/,
  );
  check(
    "第四張 grant/revoke 都先失效舊 click，再用同一把 admission lock 寫 consent",
    azureConsentChange !== null &&
      ordered(consentSet, [
        `${azureConsentChange?.[1]}.then(|| azure_tts_admission(&shell))`,
        "stop_azure_tts_intent(&app, &shell)",
        "sister_core::consent::mutate",
      ]).ok,
    consentSet,
  );
  const speak = section(
    backend,
    "async fn azure_tts_speak(",
    "#[cfg(test)]\nmod azure_tts_mapping_tests",
  );
  check(
    "speak 在 transition 內入場、發布 active baseline 並消耗世代",
    ordered(speak, [
      "let request_generation = next_azure_tts_generation(expected.generation)",
      "let _transition = azure_tts_transition(&shell)",
      ".azure_tts_in_flight",
      ".compare_exchange(false, true",
      ".azure_tts_active_generation",
      ".store(expected.generation, Ordering::Release)",
      ".azure_tts_generation",
      ".compare_exchange(",
      "expected.generation,",
      "request_generation,",
      "spawn_blocking",
      "generation: request_generation",
    ]).ok,
  );
}

console.log("④ key 不進 config/log/renderer projection；secret scope 離開即清零");
{
  const view = exactFields(MAIN, "AzureTtsView", [
    "generation",
    "config_readable",
    "enabled",
    "region",
    "voice",
    "endpoint",
    "credential",
    "consented",
    "consent_at",
    "ready",
    "config_error",
  ]);
  const expected = exactFields(MAIN, "AzureTtsExpected", [
    "generation",
    "enabled",
    "region",
    "voice",
    "consent_at",
    "credential_present",
  ]);
  const audio = exactFields(MAIN, "AzureTtsAudioView", [
    "generation",
    "content_type",
    "audio_bytes",
    "data_url",
  ]);
  const config = exactFields(CONFIG, "AzureTtsConfig", ["enabled", "region", "voice"]);
  check("status DTO 只有非密 projection", view.ok, view.actual);
  check("expected DTO 只有六格 non-secret gate snapshot", expected.ok, expected.actual);
  check(
    "expected 拒絕陌生欄位且只用 camelCase 投影",
    /#\[serde\(rename_all = "camelCase", deny_unknown_fields\)\]\s*struct AzureTtsExpected/.test(
      MAIN,
    ),
  );
  check("speak DTO 只回下一代與 bounded audio metadata/data", audio.ok, audio.actual);
  check("config.toml 只有 enabled/typed region/typed voice", config.ok, config.actual);
  check(
    "key command 只回 AzureTtsView",
    /fn azure_tts_key_set\([\s\S]*?\) -> Result<AzureTtsView, String>/.test(MAIN) &&
      /fn azure_tts_key_delete\([\s\S]*?\) -> Result<AzureTtsView, String>/.test(MAIN),
  );
  check(
    "SecretKey Debug 固定遮蔽，Drop 原位清零",
    CREDENTIAL.includes('formatter.write_str("SecretKey([REDACTED])")') &&
      /impl Drop for SecretKey[\s\S]*?self\.0\.fill\(0\)/.test(CREDENTIAL),
  );
  const azureBackend = section(
    MAIN,
    "// ---------- 可選 Azure 雲端朗讀 ----------",
    "/// 設定頁上看得到、改得動的那幾項。",
  );
  check(
    "Azure/credential 路徑沒有 debug 或 logger sink",
    !/\b(?:dbg|println|eprintln)!\s*\(|\b(?:log|tracing)::/.test(azureBackend + CREDENTIAL),
  );
}

console.log("⑤ app.js 用正文 attribute allowlist；唯一 speak IPC 位於 trusted handler");
{
  const extractor = section(APP, "function azureAnswerText()", "function answerReadLine()");
  const handler = section(APP, "function answerAzureLine()", "/**\n * @param hits");
  const statusParser = section(APP, "function usableAzureTtsStatus(", "function syncAzureAnswerLine(");
  const statusApply = section(APP, "function applyAzureTtsStatus(", "function readAzureTts(");
  const statusRead = section(APP, "function readAzureTts(", "/**\n * 她有沒有一件事想讓人看見");
  const stop = section(APP, "function stopAzureSpeech(", "function stopLocalSpeech(");
  check(
    "Azure extractor 只收 data-azure-answer-body，不做整頁 denylist",
    extractor.includes('querySelectorAll("[data-azure-answer-body]")') &&
      !extractor.includes("cloneNode") &&
      !extractor.includes("hitList.textContent") &&
      !/\.hit-source|\.hits-note|button|\ba\b/.test(extractor),
    extractor,
  );
  const clickOrder = ordered(handler, [
    "event?.isTrusted !== true",
    "if (azureCancelPending)",
    "stopPersonaMedia()",
    "const text = azureAnswerText()",
    "const nativeExpected = azureNativeExpected",
    'invoke("azure_tts_speak", {',
    "expected: nativeExpected",
  ]);
  check(
    "trusted click、cancel latch、停止舊播放、抽正文、完整 expected snapshot 的順序固定",
    clickOrder.ok,
    clickOrder.positions,
  );
  check(
    "status 嚴格驗 generation/endpoint/consent timestamp/ready，再投影 click snapshot",
    statusParser.includes("Number.isSafeInteger(raw.generation)") &&
      /raw\.generation\s*(?:<|>=)\s*0/.test(statusParser) &&
      statusParser.includes("raw.endpoint ===") &&
      statusParser.includes("raw.consent_at") &&
      statusParser.includes("raw.config_error === null") &&
      statusParser.includes("raw.ready ===") &&
      statusApply.includes("consentAt: status.consent_at") &&
      statusApply.includes('credentialPresent: status.credential === "present"'),
    { statusParser, statusApply },
  );
  check(
    "stop 只把 active generation 傳給 scoped cancel，沒有零參數全域 bump",
    /invoke\("azure_tts_cancel",\s*\{\s*expectedGeneration:/.test(stop) &&
      !/invoke\("azure_tts_cancel"\s*\)/.test(stop),
    stop,
  );
  check(
    "cancel pending 會作廢 read、阻止連按，settle 後才重讀",
    stop.includes("azureCancelPending = true") &&
      stop.includes("azureStatusReadRevision += 1") &&
      stop.includes("azureCancelPending = false") &&
      statusApply.includes("if (azureCancelPending)") &&
      statusRead.includes("if (azureCancelPending)") &&
      handler.includes("if (azureCancelPending)"),
  );
  check(
    "audio 只接受 baseline 精確 +1，才更新下一個 click token",
    handler.includes("audio.generation !==") &&
      handler.includes("nativeGeneration + 1") &&
      ordered(handler, [
        "audio.generation !==",
        "azureNativeGeneration = audio.generation",
        "generation: audio.generation",
      ]).ok,
  );
  check(
    "整份 renderer 只有這一個 Azure speak IPC，不存在 boot/event autoplay",
    (APP.match(/invoke\("azure_tts_speak"/g) ?? []).length === 1,
  );
  check(
    "Azure 失敗路徑不呼叫 localService fallback",
    !handler.includes("speakWithLocalSystemVoice") &&
      handler.includes("我沒有自動改用本機或另一個雲端"),
  );
}

console.log("");
if (failed > 0) {
  console.log(`✗ ${failed} 條沒過——Azure 朗讀的 typed gate、secret 或單次 click 邊界已分岔。`);
  process.exit(1);
}
console.log("✓ Azure 朗讀只從 trusted answer click 穿過四道 native gate與唯一 fixed POST；key 不回 renderer/config/log，取消會丟晚 response，且一次只准一個 in-flight");
