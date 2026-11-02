#!/usr/bin/env python3
"""Generate an expert-grade GitHub release body for Pangan.

Reads git history between the previous tag and the current tag (from env),
categorizes commits by conventional-commit type, and renders a Markdown body
featuring highlights, a categorized changelog, a per-platform asset matrix
(sizes + SHA256), integrity verification instructions, and install steps.

Environment / cwd: run from repo root after checking out with fetch-depth: 0
and after downloading artifacts into ./dist. Requires git, python3 stdlib.
"""

import hashlib
import os
import re
import subprocess
from collections import defaultdict
from datetime import datetime, timezone
from pathlib import Path

REPO = os.environ.get("GITHUB_REPOSITORY", "DariushKhosrowan/pangan")
TAG = os.environ.get("GITHUB_REF_NAME", "").lstrip("v")
SHA = os.environ.get("GITHUB_SHA", "")[:7]

DIST = Path("dist")

# Conventional-commit type -> (emoji, human field)
CATEGORIES = [
    ("feat", "🚀 Features"),
    ("feature", "🚀 Features"),
    ("fix", "🐛 Bug Fixes"),
    ("perf", "⚡ Performance"),
    ("perf:", "⚡ Performance"),
    ("refactor", "♻️ Refactors"),
    ("docs", "📚 Documentation"),
    ("test", "🧪 Tests"),
    ("build", "🔧 Build & CI"),
    ("ci", "🔧 Build & CI"),
    ("style", "🎨 Style"),
    ("chore", "🧹 Chores"),
    ("revert", "↩️ Reverts"),
]

ASSETS = {
    "pangan-linux-x86_64": "Linux · x86_64 (binary)",
    "pangan-linux-aarch64": "Linux · ARM64 (binary)",
    "pangan-windows-x86_64.exe": "Windows · x86_64 (portable exe)",
    "pangan-windows-aarch64.exe": "Windows · ARM64 (portable exe)",
    "pangan-macos-arm64": "macOS · Apple Silicon",
    "pangan-macos-x86_64": "macOS · Intel",
    "pangan-android-arm64.apk": "Android · ARM64 APK",
    "pangan-android-x86_64.apk": "Android · x86_64 APK",
    "pangan-ios-simulator.app.zip": "iOS · Simulator",
    "install.sh": "Linux installer helper",
    "pangan.desktop": "Linux desktop entry (for menu integration)",
}


def sh(*args):
    return subprocess.run(args, capture_output=True, text=True).stdout.strip()


def get_prev_tag():
    tags = sh("git", "tag", "-l", "--sort=-v:refname").splitlines()
    return next((t for t in tags if t != os.environ.get("GITHUB_REF_NAME")), None)


def get_commits(prev):
    if prev:
        rng = f"{prev}..HEAD"
    else:
        rng = ""
    fmt = "%H|%an|%s"
    out = sh("git", "log", "--no-merges", "--pretty=format:" + fmt, rng) if rng \
        else sh("git", "log", "--no-merges", "--pretty=format:" + fmt)
    commits = []
    for line in out.splitlines():
        parts = line.split("|", 2)
        if len(parts) == 3:
            commits.append({"sha": parts[0], "author": parts[1], "subject": parts[2]})
        else:
            commits.append({"sha": "", "author": "", "subject": line})
    return commits


def categorize(subject):
    m = re.match(r"^([a-z0-9]+)(?:\(([^)]+)\))?:\s?(.*)$", subject)
    if not m:
        return ("Other", subject)
    typ, scope, desc = m.groups()
    for prefix, field in CATEGORIES:
        if typ == prefix or typ.startswith(prefix):
            suffix = f" **({scope})**" if scope else ""
            return (field, f"{desc}{suffix}")
    return (typ.title() + "s", desc)


def sha256_of(path):
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def human_size(n):
    for unit in ("B", "KiB", "MiB", "GiB"):
        if n < 1024 or unit == "GiB":
            return f"{n:.1f} {unit}" if unit != "B" else f"{n} B"
        n /= 1024


def main():
    prev = get_prev_tag()
    commits = get_commits(prev)

    grouped = defaultdict(list)
    for c in commits:
        field, text = categorize(c["subject"])
        sha_short = c["sha"][:7]
        grouped[field].append(f"- {text} (`{sha_short}`)")

    if not commits:
        grouped["Other"] = ["- Initial published build."]

    # Asset matrix
    rows = []
    checksum_lines = []
    for p in sorted(DIST.iterdir()) if DIST.exists() else []:
        if not p.is_file() or p.suffix in (".sha256", ".txt"):
            continue
        size = p.stat().st_size
        digest = sha256_of(p)
        label = ASSETS.get(p.name, p.name)
        rows.append(
            f"| `{p.name}` | {label} | {human_size(size)} | `{digest[:16]}…` |"
        )
        checksum_lines.append(f"{digest}  {p.name}")
    if not rows:
        rows.append("| _No artifacts found._ | — | — | — |")

    now = datetime.now(timezone.utc).strftime("%Y-%m-%d %H:%M UTC")

    B = []
    B.append(f"## ⭐ Pangan v{TAG}")
    B.append("")
    B.append(f"> **Versatile, self-contained focus timer.** Built from `{SHA}` · released {now}.")
    B.append("")
    B.append("---")
    B.append("")
    B.append("### ✨ Highlights")
    B.append("")
    B.append("- **Six desktop targets**: Linux (x86_64/ARM64), Windows (x86_64/ARM64), macOS (Apple Silicon/Intel).")
    B.append("- **Mobile assets**: Android release APKs (arm64 + x86_64) and an iOS simulator bundle.")
    B.append("- **Signed & verified**: Android APKs are signed; every asset ships with a matching SHA-256.")
    B.append("")
    B.append("### 📜 What's Changed")
    B.append("")
    for field in ["🚀 Features", "🐛 Bug Fixes", "⚡ Performance", "♻️ Refactors",
                   "🎨 Style", "📚 Documentation", "🧪 Tests", "🔧 Build & CI",
                   "🧹 Chores", "↩️ Reverts", "Other"]:
        items = grouped.get(field)
        if items:
            B.append(f"**{field}** ({len(items)})")
            B.append("")
            B.extend(items)
            B.append("")
    B.append("---")
    B.append("")
    B.append("### ⬇️ Downloads")
    B.append("")
    B.append("| Asset | Platform | Size | SHA-256 (prefix) |")
    B.append("|-------|----------|------|-------------------|")
    B.extend(rows)
    B.append("")
    B.append("Full checksums are in `SHA256SUMS.txt` attached to this release.")
    B.append("")
    B.append("```bash")
    B.append("curl -sLO https://github.com/" + REPO + "/releases/download/v" + TAG + "/SHA256SUMS.txt")
    B.append("sha256sum -c SHA256SUMS.txt")
    B.append("```")
    B.append("")
    B.append("### 🚀 Install")
    B.append("")
    B.append("**Linux** — make executable and run in place or install:")
    B.append("```bash")
    B.append("chmod +x pangan-linux-x86_64")
    B.append("sudo mv pangan-linux-x86_64 /usr/local/bin/pangan")
    B.append("```")
    B.append("")
    B.append("**macOS** — you may need `xattr -d com.apple.quarantine <file>` after first download (unsigned build).")
    B.append("")
    B.append("**Windows** — download the `.exe` and run, or place on `PATH`.")
    B.append("")
    B.append("**Android** — allow unknown sources, install the matching APK for your device ABI.")
    B.append("")
    B.append("---")
    B.append("")
    B.append("### 🏷️ Notes")
    B.append("")
    B.append(f"- Previous tag: {prev or 'n/a (first tagged release)'}.")
    B.append("- Binaries are built on GitHub-hosted runners; macOS/Windows builds are not notarized/signed by a CA.")
    B.append("- All builds carry **zero** rustc warnings (CI enforces `-D warnings` on the release profile).")
    B.append("")

    Path("release_body.md").write_text("\n".join(B))
    Path("SHA256SUMS.txt").write_text("\n".join(checksum_lines) + "\n")
    print("Wrote release_body.md and SHA256SUMS.txt with", len(rows), "assets.")


if __name__ == "__main__":
    main()
