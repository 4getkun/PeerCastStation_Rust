const fs = require("node:fs");
const path = require("node:path");
const sharp = require("C:/Users/onem4/.cache/codex-runtimes/codex-primary-runtime/dependencies/node/node_modules/sharp");

const iconsRoot = path.resolve(__dirname, "..");
const logoPath = path.resolve(__dirname, "../../frontend/assets/peer-loop.png");
const tauriConfig = JSON.parse(fs.readFileSync(path.resolve(__dirname, "../../tauri.conf.json"), "utf8"));

function encodeBmp24(width, height, rgb) {
  const rowSize = Math.ceil((width * 3) / 4) * 4;
  const imageSize = rowSize * height;
  const output = Buffer.alloc(54 + imageSize);
  output.write("BM", 0, "ascii");
  output.writeUInt32LE(output.length, 2);
  output.writeUInt32LE(54, 10);
  output.writeUInt32LE(40, 14);
  output.writeInt32LE(width, 18);
  output.writeInt32LE(height, 22);
  output.writeUInt16LE(1, 26);
  output.writeUInt16LE(24, 28);
  output.writeUInt32LE(imageSize, 34);

  for (let y = 0; y < height; y += 1) {
    const sourceY = height - 1 - y;
    for (let x = 0; x < width; x += 1) {
      const source = (sourceY * width + x) * 3;
      const target = 54 + y * rowSize + x * 3;
      output[target] = rgb[source + 2];
      output[target + 1] = rgb[source + 1];
      output[target + 2] = rgb[source];
    }
  }
  return output;
}

async function drawBmp({ width, height, svg, logoRect, filename }) {
  const logo = await sharp(logoPath)
    .trim({ background: { r: 0, g: 0, b: 0, alpha: 0 } })
    .resize(logoRect.width, logoRect.height, {
      fit: "contain",
      background: { r: 0, g: 0, b: 0, alpha: 0 },
    })
    .png()
    .toBuffer();
  const png = await sharp({
    create: { width, height, channels: 4, background: { r: 251, g: 250, b: 255, alpha: 1 } },
  })
    .composite([
      { input: Buffer.from(svg), left: 0, top: 0 },
      { input: logo, left: logoRect.left, top: logoRect.top },
    ])
    .png()
    .toBuffer();
  const { data, info } = await sharp(png).removeAlpha().raw().toBuffer({ resolveWithObject: true });
  if (info.channels !== 3 || info.width !== width || info.height !== height) {
    throw new Error(`Unexpected raster dimensions or channels for ${filename}`);
  }
  const bmp = encodeBmp24(width, height, data);
  fs.writeFileSync(path.join(iconsRoot, filename), bmp);
  if (bmp.readUInt16LE(28) !== 24 || bmp.readInt32LE(18) !== width || bmp.readInt32LE(22) !== height) {
    throw new Error(`Invalid 24-bit BMP header for ${filename}`);
  }
}

async function main() {
  await drawBmp({
    width: 150,
    height: 57,
    filename: "installer-header.bmp",
    logoRect: { left: 12, top: 17, width: 34, height: 18 },
    svg: `<svg xmlns="http://www.w3.org/2000/svg" width="150" height="57" viewBox="0 0 150 57">
      <rect width="150" height="57" fill="#ffffff"/>
      <path d="M0 55.5H150" stroke="#ddd7ea"/>
      <text x="52" y="27" font-family="Segoe UI,Yu Gothic UI,Meiryo UI,sans-serif" font-size="13" font-weight="700" fill="#241c35">PeerCast RT</text>
      <text x="52" y="41" font-family="Segoe UI,Yu Gothic UI,Meiryo UI,sans-serif" font-size="8" fill="#7d7688">セットアップ</text>
    </svg>`,
  });

  await drawBmp({
    width: 164,
    height: 314,
    filename: "installer-sidebar.bmp",
    logoRect: { left: 24, top: 22, width: 116, height: 58 },
    svg: `<svg xmlns="http://www.w3.org/2000/svg" width="164" height="314" viewBox="0 0 164 314">
      <defs><linearGradient id="panel" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="#f3effc"/><stop offset="1" stop-color="#ebe5f8"/></linearGradient></defs>
      <rect width="164" height="314" fill="url(#panel)"/>
      <rect width="4" height="314" fill="#735bd3"/>
      <text x="24" y="104" font-family="Segoe UI,Yu Gothic UI,Meiryo UI,sans-serif" font-size="15" font-weight="700" fill="#241c35">PeerCast RT</text>
      <text x="24" y="126" font-family="Segoe UI,Yu Gothic UI,Meiryo UI,sans-serif" font-size="9" fill="#665f73">配信と視聴を、</text>
      <text x="24" y="141" font-family="Segoe UI,Yu Gothic UI,Meiryo UI,sans-serif" font-size="9" fill="#665f73">ひとつのアプリで。</text>
      <rect x="20" y="169" width="124" height="42" rx="6" fill="#ffffff" fill-opacity="0.78"/>
      <rect x="20" y="169" width="3" height="42" rx="1.5" fill="#7b63dc"/>
      <circle cx="38" cy="190" r="4" fill="#7b63dc"/>
      <text x="51" y="194" font-family="Segoe UI,Yu Gothic UI,Meiryo UI,sans-serif" font-size="10" font-weight="600" fill="#3d344e">ようこそ</text>
      <text x="24" y="276" font-family="Segoe UI,Yu Gothic UI,Meiryo UI,sans-serif" font-size="9" fill="#746d80">Windows x64</text>
      <text x="24" y="292" font-family="Segoe UI,Yu Gothic UI,Meiryo UI,sans-serif" font-size="8" fill="#958da1">Version ${tauriConfig.version}</text>
    </svg>`,
  });

  // The custom wide installer uses a 239x588 physical-pixel brand rail on
  // the target 150% DPI layout. Rendering at the final size avoids NSIS's
  // low-quality non-uniform stretch of the stock 164x314 sidebar bitmap.
  await drawBmp({
    width: 239,
    height: 588,
    filename: "installer-brand-rail.bmp",
    logoRect: { left: 39, top: 46, width: 162, height: 81 },
    svg: `<svg xmlns="http://www.w3.org/2000/svg" width="239" height="588" viewBox="0 0 239 588">
      <defs><linearGradient id="rail" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="#f4f0fd"/><stop offset="1" stop-color="#ebe5f8"/></linearGradient></defs>
      <rect width="239" height="588" fill="url(#rail)"/>
      <rect width="6" height="588" fill="#735bd3"/>
      <text x="39" y="169" font-family="Segoe UI,Yu Gothic UI,Meiryo UI,sans-serif" font-size="24" font-weight="700" fill="#241c35">PeerCast RT</text>
      <text x="39" y="210" font-family="Segoe UI,Yu Gothic UI,Meiryo UI,sans-serif" font-size="14" fill="#665f73">配信と視聴を、</text>
      <text x="39" y="234" font-family="Segoe UI,Yu Gothic UI,Meiryo UI,sans-serif" font-size="14" fill="#665f73">ひとつのアプリで。</text>
      <rect x="34" y="308" width="185" height="72" rx="8" fill="#ffffff" fill-opacity="0.82"/>
      <rect x="34" y="308" width="5" height="72" rx="2.5" fill="#7b63dc"/>
      <circle cx="63" cy="344" r="7" fill="#7b63dc"/>
      <text x="84" y="350" font-family="Segoe UI,Yu Gothic UI,Meiryo UI,sans-serif" font-size="15" font-weight="600" fill="#3d344e">ようこそ</text>
      <text x="39" y="535" font-family="Segoe UI,Yu Gothic UI,Meiryo UI,sans-serif" font-size="13" fill="#746d80">Windows x64</text>
      <text x="39" y="563" font-family="Segoe UI,Yu Gothic UI,Meiryo UI,sans-serif" font-size="12" fill="#958da1">Version ${tauriConfig.version}</text>
    </svg>`,
  });
}

main().catch((error) => {
  process.stderr.write(`${error.stack || error}\n`);
  process.exitCode = 1;
});
