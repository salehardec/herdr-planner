#!/usr/bin/env node
// A big Galaxy: 50 universes / 500 planets (scripts/fixture.mjs --scale 50x500). The layout must stay orderly
// (nothing overlaps, everything inside the boundary), the labels few and without collisions, and the
// page's own work per frame small. The frame time in the headless browser is software GL: it is only
// a sanity bound; the real number (median < 20 ms on integrated graphics) is checked by hand.
//   FIXTURE=DIR node scripts/perf-check.mjs http://127.0.0.1:PORT [screenshot-dir]

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

for (let i = 0; i < 60; i++) { port = port || readPort(); try { if (port && (await fetch(`http://127.0.0.1:${port}/json/version`)).ok) break; } catch {} await sleep(200); }
const tabs = await (await fetch(`http://127.0.0.1:${port}/json/list`)).json();
const ws = new WebSocket(tabs.find((t) => t.type === 'page').webSocketDebuggerUrl);
await new Promise((r) => { ws.onopen = r; });
let id = 0; const pending = new Map(); const errors = [];
ws.onmessage = (e) => { const m = JSON.parse(e.data); if (m.id && pending.has(m.id)) { pending.get(m.id)(m); pending.delete(m.id); } else if (m.method === 'Runtime.exceptionThrown') errors.push(m.params.exceptionDetails?.exception?.description || 'exception'); else if (m.method === 'Runtime.consoleAPICalled' && m.params.type === 'error') errors.push(m.params.args?.map((a) => a.value || a.description).join(' ')); };
// A browser that stopped answering (a crashed renderer) would leave the check waiting for ever: when the latest call
// has had no reply for 120 s, say so and stop. (A call somebody gave up on earlier does not count: later ones are answered.)
let lastSentAt = 0; let lastRepliedAt = 0;
setInterval(() => {
  if (lastSentAt > lastRepliedAt && Date.now() - lastSentAt > 120000) { console.log('FAIL the browser stopped answering'); process.exit(1); }
}, 5000).unref();
const send = (method, params = {}) => new Promise((r) => {
  const i = ++id;
  lastSentAt = Date.now();
  pending.set(i, (m) => { lastRepliedAt = Date.now(); r(m); });
  ws.send(JSON.stringify({ id: i, method, params }));
});
const evalJs = async (expr) => (await send('Runtime.evaluate', { expression: expr, returnByValue: true, awaitPromise: true })).result?.result?.value;
await send('Runtime.enable'); await send('Page.enable');
await send('Emulation.setFocusEmulationEnabled', { enabled: true });
await send('Page.navigate', { url: base + '/?noauto&nocoach&bloom=0' });
for (let i = 0; i < 100; i++) { if (await evalJs('!!window.world && world.debug().planets > 400')) break; await sleep(300); }
await sleep(3000);

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
// and one slow frame says more about the load than about the page.
const bestUpdate = () => evalJs(`(async () => { let best = Infinity; for (let i = 0; i < 12; i++) { best = Math.min(best, world.debug().updateMs); await new Promise((r) => setTimeout(r, 250)); } return best; })()`);
const d = await evalJs('(() => { const x = world.debug(); return { planets: x.planets, universes: x.universes, quiet: x.quiet, labels: x.labels, collisions: x.collisions, updateMs: x.updateMs, particles: x.particles, frameMs: x.frameMs, webgl: x.webgl }; })()');
check('every planet is there: 500 and more', d.planets >= 500, JSON.stringify(d));
check('50 universes and more, a good part of them quiet', d.universes >= 50 && d.quiet >= 15, JSON.stringify({ u: d.universes, q: d.quiet }));
const lo = await evalJs(labelOverlaps);
check('the labels are few: at most 24, none on top of another', d.labels > 0 && d.labels <= 24 && lo.shown === d.labels && lo.overlaps === 0, JSON.stringify({ labels: d.labels, ...lo }));
const upd = await bestUpdate();
check('the page\'s own work per frame is small (< 12 ms for the update, with other checks running)', upd < 12, `${upd} ms`);
check('the particles are bounded (LOD): under 60 000', d.particles < 60000, String(d.particles));
if (shots) { const r = await send('Page.captureScreenshot', { format: 'png' }); writeFileSync(join(shots, 'scale-overview.png'), Buffer.from(r.result.data, 'base64')); }
// into a nebula: the cost stays small and the labels stay in order
await evalJs(`planner.navigateTo({kind:'universe',galaxy:'',id:'s05'}); true`);
for (let i = 0; i < 80; i++) { if (await evalJs('world.debug().flight === "idle"')) break; await sleep(250); }
await sleep(1500);
const inside = await evalJs('(() => { const x = world.debug(); return { level: x.level, labels: x.labels, updateMs: x.updateMs }; })()');
const lo2 = await evalJs(labelOverlaps);
check('inside a nebula: the labels stay few and apart', inside.level === 'nebula' && inside.labels <= 24 && lo2.overlaps === 0 && (await bestUpdate()) < 12, JSON.stringify({ ...inside, ...lo2 }));
// the selected planet's card and the one under the pointer must not sit on other labels either
await evalJs(`planner.navigateTo({kind:'planet',galaxy:'',id:'s05/1'}); true`);
for (let i = 0; i < 80; i++) { if (await evalJs('world.debug().flight === "idle"')) break; await sleep(250); }
await sleep(1500);
const lo3 = await evalJs(labelOverlaps);
check('with a planet selected, its card does not sit on another label', lo3.shown > 0 && lo3.overlaps === 0, JSON.stringify(lo3));
// manual flight across the Galaxy: the boundary holds
await evalJs('planner.goOverview()');
await sleep(3500);
await evalJs('planner.setMode("manual")');
await sleep(4000);
const away = await evalJs(`(async () => {
  for (let i = 0; i < 30; i++) { document.body.dispatchEvent(new KeyboardEvent('keydown', { code: 'KeyS', key: 's', bubbles: true })); await new Promise((r) => setTimeout(r, 250)); }
  document.body.dispatchEvent(new KeyboardEvent('keyup', { code: 'KeyS', key: 's', bubbles: true }));
  const s = world.debug().ship; return { r: Math.hypot(s.x, s.y, s.z), edge: world.debug().edge };
})()`);
check('flying backwards for a long time, the ship does not leave the Galaxy', away.r < 1600, JSON.stringify(away));
check('no console errors', errors.length === 0, errors.slice(0, 2).join('; '));
ws.close();
chrome.kill();
process.exit(failed ? 1 : 0);
