import { describe, expect, it } from 'vitest';
import { markFrom, markLabel, markTip, marksOf } from './crew-roles';
import type { RoleMark } from '../types';

const mark = (over: Partial<RoleMark> = {}): RoleMark => ({ role: 'r1', name: 'Дизайнер', how: 'worker', by: 'chosen', rule: '', of: '', ...over });

describe('the crew roles on a row', () => {
  it('says the role by its name, with how it went on when it is not the worker', () => {
    expect(markLabel(mark())).toBe('Дизайнер');
    expect(markLabel(mark({ name: 'Ревьюер безопасности', how: 'after' }))).toBe('Ревьюер безопасности · после');
    expect(markLabel(mark({ name: 'Ревьюер безопасности', how: 'instead' }))).toBe('Ревьюер безопасности · вместо ревьюера');
    expect(markLabel(mark({ name: 'Ревьюер безопасности', how: 'followup', of: 'planner/12' }), () => 'PLN-12')).toBe('Ревьюер безопасности · проверка PLN-12');
  });

  it('says where it came from: by hand, or by which rule', () => {
    expect(markFrom(mark())).toBe('выбрана вручную');
    expect(markFrom(mark({ by: 'rule', rule: 'репозиторий web' }))).toBe('по правилу: репозиторий web');
    expect(markFrom(mark({ by: 'rule', rule: '' }))).toBe('по правилу: правило удалено');
  });

  it('puts the three in the tooltip', () => {
    expect(markTip(mark({ by: 'rule', rule: 'ветка: ui/*' }))).toBe('Дизайнер · исполнитель · по правилу: ветка: ui/*');
    expect(markTip(mark({ how: 'followup', of: 'a/1' }), () => 'A-1')).toContain('проверка A-1');
  });

  it('reads the marks of a task that has none', () => {
    expect(marksOf({})).toEqual([]);
    expect(marksOf({ crew_roles: [mark()] })).toHaveLength(1);
  });
});
