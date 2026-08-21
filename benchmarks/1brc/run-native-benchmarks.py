#!/usr/bin/env python3
"""Run the 1BRC benchmark suite natively on macOS.

This runner avoids Docker-specific paths and GNU tools. It intentionally skips
PicoLisp because there is no Homebrew core formula for native macOS.
"""

from __future__ import annotations

import json
import hashlib
import os
import platform
import re
import shutil
import subprocess
import sys
import time
from dataclasses import dataclass
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
BENCH = ROOT / "benchmarks" / "1brc"
SIMPLE = BENCH / "simple"
DATA_DEFAULT = ROOT / "benchmarks" / "data" / "bench-10m.txt"
RESULTS = BENCH / "results" / "native-macos-arm64"
BUILD = RESULTS / "build"
BREW = Path("/opt/homebrew/bin")
GAMBIT = Path("/opt/homebrew/opt/gambit-scheme/bin")
SEMA = ROOT / "target" / "release" / "sema"
JAVA_US_LOCALE = {"JAVA_TOOL_OPTIONS": "-Duser.language=en -Duser.country=US"}
KEEP_OUTPUTS = os.environ.get("SEMA_BENCH_KEEP_OUTPUTS") == "1"


@dataclass
class Bench:
    name: str
    cmd: list[str]
    env: dict[str, str] | None = None


def brew(name: str) -> str:
    return str(BREW / name)


def gambit(name: str) -> str:
    return str(GAMBIT / name)


def run_checked(cmd: list[str], cwd: Path = BENCH, env: dict[str, str] | None = None) -> None:
    print("$ " + " ".join(cmd))
    subprocess.run(cmd, cwd=cwd, env=env, check=True)


def ensure_sema_current() -> str:
    # SEMA_SKIP_BUILD=1 benchmarks the existing target/release/sema as-is — used
    # to measure a PGO build (`jake build-pgo`), which a plain rebuild would clobber.
    if not os.environ.get("SEMA_SKIP_BUILD"):
        run_checked(["cargo", "build", "--release", "-p", "sema-lang"], cwd=ROOT)

    proc = subprocess.run([str(SEMA), "--version"], text=True, stdout=subprocess.PIPE, check=True)
    sema_version = proc.stdout.strip()

    cargo_toml = (ROOT / "Cargo.toml").read_text()
    match = re.search(r'(?m)^version = "([^"]+)"', cargo_toml)
    workspace_version = match.group(1) if match else None
    if workspace_version and workspace_version not in sema_version:
        raise RuntimeError(f"{SEMA} is {sema_version}, expected workspace version {workspace_version}")

    return sema_version


def prepare_build() -> None:
    BUILD.mkdir(parents=True, exist_ok=True)

    run_checked([brew("csc"), "-O3", "-o", str(BUILD / "1brc-chicken"), str(BENCH / "1brc.chicken.scm")])
    run_checked([brew("csc"), "-O3", "-o", str(BUILD / "1brc-chicken-simple"), str(SIMPLE / "1brc.chicken.scm")])

    run_checked([gambit("gsc"), "-exe", "-o", str(BUILD / "1brc-gambit"), str(BENCH / "1brc.gambit.scm")])
    run_checked([gambit("gsc"), "-exe", "-o", str(BUILD / "1brc-gambit-simple"), str(SIMPLE / "1brc.gambit.scm")])

    run_checked([
        brew("ecl"),
        "--eval",
        f'(compile-file "{BENCH / "1brc.ecl.lisp"}" :output-file "{BUILD / "1brc.ecl.fas"}")',
        "--eval",
        "(ext:quit 0)",
    ])
    run_checked([
        brew("ecl"),
        "--eval",
        f'(compile-file "{SIMPLE / "1brc.ecl.lisp"}" :output-file "{BUILD / "1brc-simple-ecl.fas"}")',
        "--eval",
        "(ext:quit 0)",
    ])

    for source, target in [
        (BENCH / "1brc.el", BUILD / "1brc.el"),
        (SIMPLE / "1brc.el", BUILD / "1brc-simple.el"),
    ]:
        shutil.copyfile(source, target)
        run_checked([brew("emacs"), "--batch", "-Q", "--eval", f'(byte-compile-file "{target}")'])


def optimized_benches(data: Path) -> list[Bench]:
    return [
        Bench("sbcl", [brew("sbcl"), "--script", str(BENCH / "1brc.lisp"), str(data)]),
        Bench("chez", [brew("chez"), "--script", str(BENCH / "1brc.ss"), str(data)]),
        Bench("chicken", [str(BUILD / "1brc-chicken"), str(data)]),
        Bench("gambit", [str(BUILD / "1brc-gambit")], {"BENCH_FILE": str(data)}),
        Bench("fennel", [brew("fennel"), "--lua", brew("luajit"), str(BENCH / "1brc.fnl"), str(data)]),
        Bench("clojure", [brew("clojure"), "-M", str(BENCH / "1brc.clj"), str(data)], JAVA_US_LOCALE),
        Bench("kawa", [brew("kawa"), "--script", str(BENCH / "1brc.kawa.scm"), str(data)]),
        Bench("sema-vm", [str(SEMA), "--no-llm", str(BENCH / "1brc.sema"), "--", str(data)]),
        Bench("racket", [brew("racket"), str(BENCH / "1brc.rkt"), str(data)]),
        Bench("guile", [brew("guile"), str(BENCH / "1brc.scm"), str(data)]),
        Bench("gauche", [brew("gosh"), str(BENCH / "1brc.gauche.scm"), str(data)]),
        Bench("janet", [brew("janet"), str(BENCH / "1brc.janet"), str(data)]),
        Bench("ecl", [brew("ecl"), "--load", str(BUILD / "1brc.ecl.fas"), "--", str(data)]),
        Bench("emacs", [brew("emacs"), "--batch", "-Q", "-l", str(BUILD / "1brc.elc"), str(data)]),
        Bench("newlisp", [brew("newlisp"), str(BENCH / "1brc.lsp"), str(data)]),
    ]


def simple_benches(data: Path) -> list[Bench]:
    return [
        Bench("sbcl", [brew("sbcl"), "--script", str(SIMPLE / "1brc.lisp"), str(data)]),
        Bench("chez", [brew("chez"), "--script", str(SIMPLE / "1brc.ss"), str(data)]),
        Bench("chicken", [str(BUILD / "1brc-chicken-simple"), str(data)]),
        Bench("gambit", [str(BUILD / "1brc-gambit-simple")], {"BENCH_FILE": str(data)}),
        Bench("fennel", [brew("fennel"), "--lua", brew("luajit"), str(SIMPLE / "1brc.fnl"), str(data)]),
        Bench("clojure", [brew("clojure"), "-M", str(SIMPLE / "1brc.clj"), str(data)], JAVA_US_LOCALE),
        Bench("kawa", [brew("kawa"), "--script", str(SIMPLE / "1brc.kawa.scm"), str(data)]),
        Bench("sema-vm", [str(SEMA), "--no-llm", str(SIMPLE / "1brc.sema"), "--", str(data)]),
        Bench("guile", [brew("guile"), str(SIMPLE / "1brc.scm"), str(data)]),
        Bench("gauche", [brew("gosh"), str(SIMPLE / "1brc.gauche.scm"), str(data)]),
        Bench("janet", [brew("janet"), str(SIMPLE / "1brc.janet"), str(data)]),
        Bench("ecl", [brew("ecl"), "--load", str(BUILD / "1brc-simple-ecl.fas"), "--", str(data)]),
        Bench("emacs", [brew("emacs"), "--batch", "-Q", "-l", str(BUILD / "1brc-simple.elc"), str(data)]),
        Bench("newlisp", [brew("newlisp"), str(SIMPLE / "1brc.lsp"), str(data)]),
    ]


def normalize_output(text: str) -> str:
    start = text.find("{")
    end = text.rfind("}")
    if start == -1 or end == -1:
        return text.strip()
    return "".join(text[start : end + 1].split())


# Per-dialect version probes, recorded into metadata.json so a results
# directory states which runtime produced each row. Best effort: a probe
# that fails or times out just leaves the dialect out of the map.
VERSION_PROBES: dict[str, list[list[str]]] = {
    "sbcl": [[brew("sbcl"), "--version"]],
    "chez": [[brew("chez"), "--version"]],
    "chicken": [[brew("csc"), "-version"]],
    "gambit": [[gambit("gsc"), "-v"]],
    "fennel": [[brew("fennel"), "--version"], [brew("luajit"), "-v"]],
    "clojure": [[brew("clojure"), "--version"]],
    "kawa": [[brew("kawa"), "--version"]],
    "racket": [[brew("racket"), "--version"]],
    "guile": [[brew("guile"), "--version"]],
    "gauche": [[brew("gosh"), "-V"]],
    "janet": [[brew("janet"), "-v"]],
    "ecl": [[brew("ecl"), "--version"]],
    "emacs": [[brew("emacs"), "--version"]],
    "newlisp": [[brew("newlisp"), "-v"]],
}


def tool_versions() -> dict[str, str]:
    versions: dict[str, str] = {}
    for name, attempts in VERSION_PROBES.items():
        parts = []
        for cmd in attempts:
            try:
                proc = subprocess.run(
                    cmd,
                    text=True,
                    stdout=subprocess.PIPE,
                    stderr=subprocess.STDOUT,
                    timeout=60,
                )
            except (OSError, subprocess.SubprocessError):
                continue
            out = " ".join(proc.stdout.split())
            if out:
                parts.append(out[:120])
        if parts:
            versions[name] = " | ".join(parts)[:200]
    return versions


def run_suite(label: str, benches: list[Bench], rows: int) -> list[dict[str, object]]:
    suite_dir = RESULTS / label
    suite_dir.mkdir(parents=True, exist_ok=True)
    results: list[dict[str, object]] = []
    baseline: str | None = None

    print(f"\n=== {label} ===")
    for bench in benches:
        print(f"--- {bench.name} ---")
        if not Path(bench.cmd[0]).exists():
            print(f"  SKIPPED ({bench.cmd[0]} not found)")
            results.append({"name": bench.name, "best_ms": None, "rows": rows, "error": "not installed"})
            continue

        best_ms: int | None = None
        best_stdout = ""
        best_stderr = ""
        run_times: list[int] = []

        for idx in range(1, 4):
            data_path = benches[0].cmd[-1]
            if Path(data_path).exists():
                with open(data_path, "rb") as f:
                    while f.read(1024 * 1024):
                        pass

            env = os.environ.copy()
            if bench.env:
                env.update(bench.env)

            started = time.perf_counter_ns()
            proc = subprocess.run(
                bench.cmd,
                cwd=BENCH,
                env=env,
                text=True,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                timeout=600,
            )
            elapsed_ms = (time.perf_counter_ns() - started) // 1_000_000

            if proc.returncode != 0:
                print(f"  Run {idx}: FAILED (exit {proc.returncode})")
                print("\n".join("    " + line for line in proc.stderr.splitlines()[:5]))
                continue

            print(f"  Run {idx}: {elapsed_ms} ms")
            run_times.append(elapsed_ms)
            if best_ms is None or elapsed_ms < best_ms:
                best_ms = elapsed_ms
                best_stdout = proc.stdout
                best_stderr = proc.stderr

        if KEEP_OUTPUTS:
            (suite_dir / f"{bench.name}.stdout").write_text(best_stdout)
            (suite_dir / f"{bench.name}.stderr").write_text(best_stderr)

        if best_ms is None:
            results.append({"name": bench.name, "best_ms": None, "rows": rows, "error": "failed"})
            continue

        normalized = normalize_output(best_stdout)
        output_sha256 = hashlib.sha256(normalized.encode()).hexdigest()
        if baseline is None:
            baseline = normalized
            verified = True
        else:
            verified = normalized == baseline

        print(f"  Best: {best_ms} ms")
        print(f"  Output: {'OK' if verified else 'MISMATCH'}")
        results.append({
            "name": bench.name,
            "best_ms": best_ms,
            "runs_ms": run_times,
            "rows": rows,
            "verified": verified,
            "output_sha256": output_sha256,
        })

    return results


def write_summary(label: str, results: list[dict[str, object]]) -> None:
    out = RESULTS / f"{label}.json"
    out.write_text(json.dumps(results, indent=2) + "\n")

    ranked = [r for r in results if r.get("best_ms")]
    ranked.sort(key=lambda r: int(r["best_ms"]))
    baseline = int(ranked[0]["best_ms"]) if ranked else 0

    lines = [
        f"# Native macOS {label} 1BRC Results",
        "",
        "| Dialect | Best (ms) | Relative | Verified |",
        "| --- | ---: | ---: | --- |",
    ]
    for row in ranked:
        ms = int(row["best_ms"])
        rel = ms / baseline if baseline else 0
        verified = "yes" if row.get("verified") else "no"
        lines.append(f"| {row['name']} | {ms:,} | {rel:.1f}x | {verified} |")
    for row in results:
        if not row.get("best_ms"):
            lines.append(f"| {row['name']} | FAILED | | no |")

    (RESULTS / f"{label}.md").write_text("\n".join(lines) + "\n")


def main() -> int:
    data = (Path(sys.argv[1]) if len(sys.argv) > 1 else DATA_DEFAULT).resolve()
    if not data.exists():
        print(f"data file not found: {data}", file=sys.stderr)
        return 1

    rows = sum(1 for _ in data.open("rb"))
    sema_version = ensure_sema_current()
    RESULTS.mkdir(parents=True, exist_ok=True)
    metadata = {
        "platform": platform.platform(),
        "machine": platform.machine(),
        "processor": platform.processor(),
        "data_file": str(data),
        "rows": rows,
        "sema_version": sema_version,
        "tools": tool_versions(),
        "skipped": ["picolisp"],
    }
    (RESULTS / "metadata.json").write_text(json.dumps(metadata, indent=2) + "\n")

    prepare_build()
    optimized = run_suite("optimized", optimized_benches(data), rows)
    simple = run_suite("simple", simple_benches(data), rows)
    write_summary("optimized", optimized)
    write_summary("simple", simple)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
