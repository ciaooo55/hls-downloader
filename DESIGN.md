# HLS Downloader 7 design contract

## Product structure

Compose is the only main workbench. Rust Core is the only task, transfer and database owner. WXT is the only browser integration. The native presenter owns only latency-sensitive temporary windows.

## Workbench geometry

- Windows title bar and 48dp command bar.
- Queue/category sidebar, virtualized task table and bottom status bar.
- One total progress indicator per task row; connection parts belong in details.
- Minimum effective workspace 1024x600 at 100%, 125% and 150% scaling.
- Long labels, translated text and bottom actions must remain visible.

## Visual system

- Compose Foundation primitives and the shared `WorkbenchComponents` layer.
- Restrained neutral surfaces with one blue action color.
- 6-10dp corner radius; glass/Mica only on title/chrome and temporary overlays.
- Vector icons, tooltips, focus rings and reduced-motion support.
- Hover, press, selection and status transitions must not shift layout.

## Interaction contract

Task selection supports Ctrl, Shift, select-all, keyboard navigation, secondary-click menus and drag ordering. Every action must map to a structured Core command and observable success/error result. Internal protocol or implementation names never appear in product copy.

## Release contract

The default branch does not publish a package until feature parity, real transfer, presenter latency, UI Automation, performance and clean-machine installation gates pass. Historical UI and behavior references are read from Git tags, not copied source trees.
