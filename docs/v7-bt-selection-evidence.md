# BT Dynamic File Selection Evidence

This evidence covers the Core coordinator path for a multi-file swarm transfer
where the selection sidecar changes while a block is in flight. The loopback
tracker and peer withhold the deselected file, the reader sends a BitTorrent
Cancel for that block, and the selected file completes. The test then verifies
that the deselected range remains zero, the selected range contains the
expected payload, the task reports the selected byte count, and only the
remaining file is published.

## Reproducible command

From the repository root, run this command in either Windows PowerShell 5.1
or PowerShell 7:

```powershell
pwsh -NoProfile -File .\scripts\verify-v7-bt-selection.ps1 -Runs 3
```

The script locates `cargo.exe` at `$HOME\.cargo\bin\cargo.exe` when Cargo is
not already on `PATH`. It runs the Core coordinator cancellation test three
times and then runs the multi-file resume/materialization test once.

## Recorded result

Command:

```text
pwsh -NoProfile -File .\scripts\verify-v7-bt-selection.ps1 -Runs 3
```

Output:

```text
BT_SELECTION_CANCEL_RUN=1 EXIT=0 RESULT=cancelled_deselected_file_other_file_completed
BT_SELECTION_CANCEL_RUN=2 EXIT=0 RESULT=cancelled_deselected_file_other_file_completed
BT_SELECTION_CANCEL_RUN=3 EXIT=0 RESULT=cancelled_deselected_file_other_file_completed
BT_SELECTION_RESUME_RUN=1 EXIT=0 RESULT=missing_pieces_reused_selected_files_materialized
BT_SELECTION_EVIDENCE=PASS RUNS=3
```

Process exit status: `0`.

The underlying tests are
`download_worker::tests::live_torrent_selection_update_cancels_requested_file_and_publishes_remaining_file`
and
`torrent_engine::tests::multifile_swarm_resumes_without_refetching_and_materializes_selection`.
