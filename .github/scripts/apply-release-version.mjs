import { readFileSync, writeFileSync } from "node:fs";

const version = process.argv[2];

if (!/^\d+\.\d+\.\d+$/.test(version ?? "")) {
  throw new Error(`Expected a stable semantic version, received: ${version}`);
}

function updateJson(path) {
  const contents = JSON.parse(readFileSync(path, "utf8"));
  contents.version = version;
  writeFileSync(path, `${JSON.stringify(contents, null, 2)}\n`);
}

updateJson("package.json");
updateJson("src-tauri/tauri.conf.json");

const cargoTomlPath = "src-tauri/Cargo.toml";
const cargoToml = readFileSync(cargoTomlPath, "utf8");
const updatedCargoToml = cargoToml.replace(
  /^(\[package\][\s\S]*?^version = ")[^"]+("(?=\r?$))/m,
  `$1${version}$2`,
);

if (updatedCargoToml === cargoToml) {
  throw new Error("Could not update the Pulse package version in Cargo.toml.");
}
writeFileSync(cargoTomlPath, updatedCargoToml);

const cargoLockPath = "src-tauri/Cargo.lock";
const cargoLock = readFileSync(cargoLockPath, "utf8");
const updatedCargoLock = cargoLock.replace(
  /(\[\[package\]\]\r?\nname = "pulse"\r?\nversion = ")[^"]+("(?=\r?\n))/,
  `$1${version}$2`,
);

if (updatedCargoLock === cargoLock) {
  throw new Error("Could not update the Pulse package version in Cargo.lock.");
}
writeFileSync(cargoLockPath, updatedCargoLock);

console.log(`Injected Pulse release version ${version}.`);
