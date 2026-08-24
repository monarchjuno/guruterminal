import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const e2eRoot = dirname(fileURLToPath(import.meta.url));
const appRoot = resolve(e2eRoot, "..");
const read = (path) => readFileSync(path, "utf8");

assert.match(read(resolve(e2eRoot, ".npmrc")), /^ignore-scripts=true$/m);

const packageManifest = JSON.parse(read(resolve(e2eRoot, "package.json")));
assert.equal(packageManifest.devDependencies.webdriverio, "9.30.1");
assert.equal(packageManifest.scripts.mcp, undefined);

const appLauncher = read(resolve(e2eRoot, "run-app.sh"));
assert.match(appLauncher, /\/usr\/bin\/env -i/);
assert.match(appLauncher, /GURUTERMINAL_E2E_APP_DATA_DIR/);
assert.doesNotMatch(appLauncher, /GURUTERMINAL_E2E_APPROVE_NATIVE/);
assert.doesNotMatch(appLauncher, /GURUTERMINAL_AGENT_E2E_KEYCHAIN/);

const devLauncher = read(resolve(e2eRoot, "run-dev-app.sh"));
assert.match(devLauncher, /\/usr\/bin\/env -i/);
assert.doesNotMatch(devLauncher, /GURUTERMINAL_AGENT_E2E_KEYCHAIN/);

const financeCredentialStore = read(
  resolve(appRoot, "src-tauri/src/finance_credentials.rs"),
);
assert.match(
  financeCredentialStore,
  /cfg\(any\(test, feature = "webdriver"\)\)\][\s\S]*?fn read_blob[\s\S]*?read_memory_blob\(entry_id\)/,
  "WebDriver builds must not open the native credential store",
);
assert.match(
  financeCredentialStore,
  /not\(any\(test, feature = "webdriver"\)\)[\s\S]*?fn read_blob[\s\S]*?read_native_blob\(NATIVE_SERVICE, entry_id\)/,
  "only normal app builds may use the product native credential service",
);
assert.doesNotMatch(financeCredentialStore, /GURUTERMINAL_AGENT_E2E_KEYCHAIN/);

const noScriptOrCookies = /executeScript|eval\(|getCookies|setCookies/;
assert.doesNotMatch(read(resolve(e2eRoot, "agent-driver.mjs")), noScriptOrCookies);
assert.doesNotMatch(read(resolve(e2eRoot, "detach-launch.mjs")), noScriptOrCookies);
assert.doesNotMatch(read(resolve(e2eRoot, "wait-session-lib.mjs")), noScriptOrCookies);
assert.doesNotMatch(read(resolve(appRoot, "scripts/tauri.mjs")), noScriptOrCookies);

assert.doesNotMatch(read(resolve(e2eRoot, "down.sh")), /pkill/);

const cargoManifest = read(resolve(appRoot, "src-tauri/Cargo.toml"));
assert.match(cargoManifest, /^webdriver = \["dep:tauri-plugin-wdio-webdriver"\]$/m);
assert.match(cargoManifest, /^e2e = \["webdriver"\]$/m);

assert.match(
  read(resolve(appRoot, "src-tauri/build.rs")),
  /WebDriver is forbidden in release builds/,
);
assert.match(
  read(resolve(appRoot, "src-tauri/src/lib.rs")),
  /compile_error!\("WebDriver is forbidden in release builds"\)/,
);
assert.match(
  read(resolve(appRoot, "src-tauri/src/lib.rs")),
  /com\.monarchjuno\.guruterminal\.e2e/,
);
assert.doesNotMatch(
  read(resolve(appRoot, "src-tauri/src/app.rs")),
  /GURUTERMINAL_E2E_APPROVE_NATIVE/,
);

const tauriConfig = JSON.parse(
  read(resolve(appRoot, "src-tauri/tauri.e2e.conf.json")),
);
assert.equal(tauriConfig.identifier, "com.monarchjuno.guruterminal.e2e");
assert.equal(tauriConfig.bundle.active, false);

console.log("Guru Terminal E2E security invariants passed.");
