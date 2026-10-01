#!/usr/bin/env python3

from __future__ import annotations

import hashlib
import importlib.util
import json
import re
import pathlib
import shutil
import tempfile
import unittest


SCRIPT = pathlib.Path(__file__).parents[1] / "build-website.py"
SPEC = importlib.util.spec_from_file_location("build_website", SCRIPT)
assert SPEC is not None and SPEC.loader is not None
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)

PAGES_URL = "https://teddashh.github.io/AI-Sister/"


def one(pattern: str, html: str) -> str:
    found = re.findall(pattern, html)
    if len(found) != 1:
        raise AssertionError(f"{pattern!r} 應恰好出現一次，實際 {len(found)} 次")
    return found[0]


class WebsiteBuildTests(unittest.TestCase):
    def test_build_binds_current_release_and_all_personas(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            output = pathlib.Path(directory) / "site"
            MODULE.build(output)

            version = MODULE.product_version()
            self.assertEqual(sorted(MODULE.PAGES), ["en/index.html", "index.html"])
            pages = {
                page: (output / page).read_text(encoding="utf-8")
                for page in MODULE.PAGES
            }
            for page, html in pages.items():
                with self.subTest(page=page):
                    self.assertIn(f"v{version}", html)
                    self.assertIn(
                        f"/releases/download/v{version}/AI-Sister-Setup.exe", html
                    )
                    self.assertIn(
                        f"/releases/download/v{version}/AI-Sister-Linux-X11-amd64.deb",
                        html,
                    )
                    self.assertIn("macOS 14+", html)
                    self.assertIn(
                        f"/tree/v{version}/.claude/skills/ai-sister-memory", html
                    )
                    self.assertIn(
                        f"/tree/v{version}/.agents/skills/ai-sister-memory", html
                    )
                    self.assertFalse(any(token in html for token in MODULE.TOKENS))
            self.assertIn("尚無公開安裝檔", pages["index.html"])
            self.assertIn("No public installer yet", pages["en/index.html"])

            claude_skill = (
                MODULE.ROOT / ".claude/skills/ai-sister-memory/SKILL.md"
            ).read_text(encoding="utf-8")
            codex_skill = (
                MODULE.ROOT / ".agents/skills/ai-sister-memory/SKILL.md"
            ).read_text(encoding="utf-8")
            for skill in (claude_skill, codex_skill):
                self.assertIn("name: ai-sister-memory", skill)
                self.assertIn("sister query --limit 10 --json", skill)
                self.assertIn("Do not read `sister.db` or `frames/` directly", skill)
                self.assertNotIn("[TODO:", skill)

            personas = sorted((output / "assets/personas").glob("*.webp"))
            self.assertEqual(len(personas), 17)
            source_manifest = json.loads(
                (MODULE.PERSONAS / "manifest.json").read_text(encoding="utf-8")
            )
            self.assertEqual(
                sum(path.stat().st_size for path in personas),
                source_manifest["totals"]["webpBytes"],
            )
            self.assertTrue((output / ".nojekyll").is_file())

    def test_the_notice_the_public_downloads_is_still_true_where_it_lands(self) -> None:
        """網站那份 NOTICE 說「這份授權只對下面這份 exact manifest 生效」。

        `build-website.py` 是 `copy2` 兩個檔案過去的（manifest 與 NOTICE），
        那句話因此在**新的位置**重新被宣稱一次。`check-notice-claims-are-
        accounted-for.py` 掃的是 git 追蹤的檔案，掃不到建出來的網站；上面那條
        既有測試只比 17 張的**總 bytes**，也完全沒讀那份 NOTICE。

        「總和看不出哪一張被換掉」是量出來的，不是推的：讓 build 把 kimi 和
        grok 兩張的內容互換（總 bytes 一個位元組都沒變），既有那條**綠的**，
        這一條紅在 `kimi.webp 不是 manifest 列的那一份`。公開的網站上兩個角色
        的臉會對調，而舊斷言看不見。
        """
        with tempfile.TemporaryDirectory() as raw:
            output = pathlib.Path(raw) / "site"
            MODULE.build(output)
            shipped = output / "assets/personas"
            manifest = json.loads((shipped / "manifest.json").read_text(encoding="utf-8"))
            notice = (shipped / "NOTICE.md").read_text(encoding="utf-8")

            quoted = re.findall(r"\b[0-9a-f]{64}\b", notice)
            self.assertEqual(
                quoted,
                [hashlib.sha256((shipped / "manifest.json").read_bytes()).hexdigest()],
                "網站那份 NOTICE 引的 SHA-256 不是它旁邊那份 manifest 的",
            )

            said = re.search(r"的 (\d+) 張 WebP", notice)
            self.assertIsNotNone(said, "網站那份 NOTICE 沒說幾張，等於沒有這個宣稱")
            listed = {row["file"]: row["sha256"] for row in manifest["assets"]}
            self.assertEqual(int(said.group(1)), len(listed))

            for name, want in sorted(listed.items()):
                got = hashlib.sha256((shipped / name).read_bytes()).hexdigest()
                self.assertEqual(got, want, f"網站上的 {name} 不是 manifest 列的那一份")

            self.assertEqual(
                sorted(p.name for p in shipped.iterdir()),
                sorted([*listed, "NOTICE.md", "manifest.json"]),
                "NOTICE 說這裡只有那些檔案，而公開的資料夾裡不是",
            )

    def test_both_languages_point_at_each_other_and_share_one_page(self) -> None:
        """英文頁是中文頁的翻譯，不是另一個網站：同一組 CSP、同一組段落與角色、
        同樣多的下載連結，hreflang 與頁首的 繁中／EN 互相指到對方。"""
        with tempfile.TemporaryDirectory() as directory:
            output = pathlib.Path(directory) / "site"
            MODULE.build(output)
            zh = (output / "index.html").read_text(encoding="utf-8")
            en = (output / "en/index.html").read_text(encoding="utf-8")

            self.assertEqual(one(r'<html lang="([^"]+)"', zh), "zh-Hant")
            self.assertEqual(one(r'<html lang="([^"]+)"', en), "en")

            csp = r'http-equiv="Content-Security-Policy"\s+content="([^"]+)"'
            self.assertEqual(one(csp, en), one(csp, zh))
            self.assertIn("script-src 'self'", one(csp, zh))

            alternates = r'<link rel="alternate" hreflang="([^"]+)" href="([^"]+)"'
            expected = {
                "zh-Hant": PAGES_URL,
                "en": f"{PAGES_URL}en/",
                "x-default": f"{PAGES_URL}en/",
            }
            for html in (zh, en):
                pairs = re.findall(alternates, html)
                self.assertEqual(len(pairs), len(expected))
                self.assertEqual(dict(pairs), expected)
            self.assertEqual(one(r'<link rel="canonical" href="([^"]+)"', zh), PAGES_URL)
            self.assertEqual(
                one(r'<link rel="canonical" href="([^"]+)"', en), f"{PAGES_URL}en/"
            )

            switch = r'<nav class="lang-switch"[^>]*>(.*?)</nav>'
            zh_switch = one(switch, zh.replace("\n", " "))
            en_switch = one(switch, en.replace("\n", " "))
            self.assertIn('<span aria-current="true">繁中</span>', zh_switch)
            self.assertIn('<a href="./en/" hreflang="en"', zh_switch)
            self.assertIn('<span aria-current="true">EN</span>', en_switch)
            self.assertIn('<a href="../" hreflang="zh-Hant"', en_switch)

            for pattern in (
                r'<section[^>]*\bid="([^"]+)"',
                r'(?<![\w-])id="([^"]+)"',
                r'data-persona="([a-z0-9]+)"',
                r'href="(#[^"]+)"',
            ):
                with self.subTest(pattern=pattern):
                    self.assertEqual(re.findall(pattern, en), re.findall(pattern, zh))
            for url in ("AI-Sister-Setup.exe", "AI-Sister-Linux-X11-amd64.deb"):
                self.assertEqual(en.count(url), zh.count(url))

            for html in (zh, en):
                self.assertIn('href="https://github.com/teddashh/AI-Sister/releases"', html)
            self.assertIn('src="../assets/personas/chatgpt.webp"', en)
            self.assertIn('href="../styles.css"', en)
            self.assertIn('src="../site.js"', en)

    def test_every_page_gets_the_same_build_guarantees(self) -> None:
        """每一條只對 index.html 成立的檢查，對 en/index.html 也要紅得起來。"""

        def drop_linux_link(html: str) -> str:
            return html.replace("__LINUX_DOWNLOAD_URL__", "#download")

        def drop_one_persona(html: str) -> str:
            return re.sub(r'\s*<button[^>]*data-persona="mimo".*?</button>', "", html, flags=re.S)

        def point_at_a_missing_file(html: str) -> str:
            return html.replace('href="../styles.css"', 'href="../style.css"')

        def break_an_anchor(html: str) -> str:
            return html.replace('href="#privacy"', 'href="#boundaries"')

        cases = [
            ("缺少 __LINUX_DOWNLOAD_URL__", drop_linux_link),
            ("picker 與 persona manifest 不符", drop_one_persona),
            ("output 裡沒有的檔案：../style.css", point_at_a_missing_file),
            ("#boundaries 在同一頁找不到 id", break_an_anchor),
        ]
        for message, mutate in cases:
            with self.subTest(message=message), tempfile.TemporaryDirectory() as raw:
                source = pathlib.Path(raw) / "source"
                shutil.copytree(MODULE.SOURCE, source)
                page = source / "en/index.html"
                before = page.read_text(encoding="utf-8")
                after = mutate(before)
                self.assertNotEqual(after, before, "這一刀沒有切到東西")
                page.write_text(after, encoding="utf-8")
                with self.assertRaisesRegex(ValueError, re.escape(message)):
                    MODULE.build(pathlib.Path(raw) / "site", source=source)

        for stray in ("fr/index.html", "en/draft.html"):
            with self.subTest(stray=stray), tempfile.TemporaryDirectory() as raw:
                source = pathlib.Path(raw) / "source"
                shutil.copytree(MODULE.SOURCE, source)
                (source / stray).parent.mkdir(parents=True, exist_ok=True)
                (source / stray).write_text("<!doctype html>", encoding="utf-8")
                output = pathlib.Path(raw) / "site"
                with self.assertRaisesRegex(ValueError, "發布白名單不符"):
                    MODULE.build(output, source=source)
                self.assertFalse(output.exists())

    def test_existing_output_is_never_overwritten(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            output = pathlib.Path(directory) / "site"
            output.mkdir()
            sentinel = output / "keep.txt"
            sentinel.write_text("keep", encoding="utf-8")
            with self.assertRaisesRegex(ValueError, "output 已存在"):
                MODULE.build(output)
            self.assertEqual(sentinel.read_text(encoding="utf-8"), "keep")

    def test_pages_deploys_only_after_the_release_job(self) -> None:
        workflow = (MODULE.ROOT / ".github/workflows/ci.yml").read_text(encoding="utf-8")
        marker = "\n  website:\n"
        self.assertEqual(workflow.count(marker), 1)
        website = workflow.split(marker, maxsplit=1)[1]
        required = [
            "    needs: [release]",
            "    if: startsWith(github.ref, 'refs/tags/v')",
            "      contents: read",
            "      pages: write",
            "      id-token: write",
            "      name: github-pages",
            "      url: ${{ steps.deployment.outputs.page_url }}",
            "          python3 ./scripts/build-website.py --output ./_site",
            "        uses: actions/configure-pages@v6",
            "        uses: actions/upload-pages-artifact@v5",
            "        uses: actions/deploy-pages@v5",
        ]
        positions = []
        for fragment in required:
            self.assertEqual(website.count(fragment), 1, fragment)
            positions.append(website.index(fragment))
        self.assertEqual(positions, sorted(positions))


if __name__ == "__main__":
    unittest.main()
