#!/usr/bin/env python3
"""Collect the licenses of every dependency compiled into WoW-don (issue #194).

The GPL (D40) covers our own code; the crates and npm packages inside the binary keep
their own licenses, and MIT and Apache-2.0 (section 4(a)) ask that a copy travels with
every binary. release.yml runs this before each `pnpm tauri build` and the bundle picks
the file up through src-tauri/tauri.release*.conf.json.

    python3 scripts/third-party-licenses.py --target <triple> [--target <triple>] --output <file>

Rust: `cargo metadata --filter-platform <triple>` for each target, walked from `wow-don`
along normal dependencies only — dev and build dependencies are not shipped. A universal
macOS build passes both Apple triples and gets the union. npm: `pnpm licenses list --prod`.

For each package the file carries its name, version, license expression and the full
text of the LICENSE*, LICENCE*, COPYING* and NOTICE* files in its folder. Packages that
ship none are listed with their expression, and the standard Apache-2.0 and MIT texts
found in other packages are appended once. Paths are never written: the output must not
name the machine that built it. Standard library only (no new dependency).
"""

import argparse
import json
import os
import shutil
import subprocess
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
MANIFEST = os.path.join(ROOT, "src-tauri", "Cargo.toml")
ROOT_PACKAGE = "wow-don"
LICENSE_PREFIXES = ("LICENSE", "LICENCE", "COPYING", "NOTICE")
RULE = "=" * 78


def run_json(argv, cwd):
    """Runs a tool found on PATH and parses its stdout as JSON; any failure is fatal."""
    exe = shutil.which(argv[0])
    if exe is None:
        sys.exit(f"third-party-licenses: {argv[0]} not found on PATH")
    result = subprocess.run(
        [exe, *argv[1:]], cwd=cwd, capture_output=True, text=True, encoding="utf-8"
    )
    if result.returncode != 0:
        sys.stderr.write(result.stderr)
        sys.exit(f"third-party-licenses: {argv[0]} {argv[1]} failed ({result.returncode})")
    try:
        return json.loads(result.stdout)
    except json.JSONDecodeError:
        sys.exit(f"third-party-licenses: {argv[0]} {argv[1]} did not print JSON")


def license_files(folder):
    """The license texts in a package folder, by file name, sorted; unreadable ones skipped."""
    texts = []
    try:
        names = sorted(os.listdir(folder))
    except OSError:
        return texts
    for name in names:
        path = os.path.join(folder, name)
        if not name.upper().startswith(LICENSE_PREFIXES) or not os.path.isfile(path):
            continue
        try:
            with open(path, encoding="utf-8", errors="replace") as handle:
                texts.append((name, handle.read().strip()))
        except OSError:
            continue
    return texts


def crates(targets):
    """Every crate shipped for any of `targets`: {(name, version): (license, folder)}."""
    found = {}
    for target in targets:
        meta = run_json(
            [
                "cargo", "metadata", "--locked", "--format-version", "1",
                "--filter-platform", target, "--manifest-path", MANIFEST,
            ],
            ROOT,
        )
        packages = {p["id"]: p for p in meta["packages"]}
        nodes = {n["id"]: n for n in meta["resolve"]["nodes"]}
        roots = [i for i, p in packages.items() if p["name"] == ROOT_PACKAGE]
        if len(roots) != 1:
            sys.exit(f"third-party-licenses: {ROOT_PACKAGE} not found in cargo metadata")
        seen, stack = set(), list(roots)
        while stack:
            current = stack.pop()
            if current in seen:
                continue
            seen.add(current)
            for dep in nodes[current]["deps"]:
                # `kind: null` is a normal dependency; "dev" and "build" never reach the binary.
                if any(k.get("kind") is None for k in dep["dep_kinds"]):
                    stack.append(dep["pkg"])
        for pkg_id in seen:
            pkg = packages[pkg_id]
            if pkg["name"] == ROOT_PACKAGE:
                continue
            key = (pkg["name"], pkg["version"])
            found[key] = (pkg.get("license") or "unspecified", os.path.dirname(pkg["manifest_path"]))
    return found


def npm_packages():
    """Production npm packages: {(name, version): (license, folder)}."""
    found = {}
    listing = run_json(["pnpm", "licenses", "list", "--prod", "--json"], ROOT)
    for license_id, entries in listing.items():
        for entry in entries:
            for version, folder in zip(entry["versions"], entry["paths"]):
                found[(entry["name"], version)] = (entry.get("license") or license_id, folder)
    return found


def section(title, packages, standard):
    """Writes one ecosystem; returns the lines and the packages without license files."""
    lines, missing = [f"{RULE}\n{title}\n{RULE}\n"], []
    for (name, version) in sorted(packages):
        license_id, folder = packages[(name, version)]
        texts = license_files(folder)
        if not texts:
            missing.append((name, version, license_id))
            continue
        lines.append(f"\n--- {name} {version} ({license_id}) ---\n")
        for file_name, text in texts:
            lines.append(f"\n[{file_name}]\n\n{text}\n")
            upper = file_name.upper()
            if "APACHE" in upper and "Apache License" in text:
                standard.setdefault("Apache-2.0", text)
            elif "MIT" in upper:
                standard.setdefault("MIT", text)
    return lines, missing


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--target", action="append", required=True, help="Rust target triple")
    parser.add_argument("--output", required=True, help="file to write")
    args = parser.parse_args()

    standard = {}
    rust_lines, rust_missing = section("Rust crates", crates(args.target), standard)
    npm_lines, npm_missing = section("npm packages", npm_packages(), standard)

    out = [
        "WoW-don is licensed under GPL-3.0-or-later (see LICENSE).\n",
        "It includes the third-party software listed below, under the licenses shown.\n\n",
        *rust_lines,
        "\n",
        *npm_lines,
    ]
    missing = rust_missing + npm_missing
    if missing:
        out.append(f"\n{RULE}\nPackages without a license file (license expression only)\n{RULE}\n\n")
        out.extend(f"{n} {v} ({l})\n" for n, v, l in sorted(missing))
        if standard:
            out.append(f"\n{RULE}\nStandard license texts\n{RULE}\n")
            for license_id in sorted(standard):
                out.append(f"\n--- {license_id} ---\n\n{standard[license_id]}\n")

    os.makedirs(os.path.dirname(os.path.abspath(args.output)), exist_ok=True)
    with open(args.output, "w", encoding="utf-8", newline="\n") as handle:
        handle.write("".join(out))


if __name__ == "__main__":
    main()
