#!/usr/bin/env python3

from __future__ import annotations

import importlib.util
import json
import pathlib
import tempfile
import unittest


SCRIPT = pathlib.Path(__file__).parents[1] / "build-website.py"
SPEC = importlib.util.spec_from_file_location("build_website", SCRIPT)
assert SPEC is not None and SPEC.loader is not None
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)


class WebsiteBuildTests(unittest.TestCase):
    def test_build_binds_current_release_and_all_personas(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            output = pathlib.Path(directory) / "site"
            MODULE.build(output)

            version = MODULE.product_version()
            html = (output / "index.html").read_text(encoding="utf-8")
            self.assertIn(f"v{version}", html)
            self.assertIn(
                f"/releases/download/v{version}/AI-Sister-Setup.exe", html
            )
            self.assertIn(
                f"/releases/download/v{version}/AI-Sister-Linux-X11-amd64.deb",
                html,
            )
            self.assertFalse(any(token in html for token in MODULE.TOKENS))

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
            "        uses: actions/configure-pages@v5",
            "        uses: actions/upload-pages-artifact@v4",
            "        uses: actions/deploy-pages@v4",
        ]
        positions = []
        for fragment in required:
            self.assertEqual(website.count(fragment), 1, fragment)
            positions.append(website.index(fragment))
        self.assertEqual(positions, sorted(positions))


if __name__ == "__main__":
    unittest.main()
