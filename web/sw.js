// Service worker:
//  1. adds cross-origin isolation headers (COOP/COEP) so SharedArrayBuffer works
//     (multi-threaded GPU) - GitHub Pages can't send these headers itself;
//  2. caches the app so it starts offline once installed (network-first, so updates
//     arrive automatically whenever the device is online).
const CACHE = 'ps1web-v1';
const SHELL = [
  './', 'index.html', 'style.css', 'manifest.webmanifest',
  'js/main.js', 'js/worker.js', 'js/gpu-worker.js', 'js/import-worker.js', 'js/audio-worklet.js',
  'js/presenter.js', 'js/painter.js', 'js/ring.js', 'js/input.js', 'js/touch.js', 'js/disc.js',
  'js/library.js', 'js/memcard.js', 'js/storage.js', 'js/cheatdb.js', 'js/hw.js', 'js/touch.js', 'data/ps1-cheats.json',
  'pkg/ps1core.js', 'pkg/ps1core_bg.wasm',
  'icons/icon-192.png', 'icons/icon-512.png', 'icons/icon-maskable-512.png', 'icons/apple-touch-icon.png', 'icons/favicon-32.png',
];

self.addEventListener('install', (e) => {
  self.skipWaiting();
  e.waitUntil(caches.open(CACHE).then((c) => Promise.allSettled(SHELL.map((u) => c.add(new Request(u, { cache: 'reload' }))))));
});

self.addEventListener('activate', (e) => {
  e.waitUntil((async () => {
    for (const k of await caches.keys()) if (k !== CACHE) await caches.delete(k);
    await self.clients.claim();
  })());
});

function isolate(res) {
  if (!res || res.status === 0 || res.type === 'opaque') return res;
  const h = new Headers(res.headers);
  h.set('Cross-Origin-Embedder-Policy', 'require-corp');
  h.set('Cross-Origin-Opener-Policy', 'same-origin');
  h.set('Cross-Origin-Resource-Policy', 'same-origin');
  return new Response(res.body, { status: res.status, statusText: res.statusText, headers: h });
}

self.addEventListener('fetch', (e) => {
  const req = e.request;
  if (req.cache === 'only-if-cached' && req.mode !== 'same-origin') return;
  if (new URL(req.url).origin !== self.location.origin) return;
  e.respondWith((async () => {
    try {
      const res = await fetch(req);
      if (req.method === 'GET' && res.ok) {
        const copy = res.clone();
        caches.open(CACHE).then((c) => c.put(req, copy)).catch(() => {});
      }
      return isolate(res);
    } catch (err) {
      const hit = await caches.match(req, { ignoreSearch: true });
      if (hit) return isolate(hit);
      throw err;
    }
  })());
});
