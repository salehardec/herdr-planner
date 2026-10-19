/* Мир доски: Бог в центре, вселенные проектов, планеты задач, связи after.
   three.js вшит локально; CSS2D — подписи и чат; DOM — панели приложения. */

import * as THREE from './vendor/0.186.1/three.module.js';
import { CSS2DObject, CSS2DRenderer } from './vendor/0.186.1/CSS2DRenderer.js';
import { createCamera } from './camera.js';
import { computeLayout, planetPosition, hash } from './layout.js';
import { firstSentence } from './requests.js';

const esc = (s) => String(s ?? '').replace(/[&<>"]/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;' }[c]));
const clamp = (v, a, b) => Math.max(a, Math.min(b, v));
const STATUS_COLOR = {
  waiting: 'yellow', failed: 'red', ready: 'green', plan: 'cyan',
  review: 'magenta', working: 'blue', queued: 'muted', done: 'muted',
};
const AGENT_COLOR = { claude: 'accent', omp: 'blue', pi: 'magenta', codex: 'green' };
const AGENT_SHAPE = { claude: 'sphere', omp: 'box', pi: 'octa', codex: 'tetra' };
const STATUSES = ['waiting', 'failed', 'ready', 'plan', 'review', 'working', 'queued', 'done'];
const DEFAULT_PALETTE = {
  bg: '#1a120c', 'bg-darker': '#0d0906', fg: '#ecded6', 'fg-bright': '#fcf5f1', muted: '#806f64',
  accent: '#dfa360', red: '#e48682', yellow: '#f6bb6f', green: '#98bf7a', cyan: '#5bcbcb',
  blue: '#77b0e9', magenta: '#da8fbf',
};

export function createWorld({ canvas, labelsEl, simple = false, reduced = false }) {
  const world = {
    ok: false, webgl: false, simple: !!simple,
    onSelect: () => {}, onUniverse: () => {}, onGod: () => {}, onEmpty: () => {},
    onLink: () => {}, onConnect: () => {}, onAutoSimple: () => {},
  };
  let renderer = null;
  const supported = (() => {
    try {
      const c = document.createElement('canvas');
      return !!(c.getContext('webgl2') || c.getContext('webgl'));
    } catch { return false; }
  })();
  if (supported) {
    try {
      renderer = new THREE.WebGLRenderer({ canvas, antialias: !simple, powerPreference: 'low-power' });
      if (!renderer.getContext()) throw new Error('no webgl context');
      world.webgl = true;
    } catch (e) {
      renderer = null;
    }
  }
  world.ok = world.webgl;

  let scene = null;
  let labelRenderer = null;
  let cam = null;
  let data = { tasks: [], projects: [], links: [], selected: null, filter: null, fresh: {}, attention: 0, coordinator: {}, showQuiet: true, limits: null };
  let layout = { universes: [], positions: new Map(), god: { x: 0, y: 0, z: 0 } };
  let palette = {};
  let mode = 'dark';
  let paletteKey = '';
  let current = { kind: 'overview', id: null, universe: null };

  const groups = {};
  const shared = {};
  const statusMat = {};
  const atmMat = {};
  const agentMat = {};
  const themeMats = [];
  const colorTweens = [];
  let planets = [];
  let planetPickables = [];
  let linkPickables = [];
  let sunPickables = [];
  let godMesh = null;
  let selRing = null;
  let godLight = null;
  let godHalo = null;
  let haloTexture = null;
  let stars = null;
  let godLabel = null;
  let chatObject = null;
  let chatAnchor = new THREE.Vector3(104, -26, 40);
  let todoObject = null;
  let todoEl = null;
  let todoScaleEl = null;
  const todoAnchor = new THREE.Vector3(-104, -26, 40);
  let dropTarget = null;
  let todoShown = true;
  let labels = new Map();
  let links = [];
  let linkMats = [];
  let farStars = new Map();
  let rings = [];
  let satellites = [];
  let beacons = [];
  let beamMats = [];
  let beamGroup = null;
  let limitsGroup = null;
  let beamSparks = [];
  let fx = [];
  let burstSeen = new Set();
  let hovered = null;
  let linkMode = false;
  let drag = null;
  let tempLine = null;
  let dirty = true;
  let raf = 0;
  let rendering = false;
  let lastFrameAt = 0;
  let lastFps = 0;
  let frames = 0;
  let fpsStart = 0;
  let frameTimes = [];
  let autoChecked = false;
  let hidden = !!document.hidden;
  let time = 0;

  const raycaster = new THREE.Raycaster();
  const pointer = new THREE.Vector2();
  const plane = new THREE.Plane(new THREE.Vector3(0, 1, 0), 0);
  const clockStart = performance.now();

  function makeMaterials() {
    shared.sphere = new THREE.SphereGeometry(1, 18, 14);
    shared.sphereLow = new THREE.SphereGeometry(1, 10, 8);
    shared.box = new THREE.BoxGeometry(1.2, 1.2, 1.2);
    shared.octa = new THREE.OctahedronGeometry(0.85);
    shared.tetra = new THREE.TetrahedronGeometry(0.95);
    for (const s of STATUSES) {
      statusMat[s] = new THREE.MeshStandardMaterial({
        color: 0x888888, emissive: 0x222222, emissiveIntensity: 0.6,
        roughness: 0.45, metalness: 0.12, transparent: true, opacity: 1,
      });
      atmMat[s] = new THREE.MeshBasicMaterial({
        color: 0x888888, transparent: true, opacity: 0.16, blending: THREE.AdditiveBlending,
        depthWrite: false, side: THREE.BackSide,
      });
    }
    shared.orbitMat = new THREE.LineBasicMaterial({ color: 0x555555, transparent: true, opacity: 0.32 });
    shared.beamMat = new THREE.LineBasicMaterial({ color: 0x666666, transparent: true, opacity: 0.08 });
    shared.selMat = new THREE.MeshBasicMaterial({ color: 0xffffff, transparent: true, opacity: 0.9, side: THREE.DoubleSide });
    shared.sparkGeo = new THREE.SphereGeometry(0.55, 8, 6);
  }

  function init() {
    if (!world.webgl) return;
    scene = new THREE.Scene();
    labelRenderer = new CSS2DRenderer({ element: labelsEl });
    labelRenderer.sortObjects = false;
    cam = createCamera({
      canvas, simple: world.simple,
      onChange: () => { dirty = true; requestRender(); },
    });
    renderer.setPixelRatio(world.simple ? 1 : Math.min(devicePixelRatio || 1, 2));
    renderer.setSize(innerWidth, innerHeight, false);
    labelRenderer.setSize(innerWidth, innerHeight);
    makeMaterials();

    groups.root = new THREE.Group();
    groups.god = new THREE.Group();
    groups.universes = new THREE.Group();
    groups.links = new THREE.Group();
    groups.far = new THREE.Group();
    groups.fx = new THREE.Group();
    beamGroup = new THREE.Group();
    limitsGroup = new THREE.Group(); // PLN-5: subscription arcs around God
    scene.add(groups.root);
    groups.root.add(groups.god, groups.universes, groups.links, groups.far, groups.fx, beamGroup);

    scene.add(new THREE.AmbientLight(0xffffff, 0.5));
    godLight = new THREE.PointLight(0xffffff, 2.2, 1100, 1.4);
    scene.add(godLight);

    selRing = new THREE.Mesh(new THREE.TorusGeometry(1, 0.08, 6, 40), shared.selMat);
    selRing.visible = false;
    scene.add(selRing);

    makeGod();
    makeStars();
    makeChat();
    makeTodo();
    setPalette(DEFAULT_PALETTE, 'dark', true);
    window.addEventListener('resize', onResize);
    document.addEventListener('visibilitychange', onVisibility);
    bindPointer();
    if (world.simple) requestRender(); else startLoop();
    renderFrame(performance.now());
  }

  function makeGod() {
    godMesh = new THREE.Mesh(shared.sphere, new THREE.MeshStandardMaterial({
      color: 0xffffff, emissive: 0xffffff, emissiveIntensity: 1.6, roughness: 0.3, metalness: 0.1,
    }));
    godMesh.scale.setScalar(13);
    godMesh.userData.god = true;
    groups.god.add(godMesh);

    haloTexture = new THREE.CanvasTexture(haloCanvas());
    godHalo = new THREE.Sprite(new THREE.SpriteMaterial({
      map: haloTexture, transparent: true, depthWrite: false, blending: THREE.AdditiveBlending, opacity: 0.85,
    }));
    godHalo.scale.setScalar(120);
    groups.god.add(godHalo);

    groups.god.add(limitsGroup);
    godLabel = document.createElement('div');
    godLabel.className = 'lbl3d god';
    godLabel.innerHTML = `<div class="god-in"><b class="num">ждут тебя: 0</b></div>`;
    const obj = new CSS2DObject(godLabel);
    obj.position.set(0, 34, 0);
    obj.renderOrder = 100;
    godLabel.style.zIndex = '9000';
    groups.god.add(obj);
  }

  function haloCanvas() {
    const c = document.createElement('canvas');
    c.width = c.height = 128;
    const g = c.getContext('2d');
    const col = palette.accent || '#dfa360';
    const grad = g.createRadialGradient(64, 64, 2, 64, 64, 62);
    grad.addColorStop(0, hexA(col, 0.9));
    grad.addColorStop(0.25, hexA(col, 0.4));
    grad.addColorStop(1, hexA(col, 0));
    g.fillStyle = grad;
    g.fillRect(0, 0, 128, 128);
    return c;
  }

  function hexA(hex, a) {
    const h = String(hex).replace('#', '');
    const n = parseInt(h.length === 3 ? h.split('').map((x) => x + x).join('') : h.slice(0, 6), 16);
    if (!Number.isFinite(n)) return `rgba(223,163,96,${a})`;
    return `rgba(${(n >> 16) & 255},${(n >> 8) & 255},${n & 255},${a})`;
  }

  function makeStars() {
    const n = world.simple ? 150 : 620;
    const pos = new Float32Array(n * 3);
    for (let i = 0; i < n; i++) {
      const a = hash('star' + i) / 4294967295 * Math.PI * 2;
      const r = 900 + (hash('r' + i) % 500);
      const y = ((hash('y' + i) % 2000) / 1000 - 1) * 0.65;
      pos[i * 3] = Math.cos(a) * r * Math.sqrt(1 - y * y);
      pos[i * 3 + 1] = y * r;
      pos[i * 3 + 2] = Math.sin(a) * r * Math.sqrt(1 - y * y);
    }
    const geo = new THREE.BufferGeometry();
    geo.setAttribute('position', new THREE.BufferAttribute(pos, 3));
    stars = new THREE.Points(geo, new THREE.PointsMaterial({
      color: 0xffffff, size: 2.6, sizeAttenuation: true, transparent: true, opacity: 0.8, depthWrite: false,
    }));
    scene.add(stars);
  }

  let chatEl = null;
  let chatScaleEl = null;
  function makeChat() {
    chatEl = document.getElementById('console');
    if (!chatEl) return;
    const wrap = document.createElement('div');
    wrap.className = 'chat3d';
    const scale = document.createElement('div');
    scale.className = 'chat-scale';
    scale.appendChild(chatEl);
    wrap.appendChild(scale);
    chatScaleEl = scale;
    chatObject = new CSS2DObject(wrap);
    chatObject.position.copy(chatAnchor);
    chatObject.renderOrder = 200;
    wrap.style.zIndex = '9500';
    groups.god.add(chatObject);
  }

  function makeTodo() {
    todoEl = document.getElementById('todo');
    if (!todoEl) return;
    const wrap = document.createElement('div');
    wrap.className = 'todo3d';
    const scale = document.createElement('div');
    scale.className = 'todo-scale';
    scale.appendChild(todoEl);
    wrap.appendChild(scale);
    todoScaleEl = scale;
    todoObject = new CSS2DObject(wrap);
    todoObject.position.copy(todoAnchor);
    todoObject.renderOrder = 190;
    wrap.style.zIndex = '9400';
    groups.god.add(todoObject);
  }

  function detachTodo() {
    if (!todoObject || !todoEl) return;
    todoObject.visible = false;
    const dock = document.getElementById('todo-dock');
    if (dock && todoEl.parentElement !== dock) dock.appendChild(todoEl);
  }

  function attachTodo() {
    if (!todoObject || !todoEl || !todoScaleEl) return;
    if (todoEl.parentElement !== todoScaleEl) todoScaleEl.appendChild(todoEl);
    todoObject.visible = todoShown;
  }

  function showTodo(v) {
    todoShown = !!v;
    if (todoObject) todoObject.visible = todoShown;
    requestRender();
  }

  /** Which universe is under the pointer: over its sun, or on its disk. */
  function universeAt(x, y) {
    if (!cam) return null;
    toNDC({ clientX: x, clientY: y });
    const sun = raycaster.intersectObjects(sunPickables, false)[0];
    if (sun) return sun.object.userData.universe;
    const hit = new THREE.Vector3();
    if (!raycaster.ray.intersectPlane(plane, hit)) return null;
    let best = null;
    let bestD = Infinity;
    for (const u of layout.universes) {
      const d = Math.hypot(hit.x - u.center.x, hit.z - u.center.z);
      if (d <= u.radius + 16 && d < bestD) { best = u; bestD = d; }
    }
    return best ? best.name : null;
  }

  function setDropTarget(name) {
    dropTarget = name || null;
    for (const el of labelsEl.querySelectorAll('.lbl3d.uni')) {
      el.classList.toggle('drop', el.dataset.universe === dropTarget);
    }
    canvas.classList.toggle('drop-target', !!dropTarget);
    requestRender();
  }

  function dropFlash(name) {
    const u = layout.universes.find((x) => x.name === name);
    if (!u || world.simple) return;
    const geo = new THREE.RingGeometry(u.radius * 0.75, u.radius * 0.92, 44);
    const mesh = new THREE.Mesh(geo, new THREE.MeshBasicMaterial({
      color: palette.green || palette.accent || '#fff', transparent: true, opacity: 0.9,
      side: THREE.DoubleSide, depthWrite: false,
    }));
    mesh.rotation.x = -Math.PI / 2;
    mesh.position.set(u.center.x, 0.6, u.center.z);
    mesh.userData.ownGeo = true;
    mesh.userData.ownMat = true;
    groups.fx.add(mesh);
    fx.push({ mesh, t0: performance.now(), dur: 1200 });
    requestRender();
  }

  /** On a narrow screen the chat leaves the world and docks at the bottom. */
  function detachChat() {
    if (!chatObject || !chatEl) return;
    chatObject.visible = false;
    const dock = document.getElementById('console-dock');
    if (dock && chatEl.parentElement !== dock) dock.appendChild(chatEl);
  }

  function attachChat() {
    if (!chatObject || !chatEl || !chatScaleEl) return;
    if (chatEl.parentElement !== chatScaleEl) chatScaleEl.appendChild(chatEl);
    chatObject.visible = true;
  }

  /* ── palette ── */

  function setPalette(colors, m = 'dark', force = false) {
    if (!colors || typeof colors !== 'object') return;
    const key = JSON.stringify([colors, m]);
    if (!force && key === paletteKey) return;
    paletteKey = key;
    const first = !palette.accent;
    palette = { ...colors };
    mode = m === 'light' ? 'light' : 'dark';
    if (!world.webgl) return;
    if (first) {
      for (const [k, v] of Object.entries(colors)) if (typeof v === 'string' && v.startsWith('#')) palette[k] = v;
    }
    const bg = mode === 'light' ? colors.bg : (colors['bg-darker'] || colors.bg);
    if (scene && bg) {
      if (first || world.simple) scene.background = new THREE.Color(bg);
      else tween(scene, 'background', bg);
    }
    if (godLight && colors.accent) godLight.color.set(colors.accent);
    if (godMesh) {
      const mat = godMesh.material;
      if (first || world.simple) { mat.color.set(colors.accent); mat.emissive.set(colors.accent); }
      else { tweenColor(mat, 'color', colors.accent); tweenColor(mat, 'emissive', colors.accent); }
    }
    for (const s of STATUSES) {
      const col = colors[STATUS_COLOR[s]] || colors.muted || '#888888';
      if (first || world.simple) {
        statusMat[s].color.set(col);
        statusMat[s].emissive.set(col);
        atmMat[s].color.set(col);
      } else {
        tweenColor(statusMat[s], 'color', col);
        tweenColor(statusMat[s], 'emissive', col);
        tweenColor(atmMat[s], 'color', col);
      }
      statusMat[s].emissiveIntensity = mode === 'light' ? 0.32 : 0.62;
      atmMat[s].opacity = mode === 'light' ? 0.1 : 0.17;
    }
    if (shared.orbitMat) shared.orbitMat.color.set(mode === 'light' ? colors.muted : colors.fg);
    if (shared.beamMat) shared.beamMat.color.set(colors.accent);
    if (shared.selMat) shared.selMat.color.set(colors.accent);
    for (const t of themeMats) if (colors[t.key]) t.mat.color.set(colors[t.key]);
    for (const m2 of linkMats) {
      if (m2.userData.kind === 'waiting') m2.color.set(colors.accent);
      else if (m2.userData.kind === 'far') m2.color.set(colors.muted);
      else m2.color.set(colors.muted);
    }
    if (godHalo) {
      if (haloTexture) haloTexture.dispose();
      haloTexture = new THREE.CanvasTexture(haloCanvas());
      godHalo.material.map = haloTexture;
      godHalo.material.needsUpdate = true;
    }
    if (stars) stars.material.color.set(mode === 'light' ? colors.fg : (colors['fg-bright'] || colors.fg));
    // Arcs and satellites own their materials, so a theme change recreates
    // them from the new palette.
    if (data.limits) setLimits(data.limits);
    if (chatObject) requestRender();
    dirty = true;
    requestRender();
  }

  function tween(obj, prop, hex) {
    const to = new THREE.Color(hex);
    const from = obj[prop] && obj[prop].isColor ? obj[prop].clone() : to.clone();
    colorTweens.push({ obj, prop, from, to, t0: performance.now(), dur: 350 });
  }

  function tweenColor(mat, prop, hex) {
    const to = new THREE.Color(hex);
    const from = mat[prop] && mat[prop].isColor ? mat[prop].clone() : to.clone();
    colorTweens.push({ obj: mat, prop, from, to, t0: performance.now(), dur: 350 });
  }

  function stepTweens(now) {
    for (let i = colorTweens.length - 1; i >= 0; i--) {
      const t = colorTweens[i];
      const k = clamp((now - t.t0) / t.dur, 0, 1);
      if (t.obj[t.prop] && t.obj[t.prop].isColor) t.obj[t.prop].copy(t.from).lerp(t.to, k);
      if (k >= 1) colorTweens.splice(i, 1);
    }
  }

  /* ── data ── */

  function setData(next) {
    const prevStatus = new Map(planets.map((p) => [p.id, p.s]));
    data = { ...data, ...next };
    layout = computeLayout({ tasks: data.tasks, projects: data.projects, showQuiet: data.showQuiet !== false });
    if (!world.webgl) {
      planets = data.tasks
        .filter((t) => t.s !== 'done')
        .map((t) => ({ id: t.id, s: t.s, tag: t.tag, title: t.title }));
      dirty = true;
      return;
    }
    rebuild(prevStatus);
    dirty = true;
    requestRender();
  }

  function clearGroup(g) {
    for (const c of [...g.children]) {
      c.traverse?.((o) => {
        if (o.isCSS2DObject && o.element) o.element.remove();
        if (o.userData.ownGeo) o.geometry?.dispose?.();
        if (o.userData.ownMat) {
          const mats = Array.isArray(o.material) ? o.material : o.material ? [o.material] : [];
          for (const m of mats) m.dispose?.();
        }
      });
      g.remove(c);
    }
  }

  function rebuild(prevStatus) {
    clearGroup(groups.universes);
    clearGroup(groups.links);
    clearGroup(groups.far);
    clearGroup(groups.fx);
    clearGroup(beamGroup);
    labels = new Map();
    planets = [];
    planetPickables = [];
    linkPickables = [];
    sunPickables = [];
    rings = [];
    satellites = [];
    beacons = [];
    beamMats = [];
    beamSparks = [];
    farStars = new Map();
    fx = [];
    links = [];
    linkMats = [];
    themeMats.length = 0;

    for (const u of layout.universes) buildUniverse(u);

    links = [];
    for (const l of (data.links || [])) buildLink(l);

    buildBeams();
    buildGodLabel();
    updateFilter();
    updateFresh(prevStatus);
    if (data.selected) select(data.selected, false);
    // Satellites were just recreated with their program colours; give the
    // agents with limits their arc colour back.
    if (data.limits) setLimits(data.limits);
  }

  function buildGodLabel() {
    if (!godLabel) return;
    const n = Number(data.attention) || 0;
    const busy = !!(data.coordinator && (data.coordinator.busy || data.coordinator.status === 'working'));
    const dialog = !!(data.coordinator && data.coordinator.dialog);
    const err = !!(data.coordinator && data.coordinator.error);
    godLabel.innerHTML = `<div class="god-in"><b class="num">${dialog ? 'диалог' : `ждут тебя: ${n}`}</b></div>`;
    const col = dialog ? palette.yellow : err ? palette.red : palette.accent;
    if (godHalo) {
      godHalo.material.color.set(col || '#ffffff');
      godHalo.userData.busy = busy;
      godHalo.userData.dialog = dialog;
      godHalo.userData.err = err;
    }
    if (godLight) godLight.color.set(col || '#ffffff');
    if (godMesh) { godMesh.material.emissive.set(err ? (palette.red || '#ffffff') : (dialog ? (palette.yellow || '#ffffff') : (palette.accent || '#ffffff'))); }
  }

  function buildUniverse(u) {
    const g = new THREE.Group();
    g.position.set(u.center.x, u.center.y, u.center.z);
    groups.universes.add(g);

    if (!u.quiet) {
      const sunColor = palette['fg-bright'] || '#ffffff';
      const sun = new THREE.Mesh(shared.sphere, new THREE.MeshStandardMaterial({
        color: sunColor, emissive: sunColor, emissiveIntensity: u.active ? 0.9 : 0.2,
        roughness: 0.4, metalness: 0.1,
      }));
      sun.scale.setScalar(u.quiet ? 1.6 : 3.2);
      sun.userData.universe = u.name;
      sun.userData.ownMat = true;
      sunPickables.push(sun);
      g.add(sun);
    } else {
      const neb = new THREE.Sprite(new THREE.SpriteMaterial({
        map: haloTexture, transparent: true, opacity: 0.18, depthWrite: false, blending: THREE.AdditiveBlending, color: palette.muted || '#806f64',
      }));
      neb.scale.setScalar(26);
      g.add(neb);
    }

    // Orbit rings of the universe.
    const orbits = [...new Set(u.planets.filter((p) => !p.belt).map((p) => p.orbit))];
    for (const orbit of orbits) {
      const r = 9 + orbit * 6.5;
      const pts = [];
      for (let i = 0; i <= 64; i++) {
        const a = (i / 64) * Math.PI * 2;
        pts.push(new THREE.Vector3(Math.cos(a) * r, 0, Math.sin(a) * r));
      }
      const geo = new THREE.BufferGeometry().setFromPoints(pts);
      const line = new THREE.LineLoop(geo, shared.orbitMat);
      line.userData.ownGeo = true;
      g.add(line);
      themeMats.push({ mat: shared.orbitMat, key: mode === 'light' ? 'muted' : 'fg' });
    }

    // Universe label.
    const el = document.createElement('div');
    el.className = `lbl3d uni${u.quiet ? ' dot' : ''}`;
    el.innerHTML = `<span class="in"><b class="k">${esc(u.key)}</b><span class="ag">${esc(u.name)}</span></span>`;
    el.dataset.universe = u.name;
    const lab = new CSS2DObject(el);
    lab.position.set(0, u.quiet ? 4 : 8, 0);
    lab.renderOrder = 20;
    el.style.zIndex = '500';
    g.add(lab);

    for (const p of u.planets) buildPlanet(u, p, groups.universes);
  }

  function buildPlanet(u, p, parent) {
    const g = new THREE.Group();
    parent.add(g);
    const mat = statusMat[p.s];
    const mesh = new THREE.Mesh(p.belt ? shared.sphereLow : shared.sphere, mat);
    mesh.scale.setScalar(p.size);
    mesh.userData.id = p.id;
    mesh.userData.own = false;
    planetPickables.push(mesh);
    g.add(mesh);

    const entry = { ...p, universe: u, group: g, mesh, sat: null, beacon: null };
    planets.push(entry);

    if (!p.belt && !world.simple && p.s !== 'queued' && p.s !== 'done') {
      const atm = new THREE.Mesh(shared.sphereLow, atmMat[p.s]);
      atm.scale.setScalar(p.size * 1.5);
      atm.userData.own = false;
      g.add(atm);
      entry.atm = atm;
    }

    // The status drawn on the planet itself.
    if (!p.belt) {
      if (p.s === 'ready' || p.s === 'review' || p.s === 'failed') {
        const col = p.s === 'ready' ? palette.green : p.s === 'review' ? palette.magenta : palette.red;
        const ring = new THREE.Mesh(
          new THREE.TorusGeometry(p.size * (p.s === 'failed' ? 2.2 : 1.95), p.size * 0.09, 6, 30),
          new THREE.MeshBasicMaterial({ color: col || '#fff', transparent: true, opacity: 0.8 }),
        );
        ring.rotation.x = Math.PI / 2;
        ring.userData.ownGeo = true;
        ring.userData.ownMat = true;
        g.add(ring);
        rings.push({ mesh: ring, speed: p.s === 'ready' ? 0.5 : 0.18 });
      }
      if (p.s === 'plan') {
        const wire = new THREE.Mesh(
          new THREE.SphereGeometry(p.size * 1.18, 10, 7),
          new THREE.MeshBasicMaterial({ color: palette.cyan || '#5bcccb', wireframe: true, transparent: true, opacity: 0.5 }),
        );
        wire.userData.ownGeo = true;
        wire.userData.ownMat = true;
        g.add(wire);
        rings.push({ mesh: wire, speed: 0.25 });
      }
      if (p.s === 'working' && Number(data.tasks.find((t) => t.id === p.id)?.progress) > 0) {
        const prog = clamp(Number(data.tasks.find((t) => t.id === p.id)?.progress) / 100, 0.02, 1);
        const arc = new THREE.Mesh(
          new THREE.RingGeometry(p.size * 1.45, p.size * 1.45 + 0.22, 30, 1, 0, Math.PI * 2 * prog),
          new THREE.MeshBasicMaterial({ color: palette.blue || '#77b0e9', transparent: true, opacity: 0.85, side: THREE.DoubleSide }),
        );
        arc.rotation.x = -Math.PI / 2;
        arc.userData.ownGeo = true;
        arc.userData.ownMat = true;
        g.add(arc);
      }
    }

    // The agent: a satellite with its own shape and colour.
    if (!p.belt && p.agent) {
      const app = String(p.agent).split('/')[0].toLowerCase();
      const key = AGENT_COLOR[app] || 'fg';
      const shape = AGENT_SHAPE[app] === 'box' ? shared.box
        : AGENT_SHAPE[app] === 'octa' ? shared.octa
          : AGENT_SHAPE[app] === 'tetra' ? shared.tetra : shared.sphereLow;
      const sm = new THREE.Mesh(shape, new THREE.MeshBasicMaterial({
        color: palette[key] || palette.fg || '#ffffff', transparent: true, opacity: 0.9,
      }));
      sm.scale.setScalar(0.55);
      sm.userData.ownMat = true;
      g.add(sm);
      satellites.push({ mesh: sm, planet: entry, app, angle0: (hash(p.id) % 360) / 360 * Math.PI * 2, radius: p.size + 1.9 });
    }

    // Beacon to God for what waits for the human.
    if (!p.belt && (p.s === 'waiting' || p.s === 'plan' || p.s === 'ready')) {
      const geo = new THREE.BufferGeometry().setFromPoints([new THREE.Vector3(), new THREE.Vector3()]);
      const lm = new THREE.LineBasicMaterial({
        color: palette[STATUS_COLOR[p.s]] || palette.yellow || '#fff', transparent: true, opacity: 0.5,
      });
      const line = new THREE.Line(geo, lm);
      line.userData.ownGeo = true;
      line.userData.ownMat = true;
      groups.fx.add(line);
      const spark = new THREE.Mesh(shared.sparkGeo, new THREE.MeshBasicMaterial({
        color: palette[STATUS_COLOR[p.s]] || palette.yellow || '#fff', transparent: true, opacity: 0.95,
      }));
      spark.userData.ownMat = true;
      groups.fx.add(spark);
      entry.beacon = { line, spark, lm };
      beacons.push(entry.beacon);
    }

    // Label (CSS2D): a status icon plus the key, title and board line.
    const el = document.createElement('div');
    el.className = `lbl3d s-${p.s}`;
    el.dataset.id = p.id;
    el.innerHTML = `<span class="in"><b class="k">${esc(p.tag)}</b><span class="ag">${esc(String(p.agent || '').split('/')[0])}</span><svg class="gl"><use href="#g-${p.s}"/></svg></span><span class="ttl">${esc(p.title)}</span><span class="line">${esc(firstSentence(p.line, 120))}</span>`;
    el.style.zIndex = String(600 + Math.round(p.size * 10));
    const lab = new CSS2DObject(el);
    lab.position.set(0, p.size + 1.2, 0);
    // The label sits fully above its planet: it must not cover the status.
    lab.center.set(0.5, 1.0);
    lab.renderOrder = 10;
    g.add(lab);
    entry.label = lab;
    entry.el = el;
    labels.set(p.id, entry);
  }

  function buildLink(l) {
    const to = layout.positions.get(l.to);
    if (!to) return;
    const start = l.kind === 'far'
      ? farPosition(l.from, l.label || l.from)
      : layout.positions.get(l.from);
    if (!start) return;
    const a = new THREE.Vector3(start.x, start.y, start.z);
    const b = new THREE.Vector3(to.x, to.y, to.z);
    const same = start.u === to.u || (start.u && to.u && start.u.name === to.u.name);
    const mid = a.clone().add(b).multiplyScalar(0.5);
    mid.y = same ? 9 + a.distanceTo(b) * 0.12 : 46 + a.distanceTo(b) * 0.1;
    const curve = new THREE.QuadraticBezierCurve3(a, mid, b);
    const pts = curve.getPoints(30);
    const geo = new THREE.BufferGeometry().setFromPoints(pts);
    const col = l.kind === 'waiting' ? (palette.accent || '#fff') : (palette.muted || '#888');
    const mat = l.kind === 'done'
      ? new THREE.LineBasicMaterial({ color: col, transparent: true, opacity: 0.2 })
      : new THREE.LineDashedMaterial({ color: col, transparent: true, opacity: 0.5, dashSize: 2.4, gapSize: 2.6 });
    mat.userData.kind = l.kind;
    const line = new THREE.Line(geo, mat);
    if (mat.isLineDashedMaterial) line.computeLineDistances();
    line.userData.ownGeo = true;
    line.userData.ownMat = true;
    mat.userData.link = l;
    mat.userData.linkFrom = l.from;
    mat.userData.linkTo = l.to;
    groups.links.add(line);
    linkMats.push(mat);

    const tube = new THREE.Mesh(
      new THREE.TubeGeometry(curve, 24, 1.4, 4, false),
      new THREE.MeshBasicMaterial({ transparent: true, opacity: 0.001, depthWrite: false }),
    );
    tube.userData.link = l;
    tube.userData.ownGeo = true;
    tube.userData.ownMat = true;
    groups.links.add(tube);
    linkPickables.push(tube);

    if (l.kind !== 'done' && !world.simple) {
      const spark = new THREE.Mesh(shared.sparkGeo, new THREE.MeshBasicMaterial({
        color: col, transparent: true, opacity: 0.95,
      }));
      spark.userData.ownMat = true;
      groups.links.add(spark);
      links.push({ ...l, curve, spark, len: curve.getLength() });
    }
  }

  function farPosition(ref, label = ref) {
    if (farStars.has(ref)) return farStars.get(ref);
    const a = (hash(ref) % 360) / 360 * Math.PI * 2;
    const pos = { x: Math.cos(a) * 380, y: 0, z: Math.sin(a) * 380, u: null };
    farStars.set(ref, pos);
    const mesh = new THREE.Mesh(shared.sphereLow, new THREE.MeshBasicMaterial({
      color: palette.muted || '#888', transparent: true, opacity: 0.55,
    }));
    mesh.scale.setScalar(1.8);
    mesh.position.set(pos.x, 0, pos.z);
    mesh.userData.ownMat = true;
    groups.far.add(mesh);
    const el = document.createElement('div');
    el.className = 'lbl3d compact s-queued';
    el.innerHTML = `<span class="in"><b class="k">${esc(label)}</b><svg class="gl"><use href="#g-link"/></svg></span>`;
    const lab = new CSS2DObject(el);
    lab.position.set(pos.x, 4, pos.z);
    groups.far.add(lab);
    return pos;
  }

  function buildBeams() {
    if (!godMesh) return;
    for (const u of layout.universes) {
      if (u.quiet) continue;
      const geo = new THREE.BufferGeometry().setFromPoints([
        new THREE.Vector3(0, 0, 0), new THREE.Vector3(u.center.x, 0, u.center.z),
      ]);
      const line = new THREE.Line(geo, shared.beamMat);
      line.userData.ownGeo = true;
      beamGroup.add(line);
      beamMats.push(shared.beamMat);
      const spark = new THREE.Mesh(shared.sparkGeo, new THREE.MeshBasicMaterial({
        color: palette.accent || '#fff', transparent: true, opacity: 0,
      }));
      spark.userData.ownMat = true;
      spark.visible = false;
      beamGroup.add(spark);
      beamSparks.push({ mesh: spark, to: new THREE.Vector3(u.center.x, 0, u.center.z), phase: (hash(u.name) % 100) / 100 });
    }
  }

  /* ── status changes, freshness, filter ── */

  function updateFresh(prevStatus) {
    const fresh = data.fresh || {};
    for (const p of planets) {
      const prev = prevStatus.get(p.id);
      if (prev && prev !== p.s && !world.simple) ringFx(p, p.s);
      if (fresh[p.id] && !burstSeen.has(p.id) && !world.simple) {
        burstSeen.add(p.id);
        ringFx(p, 'done');
      }
    }
    for (const id of [...burstSeen]) if (!fresh[id]) burstSeen.delete(id);
  }

  function ringFx(p, status) {
    const col = palette[STATUS_COLOR[status]] || palette.accent || '#fff';
    const geo = new THREE.RingGeometry(p.size * 1.1, p.size * 1.25, 32);
    const mesh = new THREE.Mesh(geo, new THREE.MeshBasicMaterial({
      color: col, transparent: true, opacity: 0.9, side: THREE.DoubleSide, depthWrite: false,
    }));
    mesh.rotation.x = -Math.PI / 2;
    mesh.userData.ownGeo = true;
    mesh.userData.ownMat = true;
    const pos = layout.positions.get(p.id);
    mesh.position.set(pos ? pos.x : 0, (pos ? pos.y : 0) + 0.5, pos ? pos.z : 0);
    groups.fx.add(mesh);
    fx.push({ mesh, t0: performance.now(), dur: 1100 });
  }

  function updateFilter() {
    const f = data.filter;
    const on = (s) => !f || f.includes(s);
    for (const s of STATUSES) {
      const dim = !on(s);
      statusMat[s].opacity = dim ? 0.14 : 1;
      atmMat[s].opacity = dim ? 0.03 : (mode === 'light' ? 0.1 : 0.17);
    }
    for (const p of planets) {
      if (!p.el) continue;
      p.el.classList.toggle('dim', !on(p.s));
    }
    for (const m of linkMats) {
      const from = layout.positions.get(m.userData.linkFrom);
      const to = layout.positions.get(m.userData.linkTo);
      const dim = (from && !on(from.p.s)) || (to && !on(to.p.s));
      m.opacity = dim ? 0.06 : m.userData.kind === 'done' ? 0.2 : 0.5;
    }
  }

  /* ── selection and camera ── */

  function select(id, fly = false) {
    data.selected = id;
    for (const [pid, p] of labels) {
      p.el.classList.toggle('sel', pid === id);
      p.el.style.zIndex = pid === id ? '9000' : String(600 + Math.round((p.size || 1) * 10));
    }
    const entry = layout.positions.get(id);
    if (entry && selRing) {
      const pos = planetPosition(entry, time, !world.simple);
      const size = entry.p.size * 1.55;
      selRing.scale.setScalar(size);
      selRing.position.set(pos.x, pos.y + size * 0.9, pos.z);
      selRing.rotation.x = Math.PI / 2;
      selRing.visible = true;
    } else if (selRing) {
      selRing.visible = false;
    }
    if (fly) flyTo(id);
    requestRender();
  }

  function flyTo(target) {
    if (!world.webgl || !cam) return false;
    if (!target || target === 'overview') {
      cam.overview();
      current = { kind: 'overview', id: null, universe: null };
    } else if (target === 'god') {
      cam.god();
      current = { kind: 'god', id: null, universe: null };
    } else if (layout.positions.has(target)) {
      const entry = layout.positions.get(target);
      const pos = planetPosition(entry, time, !world.simple);
      cam.planet(new THREE.Vector3(pos.x, pos.y, pos.z));
      current = { kind: 'planet', id: target, universe: entry.u.name };
      if (data.selected !== target) {
        data.selected = target;
        select(target, false);
        world.onSelect(target);
      }
    } else {
      const u = layout.universes.find((x) => x.name === target);
      if (!u) return false;
      cam.universe(u.center);
      current = { kind: 'universe', id: null, universe: u.name };
    }
    requestRender();
    return true;
  }

  /// Subscription rings around God: one sector per agent, filled by the
  /// remaining share of its most-consumed live window. 80 % used turns the
  /// sector yellow, 95 % red and makes the halo flicker; agent satellites
  /// wear the same colour. The HUD names every window; the world shows the
  /// one closest to exhaustion.
  function setLimits(limits) {
    data.limits = limits || null;
    if (!limitsGroup) return;
    clearGroup(limitsGroup);
    const agents = limits && Array.isArray(limits.agents) ? limits.agents : [];
    let slot = 0;
    let critical = false;
    const live = [];
    for (const agent of agents) {
      const worst = worstWindow(agent);
      if (!worst) continue;
      live.push({ agent, worst });
      // Outside the 60-unit halo, still well inside the 190-unit universe ring.
      const radius = 68 + slot * 9;
      const remaining = clamp(1 - Number(worst.percent || 0), 0, 1);
      const severity = worst.severity === 'crit' ? 'crit' : worst.severity === 'warn' ? 'warn' : 'ok';
      const key = severity === 'crit' ? 'red' : severity === 'warn' ? 'yellow' : (AGENT_COLOR[agent.id] || 'accent');
      const color = palette[key] || palette.accent || '#ffffff';
      const dim = agent.stale ? 0.45 : 1;
      if (severity === 'crit') critical = true;

      const track = new THREE.Mesh(
        new THREE.RingGeometry(radius, radius + 1.5, 64, 1),
        new THREE.MeshBasicMaterial({
          color: palette.muted || '#806f64', transparent: true, opacity: 0.16 * dim,
          side: THREE.DoubleSide, depthWrite: false,
        }),
      );
      track.rotation.x = -Math.PI / 2;
      track.userData.ownGeo = true;
      track.userData.ownMat = true;

      const arc = new THREE.Mesh(
        new THREE.RingGeometry(radius, radius + 2.1, 96, 1, Math.PI / 2, Math.PI * 2 * remaining),
        new THREE.MeshBasicMaterial({
          color, transparent: true, opacity: 0.9 * dim,
          side: THREE.DoubleSide, depthWrite: false, blending: THREE.AdditiveBlending,
        }),
      );
      arc.rotation.x = -Math.PI / 2;
      arc.userData.ownGeo = true;
      arc.userData.ownMat = true;
      arc.userData.agent = agent.id;
      arc.userData.severity = severity;

      limitsGroup.add(track, arc);
      slot++;
    }
    if (godHalo) godHalo.userData.limitCrit = critical;

    // Satellites inherit their agent's arc colour; an agent with no limit
    // data keeps its program colour.
    for (const sat of satellites) {
      const entry = live.find((x) => String(x.agent.id) === sat.app);
      const severity = entry ? entry.worst.severity : '';
      const key = entry
        ? (severity === 'crit' ? 'red' : severity === 'warn' ? 'yellow' : (AGENT_COLOR[sat.app] || 'accent'))
        : (AGENT_COLOR[sat.app] || 'fg');
      sat.mesh.material.color.set(palette[key] || '#ffffff');
    }
    requestRender();
  }

  function worstWindow(agent) {
    const now = Date.now();
    const windows = (Array.isArray(agent && agent.limits) ? agent.limits : [])
      // A window whose reset has passed is no longer being spent; the next
      // Omarchy scan replaces it. Skip it here for the same reason the HUD
      // chip and /api/limits do.
      .filter((w) => !w.resets_at_ms || w.resets_at_ms > now);
    return windows.reduce((best, w) => (!best || Number(w.percent) > Number(best.percent) ? w : best), null);
  }

  /** Where a planet is on screen (for checks and the app's own placement). */
  function screenOf(id) {
    if (!cam) return null;
    const e = layout.positions.get(id);
    if (!e) return null;
    const pos = planetPosition(e, time, !world.simple);
    const v = new THREE.Vector3(pos.x, pos.y, pos.z).project(cam.camera);
    return { x: (v.x * 0.5 + 0.5) * innerWidth, y: (-v.y * 0.5 + 0.5) * innerHeight };
  }

  function setFilter(list) {
    data.filter = list && list.length ? list : null;
    updateFilter();
    requestRender();
  }

  function setSimple(v) {
    if (world.simple === !!v) return;
    world.simple = !!v;
    if (cam) cam.setSimple(world.simple);
    if (renderer) renderer.setPixelRatio(world.simple ? 1 : Math.min(devicePixelRatio || 1, 2));
    for (const p of planets) if (p.atm) p.atm.visible = !world.simple;
    if (!world.simple) startLoop();
    else { stopLoop(); renderFrame(performance.now()); }
    requestRender();
  }

  function linkModeOn(v) {
    linkMode = !!v;
    canvas.classList.toggle('link-mode', linkMode);
    requestRender();
  }

  function connect(a, b) {
    if (!a || !b || a === b) return false;
    world.onConnect(a, b);
    return true;
  }

  /* ── pointer ── */

  function toNDC(e) {
    const r = canvas.getBoundingClientRect();
    pointer.x = ((e.clientX - r.left) / r.width) * 2 - 1;
    pointer.y = -((e.clientY - r.top) / r.height) * 2 + 1;
    raycaster.setFromCamera(pointer, cam.camera);
  }

  function pickPlanet(e) {
    if (!cam) return null;
    toNDC(e);
    const hits = raycaster.intersectObjects(planetPickables, false);
    return hits[0]?.object?.userData?.id || null;
  }

  function pickLink(e) {
    if (!cam) return null;
    toNDC(e);
    const hits = raycaster.intersectObjects(linkPickables, false);
    return hits[0]?.object?.userData?.link || null;
  }

  function pickThing(e) {
    if (!cam) return null;
    toNDC(e);
    const p = raycaster.intersectObjects(planetPickables, false)[0];
    if (p) return { kind: 'planet', id: p.object.userData.id };
    const l = raycaster.intersectObjects(linkPickables, false)[0];
    if (l) return { kind: 'link', link: l.object.userData.link };
    const s = raycaster.intersectObjects(sunPickables, false)[0];
    if (s) return { kind: 'universe', name: s.object.userData.universe };
    const g = raycaster.intersectObject(godMesh, false)[0];
    if (g) return { kind: 'god' };
    return null;
  }

  function startDrag(id, e) {
    const entry = layout.positions.get(id);
    if (!entry) return;
    const pos = planetPosition(entry, time, !world.simple);
    const geo = new THREE.BufferGeometry().setFromPoints([new THREE.Vector3(pos.x, pos.y, pos.z), new THREE.Vector3(pos.x, pos.y, pos.z)]);
    const mat = new THREE.LineDashedMaterial({ color: palette.accent || '#fff', transparent: true, opacity: 0.95, dashSize: 2, gapSize: 2 });
    tempLine = new THREE.Line(geo, mat);
    tempLine.userData.ownGeo = true;
    tempLine.userData.ownMat = true;
    tempLine.computeLineDistances();
    groups.fx.add(tempLine);
    drag = { from: id, pos: new THREE.Vector3(pos.x, pos.y, pos.z) };
    if (cam) cam.controls.enabled = false;
    canvas.classList.add('link-mode');
    document.body.dataset.linkFrom = id;
  }

  function finishDrag(e) {
    if (!drag) return;
    const from = drag.from;
    drag = null;
    if (cam) cam.controls.enabled = true;
    if (tempLine) {
      groups.fx.remove(tempLine);
      tempLine.geometry.dispose();
      tempLine.material.dispose();
      tempLine = null;
    }
    delete document.body.dataset.linkFrom;
    canvas.classList.remove('link-mode');
    if (linkMode) canvas.classList.add('link-mode');
    const to = pickPlanet(e);
    if (to && to !== from) world.onConnect(from, to);
    requestRender();
  }

  function bindPointer() {
    let down = null;
    canvas.addEventListener('pointerdown', (e) => {
      if (!cam) return;
      if (e.button !== 0) return;
      const id = pickPlanet(e);
      down = { x: e.clientX, y: e.clientY, t: Date.now(), id, moved: false };
      if (id && (linkMode || e.shiftKey)) {
        e.stopImmediatePropagation();
        e.preventDefault();
        startDrag(id, e);
      }
    }, { capture: true });

    canvas.addEventListener('pointermove', (e) => {
      if (!cam) return;
      if (down && Math.abs(e.clientX - down.x) + Math.abs(e.clientY - down.y) > 5) down.moved = true;
      if (drag && tempLine) {
        toNDC(e);
        const end = new THREE.Vector3();
        if (raycaster.ray.intersectPlane(plane, end)) {
          const pts = tempLine.geometry.attributes.position;
          pts.setXYZ(1, end.x, 0, end.z);
          pts.needsUpdate = true;
          tempLine.computeLineDistances();
        }
        const over = pickPlanet(e);
        for (const [pid, p] of labels) p.el.classList.toggle('hover', pid === over && pid !== drag.from);
        return;
      }
      // Hover is cheap enough at 10 Hz.
      const now = performance.now();
      if (now - (canvas._hoverAt || 0) < 90) return;
      canvas._hoverAt = now;
      const id = pickPlanet(e);
      if (id !== hovered) {
        hovered = id;
        for (const [pid, p] of labels) p.el.classList.toggle('hover', pid === id);
        canvas.style.cursor = id ? 'pointer' : '';
        requestRender();
      }
    }, { passive: true });

    canvas.addEventListener('pointerup', (e) => {
      if (!cam) return;
      if (drag) { finishDrag(e); return; }
      if (!down) return;
      const d = down;
      down = null;
      if (d.moved || Date.now() - d.t > 600) return;
      const hit = pickThing(e);
      if (!hit) return;
      if (hit.kind === 'planet') { flyTo(hit.id); }
      else if (hit.kind === 'universe') { world.onUniverse(hit.name); flyTo(hit.name); }
      else if (hit.kind === 'link') world.onLink(hit.link);
      else if (hit.kind === 'god') { world.onGod(); flyTo('god'); }
    });

    canvas.addEventListener('dblclick', (e) => {
      if (!cam) return;
      if (pickThing(e)) return;
      world.onEmpty();
      flyTo('overview');
    });

    canvas.addEventListener('pointercancel', () => {
      if (drag) {
        drag = null;
        if (cam) cam.controls.enabled = true;
        if (tempLine) {
          groups.fx.remove(tempLine);
          tempLine.geometry.dispose();
          tempLine.material.dispose();
          tempLine = null;
        }
      }
      down = null;
    });
  }

  /* ── animation ── */

  function startLoop() {
    if (raf || !world.webgl) return;
    fpsStart = performance.now();
    frames = 0;
    frameTimes = [];
    raf = requestAnimationFrame(loop);
  }

  function stopLoop() {
    if (raf) cancelAnimationFrame(raf);
    raf = 0;
  }

  function loop(now) {
    raf = requestAnimationFrame(loop);
    const dt = now - (loop.last || now);
    loop.last = now;
    if (dt < 250) frameTimes.push(dt);
    frames++;
    if (!autoChecked && reduced === false && !world.simple && now - clockStart > 3000) {
      autoChecked = true;
      const sorted = frameTimes.slice().sort((a, b) => a - b);
      const median = sorted[Math.floor(sorted.length / 2)] || 0;
      if (sorted.length > 20 && median > 33) {
        setSimple(true);
        world.onAutoSimple();
      }
    }
    if (now - fpsStart > 1000) {
      lastFps = Math.round((frames * 1000) / (now - fpsStart));
      frames = 0;
      fpsStart = now;
    }
    renderFrame(now);
  }

  function requestRender() {
    if (!world.webgl) return;
    dirty = true;
    // The simple mode draws exactly one frame per change and never waits for
    // rAF: a throttled or hidden tab must still get its labels.
    if (world.simple) { renderFrame(performance.now()); return; }
    if (hidden) { renderFrame(performance.now()); return; }
    if (!raf) startLoop();
    // A throttled tab (or a stalled rAF) still needs one frame per change.
    if (performance.now() - lastFrameAt > 350) renderFrame(performance.now());
  }

  function renderFrame(now) {
    if (!world.webgl || !scene || rendering) return;
    rendering = true;
    time = (now - clockStart) / 1000;
    stepTweens(now);
    animate(now, time);
    if (cam) {
      cam.update(now);
      cam.setAspect(innerWidth, innerHeight);
    }
    renderer.render(scene, cam.camera);
    // One label pass puts every element into the DOM so LOD and the
    // anti-overlap pass can measure it; the second applies the result.
    labelRenderer.render(scene, cam.camera);
    updateLODs();
    updateChatScale();
    labelRenderer.render(scene, cam.camera);
    lastFrameAt = performance.now();
    dirty = false;
    rendering = false;
  }

  function animate(now, t) {
    const moving = !world.simple;
    // Labels are callouts in screen space: up is the camera's up, so they stay
    // above their planet at every angle, and a small sideways stagger spreads
    // neighbours. The anti-overlap pass then downgrades what still collides.
    const camUp = cam ? new THREE.Vector3(0, 1, 0).applyQuaternion(cam.camera.quaternion).normalize() : new THREE.Vector3(0, 1, 0);
    const camRight = cam ? new THREE.Vector3(1, 0, 0).applyQuaternion(cam.camera.quaternion).normalize() : new THREE.Vector3(1, 0, 0);
    for (const p of planets) {
      const entry = layout.positions.get(p.id);
      if (!entry) continue;
      const pos = planetPosition(entry, t, moving);
      p.group.position.set(pos.x, pos.y, pos.z);
      p.pos = pos;
      if (p.label) {
        const side = ((hash(p.id) % 3) - 1) * 1.6;
        p.label.position.set(
          camUp.x * 5 + camRight.x * side,
          camUp.y * 5 + camRight.y * side,
          camUp.z * 5 + camRight.z * side,
        );
      }
    }
    for (const s of satellites) {
      const pp = s.planet.pos || { x: 0, y: 0, z: 0 };
      const a = s.angle0 + (moving ? t * 1.35 : 0);
      s.mesh.position.set(pp.x + Math.cos(a) * s.radius, pp.y + 0.4, pp.z + Math.sin(a) * s.radius);
    }
    if (moving) {
      for (const r of rings) r.mesh.rotation.y = t * r.speed;
    }
    // Beacons: a line from the planet to God, with one spark travelling in.
    for (const p of planets) {
      if (!p.beacon) continue;
      const pp = p.pos || { x: 0, y: 0, z: 0 };
      const pts = p.beacon.line.geometry.attributes.position;
      pts.setXYZ(0, pp.x, pp.y, pp.z);
      pts.setXYZ(1, 0, 0, 0);
      pts.needsUpdate = true;
      if (moving) {
        const k = ((t * (p.s === 'waiting' ? 0.55 : 0.35)) + (hash(p.id) % 100) / 100) % 1;
        p.beacon.spark.position.set(pp.x * (1 - k), pp.y * (1 - k), pp.z * (1 - k));
        p.beacon.spark.visible = true;
      } else {
        p.beacon.spark.visible = false;
      }
    }
    // Dependencies: the spark travels from the predecessor to the waiting task.
    for (const l of links) {
      if (!l.spark) continue;
      const k = (t * 0.22 + (hash(l.from + l.to) % 100) / 100) % 1;
      const pos = l.curve.getPointAt(k);
      l.spark.position.copy(pos);
      l.spark.visible = moving;
    }
    // God: pulse when busy, glow when a dialog waits.
    if (godHalo) {
      const busy = !!godHalo.userData.busy;
      const limitCrit = !!godHalo.userData.limitCrit;
      const pulse = moving && (busy || limitCrit) ? 1 + Math.sin(t * 4.2) * 0.08 : 1;
      godHalo.scale.setScalar(120 * pulse);
      godHalo.material.opacity = godHalo.userData.dialog ? 0.95
        : busy ? 0.75 + Math.sin(t * 4.2) * 0.12
          : limitCrit ? 0.8 + Math.sin(t * 3.4) * 0.14 : 0.8;
    }
    // Light packets from God to the universes while the Coordinator works.
    const busy = !!(data.coordinator && (data.coordinator.busy || data.coordinator.status === 'working'));
    for (const s of beamSparks) {
      s.mesh.visible = moving && busy;
      if (busy) {
        const k = (t * 0.4 + s.phase) % 1;
        s.mesh.position.set(s.to.x * k, s.to.y * k, s.to.z * k);
      }
    }
    // Status-change rings.
    for (let i = fx.length - 1; i >= 0; i--) {
      const f = fx[i];
      const k = (now - f.t0) / f.dur;
      if (k >= 1) {
        groups.fx.remove(f.mesh);
        f.mesh.geometry.dispose();
        f.mesh.material.dispose();
        fx.splice(i, 1);
      } else {
        f.mesh.scale.setScalar(1 + k * 2.6);
        f.mesh.material.opacity = 0.9 * (1 - k);
      }
    }
    // A selected planet's ring follows it.
    if (selRing?.visible && data.selected) {
      const entry = layout.positions.get(data.selected);
      if (entry) {
        const pp = entry && planets.find((p) => p.id === data.selected)?.pos;
        if (pp) selRing.position.set(pp.x, pp.y + entry.p.size * 1.4, pp.z);
      }
    }
  }

  let lodKey = '';
  let lodAt = 0;
  const LOD_ORDER = { waiting: 0, failed: 1, ready: 2, plan: 3, review: 4, working: 5, queued: 6, done: 7 };

  function overlap(a, b, pad = 2) {
    return Math.min(a.right, b.right) - Math.max(a.left, b.left) > pad
      && Math.min(a.bottom, b.bottom) - Math.max(a.top, b.top) > pad;
  }

  function updateLODs() {
    if (!cam) return;
    const now = performance.now();
    if (!world.simple && !document.hidden && now - lodAt < 120) return;
    lodAt = now;
    const pos = cam.camera.position;
    const key = `${Math.round(pos.x / 20)},${Math.round(pos.y / 20)},${Math.round(pos.z / 20)},${data.selected || ''},${hovered || ''}`;
    if (key !== lodKey) {
      lodKey = key;
      for (const p of planets) p.dotOverride = false;
    }
    const cd = cam.camera.position;
    // Exactly one label may be full: the hovered/picked planet, or the single
    // nearest one when the camera is very close. A zoomed universe therefore
    // never turns every nearby planet into a pile of full labels.
    const pinned = hovered || data.selected;
    let nearest = null;
    let nearestD = Infinity;
    for (const p of planets) {
      if (!p.el) continue;
      const at = p.pos || { x: 0, y: 0, z: 0 };
      const d = cd.distanceTo(new THREE.Vector3(at.x, at.y, at.z));
      p.dist = d;
      if (d < nearestD) { nearestD = d; nearest = p; }
    }
    for (const p of planets) {
      if (!p.el) continue;
      const d = p.dist ?? 0;
      const full = p.id === pinned || (!pinned && p === nearest && nearestD < 80);
      const dot = !full && (d > 520 || p.dotOverride);
      p.el.classList.toggle('dot', !full && dot);
      p.el.classList.toggle('compact', !full && !dot);
      if (full) { p.el.classList.remove('dot'); p.el.classList.remove('compact'); }
    }

    // Compact chips may still collide on a tilted orbit; downgrade the one
    // that matters less to its status icon until nothing overlaps.
    let changed = false;
    for (let pass = 0; pass < 3; pass++) {
      const taken = [];
      for (const p of planets) {
        if (!p.el) continue;
        const dot = p.el.classList.contains('dot');
        const full = !dot && !p.el.classList.contains('compact');
        if (!dot && !full) continue;
        const r = p.el.getBoundingClientRect();
        if (r.width > 0) taken.push(r);
      }
      const compact = planets
        .filter((p) => p.el && p.el.classList.contains('compact'))
        .sort((a, b) => (LOD_ORDER[a.s] ?? 9) - (LOD_ORDER[b.s] ?? 9));
      let downgraded = false;
      for (const p of compact) {
        const r = p.el.getBoundingClientRect();
        if (!r.width) continue;
        if (taken.some((t) => overlap(r, t))) {
          p.dotOverride = true;
          p.el.classList.add('dot');
          p.el.classList.remove('compact');
          const d = p.el.getBoundingClientRect();
          if (d.width) taken.push(d);
          downgraded = true;
          changed = true;
        } else {
          taken.push(r);
        }
      }
      if (!downgraded) break;
    }
    if (changed) requestRender();

    const gd = cd.distanceTo(new THREE.Vector3(0, 0, 0));
    for (const el of labelsEl.querySelectorAll('.lbl3d.uni')) {
      el.classList.toggle('compact', gd > 700);
    }
  }

  function updateChatScale() {
    if (!chatObject || !cam) return;
    const d = cam.camera.position.distanceTo(chatAnchor);
    const s = clamp(1.16 - d / 900, 0.75, 1);
    const scale = chatObject.element.firstElementChild;
    if (scale) scale.style.setProperty('--chat-scale', world.simple ? 1 : s.toFixed(3));
    if (todoObject && todoScaleEl) {
      const ds = cam.camera.position.distanceTo(todoAnchor);
      const ts = clamp(1.16 - ds / 900, 0.75, 1);
      todoScaleEl.style.setProperty('--todo-scale', world.simple ? 1 : ts.toFixed(3));
    }
  }

  /* ── misc ── */

  function onResize() {
    if (!world.webgl) return;
    renderer.setSize(innerWidth, innerHeight, false);
    labelRenderer.setSize(innerWidth, innerHeight);
    requestRender();
  }

  function onVisibility() {
    hidden = document.hidden;
    if (hidden) stopLoop();
    else { startLoop(); renderFrame(performance.now()); }
  }

  function debug() {
    return {
      webgl: world.webgl,
      simple: world.simple,
      planets: new Set(planets.filter((p) => p.s !== 'done').map((p) => p.id)).size,
      statuses: [...new Set(planets.filter((p) => p.s !== 'done').map((p) => p.s))].sort(),
      universes: layout.universes.length,
      quiet: layout.universes.filter((u) => u.quiet).length,
      links: (data.links || []).map((l) => ({ from: l.from, to: l.to, kind: l.kind })),
      view: current.kind,
      selected: data.selected,
      palette: { accent: palette.accent, mode },
      attention: Number(data.attention) || 0,
      fps: lastFps,
      labels: labels.size,
      limitArcs: limitsGroup ? limitsGroup.children.filter((c) => c.userData.agent).length : 0,
      limitCrit: !!(godHalo && godHalo.userData.limitCrit),
      filter: data.filter,
      dim: [...labels.values()].filter((p) => p.el && p.el.classList.contains('dim')).length,
      camera: cam ? { x: Math.round(cam.camera.position.x), y: Math.round(cam.camera.position.y), z: Math.round(cam.camera.position.z) } : null,
    };
  }

  init();

  const api = {
    ...world,
    get webgl() { return world.webgl; },
    get simple() { return world.simple; },
    setData, setPalette, setSimple, setFilter, select, flyTo, connect, linkModeOn, debug, setLimits,
    requestRender, detachChat, attachChat, detachTodo, attachTodo,
    universeAt, setDropTarget, dropFlash, showTodo, screenOf, get dropTarget() { return dropTarget; },
    orbitBy: (dx, dy) => { if (cam) cam.orbitBy(dx, dy); },
    zoomBy: (factor) => { if (cam) cam.zoomBy(factor); },
    get selected() { return data.selected; },
    get current() { return current; },
    get positions() { return layout.positions; },
  };
  // The app assigns the callbacks after createWorld returns; they must reach
  // the closure that calls them, not a stale copy.
  for (const k of ['onSelect', 'onUniverse', 'onGod', 'onEmpty', 'onLink', 'onConnect', 'onAutoSimple']) {
    Object.defineProperty(api, k, {
      get: () => world[k],
      set: (fn) => { world[k] = fn; },
      configurable: true,
    });
  }
  return api;
}
