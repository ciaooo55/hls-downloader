# Firefox reviewer build instructions

The submitted Firefox ZIP is built entirely from the files in this source archive.
No private packages, generated source files, or remote build services are required.

## Environment

- Windows 11 x64 (the same commands also work on Linux)
- Node.js 24
- Corepack
- pnpm 11.7.0

## Build

From this directory run:

```powershell
corepack enable
corepack prepare pnpm@11.7.0 --activate
pnpm install --frozen-lockfile
pnpm run build:firefox
```

The unpacked extension is written to `.output/firefox-mv3`. To create the upload
ZIP, run `pnpm run zip:firefox`. WXT places the archive in `.output`.

The project uses WXT 0.20.27, TypeScript 5.8.3, React 19.1.0, and pnpm's public
npm registry. Exact direct and transitive dependency versions are recorded in
`pnpm-lock.yaml`.

## Permission notes

- `downloads`: use an explicit file-link click to hand off directly to the
  desktop app, and pause dynamic browser downloads as a fallback. The browser
  copy is restored when the local handoff fails.
- `nativeMessaging`: communicate with the locally installed HLS Downloader.
- `webRequest` and `<all_urls>`: observe response metadata to identify downloadable
  files and media on the page the user visits.
- `cookies`: read cookies only for hosts the user explicitly authorizes in the
  popup, then pass them to the local downloader for authenticated requests.
- `storage`: store takeover preferences, authorized hosts, and the current tab's
  detected-resource list locally.

No analytics, advertising, telemetry, or remote code is used. URLs and optionally
authorized cookies are sent only to the Native Messaging application on the same
Windows computer.

`web-ext lint` reports `UNSAFE_VAR_ASSIGNMENT` warnings for the static Shadow DOM
template and React's bundled DOM runtime. Resource names, URLs, MIME values, and
native-host errors are rendered through `textContent` or React text nodes; no
captured page value is assigned to `innerHTML`.
