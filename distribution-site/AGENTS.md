# Prototype Instructions

## Project-specific decisions

- Build one Japanese single-page distribution site for PeerCast RT, focused on video streaming and P2P relaying.
- The sticky top navigation uses in-page anchors for 「はじめに」「視聴する」「配信する」「ライセンス」「ダウンロード」, in that order. Place the direct GitHub link to `https://github.com/4getkun/PeerCastStation_Rust` rightmost. Keep license details before the final download section. Do not add an update-history section.
- Do not add a support navigation item. Keep a footer.
- Use the selected product-hero mockup as the visual direction: white and pale lavender, blue-violet accents, flat Windows / Visual Studio 2015-like controls, plain non-corporate Japanese copy.
- The hero app visual is an illustrative video-channel UI mockup, clearly labeled as not an actual screenshot. Do not generate a software screenshot.
- Replace the illustrative mockup with a fresh real PeerCast RT screenshot later.
- The license section should state the repository's GPL-3.0-or-later project license and accurately attribute third-party icon assets; link to the license documents packaged under the ZIP's `licenses/` directory.
- Show the verified release metadata and ZIP SHA-256 alongside the download links. Keep the archive checksum sidecar and license documents available from the public download host.

Run the local server yourself and open the preview in the browser available to this environment. Do not give the user server-start instructions when you can run it.

Before making substantial visual changes, use the Product Design plugin's `get-context` skill when the visual source is unclear or no longer matches the current goal. When the user gives durable prototype-specific design feedback, preferences, or decisions, record them in `AGENTS.md`.

When implementing from a selected generated mock, treat that image as the source of truth for layout, component anatomy, density, spacing, color, typography, visible content, and hierarchy.

Build app UI in `src/`. Keep `.openai/hosting.json`, `worker/index.js`, `scripts/prepare-sites-build.mjs`, and `tests/sites-worker.test.mjs` intact so the same local prototype can be handed to Sites. Before a Sites handoff, run `npm run build` and `npm run test:sites`; the build must leave `dist/client/index.html`, `dist/server/index.js`, and `dist/.openai/hosting.json`.
