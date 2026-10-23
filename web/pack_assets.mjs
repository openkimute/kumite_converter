// Dev: a converted assets folder as the browser converter's pack (UNIVERSAL.md §6): OUT/assets.pack (the files'
// bytes) and OUT/assets.idx ({"version": 1, "files": {"path": [offset, length]}, "done": []}), for import_pack.html.
// usage: node web/pack_assets.mjs ASSETS_DIR OUT_DIR
import { readdirSync, readFileSync, statSync, openSync, writeSync, closeSync, writeFileSync, mkdirSync } from 'node:fs';
import { join } from 'node:path';
const [dir, out] = process.argv.slice(2);
if (!dir || !out) { console.error('usage: node web/pack_assets.mjs ASSETS_DIR OUT_DIR'); process.exit(1); }
mkdirSync(out, { recursive: true });
const pack = openSync(join(out, 'assets.pack'), 'w');
const files = {};
let at = 0;
const walk = (rel) => {
  for (const name of readdirSync(join(dir, rel)).sort()) {
    const p = rel ? `${rel}/${name}` : name;
    if (statSync(join(dir, p)).isDirectory()) { walk(p); continue; }
    if (/\.(gz|br)$/.test(name)) continue; // the server's compressed copies
    const bytes = readFileSync(join(dir, p));
    writeSync(pack, bytes);
    files[p] = [at, bytes.length];
    at += bytes.length;
  }
};
walk('');
closeSync(pack);
writeFileSync(join(out, 'assets.idx'), JSON.stringify({ version: 1, files, done: [] }));
console.log(`${Object.keys(files).length} files, ${at} bytes`);
