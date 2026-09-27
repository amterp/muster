#!/usr/bin/env python3
"""Write licenses/THIRD-PARTY.md from what actually ends up inside the app.

Muster redistributes more than it used to say. Three of the five Mach-Os in the
bundle are Rust, and the crates they link are compiled into them rather than
copied beside them - so nothing in Contents/Resources tells you they are there.
swift-protobuf is the same story on the Swift side. This walks the dependency
graph, collects the license text each crate published, and writes one file the
bundle can carry.

Three things it deliberately does:

  It starts from the artifacts rather than from Cargo.lock. The lockfile holds
  123 packages; 59 of them reach a shipped binary. bindgen and protox run at
  build time and are not redistributed, and listing them would make the file
  claim something untrue in the other direction.

  It follows only normal dependency edges. A [build-dependencies] or
  [dev-dependencies] edge is where the build-time crates come in, and cargo
  labels every edge with its kind, so this is a fact to read rather than a
  judgement to make.

  It groups by license text rather than by package. Fifty crates under the same
  Apache-2.0 wording is one copy of the wording and a list of fifty names; the
  alternative is half a megabyte of the same paragraph.

Run with --check to compare against the committed file instead of rewriting it,
which is what ./dev does.
"""

from __future__ import annotations

import argparse
import glob
import hashlib
import json
import os
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
OUTPUT = ROOT / "licenses" / "THIRD-PARTY.md"

# The crates that become a file inside muster.app: the cdylib the shell calls
# through, the CLI, and the per-pane bridge. Anything reachable from one of
# these along a normal edge is redistributed.
SHIPPED_ROOTS = ("muster-seam", "muster-cli", "muster-bridge")

# Source Muster carries from another project rather than links as a crate. Cargo cannot
# see it, so it is named here: which workspace crate holds it, and the license it came
# under. It is attributed below once that crate reaches a shipped binary - before then
# the bundle does not contain it, and saying otherwise is the untruth this file avoids.
# herdr's detection manifests come from two places (NOTICE says which is which), so they are
# two entries: labelling all of them v0.8.0 would say something untrue of eight.
DETECT_MANIFESTS = "crates/muster-detect/manifests/"
VENDORED = (
    {
        "crate": "muster-detect",
        "project": "herdr",
        "version": "0.8.0",
        "license": "Apache-2.0",
        "repository": "https://github.com/herdrdev/herdr",
        "license_file": "licenses/herdr/LICENSE",
        "paths": (
            "crates/muster-detect/src",
            *(
                DETECT_MANIFESTS + name
                for name in (
                    "amp.toml",
                    "antigravity.toml",
                    "devin.toml",
                    "droid.toml",
                    "gemini.toml",
                    "hermes.toml",
                    "kilo.toml",
                    "kimi.toml",
                    "maki.toml",
                    "opencode.toml",
                    "qodercli.toml",
                )
            ),
        ),
    },
    {
        "crate": "muster-detect",
        "project": "herdr's agent-detection catalog",
        "version": "(each manifest's own version, in NOTICE)",
        "license": "Apache-2.0",
        "repository": "https://herdr.dev/agent-detection/",
        "license_file": "licenses/herdr/LICENSE",
        "paths": tuple(
            DETECT_MANIFESTS + name
            for name in (
                "claude.toml",
                "cline.toml",
                "codex.toml",
                "cursor.toml",
                "github-copilot.toml",
                "grok.toml",
                "kiro.toml",
                "pi.toml",
            )
        ),
    },
)

# The release is Apple Silicon only, so this is the graph that ships. Without it
# cargo resolves for every platform and pulls in Windows crates that are not in
# the local registry, which turns an offline run into a download or a failure.
TARGET = "aarch64-apple-darwin"

# Ordered by how much a reader wants to see: a file naming its license beats one
# that only names a project.
LICENSE_FILENAMES = (
    "LICENSE-APACHE",
    "LICENSE-MIT",
    "LICENSE-BSD",
    "LICENSE-ZLIB",
    "LICENSE.txt",
    "LICENSE.md",
    "LICENSE",
    "LICENCE",
    "COPYING",
    "UNLICENSE",
    "NOTICE",
)


def cargo_metadata() -> dict:
    out = subprocess.run(
        [
            "cargo",
            "metadata",
            "--format-version",
            "1",
            "--offline",
            "--filter-platform",
            TARGET,
        ],
        cwd=ROOT,
        capture_output=True,
        text=True,
    )
    if out.returncode != 0:
        sys.exit(
            "cargo metadata failed, so the shipped dependency set is unknown.\n"
            "Impact: licenses/THIRD-PARTY.md cannot be written or verified, and\n"
            "the bundle would ship attribution that no longer matches what is in\n"
            "it. Usually this means the registry cache is empty - run a build\n"
            "first, since --offline reads what cargo already fetched.\n\n"
            + out.stderr.strip()
        )
    return json.loads(out.stdout)


def shipped_packages(meta: dict) -> tuple[list[dict], set[str]]:
    """Every third-party package reachable from a shipped crate, and the names of the
    workspace crates reached on the way."""
    by_id = {p["id"]: p for p in meta["packages"]}
    nodes = {n["id"]: n for n in meta["resolve"]["nodes"]}
    workspace = {by_id[i]["name"] for i in meta["workspace_members"]}

    roots = [
        i
        for i in meta["workspace_members"]
        if by_id[i]["name"] in SHIPPED_ROOTS
    ]
    missing = set(SHIPPED_ROOTS) - {by_id[i]["name"] for i in roots}
    if missing:
        sys.exit(
            f"These crates are named as shipping but are not in the workspace: "
            f"{sorted(missing)}.\n"
            "Impact: whatever they pull in would go unattributed. Either they were\n"
            "renamed, in which case update SHIPPED_ROOTS here, or they were dropped,\n"
            "in which case check that build_bundle in ./dev no longer copies them."
        )

    seen: set[str] = set()
    stack = list(roots)
    while stack:
        pid = stack.pop()
        if pid in seen:
            continue
        seen.add(pid)
        for dep in nodes[pid]["deps"]:
            # kind null is a normal dependency; "build" and "dev" are not
            # redistributed. A package can be reached by several kinds at once.
            if not any(k["kind"] is None for k in dep["dep_kinds"]):
                continue
            stack.append(dep["pkg"])

    third_party = sorted(
        (by_id[i] for i in seen if by_id[i]["name"] not in workspace),
        key=lambda p: (p["name"].lower(), p["version"]),
    )
    return third_party, {by_id[i]["name"] for i in seen} & workspace


def shipped_vendored(meta: dict, shipped_crates: set[str]) -> list[dict]:
    """The VENDORED entries that ship, after checking every entry still describes the tree."""
    by_id = {p["id"]: p for p in meta["packages"]}
    workspace = {by_id[i]["name"] for i in meta["workspace_members"]}
    for entry in VENDORED:
        missing = [
            path
            for path in (entry["license_file"], *entry["paths"])
            if not (ROOT / path).exists()
        ]
        if entry["crate"] not in workspace or missing:
            sys.exit(
                f"VENDORED names {entry['project']} source in {entry['crate']}, but "
                f"{'that crate is not in the workspace' if entry['crate'] not in workspace else 'these paths are gone: ' + ', '.join(missing)}.\n"
                "Impact: the attribution would describe code that is not where it says,\n"
                "so it cannot be trusted to cover the code that is. If the source moved,\n"
                "update the entry in tools/licenses.py; if it was removed, drop the entry."
            )
    listed = sorted(
        path for entry in VENDORED for path in entry["paths"] if path.startswith(DETECT_MANIFESTS)
    )
    on_disk = sorted(
        DETECT_MANIFESTS + path.name for path in (ROOT / DETECT_MANIFESTS).glob("*.toml")
    )
    if listed != on_disk:
        sys.exit(
            f"VENDORED lists the detection manifests {listed}, and {DETECT_MANIFESTS} holds "
            f"{on_disk}.\n"
            "Impact: a manifest would ship attributed to the wrong source, or to none.\n"
            "Add a new manifest to the VENDORED entry for where it came from (NOTICE says\n"
            "which manifests came from where), or drop one that was removed."
        )
    return [entry for entry in VENDORED if entry["crate"] in shipped_crates]


def license_texts(pkg: dict) -> list[tuple[str, str]]:
    """(filename, text) for every license file the crate published."""
    source = Path(pkg["manifest_path"]).parent
    found = []
    for name in LICENSE_FILENAMES:
        for path in sorted(glob.glob(str(source / name))):
            text = Path(path).read_text(encoding="utf-8", errors="replace").strip()
            if text:
                found.append((os.path.basename(path), text))
    return found


def swift_packages() -> list[dict]:
    """The resolved SwiftPM dependencies, which link into the app executable."""
    resolved = json.loads((ROOT / "Package.resolved").read_text())
    pins = resolved.get("pins", []) or resolved.get("object", {}).get("pins", [])
    out = []
    for pin in pins:
        state = pin.get("state", {})
        out.append(
            {
                "name": pin.get("identity", pin.get("package", "?")),
                "version": state.get("version", state.get("revision", "?")),
                "revision": state.get("revision", ""),
                "repository": pin.get("location", pin.get("repositoryURL", "")),
            }
        )
    return sorted(out, key=lambda p: p["name"])


def swift_license_text(pkg: dict) -> str:
    """A Swift dependency's own license, from wherever SwiftPM left it.

    Two places, because a checkout only exists once something has been built and
    this has to work on a tree that has merely resolved. The bare mirror SwiftPM
    keeps is always there, and holds the same file one `git show` away.
    """
    name = pkg["name"]
    checkouts = ROOT / ".build" / "checkouts"
    if checkouts.exists():
        for path in sorted(checkouts.glob(f"{name}*/LICENSE*")):
            text = path.read_text(encoding="utf-8", errors="replace").strip()
            if text:
                return text

    cache = Path.home() / "Library" / "Caches" / "org.swift.swiftpm" / "repositories"
    for repo in sorted(cache.glob(f"{name}-*")) if cache.exists() else []:
        listing = subprocess.run(
            ["git", "-C", str(repo), "ls-tree", "--name-only", pkg["revision"]],
            capture_output=True,
            text=True,
        )
        for entry in listing.stdout.split():
            if not entry.upper().startswith(("LICENSE", "LICENCE", "COPYING")):
                continue
            blob = subprocess.run(
                ["git", "-C", str(repo), "show", f"{pkg['revision']}:{entry}"],
                capture_output=True,
                text=True,
            )
            if blob.returncode == 0 and blob.stdout.strip():
                return blob.stdout.strip()

    sys.exit(
        f"No license text could be found for the Swift package {name}.\n"
        "Impact: it links into Contents/MacOS/muster and would ship unattributed.\n"
        "SwiftPM keeps the text in .build/checkouts once something has been built,\n"
        "and in ~/Library/Caches/org.swift.swiftpm/repositories once anything has\n"
        "been resolved - neither had it. Try: swift build"
    )


def render(packages: list[dict], swift: list[dict], vendored: list[dict]) -> str:
    # Group by the exact text, so one wording appears once however many crates
    # publish it.
    groups: dict[str, dict] = {}
    unlicensed: list[dict] = []
    for pkg in packages:
        texts = license_texts(pkg)
        if not texts:
            unlicensed.append(pkg)
            continue
        for filename, text in texts:
            key = hashlib.sha256(text.encode()).hexdigest()
            group = groups.setdefault(
                key, {"text": text, "filenames": set(), "packages": []}
            )
            group["filenames"].add(filename)
            group["packages"].append(pkg)

    swift_texts = {pkg["name"]: swift_license_text(pkg) for pkg in swift}

    lines: list[str] = []
    w = lines.append

    w("# Third-party software in Muster")
    w("")
    w("Generated by `tools/licenses.py`; edit that rather than this file.")
    w("")
    w(
        "This covers what is compiled *into* the binaries in `Muster.app` and is "
        "therefore"
    )
    w(
        "invisible from the bundle's own contents. What ships beside them as a "
        "whole file -"
    )
    w(
        "the herdr daemon, and libghostty - is attributed in `NOTICE`, which is "
        "the other"
    )
    w("half of the same account.")
    w("")
    w(
        f"The Rust set is every crate reachable along a normal dependency edge "
        f"from `muster-seam`,"
    )
    w(
        f"`muster-cli` or `muster-bridge`, resolved for `{TARGET}`. Crates that "
        f"only run at"
    )
    w("build time or under `cargo test` are not here, because they are not "
      "redistributed.")
    w("")
    w(f"**{len(packages)} Rust crates** and **{len(swift)} Swift package(s)**.")
    w("")

    w("## What is in the binaries")
    w("")
    w("| Package | Version | License | Source |")
    w("| --- | --- | --- | --- |")
    for pkg in packages:
        repo = pkg.get("repository") or ""
        w(
            f"| {pkg['name']} | {pkg['version']} | {pkg.get('license') or '-'} "
            f"| {repo} |"
        )
    for pkg in swift:
        w(
            f"| {pkg['name']} (Swift) | {pkg['version']} | Apache-2.0 "
            f"| {pkg['repository']} |"
        )
    w("")

    if unlicensed:
        w("## Published without a license file")
        w("")
        w(
            "These name a license in their manifest but ship no copy of it in the "
            "published"
        )
        w(
            "crate, so the text below cannot include one. What the manifest claims, "
            "and where"
        )
        w("the original lives:")
        w("")
        for pkg in unlicensed:
            w(
                f"- **{pkg['name']} {pkg['version']}** - "
                f"{pkg.get('license') or 'no license field'} - "
                f"{pkg.get('repository') or 'no repository'}"
            )
        w("")

    w("## License texts")
    w("")
    for key in sorted(groups, key=lambda k: (-len(groups[k]["packages"]), k)):
        group = groups[key]
        names = sorted(
            {f"{p['name']} {p['version']}" for p in group["packages"]},
            key=str.lower,
        )
        label = ", ".join(sorted(group["filenames"]))
        if len(names) == 1:
            w(f"### {names[0]} - {label}")
        else:
            w(f"### {names[0]} and {len(names) - 1} others - {label}")
            w("")
            w(", ".join(names))
        w("")
        w("```")
        w(group["text"])
        w("```")
        w("")

    for name, text in sorted(swift_texts.items()):
        w(f"### {name} (Swift)")
        w("")
        w("```")
        w(text)
        w("```")
        w("")

    if vendored:
        w("## Source ported from other projects")
        w("")
        w("Compiled in as part of a Muster crate rather than linked as a crate of its own.")
        w("")
        for entry in vendored:
            w(
                f"### {entry['project']} {entry['version']} - {entry['license']} - "
                f"{entry['repository']}"
            )
            w("")
            w(f"In `{entry['crate']}`: " + ", ".join(f"`{p}`" for p in entry["paths"]) + ".")
            w("")
            w("```")
            w((ROOT / entry["license_file"]).read_text(encoding="utf-8").strip())
            w("```")
            w("")

    return "\n".join(lines).rstrip() + "\n"


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--check",
        action="store_true",
        help="fail if the committed file no longer describes what ships",
    )
    args = parser.parse_args()

    meta = cargo_metadata()
    packages, shipped_crates = shipped_packages(meta)
    swift = swift_packages()
    rendered = render(packages, swift, shipped_vendored(meta, shipped_crates))

    if not args.check:
        OUTPUT.parent.mkdir(parents=True, exist_ok=True)
        OUTPUT.write_text(rendered)
        print(
            f"wrote {OUTPUT.relative_to(ROOT)}: "
            f"{len(packages)} Rust crates, {len(swift)} Swift package(s)"
        )
        return 0

    current = OUTPUT.read_text() if OUTPUT.exists() else ""
    if current == rendered:
        print(
            f"{len(packages)} Rust crates and {len(swift)} Swift package(s) "
            f"attributed"
        )
        return 0

    print(
        f"{OUTPUT.relative_to(ROOT)} no longer describes what the bundle ships.\n"
        "\n"
        "Impact: muster.app would carry attribution for a different set of\n"
        "dependencies than the ones inside it, which is the one thing that file\n"
        "exists to prevent. A dependency almost always arrives inside a change\n"
        "about something else, so this is checked here rather than remembered.\n"
        "\n"
        "Regenerate and commit the result:\n"
        "  python3 tools/licenses.py",
        file=sys.stderr,
    )
    return 1


if __name__ == "__main__":
    sys.exit(main())
