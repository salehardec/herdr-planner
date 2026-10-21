/* Labels over the scene: plain DOM, a handful at a time. Candidates come with
   a priority and a screen position; the most important ones that do not
   collide with a better one are shown, up to a budget. */

import type { Status } from '../types';
import { esc } from '../util/format';
import { scaleK } from '../scale';

export interface LabelCandidate {
  key: string;
  kind: 'planet' | 'universe' | 'quasar' | 'galaxy' | 'moon' | 'bridge';
  x: number;
  y: number;
  /** Pixels between the point and the label's bottom edge. */
  lift: number;
  text: string;
  sub?: string;
  status?: Status;
  priority: number;
  /** Always shown, even over others. */
  force?: boolean;
  selected?: boolean;
  hovered?: boolean;
  drop?: boolean;
  dim?: boolean;
  /** Extra marker text on the right (lag badge, «ждёт N»). */
  badge?: string;
  off?: boolean;
  /** Full card (key + title + line) instead of a chip. */
  full?: boolean;
  line?: string;
}

interface Box { l: number; t: number; r: number; b: number }

interface Slot {
  el: HTMLElement;
  last: string;
  html: string;
}

const CHAR_W = 6.7;

export class Labels {
  private slots = new Map<string, Slot>();
  onClick: (key: string) => void = () => {};
  count = 0;
  /** Candidates left out because a better label sits there. */
  collisions = 0;
  /** Shown labels that still overlap (only forced ones can, when there is no room to nudge). */
  overlaps = 0;

  constructor(private root: HTMLElement) {
    root.addEventListener('click', (e) => {
      const el = (e.target as HTMLElement).closest<HTMLElement>('[data-lkey]');
      if (el?.dataset.lkey) this.onClick(el.dataset.lkey);
    });
  }

  /** The size a label takes on the screen: its text grows with the interface. */
  private sizeOf(c: LabelCandidate): { w: number; h: number } {
    const { w, h } = this.baseSize(c);
    const k = scaleK();
    return { w: w * k, h: h * k };
  }

  private baseSize(c: LabelCandidate): { w: number; h: number } {
    if (c.full) {
      const w = Math.min(260, Math.max(120, Math.max((c.text.length + 8) * CHAR_W, (c.sub?.length ?? 0) * 6.2, (c.line?.length ?? 0) * 6)));
      return { w, h: 44 + (c.line ? 14 : 0) };
    }
    const w = (c.text.length + (c.sub ? c.sub.length + 1 : 0) + (c.badge ? c.badge.length + 2 : 0) + 3) * CHAR_W + 14;
    return { w: Math.min(240, w), h: 20 };
  }

  render(cands: LabelCandidate[], budget: number, width: number, height: number): void {
    // The forced ones (the selected, the hovered, the Quasar, the galaxies) go first and are nudged apart from each
    // other; everything else must keep clear of all of them, so no two shown labels overlap.
    cands.sort((a, b) => Number(!!b.force) - Number(!!a.force) || b.priority - a.priority);
    const taken: Box[] = [];
    const hits = (box: Box): boolean => taken.some((o) => Math.min(o.r, box.r) - Math.max(o.l, box.l) > -3 && Math.min(o.b, box.b) - Math.max(o.t, box.t) > -3);
    const shown = new Set<string>();
    let count = 0;
    this.collisions = 0;
    this.overlaps = 0;
    for (const c of cands) {
      if (c.x < -40 || c.x > width + 40 || c.y < -20 || c.y > height + 40) continue;
      const { w, h } = this.sizeOf(c);
      let box: Box = { l: c.x - w / 2, r: c.x + w / 2, b: c.y - c.lift * scaleK(), t: c.y - c.lift * scaleK() - h };
      if (c.force) {
        for (const dy of [0, -(h + 3), h + 3, -2 * (h + 3), 2 * (h + 3), -3 * (h + 3)]) {
          const shifted = { ...box, t: box.t + dy, b: box.b + dy };
          if (!hits(shifted)) { box = shifted; break; }
        }
        if (hits(box)) this.overlaps++;
      } else {
        if (count >= budget) continue;
        if (hits(box)) {
          this.collisions++;
          continue;
        }
      }
      taken.push(box);
      count++;
      shown.add(c.key);
      this.apply(c, box.l, box.t, w);
    }
    for (const [key, slot] of this.slots) {
      if (!shown.has(key) && slot.el.style.display !== 'none') slot.el.style.display = 'none';
    }
    this.count = count;
  }

  private apply(c: LabelCandidate, left: number, top: number, w: number): void {
    let slot = this.slots.get(c.key);
    if (!slot) {
      const el = document.createElement('div');
      el.dataset.lkey = c.key;
      this.root.appendChild(el);
      slot = { el, last: '', html: '' };
      this.slots.set(c.key, slot);
    }
    const cls = `lb ${c.kind}${c.status ? ` s-${c.status}` : ''}${c.selected ? ' sel' : ''}${c.hovered ? ' hover' : ''}${c.drop ? ' drop' : ''}${c.dim ? ' dim' : ''}${c.off ? ' off' : ''}${c.full ? ' full' : ''}`;
    const html = c.full
      ? `<span class="in"><b class="k">${esc(c.text)}</b>${c.badge ? `<span class="bd">${esc(c.badge)}</span>` : ''}</span><span class="ttl">${esc(c.sub ?? '')}</span>${c.line ? `<span class="line">${esc(c.line)}</span>` : ''}`
      : `<span class="in"><b class="k">${esc(c.text)}</b>${c.sub ? `<span class="sub">${esc(c.sub)}</span>` : ''}${c.badge ? `<span class="bd">${esc(c.badge)}</span>` : ''}</span>`;
    if (slot.html !== html) { slot.el.innerHTML = html; slot.html = html; }
    if (slot.el.className !== cls) slot.el.className = cls;
    const style = `${Math.round(left)}|${Math.round(top)}|${Math.round(w)}`;
    if (slot.last !== style) {
      slot.el.style.transform = `translate(${Math.round(left)}px,${Math.round(top)}px)`;
      slot.last = style;
    }
    if (slot.el.style.display === 'none') slot.el.style.display = '';
  }

  clear(): void {
    for (const s of this.slots.values()) s.el.remove();
    this.slots.clear();
  }
}
