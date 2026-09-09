# v7 UI polish

The v7 visual pass keeps the v3 workbench geometry and the existing blue action
color, then applies the same interaction rules to the Compose desktop app and
the WXT popup.

## Desktop workbench

- `WorkbenchComponents` now owns hover, press, focus and disabled feedback.
- Buttons use a 150-200ms color transition and a small press scale; progress,
  switches, radios and checkboxes animate state changes without changing layout.
- Task rows distinguish hover and selection, while status badges add a compact
  state dot. The task row still renders one total progress track; segment and
  connection details stay below it.
- Empty state has a bounded icon panel and a direct next-step sentence instead
  of an unstructured blank area.
- Dialogs enter with a short fade/scale transition and still use an opaque
  fallback surface for machines without Mica/blur support.

## Browser extension

- Popup theme, open, close, download, TVBox, cast, quality and selected-state
  controls use consistent inline vector icons instead of character glyphs.
- Popup cards, menus and status indicators have restrained hover, focus, busy,
  online-pulse and reduced-motion behavior.
- The shared theme token sheet remains the single source for light/dark colors,
  borders, focus rings and motion timing.

## Verification

Compose tests and the WXT suite are green after the pass. The opt-in local UI
test API validates the installed 1280x760 workbench, icon, window visibility,
authorization boundary and nonblank PNG output without relying on Computer Use.
The API also exposes stable task-selection state for Ctrl, Shift and marquee
selection tests; it is disabled during normal Start menu launches.
