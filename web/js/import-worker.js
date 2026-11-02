// Copies game files into the Origin Private File System (OPFS) so they survive a refresh.
// Uses synchronous access handles (supported in Chrome, Edge, Firefox and Safari workers).
onmessage = async (e) => {
  const { dir, files } = e.data;
  try {
    const root = await navigator.storage.getDirectory();
    const games = await root.getDirectoryHandle('games', { create: true });
    const gdir = await games.getDirectoryHandle(dir, { create: true });
    const total = files.reduce((a, f) => a + f.size, 0);
    const reader = new FileReaderSync();
    const CHUNK = 4 << 20;
    let done = 0;
    let lastReport = 0;
    for (const f of files) {
      const fh = await gdir.getFileHandle(f.name, { create: true });
      const h = await fh.createSyncAccessHandle();
      try {
        h.truncate(0);
        for (let o = 0; o < f.size; o += CHUNK) {
          const buf = new Uint8Array(reader.readAsArrayBuffer(f.slice(o, o + CHUNK)));
          h.write(buf, { at: o });
          done += buf.length;
          const now = performance.now();
          if (now - lastReport > 200) {
            lastReport = now;
            postMessage({ type: 'progress', done, total });
          }
        }
        h.flush();
      } finally {
        h.close();
      }
    }
    postMessage({ type: 'done', total });
  } catch (err) {
    postMessage({ type: 'error', message: String(err?.message || err) });
  }
};
