#!/usr/bin/env bash
# The fork is maintained in a local checkout, including unpublished changes. There is no
# public GitHub SHA to pin; IROHAMOD_FRONTEND selects the source snapshot for this build.
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"
node --input-type=module <<'JS'
import fs from 'node:fs';
import path from 'node:path';
const source = path.resolve(process.env.IROHAMOD_FRONTEND || '../moeflow-irohamod/irohamod/frontend');
const target = path.resolve('frontend-irohamod');
if (source === target || source.startsWith(target + path.sep)) throw new Error('Source must be outside frontend-irohamod');
if (!fs.existsSync(path.join(source, 'package.json'))) throw new Error(`Missing frontend: ${source}`);
fs.mkdirSync(target, { recursive: true });
// Preserve the dependency junction; never traverse or remove its target while refreshing.
const excluded = new Set(['node_modules', 'build', '.git']);
for (const name of fs.readdirSync(target)) {
  if (!excluded.has(name)) fs.rmSync(path.join(target, name), { recursive: true, force: true });
}
for (const name of fs.readdirSync(source)) {
  if (excluded.has(name)) continue;
  fs.cpSync(path.join(source, name), path.join(target, name), {
    recursive: true, dereference: true,
    filter: file => !excluded.has(path.basename(file)),
  });
}
console.log(`Vendored irohamod from ${source}`);
JS
