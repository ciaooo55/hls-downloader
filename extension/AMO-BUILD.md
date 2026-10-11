# Firefox reviewer build instructions

The submitted Firefox ZIP is built entirely from the files in this source archive.
No private packages, generated source files, or remote build services are required.

## Environment

- Windows 11 x64 (the same commands also work on Linux)
- Node.js 24.19.0
- pnpm 11.7.0

Install Node.js from <https://nodejs.org/en/download>. Install the pinned package
manager with `npm install --global pnpm@11.7.0`.

## Build

From this directory run:

```powershell
pnpm install --frozen-lockfile
pnpm run build:firefox
pnpm exec web-ext lint --source-dir .output/firefox-mv3
```

The unpacked extension is written to `.output/firefox-mv3`. To create the upload
ZIP, run `pnpm run zip:firefox`. WXT places the archive in `.output`.

Dependencies are installed from pnpm's public npm registry.
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
  (including redirects and response filenames), pause GET downloads, and send them to the
  desktop confirmation dialog. The browser download remains paused until the
  user accepts and the desktop task is ready. The browser copy is then canceled
  and erased; rejection or a failed handoff resumes the browser download.
  POST downloads and client-generated Blob/data exports finish in the browser
  first; automatic takeover does not pause or replay POST requests. The extension
  sends the completed file's local path to the desktop confirmation dialog.
  After acceptance the desktop copies the file into its task output; only a
  completed desktop task permits deleting the browser file and download entry.
  Rejection or copying failure retains the original browser file.
- `nativeMessaging`: communicate with the locally installed HLS Downloader over
  a persistent Native Messaging connection so browser downloads can be paused
  and transferred without launching a new helper process for every request.
- `webRequest` and `<all_urls>`: observe response metadata to identify downloadable
  files and media on the page the user visits.
- `cookies`: read cookies only for hosts the user explicitly authorizes in the
  popup, then pass them to the local downloader for authenticated requests.
- `storage`: store takeover preferences, authorized hosts, and the current tab's
  detected-resource list locally.
- `contextMenus`: send a selected link, media element, or selected links to the
  desktop confirmation dialog.
- `alarms`: expire paused handoffs and restore browser downloads when the desktop
  application cannot complete the handoff.

Firefox data transmission permissions are declared as `browsingActivity`,
`websiteContent`, and `authenticationInfo`. Native Messaging transfers outside
the browser are included in this declaration, even though the developer does not
receive those messages. The payloads include the selected resource URL, page
identity, media metadata, replay headers, explicitly authorized cookies, and
user-configured subtitle service credentials, and completed local export paths.

No analytics, advertising, telemetry, or remote code is used. URLs and optionally
authorized cookies are sent only to the Native Messaging application on the same
Windows computer. When the user explicitly starts subtitle translation, captured
webpage audio is sent to the local desktop application, which connects to the
user-configured Alibaba Cloud Bailian service. Translation is disabled by default.
The user provides the service workspace and API key; no developer-hosted
translation service or shared credentials are used. `PRIVACY.md` describes these
extension-specific flows.

## Verification without service credentials

Install and start the HLS Downloader desktop application on Windows. The browser
extension relies on its registered Native Messaging host; it is not a standalone
download engine. To review the UI or build without the desktop application, the
extension displays its desktop-disconnected state.

Download detection, confirmation, cancellation, retry, popup controls, and
authenticated handoff can be exercised without a subtitle API key. Cloud subtitle
translation requires the user's own Bailian account. Local capture and message
handling tests use a fixture; those tests do not claim that a real cloud service
request succeeded.

The content script builds its Shadow DOM with DOM APIs and renders resource names,
URLs, MIME values, and native-host errors through `textContent`. The popup builds
SVG icons with `createElementNS` and static shape attributes. No captured page
value is assigned to HTML parsing APIs.
