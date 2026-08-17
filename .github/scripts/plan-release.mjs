import { execFileSync } from "node:child_process";
import { appendFileSync, readFileSync } from "node:fs";

const requestedBump = process.env.RELEASE_BUMP ?? "auto";
const allowedBumps = new Set(["auto", "patch", "minor", "major"]);

if (!allowedBumps.has(requestedBump)) {
  throw new Error(`Unsupported release bump: ${requestedBump}`);
}

function git(...args) {
  return execFileSync("git", args, { encoding: "utf8" }).trim();
}

function parseVersion(value) {
  const match = /^(\d+)\.(\d+)\.(\d+)$/.exec(value);
  if (!match) {
    throw new Error(`Expected a stable semantic version, received: ${value}`);
  }

  return match.slice(1).map(Number);
}

function compareVersions(left, right) {
  for (let index = 0; index < left.length; index += 1) {
    if (left[index] !== right[index]) {
      return left[index] - right[index];
    }
  }
  return 0;
}

function detectBump(commits) {
  let level = 0;

  for (const message of commits) {
    const subject = message.split("\n", 1)[0];
    const breaking =
      /^[a-z]+(?:\([^)]+\))?!:/.test(subject) ||
      /(?:^|\n)BREAKING(?:-| )CHANGE:\s/m.test(message);

    if (breaking) {
      return "major";
    }
    if (/^feat(?:\([^)]+\))?:/.test(subject)) {
      level = Math.max(level, 2);
    } else if (/^(?:fix|perf)(?:\([^)]+\))?:/.test(subject)) {
      level = Math.max(level, 1);
    }
  }

  return [null, "patch", "minor"][level];
}

function bumpVersion(version, bump) {
  const [major, minor, patch] = version;

  if (bump === "major") return [major + 1, 0, 0];
  if (bump === "minor") return [major, minor + 1, 0];
  return [major, minor, patch + 1];
}

function writeOutputs(values) {
  const lines = Object.entries(values).map(([key, value]) => `${key}=${value}`);

  if (process.env.GITHUB_OUTPUT) {
    appendFileSync(process.env.GITHUB_OUTPUT, `${lines.join("\n")}\n`);
  }
  console.log(lines.join("\n"));
}

const stableTags = git("tag", "--list", "v*")
  .split("\n")
  .filter(Boolean)
  .map((tag) => {
    const match = /^v(\d+\.\d+\.\d+)$/.exec(tag);
    return match ? { tag, version: parseVersion(match[1]) } : null;
  })
  .filter(Boolean)
  .sort((left, right) => compareVersions(right.version, left.version));

const latest = stableTags[0];
const baseVersion = latest
  ? latest.version
  : parseVersion(JSON.parse(readFileSync("package.json", "utf8")).version);
const range = latest ? `${latest.tag}..HEAD` : "HEAD";
const log = execFileSync("git", ["log", "-z", "--format=%B", range], {
  encoding: "utf8",
});
const commits = log.split("\0").map((message) => message.trim()).filter(Boolean);
const bump = requestedBump === "auto" ? detectBump(commits) : requestedBump;

if (!bump) {
  writeOutputs({ should_release: "false", version: "", tag_name: "" });
  process.exit(0);
}

const version = bumpVersion(baseVersion, bump).join(".");
writeOutputs({ should_release: "true", version, tag_name: `v${version}` });
