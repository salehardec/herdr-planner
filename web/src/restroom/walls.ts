/* The wallpapers of the rest room: three pictures drawn in SVG (the sea at sunset, the Earth through the ship's greenhouse,
   an aurora over a forest) and the person's own. Pictures are not tied to the theme: they are things in the room. */

const svg = (body: string): string => `<svg viewBox="0 0 1600 900" preserveAspectRatio="xMidYMid slice" xmlns="http://www.w3.org/2000/svg">${body}</svg>`;
const range = (n: number): number[] => Array.from({ length: n }, (_, i) => i);

const earth = svg(`
<defs>
<radialGradient id="ea" cx="50%" cy="100%" r="80%"><stop offset="0" stop-color="#1f5fa8"/><stop offset=".55" stop-color="#2c7fc4"/><stop offset=".8" stop-color="#3f9bd8"/><stop offset="1" stop-color="#9fe0ff"/></radialGradient>
<radialGradient id="glow" cx="50%" cy="100%" r="62%"><stop offset=".78" stop-color="#6cc6ff" stop-opacity="0"/><stop offset=".86" stop-color="#8fd8ff" stop-opacity=".55"/><stop offset="1" stop-color="#8fd8ff" stop-opacity="0"/></radialGradient>
<linearGradient id="sp" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="#03040b"/><stop offset="1" stop-color="#0b1430"/></linearGradient>
<linearGradient id="fl" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="#2a2a22"/><stop offset="1" stop-color="#121410"/></linearGradient>
<radialGradient id="lamp" cx="50%" cy="0" r="1"><stop offset="0" stop-color="#ffd9a0" stop-opacity=".35"/><stop offset="1" stop-color="#ffd9a0" stop-opacity="0"/></radialGradient>
</defs>
<rect width="1600" height="900" fill="url(#sp)"/>
<g fill="#fff">${range(120).map((i) => `<circle cx="${(i * 137.5) % 1600}" cy="${(i * 71.3) % 520}" r="${(i % 5) / 3 + 0.4}" opacity="${0.3 + (i % 7) / 10}"/>`).join('')}</g>
<circle cx="800" cy="1330" r="1040" fill="url(#glow)"/>
<circle cx="800" cy="1330" r="970" fill="url(#ea)"/>
<g fill="#3e8a4a" opacity=".85" transform="translate(0,-150)"><path d="M330 760 q80-90 200-70 t180 40 q40 60-60 120 H300z"/><path d="M960 600 q90-40 170 10 t120 120 q-30 40-120 20 t-170-60z"/></g>
<g fill="#fff" opacity=".55" transform="translate(0,-150)"><ellipse cx="560" cy="640" rx="160" ry="16"/><ellipse cx="1040" cy="560" rx="200" ry="14"/><ellipse cx="760" cy="720" rx="230" ry="18"/><ellipse cx="1220" cy="700" rx="120" ry="12"/></g>
<rect width="1600" height="900" fill="url(#lamp)"/>
<g fill="none" stroke="#1a1d17" stroke-width="26"><path d="M80 900 V330 Q800 -120 1520 330 V900"/></g>
<g fill="none" stroke="#1a1d17" stroke-width="10"><path d="M800 50 V900"/><path d="M430 140 Q520 400 520 900"/><path d="M1170 140 Q1080 400 1080 900"/><path d="M100 520 Q800 300 1500 520"/></g>
<g fill="none" stroke="#c7b48a" stroke-width="1.5" opacity=".35"><path d="M800 50 V900"/><path d="M100 520 Q800 300 1500 520"/></g>
<rect y="780" width="1600" height="120" fill="url(#fl)"/>
<g fill="#5a3e2a"><rect x="0" y="770" width="560" height="40" rx="6"/><rect x="1060" y="770" width="540" height="40" rx="6"/></g>
<g>${range(46).map((i) => { const left = i < 23; const x = left ? i * 24 + 10 : 1070 + (i - 23) * 24; const h = 80 + ((i * 53) % 140); const c = ['#2f6b39', '#3d8547', '#25542d', '#4a9a52'][i % 4]; const b = left ? -1 : 1; return `<path d="M${x} 775 q${b * 30} ${-h * 0.6} ${b * (10 + (i % 20))} ${-h} q${-b * 20} ${h * 0.5} ${-b * (10 + (i % 20))} ${h}z" fill="${c}"/>`; }).join('')}</g>
<g fill="#e7c14a"><circle cx="140" cy="660" r="9"/><circle cx="300" cy="640" r="7"/><circle cx="1260" cy="650" r="9"/><circle cx="1420" cy="630" r="7"/></g>
<g fill="#d96a7a"><circle cx="220" cy="690" r="8"/><circle cx="1340" cy="690" r="8"/></g>
<g stroke="#2f6b39" stroke-width="4" fill="none">${range(9).map((i) => { const x = 200 + i * 150; const l = 60 + ((i * 37) % 120); return `<path d="M${x} ${Math.abs(800 - x) / 3.2 + 30} q10 ${l / 2} -6 ${l}"/>`; }).join('')}</g>
<g fill="#3d8547">${range(9).map((i) => { const x = 200 + i * 150; const l = 60 + ((i * 37) % 120); return `<ellipse cx="${x - 4}" cy="${Math.abs(800 - x) / 3.2 + 30 + l}" rx="9" ry="5"/>`; }).join('')}</g>`);

const sea = svg(`
<defs><linearGradient id="sk" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="#253a6b"/><stop offset=".5" stop-color="#e9867a"/><stop offset=".62" stop-color="#ffcf8a"/></linearGradient>
<linearGradient id="se" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="#2f5f8a"/><stop offset="1" stop-color="#0c2238"/></linearGradient>
<linearGradient id="sun" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="#fff2c4"/><stop offset="1" stop-color="#ffb26b"/></linearGradient></defs>
<rect width="1600" height="560" fill="url(#sk)"/>
<circle cx="1050" cy="520" r="90" fill="url(#sun)"/>
<g fill="#fff" opacity=".22"><ellipse cx="400" cy="300" rx="260" ry="22"/><ellipse cx="1200" cy="250" rx="200" ry="16"/><ellipse cx="760" cy="380" rx="300" ry="14"/></g>
<rect y="540" width="1600" height="360" fill="url(#se)"/>
<g fill="#ffd79a" opacity=".7">${range(14).map((i) => `<rect x="${1050 - (30 + i * 14)}" y="${552 + i * 20}" width="${60 + i * 28}" height="3" rx="1.5" opacity="${1 - i / 15}"/>`).join('')}</g>
<g stroke="#9cc6e6" stroke-width="2" fill="none" opacity=".35">${range(12).map((i) => `<path d="M${(i * 170) % 1600 - 100} ${600 + i * 24} q30 -8 60 0 t60 0"/>`).join('')}</g>
<path d="M0 640 Q180 600 330 650 T560 700 L560 900 H0z" fill="#16202a"/><path d="M60 620 l10 -120 l8 120z" fill="#16202a"/>
<path fill-rule="evenodd" fill="#111317" d="M0 0H1600V900H0z M800 450 m-560 0 a560 400 0 1 0 1120 0 a560 400 0 1 0 -1120 0z"/>
<ellipse cx="800" cy="450" rx="560" ry="400" fill="none" stroke="#3a3d44" stroke-width="18"/>
<g fill="#5a5e66">${range(16).map((i) => { const a = (i / 16) * Math.PI * 2; return `<circle cx="${800 + Math.cos(a) * 580}" cy="${450 + Math.sin(a) * 418}" r="7"/>`; }).join('')}</g>`);

const aurora = svg(`
<defs><linearGradient id="ns" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="#040814"/><stop offset="1" stop-color="#0f2a33"/></linearGradient>
<linearGradient id="au" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="#7affc4" stop-opacity="0"/><stop offset=".6" stop-color="#5cf0a8" stop-opacity=".55"/><stop offset="1" stop-color="#3fb0c9" stop-opacity="0"/></linearGradient>
<linearGradient id="au2" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="#c47aff" stop-opacity="0"/><stop offset=".7" stop-color="#b07af0" stop-opacity=".35"/><stop offset="1" stop-color="#b07af0" stop-opacity="0"/></linearGradient></defs>
<rect width="1600" height="900" fill="url(#ns)"/>
<g fill="#fff">${range(140).map((i) => `<circle cx="${(i * 131.7) % 1600}" cy="${(i * 47.9) % 560}" r="${(i % 4) / 3 + 0.3}" opacity="${0.3 + (i % 6) / 9}"/>`).join('')}</g>
<path d="M-50 380 C300 120 500 420 820 200 S1300 260 1650 120 L1650 520 C1300 600 900 420 600 560 S100 520 -50 600z" fill="url(#au)"/>
<path d="M-50 300 C400 200 700 380 1000 260 S1400 180 1650 260 L1650 420 C1200 460 800 480 400 430 S0 460 -50 470z" fill="url(#au2)"/>
<path d="M0 640 L220 470 L360 580 L560 420 L760 600 L980 460 L1200 610 L1420 500 L1600 600 V900 H0z" fill="#0b1a20"/>
<rect y="680" width="1600" height="220" fill="#0a1f26"/>
<g opacity=".25"><path d="M-50 980 C300 1240 500 940 820 1160 S1300 1100 1650 1240" stroke="#5cf0a8" stroke-width="60" fill="none" transform="translate(0,-260) scale(1,.8)"/></g>
<g fill="#060d10">${range(40).map((i) => { const x = i * 42 - 10; const h = 90 + ((i * 61) % 110); return `<path d="M${x} 700 l18 -${h} l18 ${h}z"/>`; }).join('')}</g>`);

export interface Wall { id: string; name: string; image: string }

/** `name` is a key of the dictionary. */
export const WALLS: Wall[] = [
  { id: 'earth', name: 'restroom.wall.earth', image: earth },
  { id: 'sea', name: 'restroom.wall.sea', image: sea },
  { id: 'aurora', name: 'restroom.wall.aurora', image: aurora },
];

export const WALL_DEFAULT = 'earth';

/** The picture as a CSS value (a data: URL, so that no two pictures share the ids of their gradients). */
export const wallCss = (w: Wall): string => `url("data:image/svg+xml;utf8,${encodeURIComponent(w.image)}")`;
