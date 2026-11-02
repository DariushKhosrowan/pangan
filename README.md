<div align="center">

# Pangan · پنگان

### Focus deeply. Plan meaningfully. Make time count.

**A free, open-source Pomodoro timer and productivity app, inspired by Iranian timekeeping.**  
Built with **Rust** and **Slint**, with a Persian calendar, task planning, and a focus journal.

<p>
  <a href="https://github.com/DariushKhosrowan/pangan/releases/latest"><img src="https://img.shields.io/github/v/release/DariushKhosrowan/pangan?style=flat-square&label=latest%20release&color=B48B31" alt="Latest release" /></a>
  <a href="https://github.com/DariushKhosrowan/pangan/actions"><img src="https://img.shields.io/github/actions/workflow/status/DariushKhosrowan/pangan/build.yml?style=flat-square&label=build" alt="Build status" /></a>
  <a href="LICENSE"><img src="https://img.shields.io/github/license/DariushKhosrowan/pangan?style=flat-square&color=656975" alt="License" /></a>
</p>

**[Download Pangan](https://github.com/DariushKhosrowan/pangan/releases/latest)** · **[Explore features](#what-you-can-do)** · **[Build from source](#build-from-source)** · **[Contribute](#contributing)**

<sub>Desktop: Linux · Windows · macOS | Mobile: Android APKs + iOS simulator build</sub>

</div>

---

## Meet Pangan

*Pangan* (پنگان) is named after a traditional Persian water clock. Its purpose is simple: turn time into focused work—without losing sight of your intentions or the rhythm of your day.

Pangan brings together a customizable **Pomodoro timer**, **daily tasks**, **Persian (Jalali) calendar**, historical Iranian day names and events, **focus statistics**, and an optional **Google Drive sync**. It is built as a native Rust application rather than a browser tab.

## A look inside

<div align="center">
<table>
  <tr>
    <td align="center" width="33%"><a href="docs/screenshots/01-focus-timer.png"><img src="docs/screenshots/01-focus-timer.png" alt="Pangan focus timer with circular progress ring and daily tasks" width="185" /></a></td>
    <td align="center" width="33%"><a href="docs/screenshots/02-persian-calendar.png"><img src="docs/screenshots/02-persian-calendar.png" alt="Pangan Persian calendar with Shahanshahi date and Sasanian day name" width="185" /></a></td>
    <td align="center" width="33%"><a href="docs/screenshots/03-historical-themes.png"><img src="docs/screenshots/03-historical-themes.png" alt="Pangan's historical Iranian theme settings" width="185" /></a></td>
  </tr>
  <tr>
    <td align="center"><sub><b>Focus timer</b><br />A calmer way to work</sub></td>
    <td align="center"><sub><b>Persian calendar</b><br />Time in cultural context</sub></td>
    <td align="center"><sub><b>Historical themes</b><br />A distinctive visual identity</sub></td>
  </tr>
</table>
</div>

<details>
<summary><b>View more screenshots</b> — events, personalization, and cloud sync</summary>
<br />
<div align="center">
<table>
  <tr>
    <td align="center"><a href="docs/screenshots/04-iranian-events.png"><img src="docs/screenshots/04-iranian-events.png" alt="Chronicle of Iranian historical and cultural events" width="175" /></a></td>
    <td align="center"><a href="docs/screenshots/05-appearance-and-audio.png"><img src="docs/screenshots/05-appearance-and-audio.png" alt="Pangan language, appearance, audio and automation settings" width="175" /></a></td>
    <td align="center"><a href="docs/screenshots/06-cloud-sync.png"><img src="docs/screenshots/06-cloud-sync.png" alt="Pangan Google Drive synchronization, backup and restore settings" width="175" /></a></td>
  </tr>
  <tr>
    <td align="center"><sub>Iranian events</sub></td>
    <td align="center"><sub>Personalization</sub></td>
    <td align="center"><sub>Cloud sync</sub></td>
  </tr>
</table>
</div>
</details>

---

## What you can do

### Focus without distractions

Set work, short-break, and long-break durations that fit your routine. Follow the circular progress timer, use quick session controls, and let the app handle the transition between sessions. A compact window mode is available when you want Pangan to take up less space.

### Plan the day, not just the next 25 minutes

Use tasks and a **daily rota** to connect focus sessions to specific work. Record your intentions before starting, then review what you actually completed.

### Explore the Iranian calendar

Navigate Persian calendar dates, historical events, and traditional **Sasanian day names**. Pangan also brings Iranian historical motifs into its appearance themes, including **Cyrus, Darius, Parthian, and Sasanian** styles.

### Understand your progress

See time spent focusing, completed sessions, and productivity trends. Pangan's journal features make it possible to reflect on the *outcome* of your work—not only the minutes counted.

### Make it yours

Choose light or dark appearance, Persian or English language, focus-session behavior, and Iranian-inspired procedural sounds. Desktop integrations include optional global hotkeys and automation commands.

### Keep your data under your control

Pangan stores productivity data locally and supports optional synchronization through your own Google Drive account, alongside backup and restore controls. Google Drive authorization may require configuration depending on the build.

---

## Download

Find the latest available desktop and mobile packages and checksums on the **[Releases page](https://github.com/DariushKhosrowan/pangan/releases/latest)**.

| Platform | Status |
| --- | --- |
| Linux | Desktop builds available |
| Windows | Desktop builds available |
| macOS | Desktop builds available |
| Android | Universal release APK (all ABIs, one file) |
| iOS | Simulator build available |

Choose a release asset for your operating system and CPU architecture. Actual availability is listed on the release page. Android users should install the single `pangan-android-universal.apk` (contains arm64, x86_64, and armeabi-v7a binaries). It installs on Android 7.0+ (API 24) devices — 64-bit arm64/x86_64 or 32-bit ARM. If you previously installed an older Pangan APK signed with a different key, uninstall it first or the new signature will be rejected.

## Build from source

Install a compatible [Rust toolchain](https://rustup.rs/) and the system libraries needed by Slint on your platform.

```bash
git clone https://github.com/DariushKhosrowan/pangan.git
cd pangan
cargo build --release
```

On Linux or macOS, run:

```bash
./target/release/pangan
```

On Windows, run `target\release\pangan.exe` (or launch the built executable from File Explorer).

To run the available Rust tests:

```bash
cargo test
```

The repository's build configuration and test scripts are the authoritative references if these commands change as mobile support is developed.

## Under the hood

| Area | Implementation |
| --- | --- |
| Core application | Rust |
| Interface | Slint |
| Calendar | Persian/Jalali and Iranian historical calendar features |
| Focus data | Locally persisted session records, tasks, and journal entries |
| Sound | Procedural Iranian-inspired audio synthesis |
| Synchronization | Optional Google Drive integration |
| Builds | GitHub Actions for supported desktop targets |

Pangan aims to remain a lightweight native tool with an identifiable Iranian character and straightforward source code.

## Contributing

Contributions are welcome, including bug reports, documentation, translations, accessibility improvements, platform fixes, and test coverage.

1. Check [existing issues](https://github.com/DariushKhosrowan/pangan/issues) before reporting a problem.
2. Keep pull requests focused and describe how you tested your changes.
3. Avoid unrelated interface changes in backend or platform-specific contributions.

[Open an issue](https://github.com/DariushKhosrowan/pangan/issues/new) · [Browse the source](https://github.com/DariushKhosrowan/pangan)

## Why the name Pangan?

**پنگان** refers to a traditional Persian water clock once used to divide and measure time. The name connects a historical approach to timekeeping with a modern tool for attention, planning, and reflection.

---

<div align="center">

**Make time for what matters.**

[Releases](https://github.com/DariushKhosrowan/pangan/releases) · [Issues](https://github.com/DariushKhosrowan/pangan/issues) · [Source](https://github.com/DariushKhosrowan/pangan)

<sub>Created by <a href="https://github.com/DariushKhosrowan">Dariush Khosrowan</a> · <a href="LICENSE">MIT License</a></sub>

</div>
