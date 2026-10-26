const fs = require("node:fs");
const path = require("node:path");
const sharp = require("C:/Users/onem4/.cache/codex-runtimes/codex-primary-runtime/dependencies/node/node_modules/sharp");

const root = path.resolve(__dirname, "..");
const sizes = [16, 20, 24, 32, 40, 48, 64, 128, 256];

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
  const frames = [];
  for (const size of sizes) {
    const inset = Math.max(1, Math.round(size * 0.055));
    const width = size - inset * 2;
    const height = Math.round(width * crop.height / crop.width);
    const mark = await sharp(logo).resize(width, height, { kernel: "lanczos3" })
      .sharpen({ sigma: 0.5, m1: 0.4, m2: 1.2 }).png().toBuffer();
    const svg = Buffer.from(`<svg xmlns="http://www.w3.org/2000/svg" width="${size}" height="${size}" viewBox="0 0 ${size} ${size}"><rect x="0.5" y="0.5" width="${size - 1}" height="${size - 1}" rx="${size * 0.22}" fill="#eee8ff" stroke="#c9bbf4" stroke-width="1"/></svg>`);
    const png = await sharp(svg).composite([{ input: mark, left: inset, top: Math.floor((size - height) / 2) }]).png().toBuffer();
    frames.push({ size, png });
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
