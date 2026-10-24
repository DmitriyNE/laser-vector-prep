# Known defects

Open defects first, then fixed ones. One line each; details go in the log or an issue.

| ID | Status | Area | Description | Found | Fixed |
|---|---|---|---|---|---|
| DEF-2 | fixed | web | Picking several files at once imported only the first: the input was reset while the store iterated its live `FileList` across awaits. Found by the Gerber pair e2e test. | 2026-10-24 | 2026-10-24 |
| DEF-1 | fixed | web | Choosing a built-in preset in the profiles bar did nothing: `structuredClone` of a reactive `$state` proxy throws; needs `$state.snapshot` first. Not covered by unit tests, found by the presets e2e test. | 2026-10-24 | 2026-10-24 |
