// SolidCraft's service worker: the web app keeps working offline and updates itself.
//
// `cargo xtask web-offline` (Trunk's post_build hook) writes the build into the line below: the
// version (workspace version + a hash of every built file) and the files to keep, each with its
// SHA-256, and the same as `build.json` next to it. Files the app only fetches when it needs
// them (the CJK font) aren't kept: they come from the network. Every download is checked
// against its SHA-256: a deployment that lands mid-download (another build's `index.html`) fails
// the download instead of mixing into the kept build. A new build changes this file, so the
// browser installs the new worker, which keeps the whole new build before taking over.
//
// A launch only ever starts a build whose every file is kept, so a half-downloaded update can't
// break it. It asks for `build.json` (waiting at most NAVIGATE_TIMEOUT_MS): offline, slow or
// unchanged, the kept build starts; when a new build is deployed, it is downloaded first
// (waiting at most UPDATE_TIMEOUT_MS; the download goes on in the background after that) and
// started. When the site has no `build.json` (the app was removed), launches go to the network,
// the kept build answering when that fails. So they do when `build.json` is redirected or
// refused (a sign-in in front of the app): the browser then shows the sign-in. Each page then
// gets its files from the build it started (remembered in PAGES), never from another build
// with a file of the same name.
//
// A cache is only deleted when nothing uses it: a download holds its lock (Web Locks, shared,
// in any tab or worker; the browser releases it when the download ends or its worker stops), and
// an open page uses the build it started. Then unfinished downloads are dropped, and so are
// builds replaced by a newer worker (RETIRED). The origin is shared with other apps (github.io)
// and other copies of this one (the release zip works from any path): only caches named after
// this worker's path (CACHE_PREFIX…, PAGES) are ever touched.
//
// Unbuilt (VERSION "dev", as debug builds get it), the worker retires: it drops what a release
// build kept and unregisters, so `trunk serve` after `trunk serve --release --no-autoreload`
// shows the development build. Deploying this file unbuilt switches the worker off for everyone.
/* @build */ const VERSION = "dev", FILES = [];
const RETIRE = VERSION === "dev";

const PATH = new URL(self.registration.scope).pathname;
const CACHE_PREFIX = `solidcraft-app ${PATH} `;
const CACHE = CACHE_PREFIX + VERSION;
/** Which build each open page started: page id → cache name (or NETWORK). */
const PAGES = `solidcraft-app-pages ${PATH}`;
/** A page that came from the network gets its files from there too. */
const NETWORK = "network";
const NAVIGATE_TIMEOUT_MS = 4000;
const UPDATE_TIMEOUT_MS = 20000;
/** The entry that marks a cache as holding its whole build. */
const COMPLETE = "build-complete";
/** The entry that marks a build replaced by a newer worker. */
const RETIRED = "build-retired";
/** A page remembered this recently may still be loading (not yet listed among the clients). */
const PAGE_LOADING_MS = 30 * 1000;

const url = (file) => new URL(file, self.registration.scope).href;
const has = (name, entry) => caches.match(url(entry), { cacheName: name }).then(Boolean);

/** Run `fn` while using the cache `name`: it isn't deleted meanwhile. */
const using = (name, fn) => navigator.locks.request(name, { mode: "shared" }, fn);

/** Delete the cache `name` if nothing is using it and `unwanted()` (asked once no one can start to) agrees. */
const drop = (name, unwanted) =>
  navigator.locks.request(name, { ifAvailable: true }, async (free) => free && (await unwanted()) && caches.delete(name));

const hex = (buffer) => Array.from(new Uint8Array(buffer), (b) => b.toString(16).padStart(2, "0")).join("");

/**
 * Keep every file of a build (an earlier partial download is resumed); resolves to its cache's
 * name. The first file that fails cancels the others.
 */
function keep(version, files) {
  const name = CACHE_PREFIX + version;
  return using(name, async () => {
    const cache = await caches.open(name);
    await cache.delete(url(RETIRED)); // Back in use (a rollback).
    if (await has(name, COMPLETE)) return name;
    const cancel = new AbortController();
    const results = await Promise.allSettled(
      files.map(async ({ file, sha256 }) => {
        try {
          if (await cache.match(url(file))) return;
          // `no-cache`: revalidate with the server, so a stale HTTP-cached page never mixes builds.
          const response = await fetch(new Request(url(file), { cache: "no-cache" }), { signal: cancel.signal });
          if (!response.ok) throw new Error(`${file}: HTTP ${response.status}`);
          const body = await response.arrayBuffer();
          if (hex(await crypto.subtle.digest("SHA-256", body)) !== sha256) throw new Error(`${file}: not this build's`);
          await cache.put(url(file), new Response(body, { headers: response.headers }));
        } catch (error) {
          cancel.abort();
          throw error;
        }
      }),
    );
    const failed = results.find((r) => r.status === "rejected");
    if (failed) throw failed.reason;
    await cache.put(url(COMPLETE), new Response(version));
    return name;
  });
}

const pageEntry = (id) => url(`~page/${encodeURIComponent(id)}`);

/** Remember which build a page started. */
async function started(id, build) {
  if (!id) return;
  const record = JSON.stringify({ build, at: Date.now() });
  await (await caches.open(PAGES)).put(pageEntry(id), new Response(record));
}

/** The build a page started; a page started before it was remembered gets this worker's. */
async function startedBy(id) {
  const response = id ? await caches.match(pageEntry(id), { cacheName: PAGES }) : null;
  return response ? (await response.json()).build : CACHE;
}

/** The builds open pages use (pages closed since are forgotten). */
async function inUse() {
  const open = new Set((await self.clients.matchAll({ includeUncontrolled: true })).map((c) => c.id));
  const pages = await caches.open(PAGES);
  const builds = new Set();
  for (const request of await pages.keys()) {
    const id = decodeURIComponent(request.url.split("~page/")[1] ?? "");
    const record = await pages.match(request).then((r) => r?.json()).catch(() => null);
    if (record && (open.has(id) || Date.now() - record.at < PAGE_LOADING_MS)) builds.add(record.build);
    else await pages.delete(request);
  }
  return builds;
}

/** Drop unfinished downloads nothing is using, and replaced builds no open page uses. */
async function sweep() {
  for (const name of await caches.keys()) {
    if (!name.startsWith(CACHE_PREFIX) || name === CACHE) continue;
    await drop(name, async () => !(await has(name, COMPLETE)) || ((await has(name, RETIRED)) && !(await inUse()).has(name)));
  }
}

self.addEventListener("install", (event) => {
  // Clean up first: a page open for days sees updates (and failed ones) without any launch.
  const install = () => sweep().catch(() => {}).then(() => keep(VERSION, FILES));
  event.waitUntil(RETIRE ? self.skipWaiting() : install().then(() => self.skipWaiting()));
});

self.addEventListener("activate", (event) => {
  event.waitUntil(
    (async () => {
      if (RETIRE) {
        for (const name of await caches.keys()) if (name.startsWith(CACHE_PREFIX) || name === PAGES) await caches.delete(name);
        return self.registration.unregister();
      }
      for (const name of await caches.keys()) {
        if (name.startsWith(CACHE_PREFIX) && name !== CACHE) await (await caches.open(name)).put(url(RETIRED), new Response(""));
      }
      await sweep();
      await self.clients.claim();
    })(),
  );
});

/** The promise's value, or null when it fails or takes longer than `ms`. */
function within(ms, promise) {
  return Promise.race([promise.catch(() => null), new Promise((resolve) => setTimeout(resolve, ms, null))]);
}

/** No app is deployed here any more (the server says `build.json` doesn't exist). */
const GONE = "gone";

/** The site wants a sign-in first (`build.json` is redirected, often to another origin, or refused). */
const SIGN_IN = "sign-in";

/** The deployed build's version and files, GONE, SIGN_IN, or null (unknown: offline, failing, invalid). */
async function deployed() {
  // `manual`: a redirect to another origin would only fail (CORS), looking like being offline.
  const response = await fetch(url("build.json"), { cache: "no-cache", redirect: "manual" });
  if (response.type === "opaqueredirect" || response.status === 401 || response.status === 403) return SIGN_IN;
  if (response.status === 404 || response.status === 410) return GONE;
  const build = response.ok ? await response.json() : null;
  const valid = build && typeof build.version === "string" && Array.isArray(build.files);
  return valid && build.files.every((f) => typeof f.file === "string" && typeof f.sha256 === "string") ? build : null;
}

/** Keep the worker alive for `promise` (allowed while the launch is being answered). */
function extend(event, promise) {
  try {
    event.waitUntil(promise.catch(() => {}));
  } catch {
    // The event has ended; the work goes on while the worker runs.
  }
}

/** A launch: the start page of a complete build (whatever query the URL has: `?sample`), and that build. */
async function launch(event) {
  const kept = await caches.match(url("index.html"), { cacheName: CACHE });
  if (!kept) return [await fetch(event.request), NETWORK]; // Nothing kept yet (the first visit).
  extend(event, sweep());
  const build = await within(NAVIGATE_TIMEOUT_MS, deployed());
  if (build === SIGN_IN) return [await fetch(event.request), NETWORK];
  if (build === GONE) {
    const page = await fetch(event.request).catch(() => null);
    return page && page.ok ? [page, NETWORK] : [kept, CACHE];
  }
  if (!build || build.version === VERSION) return [kept, CACHE];
  const download = keep(build.version, build.files);
  extend(event, download);
  const name = await within(UPDATE_TIMEOUT_MS, download);
  const page = name && (await caches.match(url("index.html"), { cacheName: name }));
  return page ? [page, name] : [kept, CACHE];
}

/**
 * A launch that fails unexpectedly still starts the kept build (or asks the network). A newer
 * worker only takes over once the launch has ended, by when its page is remembered (in use).
 */
async function launchOrKept(event) {
  let page, build;
  try {
    [page, build] = await launch(event);
  } catch {
    page = await caches.match(url("index.html"), { cacheName: CACHE }).catch(() => null);
    build = page ? CACHE : NETWORK;
    page = page || (await fetch(event.request));
  }
  await started(event.resultingClientId, build).catch(() => {});
  return page;
}

/** A page's files come from the build it started, else the network. */
async function asset(event) {
  const build = await startedBy(event.clientId).catch(() => CACHE);
  const kept = build === NETWORK ? null : await caches.match(event.request, { cacheName: build, ignoreSearch: true });
  return kept || fetch(event.request);
}

self.addEventListener("fetch", (event) => {
  const request = event.request;
  if (RETIRE || request.method !== "GET" || new URL(request.url).origin !== self.location.origin) return;
  const path = request.url.split(/[?#]/)[0];
  const start = request.mode === "navigate" && (path === url("") || path === url("index.html"));
  event.respondWith(start ? launchOrKept(event) : asset(event));
});
