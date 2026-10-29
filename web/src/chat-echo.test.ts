import { describe, expect, it } from 'vitest';
import { matchesEcho, reconcileEchoes, restoreEchoes, type Echo } from './chat-echo';
import type { TurnData } from './types';

const file = { id: 'a'.repeat(32) + '.png', name: 'screen.png' };
const echo = (text = 'продолжай', base = 0, id = 'send-1'): Echo => ({ id, text, base, at: 123, attachments: [] });
const turn = (n: number, q = 'продолжай', patch: Partial<TurnData> = {}): TurnData => ({ n, q, at: 0, from: 'human', a: '', ...patch });
const waiting = (list: Echo[]) => list.filter((e) => e.delivered === undefined);

describe('planner echo and native transcript', () => {
  it('waits for the actual message, not a reply, substring or an old identical question', () => {
    const e = echo();
    expect(waiting(reconcileEchoes([e], [turn(0), turn(1, 'не продолжай'), turn(2, 'продолжай', { from: 'board' })]))).toEqual([e]);
    expect(waiting(reconcileEchoes([e], [turn(1)]))).toEqual([]);
  });
  it('acknowledges mid-turn input without waiting for the Coordinator to finish', () => {
    const e = echo();
    const transcript = [turn(0, 'первый вопрос', { a: 'Работаю', doing: 'Bash' }), turn(1)];
    expect(waiting(reconcileEchoes([e], transcript))).toEqual([]);
    expect(transcript.map((t) => t.q)).toEqual(['первый вопрос', 'продолжай']);
  });
  it('matches image markers and the header removed by Claude, requiring every image', () => {
    const e = { ...echo('По лимитам\n\nещё мысль'), attachments: [file, { ...file, id: 'b'.repeat(32) + '.png' }] };
    const q = '[Image #26] [Image #27]По лимитам\nещё мысль\nВложения (файлы на машине с доской):';
    expect(matchesEcho(e, turn(1, q, { images: ['image/png'] }))).toBe(false);
    expect(matchesEcho(e, turn(1, q, { images: ['image/png', 'image/png'] }))).toBe(true);
    expect(matchesEcho({ ...echo(''), attachments: [file] }, turn(1, '[Image #2]', { images: ['image/png'] }))).toBe(true);
  });
  it('compares surviving attachment IDs, not just the count', () => {
    const e = { ...echo(), attachments: [file] };
    const q = `продолжай\nКартинки (файлы на машине с доской):\n/attachments/${file.id}`;
    expect(matchesEcho(e, turn(1, q))).toBe(true);
    expect(matchesEcho(e, turn(1, q.replace(file.id, 'b'.repeat(32) + '.png')))).toBe(false);
  });
  it('keeps two consecutive identical sends separate across partial pages and reconnects', () => {
    const es = [echo(), echo('продолжай', 0, 'send-2')];
    const first = reconcileEchoes(es, [turn(1)]);
    expect(waiting(first).map((e) => e.id)).toEqual(['send-2']);
    expect(waiting(reconcileEchoes(first, [turn(1)]))).toHaveLength(1);
    expect(waiting(reconcileEchoes(first, [turn(2)]))).toEqual([]);
  });
  it('uses transcript positions even when timestamps go backwards or browser clocks differ', () => {
    const es = [echo('один'), echo('два', 0, 'send-2')];
    const got = reconcileEchoes(es, [turn(1, 'два', { at: 500 }), turn(2, 'один', { at: 1 })]);
    expect(got.map((e) => e.delivered)).toEqual([2, 1]);
  });
  it('restores a burst on reload without losing receipts or consuming a delivered occurrence twice', () => {
    const es = [echo(), echo('продолжай', 0, 'send-2'), echo('три', 0, 'send-3'), echo('четыре', 0, 'send-4'), echo('пять', 0, 'send-5')];
    const saved = JSON.stringify(reconcileEchoes(es, [turn(1)]));
    const restored = restoreEchoes(saved);
    expect(waiting(reconcileEchoes(restored, [turn(1)]))).toHaveLength(4);
    const got = reconcileEchoes(restored, [turn(1), turn(2), turn(3, 'три'), turn(4, 'четыре'), turn(5, 'пять')]);
    expect(waiting(got)).toEqual([]);
    expect(reconcileEchoes(got, [turn(1), turn(2)])).toEqual(got);
  });
  it('keeps an unobserved message indefinitely and ignores malformed storage', () => {
    expect(waiting(reconcileEchoes([echo()], [turn(1, 'ещё работаю')]))).toHaveLength(1);
    for (const raw of [null, '{', '{}', '[null,1,{"id":"bad"}]']) expect(restoreEchoes(raw)).toEqual([]);
  });
});
