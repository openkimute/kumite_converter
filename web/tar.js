// The converted game in the browser's storage (the pack: assets.pack and its index) as a tar archive, for browsers
// without a folder on the disk (Firefox, Safari): convert.html's "Save as .tar". The Blob is built from slices of the
// pack's File, so nothing is read into memory; the browser streams it from the disk as the download is saved.
// ustar headers (a PAX header for a path longer than ustar holds); everything under `root`/.
const enc = new TextEncoder();

// a 512-byte header: `name` (bytes), `size`, `type` ('0' a file, 'x' a PAX record)
function header(name, size, type, mtime, prefix = new Uint8Array(0)) {
  const h = new Uint8Array(512);
  const put = (at, len, bytes) => h.set(bytes.subarray(0, len), at);
  const octal = (at, len, n) => put(at, len, enc.encode(`${n.toString(8).padStart(len - 1, '0')}\0`));
  put(0, 100, name);
  octal(100, 8, 0o644);
  octal(108, 8, 0);
  octal(116, 8, 0);
  octal(124, 12, size);
  octal(136, 12, mtime);
  h.fill(0x20, 148, 156); // the checksum's own field counts as spaces
  h[156] = type.charCodeAt(0);
  put(257, 8, enc.encode('ustar\x0000'));
  put(345, 155, prefix);
  const sum = h.reduce((a, b) => a + b, 0);
  put(148, 8, enc.encode(`${sum.toString(8).padStart(6, '0')}\0 `));
  return h;
}

const pad = (size) => new Uint8Array((512 - (size % 512)) % 512);

// the path as ustar's (prefix, name): split at a '/' so that each fits, or null
function split(path) {
  if (path.length <= 100) return [new Uint8Array(0), path];
  for (let i = path.lastIndexOf('/'); i > 0; i = path.lastIndexOf('/', i - 1)) {
    if (i <= 155 && path.length - i - 1 <= 100) return [path.slice(0, i), path.slice(i + 1)];
  }
  return null;
}

// the parts of one file's entry
function entry(path, size, mtime) {
  const bytes = enc.encode(path);
  const parts = bytes.length === path.length ? split(path) : null; // (ustar names: ASCII here)
  if (parts) return [header(enc.encode(parts[1]), size, '0', mtime, enc.encode(parts[0]))];
  // a PAX record "LEN path=PATH\n", LEN counting itself
  const body = (len) => `${len} path=${path}\n`;
  let len = body(0).length;
  while (enc.encode(body(len)).length !== len) len = enc.encode(body(len)).length;
  const record = enc.encode(body(len));
  return [header(enc.encode('PaxHeader'), record.length, 'x', mtime), record, pad(record.length),
    header(enc.encode(path.slice(-100)), size, '0', mtime)];
}

// the archive: `idx` the pack's index ({files: {path: [offset, length]}}), `data` the pack (a File or Blob)
export function tarBlob(idx, data, root = 'assets') {
  const mtime = Math.floor(Date.now() / 1000);
  const parts = [];
  for (const path of Object.keys(idx.files).sort()) {
    if (path.startsWith('.tmp/')) continue; // (the converter's scratch)
    const [at, size] = idx.files[path];
    parts.push(...entry(`${root}/${path}`, size, mtime), data.slice(at, at + size), pad(size));
  }
  parts.push(new Uint8Array(1024)); // the end: two zero blocks
  return new Blob(parts, { type: 'application/x-tar' });
}
