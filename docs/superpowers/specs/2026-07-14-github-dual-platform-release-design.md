# GitHub Dual-Platform Release Design

## Goal

Publish HLS Downloader as a public, source-only GitHub repository under
`ciaooo55/hls-downloader`. GitHub Actions must test the project and build usable
Windows and Linux release packages without committing generated executables,
FFmpeg binaries, dependency directories, or runtime data to Git.

## Supported Platforms And Artifacts

The first public release supports x86-64 Windows and x86-64 Ubuntu/Debian Linux.
Each versioned GitHub Release contains:

- `HLSDownloader-Windows-x64-Setup.exe`
- `HLSDownloader-Windows-x64-Portable.zip`
- `HLSDownloader-Linux-x64.deb`
- `HLSDownloader-Linux-x64.tar.gz`
- `m3u8-sniffer.user.js`
- `SHA256SUMS.txt`

Windows 10 and 11 are supported. The Linux `.deb` package is the primary Linux
distribution and targets currently supported Ubuntu/Debian x86-64 releases. The
Linux tarball is a portable archive but may still require GTK/WebKit system
libraries documented in the README.

## Repository Contents

The repository contains source code, tests, dependency lock files, installer
definitions, build scripts, documentation, and GitHub workflow definitions. It
does not contain local FFmpeg executables, finished release packages, build
directories, frontend dependencies, SQLite databases, WebView profiles,
downloads, or credentials.

The repository uses the MIT license. The README is written primarily in Chinese
and includes features, platform support, installation, userscript setup, source
development, local packaging, automated release instructions, limitations, and
troubleshooting. Build and test badges show the current GitHub Actions status.

## Cross-Platform Runtime

Platform-specific behavior is isolated behind runtime helpers:

- Windows uses Edge Chromium through pywebview and packaged `ffmpeg.exe` and
  `ffprobe.exe` binaries.
- Linux uses the GTK/WebKit pywebview backend and Unix FFmpeg paths.
- Startup errors use a native Windows dialog on Windows and a Linux-compatible
  dialog or stderr fallback on Linux.
- File browsing lists Windows drives only on Windows and begins from the user's
  home directory on Linux.
- Resource paths and writable data paths are distinct. Installed Linux assets
  may be read-only, while configuration, the task database, temporary files, and
  downloads remain writable in the current user's data directory.

Portable archives keep their writable state beside the executable when a
portable marker is present. Installed packages use platform-appropriate user
data locations. Existing relative configuration values remain supported.

## Continuous Integration

The CI workflow runs for pushes and pull requests. Python tests run on both
`windows-latest` and `ubuntu-latest`. Frontend dependency installation, Vitest,
and the TypeScript/Vite production build run with the checked-in pnpm lockfile.
CI does not upload application installers.

## Automated Releases

The release workflow can be started manually and also runs for tags matching
`v*`. Separate Windows and Linux jobs install pinned toolchains, restore caches,
download or install FFmpeg, build the frontend, package the Python application,
and smoke-test the staged application.

The Windows job produces the NSIS installer and portable ZIP. The Linux job
produces the Debian package and portable tarball, including a desktop entry and
the required runtime layout. A final release job downloads all job artifacts,
creates `SHA256SUMS.txt`, and verifies that every expected file exists.

A tag-triggered run creates the corresponding GitHub Release and uploads all
verified artifacts. A manually triggered run uploads workflow artifacts for
inspection but does not create a public GitHub Release unless a valid version
tag was supplied.

## Dependency And Supply-Chain Handling

Python and frontend versions are driven by checked-in requirement and lock
files. GitHub Actions are pinned to stable major versions. FFmpeg and other
downloaded packaging inputs use explicit sources and integrity checks whenever
the upstream provides stable checksums. Tokens are provided only by GitHub's
built-in `GITHUB_TOKEN`; local personal tokens are never written to the
repository or workflow files.

## Error Handling

Each build stops immediately when dependency installation, compilation,
packaging, smoke testing, or checksum generation fails. The release job does not
publish a partial release. Artifact names include platform and architecture so
users can select the correct download without opening an archive.

## Verification

Acceptance requires:

- Existing Python tests pass on Windows and Linux.
- Existing frontend tests and the production build pass.
- New tests cover platform-specific paths, data/resource separation, GUI backend
  selection, file browsing roots, and package layout.
- Both packaged applications answer the local health endpoint during a smoke
  test and terminate without leaving the API port occupied.
- The Windows installer, Windows portable archive, Linux Debian package, Linux
  portable archive, userscript, and checksum manifest are all generated.
- A GitHub Actions manual run completes before the first version tag is created.

## Non-Goals

This release does not add macOS or ARM builds, Linux distributions outside the
Ubuntu/Debian family, code signing, paid certificates, automatic in-app updates,
live HLS support, DRM support, or independent audio/subtitle packaging.
