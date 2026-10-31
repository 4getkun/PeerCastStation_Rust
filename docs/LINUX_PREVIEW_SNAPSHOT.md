# Linux preview source snapshot — 2026-10-31

This branch preserves the Linux preview implementation based on upstream
`2c529d61c6093650f428fcc3451eb7bb8d87f9ec` (1.2.4). It is a preview snapshot;
full-workspace validation is still in progress. This commit does not merge to
main, publish a new release, or change the existing binary/source downloads.

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

## Outstanding validation

A previous full native desktop run had two OS-default expectation failures,
addressed by the explicit OS-specific tests above, and one intermittent
`directory_selection_creates_one_dynamic_relay_and_a_local_playlist` failure
with `runtime has stopped`. That test passed alone; the full-run failure is
still unresolved and is not reported as fixed.

A subsequent full-workspace attempt stopped during compilation with EIO/SIGBUS
when the Windows host disk filled. The new run keeps Cargo target/cache/build
output on a separate disk, with a small dedicated ext4 test fixture directory,
a 100 MiB fixture limit and at least 5 GiB host free space. At this snapshot,
cache preparation is running and the new full-suite result is pending. It
will be recorded separately. No tests, including the ignored explicit public
YP probe, are enabled/disabled merely to claim success.

Continue validation with the provisioned native dependencies and an adequately
sized `CARGO_TARGET_DIR`. `cargo test --workspace --locked --offline` requires
its Cargo dependencies already cached. The separate actual-module harness is
`cargo test --manifest-path tools/linux/policy-tests/Cargo.toml --locked --offline`.
The existing upstream packaging tests look under repo_root()/target and expect
a release CLI binary; account for this when placing outputs on another disk.
