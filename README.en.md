# AI-Sister

[繁體中文](README.md) · **English**

> A sister who stands in the corner of your desktop. She is always there, sees your day, and remembers
> the details. She stays quiet 95% of the time and speaks only when it's time to, and every sentence
> she says opens to its evidence.
>
> An open-source, local-first desktop companion: a filing cabinet that never
> forgets, an event-driven brain that can admit it's wrong, and a desktop sister
> who knows when to stay quiet. Screen pixels never leave your machine; after
> explicit opt-in, your selected CLI receives selected memory text for questions and background understanding. A
> separate, default-off Azure TTS option can send only each newly completed
> answer body after its own consent; manual replay sends it again. Local speech remains the default.

**Project page:** https://teddashh.github.io/AI-Sister/en/ · Traditional Chinese: https://teddashh.github.io/AI-Sister/

**Status: v0.1.0-alpha.170**

The app itself (interface, consent sheets, character voices) is in Traditional Chinese and Mandarin.
This file is an English translation of [README.md](README.md). Where the app shows Chinese text, the
Chinese is quoted with an English gloss.

AI-Sister already does local recording, OCR, L0 to L3 memory, local RAG, a clickable source for every
sentence, and sign-in for four brains: Claude Code, Codex, Gemini CLI, and Grok CLI. There are 17
characters, four sisters and 13 friends; the character art, the workplace rigs, and each character's
8 base plus 24 expansion voice lines all install with the app.

Once you pick and sign in to a CLI and accept the second consent, **every typed question** first goes
to that CLI, which decides which memories to look up. AI-Sister runs up to three queries locally, then
hands the matching text and its sources back to the same CLI to answer. The CLI never gets the SQLite
path or the whole database, and a question with no hits still goes to the CLI. Every sentence in an
answer must still cite a local source that was actually found in that round. While recording,
background understanding also checks working hypotheses segment by segment in time order; after the
alpha.143 upgrade it backfills in batches starting from the oldest records still kept, saves its
progress across restarts, and always handles current activity first.

The main desktop window is a transparent, full-body desktop pet: the character doesn't resize when an
answer appears, and the answer and its clickable evidence sit in a single speech bubble pointing at
her, with no cream-colored frame around the window. On first launch, the four consent sheets are also
asked one by one in that bubble; you answer 「同意」 (agree) or 「不同意」 (decline) in the normal input
box, and afterwards you can still review or change them in Settings, from the gear at the top. If a
question was blocked by the second sheet because its wording changed, the original question is kept
and resent automatically to the currently selected CLI once you have answered.

- Windows 10+: Setup, the desktop app, the always-on recorder, and full S1 (the core recall loop).
- Ubuntu 24.04 X11: `.deb`, native X11 capture, AT-SPI privacy context, local Tesseract, and full S1.
- macOS 14+: ScreenCaptureKit, Vision OCR, AX privacy context, and the TCC settings UI are wired into
  the product build; a public download joins the Release after Developer ID notarization and native
  TCC/S1 acceptance are done.

Download from the [project page](https://teddashh.github.io/AI-Sister/en/) or
[Releases](https://github.com/teddashh/AI-Sister/releases).

## Claude Code / Codex Skill

The repo has two official entry points for the same AI-Sister memory lookup skill:

- [Claude Code skill](.claude/skills/ai-sister-memory/SKILL.md): the project path is
  `.claude/skills/ai-sister-memory/`, the personal install path is
  `~/.claude/skills/ai-sister-memory/`, and you call it with `/ai-sister-memory`.
- [Codex skill](.agents/skills/ai-sister-memory/SKILL.md): the project path is
  `.agents/skills/ai-sister-memory/`, the personal install path is
  `~/.agents/skills/ai-sister-memory/`, and you call it with `$ai-sister-memory`.

To install both from a cloned repo into your current account, run this in PowerShell:

```powershell
New-Item -ItemType Directory -Force "$HOME\.claude\skills", "$HOME\.agents\skills"
Copy-Item -Recurse -Force .\.claude\skills\ai-sister-memory "$HOME\.claude\skills\"
Copy-Item -Recurse -Force .\.agents\skills\ai-sister-memory "$HOME\.agents\skills\"
```

Both entry points run `sister query --json` only when you explicitly ask to search your own AI-Sister
memory, and they bring back the time, app, window title, and frame/chunk source with each hit. They
won't sign consents for you, start recording, delete memories, or run hands. The text and source
fields a query returns enter the current Claude Code/Codex conversation, but the skill never reads
screenshot bytes. When `privacy.query_log` is on, AI-Sister still records the question locally. If the
agent is itself being called by AI-Sister as the L2/L3 brain, the skill explicitly forbids running
`sister` back, so there is no recursion.

Before she starts watching, or hands answer text to Azure, there are **four separate consent sheets,
each revocable at any time**. The app shows the wording in Chinese (the exact text lives in
`crates/sister-core/src/consent.rs` and in [README.md](README.md)); here is what each one says and does:

- `local-recording`: "I agree to record my screen on my hard drive." Without it, `sister record`
  won't start. If you revoke it mid-recording, the running recorder rereads the consent file every
  5 seconds, so it records at most 5 more seconds plus one tick; when `capture.min_interval_ms` is
  longer than 5 seconds, most of that wait is the tick. Unsigned, it refuses to start and exits
  non-zero; signed, it may record locally.
- `cloud-reading`: "I agree to let the CLI selected in Settings interpret my local memory. When I ask
  a question, it hands over the question and the text the lookup matched; explanation, review,
  supervision, and background understanding during recording automatically hand over selected
  excerpts, including a one-time reread of older records after an upgrade. The content may include raw
  screen text, facts the program extracted, existing working hypotheses, times, apps, window titles,
  and URLs, and it may call the CLI repeatedly and use my CLI plan's quota. It never sends frame
  files, the database path, or the whole database; whatever is in the text is sent as is, with nothing
  masked first."
  Without it, questions, local memory text, and working hypotheses are never handed to a CLI;
  background explanation, review, supervision, and the reread of old memories don't call it either,
  and you can still view raw search results locally. Screen images never leave this machine; what
  goes out is the selected text described above, **raw and unmasked**. The CLI never gets the database
  path, the whole database, or screenshot bytes.
- `frame-storage`: "I agree to keep screenshots of changed frames, not just the text on them." Not
  signing doesn't block recording: she tells you on the spot that she is running degraded, records
  text only, and writes no screenshots at all. Signed, she may keep changed frames as configured.
- `azure-tts`: "I agree that when Azure auto-read for new answers is turned on in Settings, each new
  answer, once complete, is sent without asking me each time: its raw body goes to the Microsoft Azure
  Speech service in the region I chose in Settings and plays automatically. The body may contain
  names, phone numbers, and amounts and is not masked first; screenshots, source links, memory ids,
  the whole database, and any other text are not sent." Without it, she never calls the Azure Speech
  service, not once; local read-aloud is unaffected.

For example, to let her record text locally and keep screenshots, you write
`sister consent --grant local-recording --grant frame-storage`. Three interfaces (the main desktop
chat, the full set of four cards in Settings, and `sister consent`) all take the same wording and the
same consequences of not signing from core; `sister doctor` reads the same file and also reports
whether each sheet is signed right now and what will happen. Answering 「不同意」 (decline) keeps the
feature off, but it also remembers that this sheet has been asked, so it won't keep asking on every
launch. When the wording changes, only the sheet that actually changed is asked again. alpha.143
widened the second sheet's wording to cover background understanding and a one-time reread of old
memories, so old second-sheet signatures became invalid and only that sheet is asked again; the
first, third, and fourth sheets stay as they were.
A shared wording revision of the first three sheets invalidates all three old signatures; the second
and fourth sheets also each have their own wording version. If the file can't be read, is corrupted,
or has a mismatched version, everything fails closed. Old files from before alpha.109 without an
Azure field keep the first three sheets and leave the fourth unsigned; the per-click wording signed
in alpha.109 also shows as expired in alpha.110, and only the fourth sheet needs re-signing. No old
consent can ever be read as authorizing auto-read. When the CLI is given `--data-dir`, the consent
file follows that folder; the desktop sister only reads the default folder, so the two are not
necessarily the same file.

**alpha.46 was measured on Ted's real Windows machine at 1920×1080, over 60 seconds of normal coding
while switching Better Agent workspaces: CPU averaged 44.0% and RAM peaked at 73.7MB.** On 2026-08-23
Ted chose to keep the current observation density; CPU is still measured honestly in every session,
but it is no longer a Phase 0 blocker. `<3%` stays as a long-term product target and isn't used to
overturn this accepted Phase 0 baseline. RAM has passed `<400MB`. Disk is still open, but alpha.47
broke the gap down: those 60 seconds wrote 2.7MB of frames, and 2.8MB of the 2.9MB that the summary
called "other" was reusable SQLite WAL; SQLite's logical allocation grew by only 156KB. The 4.3GB/day
the summary printed at the time (4.1GB/day of it "other") treated the WAL working file as if it grew
permanently every day, so it **is not used as the Phase 0 verdict**; the accounting on main now
extrapolates only from SQLite logical allocation and frames. Capacity is still above the long-term
target, but Ted decided to finish product features and experience first and come back to optimize
capacity later; it is published as is and no longer blocks the current feature milestone.
Details and next steps are in [`docs/WINDOWS-CHECKLIST.md`](docs/WINDOWS-CHECKLIST.md).

The bottom layer watches the screen, reads the text, pulls out facts such as phone numbers and
amounts, stores them in SQLite, and finds them again; **this layer makes no model calls at all**, it is
all the program transcribing. The L2/L3 Interpreter, Reviewer, commitment table, and Gatekeeper, plus
hands that only do allowlisted actions, are also connected; any part that hands OCR text to a model
still requires the second consent and a CLI you set up yourself, and without them it stays pure local
retrieval.

Since alpha.106, the Windows release contract has one default entry point and two no-install or
diagnostic fallback files:

| | What it does |
|---|---|
| `AI-Sister-Setup.exe` | **Most people should download this one.** The current-user NSIS installer carries the exact `sister.exe` sidecar and the WebView2 offline installer; installing needs no network, and the installer's size in bytes is listed on that version's Release asset |
| `sister.exe` | Recording, search, replay evaluation, and data management; it also includes `interpret`, `review`, and `watch`, the Gatekeeper's `speak`, and the `do`, `hands`, and `url-policy` entry points for actions and auditing |
| `sister-desktop.exe` | The sister in the corner of your desktop: recording status, search with clickable sources, the timeline, and deletion; it also shows the current guess, Gatekeeper and hands suggestions, and actively asks about the unattended URL policy |

The installer has no built-in auto-update. To upgrade, you download the new `AI-Sister-Setup.exe` and
install it in place. If a desktop app or recorder from alpha.113 or later is still running, the GUI
Setup stops at Retry/Cancel and tells you to choose 「結束（記錄也會停）」 ("Quit (recording stops too)")
in the system tray menu (it reads just 「結束」 when nothing is recording) or to stop the command in
its terminal. Once the process has exited, pressing Retry measures the product lock again, so you
don't need to close and reopen Setup, and it never force-kills the recorder; silent `/S` still exits
with 32. alpha.115 uses a pinned tauri-bundler 2.9.4 custom NSIS template; Setup never runs the
installed NSIS uninstaller nested inside itself. For a same-version repair or an upgrade from an older
version, it takes the lifecycle mutex in `.onInit`, binds to the exact root recorded in the
current-user product key, and overwrites in place. GUI, passive `/P`, and silent `/S` all re-verify
`DisplayVersion`, the root, and the quoted `UninstallString` before touching WebView2, program files,
or install registration; if anything doesn't match, it stops first. If this Setup is older than the
installed version, close Setup and remove the current version separately from Windows "Installed apps".

The direct uninstaller takes the mutex at `PREUNINSTALL`, after the confirmation page and before any
removal, then re-verifies the exact version, root, and uninstall string; if a newer version was
installed over it after you confirmed, it refuses, so a stale confirmation can't delete the newer
version. The Setup and uninstaller built by this version still override Tauri's stock running-app
macro; when the current-user scanner **reports a match**, they only refuse, and never offer or perform
a kill. A refusal releases the mutex first; a cancel is closed by process teardown. An unrelated
second Setup can't enter the lifecycle at the same time.

alpha.113-aware desktop and CLI builds run a mutex → product event → mutex handshake before touching
product logs, the DB, WebView, memory, or settings, and hold the event until the process exits; after
the installer takes the mutex, it continues only if it explicitly measures that the event doesn't
exist. Old binaries have no event, and the legacy scanner has no Unknown, so alpha.117 adds a
file-level exclusive open after the last scan and before the NSIS `File` step: if any process is
running the installed `sister-desktop.exe` or `sister.exe`, the open fails with a sharing violation,
and Setup or the uninstaller refuses without killing anything. Once it has the handle, the old file is
renamed first and the handle is held until the end, so during that window the Windows loader can't
map any version of the exe. Old uninstallers that already shipped or were copied to temp still can't be
changed retroactively by a newer version. Windows signing and release rules are in
[`docs/WINDOWS-CODE-SIGNING.md`](docs/WINDOWS-CODE-SIGNING.md); the current public alphas are not
signed yet.

## Run it

**Windows 10 or later**: from
[Releases](https://github.com/teddashh/AI-Sister/releases), download
`AI-Sister-Setup.exe` first. If you only want the CLI or need to diagnose an installer problem, you
can still download the two no-install executables and put them in the same folder (the desktop sister
looks for `sister.exe` next to itself). She won't do anything until she has the first consent:

```
sister consent --grant local-recording
sister doctor
sister record --duration 60
sister stop-all                 # stops all three layers; never resumes on its own
sister stop-all --off           # lifts the full stop; leaves an existing pause or unplugged hands alone
sister diagnose                 # when something breaks, writes it up as text you can paste in full
```

With the installed version you can just open AI-Sister; for the no-install route, open
`sister-desktop.exe`. Then ask her what happened in that last minute, ask 「她知道了什麼」 ("what does
she know") on the desktop to see the local L2 she has organized, or run
`sister query 電話` (電話 means "phone").
`doctor` comes before recording on purpose: it shows on the spot whether this machine can read URLs
**right now**, whether OCR is installed, and which exclusion rules don't actually take effect. That
beats finding out after 60 seconds of recording that nothing got in.

On Windows, opening `sister-desktop.exe` a second time only asks the existing desktop sister to show
itself and take focus; it doesn't open another copy, and it doesn't stop or restart the recorder. The
show and focus behavior itself is still on the real-machine manual checklist below.

alpha.107's installed-version Settings page also has an **immediate, default-off** switch,
「登入 Windows 後在背景啟動 AI-Sister」 ("start AI-Sister in the background when I sign in to Windows").
The only source of truth is the current user's
`HKCU\Software\Microsoft\Windows\CurrentVersion\Run`: a missing value is `disabled`, and only
`"<the currently installed sister-desktop.exe>" --ai-sister-login` matching exactly is `enabled`; an
old path, a missing argument, or a non-string value is `mismatch`, a value that can't be read is
`unreadable`, and a portable or non-exact installed copy is `unsupported`. None of the last three
pretend to be off. It may only change the value when the uninstaller metadata's
`InstallLocation` exactly matches the current exe's parent folder; a portable copy only shows the
status and never touches the installed version's value. This only guarantees that the Run value is
registered; Windows `StartupApproved` can still disable it separately under "Startup apps". Turning
the switch off only affects later sign-ins and doesn't stop the current recorder.

When launched with `--ai-sister-login`, it stays in the system tray only: it doesn't show or focus the
desktop sister and doesn't pop up the consent sheets. A single desktop worker only accepts the first
login intent; later delayed or secondary duplicates are ignored and don't reveal the window either. If
the first (local recording) consent is valid, it asks to start the recorder that the desktop owns
itself; if that consent is unsigned, unreadable, or out of date, it doesn't start. An existing pause is
always kept, and neither sign-in nor retries can lift it for you; they also can't clear an earlier
Stop or the `consent-revoked` latch.

The login's transient preflight rechecks from scratch every 500ms, for at most five minutes, and
doesn't use up the watchdog's 1/5/30-second failure budget. Only a typed `desktop-quit` may wait,
within that deadline, for the previous run's lease and heartbeat to leave and then pass the full
barrier; if the marker is Absent but any owner evidence is visible, that is External, and even if it
stops later, this login intent won't take it over or restart it. For an ordinary external owner, a
stale last tick or a temporary read failure still doesn't count as having left; only a `stopped`
tombstone can prove departure. `desktop-quit` is a narrow exception left by the previous run's normal
exit, not a general rule that mere staleness means an empty room. The five-minute deadline is
hard-checked before every retry and before the final commit, so it can't sleep past the deadline and
spawn late. A real person's Stop, Quit, or Explicit Start, `requested` or `consent-revoked`, and
consent or control state that is invalid or can't be read safely all cancel this Login intent, and a
failed barrier doesn't clear the marker.

A normal exit leaves a weaker `desktop-quit`; the next brand-new opt-in Windows sign-in may clear only
this kind, and only after the bounded handoff above and the full consent, stop, lease, and heartbeat
barrier, so a normal shutdown doesn't permanently shut off launching at sign-in.
When you revoke the local recording consent, the program publishes a separate
`consent-revoke.barrier` inside the same exclusive `consent.lock` transaction, **before** it writes
`consent.toml`. It isn't a stop marker that Start can clear; even if saving the consent file fails and
the old consent stays valid, this barrier keeps blocking starts. Only a successfully committed local
recording regrant can use a generation ticket to clear the matching old barrier; if there is no other
stop before clearing, it first leaves a `consent-revoked` stop latch, and an existing `requested` or
`desktop-quit` is left alone, so re-signing only re-authorizes: it never quietly restarts recording or
swallows a real person's Stop.

Every recorder start takes its locks in a fixed order: shared `consent.lock` → `stop.lock` →
`recording.lock` or the old heartbeat barrier. The CLI recorder's shared guard lives through the
explicit clear and the first tick; the desktop parent's guard lives from preflight and clear until
`Command::spawn` returns and is released right away, without waiting for the child's heartbeat, and
the child takes its own guard, nonblocking, and holds it until the first tick. If the revoke writer
gets the exclusive consent lock first, that start doesn't clear the marker, doesn't spawn, and doesn't
write a heartbeat; if the start gets the shared guard first, it is explicitly ordered before the
revoke, and a late revoke is still wound down by the barrier and the recorder's own checks, so
neither side can slip past the other on an old snapshot taken outside the lock.

If a desktop-owned recorder fails abnormally, the first three consecutive retries wait 1 second,
5 seconds, and 30 seconds; after the fourth failure it gives up. The ten-minute health window only
advances on **a `Recording` heartbeat that is actually newer than the last observed sample and still
fresh**; reading the same tick again isn't new evidence, and once missing, stalled, unreadable, or
`Thinking` is observed, the continuous interval resets to zero. Exit 0, a manual stop, revoking the
first consent, and quitting the desktop all cancel retries. The moment a real person presses Stop, the
worker cancels Login and watchdog automation before any durable write that might fail; if the write
fails, it says honestly that the existing recorder may still be running, but it won't revive
automatically afterwards either. Only the next real Explicit Start that **commits successfully** lifts
this in-memory latch, and a failed Start doesn't count. Quit also writes a durable stop first; if that
write fails, it still cancels in-memory retries immediately, but the desktop doesn't exit: it shows
the error and waits for you to fix it, instead of leaving behind a recorder that can't be proven to
stop. When the recorder slot is taken or its state is unknown, it fails closed and doesn't start a
second one; a recorder started externally is never taken over, and there is no self-relaunch service
if the desktop itself crashes.
Also, every CLI or desktop recorder must take an OS whole-file exclusive lock on the empty
`recording.lock` file in the data dir, nonblocking, before its first heartbeat tick, and hold it for
the whole session; if two start at once, only one succeeds. A process crash or a dropped handle makes
the OS release the lock, but the empty file itself can persist, so "the file exists" can't be taken to
mean occupied; a symlink or non-regular path fails closed. When a real person starts from the desktop,
the parent first holds the shared consent start guard, then within the same stop transaction briefly
takes this lease and checks the old heartbeat, and only clears the old latch after commit; the child
still reacquires the formal lease in supervised mode and rereads consent and stop state before the
first tick, so a Stop or Quit that arrives a bit later isn't wiped out by the child.
The exact command and five states, the watchdog, the recorder lease, and the consent transaction all
have automated tests; the manual check of the official alpha.107 installer's real Windows sign-in and
failure timing is still listed in
[`docs/WINDOWS-CHECKLIST.md`](docs/WINDOWS-CHECKLIST.md).

**You can now pick any of the four sisters and 13 friends directly in Settings: ChatGPT, Claude,
Gemini, Grok, DeepSeek, Qwen, Mistral, Llama, Sakana, Perplexity, GLM, Kimi, Hunyuan, MiniMax,
Nemotron, Cohere, and MiMo.** Each one's layered workplace rig ships offline with the desktop app;
the 17 rigs total 402 PNGs, 35,140,885 bytes. The screen decodes only the current character's 21 to 26
layers and switches only after all of them succeed; the full 1280 canvas is shown as a transparent,
full-body figure floating on the desktop, no longer cropped to an upper-body portrait in a frame. If
any layer fails, it keeps showing the same character's transparent full-body bundled WebP. The default
is ChatGPT; there is no Neutral and no S/T/C/G/X letter fallback. Selection, sizes, SHA-256, and
license boundaries are in `apps/desktop/ui/persona-reels/manifest.json` and `NOTICE.md`; the
equivalent data for the 17 transparent full-body WebPs is in `apps/desktop/ui/personas/`. The images
are not covered by the Apache-2.0 code license; the five app icons derived from the ChatGPT preview
are pinned separately in `apps/desktop/src-tauri/icons/{manifest.json,NOTICE.md}` and don't borrow the
earlier grant to widen its scope. The reproducible selector and its exclusions are described in
[Persona Reel selection](docs/PERSONA-REELS.md).

Local character voices also install with the desktop app: each of the 17 characters has an 8-line
base pack and a 24-line expansion pack, **544 Ogg Opus clips, 8,895,060 bytes** in total; plus four
per character, **68 spoken consent readings, 5,932,965 bytes** in total; plus 20 per character,
**340 short banter lines, 2,249,872 bytes** in total.
Short phrases sent from the input box go, like any other question, to the selected CLI and the local
memory path; fixed lines are never used to bypass the brain or to pose as a dynamic answer. Banter
doesn't go into answers either: it isn't an answer and cites no source.

Sound is off by default. Fixed character lines play only when you tap the character. alpha.132 added
a banter pack, **the only bundled voice that can play once sound is on without first tapping the
character or turning on read-aloud**, and it has only two outlets: a small laugh to herself when
nothing is going on (after a random 2 to 5 minutes, and only if the window is visible, she isn't busy,
nothing is paused, and you aren't typing), and one short filler sound the moment an answer lands (not
when Azure read-aloud is on, because two overlapping voices make both hard to follow). Startup,
polling, recording, and memory events still never make her speak. A full stop and turning the
character off block both outlets, turning off fixed lines blocks only the laugh, and the answer filler
follows the sound switch. With sound off, she still shows the laugh line as text beside her for six
seconds; that is a visual, not a sound.
Consent read-aloud ships turned off. Once you turn it on with one trusted click, each sheet's wording
is played once in the current character's local bundled Ogg, without pressing again for each sheet.
Turning it off stops playback immediately, and the choice is saved in config across restarts; it
doesn't borrow Azure or `localService`, and if the transcript doesn't match the current native wording,
reading is disabled for that sheet. These fixed character voices don't go online, don't call a CLI,
and don't borrow a system voice. The 「用本機聲音朗讀」 ("read aloud with a local voice") button under a
dynamic answer still has to be pressed separately, and it only accepts a Traditional Chinese or
Chinese system voice that WebView explicitly reports as `localService = true`.
Voice sources, per-file hashes, rights scope, and the full inventory are in
`apps/desktop/ui/persona-voices/v1/{manifest.json,NOTICE.md}`,
`apps/desktop/ui/persona-consent-voices/v1/{manifest.json,NOTICE.md}`, and
`apps/desktop/ui/persona-banter-voices/v1/{manifest.json,NOTICE.md}`.

alpha.110's Azure Traditional Chinese read-aloud is still **optional and off by default**; it isn't an
automatic fallback for the local voice: if no local voice is found, she stays silent, and if Azure
fails, she doesn't switch to the other path either. After you enable Azure, pick `eastasia`,
`southeastasia`, or `japaneast`, save the subscription key in Windows Credential Manager under the
fixed target `ted-h/AI-Sister/AzureSpeech/v1`, and sign the fourth sheet, `azure-tts`, each newest
new answer, once complete, triggers one HTTPS `POST` from native Rust to the chosen region's fixed
`https://<region>.tts.speech.microsoft.com/cognitiveservices/v1` and plays once automatically. Under
the answer you can stop it or start a trusted manual replay, and a replay POSTs again. Turning the
setting off means no new auto-read starts. Startup, rereading settings or state, redrawing old
answers, demos, tapping the character, recording, and memory events never send an answer after the
fact. WebView still only goes through IPC, can't pass a URL, and there is no redirect, proxy, or retry.

Each POST contains only **the raw body of the current answer**, which may include names, phone
numbers, and amounts and is not masked first; it doesn't include screenshots, source links or source
chips, memory ids, the database, or any other text. The subscription key never goes into
`config.toml`, logs, the database, or exports. While you type it, it briefly exists in the password
field and in Tauri IPC; the page clears it right after saving, and the native reply carries only a
Present, Missing, Unreadable, or Unsupported status, so the secret is never read back into the
renderer. The program keeps no text or MP3 cache; every new answer and every manual replay can be a
new request. Pressing stop or switching to another question immediately invalidates that generation
of audio, and an MP3 that arrives late isn't played or cached; but a blocking POST that has already
been cleared to send **can't be recalled midway**, may still run until the 45-second timeout, and Azure
may already have counted it toward your usage.
If the transport has already been cleared to send, a native cancel, turning the switch off, changing
region or voice, saving or deleting the key, or changing the fourth consent may wait until that POST
finishes or times out before replying; stopping playback on screen still happens immediately. After
any of these operations replies successfully, an old settings or consent snapshot can't start another
POST.
Microsoft currently lists the Azure Speech F0 neural TTS allowance as 0.5 million characters per
month; availability, billing, and quota depend on your Azure account, resource, and plan, and on
Microsoft's rules at the time. AI-Sister doesn't provide or guarantee any free allowance. See
[Azure Speech pricing](https://azure.microsoft.com/en-us/pricing/details/speech/).

**alpha.100 asks one more question, only about unattended URLs.** If the setting hasn't been answered
yet, the first time you open `sister-desktop.exe` it asks directly:
「有時候我讀到的東西裡會有一個網址。你不在的時候，要我自己按下去嗎？」 ("Sometimes what I read contains a
URL. When you're not here, do you want me to click it myself?") The CLI entry point for the same
question is `sister url-policy`. Both answers are valid:

- `only-on-my-press`: 「等我在。」 ("Wait until I'm here.") URLs always wait for you to click them in
  person, and a standing grant can't carry them.
- `when-you-can-name-the-origin`: 「可以，但你要說得出它從哪來。」 ("Yes, but you have to be able to
  say where it came from.") A standing grant can carry a URL only if she saw the exact same full URL
  in a retained alpha.103 v2 real Windows recording: the scheme, port, path, query, and fragment must
  all match; only when the address bar omits the scheme is a one-level `www.` difference allowed, and a
  subdomain is not treated as the same site. v1 recordings from alpha.100 to alpha.102 are still
  searchable but no longer count as an origin ticket for unattended URLs.

No answer is neither a refusal nor a default choice; until you answer, unattended URLs fail closed.
This setting only governs the two paths that don't wait for you to click in person: `sister do` with
`--use-grant --unattended`, and every step of `sister takeover` after you answer 「好」 (yes). A URL you
click in person runs normally under either answer, and `--dry-run` doesn't go through this gate either. v1 and older
recordings, imports, and replays are never promoted to origin tickets; after upgrading, the v2
recorder has to actually see that URL once. **Having seen the same URL is only a record of where it
came from. It doesn't prove that the URL is safe or that you opened it yourself, and it doesn't prove
that a redirect or the site's content can be trusted.**

**Unattended mode only does the one step you picked by hand when you saved the grant.**
`sister do --save-grant` lists the commitments that fit the grant's scope, and it saves only after you
enter a number and answer 「好」 (yes). After that, `--use-grant --unattended` only lets that
commitment's single action through; if the model changes the target or proposes another step, it is
refused. Opening a URL also requires writing out the full URL with `--target-url` when you save, and
it must be the address bar of the screen she cited; if the text on that same screen also contains this
URL, that step is handed back to you to click in person. This binding stops "the model swapping the
target on its own"; it doesn't judge for you whether the target can be trusted: if the commitment you
picked had a target that came from instructions on the screen in the first place, it still runs.

**Takeover (`sister takeover`, since alpha.170).** She lays out, all at once, the next steps on the
commitment table that she can do herself (at most five; each step only opens a URL, opens a file, or
switches to a window), along with the items she won't take on this time and why. You answer 「好」
(yes) once in the terminal, and she does them in order. The proposal is valid for ten minutes, and if
she isn't recording, she doesn't propose anything. Before each step she rereads that commitment and
reruns the same chain of checks as unattended mode; after each step she looks at the next screen. If
it doesn't match, the step didn't work, the hands are unplugged, or a full stop fires, none of the
later steps run, and the finished ones aren't undone. That authorization covers only the steps she
laid out, lives only within that one run, and is never saved as `grant.json`.

```bash
sister takeover                   # lays out the next steps she can do; answer yes or no once
sister takeover --status          # how far the last takeover got: waiting for an answer, which step, interrupted, or finished
```

An interrupted run doesn't pick up again on its own; the next takeover rereads the commitment table,
and a step that already succeeded isn't done a second time.

At the end of `sister watch`, you can optionally append a line to a local JSONL file, or POST to a
Discord webhook given explicitly in that command. Both contain only the result, a fixed summary, the
start, end, and duration, and four counts; no question, screen text, app, URL, path, or memory ID. Keep
the webhook in an environment variable, not on the command line; when a full stop has fired or the
state is unknown, nothing is sent to Discord. The JSONL file you name is not managed by `forget`,
`prune`, or memory export (「等測試跑完」 below means "wait for the tests to finish"):

```bash
sister watch "等測試跑完" --remote-json ./watch-reports.jsonl
SISTER_DISCORD_WEBHOOK='https://discord.com/api/webhooks/…/…' \
  sister watch "等測試跑完" --discord-webhook-env SISTER_DISCORD_WEBHOOK
sister hands runs --json          # full local takeover audit; includes the task and action targets, not a redacted report
```

**Linux X11 (Ubuntu 24.04) install**: from [Releases](https://github.com/teddashh/AI-Sister/releases),
download `AI-Sister-Linux-X11-amd64.deb`:

```bash
sudo apt install ./AI-Sister-Linux-X11-amd64.deb
```

The package installs the desktop app, the `sister` CLI, AT-SPI, and Traditional Chinese and English
Tesseract.

**macOS 14+ status**: there is currently no publicly downloadable `.app` or `.dmg`. ScreenCaptureKit,
Vision OCR, AX privacy context, and the TCC settings UI are in the same product build; a public
installer is added to [Releases](https://github.com/teddashh/AI-Sister/releases) only after Developer
ID notarization and TCC/S1 acceptance on real Apple Silicon hardware both pass. CI's hardened ad-hoc
app-tree diagnostic is not a Preview and isn't offered for general installation.

**From source**: Windows, macOS, and Linux can all build the same CLI. First, replay the
deterministic script in the repo:

```
git clone https://github.com/teddashh/AI-Sister.git
cd AI-Sister
cargo build --release -p sister-cli --locked
./target/release/sister --data-dir ./data replay scenarios/bill-lookup.json
./target/release/sister --data-dir ./data query 電話
```

The last line gives you this (the output is in Chinese: two phone numbers, each with the line it
came from on screen, the time, the app, the window title, and the frame):

```
🔍 「電話」 2 筆答案、0 筆原文，0.3 ms

我最後看到的是：
  ★ +886800080123  「0800-080-123」
    ↳ phone · 2026-08-19 04:37:39 (剛剛) · chrome.exe · 中華電信 客戶服務 - 帳單查詢 · frame #1
  ★ +886912345678  「0912-345-678」
    ↳ phone · 2026-08-19 04:36:47 (1 分鐘前) · chrome.exe · 中華電信 客戶服務 - 帳單查詢 · frame #1
```

**Clone to first answer was measured at 33 seconds** (a clean `CARGO_HOME`: fetching 108 MB of
dependencies plus a 32-second build, on a 16-core dev machine; GitHub's runner takes about 2.1 times
as long). You need Rust 1.88 or later, because this code is edition 2024; CI runs `cargo check` with
Rust 1.88.0 on both the root and the desktop workspaces. No `sudo`, no services, and no accounts
anywhere along the way.

This step **doesn't read a single pixel of your screen**, so it needs no consent: `replay` reads the
JSON script in the repo, and there is nothing to consent to. To see what she does on your own machine,
take the Windows path above.

The `sister replay scenarios/bill-lookup.json` command is unchanged; scenario JSON must now spell out
`privacy_context` and `system_state` (for example `"clear"` or `"active"`). These two fields are the
test corpus declaring its safety preconditions; if either is missing, it refuses to run instead of
assuming "known safe". You can now also turn a real workday you recorded into replay corpus:

```bash
sister replay export --last 24h --to ./workday.sister-replay-draft.json
sister replay import ./workday.sister-replay-draft.json --dry-run
```

`export` always writes a private **Draft**: times become relative, text is automatically redacted
first, and there are zero screenshots and zero source data or image paths. But "automatic redaction
finished" doesn't mean "safe to share": real screen text can still contain names, internal case
numbers, and conversations the program doesn't recognize, so a person must review it item by item
before changing the JSON's `review` from `draft` to `reviewed`. Only a **Reviewed** corpus like that
can be shared. `import --dry-run` validates a Draft locally, rebuilding the search index and L1 facts
from the redacted L0, and doesn't block local replay just because the Draft isn't Reviewed yet.
Neither command uploads anything.

To also turn the questions you actually asked her during the same period into a question set waiting
for labels, give `export` one more output file:

```bash
sister replay export --last 14d --to ./workday.sister-replay-draft.json \
  --questions-to ./workday.sister-questions-draft.json
```

The question set and the corpus are bound to the same fingerprint, and times keep only relative
milliseconds, with no database row ids or real epochs. Every question's `expected` is `null`:
returning 0 hits at the time, clicking a source, or pressing ★ only count as labeling hints and are
never guessed to be the right answer. Questions keep your original wording without automatic
redaction, so this file is a private Draft. You no longer need to edit the JSON by hand; you can walk
through the whole question set in the terminal:

```bash
sister replay questions status ./workday.sister-replay-draft.json ./workday.sister-questions-draft.json
sister replay questions annotate ./workday.sister-replay-draft.json ./workday.sister-questions-draft.json \
  --to ./workday-labeled.sister-questions-draft.json
sister replay questions review ./workday.sister-replay-draft.json \
  ./workday-labeled.sister-questions-draft.json --to ./workday.sister-questions.json \
  --confirm-private-text-reviewed
```

`annotate` shows the product's real `facts` retrieval candidates for each question; you can also use
`f <text>` to search the corpus and `e EVENT` to see text that can serve as evidence. A label is
recorded only when a person enters `a EVENT <answer>` or `n`. Output always goes to another new file,
never changing the source or overwriting an existing file; before going interactive it checks that the
destination is writable, it syncs a still-valid Draft after each question, and `q` lets you leave
with your progress. `review` produces a Reviewed question set only when everything is labeled, the
fingerprint and evidence are valid, and a person explicitly confirms that the unredacted question
wording has been reviewed. The corpus and the question set are still reviewed separately; neither one
passes review on behalf of the other.

Phase 2's first runner also works directly:

```bash
sister replay evaluate scenarios/recall-baseline.corpus.json scenarios/recall-baseline.questions.json --k 5 --runs 3
```

The full syntax is `sister replay evaluate <corpus> <questions> [--k K] [--runs N] [--json | --to FILE]`;
`--k` defaults to 5 and `--runs` to 3. `--json` prints the full report to
stdout, and `--to` writes a new file and refuses to overwrite. The repo's
`scenarios/recall-baseline.corpus.json` and `scenarios/recall-baseline.questions.json`
are a Reviewed smoke fixture of 3 purely synthetic events and 5 QA questions, with no real workday
data.

Both configurations go through the real product retrieval wiring. `baseline_text` is the existing
text path: three FTS5 indexes plus a bounded LIKE fallback when needed, not a single "pure FTS" query;
`facts` adds L1 typed facts on the same text path and ranks fact results ahead of text results. The
tables below are generated automatically from the stable fields of an actual
`sister replay evaluate --json` run; CI reruns the same fixture, and the regression contract checked
into the script locks in the currently accepted scores. To deliberately accept a new baseline, first
find out what changed, update the script's `expected_scores`, then run
`python3 scripts/check-recall-baseline.py --update-readme` to regenerate the table. That command
regenerates the tables in the Chinese [README.md](README.md); the two tables below are a translated
copy as of v0.1.0-alpha.170.

| Configuration | Recall@5 | Answer accuracy | Source accuracy | Model calls | Cost |
|---|---:|---:|---:|---:|---:|
| `baseline_text` | 2/4 (50.0%) | 3/5 (60.0%) | 2/4 (50.0%) | no brain run | no brain run |
| `facts` | 4/4 (100.0%) | 5/5 (100.0%) | 4/4 (100.0%) | no brain run | no brain run |
| `facts_session` | 4/4 (100.0%) | 5/5 (100.0%) | 4/4 (100.0%) | no brain run | no brain run |

Those 5 questions **have no time range at all**, so `facts_session` is identical to `facts`: sessions
add nothing for that kind of question, and that row is published as is. The next table is a different
fixture (a 115-minute synthetic afternoon, 3 questions) dedicated to questions with a time range,
generated and locked the same way by the same script:

| Configuration | Recall@5 | Answer accuracy | Source accuracy | Model calls | Cost |
|---|---:|---:|---:|---:|---:|
| `baseline_text` | 2/2 (100.0%) | 3/3 (100.0%) | 2/2 (100.0%) | no brain run | no brain run |
| `facts` | 2/2 (100.0%) | 3/3 (100.0%) | 2/2 (100.0%) | no brain run | no brain run |
| `facts_session` | 2/2 (100.0%) | 3/3 (100.0%) | 2/2 (100.0%) | no brain run | no brain run |

**All three configurations now tie on this one, so it can't tell what sessions add.**
「我昨天下午在弄什麼」 ("what was I working on yesterday afternoon") asks only about time, and all three
configurations simply list what happened in that window. 「昨天下午那份週報寫了什麼」 ("what did that
weekly report say yesterday afternoon") could only be answered by `facts_session` in alpha.156: the
other two first searched for 「份週報寫」, then dropped the leading 「份」 and tried 「週報寫」, and neither
string was on screen. Since alpha.157, the unmatched trailing 「寫」 is dropped too and 「週報」 (weekly
report) is searched instead, so they tie. The denominator was only 2 questions to begin with, not a
statistically meaningful number; for that, the question set has to grow to ≥ 100 questions (one of
Phase 2's exit criteria, not reached yet).

Latency varies with the machine and runner, so it isn't part of the CI comparison above. What follows
is only a dated snapshot with its environment: 2026-08-23, a release build on the current Linux dev
machine, 1 warm-up round, then each question timed 3 times:

| Configuration | Latency p50 / p95 |
|---|---:|
| `baseline_text` | 0.06 / 0.09 ms |
| `facts` | 0.15 / 0.19 ms |

The questions come from the query log (0), manual labels (3), and script-planted questions (2).
Neither configuration has a model path, so model calls are 0 and cost is US$0/day; nudge false
alarms and misses, segmentation F1, the Reviewer's check-back rate, CPU, RAM, battery, and disk
haven't been measured yet and are `null` in the JSON report, not 0. Latency only represents this one
run on this one machine; this 5-question synthetic fixture is the runner's reproducible smoke test,
not the public ≥100-question Phase 2 baseline, and it doesn't represent real workday quality. The full
report carries returned text; the corpus and the question set each have their own Draft or Reviewed
status, and while either input is still a private Draft, the report is private data too. Don't share
it before a human review.

To see this report on the desktop, first turn on the developer entry point explicitly. On Windows, the
file the desktop actually reads is `ted-h\AI-Sister\config\config.toml` under `%APPDATA%`. If the file
already has a `[shell]` section, only add or change `developer_mode` in that section and don't paste a
second `[shell]`; add the snippet below only if the section doesn't exist. Leaving it out is the same
as `false`, and ordinary users' system tray won't show the entry:

```toml
[shell]
developer_mode = true
```

Fully quit and reopen `sister-desktop.exe`, and the system tray gains an item called 「評測指標…」
("evaluation metrics…"). First use the CLI to write the report to another new file, then open it with
the page's native file picker:

```bat
.\sister.exe replay evaluate .\workday.sister-replay-draft.json .\workday.sister-questions.json --to .\report.json
```

After you pick the file, the full report text briefly enters this local WebView, and then Rust in the
same process parses it strictly; what the page actually keeps and shows is the numeric projection Rust
returns. The projection strips all free text from the report, including the corpus and question set
names, fingerprints, each question's original wording, returned content, and free-form question ids;
failed questions are located by their 1-based number in the question set instead. The whole path
stays offline and uploads nothing, and the page doesn't keep a separate copy of the report. But the
original `report.json` on disk still contains that text; while any input is a Draft, the page keeps
showing a private Draft warning, and the page not showing verbatim content doesn't make the original
file shareable. This entry point is wired, but the real Windows system tray, file picker, and three
loading states are still on the real-machine checklist, and Linux tests aren't used to stand in for
them.

Asking her 「**她知道了什麼**」 ("what does she know") no longer runs a full-text search for 「知道」
(know). The desktop sister directly lists up to three of the most recent current L2 understanding
cards; each one says plainly that it is a correctable hypothesis and keeps its author, confidence
source, and screen source buttons. If there are raw records but no L2 has been organized yet, she says
exactly that; if the recent candidates currently have no screen source, she doesn't fill the gap with
source-less cards or OCR snippets. This narrow intent only recognizes the complete phrasing, so
「妳知道客服電話嗎」 ("do you know the support phone number") is still searched as 「客服電話」 (support
phone number). When a CLI is selected and the second consent is valid, the overview question also goes
to that CLI first to decide the lookups; the local L2 overview fallback only shows when no brain is
selected, the consent is missing, or the lookup fails. Existing L2 cards may have been organized
earlier, under the second consent, by the CLI you selected.

Asking her 「**剛剛發生什麼事**」 ("what just happened") gets an answer, not 「我記得的東西裡沒有這件事」
("nothing I remember covers this"). That question is about time, not keywords, so she doesn't try to
match those seven characters: she lists the last few things she saw, each with its time and source,
and first says 「我把它當成時間問題了」 ("I took that as a question about time"), so you know why the
answer doesn't match what you typed. The judgment is deliberately timid: if any word with real content
is left in the sentence (「剛剛那個電話號碼」, "that phone number just now"), it goes through search as
usual, because losing what you actually wanted to ask is much worse than one extra lookup.

And when it does go through search, the word 「剛剛」 (just now) **isn't included in the match**.
Chinese has no spaces, so the whole sentence would be searched as one long substring, and nobody's
screen says 「剛剛那個優惠方案」 ("that discount plan just now"); so time words and function words at the
start and end are stripped first, and the middle is kept as is. Adding 「剛剛」 and getting zero hits
is exactly the kind of answer that loses this product's trust fastest. Ordinary retrieval questions
still share one set of rules between `sister query` and the desktop sister; the L2 overview above is a
narrow intent in the outer layer of desktop Q&A, and the CLI currently doesn't turn the same sentence
into an overview.

When you ask for 「**電話**」 (phone), the answer is the number itself (`★ +886800080123`, with the
original screen line 「客服專線 0800-080-123」 ("support line 0800-080-123") and how many times it was
seen attached underneath), not a pile of text that happens to mention phones. The word 「電話」 never
appeared on screen, so full-text matching could never connect them; but when that string of digits
was recorded, it was already tagged as a phone number. This layer used to exist only in the terminal,
and the desktop sister could only do full-text matching: for the same sentence, `sister query` could
answer while she said she couldn't find it. Now both share the same code (`sister-core::answer`),
under the same discipline as the 「剛剛」 judgment above.

**When she can't answer, she tells you the reasons she can find instead of guessing.** Ask for a
「轉帳帳號」 (transfer account number) when she has nothing, and you used to get 「這件事我沒看到過」 ("I
never saw this"): an assertion, when the real answer is often "you told me yourself not to look at
that site". She can actually find that out: how many segments the exclusion rules blocked and how
many times she was paused are all sitting in the database. So that sentence is now followed by a line
like "but your own exclusion rules did block things (excluded url 12 segments, excluded app:
keepassxc 3 segments); I could never have known what was in there". When she can't find any reason,
she says so directly: none of the segments she recorded contains this word. There is no comfort in
that sentence, but it is true.

Even the opening line is about her own records, not about the world: 「**我記得的東西裡沒有這件事**」
("nothing I remember covers this"), not 「這件事我沒看到過」 ("I never saw this"). The thing might have
been right there on screen, only blocked by an exclusion rule, skipped during a pause, or missed by
OCR; she can't even count that last kind, so the reasons listed under it will never be complete. It is
the same discipline as the 「我最後看到的是：」 ("the last thing I saw was:") line above the ★ results.

The desktop sister also tells you **whether anyone is actually recording right now**, and right under
that sentence is the button to start her. These two things used to be mixed up: `sister record` is a
separate executable, and when nobody had started it, the pause flag was clean, so she showed 「在聽」
("listening") while seeing nothing at all. Now when nobody has started her, she is gray and says, in
effect, "No one is recording. Anything that happens from now on, she won't know," and she grows a
**Start recording** button (「開始記錄」), the only thing in color on the whole grayed-out screen. It
looks different from pause because **the next step is different**: one needs Resume, the other needs
the recorder started (the button doesn't appear during a pause, or there would be two recorders each
recording their own copy). The judgment relies on the timestamp the recorder stamps every 5 seconds,
not on the session row in the database, because when the recorder crashes, that row stays "not
finished yet" forever.

When she is grayed out, she also says **when the last time was and why it stopped**
(「上一次 08-19 02:53 停的：你按了停止」, "last stopped 08-19 02:53: you pressed stop"). Without that
line, the gray text you see when you open your computer in the morning could mean you pressed stop
yourself last night, or that she crashed in the middle of the night and you went unrecorded all day,
and only the second one needs you to do something. `sister doctor` shows the same line.

If you press it and she can't start, the last few lines of `record.log` show up right on her (consent
not signed, `sister.exe` not found, one already running); that file is deep inside `%APPDATA%`, and
someone staring at an unresponsive button won't go digging for it. Stop is in the system tray menu (its
label follows the current state), or you can run `sister stop`. **Quitting the desktop sister stops
recording too**, and while recording, that menu item reads 「結束（記錄也會停）」 ("Quit (recording stops
too)"); otherwise you would be closing the only visible window while the screen keeps being recorded.
Stopping goes through a file rather than killing the process: a killed recorder doesn't finish its
session or clean up its heartbeat, so for the next 16 seconds she would claim she is still recording.
**Pressing it during the minutes she is still opening the database also counts**: the request waits
until she has finished opening it, and then she stops without recording a single word (a database
kept for a year rebuilds its indexes the first time it opens, and that takes a while).

Being able to stop is this product's premise, so pause is reachable from four places (the global
hotkey `Ctrl+Alt+P`, the desktop sister's `⏸`, the system tray menu, and `sister pause`), and it
**never resumes on its own**: a pause that wakes up by itself isn't a pause. The hotkey route doesn't
require finding her first, but global hotkeys are first come, first served: the Settings page tells
you plainly whether this combination is currently registered, and if it isn't, it shows the reason in
a warning color instead of letting you press it and get no response. During a pause she is gray all
over, `sister record` mentions it once a minute, entering and leaving a pause each leave an audit
record, and `sister stats` can tell you afterwards whether it was actually stopped that day. Details
are in [docs/PRIVACY.md](docs/PRIVACY.md#停用不等於刪除).

She also remembers **what you asked her**, only on this machine. That is the only table in the whole
database that stores text you typed yourself (every other table holds things she observed), so it has
its own switch and its own section of documentation, and "forget this period" takes the questions
asked during that period with it. The reason to keep it is that the next stage of evaluation needs
real phrasing for its question set, and that can't be rebuilt after the fact: nobody remembers how
they asked something last week. **The questions that found nothing are the most valuable**: the ones
she can answer only prove what she can do now, and the ones she can't are what the next version needs
to fix. `sister queries --empty` asks exactly that.

One column in that table isn't something she recorded; it is something you press. The moment she
correctly answers something you had long forgotten, the 「這件事我本來已經忘了」 ("I had actually
forgotten this") button under the answer (`sister mark` in the terminal) records that moment. Why a new
column for this: none of the question set's other columns can answer it. They remember what you
asked, how many hits she gave, and which source you opened, but **not whether you knew the answer at
the time**. It is also the only column that can't be filled in later: it is the state of your head the
moment you saw the answer, and a week later you can't recover it by looking through the question set.
If you press it by mistake, pressing again takes it back; `sister queries --marked` lists which
questions are marked.

You can only trust what you can see, so the timeline (the `▤` on the drag bar) lists every day she has
records for, and **every gap explains itself**: her being paused and you staring at the same document
without moving show up on that line in two different colors with two different sentences. Scroll to a
period you don't want to keep, and the bar below it can forget it: deleting takes two presses, and the
first one shows you how much will be deleted. In the terminal it is `sister forget --last 2h` (also
two-step, and a unitless form like `--last 30` is refused outright: it looks like 30 minutes, and just
as much like 30 days). See [docs/PRIVACY.md](docs/PRIVACY.md#忘掉某一段時間).

**It is only yours if you can take it with you.** The destination of `sister export --to <dir>` is a
data directory in its own right, not another format, so restoring needs no tool, and doesn't even need
this project to still be alive:

```bash
sister export --to ~/sister-backup --with-frames
sister --data-dir ~/sister-backup query 電話      # answers straight away
```

Don't copy `sister.db` yourself: the database runs in WAL mode, and while she is recording, the most
recent stretch still sits in the `-wal` file next to it. A backup that copies only the main file will
quietly be missing the last few hours, and you will find out on the day you actually need it.

The 17 characters' workplace rigs and WebP fallbacks all ship offline with the desktop app, and they
still work if a download is refused or an old cache is broken; there is no fallback that fakes a
character with a single letter. The optional pack only adds the four sisters' fixed voice lines, and
it still has to pass the public rights projection plus whole-pack and selected-file verification
before it can play. Both sets of bundled images also have their own source manifest and license
exclusions; being in the installer is not an endorsement of the assets' rights.

Run `sister doctor` first: it doesn't claim anything, it just demonstrates on the spot whether it can
read your current URL, whether the OCR engine can read the text on its built-in image, and which
privacy rules don't actually take effect right now.

What has been measured so far (GitHub Actions' windows-latest runner, release build, 1024×768; these
are **not** typical desktop numbers and mainly exist to catch regressions):

| Per operation | Measured |
|---|---|
| One screen read (only one per tick) | 17 to 33 ms |
| OCR, one image | 126 to 193 ms |
| Writing one PNG | 6 ms |
| Ticks where nobody touches keyboard or mouse | **0 ms**, the screen isn't touched at all |

The first row will be a completely different number on your machine, and it drives everything else:
the cost of a screen read scales with **source pixels**, and going from 1024×768 to 2560×1440 is 4.7
times as many. One measured 2560×1440 machine took 127 ms per read. This can't be inferred, so there
is `sister bench`: it splits one capture into three stages (creating GDI objects, `BitBlt`, and
`GetDIBits`), changes only one variable at a time, exits when done, and writes nothing to the database
and keeps no frames.

Why care about the numbers other than the last row: the power-saving gate only says "don't look when
nobody is touching anything", but even if nobody touches anything all day, she still has to open her
eyes every 5 seconds ("no input" is only a guess that the screen didn't change, not a guarantee). So
there are still at least 17,280 captures a day; but "how many ms per capture" measures wall time,
including waiting on the display driver, and can't be converted directly into a CPU percentage. The
end-of-recording summary lists capture time and whole-session CPU separately and honestly; alpha.46's
44.0% is the accepted active-coding baseline, and this capture floor is no longer used to guess at
its cause.

On the query side, measured on a database holding **about a month and a half of data** (3,110,400
lines of text, dev machine, release build; CI reruns the same benchmark on every push):

| What you typed | Which path | Measured |
|---|---|---|
| `客服專線` ("support line", three or more characters) | trigram index | 0.1 ms |
| `0800` (a whole token) | unicode61 index | 0.7 ms |
| `客服` ("support", two Chinese characters) | bigram index | 0.1 ms |
| Something that isn't there | bigram index (definitely absent) | 0.1 ms |
| `工` (one Chinese character) | **no index, can only scan 30 days** | 0.1 ms |

The third and fourth rows used to be 224 ms and 96.7 ms, and could only find the last 30 days. That was
a real gap, not a lack of tuning: trigram can't match fewer than 3 characters, and unicode61 treats
「客服專線」 as **one** token (it doesn't split per character), so `MATCH "客服"` returns 0 hits, and two
characters is exactly the most common word length in Chinese. The only way left to find it was to scan
all the text, with a cost proportional to how long you have been using it, so it had to be capped at
30 days.

The fix is a third index: Chinese is stored as **adjacent character pairs** (「客服專線」 → 「客服 服專
專線」). The measured cost is **+29%** database size (207,360 lines of text: 97.9 MB → 126.0 MB), not
the doubling the prototype estimated. The time limit went away with it.

One row remains: a **single-character** query produces no pairs, so it is still a scan and still only
sees 30 days. `sister doctor` tells you directly how many lines of Chinese in your own database are
already indexed.

(Incidentally, the same benchmark takes about 2.1 times as long on the runner as on the dev machine.
So any number in the table above that sits close to a threshold would flip on a different machine.
That is also why "the scan doesn't grow with the data" is tested by behavior, not with a stopwatch.)

The last row of the first table is the biggest saving so far. If nobody touches the keyboard or mouse,
the screen most likely hasn't changed, so don't even read it (she still opens her eyes at least every
5 seconds, and on every window switch, or videos and progress bars would vanish entirely). The
difference measured on CI: 40 ms → 13 ms per tick, CPU 3.5% → 2.5%.

Capture cost is almost entirely determined by **how many source pixels are read**, no matter how small
you scale the result: native 1024×768 takes 28.4 ms, and scaled down to 256×192 it takes 24.2 ms, with
12 times fewer destination pixels. So there is no "take a quick look with a small image first" here:
that would only throw away a fully read screen and then read it again.

Phase 0's seven-day self-recording and disk budget aren't met yet; the real-machine CPU and RAM
baselines and the remaining disk gap are recorded in the opening section.

## Document map

Most documents under `docs/` and `research/` are in Traditional Chinese;
[docs/WINDOWS-CODE-SIGNING.md](docs/WINDOWS-CODE-SIGNING.md) is in English.

| Document | Contents |
|---|---|
| [docs/PRODUCT.md](docs/PRODUCT.md) | Final Product definition: positioning, principles, killer scenarios, competitors, moat, non-goals |
| [docs/SPEC.md](docs/SPEC.md) | Final Spec: the four-layer truth model, five subsystems, privacy architecture, cost model, technology choices, verdicts on open questions |
| [docs/PHASES.md](docs/PHASES.md) | Phase 0 to 8 milestones: each phase retires one fatal risk, with measurable exit criteria |
| [docs/PRIVACY.md](docs/PRIVACY.md) | Promises, boundaries, and **what we can't do** (bystander consent, the limits of rule-based exclusion) |
| [docs/DATA_INVENTORY.md](docs/DATA_INVENTORY.md) | A field-by-field inventory of what she actually stores, including known gaps |
| [docs/THREAT_MODEL.md](docs/THREAT_MODEL.md) | Assets, attackers, what is explicitly not defended, and three silent failures that really happened |
| [docs/WINDOWS-CHECKLIST.md](docs/WINDOWS-CHECKLIST.md) | Not one line of Windows capture code has ever run on the dev machine; these are the questions only a real machine can answer |
| [research/landscape.md](research/landscape.md) | Competitors and the ecosystem (checked 2026-08): Recall, Rewind/Limitless, Screenpipe, Everywhere, desktop AI apps, computer-use projects |
| [research/tech-stack.md](research/tech-stack.md) | Technology survey: per-OS capture, Traditional Chinese OCR, SQLite FTS5 and vectors, Tauri overlay, resource budget |
| [research/cost-model.md](research/cost-model.md) | LLM cost estimates (2026-08 prices): monthly cost for four architecture scenarios |

The design started from a roundtable debate among four models (Claude / Gemini / Grok / ChatGPT), 7
questions × 5 rounds. The verbatim extraction is a private conversation and hasn't been published, but
every converged conclusion and the reasoning behind each verdict went into the three design documents
above (especially the open-questions decision table in SPEC §17).

## Three one-liners

- **Product**: a filing cabinet (L0/L1, zero LLM) + a brain (L2/L3, event-driven, can be overturned,
  can close things out) + a gatekeeper (speaking on a budget).
- **Principles**: spend brute force on preservation, not generation; transcription belongs to the
  program, intent to the model; being something you dare to leave running matters more than being
  clever.
- **Roadmap**: "can find it" and "thinks it through right" are connected. Phase 6 finished the
  authorization and prompt-injection boundaries for "can take over", and Phase 7's takeover
  (`sister takeover`) is connected too, with every step gated by replay and hostile-fixture results.
