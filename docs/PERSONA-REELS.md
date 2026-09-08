# Persona Reel 選材

`scripts/select-persona-reels.py` 從本機 tachie 素材樹選出固定的 17 人 workplace
分層 PNG rig。它不是通用素材複製器：角色、主題與來源相對路徑都寫死，不能從參數
換成別人、別套服裝或遠端 URL。

固定選材是四姊妹 Claude、Gemini、Grok、ChatGPT，以及 13 位閨密 DeepSeek、Qwen、
Mistral、Venice、Sakana、Perplexity、GLM、Kimi、Hunyuan、MiniMax、Nemotron、Cohere、
MiMo；每人只取 `workplace` 的 `v2_parts`。reactions、其他 19 套服裝、舊版 outfit、
normalized source，以及 GLM 目錄裡的 `_flat.png`／`_bust.png` 都不進 runtime 包。

## 使用

兩個路徑都必須是本機絕對路徑；輸出父目錄要先存在，輸出目錄本身必須還不存在，
而且不能位於素材樹或任何 Git worktree 內。預設只驗證並把 sanitized manifest 印到
stdout，不建立輸出目錄：

```bash
python3 scripts/select-persona-reels.py \
  --tachie-root /absolute/path/to/tachie \
  --output /absolute/path/to/new-runtime-dir
```

確定報告後才明確加 `--write`：

```bash
python3 scripts/select-persona-reels.py \
  --tachie-root /absolute/path/to/tachie \
  --output /absolute/path/to/new-runtime-dir \
  --write
```

寫入成功後只有：

```text
new-runtime-dir/
├── NOTICE.md
├── manifest.json
├── manifest.js
└── rigs/
    ├── chatgpt/*.png
    ├── claude/*.png
    └── …其餘固定 15 人/*.png
```

`manifest.json` 的檔案位置一律是相對輸出根目錄的 POSIX path。每張 PNG 都有 bytes、
SHA-256、位置與尺寸；manifest 沒有產生時間，所以同一組輸入會得到相同 bytes。
`manifest.js` 是同一份資料的 compact local script，讓 CSP 不必為讀 JSON 新增
`connect-src`；它只設定 `globalThis.__AI_SISTER_PERSONA_REELS__`。
每套 rig 另固定 1280 canvas 裡的 `320,0,640,640` viewport，桌面小窗呈現頭像與
上半身，不把全身縮成看不清的 160 px 小人。

## 驗證與資料邊界

selector 在 dry-run 就完成全部檢查：

- 只讀固定的 `parts/<id>__workplace__v2_parts/{parts.json,source.json}`、其中明確引用的
  PNG，以及固定的 `outfits_v2_norm/doll_<id>__workplace.png` 來源 pin。
- 驗 17 人完整、1280×1280 canvas、來源 SHA-256、residual／identity binding、唯一且
  遞增的 z、畫布邊界，以及 face、mouth、雙眉、雙眼白、雙虹膜、雙睫毛與
  source residual 必要層。
- 逐張驗 PNG signature、chunk CRC、結尾、檔案大小、8-bit RGBA／non-interlaced、
  實際像素尺寸，並從實際 bytes 重算 SHA-256。symlink、絕對 layer path、`..`、重名與
  Windows 大小寫碰撞都會 fail closed。
- raw `parts.json` 不複製；只投影 `tag/z/x/y/w/h/cx/cy/file` 所需內容。`source.json`、
  PSD／研究目錄絕對路徑、私有來源欄位、產生時間、identity region receipt 與 debug PNG
  都不會出現在輸出。
- 圖層另投影封閉的 `body/mouth/eye/brow` role 與 `render_z`。`render_z` 只在
  原本的眼睛／眉毛 z slots 內重排 `white < iris < pupil < lash < brow`，修正
  See-Through 偶發用眼白蓋住虹膜的原始順序；其他圖層的 slot 不變。
- `--write` 先在輸出父目錄的暫存區重驗複本，再獨占建立全新的輸出；
  `manifest.json` 最後出現。既有輸出不覆寫。

定點測試和 shipped-byte checker 在同一個 Persona asset CI step 裡執行；它們守選材與
出貨 bytes，不發明新的產品 milestone：

```bash
python3 scripts/tests/test_select_persona_reels.py
python3 scripts/check-persona-reel-assets.py
```

## 權利狀態

這支 selector 先證明「選了哪些 bytes、runtime 需要哪些欄位」；授權則另外明記。
素材所有人 Ted Huang 於 2026-09-08 明確提供這批本機素材給 AI-Sister
選用與出貨，所以 manifest 固定標記 `"rights_review": "approved-owner-grant"`，
並把範圍縮到這份 manifest 列出、逐檔 hash 的 17 套 workplace rig 原檔。
`NOTICE.md` 另釘住這份 exact `manifest.json` 完整 bytes 的 SHA-256；CI 同時 pin
manifest 與 NOTICE，不能同步改一張圖和它自稱的 hash 後仍讓舊授權範圍通過。

這些 PNG 可以原樣收進 AI-Sister source tree 與官方安裝包，但不包含在
本專案 Apache-2.0 程式碼授權裡。不能用這份 grant 替其他 theme、reaction、
source PSD 或未列入 manifest 的 debug 圖背書。
