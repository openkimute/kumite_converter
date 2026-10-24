// The converter's worker (convert.html): convert_web in wasm, one thread, with the functions it imports:
// the picked files (FileReaderSync on slices, 1 MB read ahead), the output and the log. The output is the pack in
// OPFS (assets.pack and assets.idx through sync access handles), or a folder the user picked (`dir` in the message,
// Chrome and Edge: async functions the wasm waits on through JSPI). Messages: {cmd: 'probe' | 'convert' |
// 'fingerprint', dir?}.
// (the JSPI build where the browser has JSPI: its exnref would not compile in Safari before 18.4)
const ready = import(WebAssembly.Suspending ? './pkg/convert_web_jspi.js' : './pkg/convert_web.js')
  .then(async (m) => { await m.default(); return m; });

let files = [];   // the picked files, by id
let handles = []; // store 0: assets.pack, 1: assets.idx
let dir = null;   // the output folder
let folders = new Map(); // its sub-folders' handles by path, as found or made
const reader = new FileReaderSync();
let ahead = { id: -1, at: 0, bytes: new Uint8Array(0) };
// (Rust gets "Name: message" strings: a QuotaExceededError keeps its name)
const message = (e) => (typeof e === 'string' ? e : `${e?.name}: ${e?.message}`);
const wrap = (f) => (...a) => { try { return f(...a); } catch (e) { throw message(e); } };
const wrapAsync = (f) => async (...a) => { try { return await f(...a); } catch (e) { throw message(e); } };
// the output folder's sub-folder at `path` ('': the folder), made if `create`
const folder = async (path, create) => {
  let h = dir, at = '';
  for (const name of path.split('/').filter(Boolean)) {
    at = at ? `${at}/${name}` : name;
    let next = folders.get(at);
    if (!next) folders.set(at, (next = await h.getDirectoryHandle(name, { create })));
    h = next;
  }
  return h;
};
// a path as [its folder's path, its name]
const split = (path) => {
  const parts = path.split('/').filter(Boolean);
  return [parts.slice(0, -1).join('/'), parts.at(-1) ?? ''];
};
const missing = (e) => e?.name === 'NotFoundError' || e?.name === 'TypeMismatchError';
Object.assign(self, {
  t3c_dir_read: wrapAsync(async (path) => {
    const [d, name] = split(path);
    const file = await (await (await folder(d, false)).getFileHandle(name)).getFile();
    return new Uint8Array(await file.arrayBuffer());
  }),
  t3c_dir_write: wrapAsync(async (path, data) => {
    data = data.slice(); // (a view of the wasm's memory)
    const [d, name] = split(path);
    const w = await (await (await folder(d, true)).getFileHandle(name, { create: true })).createWritable();
    try { await w.write(data); await w.close(); } catch (e) { await w.abort().catch(() => {}); throw e; }
  }),
  t3c_dir_exists: async (path) => {
    const [d, name] = split(path);
    try {
      const f = await folder(d, false);
      if (name) await f.getFileHandle(name).catch(() => f.getDirectoryHandle(name));
      return true;
    } catch { return false; }
  },
  t3c_dir_list: wrapAsync(async (path) => {
    let f;
    try { f = await folder(path, false); } catch (e) { if (missing(e)) return ''; throw e; }
    const names = [];
    for await (const [name, h] of f.entries()) names.push(h.kind === 'directory' ? `${name}/` : name);
    return names.join('\n');
  }),
  t3c_dir_remove: wrapAsync(async (path) => {
    const [d, name] = split(path);
    if (!name) throw 'the converter does not empty the folder picked';
    const gone = d ? `${d}/${name}` : name;
    for (const k of [...folders.keys()]) if (k === gone || k.startsWith(`${gone}/`)) folders.delete(k);
    try { await (await folder(d, false)).removeEntry(name, { recursive: true }); } catch (e) { if (!missing(e)) throw e; }
  }),
  t3c_input_name: (id) => files[id].name,
  t3c_input_size: (id) => files[id].size,
  t3c_input_read: wrap((id, at, buf) => {
    const end = at + buf.length;
    if (ahead.id !== id || at < ahead.at || end > ahead.at + ahead.bytes.length) {
      const slice = files[id].slice(at, Math.max(end, at + (1 << 20)));
      ahead = { id, at, bytes: new Uint8Array(reader.readAsArrayBuffer(slice)) };
      if (end > at + ahead.bytes.length) throw `${files[id].name} ends before byte ${end}`;
    }
    buf.set(ahead.bytes.subarray(at - ahead.at, end - ahead.at));
  }),
  t3c_store_read: wrap((s, at, buf) => {
    if (handles[s].read(buf, { at }) !== buf.length) throw `the pack ends before byte ${at + buf.length}`;
  }),
  t3c_store_write: wrap((s, at, data) => {
    if (handles[s].write(data, { at }) !== data.length) throw 'QuotaExceededError: a write was cut short';
  }),
  t3c_store_size: wrap((s) => handles[s].getSize()),
  t3c_store_truncate: wrap((s, len) => handles[s].truncate(len)),
  t3c_store_flush: wrap((s) => handles[s].flush()),
  t3c_log: (line) => postMessage({ log: line }),
  t3c_panic: (message) => postMessage({ panic: message }),
});

onmessage = async ({ data }) => {
  const root = await navigator.storage.getDirectory();
  try {
    if (data.cmd === 'probe') {
      // sync access handles work (Safari before 17 lacks them or fails)
      const h = await (await root.getFileHandle('probe.tmp', { create: true })).createSyncAccessHandle();
      h.write(new Uint8Array([1]), { at: 0 });
      h.close();
      await root.removeEntry('probe.tmp');
      return postMessage({ probe: true });
    }
    const { convert, fingerprint } = await ready;
    dir = data.dir ?? null;
    folders = new Map();
    for (const name of dir ? [] : ['assets.pack', 'assets.idx']) {
      const file = await root.getFileHandle(name, { create: true });
      // (a cancelled conversion's worker, terminated, lets go of its handles a moment later)
      for (let tries = 0; ; tries++) {
        try { handles.push(await file.createSyncAccessHandle()); break; } catch (e) {
          if (e?.name !== 'NoModificationAllowedError' || tries >= 50) throw e;
          await new Promise((ok) => setTimeout(ok, 100));
        }
      }
    }
    if (data.cmd === 'fingerprint') return postMessage({ fingerprint: await fingerprint(!!dir) });
    files = [];
    const id = (f) => (f ? files.push(f) - 1 : null);
    const options = {
      t3: data.t3.map(id), t2: data.t2?.map(id) ?? null, cs: data.cs?.map(([path, f]) => [path, id(f)]) ?? null, now: data.now, dir: !!dir,
    };
    postMessage({ done: await convert(JSON.stringify(options)) });
  } catch (e) {
    const error = message(e);
    // out of space: no partial output stays in the browser's storage (a folder the user picked keeps what it got)
    const quota = /QuotaExceededError/.test(error);
    if (quota) for (const h of handles) h.truncate(0);
    postMessage({ error, quota });
  } finally {
    for (const h of handles) h.close();
    handles = [];
  }
};
