#!/usr/bin/env node
// The fixture's stand-in for the board writes the files the planner reads on every refresh. Two things must hold, or the
// page checks fail on a loaded machine and pass on a quiet one (PLN-108): a reader never sees a file half-written, and
// two board commands at once do not lose each other's setting. A lost `merge.pr false` turned a GitLab checkout refusal
// back into the board's «Одобрить», and `pages (ui3)` was red on GitHub Actions while the yard was green.
//
//   node scripts/check-fixture.mjs
//
// A fixture is built in a temporary folder, a reader parses settings/config.json in a loop while sixteen `set`s go at
// once, and then every one of them must be in the store and in the file the planner reads. Nothing real is touched.

import { execFileSync, spawn } from 'node:child_process';
import { mkdtempSync, readFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const here = dirname(fileURLToPath(import.meta.url));
// Long enough for the reader to look many times at a file that is being rewritten, short enough for every check run.
const READ_MS = 2500;
const WRITERS = 16;

let failed = 0;
const check = (name, ok, detail = '') => {
  console.log(`${ok ? 'ok  ' : 'FAIL'} ${name}${detail ? ' — ' + detail : ''}`);
  if (!ok) failed++;
};

const dir = mkdtempSync(join(tmpdir(), 'herdr-fixture-check-'));
const store = join(dir, 'state', 'board-settings.json');
const config = join(dir, 'settings', 'config.json');
const counts = join(dir, 'reader.json');
const json = (file) => { try { return JSON.parse(readFileSync(file, 'utf8')); } catch { return null; } };

try {
  execFileSync(process.execPath, [join(here, 'fixture.mjs'), dir], { stdio: 'ignore' });
  // The fixture's own route, which the page checks read: delta is a GitLab repository, merged through an MR.
  check('the fixture says delta is merged through an MR (merge.pr false)', json(config)?.repos?.delta?.merge?.pr === false, config);

  // A reader of its own process, as the planner is: it reads and parses the file as fast as it can while the writers go.
  // It runs before the writes start, so a slow machine cannot make the window of the check narrower.
  const reader = spawn(process.execPath, ['-e', `
    const fs = require('node:fs');
    const [file, out, ms] = process.argv.slice(1);
    let reads = 0, torn = 0;
    const end = Date.now() + Number(ms);
    while (Date.now() < end) {
      try { JSON.parse(fs.readFileSync(file, 'utf8')); reads++; } catch (e) { torn++; }
    }
    fs.writeFileSync(out, JSON.stringify({ reads, torn }));
  `, config, counts, String(READ_MS)], { stdio: 'ignore' });
  const readerDone = new Promise((resolve) => reader.on('close', resolve));

  // Sixteen board commands at once: every one sets a name of its own, half of them for the repository alpha.
  const codes = await Promise.all(Array.from({ length: WRITERS }, (_, i) => new Promise((resolve) => {
    // Staggered a little: the racing pair is a command that starts while another one is between its read and its write.
    setTimeout(() => {
      const p = spawn(join(dir, 'bin', 'herdr-projects'), ['set', `probe.n${i}`, String(i), ...(i % 2 ? ['--repo', 'alpha'] : [])], { stdio: 'ignore' });
      p.on('close', resolve);
      p.on('error', () => resolve(-1));
    }, Math.floor(Math.random() * 60));
  })));
  check('every board command answered 0', codes.every((c) => c === 0), JSON.stringify(codes));
  await readerDone;

  const read = json(counts) || { reads: 0, torn: 0 };
  check('the reader never saw settings/config.json half-written', read.torn === 0, `${read.torn} of ${read.reads} reads did not parse`);
  check('the reader really did look', read.reads > 100, `${read.reads} reads`);
  const kept = json(store) || {};
  const lost = Array.from({ length: WRITERS }, (_, i) => i).filter((i) => ((i % 2 ? kept.alpha : kept['']) || {})[`probe.n${i}`] !== String(i));
  check('no level of the store was lost by a board command running at the same time', lost.length === 0, lost.length ? `lost: probe.n${lost.join(', probe.n')}` : `${WRITERS} names kept`);
  const seen = json(config) || {};
  const missed = Array.from({ length: WRITERS }, (_, i) => i).filter((i) => ((i % 2 ? seen.repos?.alpha : seen)?.probe || {})[`n${i}`] !== String(i));
  check('and settings/config.json has all of them (the planner reads its merge route there)', missed.length === 0, missed.length ? `missing: probe.n${missed.join(', probe.n')}` : `${WRITERS} names`);
  check('the GitLab route of delta is still there after the writes', seen.repos?.delta?.merge?.pr === false, JSON.stringify(seen.repos?.delta));
} finally {
  rmSync(dir, { recursive: true, force: true });
}
console.log(failed ? `check-fixture: failed: ${failed}` : 'check-fixture: ok');
process.exit(failed ? 1 : 0);
