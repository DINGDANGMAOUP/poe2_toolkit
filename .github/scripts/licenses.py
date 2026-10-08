#!/usr/bin/env python3
"""Collect locked dependency metadata and available upstream license notices."""

import argparse
import json
import shutil
import subprocess
from pathlib import Path

root = Path(__file__).resolve().parents[2]
p = argparse.ArgumentParser(description=__doc__)
p.add_argument('--target', default='x86_64-pc-windows-msvc')
p.add_argument('--metadata-output', type=Path, default=root / 'dist/dependency-licenses.json')
args = p.parse_args()
metadata = json.loads(
    subprocess.check_output(
        [
            "cargo",
            "metadata",
            "--locked",
            "--format-version",
            "1",
            "--filter-platform",
            args.target,
        ],
        cwd=root,
    )
)
ids = {n["id"] for n in metadata["resolve"]["nodes"]} - set(
    metadata["workspace_members"]
)
records = []
out = root / "dist" / "third-party-licenses"
out.mkdir(parents=True, exist_ok=True)
for pkg in sorted(metadata["packages"], key=lambda p: (p["name"], p["version"])):
    if pkg["id"] not in ids:
        continue
    record = {k: pkg[k] for k in ["name", "version", "license", "repository"]}
    directory = Path(pkg["manifest_path"]).parent
    candidates = [
        p
        for p in directory.iterdir()
        if p.name.upper().startswith(("LICENSE", "LICENCE", "COPYING", "NOTICE"))
    ]
    for folder in ("licenses", "LICENSES"):
        if (directory / folder).is_dir():
            candidates += list((directory / folder).glob("*"))
    if pkg.get("license_file"):
        candidates.append(directory / pkg["license_file"])
    cached = (
        root / "licenses" / (pkg["name"] + "-" + pkg["version"])
    )
    if cached.is_dir():
        candidates += [p for p in cached.iterdir()
                       if p.name.upper().startswith(("LICENSE", "LICENCE", "COPYING", "NOTICE"))]
    record["notice_files"] = []
    for source in sorted(set(candidates)):
        if source.is_file():
            destination = out / (pkg["name"] + "-" + pkg["version"]) / source.name
            destination.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(source, destination)
            record["notice_files"].append(destination.relative_to(out).as_posix())
    if not record["notice_files"]:
        raise RuntimeError(f'Missing upstream license notice: {pkg["name"]} {pkg["version"]}')
    records.append(record)
data = dict(
    platform=args.target,
    source="Cargo.lock and cargo metadata --filter-platform",
    packages=records,
)
args.metadata_output.parent.mkdir(parents=True, exist_ok=True)
args.metadata_output.write_text(
    json.dumps(data, ensure_ascii=False, indent=2) + "\n"
)
for source in (root / "crates/formats/schemas").glob("*LICENSE*"):
    shutil.copy2(source, out / source.name)
print(
    json.dumps(
        {
            "packages": len(records),
            "without_bundled_notice": [
                r["name"] for r in records if not r["notice_files"]
            ],
            "output": str(out),
        }
    )
)
