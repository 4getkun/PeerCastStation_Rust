#!/bin/sh
# First-run config is local-only and has no automatic directory requests.
set -eu
ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
if [ "$(uname -s)" != Linux ]; then echo 'This launcher is for Linux.' >&2; exit 1; fi
if [ "$#" -gt 1 ]; then echo 'Usage: run-desktop.sh [settings.json]' >&2; exit 2; fi
SETTINGS=${1:-${XDG_DATA_HOME:-${HOME:?HOME required}/.local/share}/com.fourgetkun.peercast-rt/linux-desktop.json}
case "$SETTINGS" in /*) ;; *) echo 'Use an absolute settings path.' >&2; exit 2;; esac
pkg-config --exists gtk+-3.0 webkit2gtk-4.1 alsa || {
  echo 'Install GTK3, WebKitGTK 4.1, ALSA development packages, libayatana-appindicator3-dev and a Rust toolchain first (see docs/LINUX_DESKTOP.md).' >&2
  exit 1
}
if [ ! -f "$SETTINGS" ]; then
  mkdir -p -- "$(dirname -- "$SETTINGS")"
  # Noclobber also protects a file created after the existence check.
  (umask 077; set -C; cat "$ROOT/tools/linux/offline-desktop.json" > "$SETTINGS")
fi
if [ ! -x /usr/bin/mpv ]; then
  echo 'mpv is not installed at /usr/bin/mpv. Choose an absolute player executable in Settings; ffplay users should use "<directstream/>" as its argument.' >&2
fi
cd "$ROOT"
exec cargo run --locked -p peercast-desktop -- --settings "$SETTINGS"
