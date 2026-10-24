# Known defects

Open defects first, then fixed ones. One line each; details go in the log or an issue.

| ID | Status | Area | Description | Found | Fixed |
|---|---|---|---|---|---|
| DEF-1 | fixed | web | Choosing a built-in preset in the profiles bar did nothing: `structuredClone` of a reactive `$state` proxy throws; needs `$state.snapshot` first. Not covered by unit tests, found by the presets e2e test. | 2026-10-24 | 2026-10-24 |
