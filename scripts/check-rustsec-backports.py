"""Prevent the Linux GTK stack from silently losing its RUSTSEC-2024-0429 fix."""

from pathlib import Path
import tomllib


# Upstream gtk-rs/gtk-rs-core#1343, backported verbatim to glib 0.18.5.
# Dependabot still flags the version, so independently check the locked source.
GLIB_REV = "7e91f1a9420fd3e6c58b7913bbbf201888304ea9"
GLIB_SOURCE = (
    "git+https://github.com/andrew-wommack-ministries/glib-rustsec-2024-0429-backport"
    f"?rev={GLIB_REV}#{GLIB_REV}"
)


def check_lock(lock_path: Path) -> None:
    with lock_path.open("rb") as lock:
        packages = tomllib.load(lock)["package"]
    for package in packages:
        if package["name"] != "glib":
            continue
        version = package["version"].split("+", 1)[0]
        core, _, prerelease = version.partition("-")
        major, minor, patch = map(int, core.split("."))
        affected = (0, 15, 0) <= (major, minor, patch) < (0, 20, 0) or (
            core == "0.20.0" and prerelease
        )
        if affected and not (
            version == "0.18.5" and package.get("source") == GLIB_SOURCE
        ):
            raise SystemExit(f"glib {version} lacks the verified RUSTSEC-2024-0429 backport")


if __name__ == "__main__":
    check_lock(Path(__file__).resolve().parents[1] / "src-tauri" / "Cargo.lock")
