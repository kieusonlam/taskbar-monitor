# Taskbar Monitor

Native Rust (windows crate 0.62, Win32 + GDI) app for Windows 11 that draws CPU/GPU/VRAM/RAM usage
in a small window over the taskbar, with a detail popup. One exe, two distributions:

- **GitHub Releases**: portable exe that updates itself and starts with Windows through the Run key.
- **Microsoft Store** (ID `9MVC9WD60H1S`): the same exe in an MSIX. The Store updates it; startup uses
  the manifest's StartupTask. `package::packaged()` switches between the two behaviours at runtime.

User-facing text (README, release notes, Store listing, commit messages) is in English.

## Commands

```
cargo build --release          # target\release\taskbar-monitor.exe
cargo clippy --release
python store/build.py          # MSIX -> target\store\TaskbarMonitor_<ver>.0_x64.msix (needs Windows SDK)
python store/make_screenshots.py target/store/screenshots
```

There are no automated tests: every change is checked by running the app (see Verifying).
A running copy locks the exe, so exit it from the right-click menu (or `taskkill /im taskbar-monitor.exe`)
before rebuilding over it. Launch it detached with `explorer.exe <path-to-exe>` so it outlives the shell
that started it.

## Layout

| File | Role |
|---|---|
| `src/main.rs` | Entry, single-instance mutex, message loop, 1 s `tick()`, `--update` CLI |
| `src/overlay.rs` | The bar: placement over the taskbar, z-order fights, right-click menu, settings |
| `src/popup.rs` | Detail panel (two columns, history graphs) |
| `src/metrics.rs` | PDH counters (CPU, GPU engines), DXGI adapters/VRAM, memory |
| `src/gdi.rs` | Double-buffered GDI drawing helpers, DPI scaling (`px`) |
| `src/i18n.rs` | English/Vietnamese; `tr!("English", "Tiếng Việt")` for every UI string |
| `src/update.rs` | GitHub self-update (`.exe.new` / `.exe.old` rename dance) |
| `src/autostart.rs` | Run key, or StartupTask when packaged |
| `src/package.rs` | `packaged()`: is the process running from its MSIX |
| `store/` | MSIX manifest, build script, logo generator, Store listing text |
| `.github/workflows/release.yml` | Tag `vX.Y.Z` builds and publishes the GitHub release |

Settings live in `HKCU\Software\TaskbarMonitor`: `Metrics` (bitmask, bit i = `LABELS[i]`, default
`0b1011`), `Position` (0 right, 1 left), `Language` ("en"/"vi", absent = follow Windows).

## Rules

- Single UI thread. State lives in `thread_local!`s; only the update check and download run on
  worker threads and report back with `WM_APP_*` messages. Don't add locks or cross-thread window calls.
- Every visible string goes through `tr!` with both languages.
- Anything about self-update or the Run key must be gated on `!packaged()`. Store policy forbids
  self-updating, and PRIVACY.md promises the Store build makes no network connections.
- Keep it tiny: no new crates without a strong reason (the exe is ~350 KB). New Win32 APIs usually
  need a feature added in `Cargo.toml`.
- Keep the README and `store/listing.md` in sync with user-visible changes; recapture `docs/*.png`
  when the bar, popup or menu changes.

## Hard-won behaviour (don't "simplify" these)

- **z-bands.** While Start or Quick Settings is open the shell moves the taskbar from band 1 to band 6,
  above all topmost windows, and it can stay there after the panel closes. The bar is created *owned
  by `Shell_TrayWnd`* so the shell lifts it to band 6 too. If it was created while the taskbar was
  already in band 6 it misses the lift: `overlay::ensure_lifted` destroys it once both are back in
  band 1, and the message loop recreates it on `WM_APP_RECREATE`. Band is read with the undocumented
  `GetWindowBand` (user32, loaded by name).
- **Menus and message boxes are owned by the popup window**, not the bar: `TrackPopupMenu` and
  `MessageBox` fail with ERROR_ACCESS_DENIED from a band-6 window. Close the popup before showing the
  menu, since the popup re-asserts HWND_TOPMOST every second.
- **Fullscreen hiding** uses an appbar registered with `ABM_NEW` only (no `ABM_SETPOS`, so the work
  area is untouched) to get `ABN_FULLSCREENAPP` instantly. Re-register on `TaskbarCreated`
  (Explorer restart). `WM_DESTROY` removes it; the app only quits when `QUITTING` is set.
- **Left position** only when the taskbar is centered (`TaskbarAl != 0`); offset 220 logical px if
  Widgets is shown (`TaskbarDa`), otherwise 8.
- **Self-update** can't overwrite a running exe: download `.exe.new`, rename current to `.exe.old`,
  rename new into place, restart with `--after-update`; `.old` is deleted on next start.

## Verifying a change

Build, exit the running copy, launch the new exe, then check what the change touches: the bar's
numbers against Task Manager, the popup, every menu item, Start/Quick Settings open and closed by
clicking their own buttons (bar must stay visible), a fullscreen video (bar hides at once), left
and right positions, both languages. When capturing images, capture only the app's own windows or
the taskbar strip, never the whole screen.

Not testable on the dev machine without Developer Mode: installing the MSIX locally. The Store
build's StartupTask and hidden Update item were only verified after publication.

## Releasing

`/release X.Y.Z` and `/store-update` (in `.claude/skills/`) walk through the steps below.

**GitHub**: bump `version` in `Cargo.toml`, commit, push, then `git tag vX.Y.Z && git push origin vX.Y.Z`.
The workflow refuses a tag that doesn't match `Cargo.toml`. Installed copies update within a day.

**Microsoft Store** (after the GitHub release, from the same version):
1. `cargo build --release && python store/build.py`
2. Partner Center → Taskbar Monitor → Start update: upload the new `.msix` and remove the old one
   under Packages, fill "What's new in this version" in the Store listing, submit.
3. The package version must be higher than the published one; the last part stays `0`.
4. If the update changes what the app does with content, data or purchases, the IARC age-rating
   questionnaire must be answered again.

The MSIX is unsigned on purpose: the Store signs it. Package identity (`LamKieu.TaskbarMonitor`,
publisher `CN=B4DA8985-8432-44DD-8D60-4D3D5DBBCAD8`) comes from Partner Center and must not change.
