/* The world search on the client: fuzzy (typos, other keyboard layout), grouped by kind,
   instant. The server adds what the page does not hold (the old chat, memory, closing notes). */
import uFuzzy from '@leeoniya/ufuzzy';
import type { GalaxyView, SearchHit, Snapshot, TurnData } from '../types';

export interface Doc {
  kind: SearchHit['kind'];
  id: string;
  path: string;
  title: string;
  text: string;
  status: string;
}

export const GROUPS: SearchHit['kind'][] = ['galaxy', 'quasar', 'universe', 'planet', 'moon', 'runner', 'todo', 'memory', 'closing', 'chat'];
const PER_GROUP = 5;
const TOTAL = 28;

const RU = 'йцукенгшщзхъфывапролджэячсмитьбю';
const EN = "qwertyuiop[]asdfghjkl;'zxcvbnm,.";

/** The text typed on the wrong layout, turned back (both ways). */
export function swapLayout(s: string): string {
  const out: string[] = [];
  for (const ch of s.toLowerCase()) {
    const a = EN.indexOf(ch);
    const b = RU.indexOf(ch);
    out.push(a >= 0 ? RU[a] : b >= 0 ? EN[b] : ch);
  }
  return out.join('');
}

export function buildDocs(
  view: GalaxyView, snap: Snapshot | null, turns: TurnData[], labels: { quasar: string; todo: string; chat: string },
): Doc[] {
  const docs: Doc[] = [];
  const host = snap?.host ?? '';
  docs.push({ kind: 'galaxy', id: '', path: '', title: host, text: 'galaxy галактика', status: 'working' });
  docs.push({ kind: 'quasar', id: 'quasar', path: host, title: labels.quasar, text: 'coordinator координатор quasar квазар', status: snap?.coordinator.status ?? '' });
  for (const m of snap?.coordinator.subagents ?? []) {
    docs.push({ kind: 'moon', id: `quasar#${m.id}`, path: `${host} › ${labels.quasar}`, title: m.role, text: `${m.title} ${m.doing}`, status: m.state });
  }
  for (const p of view.projects) {
    docs.push({ kind: 'universe', id: p.name, path: host, title: `${p.name} ${p.key}`, text: p.origin, status: p.quiet ? 'queued' : 'working' });
  }
  for (const t of view.tasks) {
    docs.push({
      kind: 'planet', id: t.id, path: `${host} › ${t.repo} › ${t.tag}`, title: `${t.tag} ${t.title}`,
      text: `${t.description} ${t.msg} ${t.board_label} ${t.app} ${t.worker_agent} ${t.model} ${t.label}`, status: t.status,
    });
    for (const s of t.subagents) {
      docs.push({ kind: 'moon', id: `${t.id}#${s.id}`, path: `${host} › ${t.repo} › ${t.tag}`, title: s.role, text: `${s.title} ${s.doing}`, status: s.state });
    }
  }
  for (const r of snap?.runners ?? []) {
    docs.push({
      kind: 'runner', id: r.id, path: r.target, title: r.id,
      text: `${r.target} ${r.note} ${r.containers.map((c) => c.name).join(' ')}`,
      status: r.ok ? 'working' : 'failed',
    });
  }
  for (const it of snap?.todo ?? []) {
    docs.push({ kind: 'todo', id: it.id, path: `${host} › ${labels.todo}`, title: it.text, text: it.to, status: '' });
  }
  for (const turn of turns) {
    docs.push({ kind: 'chat', id: String(turn.n), path: labels.chat, title: turn.q, text: turn.a.slice(0, 400), status: '' });
  }
  return docs;
}

const uf = new uFuzzy({
  unicode: true,
  interSplit: "[^\\p{L}\\d']+",
  intraSplit: '\\p{Ll}\\p{Lu}',
  intraBound: '\\p{L}\\d|\\d\\p{L}|\\p{Ll}\\p{Lu}',
  intraChars: "[\\p{L}\\d']",
  intraContr: "'\\p{L}{1,2}\\b",
  intraMode: 1,
});

export interface Found { doc: Doc; score: number }

/** Finds docs: title first (strong), then the whole text. Grouped and trimmed. */
export function searchDocs(docs: Doc[], query: string): Found[] {
  const q = query.trim();
  if (!q) return [];
  const needles = [q];
  const swapped = swapLayout(q);
  if (swapped !== q.toLowerCase()) needles.push(swapped);
  const best = new Map<number, number>();
  const titles = docs.map((d) => d.title);
  const full = docs.map((d) => `${d.title} ${d.text}`);
  for (const needle of needles) {
    for (const [hay, weight] of [[titles, 0], [full, 40]] as const) {
      const [idxs, info, order] = uf.search(hay as string[], needle, 1, 1e4);
      if (!idxs) continue;
      const list = info && order ? order.map((o) => info.idx[o]) : idxs;
      list.slice(0, 400).forEach((idx, rank) => {
        const score = weight + rank;
        const prev = best.get(idx);
        if (prev === undefined || score < prev) best.set(idx, score);
      });
    }
    // A task key typed in part («alp7», «ALP-7»): compare without dashes and case.
    const flat = needle.toLowerCase().replace(/[\s-]/g, '');
    if (flat.length >= 2) {
      docs.forEach((d, idx) => {
        if (d.title.toLowerCase().replace(/[\s-]/g, '').startsWith(flat)) {
          const prev = best.get(idx);
          if (prev === undefined || prev > -1) best.set(idx, -1);
        }
      });
    }
  }
  const found: Found[] = [...best.entries()].map(([idx, score]) => ({ doc: docs[idx], score }));
  found.sort((a, b) => a.score - b.score);
  const out: Found[] = [];
  const per = new Map<string, number>();
  for (const f of found) {
    const n = per.get(f.doc.kind) ?? 0;
    if (n >= PER_GROUP) continue;
    per.set(f.doc.kind, n + 1);
    out.push(f);
    if (out.length >= TOTAL) break;
  }
  out.sort((a, b) => GROUPS.indexOf(a.doc.kind) - GROUPS.indexOf(b.doc.kind) || a.score - b.score);
  return out;
}
