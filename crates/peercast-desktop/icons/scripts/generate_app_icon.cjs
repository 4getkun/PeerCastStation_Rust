const fs = require("node:fs");
const path = require("node:path");
const sharp = require("C:/Users/onem4/.cache/codex-runtimes/codex-primary-runtime/dependencies/node/node_modules/sharp");

const root = path.resolve(__dirname, "..");
const sizes = [16, 20, 24, 32, 40, 48, 64, 128, 256];
const previewDirectory = process.env.PEERCAST_ICON_PREVIEW_DIR;

async function main() {
  const { data, info } = await sharp(path.join(root, "../frontend/assets/peer-loop.png"))
    .ensureAlpha().raw().toBuffer({ resolveWithObject: true });
  let left = info.width, top = info.height, right = 0, bottom = 0;
  for (let y = 0; y < info.height; y++) {
    for (let x = 0; x < info.width; x++) {
      const alpha = (y * info.width + x) * 4 + 3;
      // The original artwork has faint isolated pixels outside the actual mark.
      if (data[alpha] < 32) { data[alpha] = 0; continue; }
      left = Math.min(left, x); top = Math.min(top, y);
      right = Math.max(right, x + 1); bottom = Math.max(bottom, y + 1);
    }
  }
  const crop = { left, top, width: right - left, height: bottom - top };
  const logo = await sharp(data, { raw: info }).extract(crop).png().toBuffer();
  // At notification-area sizes the 3D shading loses its contour. Keep the
  // original silhouette, using a flat light mark on a contrasting violet tile.
  const smallLogo = await sharp(logo).linear([0, 0, 0, 1], [248, 246, 255, 0]).png().toBuffer();
  const frames = [];
  for (const size of sizes) {
    const compact = size <= 40;
    const inset = compact ? Math.max(1, Math.round(size * 0.085)) : Math.max(1, Math.round(size * 0.055));
    const width = size - inset * 2;
    const height = Math.round(width * crop.height / crop.width * (compact ? 1.12 : 1));
    const mark = await sharp(compact ? smallLogo : logo).resize(width, height, { kernel: "lanczos3" })
      .sharpen({ sigma: 0.5, m1: 0.4, m2: 1.2 }).png().toBuffer();
    const fill = compact ? "#6045d5" : "#eee8ff";
    const stroke = compact ? "#5136c3" : "#c9bbf4";
    const svg = Buffer.from(`<svg xmlns="http://www.w3.org/2000/svg" width="${size}" height="${size}" viewBox="0 0 ${size} ${size}"><rect x="0.5" y="0.5" width="${size - 1}" height="${size - 1}" rx="${size * 0.22}" fill="${fill}" stroke="${stroke}" stroke-width="1"/></svg>`);
    const png = await sharp(svg).composite([{ input: mark, left: inset, top: Math.floor((size - height) / 2) }]).png().toBuffer();
    frames.push({ size, png });
    if (previewDirectory) {
      fs.mkdirSync(previewDirectory, { recursive: true });
      fs.writeFileSync(path.join(previewDirectory, `${size}.png`), png);
    }
    if (size === 32) {
      fs.writeFileSync(path.join(root, "tray-icon.rgba"), await sharp(png).ensureAlpha().raw().toBuffer());
    }
  }
  const header = Buffer.alloc(6 + frames.length * 16);
  header.writeUInt16LE(1, 2); header.writeUInt16LE(frames.length, 4);
  let offset = header.length;
  frames.forEach(({ size, png }, index) => {
    const entry = 6 + index * 16;
    header[entry] = size === 256 ? 0 : size;
    header[entry + 1] = size === 256 ? 0 : size;
    header.writeUInt16LE(1, entry + 4); header.writeUInt16LE(32, entry + 6);
    header.writeUInt32LE(png.length, entry + 8); header.writeUInt32LE(offset, entry + 12);
    offset += png.length;
  });
  fs.writeFileSync(path.join(root, "icon.ico"), Buffer.concat([header, ...frames.map(({ png }) => png)]));
  if (process.argv[2]) {
    fs.writeFileSync(path.resolve(process.argv[2]), frames.find(({ size }) => size === 256).png);
  }
  console.log(`Generated ${frames.length} ICO sizes and a 32px tray icon from the original ${crop.width}x${crop.height} mark.`);
}

main().catch((error) => { console.error(error); process.exitCode = 1; });
