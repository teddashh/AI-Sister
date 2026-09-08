# Persona Reel 選材

`scripts/select-persona-reels.py` 從本機 tachie 素材樹選出固定的 17 人 workplace
分層 PNG rig。它不是通用素材複製器：角色、主題與來源相對路徑都寫死，不能從參數
換成別人、別套服裝或遠端 URL。

固定選材是四姊妹 Claude、Gemini、Grok、ChatGPT，以及 13 位閨密 DeepSeek、Qwen、
Mistral、Llama（stable id: `venice`）、Sakana、Perplexity、GLM、Kimi、Hunyuan、MiniMax、Nemotron、Cohere、
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
每套 rig 的 viewport 固定保留完整 `0,0,1280,1280` 透明 canvas；桌面小窗用較大的
最高 300 px 的舞台呈現全身輪廓與 alpha drop shadow，不再裁成相框裡的頭像／上半身。

### 同一人物的輕量 WebP 退路

`scripts/select-persona-previews.py` 不另選角色或服裝；它只接受 SHA-256 為
`318924b3dd6fb575fdf36d91572b5d7b93ae9c8d6e74f366e93bb1b920b550ce`、
且 `rights_review`／`owner_grant` 投影逐欄相同的 production Reel manifest，再讀
其中 exact 17 IDs 與 `canvas_sha256`。對應輸入只能是 1280×1280 RGBA workplace
canvas，再等比縮成 640×640 透明全身 WebP。預設同樣只驗證與印 manifest；輸出必須是
Git worktree 外尚不存在的新目錄：

```bash
python3 scripts/select-persona-previews.py \
  --canvas-root /absolute/path/to/layerdiff_output \
  --reel-manifest /absolute/path/to/persona-reels/manifest.json \
  --output /absolute/path/to/new-preview-dir

python3 scripts/select-persona-previews.py \
  --canvas-root /absolute/path/to/layerdiff_output \
  --reel-manifest /absolute/path/to/persona-reels/manifest.json \
  --output /absolute/path/to/new-preview-dir \
  --write
```

`--canvas-root` 下面只讀固定的
`doll_<id>__workplace/src_img.png`。為避免同一參數在不同 encoder 產生不同 shipped
bytes，selector 固定 Pillow 12.1.1、libwebp 1.5.0、Lanczos、quality 90、
alpha quality 100、method 6 與 exact transparent RGB；版本不同就明確拒絕。
輸出 manifest 只留來源 canvas hash、產物 bytes/hash 與 encoder contract，不留私有
絕對路徑。已審 preview manifest 的 SHA-256 另固定為
`cf8e6e1b22f90f09ba021c092c3e0e9f5ae0dd39cf5644ffdfeb457ff3dd69c0`；encoder、
來源或輸出 bytes 只要改變，selector 會要求新的 owner review，不會自行替新產物寫
`approved-owner-grant`。對應 `NOTICE.md` 也固定 SHA-256
`981557ff2db030abf75a644fd6fea2a50e69e7aedb27197406fbc324e05712fc`；法律字樣改變
同樣 fail closed。public clone 沒有原始 canvas，因此 CI 不冒充能重生圖；
不依賴 Pillow 的 selector authority tests 仍會執行，並由 Rust 實際 decode checked-in
WebP，驗 640×640、透明 alpha、上下全身跨度，再交叉綁回 Reel canvas hash。

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
python3 scripts/tests/test_select_persona_previews.py
python3 scripts/check-persona-reel-assets.py
```

## 權利狀態

這支 selector 先證明「選了哪些 bytes、runtime 需要哪些欄位」；授權則另外明記。
素材所有人 Ted Huang 於 2026-09-08 明確提供這批本機素材給 AI-Sister
選用與出貨，所以 manifest 固定標記 `"rights_review": "approved-owner-grant"`，
並把範圍縮到這份 manifest 列出、逐檔 hash 的 17 套 workplace rig 原檔。
`NOTICE.md` 另釘住這份 exact `manifest.json` 完整 bytes 的 SHA-256；shipped-byte
checker 會核對 checked-in manifest、NOTICE、明列 pin 與實際 PNG bytes。若改圖、
manifest、NOTICE 或 pin，都必須重新做 owner review；CI 不能從同一份 diff 裡
自己寫的授權欄位獨立推導出權利已成立。

這些 PNG 可以原樣收進 AI-Sister source tree 與官方安裝包；同 canvas 縮出的 exact
17 張 WebP 衍生 preview 另由 `personas/manifest.json` 與 `NOTICE.md` 固定授權範圍。
兩組圖像都不包含在本專案 Apache-2.0 程式碼授權裡。不能用這些 grant 替其他 theme、
reaction、source PSD 或未列入 manifest 的 debug／衍生圖背書。

由 `chatgpt.webp` 裁出的五個 checked-in 應用程式圖示不混進 preview manifest；
它們另由 `apps/desktop/src-tauri/icons/{manifest.json,NOTICE.md}` 固定 source、
`icon.html`／`make-icons.sh` recipe、逐檔 bytes/hash 與 exact owner grant。CI 直接跑
`scripts/check-app-icons.py`，也拒絕 icons 目錄裡任何未列 entry、目錄或 symlink。
