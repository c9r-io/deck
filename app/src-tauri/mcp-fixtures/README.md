# MCP contract fixtures

Shared by the three MCP crates: the app (`src/mcp/`), the STDIO adapter
(`mcp-adapter/`) and the pane runner (`mcp-runner/`). Each crate's own unit
tests read these files with `include_str!` and compare them with **its own**
constants; no crate parses another crate's source. Changing a limit, error,
flag or tool on one side without the fixture fails that side's tests; changing
the fixture fails every side that did not follow.

Not bundled: nothing here is copied into the app, the adapter or the runner.
