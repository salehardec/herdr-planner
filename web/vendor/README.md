# three.js (vendored)

- Version: **0.186.1** (2026-09-24), MIT — see `LICENSE-three.txt`.
- Upstream: `npm pack three@0.186.1`, files from the package:
  - `build/three.module.js` → `three.module.js`
  - `build/three.core.js` → `three.core.js` (`three.module.js` imports it)
  - `examples/jsm/controls/OrbitControls.js` → `OrbitControls.js`
  - `examples/jsm/renderers/CSS2DRenderer.js` → `CSS2DRenderer.js`
- Changes: only the bare `from 'three'` in the two `examples/jsm` files is
  rewritten to `from './three.module.js'`, because the page has no import map
  (CSP `script-src 'self'` allows inline-free modules only). Nothing else is
  edited. 0.186.1 ships no minified `three.module.min.js`; the unminified build
  is served with a one-year cache.
- Update: `npm pack three@<version>`, copy those five files (including the
  license), apply the same one-line rewrite, then change `VENDOR` in
  `src/server.rs` and the `/vendor/<version>/…` links in `web/index.html`.
