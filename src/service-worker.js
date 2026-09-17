// Copyright (c) 2026 Witalis Domitrz <witekdomitrz@gmail.com>
// AGPL License
// Browser lifecycle/cache plumbing only; all mosaic/app logic remains Rust.
// No skipWaiting: an update takes over after old app tabs close, avoiding
// mixed-version wasm/bindings during an in-progress build.
const ROOT = new URL('./', self.location.href);
const CACHE = 'mosaic-studio-' + ROOT.pathname + '-__VERSION__';
const ASSETS = ['./', 'mosaic.js', 'mosaic_bg.wasm', 'worker.js', 'manifest.webmanifest', 'icon-192.png', 'icon-512.png'].map(p => new URL(p, ROOT).href);
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
  // Deliberately leave unrelated pages, API requests, files and blobs alone.
  if (event.request.method !== 'GET' || !ASSETS.includes(event.request.url)) return;
  event.respondWith((async () => {
    const cache = await caches.open(CACHE);
    const cached = await cache.match(event.request);
    return cached || fetch(event.request);
  })());
});
