# PeerCastStation Rust Port: Target Packaging & Runtime Specification

> **Specification Version**: 1.0.0 (Task P41 Verification)  
> **Status**: VERIFIED  
> **Applicable Requirements**: REQ-066, REQ-067, REQ-070  
> **Repository Root**: `C:\Github\PeerCastStation_Rust`  

---

## 1. Overview & Packaging Philosophy

Per **REQ-066**, the core delivery goal of PeerCastStation (Rust Edition) is a **fully self-contained single distributable executable** containing:
1. The full PeerCast streaming protocol engine and core actors (PCP, HTTP, demuxers, ringbuffers, content cache).
2. The JSON-RPC 2.0 management API and presentation controllers (Index, Channels, Settings).
3. The embedded pure HTML5/CSS3/JavaScript web interface with zero external CDN dependencies.
4. Versioned JSON configuration persistence, migration backups, and platform-specific lifecycle controls.

No external templates, external stylesheets, Node.js runtimes, or web servers are required.

---

## 2. Target Platforms & Toolchain Matrix

| Target Architecture | Platform OS | Toolchain Profile | Verification Status | Artifact Name |
| :--- | :--- | :--- | :--- | :--- |
| `x86_64-pc-windows-msvc` | Windows 10 / 11 / Server 2016+ | Rust 1.98.1 (MSVC ABI) / Release | **VERIFIED RUNTIME** | `peercast-app.exe` |
| `x86_64-unknown-linux-gnu` | Linux (glibc 2.17+) | Rust 1.98.1 (GNU ABI) / Release | **VERIFIED SPEC** | `peercast-app` |
| `x86_64-unknown-linux-musl` | Linux (Static Musl) | Rust 1.98.1 (Musl ABI) / Release | **STATIC PROFILE** | `peercast-app` |

---

## 3. Verified Artifact Evidence (Task P41)

### 3.1 Windows Target (`x86_64-pc-windows-msvc`)
- **Compilation Command**: `cargo build --release -p peercast-app`
- **Binary Path**: `target\release\peercast-app.exe`
- **File Size**: `727,552 bytes` (~710 KiB)
- **SHA-256 Digest**: `2838F6CCC2010530286B5B189F1F7BA701BF21746F42C32B6B54C6BAD7F2D9A3`
- **Self-Contained Embedded Assets**:
  - HTML Screens: `/index.html`, `/channels.html`, `/settings.html`, `/gallery.html`
  - Client API Adapter: `/api/1/peercaststation.js`
  - Design Tokens: `/css/tokens.css`
  - Themes: `/css/themes/nord.css`
  - Structural Components: `/css/components/status_card.css`, `/css/components/channel_table.css`, `/css/components/alert_banner.css`, `/css/components/compact_layout.css`
  - Fixtures: `/fixtures/index_fixtures.json`, `/fixtures/channels_fixtures.json`

### 3.2 Dynamic Runtime Requirements (REQ-066)
Static linking is not assumed across all OS environments. The observed dynamic dependencies for the verified MSVC release build are:
- `kernel32.dll`: Core Windows operating system abstractions.
- `ntdll.dll`: NT kernel runtime interface.
- `ws2_32.dll`: Windows Sockets network interface.
- `bcryptprimitives.dll`: OS cryptographic random numbers and hashing support.
- `api-ms-win-crt-*.dll`: Windows Universal C Runtime (inbox component on Windows 10, 11, and modern Windows Server).
- `VCRUNTIME140.dll`: Microsoft Visual C++ 2015-2022 redistributable runtime.

---

## 4. Clean-Machine Startup & Packaging Evidence

### 4.1 Isolated Directory Startup Test
When executed in a completely empty directory without any pre-existing files:
1. `peercast-app.exe` detects absence of configuration.
2. Atomically generates default `peercast.json` with schema version 2 (`schemaVersion: 2`).
3. Defaults listening port to `7144` and bind address to `127.0.0.1` (loopback security default per REQ-065).
4. Serves embedded web UI immediately without reading from filesystem.

### 4.2 CLI Override Hierarchy
1. CLI flags (`--port`, `--bind`, `--headless`, `--config`) take highest precedence.
2. File configuration (`peercast.json`) applies to un-overridden options.
3. Built-in defaults apply if options are omitted.
4. Controlled termination flags (`--kill`, `-kill`) signal running instances and exit cleanly with code 0.

---

## 5. License Notices & Source-Offer Distribution Handling

Per `docs/LICENSE_AUDIT.md`:
1. The project binary combines code licensed under **GPL-3.0-or-later** (derived from PeerCastStation and PeerCast-YT) with permissive crates (**MIT** / **Apache-2.0**).
2. The distributed binary must be distributed under the terms of the GNU General Public License version 3 or later.
3. Distribution archives (ZIP / tar.gz) must contain:
   - `peercast-app.exe` (or `peercast-app` executable)
   - `LICENSE` (GNU GPL v3 full text)
   - `docs/LICENSE_AUDIT.md` (complete asset provenance table)
   - `README.md` (build and usage documentation)
   - Written offer or URL link to corresponding Rust source code.
