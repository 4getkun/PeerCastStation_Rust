const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');

const source = fs.readFileSync(path.join(__dirname, '../icons/scripts/generate_nsis_branding.cjs'), 'utf8');
const template = fs.readFileSync(path.join(__dirname, '../nsis/installer.nsi'), 'utf8');
const buildScript = fs.readFileSync(path.join(__dirname, '../../../tools/build-installer.ps1'), 'utf8');
const brandRail = fs.readFileSync(path.join(__dirname, '../icons/installer-brand-rail.bmp'));
const sizes = [...source.matchAll(/logoRect:\s*\{[^}]*width:\s*(\d+),\s*height:\s*(\d+)/g)]
  .map((match) => [Number(match[1]), Number(match[2])]);
assert.equal(sizes.length, 3, 'All NSIS brand images must declare a logo frame');
for (const [width, height] of sizes) {
  assert(width > height, 'The original wide PeerCast loop must keep a landscape logo frame');
}
assert.match(source, /fit:\s*"contain"/, 'Logo resizing must preserve the original aspect ratio');
assert.match(source, /\.trim\(/, 'Transparent source padding must be removed before the logo is resized');
assert(!source.includes('M34 270'), 'Rejected decorative network lines must not return');
assert.match(buildScript, /template\s*=.*installer\.nsi/, 'Release builds must use the custom NSIS template');
assert.match(buildScript, /installerHooks\s*=.*installer-assets\.nsh/, 'The custom template must receive its exact-size brand rail through the asset include');
assert.match(template, /Page custom PageReady PageReadyLeave/, 'The branded welcome must be a real custom NSIS page');
assert.match(template, /SetCtlColors \$ReadyDialog "241C35" "F8F6FD"/, 'The welcome must share the continuous pale surface with the brand artwork');
assert.match(template, /PEERCAST_BRANDRAIL/, 'The original PeerCast loop must be rendered from the exact-size brand rail');
assert(!template.includes('Install Misaca'), 'Borrowed product copy must not leak into the installer');
assert.equal(brandRail.readInt32LE(18), 239, 'Brand rail width must match the measured installer column');
assert.equal(brandRail.readInt32LE(22), 588, 'Brand rail height must match the measured installer column');
console.log('Installer branding regression: custom template, original logo and approved pale-lavender layout passed');

assert(!/SetCtlColors [^\n]+ "FFFFFF"/.test(template), "Native labels must not create isolated white rectangles");
assert(!source.includes('x="34" y="308" width="185"'), "The brand rail must not contain the rejected white card");
