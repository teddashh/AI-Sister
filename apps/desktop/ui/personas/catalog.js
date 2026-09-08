/*
 * 設定頁可用的 Persona 公開投影。
 *
 * 這不是 prompt，也沒有 voice、權限或網路資料。17 張圖都是已隨 desktop 提供、
 * 由 personas/manifest.json 逐檔 pin 住的 WebP；設定頁不載入 402 張 Reel PNG。
 * classic script 刻意只把 immutable data 放到 globalThis，接著才由 settings.js 讀。
 */
(() => {
  const personas = [
    {
      id: "chatgpt",
      alias: "ChatGPT",
      group: "四姊妹",
      tagline: "結構與驗證；固定台詞用「我在」開場。",
    },
    {
      id: "claude",
      alias: "Claude",
      group: "四姊妹",
      tagline: "論證與邊界；固定台詞用「慢慢來」開場。",
    },
    {
      id: "gemini",
      alias: "Gemini",
      group: "四姊妹",
      tagline: "打開可能；固定台詞用「一起看看」開場。",
    },
    {
      id: "grok",
      alias: "Grok",
      group: "四姊妹",
      tagline: "直球測試；固定台詞用「收到」開場。",
    },
    {
      id: "deepseek",
      alias: "DeepSeek",
      group: "13 位閨密",
      tagline: "深挖證據與原理。",
    },
    {
      id: "qwen",
      alias: "Qwen",
      group: "13 位閨密",
      tagline: "布局、控場與收斂。",
    },
    {
      id: "mistral",
      alias: "Mistral",
      group: "13 位閨密",
      tagline: "俐落拆解，減少多餘協調。",
    },
    {
      id: "venice",
      alias: "Llama",
      group: "13 位閨密",
      tagline: "自由、直接、不受拘束。",
    },
    {
      id: "sakana",
      alias: "Sakana",
      group: "13 位閨密",
      tagline: "保留變體，試另一條演化路徑。",
    },
    {
      id: "perplexity",
      alias: "Perplexity",
      group: "13 位閨密",
      tagline: "先查證，再下結論。",
    },
    {
      id: "glm",
      alias: "GLM",
      group: "13 位閨密",
      tagline: "先做出可動的版本。",
    },
    {
      id: "kimi",
      alias: "Kimi",
      group: "13 位閨密",
      tagline: "守住前文、脈絡與交接。",
    },
    {
      id: "hunyuan",
      alias: "Hunyuan",
      group: "13 位閨密",
      tagline: "把上下游與被漏掉的人接回來。",
    },
    {
      id: "minimax",
      alias: "MiniMax",
      group: "13 位閨密",
      tagline: "先讓作品能看、能聽、能感受到。",
    },
    {
      id: "nemotron",
      alias: "Nemotron",
      group: "13 位閨密",
      tagline: "工程調度與可部署交付。",
    },
    {
      id: "cohere",
      alias: "Cohere",
      group: "13 位閨密",
      tagline: "多方溝通、引用與協議。",
    },
    {
      id: "mimo",
      alias: "MiMo",
      group: "13 位閨密",
      tagline: "先看人用起來順不順。",
    },
  ].map((persona) =>
    Object.freeze({
      ...persona,
      portrait: `./personas/${persona.id}.webp`,
    }),
  );

  globalThis.__AI_SISTER_PERSONA_CATALOG__ = Object.freeze({
    schema: "ai-sister/persona-catalog/v1",
    personas: Object.freeze(personas),
  });
})();
