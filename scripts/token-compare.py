#!/usr/bin/env python3
"""Compare token usage between raw HTML and cortex-browser snapshots.

Usage:
    # Run against built-in test fixtures
    python3 scripts/token-compare.py

    # Run against specific HTML files
    python3 scripts/token-compare.py page.html other.html

    # Run against live URLs (requires Chrome + cortex-browser built)
    python3 scripts/token-compare.py https://example.com https://github.com

    # Mix files and URLs
    python3 scripts/token-compare.py tests/fixtures/blog.html https://example.com

Install tiktoken for accurate counts (optional):
    pip install tiktoken
"""

import os
import subprocess
import sys
import urllib.request

try:
    import tiktoken

    _encoder = tiktoken.get_encoding("cl100k_base")

    def count_tokens(text: str) -> int:
        return len(_encoder.encode(text))

    TOKENIZER = "tiktoken (cl100k_base)"
except ImportError:
    def count_tokens(text: str) -> int:
        """Approximate token count: split on whitespace and punctuation boundaries.

        This gives a rough estimate (~1.3x actual for English, closer for code/HTML).
        Install tiktoken for accurate counts: pip install tiktoken
        """
        count = 0
        in_word = False
        for ch in text:
            if ch.isalnum() or ch == '_':
                if not in_word:
                    count += 1
                    in_word = True
            else:
                if in_word:
                    in_word = False
                if not ch.isspace():
                    count += 1
        return count

    TOKENIZER = "approximate (install tiktoken for accuracy: pip install tiktoken)"


ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
BINARY = os.path.join(ROOT, "target", "release", "cortex-browser")
FIXTURES_DIR = os.path.join(ROOT, "tests", "fixtures")


def find_binary() -> str:
    if os.path.isfile(BINARY):
        return BINARY
    debug = BINARY.replace("/release/", "/debug/")
    if os.path.isfile(debug):
        return debug
    print("Error: cortex-browser binary not found. Run `make release` first.", file=sys.stderr)
    sys.exit(1)


def snapshot_file(binary: str, path: str) -> str:
    result = subprocess.run(
        [binary, "snapshot", path],
        capture_output=True, text=True, timeout=30,
    )
    if result.returncode != 0:
        raise RuntimeError(f"snapshot failed for {path}: {result.stderr.strip()}")
    return result.stdout


def snapshot_url(binary: str, url: str) -> str:
    result = subprocess.run(
        [binary, "snapshot", url, "--launch"],
        capture_output=True, text=True, timeout=60,
    )
    if result.returncode != 0:
        raise RuntimeError(f"snapshot failed for {url}: {result.stderr.strip()}")
    return result.stdout


def fetch_url_html(url: str) -> str:
    # Try curl first (handles SSL better on macOS)
    try:
        result = subprocess.run(
            ["curl", "-sL", "-m", "30", "-A", "Mozilla/5.0", url],
            capture_output=True, text=True, timeout=35,
        )
        if result.returncode == 0 and result.stdout:
            return result.stdout
    except (FileNotFoundError, subprocess.TimeoutExpired):
        pass
    # Fallback to urllib
    req = urllib.request.Request(url, headers={"User-Agent": "Mozilla/5.0"})
    with urllib.request.urlopen(req, timeout=30) as resp:
        return resp.read().decode("utf-8", errors="replace")


def format_number(n: int) -> str:
    if n >= 1_000_000:
        return f"{n / 1_000_000:.1f}M"
    if n >= 1_000:
        return f"{n / 1_000:.1f}k"
    return str(n)


def format_pct(ratio: float) -> str:
    pct = (1.0 - ratio) * 100
    return f"{pct:.1f}%"


def main():
    binary = find_binary()
    inputs = sys.argv[1:]

    # Default: use test fixtures + Wikipedia as a live example
    if not inputs:
        inputs = sorted(
            os.path.join(FIXTURES_DIR, f)
            for f in os.listdir(FIXTURES_DIR)
            if f.endswith(".html")
        )
        inputs.append("https://www.wikipedia.org")
        if not inputs:
            print("No fixtures found and no inputs provided.", file=sys.stderr)
            sys.exit(1)

    print(f"Tokenizer: {TOKENIZER}\n")

    # Table header
    header = f"{'Source':<35} {'Raw HTML':>12} {'Cortex':>12} {'Ratio':>8} {'Saved':>8}"
    print(header)
    print("─" * len(header))

    total_raw = 0
    total_cortex = 0

    for inp in inputs:
        is_url = inp.startswith("http://") or inp.startswith("https://")
        label = os.path.basename(inp) if not is_url else inp.split("//", 1)[1][:33]

        try:
            if is_url:
                raw_html = fetch_url_html(inp)
                cortex_output = snapshot_url(binary, inp)
            else:
                raw_html = open(inp).read()
                cortex_output = snapshot_file(binary, inp)

            raw_tokens = count_tokens(raw_html)
            cortex_tokens = count_tokens(cortex_output)
            ratio = cortex_tokens / raw_tokens if raw_tokens > 0 else 0.0

            total_raw += raw_tokens
            total_cortex += cortex_tokens

            print(
                f"{label:<35} {format_number(raw_tokens):>12} {format_number(cortex_tokens):>12}"
                f" {ratio:>7.2f}x {format_pct(ratio):>8}"
            )
        except Exception as e:
            print(f"{label:<35} {'ERROR':>12}   {e}", file=sys.stderr)

    if len(inputs) > 1 and total_raw > 0:
        total_ratio = total_cortex / total_raw
        print("─" * len(header))
        print(
            f"{'TOTAL':<35} {format_number(total_raw):>12} {format_number(total_cortex):>12}"
            f" {total_ratio:>7.2f}x {format_pct(total_ratio):>8}"
        )

    print()
    print(f"Raw HTML tokens:    {total_raw:,}")
    print(f"Cortex tokens:      {total_cortex:,}")
    if total_raw > 0:
        print(f"Tokens saved:       {total_raw - total_cortex:,} ({format_pct(total_cortex / total_raw)} reduction)")


if __name__ == "__main__":
    main()
