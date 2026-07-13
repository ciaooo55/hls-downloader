# Userscript Onboarding Design

## Goal

Make the installed application expose the bundled Tampermonkey userscript, show a concise Chinese usage guide at startup, and report whether the userscript has recently contacted the local downloader.

## Design

- The installer copies `userscript/m3u8-sniffer.user.js` into the application directory.
- The backend serves the script at `/userscript/m3u8-sniffer.user.js`, preserving the `.user.js` URL needed by userscript managers.
- The executable prints the UI, tutorial, and userscript installation URLs in its console, then opens `/help` in the default browser.
- `/help` is a small server-rendered Chinese page. It links to the application UI and userscript URL, refreshes periodically, and displays the current detection state.
- The userscript posts its version and current page URL to `/api/userscript/ping` after loading and once per minute. `/api/userscript/status` exposes the latest in-memory observation to authenticated local clients.
- Detection wording is deliberately limited to "recently detected running". Browser security prevents the application from reading Tampermonkey's installed-script list directly.

## State And Privacy

The backend stores only the most recent timestamp, userscript version, and page origin. It does not persist browsing history. A report less than 150 seconds old is considered currently detected; an older report is shown as previously seen.

## Error Handling

Userscript ping failures are silent so browsing is unaffected when the downloader is closed. Missing bundled script returns HTTP 404. Invalid API tokens return HTTP 401 through the existing token check.

## Verification

- Backend tests cover script serving, authenticated ping/status, stale status, and help-page content.
- Packaging tests assert that both staging and NSIS installation include the userscript.
- The final installer is smoke-tested by installing it, launching it, requesting the guide and script, posting a ping, checking status, and uninstalling it.
