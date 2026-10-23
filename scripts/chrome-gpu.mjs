// Which GL the checks' headless Chromium draws with (PLN-58). On the shipyard the task's container is started with
// `--gpus all` (a yard labelled «gpu» whose driver answers, see scripts/yard.sh), and Chrome then reaches the card with
// no display: ANGLE on Vulkan, or on EGL. Everywhere else — GitHub CI, a laptop — the software renderer draws, as it
// always did, and the flags are the ones the checks were seen green with.
//
// UI_GPU forces the answer: `vulkan`, `egl`, `gl` or `swiftshader` (and `0`/`off` for the last one, `1`/`on` for the
// card). `scripts/ui-check.mjs` prints the renderer it got, so a silent fall back to SwiftShader is seen in the output.

import { execFileSync } from 'node:child_process';
import { existsSync } from 'node:fs';

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

/** A card the browser can use: the NVIDIA driver's device node, or `nvidia-smi` answering. */
export function hasGpu() {
  if (existsSync('/dev/nvidia0')) return true;
  try {
    execFileSync('nvidia-smi', ['-L'], { stdio: 'ignore', timeout: 5000 });
    return true;
  } catch {
    return false;
  }
}

/** The mode: UI_GPU's word when it has one, else the card's (Vulkan) or the software renderer. */
export function chooseMode(env = process.env, gpu = hasGpu()) {
  const forced = String(env.UI_GPU || '').trim().toLowerCase();
  if (forced === '1' || forced === 'on' || forced === 'auto' || forced === 'gpu') return 'vulkan';
  if (forced === '0' || forced === 'off' || forced === 'no' || forced === 'software') return SOFTWARE_MODE;
  if (MODE_FLAGS[forced]) return forced;
  if (forced) console.error(`chrome-gpu: неизвестный UI_GPU=${forced}; беру ${gpu ? 'vulkan' : SOFTWARE_MODE}`);
  return gpu ? 'vulkan' : SOFTWARE_MODE;
}

/** The flags of one mode; an unknown mode gets the software renderer's. */
export function gpuFlags(mode) {
  return [...(MODE_FLAGS[mode] || MODE_FLAGS[SOFTWARE_MODE])];
}
