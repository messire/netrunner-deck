#!/usr/bin/env node
// One version for the whole app: package.json is the source of truth
// (tauri.conf.json reads it via "version": "../package.json"); this script
// writes the same version into src-tauri/Cargo.toml, or with --check only
// verifies that they agree (CI runs the check).
import { readFileSync, writeFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const pkgPath = join(root, "package.json");
const cargoPath = join(root, "src-tauri", "Cargo.toml");
const lockPath = join(root, "src-tauri", "Cargo.lock");

const version = JSON.parse(readFileSync(pkgPath, "utf8")).version;
if (!/^\d+\.\d+\.\d+(-[0-9A-Za-z.-]+)?$/.test(version)) {
  console.error(`package.json version is not semver: ${version}`);
  process.exit(2);
}

const cargo = readFileSync(cargoPath, "utf8");
const m = cargo.match(/^\[package\][\s\S]*?^version = "([^"]+)"/m);
if (!m) { console.error("Cargo.toml: no [package] version"); process.exit(2); }
const current = m[1];

if (process.argv.includes("--check")) {
  if (current !== version) {
    console.error(`version mismatch: package.json ${version}, Cargo.toml ${current} — run \`npm run version:sync\``);
    process.exit(1);
  }
  console.log(`version ${version} in sync`);
  process.exit(0);
}

if (current !== version) {
  writeFileSync(cargoPath, cargo.replace(m[0], m[0].replace(`"${current}"`, `"${version}"`)));
  // Cargo.lock records the package's own version too; keep it current so CI's
  // --locked build does not fail on a lockfile change.
  const lock = readFileSync(lockPath, "utf8");
  const entry = new RegExp(`(\\[\\[package\\]\\]\\nname = "netrunner-deck"\\nversion = ")${current.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")}(")`);
  writeFileSync(lockPath, lock.replace(entry, `$1${version}$2`));
  console.log(`Cargo.toml: ${current} → ${version}`);
} else {
  console.log(`version ${version} already in sync`);
}
