const fs = require("node:fs");
const path = require("node:path");
const sharp = require("C:/Users/onem4/.cache/codex-runtimes/codex-primary-runtime/dependencies/node/node_modules/sharp");

const iconsRoot = path.resolve(__dirname, "..");
const logoPath = path.resolve(__dirname, "../../frontend/assets/peer-loop.png");

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
    logoRect: { left: 14, top: 13, width: 30, height: 30 },
    svg: `<svg xmlns="http://www.w3.org/2000/svg" width="150" height="57" viewBox="0 0 150 57">
      <rect width="150" height="57" fill="#fbfaff"/>
      <rect x="0" y="0" width="4" height="57" fill="#493478"/>
      <path d="M5 54.5H150" stroke="#ded5f5"/>
      <text x="54" y="35" font-family="Segoe UI,Arial,sans-serif" font-size="14" font-weight="700" fill="#30204f">Peercast RT</text>
    </svg>`,
  });

  await drawBmp({
    width: 164,
    height: 314,
    filename: "installer-sidebar.bmp",
    logoRect: { left: 10, top: 10, width: 30, height: 30 },
    svg: `<svg xmlns="http://www.w3.org/2000/svg" width="164" height="314" viewBox="0 0 164 314">
      <rect width="164" height="314" fill="#f8f6fd"/>
      <rect width="50" height="314" fill="#352457"/>
      <rect x="0" y="0" width="50" height="3" fill="#8d7ae2"/>
      <circle cx="25" cy="74" r="3" fill="#e5defb"/>
      <circle cx="25" cy="92" r="3" fill="#7662b4"/>
      <circle cx="25" cy="110" r="3" fill="#7662b4"/>
      <rect x="50" y="0" width="114" height="62" fill="#fff"/>
      <path d="M50 61.5H164" stroke="#e7e1f2"/>
      <text x="61" y="29" font-family="Segoe UI,Arial,sans-serif" font-size="14" font-weight="700" fill="#30204f">Peercast RT</text>
      <text x="62" y="44" font-family="Segoe UI,Arial,sans-serif" font-size="7" font-weight="600" letter-spacing="1" fill="#8778a6">DESKTOP</text>
      <path d="M64 86h72M64 108h61M64 130h69" stroke="#ded8ec" stroke-width="5" stroke-linecap="round"/>
      <path d="M64 178h54M64 200h74M64 222h62" stroke="#e8e3f1" stroke-width="5" stroke-linecap="round"/>
      <circle cx="137" cy="278" r="34" fill="#eee9fb"/>
    </svg>`,
  });
}

main().catch((error) => {
  process.stderr.write(`${error.stack || error}\n`);
  process.exitCode = 1;
});
