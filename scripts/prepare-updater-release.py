"""Assemble the public release from the signed native bundles built by release.yml."""

import base64
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile


# Keep the installer format across upgrades, including machines with MSI or Debian installs.
FORMATS = {
    "windows-x86_64": {"nsis": "-setup.exe", "msi": ".msi"},
    "linux-x86_64": {"appimage": ".AppImage", "deb": ".deb"},
    "darwin-aarch64": {"app": ".app.tar.gz"},
    "darwin-x86_64": {"app": ".app.tar.gz"},
}


def bundle(directory, suffix):
    matches = list(directory.rglob(f"*{suffix}"))
    if len(matches) != 1:
        raise ValueError(f"Expected one {suffix} bundle under {directory}, found {len(matches)}")
    return matches[0]


def main():
    repo = os.environ["RELEASE_REPOSITORY"]
    tag = os.environ.get("RELEASE_TAG") or os.environ["GITHUB_REF_NAME"]
    version = json.loads(Path("src-tauri/tauri.conf.json").read_text())["version"]
    if tag != f"v{version}":
        raise ValueError(f"Release tag {tag} does not match version {version}")
    output = Path("release")
    output.mkdir()
    platforms = {}
    with tempfile.TemporaryDirectory() as work:
        key = Path(work, "updater.pub")
        key.write_bytes(base64.b64decode(os.environ["UPDATER_PUBLIC_KEY"].strip(), validate=True))
        minisig = Path(work, "artifact.minisig")
        for platform, formats in FORMATS.items():
            directory = Path("artifacts", platform)
            basename = f"Agent-Mod-Manager_{version}_{platform}"
            for installer, suffix in formats.items():
                source = bundle(directory, suffix)
                signature = Path(f"{source}.sig").read_text().strip()
                minisig.write_bytes(base64.b64decode(signature, validate=True))
                subprocess.run(["minisign", "-Vm", source, "-p", key, "-x", minisig], check=True)
                trusted_comment = minisig.read_text().splitlines()[2]
                if f"version:{version}" not in trusted_comment.split("\t"):
                    raise ValueError(f"Missing signed version {version} on {source}")
                asset = output / f"{basename}{suffix}"
                shutil.copyfile(source, asset)
                platforms[f"{platform}-{installer}"] = {
                    "url": f"https://github.com/{repo}/releases/download/{tag}/{asset.name}",
                    "signature": signature,
                }
            if platform.startswith("darwin-"):
                shutil.copyfile(bundle(directory, ".dmg"), output / f"{basename}.dmg")
    (output / "latest.json").write_text(json.dumps({"version": version, "platforms": platforms}, indent=2) + "\n")


if __name__ == "__main__":
    main()
