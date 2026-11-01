# ADR: Mobile platform support (Android/iOS) — architecture decision

Status: accepted
Date: 2026-11-01
Branch: feat/mobile-platforms

## Context

Pangan is a single-crate Rust + Slint desktop app. The goal is to add Android
and iOS support while leaving the desktop application and its Slint interface
byte-for-byte unchanged.

Constraint from the goal: the existing UI (everything under `ui/`, the `.slint`
files, assets) is immutable. Backend code may be restructured.

## Baseline (recorded)

- Baseline commit: `21d315915641f477ac99e564e524717b07f7ad90` (HEAD of `main`).
- Feature branch: `feat/mobile-platforms`.
- UI-immutability guard: `tools/check_ui_immutable.sh` compares the `ui/`
  tree against the baseline commit (tracked diffs + untracked files + byte
  hashes). Runs at every milestone. Currently passing.

## Findings from the audit

- The timer core (`src/timer/clock.rs`) already uses a **wall-clock Unix
  deadline** (`finish_unix`), not monotonic `Instant`, so it is already correct
  across system suspend. This is the right foundation for mobile lifecycle.
- Crash recovery already exists: `src/timer/session_snapshot.rs` persists an
  in-flight focus block; `begin_from_recovery` rehydrates the clock.
- `src/platform/` submodules (`notify`, `tray`, `autostart`, `inhibit`) only
  have implementations for Linux/Windows/macOS. The crate will *not* compile
  for Android/iOS today.
- Persistence uses `dirs::config_dir()/pangan`. On mobile there is no home
  config dir; the storage root must come from the platform.
- Desktop hooks in `app::run()` (tray, hotkeys, autostart, inhibit, gdbus /
  gsettings theme, `xdg-open`) are desktop-specific and cfg-gated by OS.
- Build: `slint_build::compile("ui/appwindow.slint")`; Slint 1.11.

## Architecture chosen

The goal states hypothesis A (a workspace with shared core + desktop/android/ios
packages) is the preferred starting point, validated against the repo. We
adopt it in a **minimally invasive** form:

1. **Same crate becomes a library + binary.** Add `src/lib.rs` declaring the
   existing modules (all `pub` so platform entry crates can reuse them);
   `src/main.rs` becomes a thin binary that calls `pangan::app::run()`.
   All internal `crate::` paths stay valid because the modules remain in one
   crate. This preserves the desktop binary exactly (`cargo build --release` ->
   `target/release/pangan`) and exposes the whole domain/core to mobile crates
   with zero logic churn.

2. **Cargo workspace.** Root package is the desktop app. New members:
   - `platforms/android/app` — Android entry-point crate (Rust core + Android
     native host).
   - `platforms/ios/app` — iOS entry-point crate (Rust core + iOS native host).
   These depend on the root package as a library via a path dependency.

3. **Platform capability layer.** A new `src/platform/caps.rs` defines narrow
   traits for OS capabilities the app needs and that differ on mobile:
   - `StoragePaths` — data/config directory roots.
   - `Notifications` — schedule/cancel local notifications.
   - `SecureStore` — credential storage (tokens).
   - `Lifecycle` — foreground/background transition events.
   - `OpenUrl` — deep-link / browser open (used by OAuth).
   Desktop implements them with the existing behavior; Android/iOS implement
   them with native calls. The desktop `notify`/`tray`/`autostart`/
   `inhibit` modules stay untouched and remain gated to desktop OSes: the new
   factor module becomes the sole mobile path.

4. **Timer lifecycle reliability.** Keep the wall-clock deadline model. Add a
   `src/timer/lifecycle.rs` module with deterministic, injected-clock tests for
   suspension/resume/expiry/idempotent completion and non-default durations.
   Completion remains idempotent: sessions are deduped by `(at, minutes)` in
   `Store::add_sessions`, and the recovery snapshot is cleared only after the
   completed sessions are drained.

## Why not a full extraction

Extracting the domain modules into a separate `pangan-core` crate would mean
rewriting every `crate::` internal path and the module tree (~36 files) for
zero functional benefit; it raises regression risk against the untouched UI.
The lib+bin form of the *same* crate gives the mobile entry crates a reusable
core without touching module internals. This satisfies "shared core reused by
mobile targets" with the minimum surface of change.

## Protected UI

No `.slint` file, `ui/` asset, layout, theme, font, animation, or icon is
touched. `tools/check_ui_immutable.sh` is the automated enforcement and is run
at every milestone.
