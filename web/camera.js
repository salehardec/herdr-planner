/* Камера мира: OrbitControls, подлёты и клавиши камеры. */

import * as THREE from './vendor/0.186.1/three.module.js';
import { OrbitControls } from './vendor/0.186.1/OrbitControls.js';

const EASE = (t) => 1 - Math.pow(1 - t, 3);

export function createCamera({ canvas, simple = false, onChange = () => {} }) {
  const camera = new THREE.PerspectiveCamera(45, 1, 1, 5000);
  camera.position.set(0, 330, 470);
  const controls = new OrbitControls(camera, canvas);
  controls.target.set(0, 0, 0);
  controls.enableDamping = !simple;
  controls.dampingFactor = 0.08;
  controls.rotateSpeed = 0.65;
  controls.zoomSpeed = 0.9;
  controls.panSpeed = 0.7;
  controls.minDistance = 24;
  controls.maxDistance = 1300;
  controls.maxPolarAngle = Math.PI * 0.47;
  controls.minPolarAngle = 0.08;
  controls.screenSpacePanning = false;
  controls.addEventListener('change', onChange);

  let tween = null;
  let view = 'overview';
  let isSimple = simple;

  function setAspect(w, h) {
    camera.aspect = w / Math.max(1, h);
    camera.updateProjectionMatrix();
  }

  function stopTween() {
    tween = null;
    controls.enabled = true;
  }

  canvas.addEventListener('pointerdown', stopTween, { passive: true });

  function flyTo(pos, target, kind = 'overview') {
    view = kind;
    // A hidden tab gets no animation frames: go there at once so the LOD and
    // the labels are right when the tab is looked at again.
    if (isSimple || document.hidden) {
      camera.position.copy(pos);
      controls.target.copy(target);
      controls.update();
      onChange();
      return;
    }
    const t = {
      t0: performance.now(),
      dur: 700,
      fromPos: camera.position.clone(),
      toPos: pos.clone(),
      fromTarget: controls.target.clone(),
      toTarget: target.clone(),
    };
    tween = t;
    controls.enabled = false;
    // A throttled tab may not deliver animation frames: land anyway.
    setTimeout(() => {
      if (tween !== t) return;
      camera.position.copy(t.toPos);
      controls.target.copy(t.toTarget);
      camera.lookAt(controls.target);
      stopTween();
      onChange();
    }, 900);
  }

  function overview() {
    flyTo(new THREE.Vector3(0, 330, 470), new THREE.Vector3(0, 0, 0), 'overview');
  }

  function god() {
    flyTo(new THREE.Vector3(0, 70, 128), new THREE.Vector3(0, 0, 0), 'god');
  }

  function universe(center) {
    flyTo(
      new THREE.Vector3(center.x, 118, center.z + 132),
      new THREE.Vector3(center.x, 0, center.z),
      'universe',
    );
  }

  function planet(pos) {
    flyTo(
      new THREE.Vector3(pos.x, pos.y + 26, pos.z + 38),
      new THREE.Vector3(pos.x, pos.y, pos.z),
      'planet',
    );
  }

  function orbitBy(dTheta, dPhi) {
    const off = camera.position.clone().sub(controls.target);
    const sph = new THREE.Spherical().setFromVector3(off);
    sph.theta -= dTheta;
    sph.phi = Math.max(0.15, Math.min(Math.PI * 0.46, sph.phi + dPhi));
    off.setFromSpherical(sph);
    camera.position.copy(controls.target).add(off);
    controls.update();
    onChange();
  }

  function zoomBy(factor) {
    const off = camera.position.clone().sub(controls.target);
    const dist = Math.max(controls.minDistance, Math.min(controls.maxDistance, off.length() * factor));
    off.setLength(dist);
    camera.position.copy(controls.target).add(off);
    controls.update();
    onChange();
  }

  function update(now) {
    if (tween) {
      const t = Math.min(1, (now - tween.t0) / tween.dur);
      const k = EASE(t);
      camera.position.lerpVectors(tween.fromPos, tween.toPos, k);
      controls.target.lerpVectors(tween.fromTarget, tween.toTarget, k);
      camera.lookAt(controls.target);
      if (t >= 1) stopTween();
      onChange();
    } else if (controls.enableDamping) {
      controls.update();
    }
  }

  function setSimple(v) {
    isSimple = v;
    controls.enableDamping = !v;
  }

  return {
    camera,
    controls,
    setAspect,
    flyTo,
    overview,
    god,
    universe,
    planet,
    orbitBy,
    zoomBy,
    update,
    setSimple,
    get view() { return view; },
    set view(v) { view = v; },
    stopTween,
  };
}
