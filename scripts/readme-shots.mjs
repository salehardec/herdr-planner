#!/usr/bin/env node
// The pictures of README.md: the page in headless Chromium on the offline fixture, nothing real in them.
//   node scripts/readme-shots.mjs [OUT-DIR]      (default: docs/img)
// It builds a fixture (scripts/fixture.mjs) in a neutral folder, starts the planner on it, drives the page over
// the DevTools protocol and writes JPEG stills and the flight GIF. The planner binary comes from
// $HERDR_PLANNER_BIN, else ${HERDR_PLANNER_TARGET_DIR:-~/.cache/herdr-planner/target}/release|debug (build it first:
// scripts/web-build.sh, cargo build --release). Needs Chromium (CHROMIUM=...), Node 22+, ffmpeg for the GIF.
// The fixture folder is a fresh folder under the system temp dir (demo-XXXXXX) and is named without anything personal: paths of worktrees,
// folders and the machine name (`orion`) show on the pictures, so they must not be a real user's.

import { spawn, execFileSync } from 'node:child_process';
import { mkdtempSync, mkdirSync, writeFileSync, readFileSync, existsSync, rmSync } from 'node:fs';
import { tmpdir, homedir } from 'node:os';
import { join, resolve, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';
import net from 'node:net';

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const out = resolve(process.argv[2] || join(root, 'docs/img'));
mkdirSync(out, { recursive: true });
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

// ── the world: fixture + planner ─────────────────────────────────────────

const cache = process.env.XDG_CACHE_HOME || join(homedir(), '.cache');
const target = process.env.HERDR_PLANNER_TARGET_DIR || join(cache, 'herdr-planner/target');
const bin = process.env.HERDR_PLANNER_BIN
  || ['release', 'debug'].map((p) => join(target, p, 'herdr-planner')).find((p) => existsSync(p));
if (!bin) { console.error('readme-shots: build the planner first (see the header of this file)'); process.exit(2); }
// Its own folder on every run (two runs, or a stale one, must not share a fixture, a port or a profile).
const work = mkdtempSync(join(tmpdir(), 'demo-'));
mkdirSync(join(work, 'home'), { recursive: true });
const D = join(work, 'world');
execFileSync('node', [join(root, 'scripts/fixture.mjs'), D], { stdio: 'ignore' });
mkdirSync(join(D, 'cfg/herdr-planner'), { recursive: true });
mkdirSync(join(D, 'state'), { recursive: true });

const freePort = (start) => new Promise((ok) => {
  const tryOne = (p) => { const s = net.createServer(); s.once('error', () => tryOne(p + 1)); s.once('listening', () => s.close(() => ok(p))); s.listen(p, '127.0.0.1'); };
  tryOne(start);
});
const serverPort = await freePort(7800);
const HOST = 'orion';
const procs = [];
const run = (cmd, args, env) => { const p = spawn(cmd, args, { env, stdio: 'ignore' }); procs.push(p); return p; };
run(bin, ['--port', String(serverPort), '--data', join(D, 'board')], {
  ...process.env, HOME: join(work, 'home'), HERDR_PLANNER_HOST: HOST, CODEX_HOME: join(D, 'codex'),
  HERDR_PLANNER_CLAUDE: join(D, 'bin/claude'), HERDR_PLANNER_GH: join(D, 'bin/gh'), HERDR_PLANNER_GLAB: join(D, 'bin/glab'),
  HERDR_BIN: join(D, 'bin/herdr'), HERDR_PLANNER_TERMINAL: join(D, 'bin/xdg-terminal-exec'), FIXTURE_DIR: D,
  HERDR_PROJECTS_BIN: join(D, 'bin/herdr-projects'), HERDR_SOCKET_PATH: join(D, 'herdr.sock'), OMARCHY_THEME_DIR: join(D, 'theme'),
  CLAUDE_CONFIG_DIR: join(D, 'claude'), PATH: `${join(D, 'bin')}:${process.env.PATH}`, XDG_STATE_HOME: join(D, 'state'),
  XDG_CONFIG_HOME: join(D, 'cfg'), XDG_CACHE_HOME: join(D, 'cache'),
});
const base = `http://127.0.0.1:${serverPort}`;
// a second planner with a password, for the login page
const loginPort = await freePort(serverPort + 1);
const authCfg = join(D, 'cfg-auth');
mkdirSync(join(authCfg, 'herdr-planner'), { recursive: true });
const hash = execFileSync(bin, ['hash-password'], { input: 'demo-password\n', env: { ...process.env, XDG_CONFIG_HOME: authCfg } }).toString().trim();
execFileSync(bin, ['set-auth', '--user', 'me', '--listen', '127.0.0.1'], { input: hash, env: { ...process.env, XDG_CONFIG_HOME: authCfg }, stdio: ['pipe', 'ignore', 'inherit'] });
run(bin, ['--port', String(loginPort), '--data', join(D, 'board')], {
  ...process.env, HOME: join(work, 'home'), HERDR_PROJECTS_BIN: join(D, 'bin/herdr-projects'), PATH: `${join(D, 'bin')}:${process.env.PATH}`,
  XDG_STATE_HOME: join(D, 'state-auth'), XDG_CONFIG_HOME: authCfg, XDG_CACHE_HOME: join(D, 'cache'),
});
for (let i = 0; i < 80; i++) { try { if ((await fetch(`${base}/api/state`)).ok) break; } catch { /* not yet */ } await sleep(250); }

// ── the browser ──────────────────────────────────────────────────────────

const profile = mkdtempSync(join(work, 'profile-'));
const chrome = run(process.env.CHROMIUM || 'chromium', [
  '--headless=new', '--no-sandbox', '--disable-gpu', '--use-gl=angle', '--use-angle=swiftshader', '--enable-unsafe-swiftshader',
  '--hide-scrollbars', `--user-data-dir=${profile}`, '--remote-debugging-port=0', '--window-size=1440,900', 'about:blank',
], process.env);
const cleanup = () => { for (const p of procs) { try { p.kill('SIGKILL'); } catch { /* gone */ } } try { rmSync(work, { recursive: true, force: true }); } catch { /* best effort */ } };
process.on('exit', cleanup);
process.on('SIGINT', () => process.exit(130));

let port = 0;
for (let i = 0; i < 100 && !port; i++) {
  try { port = Number(readFileSync(join(profile, 'DevToolsActivePort'), 'utf8').split('\n')[0]) || 0; } catch { /* not yet */ }
  if (!port) await sleep(200);
}
const tabs = await (await fetch(`http://127.0.0.1:${port}/json/list`)).json();
const ws = new WebSocket(tabs.find((t) => t.type === 'page').webSocketDebuggerUrl);
await new Promise((r) => { ws.onopen = r; });
let id = 0; const pending = new Map(); let onFrame = null;
ws.onmessage = (e) => {
  const m = JSON.parse(e.data);
  if (m.id && pending.has(m.id)) { pending.get(m.id)(m); pending.delete(m.id); }
  else if (m.method === 'Page.screencastFrame') { void send('Page.screencastFrameAck', { sessionId: m.params.sessionId }); onFrame?.(m.params); }
};
const send = (method, params = {}) => new Promise((r) => { const i = ++id; pending.set(i, r); ws.send(JSON.stringify({ id: i, method, params })); });
const evalJs = async (expr) => {
  const r = await Promise.race([send('Runtime.evaluate', { expression: expr, returnByValue: true, awaitPromise: true }), sleep(9000).then(() => null)]);
  return r?.result?.result?.value;
};
const waitFor = async (expr, ms = 10000) => { const end = Date.now() + ms; while (Date.now() < end) { if (await evalJs(expr)) return true; await sleep(150); } return false; };
const text = (sel) => evalJs(`document.querySelector(${JSON.stringify(sel)})?.innerText || ''`);
const click = (sel) => evalJs(`(() => { const e = document.querySelector(${JSON.stringify(sel)}); if (e) e.click(); return !!e; })()`);
const press = async (code, key = code, mods = 0) => {
  const vk = { Escape: 27, Enter: 13, Slash: 191 }[code] ?? (key.length === 1 ? key.toUpperCase().charCodeAt(0) : 0);
  await send('Input.dispatchKeyEvent', { type: 'rawKeyDown', key, code, windowsVirtualKeyCode: vk, modifiers: mods, text: key.length === 1 ? key : undefined });
  await send('Input.dispatchKeyEvent', { type: 'keyUp', key, code, windowsVirtualKeyCode: vk, modifiers: mods });
};
const hold = async (code, key, ms) => {
  await evalJs(`document.body.dispatchEvent(new KeyboardEvent('keydown', { code: ${JSON.stringify(code)}, key: ${JSON.stringify(key)}, bubbles: true, cancelable: true })); true`);
  await sleep(ms);
  await evalJs(`document.body.dispatchEvent(new KeyboardEvent('keyup', { code: ${JSON.stringify(code)}, key: ${JSON.stringify(key)}, bubbles: true })); true`);
};
const typeIn = (sel, v) => evalJs(`(() => { const el = document.querySelector(${JSON.stringify(sel)}); if (!el) return false; const d = Object.getOwnPropertyDescriptor(Object.getPrototypeOf(el), 'value'); d.set.call(el, ${JSON.stringify(v)}); el.dispatchEvent(new Event('input', { bubbles: true })); return true; })()`);
const menuPick = async (label) => {
  await evalJs(`(() => { if (!document.querySelector('.menupop')) document.querySelector('#menu-btn').click(); return true; })()`);
  await waitFor(`[...document.querySelectorAll('.menupop button')].some((x) => x.innerText.includes(${JSON.stringify(label)}))`, 3000);
  return evalJs(`(() => { const it = [...document.querySelectorAll('.menupop button')].find((x) => x.innerText.includes(${JSON.stringify(label)})); if (it) it.click(); return !!it; })()`);
};
const state = () => evalJs('plannerDebug()');
const reset = async () => { for (let i = 0; i < 5; i++) { await press('Escape'); await sleep(120); } };
// Panels the picture does not need: N and F toggle the navigator and the focus card.
const panels = async (nav, focus) => {
  const s = await state();
  if (!!s?.panels?.nav !== nav) await press('KeyN', 'n');
  if (!!s?.panels?.focus !== focus) await press('KeyF', 'f');
  await sleep(500);
};
const closeHint = () => evalJs('document.querySelector("#hint .x, #hint button")?.click(); true');

const stills = [];
const shot = async (name) => {
  const r = await send('Page.captureScreenshot', { format: 'jpeg', quality: 86 });
  writeFileSync(join(out, name), Buffer.from(r.result.data, 'base64'));
  stills.push(name);
  console.log('shot', name);
};
const goTask = async (taskId) => {
  await evalJs(`planner.navigateTo({kind:'planet',id:${JSON.stringify(taskId)}}); true`);
  await waitFor('world.debug().flight === "idle"', 15000);
  await sleep(1200);
};

await send('Page.enable');
await send('Runtime.enable');
await send('Emulation.setFocusEmulationEnabled', { enabled: true });
await send('Page.bringToFront');
await send('Emulation.setDeviceMetricsOverride', { width: 1440, height: 900, deviceScaleFactor: 1, mobile: false });
const open = async (query = '?noauto&nocoach') => {
  await send('Page.navigate', { url: base + '/' + query });
  await sleep(2500);
  await waitFor('window.world && world.debug().planets > 0', 30000);
  await sleep(1200);
};

// The flight as a GIF: the browser's screencast frames, cut to the moving part, then ffmpeg (palette, 10 frames a second).
const record = async (name, action, width = 640) => {
  const frames = [];
  onFrame = (f) => frames.push({ t: f.metadata.timestamp, data: f.data });
  await send('Page.startScreencast', { format: 'jpeg', quality: 80, maxWidth: width, everyNthFrame: 1 });
  await sleep(400);
  await action();
  await sleep(600);
  await send('Page.stopScreencast');
  onFrame = null;
  if (frames.length < 5) { console.log(`gif ${name}: only ${frames.length} frames, skipped`); return; }
  const dir = mkdtempSync(join(work, 'frames-'));
  const list = [];
  frames.forEach((f, i) => {
    const file = join(dir, `f${String(i).padStart(4, '0')}.jpg`);
    writeFileSync(file, Buffer.from(f.data, 'base64'));
    const next = frames[i + 1]?.t ?? f.t + 0.4;
    list.push(`file '${file}'`, `duration ${Math.max(0.02, next - f.t).toFixed(3)}`);
  });
  list.push(`file '${join(dir, `f${String(frames.length - 1).padStart(4, '0')}.jpg`)}'`);
  writeFileSync(join(dir, 'list.txt'), list.join('\n'));
  execFileSync('ffmpeg', ['-v', 'error', '-y', '-f', 'concat', '-safe', '0', '-i', join(dir, 'list.txt'),
    '-vf', `fps=10,scale=${width}:-1:flags=lanczos,split[a][b];[a]palettegen=max_colors=64:stats_mode=diff[p];[b][p]paletteuse=dither=bayer:bayer_scale=4`,
    '-loop', '0', join(out, name)]);
  console.log(`gif ${name}: ${frames.length} frames`);
};

// ── the pictures ─────────────────────────────────────────────────────────

await open();
await reset();
await closeHint();

// 1. the Galaxy from outside: the Quasar, nebulae, the ship; no panels
await evalJs('planner.goOverview(); true');
await panels(false, false);
await sleep(1500);
await shot('galaxy.jpg');

// 2. inside a nebula: a planet, its sub-agents, the cabin
await panels(true, true);
await goTask('alpha/6');
await shot('cabin.jpg');

// 3. the Coordinator's chat
await reset();
await evalJs('planner.goOverview(); true');
await panels(false, false);
await press('Slash', '/');
await waitFor('document.querySelectorAll("#clog .turn:not(.pending)").length === 3', 6000);
await sleep(1500);
await shot('chat.jpg');
await reset();

// 4. manual flight
await evalJs('planner.goOverview(); true');
await panels(false, false);
await press('KeyP', 'p');
await waitFor('world.mode === "manual" && world.debug().flight === "idle"', 15000);
await hold('KeyW', 'w', 1200);
await hold('KeyD', 'd', 600);
await sleep(500);
await shot('manual.jpg');
await press('KeyP', 'p');
await waitFor('world.mode === "auto"', 3000);
await reset();

// 5. the station: the crew of the ship
await menuPick('Станция');
await waitFor('!!document.querySelector("[data-role=\\"plan\\"] [data-value=\\"plan.agent\\"]")', 10000);
await sleep(800);
await shot('station.jpg');
await reset();

// 6. the ship's computer
await menuPick('Бортовой компьютер');
await waitFor('!!document.querySelector("#ship-dir")', 6000);
await sleep(600);
await shot('ship.jpg');
await reset();

// 7. the flight to a planet (GIF): the autopilot from the overview into a nebula
await evalJs('planner.goOverview(); true');
await panels(false, false);
await sleep(1500);
await record('flight.gif', async () => {
  await evalJs(`planner.navigateTo({kind:'planet',id:'alpha/6'}); true`);
  await waitFor('world.debug().flight === "idle"', 20000);
  await sleep(500);
});

// 8. the login page of a planner with a password
await send('Page.navigate', { url: `http://127.0.0.1:${loginPort}/login` });
await sleep(2000);
await shot('login.jpg');

console.log(`readme-shots: ${stills.length} stills in ${out}`);
ws.close();
process.exit(0);
