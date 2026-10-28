import { describe, expect, it } from 'vitest';
import type { MemEntry } from './memory-api';
import { daysSince, dmy, marked, stuckOf, withKind } from './memory-lint';

const e = (id: string, patch: Partial<MemEntry> = {}): MemEntry => ({ id, title: id, description: '', kind: 'decision', body: '', added: '', source: '', at: 0, ...patch });

describe('memory lint', () => {
  const list = [
    e('a', { lint: [{ kind: 'oneoff', detail: 'номер задачи PLN-31' }] }),
    e('b', { lint: [{ kind: 'overdue', detail: 'пересмотреть до 2026-10-01' }, { kind: 'duplicate', detail: 'похожа на «a»', with: 'a' }] }),
    e('c'),
    e('d', { lint: [] }),
  ];

  it('counts the marked entries and those of one kind', () => {
    expect(marked(list).map((x) => x.id)).toEqual(['a', 'b']);
    expect(withKind(list, 'overdue').map((x) => x.id)).toEqual(['b']);
    expect(withKind(list, 'old')).toEqual([]);
  });

  it('counts whole days, never a negative one', () => {
    const now = Date.UTC(2026, 9, 28, 12);
    expect(daysSince(Date.UTC(2026, 9, 22, 12), now)).toBe(6);
    expect(daysSince(Date.UTC(2026, 9, 22, 18), now)).toBe(5);
    expect(daysSince(0, now)).toBe(0);
    expect(daysSince(now + 1000, now)).toBe(0);
  });

  it('writes a date the way the page does', () => {
    expect(dmy('2026-10-22')).toBe('22.10.2026');
    expect(dmy('скоро')).toBe('скоро');
  });

  it('finds the stuck drafts', () => {
    expect(stuckOf([e('a', { stuck: true, draft: 'new' }), e('b'), e('c', { stuck: true, draft: 'deleted' })])).toEqual({ n: 2, ids: ['a', 'c'] });
  });
});
