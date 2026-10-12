// Copyright (c) 2026 Witalis Domitrz <witekdomitrz@gmail.com>
// AGPL License
// Browser lifecycle/cache plumbing only; all mosaic/app logic remains Rust.
//
// Every path is relative to `self.location`, so the app installs and works from
// whatever subdirectory it is published under.
//
// The cache name is content-versioned by `build.rs`, which hashes the bytes of
// every other shell file - the wasm included - and this template. Change the
// app, or this caching logic, and the cache name changes with it.
//
// An update never swaps itself in unasked: left alone it waits for every
// page it controls to close, so nobody gets a page from one version and a
// worker from another. The one ask that exists comes from this app's own
// page (see `message` below), which reloads as the handover happens, so no
// page ever straddles the two versions.
//
// The fetch handler only ever answers for a URL inside this app's own
// directory, checked on every request rather than assumed from the scope.
// Scope is a registration's claim, not a promise, and every app on this origin
// shares it with pages that are not this app at all. An allowlist scoped to the
// directory is the guard that holds even if the scope is ever wrong, or a stale
// registration from an earlier version is still in the way.
const ROOT = new URL('./', self.location.href);
const CACHE = 'mosaic-studio-' + ROOT.pathname + '-__VERSION__';
const ASSETS = ['./', 'mosaic.js', 'mosaic_bg.wasm', 'worker.js', 'manifest.webmanifest', 'icon.svg', 'icon-192.png', 'icon-512.png'].map(p => new URL(p, ROOT).href);
const IS_OWN = url => url.startsWith(ROOT.href);
// The manifest's URL on its own. It stays in ASSETS -- a manifest that 404s
// should still fail the install, loudly -- but it is the one precached URL the
// fetch handler will never answer out of the cache (see there).
const MANIFEST = new URL('manifest.webmanifest', ROOT).href;
self.addEventListener('install', event => {
  event.waitUntil(caches.open(CACHE).then(cache => cache.addAll(ASSETS)));
});
// The page's ask. `nudge_worker_update` in `browser.rs` sends this when it
// has a worker installed and waiting: this worker takes over at once, and
// the page - the sender - hears the resulting `controllerchange` and
// reloads. A page that never asks is never swapped, which keeps the
// guarantee above; the message is the ask, and it comes from the one client
// that immediately steps out of the way.
self.addEventListener('message', event => {
  if (event.data === 'SKIP_WAITING') self.skipWaiting();
});
self.addEventListener('activate', event => {
  event.waitUntil((async () => {
    const prefix = 'mosaic-studio-' + ROOT.pathname + '-';
    for (const key of await caches.keys()) {
      if (key.startsWith(prefix) && key !== CACHE) await caches.delete(key);
    }
    await self.clients.claim();
  })());
});
self.addEventListener('fetch', event => {
  // An allowlist, not a catch-all: this worker has no business touching
  // anything it did not precache, including the pages the user navigates away
  // to. The directory check is the second half of the same rule -- a request
  // outside this app's own directory is never this worker's to answer, whatever
  // the scope says.
  //
  // The manifest is the one own-directory GET that always goes to the network.
  // It is what the browser reads to decide what app an install *is*: while a
  // cached copy answered, a fresh install of this app could read a manifest
  // from before any change to it, and derive the same stale identity the
  // family once shared -- the very bug the identity fix was meant to retire.
  // The manifest is tiny; no offline entry point needs it cached.
  const url = event.request.url;
  if (url === MANIFEST) return;
  if (event.request.method !== 'GET' || !IS_OWN(url) || !ASSETS.includes(url)) return;
  event.respondWith((async () => {
    const cache = await caches.open(CACHE);
    const cached = await cache.match(event.request);
    return cached || fetch(event.request);
  })());
});
