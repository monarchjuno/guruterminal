#!/usr/bin/env node

import {
  createHash,
  createPublicKey,
  verify as verifyEd25519,
} from "node:crypto";
import { once } from "node:events";
import { createReadStream } from "node:fs";
import { lstat, readFile } from "node:fs/promises";
import path from "node:path";
import process from "node:process";
import { TextDecoder } from "node:util";

const RELEASE_VERSION =
  /^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)(-rc\.[1-9][0-9]*)?$/;
const ED25519_SPKI_PREFIX = Buffer.from("302a300506032b6570032100", "hex");
const textDecoder = new TextDecoder("utf-8", { fatal: true });

function fail(message) {
  throw new Error(message);
}

function decodeBase64(value, label) {
  if (
    typeof value !== "string" ||
    value.length === 0 ||
    value.length % 4 !== 0 ||
    !/^(?:[A-Za-z0-9+/]{4})*(?:[A-Za-z0-9+/]{2}==|[A-Za-z0-9+/]{3}=)?$/.test(
      value,
    )
  ) {
    fail(`${label} is not canonical base64`);
  }
  const decoded = Buffer.from(value, "base64");
  if (decoded.toString("base64") !== value) {
    fail(`${label} is not canonical base64`);
  }
  return decoded;
}

function decodeUtf8Base64(value, label) {
  try {
    return textDecoder.decode(decodeBase64(value, label));
  } catch (error) {
    fail(`${label} is not base64-encoded UTF-8: ${error.message}`);
  }
}

function exactLines(value, count, label) {
  const lines = value.replace(/\r\n/g, "\n").split("\n");
  if (lines.at(-1) === "") {
    lines.pop();
  }
  if (lines.length !== count || lines.some((line) => line.length === 0)) {
    fail(`${label} must contain exactly ${count} nonempty lines`);
  }
  return lines;
}

function parsePublicKey(encoded) {
  const [comment, keyLine] = exactLines(
    decodeUtf8Base64(encoded, "updater public key"),
    2,
    "updater public key",
  );
  if (!comment.startsWith("untrusted comment: ")) {
    fail("updater public key comment is malformed");
  }
  const keyBox = decodeBase64(keyLine, "minisign public key");
  if (
    keyBox.length !== 42 ||
    keyBox[0] !== 0x45 ||
    ![0x44, 0x64].includes(keyBox[1])
  ) {
    fail("updater public key uses an unsupported minisign encoding");
  }
  const keyId = keyBox.subarray(2, 10);
  const key = createPublicKey({
    key: Buffer.concat([ED25519_SPKI_PREFIX, keyBox.subarray(10)]),
    format: "der",
    type: "spki",
  });
  return { key, keyId };
}

function parseSignature(encoded) {
  const [untrustedComment, signatureLine, trustedComment, globalLine] =
    exactLines(
      decodeUtf8Base64(encoded, "updater signature"),
      4,
      "updater signature",
    );
  if (!untrustedComment.startsWith("untrusted comment: ")) {
    fail("updater signature untrusted comment is malformed");
  }
  if (!trustedComment.startsWith("trusted comment: ")) {
    fail("updater signature trusted comment is malformed");
  }
  const signatureBox = decodeBase64(signatureLine, "minisign signature");
  const globalSignature = decodeBase64(
    globalLine,
    "minisign global signature",
  );
  if (
    signatureBox.length !== 74 ||
    signatureBox[0] !== 0x45 ||
    signatureBox[1] !== 0x44 ||
    globalSignature.length !== 64
  ) {
    fail("updater signature must use the modern prehashed minisign encoding");
  }
  return {
    keyId: signatureBox.subarray(2, 10),
    signature: signatureBox.subarray(10),
    trustedComment: Buffer.from(trustedComment.slice(17), "utf8"),
    globalSignature,
  };
}

async function requireRegularFile(file) {
  const metadata = await lstat(file);
  if (!metadata.isFile() || metadata.isSymbolicLink() || metadata.size === 0) {
    fail(`updater artifact must be a nonempty regular file: ${file}`);
  }
}

async function blake2b512(file) {
  const digest = createHash("blake2b512");
  const input = createReadStream(file);
  input.on("data", (chunk) => digest.update(chunk));
  await once(input, "end");
  return digest.digest();
}

async function verifyArtifact(publicKey, artifact) {
  const signaturePath = `${artifact}.sig`;
  await requireRegularFile(artifact);
  await requireRegularFile(signaturePath);
  const encodedSignature = (await readFile(signaturePath, "utf8")).trim();
  const signature = parseSignature(encodedSignature);
  if (!publicKey.keyId.equals(signature.keyId)) {
    fail(`updater signature key ID does not match the configured public key: ${artifact}`);
  }
  const digest = await blake2b512(artifact);
  if (
    !verifyEd25519(null, digest, publicKey.key, signature.signature) ||
    !verifyEd25519(
      null,
      Buffer.concat([signature.signature, signature.trustedComment]),
      publicKey.key,
      signature.globalSignature,
    )
  ) {
    fail(`updater signature verification failed: ${artifact}`);
  }
}

function argumentsFrom(argv) {
  if (
    argv.length !== 4 ||
    argv[0] !== "--version" ||
    argv[2] !== "--assets"
  ) {
    fail("usage: verify-updater-signatures.mjs --version X.Y.Z[-rc.N] --assets DIRECTORY");
  }
  const version = argv[1];
  if (!RELEASE_VERSION.test(version)) {
    fail("release version must be canonical X.Y.Z or X.Y.Z-rc.N");
  }
  return { version, assets: path.resolve(argv[3]) };
}

async function main() {
  const { version, assets } = argumentsFrom(process.argv.slice(2));
  const encodedPublicKey = process.env.GURUTERMINAL_UPDATER_PUBLIC_KEY?.trim();
  if (!encodedPublicKey) {
    fail("GURUTERMINAL_UPDATER_PUBLIC_KEY is required");
  }
  const publicKey = parsePublicKey(encodedPublicKey);
  const artifacts = [
    `Guru Terminal-${version}-darwin-aarch64.app.tar.gz`,
    `Guru Terminal-${version}-x86_64-pc-windows-msvc-setup.exe`,
  ];
  for (const name of artifacts) {
    await verifyArtifact(publicKey, path.join(assets, name));
  }
  console.log("Verified both updater packages against the configured public key.");
}

main().catch((error) => {
  console.error(error.message);
  process.exitCode = 1;
});
