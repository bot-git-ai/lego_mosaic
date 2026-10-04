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
// There is no `skipWaiting`: an update takes over once the old app's tabs
// close, so nobody gets a page from one version and a worker from another.
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
self.addEventListener('install', event => {
  event.waitUntil(caches.open(CACHE).then(cache => cache.addAll(ASSETS)));
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
  const url = event.request.url;
  if (event.request.method !== 'GET' || !IS_OWN(url) || !ASSETS.includes(url)) return;
  event.respondWith((async () => {
    const cache = await caches.open(CACHE);
    const cached = await cache.match(event.request);
    return cached || fetch(event.request);
  })());
});
