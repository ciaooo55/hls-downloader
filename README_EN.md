<div align="center">
  <img src="assets/app-icon.png" alt="HLS Downloader" width="112" />

  <h1>HLS Downloader</h1>

  <p><strong>More than an m3u8 downloader — a modern Windows download workbench.</strong></p>
  <p>Files · HLS · DASH · FTP · SFTP · BitTorrent · Browser handoff · Resume · Local playback · LAN casting</p>

  <p>
    <a href="README.md">简体中文</a> · <strong>English</strong>
  </p>

  <p>
    <a href="https://github.com/ciaooo55/hls-downloader/releases"><img src="https://img.shields.io/badge/Download-GitHub%20Releases-2ea44f?style=for-the-badge&logo=github" alt="Download" /></a>
    <a href="https://github.com/ciaooo55/hls-downloader/actions/workflows/ci.yml"><img src="https://github.com/ciaooo55/hls-downloader/actions/workflows/ci.yml/badge.svg" alt="CI" /></a>
    <a href="LICENSE"><img src="https://img.shields.io/github/license/ciaooo55/hls-downloader?style=flat-square" alt="License" /></a>
    <img src="https://img.shields.io/badge/platform-Windows%20x64-0078D4?style=flat-square&logo=windows11&logoColor=white" alt="Windows x64" />
    <img src="https://img.shields.io/badge/core-Rust-000000?style=flat-square&logo=rust&logoColor=white" alt="Rust Core" />
  </p>
</div>

---

HLS Downloader is a Windows-first desktop download manager that brings ordinary file downloads, HLS/DASH streaming media, FTP/SFTP, BitTorrent, and browser download handoff into one recoverable, persistent task queue.

Unlike downloaders whose transfer lifetime is tied to the visible window, HLS Downloader keeps active jobs inside an independent **Rust Core**. The desktop workbench, browser extension, and native presenter are clients of that Core, so closing the main window, browser, or player does not automatically terminate active transfers.

## 🚀 Download

The recommended way to get HLS Downloader is from **GitHub Releases**:

[![Open Releases](https://img.shields.io/badge/Open-Releases-2ea44f?style=for-the-badge&logo=github)](https://github.com/ciaooo55/hls-downloader/releases)

Release bundles may include Windows x64 **EXE / MSI / Portable ZIP** packages together with matching **Chromium / Firefox browser extensions**.

The current stable release is [7.0.4](https://github.com/ciaooo55/hls-downloader/releases/tag/v7.0.4), with EXE, MSI, portable ZIP, Chromium extension, and unsigned Firefox ZIP plus source. Windows installers are unsigned and require manual installation; automatic updates retain trusted-signature checks. Permanent installation in standard Firefox still requires a Mozilla-signed XPI. Historical candidates remain available for traceability. Consult the release notes and validation records for tested scenarios and limitations.

The installer lets you select the installation directory. New profiles save to the current Windows user's system Downloads folder, including folders moved to another drive. You can change this in settings or the download confirmation window. Personal tasks, downloaded files and service credentials are excluded from the package.

Current native implementation, component builds and validation are tracked in the [migration log](docs/validation/native-migration-progress.md) and [feature map](docs/validation/native-feature-mapping.md). The [historical delivery record](docs/validation/convergence.md) does not identify the current source or component builds. Canonical readiness permits frozen-source acceptance; formal delivery still requires matching packaged components and installation/startup gates.

### Requirements

| Item | Requirement |
| --- | --- |
| Operating system | Windows 10 or Windows 11 (x64) |
| CPU architecture | **x64 only.** Playback and part of the capture path rely on the x64 native libraries shipped in the package (libmpv, curl-impersonate), so ARM64 Windows cannot run it |
| Runtime | The native 7.0.4 stable release requires no Java and contains no JRE; historical Compose packages bundle a JRE |
| Privileges | MSI installation or upgrade requires administrator rights; see the corresponding release instructions for portable packages |
| Browser | Chromium-based (Chrome / Edge) or Firefox, for download handoff |
| Display | 1024×600 minimum; the UI follows the system DPI scaling |

> [!NOTE]
> **7.0.4 stable** was tested at source commit `8b6fa074`. `main` may continue with release-workflow and documentation maintenance; `ARTIFACT-MANIFEST.json` identifies the shipped components. Historical builds marked `candidate` or `pre-release` remain test builds.

Online subtitles can be enabled from the video player button or the extension popup, and dragged directly over the video; if the page has multiple videos, you can switch the bound player from the popup. Subtitle translation service, API key, global toggle, target language, sync mode, source text display, and default offset are configured in the desktop main window under `Settings -> Subtitle Translation` (the key is encrypted at rest by the local Core), while the browser extension only controls the per-page toggle, player selection, and starting/stopping subtitles; see the [QwenCloud subtitle guide](docs/guides/qwencloud-subtitles.md).

## ✨ Why HLS Downloader

| | |
| --- | --- |
| ⚡ **Resilient transfers**<br>Designed for long-running jobs with resume, retry, request recovery, and persistent task state. | 🎬 **HLS / DASH media**<br>Supports HLS and DASH for both VOD and live workflows, including media jobs that require request context. |
| 🌐 **Browser handoff**<br>Chromium and Firefox Manifest V3 extensions can discover downloads and media resources, then hand them to the desktop app through Native Messaging. | 🧠 **Independent Rust Core**<br>The Core owns transfer state and SQLite, so active downloads can continue even when the workbench, browser, or player is closed. |
| 📦 **One queue, multiple protocols**<br>HTTP/HTTPS, FTP, SFTP, BitTorrent, and streaming media share one task model and control path. | 🧩 **Desktop workbench**<br>Create tasks, paste or drop links, batch import, pause/resume/retry, inspect logs, speed, details, and connection state. |
| 📺 **Playback and casting**<br>Play downloaded media locally or publish it over the LAN for compatible devices / TVBox-style workflows. | 🛟 **Upgrade and rollback path**<br>Packaging and install flows include validation, Native Messaging Host registration, and rollback handling. |

## 🔌 Supported workflows

| Type | Support | Notes |
| --- | :---: | --- |
| HTTP / HTTPS | ✅ | Regular downloads, recovery, per-task request context |
| HLS / `.m3u8` | ✅ | VOD / Live |
| DASH / `.mpd` | ✅ | Streaming media downloads |
| FTP | ✅ | File transfer |
| SFTP | ✅ | SSH file transfer |
| BitTorrent / Magnet | ✅ | BT tasks and magnet-link workflows |
| Chromium extension | ✅ | Manifest V3 + Native Messaging |
| Firefox extension | ✅ | Manifest V3 + Native Messaging |
| Local playback | ✅ | Bundled media playback path in packaged builds |
| LAN casting | ✅ | LAN publishing / device selection / TVBox workflow |

## 🧭 Get started in three steps

1. Download and install the desktop app from [Releases](https://github.com/ciaooo55/hls-downloader/releases). If you want browser handoff, use the matching Chromium or Firefox extension as well.
2. Paste a URL, drop content into the workbench, create or batch-import tasks, or let the browser extension send discovered downloads and media resources to HLS Downloader.
3. Manage pause, resume, retry, logs, details, and playback from the workbench. The Core owns the download lifetime, so the main UI does not need to remain open for active jobs to exist.

## 🌐 How browser handoff works

In the current source, explicit links intercepted before a browser request are sent to the Core through Native Messaging. File and audio/video responses already requested by the browser, POST forms, and Blob/data-generated files finish in the browser and are then imported. The source file is removed only after a successful import; rejection or failure preserves it. Holding `Alt`, disabling takeover, or matching an exclusion rule keeps the download in the browser.

Preserving the original response avoids replaying one-use URLs. These transfers remain browser-owned until completion, so closing the browser can interrupt them and the desktop confirmation appears after completion. HLS/DASH manifests still use the Core media engine. Version 7.0.4 passed hosted Windows Edge / Firefox regressions for recognition, credentials, receipt recovery and real Host / Core handoff. Browser download creation remains recorded per entry point.

That keeps the browser as an entry point rather than the owner of the download lifecycle. Once a task has entered the Core, its persistent state is managed independently of the browser connection.

## 🏗️ Architecture

```mermaid
flowchart LR
    B["Chromium / Firefox"] -->|Native Messaging| H["Native Host"]
    H <-->|Named Pipe IPC| C["Rust Core"]
    UI["Rust + Slint Workbench / Presenter"] <-->|Named Pipe IPC| C
    C --> DB[(SQLite)]
    C --> E["HTTP · HLS · DASH · FTP · SFTP · BT"]
    C --> M["Local Player · LAN Cast"]
```

**The Core is the only owner of download state and SQLite**. The native workbench, extension, player and casting path cooperate through it. The stable 7.0.4 package uses the Rust / Slint native entry point; Compose source remains as a migration reference. Matching-source acceptance and publication records are in the [current evidence](docs/validation/current-evidence.json).

## 📁 Repository layout

| Path | Responsibility |
| --- | --- |
| `native_shell/` | Rust Core, SQLite, transfer engines and background media services |
| `crates/` | Shared protocol and lightweight IPC client |
| `native_clients/` | Native Host, CLI, updater and native launcher |
| `desktop_ui/` | Kotlin / Compose migration reference |
| `native_ui/` | Rust + Slint workbench, confirmation, progress and completion windows |
| `extension/` | WXT-based Chromium / Firefox Manifest V3 extension |
| `scripts/` | Windows build, test, install, upgrade, package, and verification scripts |
| `docs/` | [Documentation index](docs/README.md): guides, architecture, releases and validation evidence |
| `config/` | [Versioned product contracts](config/README.md), separate from generated output and user settings |

## 🛠️ Build from source

The active product line is Windows-first. On a clean development machine, bootstrap the pinned toolchain from the repository root:

```powershell
powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\scripts\bootstrap-v7-toolchain.ps1
```

Run the integrated test gate:

```powershell
.\scripts\build-v7.ps1 -Task test
```

Or test individual components:

```powershell
# Rust Core
cargo test --manifest-path native_shell/Cargo.toml --lib

# Native Presenter
cargo test --manifest-path native_ui/Cargo.toml

# Compose Desktop
cd desktop_ui
.\gradlew.bat test --no-daemon

# Browser Extension
cd ..\extension
pnpm install --frozen-lockfile
pnpm test
pnpm run build
```

For deeper implementation and verification details, see:

- [Maintenance script entry points](scripts/README.md)
- [Documentation index and file purposes](docs/README.md)
- [v7 architecture](docs/architecture/v7-architecture.md)
- [v7 module map](docs/architecture/v7-module-map.md)
- [v7 verification status](docs/validation/v7-verification.md)
- [Local upgrade and rollback](docs/guides/v7-local-upgrade.md)
- [Source layout and version history](docs/architecture/source-layout-and-history.md)

## 🧬 One repository, multiple generations

The complete active v7 source lives on `main`. Historical v3, v5, and v6 implementations remain available through Git tags instead of being copied into separate drifting repositories.

```powershell
git show v3.0.39:frontend/package.json
git show v5.0.13:backend/app/main.py
git show v6.0.1:native_ui/Cargo.toml
```

## 🔐 Security, privacy, and lawful use

Only download content that you are authorized to access, store, or process. Follow applicable laws, website terms, and content licenses in your jurisdiction.

Project documents: [LICENSE](LICENSE) · [SECURITY](SECURITY.md) · [PRIVACY](PRIVACY.md) · [TERMS](TERMS.md) · [Third-party notices](THIRD_PARTY_NOTICES.md)

## ⭐ Like the project?

If HLS Downloader is useful to you, consider giving the repository a **Star**. Bugs, compatibility problems, and feature requests are welcome in [Issues](https://github.com/ciaooo55/hls-downloader/issues).

<div align="center">
  <sub>Built for long-running downloads, browser handoff and resilient media workflows on Windows.</sub>
</div>
