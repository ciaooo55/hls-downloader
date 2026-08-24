# Firefox reviewer build instructions

The submitted Firefox ZIP is built entirely from the files in this source archive.
No private packages, generated source files, or remote build services are required.

## Environment

- Windows 11 x64 (the same commands also work on Linux)
- Node.js 25
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

The project uses WXT 0.20.27, TypeScript 5.8.3 and pnpm's public npm registry.
The popup and in-page controls use browser DOM APIs rather than a remote UI
runtime. Exact direct and transitive dependency versions are recorded in
`pnpm-lock.yaml`.

## Store identity continuity

- Chromium extension ID: `bbdfldcjnikaemnimalegbopgaknjhla`
- Firefox extension ID: `hls-downloader-store@ciaooo55.com`

These are the same identities used by the 3.0.39 packages. The Chromium public
key and Firefox Gecko ID are defined in `lib/storeIdentity.ts` and covered by
automated tests so an update cannot accidentally create a second listing.

## Permission notes

- `downloads`: after a real link or an explicit download control is clicked,
  observe the real browser download
  (including redirects and response filenames), pause it, and send it to the
  desktop confirmation dialog. Once the desktop acknowledges that the dialog is
  open, the browser copy is canceled and erased. It is restored only when the
  desktop app cannot receive the handoff.
- `nativeMessaging`: communicate with the locally installed HLS Downloader over
  a persistent Native Messaging connection so browser downloads can be paused
  and transferred without launching a new helper process for every request.
- `webRequest` and `<all_urls>`: observe response metadata to identify downloadable
  files and media on the page the user visits.
- `cookies`: read cookies only for hosts the user explicitly authorizes in the
  popup, then pass them to the local downloader for authenticated requests.
- `storage`: store takeover preferences, authorized hosts, and the current tab's
  detected-resource list locally.

No analytics, advertising, telemetry, or remote code is used. URLs and optionally
authorized cookies are sent only to the Native Messaging application on the same
Windows computer.

The content script builds its Shadow DOM with DOM APIs and renders resource names,
URLs, MIME values, and native-host errors through `textContent`. No captured page
value is assigned to HTML parsing APIs.
