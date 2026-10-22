#!/usr/bin/env node
// A big Galaxy: 50 universes / 500 planets (scripts/fixture.mjs --scale 50x500). The layout must stay orderly
// (nothing overlaps, everything inside the boundary), the labels few and without collisions, and the
// page's own work per frame small. The frame time in the headless browser is software GL: it is only
// a sanity bound; the real number (median < 20 ms on integrated graphics) is checked by hand.
//   FIXTURE=DIR node scripts/perf-check.mjs http://127.0.0.1:PORT [screenshot-dir]
// It runs next to other checks on shared machines, so it waits for states (never for a time) and measures the page's own work
// against a yardstick taken in the same page at the same moment, not against a number of milliseconds.

import { spawn } from 'node:child_process';
import { mkdtempSync, writeFileSync, rmSync, readFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

const base = process.argv[2] || 'http://127.0.0.1:7117';
const shots = process.argv[3] || null;
const profile = mkdtempSync(join(tmpdir(), 'herdr-planner-perf-'));
// The browser picks its own debugging port (several checks run side by side; a random port could be taken twice):
// it writes the port into its profile folder.
let port = 0;
const readPort = () => { try { return Number(readFileSync(join(profile, 'DevToolsActivePort'), 'utf8').split('\n')[0]) || 0; } catch { return 0; } };
const chrome = spawn(process.env.CHROMIUM || 'chromium', [
  '--headless=new', '--no-sandbox', '--disable-gpu', '--use-gl=angle', '--use-angle=swiftshader', '--enable-unsafe-swiftshader',
  '--hide-scrollbars', `--user-data-dir=${profile}`, '--remote-debugging-port=0', '--window-size=1500,950', 'about:blank',
], { stdio: 'ignore' });
// Whatever happens, the browser goes and its profile (hundreds of MB in /tmp) goes with it.
process.on('exit', () => {
  try { chrome.kill('SIGKILL'); } catch { /* gone already */ }
  try { rmSync(profile, { recursive: true, force: true, maxRetries: 3 }); } catch { /* best effort */ }
  // the killed browser may still write a last file into the folder: sweep once more a moment later, detached
  try { spawn('sh', ['-c', `sleep 2; rm -rf '${profile}'`], { detached: true, stdio: 'ignore' }).unref(); } catch { /* best effort */ }
});
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
let failed = 0;
const check = (name, ok, extra = '') => { console.log(`${ok ? 'ok  ' : 'FAIL'} ${name}${extra ? ' — ' + String(extra).replace(/\s+/g, ' ').slice(0, 260) : ''}`); if (!ok) failed++; };

for (let i = 0; i < 300; i++) { port = port || readPort(); try { if (port && (await fetch(`http://127.0.0.1:${port}/json/version`)).ok) break; } catch {} await sleep(200); }
const tabs = await (await fetch(`http://127.0.0.1:${port}/json/list`)).json();
const ws = new WebSocket(tabs.find((t) => t.type === 'page').webSocketDebuggerUrl);
await new Promise((r) => { ws.onopen = r; });
let id = 0; const pending = new Map(); const errors = [];
ws.onmessage = (e) => { const m = JSON.parse(e.data); if (m.id && pending.has(m.id)) { pending.get(m.id)(m); pending.delete(m.id); } else if (m.method === 'Runtime.exceptionThrown') errors.push(m.params.exceptionDetails?.exception?.description || 'exception'); else if (m.method === 'Runtime.consoleAPICalled' && m.params.type === 'error') errors.push(m.params.args?.map((a) => a.value || a.description).join(' ')); };
// A browser that stopped answering (a crashed renderer) would leave the check waiting for ever: when the latest call
// has had no reply for 300 s, say so and stop. (A call somebody gave up on earlier does not count: later ones are answered.)
let lastSentAt = 0; let lastRepliedAt = 0;
setInterval(() => {
  if (lastSentAt > lastRepliedAt && Date.now() - lastSentAt > 300000) { console.log('FAIL the browser stopped answering'); process.exit(1); }
}, 5000).unref();
const send = (method, params = {}) => new Promise((r) => {
  const i = ++id;
  lastSentAt = Date.now();
  pending.set(i, (m) => { lastRepliedAt = Date.now(); r(m); });
  ws.send(JSON.stringify({ id: i, method, params }));
});
const evalJs = async (expr) => (await send('Runtime.evaluate', { expression: expr, returnByValue: true, awaitPromise: true })).result?.result?.value;
// Waits for a state of the page; the limit only ends a wait that will never come.
const waitFor = async (expr, ms = 30000) => {
  const end = Date.now() + ms;
  while (Date.now() < end) { if (await evalJs(expr)) return true; await sleep(100); }
  return false;
};
// The world has drawn n more frames.
const frames = (n) => evalJs(`new Promise((done) => { let k = ${n}; const f = () => (--k <= 0 ? done(true) : requestAnimationFrame(f)); requestAnimationFrame(f); setTimeout(() => done(false), 30000); })`);
const flightIdle = () => waitFor('world.debug().flight === "idle"', 90000);
await send('Runtime.enable'); await send('Page.enable');
await send('Emulation.setFocusEmulationEnabled', { enabled: true });
await send('Page.navigate', { url: base + '/?noauto&nocoach&bloom=0' });
await waitFor('!!window.world && world.debug().planets > 400', 90000);
await waitFor('world.debug().labels > 0', 30000);
await frames(5);

// The labels on the page: the boxes of the shown ones, and how many pairs of them overlap.
const labelOverlaps = `(() => {
  const els = [...document.querySelectorAll('#labels > div')].filter((e) => e.style.display !== 'none' && e.getBoundingClientRect().width > 0);
  const boxes = els.map((e) => e.getBoundingClientRect());
  let n = 0;
  const pairs = [];
  for (let i = 0; i < boxes.length; i++) for (let j = i + 1; j < boxes.length; j++) {
    const a = boxes[i], b = boxes[j];
    if (Math.min(a.right, b.right) - Math.max(a.left, b.left) > 0.5 && Math.min(a.bottom, b.bottom) - Math.max(a.top, b.top) > 0.5) { n++; pairs.push([els[i].textContent, els[j].textContent, Math.round(a.left), Math.round(a.top), Math.round(a.width), Math.round(a.height), Math.round(b.left), Math.round(b.top), Math.round(b.width), Math.round(b.height), els[i].className, els[j].className]); }
  }
  return { shown: boxes.length, overlaps: n, pairs };
})()`;
// The page's own work per frame, as the best of a few samples: other checks run on the same machine in parallel,
// and one slow frame says more about the load than about the page. The limit is 24 ms (twice the 12 ms a free machine needs, for what a shared one adds); on a busy one
// it is stretched by how much slower a fixed piece of work runs in this very page right now (the yardstick, best of the
// same samples, about 5 ms when the machine is free).
const bestUpdate = () => evalJs(`(async () => {
  const yard = () => { const t = performance.now(); let x = 0; for (let i = 1; i < 400000; i++) x += Math.sqrt(i) * Math.sin(i); return performance.now() - t + (x === 1 ? 1 : 0); };
  let best = Infinity; let bestYard = Infinity;
  for (let i = 0; i < 12; i++) { best = Math.min(best, world.debug().updateMs); bestYard = Math.min(bestYard, yard()); await new Promise((r) => setTimeout(r, 120)); }
  return { ms: best, yard: Math.round(bestYard * 10) / 10, limit: 24 * Math.max(1, bestYard / 5) };
})()`);
const d = await evalJs('(() => { const x = world.debug(); return { planets: x.planets, universes: x.universes, quiet: x.quiet, labels: x.labels, collisions: x.collisions, updateMs: x.updateMs, particles: x.particles, frameMs: x.frameMs, webgl: x.webgl }; })()');
check('every planet is there: 500 and more', d.planets >= 500, JSON.stringify(d));
check('50 universes and more, a good part of them quiet', d.universes >= 50 && d.quiet >= 15, JSON.stringify({ u: d.universes, q: d.quiet }));
const lo = await evalJs(labelOverlaps);
check('the labels are few: at most 24, none on top of another', d.labels > 0 && d.labels <= 24 && lo.shown === d.labels && lo.overlaps === 0, JSON.stringify({ labels: d.labels, ...lo }));
const upd = await bestUpdate();
check('the page\'s own work per frame is small (< 24 ms for the update, stretched by the yardstick on a busy machine)', upd.ms < upd.limit, JSON.stringify(upd));
check('the particles are bounded (LOD): under 60 000', d.particles < 60000, String(d.particles));
if (shots) { const r = await send('Page.captureScreenshot', { format: 'png' }); writeFileSync(join(shots, 'scale-overview.png'), Buffer.from(r.result.data, 'base64')); }
// into a nebula: the cost stays small and the labels stay in order
await evalJs(`planner.navigateTo({kind:'universe',id:'s05'}); true`);
await flightIdle();
await waitFor('world.debug().level === "nebula" && world.debug().labels > 0', 30000);
await frames(5);
const inside = await evalJs('(() => { const x = world.debug(); return { level: x.level, labels: x.labels, updateMs: x.updateMs }; })()');
const lo2 = await evalJs(labelOverlaps);
const upd2 = await bestUpdate();
check('inside a nebula: the labels stay few and apart', inside.level === 'nebula' && inside.labels <= 24 && lo2.overlaps === 0 && upd2.ms < upd2.limit, JSON.stringify({ ...inside, ...lo2, upd: upd2 }));
// the selected planet's card and the one under the pointer must not sit on other labels either
await evalJs(`planner.navigateTo({kind:'planet',id:'s05/1'}); true`);
await flightIdle();
await waitFor('plannerDebug().selected?.id === "s05/1" && world.debug().labels > 0', 30000);
await frames(5);
const lo3 = await evalJs(labelOverlaps);
check('with a planet selected, its card does not sit on another label', lo3.shown > 0 && lo3.overlaps === 0, JSON.stringify(lo3));
// manual flight across the Galaxy: the boundary holds
await evalJs('planner.goOverview(); true');
await flightIdle();
await evalJs('planner.setMode("manual"); true');
await waitFor('world.mode === "manual"', 30000);
await flightIdle();   // from the overview the ship is first taken inside the boundary
// Backwards until the ship is at the soft edge, and then for a good many frames more: a frame is as long as the machine makes
// it (a step is capped at 0.1 s), so «for a long time» is counted in frames, not in seconds.
const key = (type) => evalJs(`document.body.dispatchEvent(new KeyboardEvent('${type}', { code: 'KeyS', key: 's', bubbles: true })); true`);
await key('keydown');
const atEdge = await waitFor('world.debug().edge === true', 120000);
await frames(40);
const away = await evalJs(`(() => { const s = world.debug().ship; return { r: Math.hypot(s.x, s.y, s.z), edge: world.debug().edge }; })()`);
await key('keyup');
check('flying backwards for a long time, the ship does not leave the Galaxy', atEdge && away.r < 1600, JSON.stringify({ atEdge, ...away }));
check('no console errors', errors.length === 0, errors.slice(0, 2).join('; '));
ws.close();
chrome.kill();
process.exit(failed ? 1 : 0);
