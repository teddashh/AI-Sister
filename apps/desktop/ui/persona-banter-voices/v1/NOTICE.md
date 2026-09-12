# Bundled persona banter voices

The 340 Ogg Opus files in this directory were generated locally with MediaTek Research's BreezyVoice-300M model. The model and inference code are available under Apache-2.0.

Before they were hash-listed, the clips were levelled in the voice lab: one constant gain each to EBU R128 -23 LUFS under a -1 dBTP true-peak ceiling, a 3 ms fade at each edge, and a trimmed tail on the clips where speech recognition heard words the written line does not contain. No compression, limiting, or other dynamics processing was applied.

The generated persona voice files are excluded from this repository's Apache-2.0 license. Ted Huang granted AI-Sister permission on 2026-09-12 to include the exact, unmodified files listed by `manifest.json` in the source tree and official builds. This grant does not extend to any source recording, private receipt, training corpus, or voice-lab working file.
