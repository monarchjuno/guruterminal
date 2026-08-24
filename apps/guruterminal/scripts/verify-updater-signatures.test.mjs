import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import {
  createHash,
  generateKeyPairSync,
  randomBytes,
  sign,
} from "node:crypto";
import { mkdtemp, rm, writeFile } from "node:fs/promises";
import { dirname, join } from "node:path";
import process from "node:process";
import test from "node:test";
import { fileURLToPath } from "node:url";

const scriptsRoot = dirname(fileURLToPath(import.meta.url));
const verifier = join(scriptsRoot, "verify-updater-signatures.mjs");
const version = "0.0.1";
const artifactNames = [
  `Guru Terminal-${version}-darwin-aarch64.app.tar.gz`,
  `Guru Terminal-${version}-x86_64-pc-windows-msvc-setup.exe`,
];
const ed25519SpkiPrefix = Buffer.from("302a300506032b6570032100", "hex");

function blake2b512(content) {
  return createHash("blake2b512").update(content).digest();
}

function publicKeyEnvironmentValue(publicKey, keyId) {
  const encodedKey = publicKey.export({ format: "der", type: "spki" });
  assert.ok(encodedKey.subarray(0, ed25519SpkiPrefix.length).equals(ed25519SpkiPrefix));
  const keyBox = Buffer.concat([
    Buffer.from([0x45, 0x64]),
    keyId,
    encodedKey.subarray(ed25519SpkiPrefix.length),
  ]);
  const minisign = `untrusted comment: updater fixture\n${keyBox.toString("base64")}\n`;
  return Buffer.from(minisign, "utf8").toString("base64");
}

function signatureFixture(privateKey, keyId, content) {
  const signature = sign(null, blake2b512(content), privateKey);
  const trustedComment = "fixture provenance";
  const globalSignature = sign(
    null,
    Buffer.concat([signature, Buffer.from(trustedComment, "utf8")]),
    privateKey,
  );
  const signatureBox = Buffer.concat([
    Buffer.from([0x45, 0x44]),
    keyId,
    signature,
  ]);
  const minisign = [
    "untrusted comment: updater fixture",
    signatureBox.toString("base64"),
    `trusted comment: ${trustedComment}`,
    globalSignature.toString("base64"),
  ].join("\n");
  return Buffer.from(`${minisign}\n`, "utf8").toString("base64");
}

function runVerifier(assets, updaterPublicKey) {
  return new Promise((resolve, reject) => {
    const child = spawn(
      process.execPath,
      [verifier, "--version", version, "--assets", assets],
      {
        env: { ...process.env, GURUTERMINAL_UPDATER_PUBLIC_KEY: updaterPublicKey },
        stdio: ["ignore", "pipe", "pipe"],
      },
    );
    let stdout = "";
    let stderr = "";
    child.stdout.setEncoding("utf8");
    child.stderr.setEncoding("utf8");
    child.stdout.on("data", (chunk) => {
      stdout += chunk;
    });
    child.stderr.on("data", (chunk) => {
      stderr += chunk;
    });
    child.once("error", reject);
    child.once("close", (code, signal) => {
      resolve({ code, signal, stdout, stderr });
    });
  });
}

async function withSignedAssets(callback) {
  const assets = await mkdtemp(join(process.env.TMPDIR ?? "/tmp", "guruterminal-updater-"));
  try {
    const { privateKey, publicKey } = generateKeyPairSync("ed25519");
    const keyId = randomBytes(8);
    const updaterPublicKey = publicKeyEnvironmentValue(publicKey, keyId);
    for (const [index, name] of artifactNames.entries()) {
      const content = Buffer.from(`updater fixture ${index}\n`, "utf8");
      await writeFile(join(assets, name), content);
      await writeFile(
        join(assets, `${name}.sig`),
        signatureFixture(privateKey, keyId, content),
        "utf8",
      );
    }
    await callback({ assets, keyId, privateKey, publicKey, updaterPublicKey });
  } finally {
    await rm(assets, { recursive: true, force: true });
  }
}

test("accepts two valid signed updater packages", async () => {
  await withSignedAssets(async ({ assets, updaterPublicKey }) => {
    const result = await runVerifier(assets, updaterPublicKey);
    assert.equal(result.code, 0, result.stderr);
    assert.equal(result.signal, null);
    assert.equal(result.stderr, "");
    assert.match(result.stdout, /Verified both updater packages/);
  });
});

test("rejects a package changed after it was signed", async () => {
  await withSignedAssets(async ({ assets, updaterPublicKey }) => {
    await writeFile(join(assets, artifactNames[0]), "tampered\n", "utf8");
    const result = await runVerifier(assets, updaterPublicKey);
    assert.equal(result.code, 1);
    assert.match(result.stderr, /updater signature verification failed/);
  });
});

test("rejects a signature from another updater key", async () => {
  await withSignedAssets(async ({ assets }) => {
    const { publicKey } = generateKeyPairSync("ed25519");
    const result = await runVerifier(
      assets,
      publicKeyEnvironmentValue(publicKey, randomBytes(8)),
    );
    assert.equal(result.code, 1);
    assert.match(result.stderr, /key ID does not match/);
  });
});

test("rejects a malformed updater signature before verification", async () => {
  await withSignedAssets(async ({ assets, updaterPublicKey }) => {
    await writeFile(join(assets, `${artifactNames[0]}.sig`), "not base64\n", "utf8");
    const result = await runVerifier(assets, updaterPublicKey);
    assert.equal(result.code, 1);
    assert.match(result.stderr, /updater signature is not canonical base64/);
  });
});
