// Run the Rust test suite with the Common Controls v6 activation manifest attached to each
// test harness.
//
// Why this exists: `download::ask_save_path` reaches `TaskDialogIndirect`, which comctl32.dll
// exports only in its side-by-side (v6) build. The shipped application gets a manifest from
// Tauri, so it is fine. A `cargo test` harness is a separate link with no manifest of its own,
// so Windows loads comctl32 v5 and the process dies with STATUS_ENTRYPOINT_NOT_FOUND
// (0xc0000139) before a single test runs — which looks like a broken test suite rather than a
// missing manifest.
//
// Cargo's `rustc-link-arg-tests` does not reach the library unit-test harness, so the manifest
// is attached here instead: Windows honours an external `<exe>.manifest` sidecar for any
// executable that has no embedded manifest, and this leaves the packaged binary untouched
// (re-embedding a manifest there would collide with Tauri's own resource).
//
// Usage: bun scripts/run-rust-tests.mjs [extra cargo test args...]

import { spawn, spawnSync } from 'node:child_process';
import { writeFileSync, existsSync } from 'node:fs';

const CARGO = process.env.CARGO ?? 'C:/Users/Administrator/.cargo/bin/cargo.exe';
const MANIFEST = `<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0">
  <dependency>
    <dependentAssembly>
      <assemblyIdentity type="win32" name="Microsoft.Windows.Common-Controls"
        version="6.0.0.0" processorArchitecture="*"
        publicKeyToken="6595b64144ccf1df" language="*" />
    </dependentAssembly>
  </dependency>
</assembly>
`;

const extra = process.argv.slice(2);

// `--no-run` first so the sidecar can be written before anything executes.
const listed = spawnSync(CARGO, ['test', '--manifest-path', 'src-tauri/Cargo.toml', '--no-run',
  '--message-format=json', ...extra], { encoding: 'utf8', maxBuffer: 64 * 1024 * 1024 });
if (listed.error) throw listed.error;

const harnesses = [];
for (const line of listed.stdout.split('\n')) {
  if (!line.startsWith('{')) continue;
  let message;
  try { message = JSON.parse(line); } catch { continue; }
  if (message.reason !== 'compiler-artifact' || !message.executable) continue;
  // Only Windows test executables need the manifest.
  if (!message.executable.endsWith('.exe')) continue;
  harnesses.push(message.executable);
}
if (listed.status !== 0 && harnesses.length === 0) {
  process.stderr.write(listed.stderr);
  process.exit(listed.status ?? 1);
}

for (const harness of harnesses) {
  if (!existsSync(harness)) continue;
  writeFileSync(`${harness}.manifest`, MANIFEST);
}
console.log(`Attached the Common Controls v6 manifest to ${harnesses.length} test harness(es).`);

let failed = 0;
for (const harness of harnesses) {
  const run = spawnSync(harness, [...extra.filter(a => !a.startsWith('--message-format'))],
    { stdio: 'inherit' });
  if (run.status !== 0) failed++;
}
if (failed > 0) {
  console.error(`${failed} test harness(es) failed.`);
  process.exit(1);
}
