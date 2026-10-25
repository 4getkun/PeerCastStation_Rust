const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");

const source = fs.readFileSync(path.join(__dirname, "../frontend/app.js"), "utf8");

assert.match(source, /UPDATE_INITIAL_CHECK_DELAY_MS\s*=\s*30_000/, "Startup update checks should wait 30 seconds");
assert.match(source, /UPDATE_CHECK_INTERVAL_MS\s*=\s*24\s*\*\s*60\s*\*\s*60\s*\*\s*1000/, "Successful checks should be limited to once per day");
assert.match(source, /invoke\("check_update"\)/, "The main view should ask the trusted Rust updater to check");
assert.match(source, /localStorage\.setItem\(UPDATE_LAST_CHECK_KEY/, "Successful checks should persist their timestamp");
assert.match(source, /status\?\.phase\s*===\s*"available"/, "Only an available update should show the startup notice");
assert.match(source, /scheduleApplicationUpdateChecks\(\)/, "Automatic checks must be scheduled during startup");
assert(!source.includes("download_update\");\n      window.setInterval"), "Startup scheduling must not silently download an update");

console.log("Updater startup regression: delayed daily checks and user-controlled download passed");
