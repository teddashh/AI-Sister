#!/usr/bin/env bash
# 清空過的資料庫，在每一個 surface 上都不可以長得像從來沒錄過。
#
# 這一整支本來是 ci.yml 裡的一段 920 行 inline shell——**全 repo 唯一一道
# 只活在 workflow 裡的閘門**，其他每一道都是 `scripts/check-*`。差別在
# alpha.43 那天現形：本機十三道閘門全綠，push 上去 Linux job 紅在這裡，
# 因為本機**根本沒有人跑得到它**。「本機全綠」於是變成一句每一行都是真的、
# 湊起來在騙人的話。搬出來就只是讓它跟其他人一樣，被 push 之前跑得到。
#
# 另外補一件搬出來才會痛的事：它本來假設 runner 每次都是乾淨的，那些
# ci-* 目錄跑完留在原地（`.gitignore` 蓋著，所以 git status 看不見）。
# 在本機連跑兩次，第二次會讀到上一次的殘骸然後給出一個**不一樣的答案**。
# 所以這裡把整場搬進 mktemp，跑完就收——一道第二次跑會改口的閘門，
# 遲早會被當成 flaky 放寬掉。
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"
cargo build -q -p sister-cli
B="$ROOT/target/debug/sister"

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT
cd "$WORK"

# **斷言寫一次，資料庫餵好幾顆。** 漏掉的從來不是斷言，是**沒有一顆
# 資料庫走到那個狀態**：上一版這幾條全都對，但它只造得出「乾淨收尾
# 之後清空」那一種，而 `sessions` 那個 bug 只在「當掉之後清空」才長
# 出來。新的漏法要加的是底下的 fixture，不是這裡。
check() {
  local dir="$1" what="$2"
  $B --data-dir "$dir" stats  > s.txt
  $B --data-dir "$dir" doctor > d.txt
  $B --data-dir "$dir" facts  > f.txt
  # `她(\*\*)?錄過`：`stats` 那一行是 `她**錄過**`（markdown 粗體），
  # `她錄過` 這個 literal 對它沒中。中間那個 `**` 要放進來，`還沒`
  # 不可以——`她還沒錄過` 正是這幾條要抓的東西。
  for x in s.txt f.txt; do
    grep -qE '她(\*\*)?錄過' "$x" \
      || { echo "::error::[$what] $x 沒說出「她錄過」——清空過的資料庫又長得像全新的了"; exit 1; }
  done
  # 指名那一列。`不在了` 光是「零當機」和三個訊號稽核就有（那幾列讀
  # 的是原始的 `ever`），不指名的話「已記錄」整列壞掉它照樣綠。
  grep -q '已記錄.*一列都不剩' d.txt \
    || { echo "::error::[$what] doctor 的「已記錄」沒說出她記的東西去哪了"; exit 1; }
  # 這幾句只對一台從來沒錄過的機器成立。
  #
  # 用 `if grep`，不是 `grep && { exit 1; }`：後者在**沒中**的時候
  # 整條 `&&` 串回 1，`set -e` 會把它當成整步失敗——也就是程式對的
  # 時候紅、錯的時候綠。`if` 的條件位置不受 `set -e` 管。
  for bad in '還沒錄過' '（還沒有任何內容）' '還沒問過任何問題' '等你錄過再驗一次'; do
    for x in d.txt f.txt s.txt; do
      if grep -q "$bad" "$x"; then
        echo "::error::[$what] $x 對一顆他親手清空的資料庫說了「$bad」"
        exit 1
      fi
    done
  done
}

# fixture 1：乾淨收尾之後清空，然後**再問一句話**。
# 「真的沒了嗎」是刪完最自然的下一個指令，而它會在 `queries` 留一列。
A="$B --data-dir ./ci-erased"
$A replay "$ROOT"/scenarios/bill-lookup.json --interval-ms 500 > /dev/null
$A query 帳單 > /dev/null
$A forget --last 24h --yes | tee forget.txt
grep -q '題庫' forget.txt \
  || { echo "::error::forget 沒有帶走題庫——底下那幾條就沒有在驗它們要驗的東西"; exit 1; }
# `queries` 要趁題庫還空著的時候看——底下那一問會讓它印出列表。
$A queries > erased-queries.txt
grep -qE '她(\*\*)?錄過' erased-queries.txt \
  || { echo "::error::queries 沒說出她錄過——那三種可能又剩下兩種了"; exit 1; }
if grep -q '問她幾個問題' erased-queries.txt; then
  echo "::error::queries 叫他去重做一件他刻意做掉的事"
  exit 1
fi
$A query 真的沒了嗎 > /dev/null
check ./ci-erased "清空之後又問了一句"

# fixture 2：**當掉**之後清空。`delete_empty_sessions` 不准碰
# `ended_at IS NULL AND id = MAX(id)` 的那一列——那可能是此刻正在錄
# 的那一場，刪掉它會把一個活著的 recorder 的外鍵扯斷。而**當掉**的
# 那一場長得一模一樣，所以它撐得過 `forget`，`sessions` 就成了一個
# 「清空之後還是 1」的計數器。
#
# 那一列直接改成沒收尾：`replay` 一定乾淨收尾，`record` 要平台擷取
# 後端（CI 上沒有），而拿 `timeout` 砍時間就是把 CI 綁在秒數上。要驗
# 的是**那個狀態**，那就把那個狀態做出來。
C="$B --data-dir ./ci-crashed"
$C replay "$ROOT"/scenarios/bill-lookup.json --interval-ms 500 > /dev/null
python3 - <<'EOF'
import sqlite3
c = sqlite3.connect('./ci-crashed/sister.db')
n = c.execute(
    'UPDATE sessions SET ended_at = NULL WHERE id = (SELECT MAX(id) FROM sessions)'
).rowcount
c.commit()
assert n == 1, f'沒有工作階段可以弄成當掉的樣子（改到 {n} 列）'
EOF
$C forget --last 24h --yes > crashed-forget.txt
$C stats | grep -q '工作階段  *[1-9]' \
  || { echo "::error::那一列沒有留下來——這個 fixture 沒驗到它要驗的東西"; exit 1; }
check ./ci-crashed "當掉之後清空"

# 留下來的那一列要**當場**講，不是等他下次跑 `stats` 才自己發現一個
# 「工作階段 1」站在一整排 0 旁邊。
grep -q '留著 1 場錄製的紀錄本身' crashed-forget.txt \
  || { echo "::error::forget 沒說出那一列留下來了"; exit 1; }
grep -q '空殼' s.txt \
  || { echo "::error::stats 上那個 1 沒有標成空殼——它是這一頁唯一一個不是 0 的數字"; exit 1; }
# 反面：乾淨收尾的那一顆不可以也印「空殼」。少了這條，把那個但書寫成
# 無條件的也一樣綠。（`s.txt` 被 `check` 蓋掉了，重印一次。）
if $B --data-dir ./ci-erased stats | grep -q '空殼'; then
  # 單引號：雙引號裡的反引號會被當成指令替換跑掉（上一版在這裡把
  # `timeout` 跑了一次，錯誤訊息裡多出一段 usage）。
  echo '::error::那一場好好地收尾了，sessions 那一列也真的被刪掉了，不該有空殼'
  exit 1
fi

# **那一列留著有兩種原因，而下一步剛好相反。** 她當掉了的話，要她
# 「再開始錄」那一列才不再是最新的一列；她正在錄的話，要等她「收
# 工」。上面那幾條 `空殼` / `留著 1 場` 兩種都會綠，所以把分支寫死
# 成任何一邊都驗不出來——而這幾行以前印的正是「當掉了，**或是**她
# 此刻正在錄」，一句把自己的懶惰講成他的功課的話。
grep -q '她當掉了' crashed-forget.txt \
  || { echo "::error::沒有 recorder 佔著這個目錄，forget 卻沒說那一場是當掉的"; exit 1; }
if grep -q '正在錄' crashed-forget.txt; then
  echo "::error::沒有任何 recorder 在跑，forget 卻說她可能正在錄"
  exit 1
fi
# `doctor` 的「上一次錄製」是同一題的另一個 surface，而它上一版沒有
# 人問。`d.txt` 還是上面那次 `check ./ci-crashed` 留下來的。
grep -qE '上一次錄製.*她當掉了' d.txt \
  || { echo "::error::沒有 recorder 佔著這個目錄，doctor 卻沒說那一場是當掉的"; grep -A1 上一次錄製 d.txt; exit 1; }

# 同一顆資料庫，另一句話——差別只有那個心跳檔。心跳就是一個裡面寫
# 著毫秒數的檔案（`heartbeat::BEAT`），所以這個狀態做得出來，和上面
# 那顆「當掉的」用的是同一招：要驗的是那個狀態，那就把它做出來。
#
# 時戳寫在一小時**之後**：未來的時戳一律算活的（那條是給調過時鐘的
# 機器用的），於是這幾行和 runner 有多慢完全無關。拿 `STALE_AFTER_MS`
# 那 16 秒去賭幾次 process spawn，就是把 CI 綁在秒數上。
python3 - <<'EOF'
import time, pathlib
pathlib.Path('./ci-crashed/recording.beat').write_text(
    str(int(time.time() * 1000) + 3_600_000))
EOF
$C forget --last 24h --yes > live-forget.txt
$C stats > live-stats.txt
# **同一顆資料庫、同一列，差別只有那個心跳檔——那一列要跟著改口。**
# 上一版這裡只問 `forget` 和 `stats`，於是 doctor 的「上一次錄製」在
# 「有人佔著」那一半從來沒有被真的執行檔驗過（`d.txt` 是幾行之前、
# 還沒有心跳的時候印的）。翻不過去的那一半就是一格空白。
$C doctor > crashed-live-doctor.txt
grep -qE '上一次錄製.*她現在還在跑' crashed-live-doctor.txt \
  || { echo "::error::有 recorder 佔著這個目錄，doctor 卻說不出她還在跑"; grep -A1 上一次錄製 crashed-live-doctor.txt; exit 1; }
if grep -qE '上一次錄製.*當掉' crashed-live-doctor.txt; then
  echo "::error::有 recorder 佔著這個目錄，doctor 卻說她當掉了"
  grep -A1 上一次錄製 crashed-live-doctor.txt
  exit 1
fi
# 而那一行真的要跟剛才不一樣。兩份逐字相同的話，上面那兩條可以靠一
# 句寫死的話同時綠——這一整節在抓的就是這個形狀。
if diff <(grep 上一次錄製 d.txt) <(grep 上一次錄製 crashed-live-doctor.txt) > /dev/null; then
  echo "::error::心跳出現前後，「上一次錄製」印的是同一句話"
  grep 上一次錄製 d.txt
  exit 1
fi
# 這兩條要的是「**她正在錄**」，不是「有人佔著這個資料目錄」。上一版
# 抓的正是後面那句話，而那句話是這一整批 bug 的形狀本人：開機那幾分
# 鐘目錄也有人佔著，而那時候那一列不是她的，是上一次當機留下來的殼。
# 一句同時涵蓋兩種相反處境的話，抓它的斷言也就同時放過兩種。
for x in live-forget.txt live-stats.txt; do
  grep -q '她此刻正在錄' "$x" \
    || { echo "::error::她此刻正在錄，$x 卻沒這麼說"; grep -n 工作階段 "$x"; exit 1; }
  for wrong in 當掉 正在起來; do
    if grep -q "$wrong" "$x"; then
      echo "::error::她此刻正在錄，$x 卻說「$wrong」"
      grep -n "$wrong" "$x"; exit 1
    fi
  done
done
# 這一刀沒有東西可以刪（上面那一刀已經清乾淨了），走的是「那段時間
# 裡她什麼都沒記到，不用忘」那條提早收工的路。那條路以前**完全不提**
# 那一列——而那一列的 `started_at` 就落在他選的那段時間裡。
grep -q '那段時間裡她什麼都沒記到' live-forget.txt \
  || { echo "::error::這個 fixture 沒走到提早收工那條路，底下那條就沒在驗它要驗的東西"; exit 1; }
grep -q '留著 1 場錄製的紀錄本身' live-forget.txt \
  || { echo "::error::forget 什麼都沒刪的那條路又把那一列藏起來了"; exit 1; }

# fixture 4：**他清空之後，她又開始錄的第一毫秒。**
#
# `Recorder::new` 的第一件事就是寫一列 `session_start`。上面三顆資料
# 庫全都到不了這個狀態：`replay` 一定會一路寫到收尾，所以「有一列
# `session_start`、而且**只有**那一列」在 CI 上一次都沒出現過。而那
# 一列曾經足以讓 `nothing_recorded_left()` 翻成 false——`facts` 說
# 「她還沒錄過」、`doctor` 說「還沒有任何內容」、`stats` 上那個 ⚠ 整
# 個不見。`check` 的每一條斷言本來就會抓到它；缺的一直是這顆資料庫。
# （第三次了：`queries`、`sessions`、現在是那兩列標籤。）
D="$B --data-dir ./ci-restarted"
cp -r ./ci-erased ./ci-restarted
python3 - <<'EOF'
import sqlite3
c = sqlite3.connect('./ci-restarted/sister.db')
sid = c.execute(
    "INSERT INTO sessions(started_at, app_version, platform)"
    " VALUES (strftime('%s','now')*1000, 'ci', 'linux/replay')"
).lastrowid
c.execute(
    "INSERT INTO system_events(session_id, ts, kind, detail)"
    " VALUES (?, strftime('%s','now')*1000, 'session_start', NULL)", (sid,))
c.commit()
n = c.execute("SELECT COUNT(*) FROM system_events").fetchone()[0]
assert n == 1, f'這顆資料庫該只有那一列開場標籤，實際上有 {n} 列'
EOF
check ./ci-restarted "清空之後她又開始錄"
# 那個 1 要說出它是什麼。`stats` 上「事件……系統 1」站在 ⚠ 說完
# 「一列都不剩」的正下方——兩句都是真的，湊在一起就是這一步在抓的謊。
grep -q '系統 1（都是那幾場錄製自己的開始／結束）' s.txt \
  || { echo "::error::那個「系統 1」沒說出它是那場錄製自己的標籤"; exit 1; }
# 反面：一顆**沒有被清空過**的資料庫，這一行不准有但書，⚠ 也不准出現。
# 少了這條，把那個但書寫成無條件的、或把整個 `Emptiness` 寫死成
# `Erased`，上面每一條都照樣綠。
E="$B --data-dir ./ci-normal"
$E replay "$ROOT"/scenarios/bill-lookup.json --interval-ms 500 > /dev/null
$E stats > normal-stats.txt
if grep -qE '開始／結束|空殼|一列都不剩' normal-stats.txt; then
  echo '::error::這一顆好好地錄著，不該印任何「被清空過」的但書'
  exit 1
fi

# **第五顆：一個字都沒存進去過，而 `forget` 從來沒被執行過。**
#
# 上面四顆全都真的存過東西，所以「`nothing_recorded_left()` 是真的、
# 而沒有任何東西被刪掉」這個狀態一顆都走不到——於是 `Emptiness` 把它
# 歸進 `Erased`，三個畫面同時說東西被 `sister forget` 忘掉了，而他一
# 次都沒刪過。這是**上面那幾條斷言自己修出來的洞**：把 `session_end`
# 從「還剩什麼」裡扣掉，順手也扣掉了這一種僅剩的證據。缺的還是
# fixture。
printf '[capture]\nenabled = false\n' > ci-off.toml
N="$B --data-dir ./ci-nulldata"
$N --config ci-off.toml replay "$ROOT"/scenarios/bill-lookup.json --interval-ms 500 > /dev/null
$N stats > null-stats.txt
$N doctor > null-doctor.txt
$N facts > null-facts.txt
# `query` 也在名單上：那一句空手話走的是 `BlindSpots`，和上面三個
# 各自的判斷是**不同的程式碼**——而它是他最常看到的那一個。
$N query 電話 > null-query.txt
for x in null-stats.txt null-doctor.txt null-facts.txt null-query.txt; do
  # 每一句都是一則指控，而這台機器上沒有任何一個刪除發生過。
  # `一列都不剩` 也不行——東西不是「不剩」，是從來沒進來過。
  #
  # 是 `過了保留期` 不是 `保留期`：`doctor` 上那一整段的標題就叫
  # 「保留期」，而它在每一台機器上都印。抓字串要抓到**那句話**，
  # 抓到那個詞就是在抓一個永遠成立的東西。
  #
  # 是 `不在了` 不是 `已經不在了`。那個「已經」讓這一條**錯過了
  # doctor 上的三列訊號稽核**：它們寫的是「那幾場的紀錄不在了」，
  # 逐字比對差兩個字，於是那三列在這台機器上指控了一個沒發生過的
  # 刪除，而這一步是綠的。多兩個字就是多一個出口。
  LIE='forget|過了保留期|不在了|一列都不剩|忘掉'
  if grep -qE "$LIE" "$x"; then
    echo "::error::[什麼都沒存過] $x 指控了一個沒發生過的刪除"
    grep -nE "$LIE" "$x"
    exit 1
  fi
  grep -q 'capture.enabled' "$x" \
    || { echo "::error::[什麼都沒存過] $x 沒指出真正的下一步"; cat "$x"; exit 1; }
done
# 而「她錄過」照樣要講得出來——往回倒成「還沒錄過」是另一種假話。
grep -qE '她(\*\*)?錄過' null-stats.txt \
  || { echo '::error::[什麼都沒存過] 她確實跑過一場，不可以說成還沒錄過'; exit 1; }

# **第六顆：一列都沒存過，而她此刻正開著。**
#
# 上面那一顆是「跑完了、收工了、什麼都沒存到」，下一步是去改設定；
# 這一顆是「三秒前才按下開始記錄」，下一步是**再等一下**。上一版把
# 兩者印成同一句，於是一個剛開始用的人被送去改一個沒問題的設定——
# 而 `Emptiness::of` 收不到 `data_dir`，結構上就問不出這一題。
#
# 心跳檔要現寫（16 秒就過期），所以這一顆造得出來、也只造得出這一
# 顆：`replay` 永遠會一路跑到收尾。缺的又是 fixture。
L="$B --data-dir ./ci-live"
cp -r ./ci-nulldata ./ci-live
python3 - <<'EOF'
import sqlite3, time
now = int(time.time() * 1000)
c = sqlite3.connect('./ci-live/sister.db')
sid = c.execute(
    "INSERT INTO sessions(started_at, app_version, platform)"
    " VALUES (?, 'ci', 'linux/replay')", (now,)).lastrowid
c.execute(
    "INSERT INTO system_events(session_id, ts, kind, detail)"
    " VALUES (?, ?, 'session_start', NULL)", (sid, now))
c.commit()
# 心跳的第二欄不是 'boot'，所以讀成「正在錄」。
open('./ci-live/recording.beat', 'w').write(str(now))
EOF
# **第十一顆是這一顆逐位元的複本**，只有心跳檔的第二欄多一個字。造在
# 這裡而不是那一節裡，是因為底下那四個 `$L` 指令會動到這顆資料庫
# （`query` 會往題庫寫一列），複製要趕在那之前。它自己的心跳等到那一
# 節再寫——這個檔案 16 秒就過期。
cp -r ./ci-live ./ci-bootlive
$L stats > live-stats.txt
$L doctor > live-doctor.txt
$L facts > live-facts.txt
$L query 電話 > live-query.txt
for x in live-stats.txt live-doctor.txt live-facts.txt live-query.txt; do
  # 同一組指控。她三秒前才開始，一列內容都沒進來過——沒有東西可以
  # 被忘掉，也沒有東西可以過期。
  if grep -qE "$LIE" "$x"; then
    echo "::error::[她正開著] $x 指控了一個沒發生過的刪除"
    grep -nE "$LIE" "$x"
    exit 1
  fi
  # 而**這一顆**不准出現 `capture.enabled`：上面那顆的下一步在這顆
  # 是誤導。正反成對——只寫「不准指控」的話，整段功能被刪掉也是綠的。
  if grep -q 'capture.enabled' "$x"; then
    echo "::error::[她正開著] $x 把一個剛按下開始記錄的人送去改設定"
    grep -n 'capture.enabled' "$x"
    exit 1
  fi
  grep -qE '正開著|正在錄|剛開始' "$x" \
    || { echo "::error::[她正開著] $x 沒講出她此刻正在錄"; cat "$x"; exit 1; }
done
# 「零當機」看的是同一批列，而**正在錄的那一場沒有 `ended_at`，在磁碟
# 上跟一次當機逐字相同**。這一顆是最小的那個形狀：一場好好收完的、
# 加上此刻正開著的一場。心跳分得出來，而分得出來就要說出來——那個數
# 字扣掉了一場，不講的話他自己數得出來對不上。
#
# 符號印在標籤**前面**（`  ✗ 零當機   …`）。這一條寫過一版
# `'零當機 +✗'`，逐字永遠不中——一條翻不成紅的反面斷言讀起來像涵
# 蓋，實際上是一格空白，而那正好是這一整節在抓的形狀，出現在抓它的
# 工具上（第二次了，見 `signal_audit` 的註解）。
if grep -qE '✗ +零當機' live-doctor.txt; then
  echo "::error::[她正開著] 把此刻正在錄的那一場算成了一次當機"
  grep 零當機 live-doctor.txt; exit 1
fi
# 而正面那一半要確定它真的印得出 ✓——不然上面那條可以靠「這一列整個
# 不見」也綠。
grep -qE '✓ +零當機' live-doctor.txt \
  || { echo "::error::[她正開著] 一場好好收完的加上正在錄的一場，那是零當機"; grep 零當機 live-doctor.txt; exit 1; }
grep -qE '零當機.*沒有算進去' live-doctor.txt \
  || { echo "::error::[她正開著] 扣掉了正在錄的那一場卻不說"; grep 零當機 live-doctor.txt; exit 1; }
# **`sister queries` 刻意不在這個迴圈裡。** 那一頁把三種可能攤開講
# （還沒問過／`query_log` 關著／問過的被忘掉了），而那三種在這台機器
# 上都還成立——`ever_stored` 答的是「她存過內容沒有」，答不出「他問
# 過問題沒有」，而 `queries` 那張表 `forget` 真的會清。把它加進來就
# 得替 `LIE` 開一個例外，而一句需要例外的斷言守不住東西。攤開留給
# 「他為了那件事才打開的那一頁」，這是這個 repo 一路的規矩。
#
# 而兩顆之間必須真的不一樣。逐字相同的話上面每一條都還是綠的
# ——那正是這一整節在抓的形狀。
#
# **先正規化，不然這一條永遠不會紅。** 每一份輸出裡都印著資料目錄
# （`✓ 資料目錄  ./ci-nulldata`），而兩顆 fixture 的目錄名本來就不一
# 樣——`diff` 於是永遠說「有差」，不管那幾句話有沒有分開。`query` 那
# 一份還多印一個毫秒數。這一條從寫下來的那天起就是一格空白，也就是這
# 整節在抓的形狀本人，第三次出現在抓它的工具上（前兩次見 `signal_
# audit` 的註解和 `'零當機 +✗'`）。驗過：把 `./ci-*` 和毫秒抹掉之
# 後，一顆沒有心跳的 `ci-nulldata` 複本和 `null-doctor.txt` 逐字相
# 同，這一條當場紅。
same() { diff -q \
  <(sed -E 's#\./ci-[a-z]+#DIR#g; s/[0-9]+(\.[0-9]+)? ms/T ms/g' "$1") \
  <(sed -E 's#\./ci-[a-z]+#DIR#g; s/[0-9]+(\.[0-9]+)? ms/T ms/g' "$2") \
  > /dev/null; }
for pair in stats doctor facts query; do
  if same "null-$pair.txt" "live-$pair.txt"; then
    echo "::error::[她正開著] $pair 在兩台機器上逐字相同"
    exit 1
  fi
done

# ── fixture 6：**開機就死的那五次，把自己的證據一起帶走了。** ──
#
# 「零當機」數的是 `sessions` 那張表，而那張表從 alpha.30 開始會刪
# 自己的列：一場「一列內容都沒存到」的錄製，收工時連紀錄一起走
# （那是 #52 要的，那一列是「他那天下午在電腦前」的證明）。於是最該
# 被算進去的那一種當機——開起來、還沒讀到第一張畫面就死掉——剛好就是
# 會把自己刪掉的那一種。分子和分母同時少一，**她死得越早，這一格讀
# 起來越乾淨**，而一台卡在開機當機迴圈裡的機器會收斂到 ✓。
#
# 這一顆是 doctor 唯一驗得到的形狀：計數器和列必須在 `prune` 那一刀
# 前後說同一件事。單元測試釘得住數字，釘不住印出來的那兩行句子。
C="$B --data-dir ./ci-crashloop"
$C replay "$ROOT"/scenarios/bill-lookup.json --interval-ms 500 > /dev/null
python3 - <<'EOF'
import sqlite3, time
now = int(time.time() * 1000)
c = sqlite3.connect('./ci-crashloop/sister.db')
# 五場「開起來就死在第一張畫面之前」。時間往前排，免得跟上面那場好
# 的搶「最新的那一場」——`delete_empty_sessions` 會放過最新的那一列
# （它可能正在錄），那樣就只掃得掉四場，這個 fixture 的數字會對不上。
for k in range(5):
    t = now - (600_000 - k * 1000)
    sid = c.execute(
        "INSERT INTO sessions(started_at, app_version, platform)"
        " VALUES (?, 'ci', 'linux/replay')", (t,)).lastrowid
    c.execute(
        "INSERT INTO system_events(session_id, ts, kind, detail)"
        " VALUES (?, ?, 'session_start', NULL)", (sid, t))
c.commit()
EOF
$C doctor > crash-before.txt
$C prune > /dev/null
$C doctor > crash-after.txt
# 前提：那幾列真的要被掃掉。掃不掉的話下面幾條在驗一個不存在的狀態。
python3 -c "
import sqlite3, sys
n = sqlite3.connect('./ci-crashloop/sister.db').execute(
    'SELECT COUNT(*) FROM sessions').fetchone()[0]
sys.exit(0 if n < 6 else 1)" \
  || { echo "::error::[開機就死] 那幾列沒被掃掉——這個 fixture 沒驗到它要驗的東西"; exit 1; }
# **同一個數字要撐過那一刀。** 掃之前說 5 段沒回來，掃之後還是 5 段。
for x in crash-before.txt crash-after.txt; do
  grep -qE '零當機.*有 5 段沒有回來' "$x" \
    || { echo "::error::[開機就死] $x 的當機數跟著紀錄一起被刪掉了"; grep 零當機 "$x"; exit 1; }
  # 一台開機當機五次的機器不准拿到打勾。（符號在標籤**前面**——寫
  # 成 `'零當機 +✓'` 的那一版逐字永遠不中。）
  if grep -qE '✓ +零當機' "$x"; then
    echo "::error::[開機就死] $x 對一台當機五次的機器畫了 ✓"; exit 1
  fi
done
# 掃完之後報得出時間的只剩一場，那句話就不可以再講成全部。同理
# 「上一次錄製」指的是**還留著紀錄的**那一場，不是真的最後一場。
for want in '連紀錄都沒留下' '還留著紀錄的最後一場'; do
  grep -q "$want" crash-after.txt \
    || { echo "::error::[開機就死] 掃完之後沒說出「$want」"; cat crash-after.txt; exit 1; }
done
# 反面：掃之前一列都沒少，那兩句補充就不准出現——不然它們會變成每台
# 機器上都有的裝飾，然後沒有人再讀它。
for bad in '連紀錄都沒留下' '還留著紀錄的最後一場'; do
  if grep -q "$bad" crash-before.txt; then
    echo "::error::[開機就死] 六列都還在，crash-before 卻說有東西不見了：$bad"
    exit 1
  fi
done

# ── fixture 7：**當機過的那台機器，此刻正開著。** ──
#
# 上面兩顆各站一半：`ci-live` 造得出心跳卻從不看「零當機」那一行，
# `ci-crashloop` 看那一行卻從不寫心跳。交叉的那一格沒有人站著，而那
# 一格正是這批 bug 第十四次犯的地方——正在錄的那一場從當機數裡扣掉
# 了，卻沒有從分母、從「還留著紀錄的最後一次」那個時間裡扣掉。印出
# 來是這樣：
#
#     ✗ 零當機     3 段錄製裡有 1 段沒有回來（最後一次 08-20 02:22）
#     ? 上一次錄製 2026-08-20 02:22:01 開始，沒有收尾——她現在還在跑
#
# 每一行單獨看都對。兩行放在一起，她「當機」的時間就是她「現在還在
# 跑」的那一場的開始時間，而真正那次當機在三天前。
V="$B --data-dir ./ci-livecrash"
cp -r ./ci-crashloop ./ci-livecrash
python3 - <<'EOF'
import sqlite3, time
now = int(time.time() * 1000)
c = sqlite3.connect('./ci-livecrash/sister.db')
# 這一列要拿到 MAX(id)，它才是「正在錄的那一場」。上面那幾場當機的
# 時間都在十分鐘前，所以兩個時間戳一定不同秒——這條測試靠那個差別。
sid = c.execute(
    "INSERT INTO sessions(started_at, app_version, platform)"
    " VALUES (?, 'ci', 'linux/replay')", (now,)).lastrowid
c.execute(
    "INSERT INTO system_events(session_id, ts, kind, detail)"
    " VALUES (?, ?, 'session_start', NULL)", (sid, now))
c.commit()
open('./ci-livecrash/recording.beat', 'w').write(str(now))
EOF
$V doctor > livecrash.txt
# 當機數不准跟著她一起長。掃完之後計數器記得的是 5 段沒回來，多開
# 一場正在錄的不會變成 6。
grep -qE '零當機.*有 5 段沒有回來' livecrash.txt \
  || { echo "::error::[當機過又正開著] 正在錄的那一場被算成了一次當機"; grep 零當機 livecrash.txt; exit 1; }
grep -qE '零當機.*沒有算進去' livecrash.txt \
  || { echo "::error::[當機過又正開著] 扣掉了正在錄的那一場卻不說"; grep 零當機 livecrash.txt; exit 1; }
# **這一條是那個 bug 本人。** 兩行各自抓出時間戳，一樣就是錯的：
# 「最後一次當機」講的不可以是此刻正在錄的這一場。
T='[0-9]{4}-[0-9]{2}-[0-9]{2} [0-9]{2}:[0-9]{2}:[0-9]{2}'
when_crash=$(grep 零當機 livecrash.txt | grep -oE "$T" | head -1)
when_live=$(grep 上一次錄製 livecrash.txt | grep -oE "$T" | head -1)
[ -n "$when_crash" ] && [ -n "$when_live" ] \
  || { echo "::error::[當機過又正開著] 兩行至少要各報得出一個時間"; cat livecrash.txt; exit 1; }
if [ "$when_crash" = "$when_live" ]; then
  echo "::error::[當機過又正開著] 她「當機」的時間就是她現在正在跑的那一場：$when_crash"
  grep -E '零當機|上一次錄製' livecrash.txt
  exit 1
fi
# 而她現在還在跑的時候，「上一次」不是一個近似值——那一列就在手上。
grep -qE '上一次錄製.*她現在還在跑' livecrash.txt \
  || { echo "::error::[當機過又正開著] 心跳在手上卻說不出她還在跑"; grep -A1 上一次錄製 livecrash.txt; exit 1; }
if grep -A1 上一次錄製 livecrash.txt | grep -q '不一定是最後一次'; then
  echo "::error::[當機過又正開著] 那一列就是最後一次，不要替一個看得見的事實道歉"
  grep -A1 上一次錄製 livecrash.txt
  exit 1
fi

# ── fixture 8：**從舊版升上來的那一顆，數不到升級之前被清掉的那幾場。** ──
#
# migration 006 的回填只數得到**還在的列**，所以那個計數器是一個下
# 限，不是一個量到的數。它於是要按一個旗標，而句子從此不准說「全
# 部」——「回填出來的數字是一個猜測穿著數字的衣服」。
#
# 這一整條路上一版只有單元測試走過：CI 這一步從來沒有一顆資料庫的
# `user_version` 小於現在的版本，於是**那句話從來沒有被真的執行檔印
# 出來過**。舊資料庫造得出來：把觸發器拿掉、把兩個計數器刪掉、版號
# 壓回 5——那逐欄就是 alpha.33 留下來的樣子。
U="$B --data-dir ./ci-upgrade"
cp -r ./ci-normal ./ci-upgrade
python3 - <<'EOF'
import sqlite3
c = sqlite3.connect('./ci-upgrade/sister.db')
assert c.execute("SELECT COUNT(*) FROM meta WHERE key='ever_recorded'").fetchone()[0] == 1, \
    'ever_recorded 不在，那 floor 那個條件本來就不會成立——這顆 fixture 沒驗到東西'
for (n,) in c.execute(
        "SELECT name FROM sqlite_master WHERE type='trigger' AND name LIKE 'sessions_%'"):
    c.execute(f'DROP TRIGGER {n}')
c.execute("DELETE FROM meta WHERE key IN "
          "('sessions_started','sessions_ended','session_counts_floor')")
# 再補一場當掉的，讓升上來之後有東西可以數。
c.execute("INSERT INTO sessions(started_at, app_version, platform)"
          " VALUES (strftime('%s','now')*1000 - 600000, 'alpha.33', 'linux')")
c.execute('PRAGMA user_version = 5')
c.commit()
EOF
$U doctor > upgrade.txt
python3 -c "
import re, pathlib, sqlite3, sys
# **版號從 db.rs 挖出來，不在這裡抄第二份。** 抄一份的下場已經發生
# 過一次：SCHEMA_VERSION 升到 7 的那個 commit 把這一格弄紅了，而紅
# 的理由跟這顆 fixture 在問的事（升上來的那一顆數不數得準）完全無
# 關——底下 diff 那裡自己寫著「一條會因為無關的事情變紅的斷言，最後
# 一定會被人放寬到失效」。
# 自動跟著走不會放過「版號加了、migration 沒補」：那一種在 db.rs 的
# \`n => bail!\` 就炸了，而這一步裡每一顆資料庫**開起來都要走那個
# loop**，所以少一段的話連第一顆 fixture 都開不起來，整步就紅了。
# （實測：把 SCHEMA_VERSION 改成 8 而不補 migration，這一步 rc=1，
# 停在 \`open ./ci-erased/sister.db … schema 8 沒有對應的 migration\`。）
want = int(re.search(r'^pub const SCHEMA_VERSION: i32 = ([0-9]+);',
                     pathlib.Path('$ROOT/crates/sister-core/src/db.rs').read_text(),
                     re.M).group(1))
c = sqlite3.connect('./ci-upgrade/sister.db')
v = c.execute('PRAGMA user_version').fetchone()[0]
m = dict(c.execute(\"SELECT key, value FROM meta WHERE key LIKE 'session%'\"))
bad = []
if v != want: bad.append(f'升完之後版號停在 {v}，db.rs 說應該升到 {want}')
if m.get('session_counts_floor') != '1': bad.append('沒有按下 floor 旗標')
if (m.get('sessions_started'), m.get('sessions_ended')) != ('2', '1'):
    bad.append(f'回填數字不對：{m}')
if bad: print('::error::[升上來的] ' + '；'.join(bad)); sys.exit(1)"
grep -qE '零當機.*升上來那天' upgrade.txt \
  || { echo "::error::[升上來的] 那個數字是回填的，句子卻說得像量到的"; grep 零當機 upgrade.txt; exit 1; }
# 反面：全新的那一顆不准講這句。它的數字是精確的，多一句範圍聲明就是
# 替一個沒有的問題道歉——而一句每台機器上都有的但書，很快就沒有人讀。
if $B --data-dir ./ci-normal doctor | grep -q '升上來那天'; then
  echo "::error::[升上來的] 全新的資料庫也在替一個沒發生的升級道歉"
  exit 1
fi
# **旗標自己也有一個坑**：migration 重跑（版號蓋到一半被砍、然後自我
# 修復）不可以替一顆數得準的資料庫貼上「我數不準」。所以它要問「計數
# 器在不在」才按——這一條把那個問題釘住，兩個方向各跑一次。
python3 -c "
import sqlite3
for d in ('./ci-upgrade', './ci-normal'):
    c = sqlite3.connect(d + '/sister.db'); c.execute('PRAGMA user_version = 5'); c.commit()"
$U doctor > upgrade-again.txt
$B --data-dir ./ci-normal doctor > normal-again.txt
# 只比那兩列。整份報告拿去 `diff` 會被「已記錄 188.1 KB → 176.1 KB」
# 這種東西弄紅——WAL 什麼時候 checkpoint 不是這顆 fixture 在問的事，
# 而一條會因為無關的事情變紅的斷言，最後一定會被人放寬到失效。
for x in upgrade.txt upgrade-again.txt; do
  grep -E '零當機|上一次錄製' "$x" > "$x.two"
done
diff -q upgrade.txt.two upgrade-again.txt.two > /dev/null \
  || { echo "::error::[升上來的] migration 重跑之後那兩列改口了"; diff upgrade.txt.two upgrade-again.txt.two; exit 1; }
if grep -q '升上來那天' normal-again.txt; then
  echo "::error::[升上來的] migration 重跑替一顆數得準的資料庫貼上了「我數不準」"
  grep 零當機 normal-again.txt
  exit 1
fi

# **兩句補充貼在一起的時候，順序自己會說話。** 升上來的那一顆補一句
# 範圍聲明，正在錄的那一台補一句扣除聲明，而「這裡的數字」如果排在
# 「現在正在錄的那一場沒有算進去」後面，最近的先行詞就變成一句根本
# 沒有數字的話。兩句各自都是真的。
#
# 這一格單元測試釘得住位置，釘不住「真的執行檔會不會把兩句一起印出
# 來」——而那個 bug 就是在真的執行檔印出來之後才看見的。
python3 - <<'EOF'
import sqlite3, time, pathlib
now = int(time.time() * 1000)
c = sqlite3.connect('./ci-upgrade/sister.db')
c.execute("INSERT INTO sessions(started_at, app_version, platform)"
          " VALUES (?, 'ci', 'linux')", (now,))
c.commit()
pathlib.Path('./ci-upgrade/recording.beat').write_text(str(now))
EOF
$U doctor > upgrade-live.txt
python3 -c "
import sys
line = [l for l in open('upgrade-live.txt') if '零當機' in l]
if not line: print('::error::[升上來又正開著] 沒有零當機那一列'); sys.exit(1)
line = line[0]
for want in ('升上來那天', '現在正在錄'):
    if want not in line:
        print(f'::error::[升上來又正開著] 兩句補充少了「{want}」：{line.strip()}'); sys.exit(1)
if line.index('升上來那天') > line.index('現在正在錄'):
    print('::error::[升上來又正開著] 範圍聲明黏到一句沒有數字的話上面了：'
          + line.strip()); sys.exit(1)"

# ── fixture 9：**她正在開機，而最新那一列是上一次當機的殼。** ──
#
# `BootBeat` 一寫下心跳，`Db::open` 才開始跑（一顆大的資料庫要幾分
# 鐘），所以那段時間裡「有人佔著這個目錄」是真的、「她那一列在資料庫
# 裡」是假的。上面每一顆 fixture 的心跳第二欄都不是 `boot`——**這個
# 狀態在 CI 上一次都沒出現過**，而它印出來的是這樣：
#
#     ✗ 零當機     3 段錄製裡有 2 段沒有回來（最後一次 02:43:02）
#     ? 上一次錄製 02:43:03 開始，沒有收尾——她現在還在跑
#
# 分母扣掉了一場不存在的錄製（她的列還沒進來），而那一列明明是上一
# 次當機留下來的殼，卻被說成「她現在還在跑」。兩個布林湊不出三種答
# 案，所以 `crash_audit` 收的是 `Phase` 本人。
W="$B --data-dir ./ci-booting"
cp -r ./ci-normal ./ci-booting
python3 - <<'EOF'
import sqlite3, time, pathlib
now = int(time.time() * 1000)
c = sqlite3.connect('./ci-booting/sister.db')
# 十分鐘前當掉的那一場。觸發器會把 `sessions_started` 加一而不加
# `sessions_ended`——那就是一次真的當機在計數器上的樣子。
c.execute("INSERT INTO sessions(started_at, app_version, platform)"
          " VALUES (?, 'ci', 'linux')", (now - 600_000,))
c.commit()
# 第二欄是 `boot`：有人佔著，但她還在開資料庫，列還沒 INSERT。
pathlib.Path('./ci-booting/recording.beat').write_text(f'{now} boot')
EOF
$W doctor > booting.txt
# 那一次當機不可以因為她正在開機就消失。分母是 2，當機數是 1。
grep -qE '零當機.*2 段錄製裡有 1 段沒有回來' booting.txt \
  || { echo "::error::[開機中] 開機把上一次當機扣掉了"; grep 零當機 booting.txt; exit 1; }
# 而且不准說「扣掉了」——沒有東西可以扣，她的列還沒進來。
if grep -qE '零當機.*沒有算進去' booting.txt; then
  echo "::error::[開機中] 宣告扣掉了一場還不存在的錄製"
  grep 零當機 booting.txt; exit 1
fi
# 「有人佔著」和「這一列是他的」是兩題，而兩句話都有人印錯過。
grep -qE '上一次錄製.*她當掉了' booting.txt \
  || { echo "::error::[開機中] 上一次當機的殼被說成她現在還在跑"; grep -A1 上一次錄製 booting.txt; exit 1; }
grep -qE '上一次錄製.*正在起來' booting.txt \
  || { echo "::error::[開機中] 明明看得到一個 recorder 正在起來，卻說沒有"; grep -A1 上一次錄製 booting.txt; exit 1; }
for bad in '她現在還在跑' '沒有任何 recorder'; do
  if grep -E '上一次錄製' booting.txt | grep -q "$bad"; then
    echo "::error::[開機中] 「$bad」在開機那一段是假的"
    grep 上一次錄製 booting.txt; exit 1
  fi
done
# 那三列數的是**上一次**那一場（她這一場還沒有任何列），所以稱呼不
# 准跟著心跳改。
if grep -E '視窗焦點|輸入節奏|文字座標' booting.txt | grep -q '這一場'; then
  echo "::error::[開機中] 她的列還沒進來，那三列數的不可能是「這一場」"
  grep -E '視窗焦點|輸入節奏|文字座標' booting.txt; exit 1
fi
# **兩種心跳要真的分得開。** 同一顆資料庫，只把第二欄的 `boot` 拿
# 掉（＝她的列進來了、開始錄了），那兩列必須改口：當機那一場被扣
# 掉、「上一次」變成她自己。少了這一半，把 `Phase` 判斷寫成
# `beat.is_some()` 一樣綠。
python3 - <<'EOF'
import pathlib, time
pathlib.Path('./ci-booting/recording.beat').write_text(
    str(int(time.time() * 1000)))
EOF
$W doctor > booting-recording.txt
grep -qE '零當機.*沒有算進去' booting-recording.txt \
  || { echo "::error::[開機中→在錄] 她的列進來了卻沒有從分母扣掉"; grep 零當機 booting-recording.txt; exit 1; }
grep -qE '上一次錄製.*她現在還在跑' booting-recording.txt \
  || { echo "::error::[開機中→在錄] 心跳說在錄，那一列卻還說是當掉的"; grep -A1 上一次錄製 booting-recording.txt; exit 1; }
for pair in 零當機 上一次錄製; do
  if diff <(grep $pair booting.txt) <(grep $pair booting-recording.txt) > /dev/null; then
    echo "::error::[開機中→在錄] 「$pair」對兩種心跳印出同一句話"
    grep $pair booting.txt; exit 1
  fi
done

# ── fixture 10：**她正在錄人生第一場，而東西已經落地了。** ──
#
# `ci-live` 站的是隔壁那一格：它從 `ci-nulldata` 複製過來，
# `capture.enabled = false`，所以一列內容都沒有——`Emptiness` 讀到的
# 是 `Barren`。這一顆有內容，讀到的是 `Erased`，而 `crash_audit` 把
# 正在錄的那一場從 `started` 扣掉之後那個 0 就掉進「分母沒了」那一
# 段：
#
#     ✓ 已記錄     4 張畫面 · 9 段文字 · 172.0 KB
#     ? 零當機     那幾場的紀錄已經不在了（`forget` 或保留期），現在算不出來
#     ? 上一次錄製 02:52:51 開始，沒有收尾——她現在還在跑
#
# 一台從來沒刪過東西的機器被指控刪過東西，夾在「4 張畫面」和「她現
# 在還在跑」中間。**扣掉一個數字就是替那個數字的 0 造一個新的意思。**
F="$B --data-dir ./ci-firstlive"
$F replay "$ROOT"/scenarios/bill-lookup.json --interval-ms 500 > /dev/null
python3 - <<'EOF'
import sqlite3, time, pathlib
c = sqlite3.connect('./ci-firstlive/sister.db')
# 唯一那一場改成「還在跑」。計數器要跟著回到 0，因為那一場的
# `session_end` 從來沒發生過——手改的話兩邊要一起改，不然造出來的是
# 一顆真實世界到不了的資料庫。
c.execute('UPDATE sessions SET ended_at = NULL')
c.execute("UPDATE meta SET value = '0' WHERE key = 'sessions_ended'")
c.commit()
pathlib.Path('./ci-firstlive/recording.beat').write_text(
    str(int(time.time() * 1000)))
EOF
$F stats > first-stats.txt
$F doctor > first-doctor.txt
$F facts > first-facts.txt
$F query 電話 > first-query.txt
for x in first-stats.txt first-doctor.txt first-facts.txt first-query.txt; do
  # 同一組指控。她三秒前才開始人生第一場，`forget` 一次都沒跑過。
  if grep -qE "$LIE" "$x"; then
    echo "::error::[第一場正在錄] $x 指控了一個沒發生過的刪除"
    grep -nE "$LIE" "$x"
    exit 1
  fi
done
grep -qE '✓ +零當機' first-doctor.txt \
  || { echo "::error::[第一場正在錄] 在她之前沒有別的，那是真的零當機"; grep 零當機 first-doctor.txt; exit 1; }
grep -qE '零當機.*第一場' first-doctor.txt \
  || { echo "::error::[第一場正在錄] 沒說出這是她的第一場"; grep 零當機 first-doctor.txt; exit 1; }
# 而那三列數的正好是被扣掉的那一場。叫它「上一場」的話，問「那幾段
# 錄製裡哪一場是上一場」，答案是都不是。
if grep -E '視窗焦點|輸入節奏|文字座標' first-doctor.txt | grep -q '上一場'; then
  echo "::error::[第一場正在錄] 那三列把此刻正在錄的那一場叫成「上一場」"
  grep -E '視窗焦點|輸入節奏|文字座標' first-doctor.txt; exit 1
fi
grep -E '視窗焦點' first-doctor.txt | grep -q '這一場' \
  || { echo "::error::[第一場正在錄] 正反成對：也要說得出那是「這一場」"; grep 視窗焦點 first-doctor.txt; exit 1; }
# 讀 JSON 的那一邊也要答得出同一題——那一份以前只有 `scope_started_at`，
# 而一個時間戳說不出那一場結束了沒有。
$F stats --json > first-stats.json
python3 -c "
import json, sys
sig = json.load(open('first-stats.json'))['signals']
if not sig: print('::error::[第一場正在錄] JSON 裡沒有 signals'); sys.exit(1)
bad = [s['name'] for s in sig if not s.get('scope_is_live')]
if bad:
    print('::error::[第一場正在錄] JSON 說那一場不是活的：' + '、'.join(bad))
    sys.exit(1)"
# 反面：她停了之後，同一顆資料庫上那三列要回到「上一場」。
#
# 造的是**墓碑**而不是刪檔：乾淨收工留下來的就是這個（#66），刪檔那
# 一版現在只代表「這台機器從來沒跑過 recorder」——那不是這一格要演的
# 情況，而且是一台已經錄過東西的機器上再也不會出現的情況。
python3 -c "
import pathlib, time
pathlib.Path('./ci-firstlive/recording.beat').write_text(
    f'0 stopped {int(time.time() * 1000)}')"
$F doctor > first-stopped.txt
if grep -E '視窗焦點|輸入節奏|文字座標' first-stopped.txt | grep -q '這一場'; then
  echo "::error::[第一場停了] 沒有心跳了，那三列還說她在錄"
  grep -E '視窗焦點|輸入節奏|文字座標' first-stopped.txt; exit 1
fi

# ── fixture 11：**一列都沒存過，而一個 recorder 正在起來。** ──
#
# 第五顆（`ci-nulldata`：跑完了、什麼都沒存到 → 下一步是改設定）和第
# 六顆（`ci-live`：此刻正在錄 → 下一步是再等一下）中間還有第三格：心
# 跳在、第二欄是 `boot`，她還在開資料庫（`Db::open` 在一顆一年份的資
# 料庫上要跑好幾分鐘）。fixture 9 站的是「資料庫裡有東西」那一半，這
# 一顆站的是「空的」那一半——而那正是把心跳壓成布林的地方。上一版
# `doctor` 收到的是 `occupied = beat.is_some()`，於是這一格走進「她正
# 在錄」，同一份報告印出：
#
#     ? 已記錄     0 張畫面 · 0 段文字——她此刻正在錄，到現在還沒有一列落地
#     ? 上一次錄製 …… 她當掉了。現在有一個 sister record 正在起來
#
# 前一句說她在錄，四行之後說她還沒開始。**一個布林湊不出三種答案，而
# 少掉的那一種永遠是「正在來、還沒好」。** 三個畫面各自判斷，所以四個
# 指令都要問。
#
# 這一顆是第六顆**逐位元的複本**（在那四個 `$L` 指令動它之前複製走
# 的），只有心跳檔的第二欄多一個 `boot`。同一批位元組要說出相反的
# 話，而那一列 `ended_at IS NULL` 的 session 在兩顆上的意思剛好相反：
# 第六顆上那是**她自己**，這一顆上那是**上一次當機留下來的殼**——她那
# 一列還沒 INSERT（`BootBeat::start` 先寫心跳，`start_session` 最後才
# 跑）。分得出來的只有心跳的第二欄。
P="$B --data-dir ./ci-bootlive"
python3 - <<'EOF'
import pathlib, time
# 現寫：這個檔案 16 秒就過期，複製過來的那一份早就死了。
pathlib.Path('./ci-bootlive/recording.beat').write_text(
    f'{int(time.time() * 1000)} boot')
EOF
$P stats > boot-stats.txt
$P doctor > boot-doctor.txt
$P facts > boot-facts.txt
$P query 電話 > boot-query.txt
for x in boot-stats.txt boot-doctor.txt boot-facts.txt boot-query.txt; do
  # 同一組指控。一列都沒進來過，就沒有東西可以被忘掉或過期。
  if grep -qE "$LIE" "$x"; then
    echo "::error::[正在起來] $x 指控了一個沒發生過的刪除"
    grep -nE "$LIE" "$x"; exit 1
  fi
  # 第五顆的下一步在這一格是誤導：他機器上沒有設定要改，他在等那個
  # 正在起來的 recorder。
  if grep -q 'capture.enabled' "$x"; then
    echo "::error::[正在起來] $x 把一個正在等 recorder 起來的人送去改設定"
    grep -n 'capture.enabled' "$x"; exit 1
  fi
  # 第六顆的那句話在這一格是假的：她一拍都還沒跑。
  if grep -q '正在錄' "$x"; then
    echo "::error::[正在起來] $x 說她在錄，而她還在開資料庫"
    grep -n '正在錄' "$x"; exit 1
  fi
  # 正反成對——只寫「不准說」的話，整段功能被刪掉也是綠的。
  grep -q '正在起來' "$x" \
    || { echo "::error::[正在起來] $x 沒說出他在等什麼"; cat "$x"; exit 1; }
done
# 而三顆之間必須真的不一樣。和第五顆逐字相同＝那個 `boot` 完全沒被
# 讀到；和第六顆逐字相同＝三種狀態又被壓回兩種。`same` 會先把資料目
# 錄名和毫秒數抹掉——理由見上面第六顆那一段。
for pair in stats doctor facts query; do
  for other in null live; do
    if same "$other-$pair.txt" "boot-$pair.txt"; then
      echo "::error::[正在起來] $pair 和 $other 逐字相同"
      exit 1
    fi
  done
done
# **同一列 `ended_at IS NULL`，兩顆上是相反的兩件事。**
#
# `crash_audit` 會把「她此刻正在錄的那一場」從三個數字裡扣掉，而那道
# 扣除認人靠的是 `id = MAX(id)`。開機那幾分鐘裡坐在 `MAX(id)` 上的不
# 是她，是上一次當機留下來的殼——扣掉它，就是把一次真的當機宣告成
# 「她現在正在錄」。第六顆上那一列該被扣掉（✓ 零當機，而且要講出扣
# 了），這一顆上不准扣（✗ 零當機，1 段沒有回來，而且不准講扣除）。
#
# 上一版這三條釘在一顆 `sessions` 空的複本上：`newest_open` 永遠是
# false，於是 `live` 對任何心跳都是 false，`✓` 和「沒有算進去」兩條
# 都翻不成紅——一格空白，第四次出現在抓它的工具上。實測過：把
# `crash_audit` 那一行改成 `beat.is_some() && newest_open`，這一顆會
# 印出「✓ 零當機 1 段錄製全部正常收尾。現在正在錄的那一場沒有算進
# 去」，正上方兩列才說她還在開資料庫。
grep -qE '✗ +零當機' boot-doctor.txt \
  || { echo "::error::[正在起來] 上一次當機被算成了她此刻正在錄的那一場"; grep 零當機 boot-doctor.txt; exit 1; }
grep -E '零當機' boot-doctor.txt | grep -q '1 段沒有回來' \
  || { echo "::error::[正在起來] 那一次當機沒有被數進去"; grep 零當機 boot-doctor.txt; exit 1; }
if grep -qE '零當機.*沒有算進去' boot-doctor.txt; then
  echo "::error::[正在起來] 宣告扣掉了一場還不存在的錄製"
  grep 零當機 boot-doctor.txt; exit 1
fi
# 而 stats 上那一列空殼要說同一件事。整份檔案的「正在起來」被上面那
# 個 ⚠ 蓋過去了，所以這一列要單獨釘——它走的是 `session_shell_why`，
# 和 doctor 那兩列不同一條路（上一版它收的正是 `beat.is_some()`）。
grep -E '工作階段' boot-stats.txt | grep -q '正在起來' \
  || { echo "::error::[正在起來] 那個空殼沒說出他在等什麼"; grep 工作階段 boot-stats.txt; exit 1; }
grep -E '工作階段' boot-stats.txt | grep -q '當掉' \
  || { echo "::error::[正在起來] 那個空殼是上一次當機留下來的，不是她的"; grep 工作階段 boot-stats.txt; exit 1; }
# 正反成對：同一份位元組、同一列，第六顆上要說相反的話。
grep -E '工作階段' live-stats.txt | grep -q '正在錄' \
  || { echo "::error::[她正開著] 那一列是她自己的，不是當機留下來的殼"; grep 工作階段 live-stats.txt; exit 1; }
if grep -E '工作階段' live-stats.txt | grep -q '當掉'; then
  echo "::error::[她正開著] 把她此刻正在錄的那一場說成當掉了"
  grep 工作階段 live-stats.txt; exit 1
fi
# `forget` 是第五個 surface，而它是他**按下不可逆的按鈕之前**讀的那
# 一頁。「她現在還在錄，先 pause」在這一格是假的（她一列都還沒寫），
# 而下一步剛好一樣——所以錯的不是建議，是理由，而他會照著理由決定要
# 不要相信這一頁。fixture 3 釘了「她正在錄」那一半（`live-forget.txt`）。
# 這裡是預覽那條路，一個位元組都不會動。
$P forget --last 24h > boot-forget.txt
grep -q '正在起來' boot-forget.txt \
  || { echo "::error::[正在起來] forget 沒說出那個 recorder 還沒開始記"; cat boot-forget.txt; exit 1; }
if grep -qE '她現在還在錄|她剛才一直在錄' boot-forget.txt; then
  echo "::error::[正在起來] forget 說她在錄，而她還在開資料庫"
  grep -nE '她現在還在錄|她剛才一直在錄' boot-forget.txt; exit 1
fi
# 再切真的那一刀。這是**唯一**印得出「留著 1 場錄製的紀錄本身」整句
# 話的地方（預覽那條路走不到它，`stats` 那一列只取前半句），而那一句
# 的後半段是「什麼時候會走」——三種心跳三個下場，而這一格等的是**別
# 人**開始錄，不是她收工。少了這一條，把這裡的下一步換成活的那一格，
# unit 和 ci 雙雙活下來（實測過）。這一顆用完了，可以動它。
$P forget --last 24h --yes > boot-erased.txt
grep -q '留著 1 場錄製的紀錄本身' boot-erased.txt \
  || { echo "::error::[正在起來] 沒被帶走的那一列沒有被講出來"; cat boot-erased.txt; exit 1; }
grep -q '等它開始錄' boot-erased.txt \
  || { echo "::error::[正在起來] 那一列等的是那個 recorder 開始錄，不是她收工"; grep 留著 boot-erased.txt; exit 1; }
if grep -q '收工' boot-erased.txt; then
  echo "::error::[正在起來] 那一列不是她的，她收工不會帶走它"
  grep -n 收工 boot-erased.txt; exit 1
fi
# 正反成對：她真的在錄的那一顆上，同一句話要說相反的下一步。
# `live-forget.txt` 是 fixture 3 那一刀（`ci-crashed`，心跳寫在一小時
# 之後），上面那個迴圈已經釘過它說「她此刻正在錄」。
grep -q '收工' live-forget.txt \
  || { echo "::error::[她正開著] 那一列是她的，等的就是她收工"; grep 留著 live-forget.txt; exit 1; }
# 「現在有沒有在看」是這一頁上唯一直接回答「她此刻在幹嘛」的一列，
# 而它以前自己再讀一次心跳（於是同一份報告的上半和下半可以描述兩個
# 不同的瞬間）。上面那個迴圈抓的是整份檔案，這一列被別人的「正在起
# 來」蓋過去——要單獨釘。三種心跳三句話，這裡釘兩種。
if grep -E '現在有沒有在看' boot-doctor.txt | grep -q '正在跑'; then
  echo "::error::[正在起來] 「正在起來」印成「正在跑」，他會照著那句話去做一件想被記住的事"
  grep 現在有沒有在看 boot-doctor.txt; exit 1
fi
grep -E '現在有沒有在看' boot-doctor.txt | grep -q '正在起來' \
  || { echo "::error::[正在起來] 那一列沒說出她還在開資料庫"; grep 現在有沒有在看 boot-doctor.txt; exit 1; }
# 符號也要分得開：`…`（還沒好）不是 `✓`（好了）。符號印在標籤**前
# 面**（見 `signal_audit` 的註解），這兩條都當場對著真的輸出驗過。
grep -qE '… +現在有沒有在看' boot-doctor.txt \
  || { echo "::error::[正在起來] 「正在起來」畫成了打勾"; grep 現在有沒有在看 boot-doctor.txt; exit 1; }
# 正反成對：她真的在錄的那一顆上，同一列要說「正在跑」而且是 ✓。
grep -qE '✓ +現在有沒有在看' live-doctor.txt \
  || { echo "::error::[她正開著] 她真的在錄，那一列要是打勾"; grep 現在有沒有在看 live-doctor.txt; exit 1; }
grep -E '現在有沒有在看' live-doctor.txt | grep -q '正在跑' \
  || { echo "::error::[她正開著] 她真的在錄，那一列要說「正在跑」"; grep 現在有沒有在看 live-doctor.txt; exit 1; }

# `sister stop` 也有三種狀態，而它只需要一個心跳檔——不必資料庫，所以
# 三顆現造。上一版它問的是 `is_occupied`，於是開機那幾分鐘印的是「會在
# 下一個 tick 把 session 寫完再結束」：那時候既沒有 session 也沒有
# tick。而且那個請求接下來會被她自己刪掉（清理排在 `Db::open` 之後）
# ——**兩個畫面都說收到了，然後她錄一整天**。
# 第四顆 `stopped` 是 #66 加的，而它是**真實世界裡最常見的那一顆**：
# 任何錄過一次又乾淨收工的機器都停在這個狀態。`none`（檔案真的不在）
# 從此只發生在全新安裝上。兩顆都要答「目前沒有任何」，而它們走的是
# 不同的分支——只留 `none` 的話，那條最常走的路一次都沒被測過。
for kind in live boot none stopped; do
  d="./ci-stop-$kind"; mkdir -p "$d"
  case "$kind" in
    live) python3 -c "import time;open('$d/recording.beat','w').write(str(int(time.time()*1000)))" ;;
    boot) python3 -c "import time;open('$d/recording.beat','w').write(f'{int(time.time()*1000)} boot')" ;;
    none) rm -f "$d/recording.beat" ;;
    stopped) python3 -c "import time;open('$d/recording.beat','w').write(f'0 stopped {int(time.time()*1000)}')" ;;
  esac
  $B --data-dir "$d" stop > "stop-$kind.txt"
  # 三種下場都要真的把請求寫下去。少了這一條，一句「已經請她收工」
  # 可以完全不做事還是綠的。
  test -f "$d/stop.request" \
    || { echo "::error::[stop/$kind] 話說了，請求沒寫下去"; cat "stop-$kind.txt"; exit 1; }
done
grep -q '還在開資料庫' stop-boot.txt \
  || { echo "::error::[stop/開機中] 沒說出她還在開資料庫"; cat stop-boot.txt; exit 1; }
if grep -q '下一個 tick' stop-boot.txt; then
  echo "::error::[stop/開機中] 主迴圈還沒開始，沒有「下一個 tick」也沒有 session"
  cat stop-boot.txt; exit 1
fi
# 正反成對：她真的在錄的那一顆上，同一句話要換成另外一句。
grep -q '下一個 tick' stop-live.txt \
  || { echo "::error::[stop/她正開著] 主迴圈在跑，收工就在下一個 tick"; cat stop-live.txt; exit 1; }
if grep -q '還在開資料庫' stop-live.txt; then
  echo "::error::[stop/她正開著] 她早就開完了"; cat stop-live.txt; exit 1
fi
# 沒有人在的兩種——全新的機器（`none`）和收工過的機器（`stopped`）
# ——要給同一個答案。第二種是他每天早上開機時的樣子。
for kind in none stopped; do
  grep -q '目前沒有任何' "stop-$kind.txt" \
    || { echo "::error::[stop/$kind] 沒有人在跑，不可以講得像攔下了誰"; cat "stop-$kind.txt"; exit 1; }
  if grep -q '已經請她收工' "stop-$kind.txt"; then
    echo "::error::[stop/$kind] 沒有人可以收工"; cat "stop-$kind.txt"; exit 1
  fi
done
