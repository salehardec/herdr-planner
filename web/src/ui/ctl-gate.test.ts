import { describe, expect, it } from 'vitest';
import type { RunnerContainer } from '../types';
import { ctlGate, grep, logsGate, stripAnsi } from './ctl-gate';

const c = (over: Partial<RunnerContainer>): RunnerContainer => ({ id: 'a'.repeat(64), name: 'pg', state: 'running', class: 'forgotten', task: { id: 't1', tag: 'X-1', status: 'done', title: '', finished_ms: 0 }, ...over } as RunnerContainer);

describe('ctlGate', () => {
  it('locks a foreign container for everything, logs included', () => {
    for (const a of ['stop', 'restart', 'delete'] as const) expect(ctlGate(c({ class: 'foreign' }), a, 't1')).toBe('ctl.noForeign');
    expect(logsGate(c({ class: 'foreign' }))).toBe('ctl.noForeign');
  });
  it('reads an unsure one but does not touch it', () => {
    expect(logsGate(c({ class: 'unsure' }))).toBe('');
    expect(ctlGate(c({ class: 'unsure' }), 'restart', 't1')).toBe('ctl.noUnsure');
  });
  it('lets a task in work be driven only from its own Focus', () => {
    const a = c({ class: 'active' });
    expect(ctlGate(a, 'stop', 't1')).toBe('');
    expect(ctlGate(a, 'stop', 't2')).toBe('ctl.noActive');
    expect(ctlGate(a, 'restart', '')).toBe('ctl.noActive');
  });
  it('follows the state: stop a running one, delete a stopped one', () => {
    expect(ctlGate(c({ state: 'exited' }), 'stop', '')).toBe('ctl.alreadyStopped');
    expect(ctlGate(c({ state: 'running' }), 'delete', '')).toBe('ctl.stopFirst');
    expect(ctlGate(c({ state: 'exited' }), 'delete', '')).toBe('');
    expect(ctlGate(c({ state: 'exited' }), 'restart', '')).toBe('');
  });
});

describe('the log text', () => {
  it('drops colour codes', () => {
    expect(stripAnsi(`${String.fromCharCode(27)}[31mred${String.fromCharCode(27)}[0m ok`)).toBe('red ok');
  });
  it('keeps the lines that hold the words, in any case', () => {
    const r = grep('INFO a\nERROR b\nwarn c\n', 'error');
    expect(r).toEqual({ lines: ['ERROR b'], total: 3 });
    expect(grep('', 'x')).toEqual({ lines: [], total: 0 });
    expect(grep('a\nb', '').lines).toEqual(['a', 'b']);
  });
});
