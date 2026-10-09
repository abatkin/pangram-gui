# Agent notes

Linux desktop client for the Pangram AI-detection API: Rust + Qt 6 Quick/QML via CXX-Qt. Not
affiliated with Pangram. The README covers features, data locations and API findings; `plan/` has
the MVP plan, the reasons behind decisions (`DECISIONS.md`) and deferred ideas (`FUTURE.md`).

## Layout

- `crates/pangram-core`: no Qt. `api` (HTTP client), `analysis` (ranges and highlights), `storage`
  (SQLite and migrations), `settings`, `credentials`, `cost`, `service` (scan lifecycle).
- `crates/pangram-desktop`: `src/backend.rs` is the only CXX-Qt bridge; `qml/` holds the views.
  QML binds properties and calls commands. Keep logic in Rust, ideally in core.
- A new QML file must also be added to `QML_FILES` in `crates/pangram-desktop/build.rs`.

## Commands

```sh
cargo fmt --all
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
QT_QPA_PLATFORM=offscreen scripts/ui-smoke.sh   # end-to-end UI test against the mock API
```

CI runs all four (`.github/workflows/ci.yml`), so run them before calling work done. UI changes
should get a step in `crates/pangram-desktop/tests/ui/smoke.qml`. Pass a directory to
`ui-smoke.sh` to get screenshots you can look at.

## Rules

- **Never spend API credits.** Tests use wiremock or `examples/mock_server.rs`. The tests in
  `tests/opt_in.rs` and the `live_probe` example make real scans; run them only when the user asks.
- Never log or persist API keys, document text or response bodies outside the history database.
  Keys live only in Secret Service or memory.
- Never resend a `POST /task` whose outcome is uncertain: it may be billed twice. Only GETs retry.
- Highlight ranges address Pangram's *returned* text, not the submitted text. Validate them, and
  withhold highlights rather than draw wrong ones.
- Versions are pinned on purpose: Rust 1.98.1 (`rust-toolchain.toml`), CXX-Qt `=0.10.0`, Qt 6.11.
  Ask before changing them.
- A new system build dependency goes in both the README requirements and
  `.github/actions/setup/action.yml`. CI and release builds run in a `fedora:44` container.
- Releases: bump `[workspace.package] version` in `Cargo.toml`, then run the Release workflow on
  `main`. Don't create tags by hand.
- Debug builds only: `--qml-script`. Test environment variables: `PANGRAM_API_BASE`,
  `PANGRAM_CREDENTIAL_STORE=memory`, `MOCK_PORT`, `MOCK_DELAY_MS`.
