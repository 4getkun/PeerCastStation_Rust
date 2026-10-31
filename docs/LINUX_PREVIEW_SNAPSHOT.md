# Linux preview source snapshot — 2026-10-31

This branch preserves the Linux preview implementation based on upstream
`2c529d61c6093650f428fcc3451eb7bb8d87f9ec` (1.2.4). It is a preview snapshot;
full-workspace validation has completed with the failures recorded below.
This branch does not merge to main, publish a new release, or change the
existing binary/source downloads.

## Current implementation

The native Tauri desktop runs with the Rust backend, external native players,
Linux recording paths, and Linux process reaping. Fresh Linux profiles start
with empty YP and automatic directory refresh disabled; saved user choices stay
intact. The Windows updater and C# autodetection are disabled on Linux.

Public login-free YouTube/Twitch registration, monitoring, listing and external
mpv playback are included. Their Linux path expects the pinned official
`yt-dlp_linux` 2026.08.19 artifact (SHA256
`58162f9bfdc27458ea47bfcb311cf47028f17d8154a8bf7d689861d46399230a`),
OS-managed Node and mpv. Helper binaries, Node and mpv are not committed here.
The managed helper location is
`<executable directory>/resources/live/linux-x86_64/yt-dlp_linux`.
The current live-player arguments use `--vo=wlshm`, verified for WSLg; general
Linux/X11 compatibility is not established. Cookie/login/config/plugin loading
and runtime remote-component download are disabled in the managed live path.

`LINUX_DESKTOP.md` records the earlier initial port stage and its limitations.
Its statements that native compilation was blocked and YouTube/Twitch were
disabled predate the later native/live work; use this snapshot note for the
current implementation and verification status.

## Verification already observed

- Native desktop check/build and actual GUI operation succeeded on Ubuntu 26.04,
  WSLg 1.0.79, x86_64. Other desktop environments and architectures remain untested.
- PeerCast synthetic playback, recording/stop/replay and a short public receive
  were exercised. External mpv exit/reaping was checked separately.
- YouTube/Twitch integrated GUI playback was observed. Audio decoding/output and
  the user's listening report were recorded separately; these are short tests,
  not long-duration or all-stream guarantees.
- Prior core tests: 373 passed. Twelve Node frontend suites passed, including
  installer branding. Cloud module/harness tests are separate from native GUI.
- Workspace rustfmt check passed using the existing Windows tool. Linux rustfmt
  and clippy components are not installed; Linux clippy is not claimed as run.
- The existing executable-permission unit test passed using a small isolated
  Unix fixture. Its permission expectations were not skipped or weakened.

## Differences from the published source archive

The published archive's 335-file application manifest was compared with this
working source. Three existing files differ before adding this note:

- `crates/peercast-desktop/tests/test_backend.rs`: Linux fresh settings expect an
  empty YP list; a saved single SP entry stays single on Linux. Non-Linux keeps
  the original five-directory defaults/migration assertions.
- `crates/peercast-desktop/src/main.rs` and
  `crates/peercast-pcp/src/yellow_page.rs`: rustfmt adjustments.

The root Cargo.lock is unchanged. The new icon.png is the original upstream
ICO's embedded PNG under the existing icon license. Existing GPL, Noto OFL and
Tabler MIT notices remain in the source; no external runtime binary is added.

## Full-workspace result (2026-10-31)

The follow-up run completed with `cargo test --workspace --locked --offline
--no-fail-fast`: **511 passed, 8 failed, 1 ignored** (exit 101). The existing
explicit public YP probe remained ignored; no ignored test was enabled merely
to obtain a result. Full-workspace validation is not green.

The eight failures are in three desktop integration-test targets:

- `test_desktop_preferences`: four failures in
  `legacy_default_yellow_pages_migrate_once_and_custom_lists_are_preserved`,
  `fresh_and_custom_or_empty_yellow_pages_have_expected_defaults`,
  `fresh_install_defaults_to_auto_refresh_and_reopens_with_it_enabled`, and
  `custom_player_and_rules_survive_limits_and_yp_saves_and_reopen`.
- `test_player`: three failures in
  `player_command_rejects_external_or_malformed_targets_and_missing_binary`,
  `invalid_contacts_are_omitted_without_changing_the_stream_target`, and
  `portable_player_command_keeps_three_literal_arguments_after_move`.
- `test_player_rules_paths`: one failure in
  `channel_metadata_cannot_add_arguments_or_expand_another_placeholder`.

Initial inspection identifies Windows YP/auto-refresh/bundled PCRPlayer
expectations and player fixtures lacking Linux executable semantics. These
failures remain open for platform-appropriate test/fixture review; they are
not reported as passing or as proven runtime fixes.

One additional stale assertion in `test_csharp_backend.rs` was changed from
unconditional auto-refresh enabled to `!cfg!(target_os = "linux")`, matching
the agreed Linux initial-profile policy and preserving the Windows assertion.
It passed in the full run. This is one further test-only difference from the
published source archive; runtime application code was not changed by this
validation follow-up. The changed file passed the existing Windows rustfmt
check, and `git diff --check` passed.

The previously intermittent
`directory_selection_creates_one_dynamic_relay_and_a_local_playlist` test
passed in both workspace attempts in this session (12/12 backend tests).
`runtime has stopped` was not reproduced; its original cause remains unknown,
so it is not declared fixed.

Additional results:

- Native workspace test-binary compilation (`--no-run`): passed.
- Release CLI build needed by the existing packaging tests: passed.
- Actual-module Linux policy harness: **28 passed**.
- Node frontend checks: **12/12 suites passed**, including installer branding.
- Existing executable-permission preflight: passed without weakening chmod
  expectations. Linux clippy remains not run (component not installed).

The older environment-disconnected session was confirmed stopped before a
single replacement runner started. Cargo target/cache/build output stayed on
a separate disk. Dedicated ext4 fixtures peaked at 64 KiB across these runs,
below the 100 MiB limit, with host free-space monitoring above 5 GiB. The
dedicated fixtures were removed and the original ignored target symlink was
restored after each run. No WSL repair/reconfiguration, main merge, release,
or site publication was performed.

Full-run log SHA256: `96e23f873ef1e0fc4f73858694c223f64f43661d0d8b55e5a4a9047588e9b2b0`.
Policy-harness log SHA256: `19c161a6dc3ff2bc4b61267215b3b7394282be0c9a6f02d5c5334c1e1b2f7474`.
Logs and build artifacts are retained locally and are not committed.

Continue validation with the provisioned native dependencies and an adequately
sized `CARGO_TARGET_DIR`. `cargo test --workspace --locked --offline` requires
its Cargo dependencies already cached. The separate actual-module harness is
`cargo test --manifest-path tools/linux/policy-tests/Cargo.toml --locked --offline`.
The existing upstream packaging tests look under repo_root()/target and expect
a release CLI binary; account for this when placing outputs on another disk.
