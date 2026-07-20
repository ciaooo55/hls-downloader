# Privacy Policy

HLS Downloader and its browser extension operate locally on the user's computer.
They do not send analytics, telemetry, browsing history, download history, or
personal information to the developer or to an advertising service.

The extension observes download and media response metadata so it can show files
available on the current page. When the user sends a resource to HLS Downloader,
the extension transfers the resource URL, filename, MIME type, source page,
Referer, Origin, and browser User-Agent to the locally installed desktop program
through Firefox or Chrome Native Messaging.

Cookie access is disabled per site until the user explicitly authorizes that site
in the extension popup. Authorized cookies are sent only to the local desktop
program so it can repeat an authenticated download request. The desktop program
stores credentials locally using Windows DPAPI encryption and does not expose
them through its task API or event stream.

Settings and detected-resource lists are stored in browser-local storage. Tasks,
configuration, temporary files, and completed downloads remain on the user's
computer. The user can delete tasks and associated files from the desktop app and
can revoke cookie authorization from the extension popup at any time.

The software does not execute remotely hosted code. Network requests are made only
to resources selected by the user and to GitHub Releases when the desktop user
explicitly checks for an application update.
