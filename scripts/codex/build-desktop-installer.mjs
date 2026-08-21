#!/usr/bin/env node

import { copyFileSync, existsSync, readFileSync, writeFileSync } from "node:fs";
import { homedir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";

const requestedBuild = process.argv[2];
const repoRoot = resolve(dirname(fileURLToPath(import.meta.url)), "../..");
const tauriConfig = JSON.parse(
  readFileSync(join(repoRoot, "src-tauri", "tauri.conf.json"), "utf8"),
);
const artifactPrefix = tauriConfig.productName.replaceAll(" ", "_");
const updaterOverride = JSON.stringify({
  bundle: { createUpdaterArtifacts: false },
});
const tauriCli = join(
  repoRoot,
  "node_modules",
  "@tauri-apps",
  "cli",
  "tauri.js",
);
const generatedSchema = join(
  repoRoot,
  "src-tauri",
  "gen",
  "schemas",
  "windows-schema.json",
);
const generatedSchemaBeforeBuild = existsSync(generatedSchema)
  ? readFileSync(generatedSchema)
  : null;

process.on("exit", () => {
  if (
    generatedSchemaBeforeBuild &&
    existsSync(generatedSchema) &&
    !readFileSync(generatedSchema).equals(generatedSchemaBeforeBuild)
  ) {
    writeFileSync(generatedSchema, generatedSchemaBeforeBuild);
  }
});

function fail(message) {
  console.error(message);
  process.exit(1);
}

function run(command, args) {
  const result = spawnSync(command, args, {
    cwd: repoRoot,
    stdio: "inherit",
  });

  if (result.error) {
    fail(`Could not run ${command}: ${result.error.message}`);
  }

  if (result.status !== 0) {
    process.exit(result.status ?? 1);
  }
}

function windowsDesktop() {
  const result = spawnSync(
    "powershell.exe",
    [
      "-NoLogo",
      "-NoProfile",
      "-NonInteractive",
      "-Command",
      "[Environment]::GetFolderPath('Desktop')",
    ],
    { encoding: "utf8" },
  );

  const detectedDesktop = result.stdout?.trim();
  return detectedDesktop || join(homedir(), "Desktop");
}

function copyToDesktop(source, desktop) {
  if (!existsSync(source)) {
    fail(`Build completed without producing ${source}`);
  }

  if (!existsSync(desktop)) {
    fail(`Desktop directory is unavailable: ${desktop}`);
  }

  const destination = join(desktop, source.split(/[\\/]/).at(-1));
  copyFileSync(source, destination);
  console.log(`Saved installer to ${destination}`);
}

if (!existsSync(tauriCli)) {
  fail("Tauri CLI is missing. Run pnpm install --frozen-lockfile first.");
}

if (requestedBuild === "macos") {
  if (process.platform !== "darwin") {
    fail("macOS builds must run on macOS.");
  }

  const architecture = process.arch === "arm64" ? "aarch64" : "x64";
  const installerName = `${artifactPrefix}_${tauriConfig.version}_${architecture}.dmg`;
  const installer = join(
    repoRoot,
    "src-tauri",
    "target",
    "release",
    "bundle",
    "dmg",
    installerName,
  );

  run(process.execPath, [
    tauriCli,
    "build",
    "--bundles",
    "dmg",
    "--config",
    updaterOverride,
  ]);
  copyToDesktop(installer, join(homedir(), "Desktop"));
} else if (requestedBuild === "windows") {
  if (process.platform !== "win32") {
    fail("Windows builds must run on Windows.");
  }

  const target = "x86_64-pc-windows-msvc";
  const installerName = `${artifactPrefix}_${tauriConfig.version}_x64-setup.exe`;
  const installer = join(
    repoRoot,
    "src-tauri",
    "target",
    target,
    "release",
    "bundle",
    "nsis",
    installerName,
  );

  run("rustup", ["target", "add", target]);
  run(process.execPath, [
    tauriCli,
    "build",
    "--target",
    target,
    "--bundles",
    "nsis",
    "--config",
    updaterOverride,
  ]);
  copyToDesktop(installer, windowsDesktop());
} else {
  fail("Usage: build-desktop-installer.mjs <macos|windows>");
}
