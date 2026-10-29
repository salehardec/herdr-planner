import { describe, expect, it } from 'vitest';
import { baseKeys, freshNotes } from './notes';

const r = (key: string, type = 'question') => ({ key, type });

describe('notices in the rest room', () => {
  it('shows only what came after the room was entered', () => {
    const before = [r('question:a/1'), r('plan:a/2')];
    const base = baseKeys(before);
    expect(freshNotes(base, new Set(), before)).toEqual([]);
    const now = [...before, r('failed:a/3', 'failed')];
    expect(freshNotes(base, new Set(), now)).toEqual([r('failed:a/3', 'failed')]);
  });

  it('drops the ones sent away with «Позже», and never shows the Coordinator\'s options or a memory proposal', () => {
    const now = [r('question:a/4'), r('permission:a/5', 'permission'), r('coordinator-options', 'coordinator-options'), r('memory:a/6', 'memory'), r('coordinator-dialog', 'coordinator-dialog')];
    const got = freshNotes(new Set(), new Set(['question:a/4']), now).map((x) => x.key);
    expect(got).toEqual(['permission:a/5', 'coordinator-dialog']);
  });
});
