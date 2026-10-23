// Which GL the checks' headless Chromium draws with (PLN-58). On the shipyard the task's container is started with
// `--gpus all` (a yard labelled «gpu» whose driver answers, see scripts/yard.sh), and Chrome then reaches the card with
// no display: ANGLE on Vulkan, or on EGL. Everywhere else — GitHub CI, a laptop — the software renderer draws, as it
// always did, and the flags are the ones the checks were seen green with.
//
// A card that answers `nvidia-smi` is not yet a card that draws: a container that has outlived a driver reload keeps
// stale device bindings, and Chrome then makes no WebGL context at all. So the hardware mode is tried once with a
// throwaway browser, and a mode that cannot draw falls back to SwiftShader with a line saying why — a full check is
// never lost to a card that stopped working.
//
// UI_GPU forces the answer: `vulkan`, `egl`, `gl` or `swiftshader` (and `0`/`off` for the last one, `1`/`on` for the
// card). `scripts/ui-check.mjs` prints the renderer it got, so a silent fall back to SwiftShader is seen in the output.

import { execFileSync, spawnSync } from 'node:child_process';
import { existsSync, mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

/** The software renderer of the headless browser: no card, every frame on the CPU (CI's flags, unchanged). */
export const SOFTWARE_MODE = 'swiftshader';

/** Each mode's flags, the whole of the difference: Chrome's headless mode needs no X either way. */
export const MODE_FLAGS = {
  swiftshader: ['--disable-gpu', '--use-gl=angle', '--use-angle=swiftshader', '--enable-unsafe-swiftshader'],
  // ANGLE over Vulkan: the card's driver and its ICD (the image carries the entry, see scripts/yard/Dockerfile).
  vulkan: ['--use-angle=vulkan', '--enable-features=Vulkan', '--ignore-gpu-blocklist'],
  // ANGLE over EGL: the driver's EGL (EGL_NVIDIA), or a Mesa one where there is no NVIDIA card.
  egl: ['--use-gl=angle', '--use-angle=gl-egl', '--ignore-gpu-blocklist'],
  // ANGLE over desktop GL (a Mesa llvmpipe included): the last way to the card, and the most forgiving.
  gl: ['--use-gl=angle', '--use-angle=gl', '--ignore-gpu-blocklist'],
};

/** A card the browser might use: the driver's device node, or `nvidia-smi` answering. */
export function hasGpu() {
  if (existsSync('/dev/nvidia0')) return true;
  try {
    execFileSync('nvidia-smi', ['-L'], { stdio: 'ignore', timeout: 5000 });
    return true;
  } catch {
    return false;
  }
}

/** One throwaway headless Chrome: does a WebGL context come up with these flags? A card that cannot draw is no card. */
export function gpuWorks(chromium, flags, timeout = 20000) {
  let dir = null;
  try {
    dir = mkdtempSync(join(tmpdir(), 'chrome-gpu-'));
    const page = 'data:text/html,<body><script>const c=document.createElement("canvas");'
      + 'const gl=c.getContext("webgl2")||c.getContext("webgl");'
      + 'document.body.textContent=gl?"webgl":"no webgl";</script></body>';
    const out = spawnSync(chromium, ['--headless=new', '--no-sandbox', ...flags, `--user-data-dir=${dir}`, '--dump-dom', page],
      { encoding: 'utf8', timeout });
    return out.status === 0 && !/no webgl/i.test(out.stdout || '') && /webgl/i.test(out.stdout || '');
  } catch {
    return false;
  } finally {
    if (dir) {
      try { rmSync(dir, { recursive: true, force: true, maxRetries: 3 }); } catch { /* best effort */ }
    }
  }
}

/** The mode: UI_GPU's word when it has one, else the card's (Vulkan, once it proves it draws) or the software one. */
export function chooseMode(env = process.env, gpu = hasGpu(), works = null) {
  const can = works ?? ((flags) => gpuWorks(env.CHROMIUM || 'chromium', flags));
  const forced = String(env.UI_GPU || '').trim().toLowerCase();
  if (forced === '0' || forced === 'off' || forced === 'no' || forced === 'software') return SOFTWARE_MODE;
  if (forced === '1' || forced === 'on' || forced === 'auto' || forced === 'gpu') {
    if (can(MODE_FLAGS.vulkan)) return 'vulkan';
    console.error(`chrome-gpu: карта не даёт WebGL; беру ${SOFTWARE_MODE}`);
    return SOFTWARE_MODE;
  }
  if (MODE_FLAGS[forced]) {
    if (!can(MODE_FLAGS[forced])) {
      console.error(`chrome-gpu: UI_GPU=${forced} не даёт WebGL; беру ${SOFTWARE_MODE}`);
      return SOFTWARE_MODE;
    }
    return forced;
  }
  if (forced) {
    console.error(`chrome-gpu: неизвестный UI_GPU=${forced}; беру ${gpu ? 'vulkan' : SOFTWARE_MODE}`);
  }
  if (!gpu) return SOFTWARE_MODE;
  if (can(MODE_FLAGS.vulkan)) return 'vulkan';
  console.error(`chrome-gpu: карта отвечает, но WebGL на ней не поднялся; беру ${SOFTWARE_MODE}`);
  return SOFTWARE_MODE;
}

/** The flags of one mode; an unknown mode gets the software renderer's. */
export function gpuFlags(mode) {
  return [...(MODE_FLAGS[mode] || MODE_FLAGS[SOFTWARE_MODE])];
}
