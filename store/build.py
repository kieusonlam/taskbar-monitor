"""Build the Microsoft Store package (.msix) from a release exe.

The package is unsigned: the Store signs it on submission. For a local test, register the layout
folder instead (needs Developer Mode): Add-AppxPackage -Register <layout>\\AppxManifest.xml

Usage: python store/build.py [EXE] [OUT_DIR]
  EXE      defaults to target/release/taskbar-monitor.exe
  OUT_DIR  defaults to target/store; gets layout/ and TaskbarMonitor_<version>_x64.msix
"""

import re
import shutil
import subprocess
import sys
from pathlib import Path

sys.stdout.reconfigure(encoding="utf-8", errors="replace")

ROOT = Path(__file__).resolve().parent.parent
STORE = ROOT / "store"


def sdk_tool(name):
    """Newest Windows SDK copy of an x64 tool."""
    bins = Path(r"C:\Program Files (x86)\Windows Kits\10\bin")
    found = sorted(bins.glob(f"10.*/x64/{name}"), key=lambda p: [int(n) for n in p.parts[-3].split(".")])
    if not found:
        sys.exit(f"{name} not found: install the Windows SDK")
    return str(found[-1])


def version():
    """Cargo.toml's version as the 4-part package version (the Store requires the last part to be 0)."""
    m = re.search(r'^version = "(\d+)\.(\d+)\.(\d+)"', (ROOT / "Cargo.toml").read_text(encoding="utf-8"), re.M)
    return f"{m[1]}.{m[2]}.{m[3]}.0"


def run(*args):
    r = subprocess.run(args, capture_output=True, text=True)
    if r.returncode:
        sys.exit(f"{Path(args[0]).name} failed:\n{r.stdout}\n{r.stderr}")


def main():
    exe = Path(sys.argv[1]) if len(sys.argv) > 1 else ROOT / "target/release/taskbar-monitor.exe"
    out = Path(sys.argv[2]) if len(sys.argv) > 2 else ROOT / "target/store"
    v = version()
    layout = out / "layout"
    shutil.rmtree(layout, ignore_errors=True)
    (layout / "Assets").mkdir(parents=True)

    shutil.copy2(exe, layout / "taskbar-monitor.exe")
    manifest = (STORE / "AppxManifest.xml").read_text(encoding="utf-8").replace("{VERSION}", v)
    (layout / "AppxManifest.xml").write_text(manifest, encoding="utf-8")
    run(sys.executable, str(STORE / "make_assets.py"), str(layout / "Assets"))

    # resources.pri lets Windows pick the right logo for each scale and size
    makepri = sdk_tool("makepri.exe")
    config = out / "priconfig.xml"
    run(makepri, "createconfig", "/cf", str(config), "/dq", "en-US", "/pv", "10.0.0", "/o")
    # A single .msix (not a bundle) has no resource packs: keep every scale in resources.pri
    text = config.read_text(encoding="utf-8")
    config.write_text(re.sub(r"\s*<packaging>.*?</packaging>", "", text, flags=re.S), encoding="utf-8")
    run(makepri, "new", "/pr", str(layout), "/cf", str(config), "/mn", str(layout / "AppxManifest.xml"),
        "/of", str(layout / "resources.pri"), "/o")

    msix = out / f"TaskbarMonitor_{v}_x64.msix"
    run(sdk_tool("makeappx.exe"), "pack", "/d", str(layout), "/p", str(msix), "/o")
    print(f"{msix} ({msix.stat().st_size // 1024} KB)")


if __name__ == "__main__":
    main()
