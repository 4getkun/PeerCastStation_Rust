# Linux PeerCast desktop development candidate

Based on upstream `4getkun/PeerCastStation_Rust` commit
`2c529d61c6093650f428fcc3451eb7bb8d87f9ec` (1.2.4).
This is a source port candidate, **not a tested native Linux release**.

## Deliberately narrow scope

| Feature | Linux policy / verification |
| --- | --- |
| PeerCast directory, relay and local HTTP playback | Retained; upstream core separately tested. Native desktop integration not run. |
| External mpv | `/usr/bin/mpv`, URL-only default; executable file + Unix mode validated. Command construction tested, no shell. |
| Other external players | Absolute executable path and configurable argument template. For ffplay use `"<directstream/>"` (ffplay does not consume m3u playlists reliably). |
| PCRPlayer | Not bundled or launched on Linux. Windows behavior retained. |
| YouTube / Twitch | Disabled in UI and backend; no Linux monitor tasks created. |
| Automatic updater | Disabled in UI/backend; Windows updater plugin not registered on Linux. Manual updates only. |
| C# backend autodetection | Disabled on Linux; native Rust backend used. |
| Recordings | Relative paths resolve below `$XDG_DATA_HOME/com.fourgetkun.peercast-rt/` or `$HOME/.local/share/com.fourgetkun.peercast-rt/`; absolute user choice preserved. |
| Open recording folder | `/usr/bin/xdg-open` with a separate path argument, not Explorer or a shell. |
| Autostart, tray, notifications, audio | Existing cross-platform code retained; Linux runtime behavior unverified. |

Argument syntax intentionally stays the existing double-quote/backslash parser on both
platforms. It is **not a shell command**: `$()`, semicolons, single quotes, environment
variables and `~` are not shell-expanded. Missing/non-executable saved player paths
do not prevent opening Settings: availability is shown separately, and launch checks
fail with an error until repaired. Metadata substitution happens after argument
splitting. Use an absolute player executable path; permission checks do not guarantee
compatibility or that the filesystem allows execution (spawn errors are reported).
Windows configuration files are not migrated automatically: use the separate Linux
configuration, and do not copy a PCRPlayer `.exe` path or its three-argument template.

## Build / launch on a provisioned Linux desktop

Needs a Rust toolchain compatible with the pinned Cargo.lock, pkg-config, GTK3,
WebKitGTK 4.1, ALSA and AppIndicator development libraries, a graphical session,
and mpv (or another explicitly selected native player).
On Debian/Ubuntu the usual package names are `build-essential pkg-config
libgtk-3-dev libwebkit2gtk-4.1-dev libasound2-dev libayatana-appindicator3-dev
librsvg2-dev patchelf mpv xdg-utils`. Install through your normal administrator flow.
This patch does not install packages or change firewall/router/security settings.

From the repository root:

```sh
cargo check --locked -p peercast-desktop
sh tools/linux/run-desktop.sh
```

All Linux entry paths (direct binary, cargo run, and launcher) now default to empty
YP and auto-refresh off. Explicit saved user choices remain unchanged. The native
default profile is `linux-desktop.json`; Windows keeps its existing profile/defaults.

The launcher creates a private, separate `linux-desktop.json` only if absent, with
empty YP/channels, auto-refresh off, recording/autostart/port-mapping off, and the
YP migration marker set. The backend binds its embedded service to a random loopback
port. Add a directory in Settings and refresh deliberately when ready for real usage.
Do not present the offline fixture as proof of live PeerCast interoperability.
The launcher checks native prerequisites before creating configuration or starting
anything. It does not erase or replace existing settings.

For an explicit test configuration:

```sh
sh tools/linux/run-desktop.sh /absolute/path/to/separate-test-settings.json
```

## Validation completed in the cloud

- Git blob SHA-1 verified for 103 downloaded upstream desktop files before editing.
- Twenty Rust tests passed in a GTK-free harness importing actual desktop production
  modules, including backend/preferences/player/recording. Fresh backend start and
  restart verify empty YP and auto-refresh off. Missing and non-executable saved
  players do not prevent startup/settings repair; snapshot warns unavailable and
  launch still rejects them. Also includes URL-only default, recording location,
  shell-metacharacter argument boundaries and direct stream option.
- Ten Node suites passed: nine relevant upstream suites plus `linux_frontend.cjs`.
  These are VM/stub tests, not screenshots or native GUI interaction.
- Rust source parsed/formatted with rustfmt; launch script passed `sh -n`.
- Native `cargo check --locked --offline -p peercast-desktop` blocked on uncached
  `alsa v0.11.0`; independent pkg-config check also confirms GTK3, WebKitGTK4.1 and ALSA
  development packages missing. Therefore desktop compilation, native GUI launch,
  real media playback, recording round-trip, and full workspace/clippy are **not passed**.
- Windows installer branding test not run successfully because its packaging script
  was not materialized. Windows binary build/regression testing remains necessary.

The connector could not materialize the large NotoSansJP font or settings-header
background. Existing font stacks and solid CSS background provide fallbacks. Windows
installer-only large assets also omitted. These omissions do not constitute a complete
upstream checkout or a redistributable release bundle; restore them from the pinned
upstream for production packaging. Keep upstream licenses and notices with any bundle.

Reproduce the GTK-free checks from the source root:

```sh
cargo test --locked --offline --manifest-path tools/linux/policy-tests/Cargo.toml
node crates/peercast-desktop/tests/linux_frontend.cjs
```

The first command needs the dependencies in Cargo cache (omit `--offline` in an
approved build environment to fetch them). It compiles actual production backend, preferences, player, recording
and platform modules, but deliberately does not claim to type-check Tauri UI code.

## Next acceptance gate

On a provisioned graphical Linux test environment, use the offline config first:
open settings, cancel file picker, select mpv, save and reopen, verify disabled service
and update controls, close/reopen app, inspect actual window/tray behavior. Then use a
local synthetic stream to verify play, record, stop, replay, folder opening and restart
persistence. Only then try a user-selected public YP/channel and conduct a soak test.
