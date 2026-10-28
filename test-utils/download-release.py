#!/usr/bin/env python3
"""Download the integration test binaries and verify the committed release pins."""

import hashlib
import json
import platform
from pathlib import Path
import shutil
import sys
import tarfile
import urllib.request


def main():
    manifest = json.loads(Path(__file__).with_name("release-artifacts.json").read_text())
    destination = Path(sys.argv[1]).resolve()
    destination.mkdir(parents=True, exist_ok=True)
    system = {"Darwin": "darwin", "Linux": "linux"}[platform.system()]
    arch = {"arm64": "arm64", "aarch64": "arm64", "x86_64": "x64"}[platform.machine()]
    binaries = [b for b in manifest["binaries"] if b["os"] == system and b["arch"] == arch]
    if {b["role"] for b in binaries} != {"cli", "prover", "photon", "surfpool"}:
        raise RuntimeError(f"unsupported release platform: {system}-{arch}")

    names = {
        "cli": "zolana",
        "prover": "prover-server",
        "photon": "photon",
        "shielded_pool": "shielded_pool_program.so",
        "user_registry": "spr_user_registry.so",
        "surfpool": "surfpool",
    }
    for asset in manifest["programs"] + binaries:
        filename = asset["asset"]
        path = destination / filename

        def matches():
            return (
                path.is_file()
                and path.stat().st_size == asset["size"]
                and hashlib.sha256(path.read_bytes()).hexdigest() == asset["sha256"]
            )

        if not matches():
            repository, tag = "helius-labs/zolana", manifest["release_tag"]
            if asset["role"] == "surfpool":
                repository, tag = "Lightprotocol/surfpool", manifest["surfpool_tag"]
            url = f"https://github.com/{repository}/releases/download/{tag}/{filename}"
            print(f"Downloading {url}", flush=True)
            temporary = path.with_suffix(path.suffix + ".download")
            try:
                with urllib.request.urlopen(url, timeout=120) as response:
                    with temporary.open("wb") as output:
                        shutil.copyfileobj(response, output)
                temporary.replace(path)
            finally:
                temporary.unlink(missing_ok=True)
            if not matches():
                raise RuntimeError(f"release size or SHA-256 mismatch: {filename}")
        print(f"Verified {filename}: {asset['sha256']}", flush=True)
        target = destination / names[asset["role"]]
        if asset["role"] == "surfpool":
            with tarfile.open(path, "r:gz") as archive:
                candidates = [m for m in archive.getmembers() if m.isfile() and Path(m.name).name == "surfpool"]
                if len(candidates) != 1:
                    raise RuntimeError("expected exactly one surfpool binary in the verified archive")
                with archive.extractfile(candidates[0]) as source, target.open("wb") as output:
                    shutil.copyfileobj(source, output)
            target.chmod(0o755)
        else:
            target.unlink(missing_ok=True)
            target.symlink_to(filename)
            if asset["role"] in {"cli", "prover", "photon"}:
                path.chmod(0o755)


if __name__ == "__main__":
    main()
