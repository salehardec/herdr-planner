#!/usr/bin/env node
// Smoke check of the page in headless Chromium over the DevTools protocol.
// No dependencies (Node 22+ has WebSocket). Usage, with the server running:
//   node scripts/ui-check.mjs [http://127.0.0.1:7117] [screenshot-dir]
// Exits 1 when a check fails. Needs `chromium` (or $CHROMIUM) on the PATH.

import { spawn } from 'node:child_process';
import { mkdtempSync, writeFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

const base = process.argv[2] || 'http://127.0.0.1:7117';
const shots = process.argv[3] || null;
const port = 9333 + Math.floor(Math.random() * 500);
const profile = mkdtempSync(join(tmpdir(), 'herdr-planner-ui-'));
const chrome = spawn(process.env.CHROMIUM || 'chromium', [
  '--headless=new', '--no-sandbox', '--disable-gpu', '--hide-scrollbars', `--user-data-dir=${profile}`,
  `--remote-debugging-port=${port}`, '--window-size=1180,820', 'about:blank',
], { stdio: 'ignore' });

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
let failed = 0;
function check(name, ok, extra = '') {
  console.log(`${ok ? 'ok  ' : 'FAIL'} ${name}${extra ? ' — ' + extra : ''}`);
  if (!ok) failed++;
}

async function waitForChrome() {
  for (let i = 0; i < 50; i++) {
    try {
      const r = await fetch(`http://127.0.0.1:${port}/json/version`);
      if (r.ok) return;
    } catch {}
    await sleep(200);
  }
  throw new Error('chromium did not start');
}

async function main() {
  await waitForChrome();
  let target = await (await fetch(`http://127.0.0.1:${port}/json/new?${base}/`, { method: 'PUT' })).json().catch(() => null);
  if (!target) target = await (await fetch(`http://127.0.0.1:${port}/json/new?${base}/`)).json();
  const ws = new WebSocket(target.webSocketDebuggerUrl);
  await new Promise((r, e) => { ws.onopen = r; ws.onerror = e; });
  let id = 0;
  const pending = new Map();
  ws.onmessage = (m) => { const d = JSON.parse(m.data); if (d.id && pending.has(d.id)) { pending.get(d.id)(d); pending.delete(d.id); } };
  const send = (method, params = {}) => new Promise((r) => { const i = ++id; pending.set(i, r); ws.send(JSON.stringify({ id: i, method, params })); });
  const evalJs = async (expr) => (await send('Runtime.evaluate', { expression: expr, returnByValue: true, awaitPromise: true })).result?.result?.value;
  const key = async (k, text) => {
    const code = { ArrowDown: 40, ArrowUp: 38, Enter: 13, Escape: 27, ArrowLeft: 37, ArrowRight: 39 }[k];
    // Shortcuts are handled on keydown; a `char` event would type the letter even
    // where the page cancels the default, so text is inserted only when asked.
    const common = code ? { key: k, code: k, windowsVirtualKeyCode: code, nativeVirtualKeyCode: code } : { key: k };
    await send('Input.dispatchKeyEvent', { type: code ? 'rawKeyDown' : 'keyDown', ...common });
    if (text) await send('Input.dispatchKeyEvent', { type: 'char', text, key: k });
    await send('Input.dispatchKeyEvent', { type: 'keyUp', ...common });
    await sleep(120);
  };
  const shot = async (name) => {
    if (!shots) return;
    const r = await send('Page.captureScreenshot', { format: 'png' });
    writeFileSync(join(shots, name), Buffer.from(r.result.data, 'base64'));
  };
  await send('Page.enable');
  await send('Runtime.enable');
  // Wait for the first snapshot to arrive over SSE.
  for (let i = 0; i < 50 && !(await evalJs('document.querySelectorAll(".row[data-row]:not(.newrow), .card").length > 0')); i++) await sleep(200);
  // The page follows the focused herdr workspace and may open a project at once.
  if (await evalJs('!!document.querySelector(".card")')) { console.log('note: opened on the active workspace\'s project'); await key('Escape'); await key('Escape'); }

  const rows = await evalJs('document.querySelectorAll(".row[data-row]").length');
  check('list has project rows', rows > 1, `${rows} rows`);
  const title = await evalJs('document.querySelector(".header .title")?.textContent');
  check('header names the planner and the count', /Планировщик проектов · \d+ проект/.test(title || ''), title);
  const overflow = await evalJs('document.scrollingElement.scrollWidth - window.innerWidth');
  check('page does not scroll horizontally', overflow <= 0, `overflow ${overflow}px`);
  const stamp = await evalJs('(() => { const r = document.querySelector(".stamp").getBoundingClientRect(); return [Math.round(r.right), window.innerWidth]; })()');
  check('refresh stamp fits the viewport', stamp && stamp[0] <= stamp[1], JSON.stringify(stamp));
  const cut = await evalJs('(() => { const e = document.querySelector(".stamp"); return [e.scrollWidth, e.clientWidth, e.textContent]; })()');
  check('refresh stamp text is not cut', cut && cut[0] <= cut[1] + 1, JSON.stringify(cut));
  const vars = await evalJs('getComputedStyle(document.documentElement).getPropertyValue("--bg").trim()');
  check('theme variables applied', /^#[0-9a-f]{6}$/i.test(vars) && vars !== '#101419', vars);
  const font = await evalJs('getComputedStyle(document.body).fontFamily');
  check('ui font from herdr-gpui config', !!font, font);
  await shot('list.png');

  await key('ArrowDown');
  await key('ArrowDown');
  const hl = await evalJs('document.querySelector(".row.hl")?.getAttribute("data-row") || document.querySelector(".row.hl")?.getAttribute("data-group")');
  check('arrow keys highlight a row', !!hl, hl);
  const hint = await evalJs('document.querySelector(".row.hl .hint")?.textContent');
  check('highlighted row shows its keys', !!hint, hint);
  await shot('highlight.png');

  await key('t');
  const picker = await evalJs('document.querySelector(".dialog h2")?.textContent');
  check('T opens the group picker', /Группа для/.test(picker || ''), picker);
  await key('Escape');
  check('Escape closes the picker', !(await evalJs('!!document.querySelector(".dialog")')));

  await key('2');
  const chip = await evalJs('document.querySelector(".badge.active")?.textContent');
  check('digit keys switch chips', /Ревью/.test(chip || ''), chip);
  await key('s');
  const sort = await evalJs('document.querySelector("[data-act=sort]")?.textContent');
  check('S toggles the sort', /внимание/.test(sort || ''), sort);
  await key('Escape');
  check('Escape resets the chip', /Все/.test(await evalJs('document.querySelector(".badge.active")?.textContent') || ''));

  await key('/');
  check('/ focuses the filter', await evalJs('document.activeElement?.id === "filter"'));
  await send('Input.insertText', { text: 'omarchy' });
  await sleep(150);
  if (process.env.DEBUG) console.log('debug', await evalJs('JSON.stringify({v: document.querySelector("#filter")?.value, active: document.activeElement?.id, note: document.querySelector(".note")?.textContent, rows: document.querySelectorAll(".row").length})'));
  const shown = await evalJs('document.querySelectorAll(".row[data-row]:not(.newrow)").length');
  check('typing filters the list', shown > 0 && shown < rows, `${shown} of ${rows}`);
  await key('ArrowDown');
  check('↓ leaves the filter and keeps the text', await evalJs('document.activeElement?.id !== "filter" && document.querySelector("#filter").value === "omarchy"'));
  await key('Enter');
  const detail = await evalJs('document.querySelector(".card")?.textContent');
  check('Enter opens the project card stub', /этапе 2/.test(detail || ''), (detail || '').slice(0, 60));
  await sleep(1500);
  await shot('detail.png');
  await key('Escape');
  check('Escape returns to the list with the project highlighted', await evalJs('!!document.querySelector(".row.hl[data-row]")'));

  ws.close();
}

main().catch((e) => { console.error('ERROR', e); failed++; }).finally(() => {
  chrome.kill();
  rmSync(profile, { recursive: true, force: true });
  process.exit(failed ? 1 : 0);
});
