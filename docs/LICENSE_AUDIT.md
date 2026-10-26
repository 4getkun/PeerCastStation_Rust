# PeerCastStation Rust License Audit & Asset Provenance Manifest (REQ-066)

## 1. Executive Summary

This document establishes the official license audit and redistribution clearance record for `PeerCastStation_Rust`. In accordance with **REQ-066**:
- Source code, embedded UI assets, bundled fonts, and third-party dependencies are inventoried with author, origin, license, and distribution obligations.
- Source reuse is recognized under the original GNU General Public License terms (a Rust reimplementation does not eliminate copyright or license obligations).
- All bundled assets are verified 100% offline-capable with **zero external CDN dependencies**, preventing silent rendering failure on isolated networks.

---

## 2. Project Licensing & Component Provenance

| Component / Subsystem | Origin / Upstream Repository | Upstream License | Copyright Holder(s) | Distribution Obligation | Bundling Status |
|---|---|---|---|---|---|
| **PeerCastStation Rust Core** | `C:\Github\PeerCastStation_Rust` | **GPL-3.0-or-later** | PeerCastStation Rust Contributors | Distribute source code under GPL-3.0; include copyright notices. | Active Core |
| **PeerCastStation (Upstream C#)** | `https://github.com/kumaryu/peercaststation` (`390c4e84`) | **GPL-3.0-or-later** | kumaryu and contributors | Offer complete source code; maintain copyright headers. | Reference & Protocol Upstream |
| **PeerCast-YT (Upstream C++)** | `https://github.com/plonk/peercast-yt` (`b60f1763`) | **GPL-2.0-or-later** | plonk and PeerCast authors | Forward-compatible with GPL-3.0-or-later; preserve notices. | Protocol Reference |
| **JavaScript Client Adapter** | `APIHost.cs` (`PeerCastStationJSApp`) | **GPL-3.0-or-later** | kumaryu and contributors | Embedded as `/api/1/peercaststation.js`. | Audited & Bundled |
| **Semantic Design Tokens** | `ui/css/tokens.css` | **GPL-3.0-or-later** | PeerCastStation Rust Contributors | Embedded in static HTTP server. | Audited & Bundled |
| **Component Stylesheets** | `ui/css/components/*.css` | **GPL-3.0-or-later** | PeerCastStation Rust Contributors | Embedded in static HTTP server. | Audited & Bundled |
| **Theme Stylesheets** | `ui/css/themes/nord.css` | **MIT / GPL-3.0-or-later** | Arctic Ice Studio & Contributors | Nord color palette; MIT compatible with GPL-3.0. | Audited & Bundled |
| **Presentation Fixtures** | `ui/fixtures/*.json`, `gallery.html` | **GPL-3.0-or-later** | PeerCastStation Rust Contributors | Offline test/dev fixture gallery. | Audited & Bundled |
| **Favorite Notification Chime** | `tools/generate-notification-sound.cjs` | **GPL-3.0-or-later** | PeerCastStation Rust Contributors | Original synthesized audio; no external recordings or samples. | Bundled in Desktop |
| **Knockout.js** | `https://github.com/knockout/knockout` (v3.5.1) | **MIT** | Knockout team | Preserve copyright & permission notice. | Upstream UI Dependency |
| **Bootstrap** | `https://github.com/twbs/bootstrap` (v5.3.3) | **MIT** | Twitter, Inc. / The Bootstrap Authors | Preserve copyright & permission notice. | Upstream UI Dependency |
| **Bootswatch Cerulean** | `https://github.com/thomaspark/bootswatch` | **MIT** | Thomas Park | Preserve copyright & permission notice. | Upstream UI Dependency |
| **jQuery** | `https://github.com/jquery/jquery` (v3.7.1) | **MIT** | OpenJS Foundation and contributors | Preserve copyright & permission notice. | Upstream UI Dependency |
| **Popper.js** | `https://github.com/floating-ui/floating-ui` (v2.11.8) | **MIT** | Federico Zivolo and contributors | Preserve copyright & permission notice. | Upstream UI Dependency |

---

## 3. Rust Cargo Dependencies Audit

All Rust crate dependencies utilized in the workspace are verified permissive (MIT or Apache-2.0) and compatible with binary distribution under GPL-3.0-or-later:

| Crate | Version | License | Crates.io URL |
|---|---|---|---|
| `tokio` | `1.43` | MIT | https://crates.io/crates/tokio |
| `bytes` | `1.10` | MIT | https://crates.io/crates/bytes |
| `serde` | `1.0` | MIT / Apache-2.0 | https://crates.io/crates/serde |
| `serde_json` | `1.0` | MIT / Apache-2.0 | https://crates.io/crates/serde_json |
| `thiserror` | `2.0` | MIT / Apache-2.0 | https://crates.io/crates/thiserror |
| `sha2` | `0.10` | MIT / Apache-2.0 | https://crates.io/crates/sha2 |
| `md-5` | `0.10` | MIT / Apache-2.0 | https://crates.io/crates/md-5 |
| `tauri-plugin-opener` | `2.7.2` | Apache-2.0 OR MIT | https://crates.io/crates/tauri-plugin-opener |
| `trash` | `5.2.9` | MIT | https://crates.io/crates/trash |

---

## 4. Offline Integrity & Content Security Policy (REQ-066)

### Remote Asset Prohibition
To guarantee that the embedded UI functions flawlessly on closed LANs and air-gapped relay environments:
- **No Remote Fonts**: Typography relies strictly on standard system font stacks (`-apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, sans-serif`). No Google Fonts (`fonts.googleapis.com`) or Typekit links are present.
- **No Remote CDNs**: All scripts, styles, icons, and fixture data are embedded directly into the binary executable via `peercast-http::embedded_assets`. No `cdn.jsdelivr.net`, `unpkg.com`, or `cdnjs.cloudflare.com` links are permitted.
- **Strict Content-Security-Policy (CSP)**:
  ```http
  Content-Security-Policy: default-src 'self'; script-src 'self' 'unsafe-inline'; style-src 'self' 'unsafe-inline'; img-src 'self' data:; connect-src 'self';
  ```
  This header actively blocks any accidental or injected outbound network connection from UI browser contexts.

---

## 5. Verification Gate & Automated Checks

The license audit and embedded UI integration are verified by:
1. `crates/peercast-http/tests/test_p39_embedded_ui.rs`:
   - Verifies all embedded routes (`/`, `/index.html`, `/channels.html`, `/settings.html`, `/gallery.html`, `/api/1/peercaststation.js`, `/css/tokens.css`, `/css/themes/nord.css`) serve with correct MIME types and ETags.
   - Verifies conditional HTTP 304 Not Modified when `If-None-Match` matches asset ETag.
   - Verifies security headers (`nosniff`, `SAMEORIGIN`, `Content-Security-Policy`).
   - Verifies that zero external HTTP/HTTPS CDN references exist in any embedded HTML or CSS payload.
   - Verifies complete end-to-end integration: embedded UI client JavaScript calls JSON-RPC 2.0 backend (`getStatus`, `getChannels`, `getSettings`) and succeeds without error.
