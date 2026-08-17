const token = process.env.GH_TOKEN;
const repository = process.env.GITHUB_REPOSITORY;
const releaseId = process.env.RELEASE_ID;
const apiUrl = process.env.GITHUB_API_URL ?? "https://api.github.com";
const dryRun = process.env.DRY_RUN === "true";

if (!repository || !releaseId) {
  throw new Error("GITHUB_REPOSITORY and RELEASE_ID are required.");
}

if (!token && !dryRun) {
  throw new Error("GH_TOKEN is required unless DRY_RUN=true.");
}

const defaultHeaders = {
  Accept: "application/vnd.github+json",
  "User-Agent": "pulse-release-workflow",
  "X-GitHub-Api-Version": "2022-11-28",
};

if (token) {
  defaultHeaders.Authorization = `Bearer ${token}`;
}

const wait = (milliseconds) =>
  new Promise((resolve) => setTimeout(resolve, milliseconds));

async function request(url, options = {}) {
  let lastError;

  for (let attempt = 1; attempt <= 5; attempt += 1) {
    try {
      const response = await fetch(url, {
        ...options,
        headers: {
          ...defaultHeaders,
          ...options.headers,
        },
      });

      if (response.ok) {
        return response;
      }

      const details = await response.text();
      lastError = new Error(
        `GitHub request failed (${response.status}): ${details}`,
      );

      if (response.status < 500) {
        throw lastError;
      }
    } catch (error) {
      lastError = error;
    }

    if (attempt < 5) {
      await wait(attempt * 2000);
    }
  }

  throw lastError;
}

const releaseResponse = await request(
  `${apiUrl}/repos/${repository}/releases/${releaseId}`,
);
const release = await releaseResponse.json();
const updaterAsset = release.assets.find((asset) => asset.name === "latest.json");

if (!updaterAsset) {
  throw new Error("The draft release does not contain latest.json.");
}

const updaterResponse = await request(updaterAsset.url, {
  headers: { Accept: "application/octet-stream" },
});
const updater = await updaterResponse.json();
const publicUrls = new Map(
  release.assets.map((asset) => [asset.url, asset.browser_download_url]),
);

for (const platform of Object.values(updater.platforms ?? {})) {
  const publicUrl = publicUrls.get(platform.url);

  if (publicUrl) {
    platform.url = publicUrl;
  } else if (!platform.url.startsWith("https://github.com/")) {
    throw new Error(`No public download URL found for ${platform.url}.`);
  }
}

const manifest = `${JSON.stringify(updater, null, 2)}\n`;

if (dryRun) {
  process.stdout.write(manifest);
  process.exit(0);
}

await request(updaterAsset.url, { method: "DELETE" });

const uploadUrl = release.upload_url.replace(/\{.*$/, "");
await request(`${uploadUrl}?name=${encodeURIComponent(updaterAsset.name)}`, {
  method: "POST",
  headers: { "Content-Type": "application/json" },
  body: manifest,
});

console.log(`Updated latest.json for ${release.tag_name}.`);
