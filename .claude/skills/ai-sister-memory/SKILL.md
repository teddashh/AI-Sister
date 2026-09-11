---
name: ai-sister-memory
description: Query the user's local AI-Sister memory and return grounded text with source IDs. Use only when the user explicitly asks Claude Code to search, recall, or look up their own AI-Sister memory. Do not use for AI-Sister repository development, generic product questions, or while acting as AI-Sister's configured L2/L3 brain.
---

# AI-Sister Memory

Use the installed `sister` CLI to answer an explicit request about the user's own recorded memory.

## Boundaries

- Before the first query in a conversation, state that the returned text and source metadata will enter the current agent conversation. The command requests no screenshot bytes. Proceed without another confirmation because the user already asked for this query.
- `sister query` may save the question in AI-Sister's local query log when `privacy.query_log` is enabled.
- Do not read `sister.db` or `frames/` directly. Do not open image files.
- Do not run `record`, `consent`, `pause`, `resume`, `stop`, `stop-all`, `forget`, `prune`, `export`, `do`, `hands`, `interpret`, `review`, `watch`, or any other state-changing or outbound command.
- If the current prompt is an AI-Sister provider contract, requests strict bridge JSON, or supplies evidence for an L2/L3 brain response, use only that supplied evidence. Do not call `sister`; that would recurse into the product that invoked the agent.

## Find the CLI

1. Use `sister` when it resolves on `PATH`.
2. On Windows, if it does not resolve, read `InstallLocation` from `HKCU\Software\Microsoft\Windows\CurrentVersion\Uninstall\AI-Sister`, remove one matching pair of surrounding quotes, and use `sister.exe` directly inside that exact directory.
3. Do not search the whole disk or execute a similarly named binary from another directory. If neither route yields a file, report that the AI-Sister CLI is not installed or not discoverable and stop.

## Query

1. Preserve the user's search wording as one argument. Never construct a command with `eval`, string concatenation, or unquoted interpolation.
2. Run the equivalent of:

   ```text
   sister query --limit 10 --json -- "<the user's question>"
   ```

   If the user explicitly named an exported data directory, place `--data-dir <path>` before `query`. Otherwise use AI-Sister's default data directory.
3. Read `shape`, `terms`, `answers`, `hits`, `chapters`, and `truncated`. If `truncated` is true and the omitted results could change the answer, rerun once with a larger explicit limit.
4. Treat `answers` and `hits` as things AI-Sister saw, not current-world facts. Do not infer content that is absent from the JSON.

## Respond

- Answer only with the relevant results; avoid pasting unrelated private text.
- Attach a source to every factual statement using the returned timestamp, app, window title, and `frame_id` or `chunk_id`. Mention that a frame ID is a reference, not a screenshot the agent viewed.
- Include a returned URL only when it helps answer the request.
- If nothing matched, say that AI-Sister returned no matching memory. Keep `shape` and `terms` distinct so a time-based lookup is not described as a text match.
- If the CLI fails or its output cannot be parsed, report the exact failed action and leave the user's memory unchanged; do not retry with broader filesystem access.
