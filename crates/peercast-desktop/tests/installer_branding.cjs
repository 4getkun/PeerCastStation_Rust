const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');

const source = fs.readFileSync(path.join(__dirname, '../icons/scripts/generate_nsis_branding.cjs'), 'utf8');
const sizes = [...source.matchAll(/logoRect:\s*\{[^}]*width:\s*(\d+),\s*height:\s*(\d+)/g)]
  .map((match) => [Number(match[1]), Number(match[2])]);
assert.equal(sizes.length, 2, 'Both NSIS images must declare a logo frame');
for (const [width, height] of sizes) {
  assert.equal(width, height, 'Installer branding must use a square logo frame to avoid a stretched mark');
}
console.log('Installer branding regression: header and sidebar use square logo frames');
