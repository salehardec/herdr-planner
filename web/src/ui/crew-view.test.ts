import { describe, expect, it } from 'vitest';
import { chainOf, chainValue, downPrograms, leadOf, modelOptions, momentLabel, offerable, presetKey, presetKeys, rolesOrEmpty, sameRoles, summary, untilLabel } from './crew-view';
import type { ModelsData, ProgramAvail, StationData, StationProgram, StationSetting } from './station-api';

const avail = (over: Partial<ProgramAvail> = {}): ProgramAvail => ({ state: 'ok', until_ms: 0, until: '', reason: '', source: '', ...over });

const program = (name: string, a: Partial<ProgramAvail> = {}, installed = true): StationProgram => ({
  name, installed, version: '1.0', signed_in: null, line: '',
  state: { signed_in: null, account: '', unknown_why: '', avail: avail(a), windows: [], limit_why: '', stale: false },
  providers: [], roles: [], tasks: [], models: { count: 0, loading: false, listed: false, suggested: 0 },
});

const setting = (name: string, value: string): StationSetting => ({ name, kind: 'work', value, from: 'you', place: 'you', own: true, note: '' });

const data = (programs: StationProgram[]): StationData => ({
  ok: true, level: 'you', repo: '', repos: [], version: '1', programs, light: 'omp/deepseek/deepseek-v4-flash', failover: {},
  presets: { list: [], active: null },
  settings: [
    setting('coordinator.agent', 'claude/opus'), setting('plan.agent', 'none'), setting('reviewer.agent', 'claude/opus'),
    setting('worker.agent', 'codex/gpt-6-luna'),
  ],
});

describe('the summary line', () => {
  it('names who leads every role; no planner is an empty lead', () => {
    const s = summary(data([program('claude'), program('codex')]));
    expect(s.leads.map((l) => `${l.role}:${l.agent}`)).toEqual([
      'coordinator:claude/opus', 'plan:', 'reviewer:claude/opus', 'worker:codex/gpt-6-luna', 'watcher:', 'light:omp/deepseek/deepseek-v4-flash',
    ]);
    expect(s.down).toEqual([]);
  });

  it('lists the subscriptions that are out of allowance or signed out, with when they come back', () => {
    const down = downPrograms([
      program('claude', { state: 'limited', until: '17:00' }),
      program('codex', { state: 'signed_out' }),
      program('omp', { state: 'unknown' }),
      program('pi', { state: 'limited' }, false),
      program('kimi'),
    ]);
    expect(down).toEqual([
      { program: 'claude', kind: 'limited', until: '17:00', untilMs: 0 },
      { program: 'codex', kind: 'signed_out', until: '', untilMs: 0 },
    ]);
  });

  it('reads a role that nothing sets as empty', () => {
    expect(leadOf({ settings: [], light: '' }, 'worker')).toBe('');
  });
});

describe('until when', () => {
  const now = new Date(2026, 9, 27, 12, 0).getTime();
  it('prefers the board\'s own words', () => {
    expect(untilLabel({ until: '17:00', until_ms: now + 1000 }, now)).toBe('17:00');
  });
  it('shows only the time for today and the day for later', () => {
    const today = untilLabel({ until: '', until_ms: new Date(2026, 9, 27, 17, 20).getTime() }, now);
    expect(today).toMatch(/17[:.]20/);
    expect(today).not.toMatch(/окт|Oct/);
    const later = momentLabel(new Date(2026, 10, 3, 5, 0).getTime(), now);
    expect(later).toMatch(/ноя|Nov/);
  });
  it('says nothing when the moment has passed or is not known', () => {
    expect(untilLabel({ until: '', until_ms: now - 1 }, now)).toBe('');
    expect(untilLabel({ until: '', until_ms: 0 }, now)).toBe('');
  });
});

describe('fallback chains', () => {
  it('splits the board\'s value into items and back', () => {
    expect(chainOf('codex, claude/claude-opus-5-5')).toEqual(['codex', 'claude/claude-opus-5-5']);
    expect(chainOf('none')).toEqual([]);
    expect(chainOf('')).toEqual([]);
    expect(chainValue(['codex', 'omp/deepseek/deepseek-v4-flash'])).toBe('codex, omp/deepseek/deepseek-v4-flash');
    expect(chainValue([])).toBe('none');
  });

  it('offers a subscription that is installed and not signed out; a limited one still counts', () => {
    expect(offerable(program('claude'))).toBe(true);
    expect(offerable(program('claude', { state: 'limited' }))).toBe(true);
    expect(offerable(program('codex', { state: 'signed_out' }))).toBe(false);
    expect(offerable(program('pi', {}, false))).toBe(false);
  });
});

describe('presets in the editor', () => {
  it('fills every role', () => {
    const r = rolesOrEmpty({ worker: { agent: 'codex', effort: 'high', fallback: 'none' } });
    expect(Object.keys(r)).toEqual(['coordinator', 'plan', 'reviewer', 'worker', 'watcher', 'light']);
    expect(r.plan).toEqual({ agent: '', effort: '', fallback: '' });
  });
  it('holds the own roles next to the standard ones, under role:<id>', () => {
    expect(presetKey('r1f')).toBe('role:r1f');
    expect(presetKeys(['r1f'])).toEqual(['coordinator', 'plan', 'reviewer', 'worker', 'watcher', 'light', 'role:r1f']);
    const saved = rolesOrEmpty({ 'role:r1f': { agent: 'codex', effort: '', fallback: 'none' } }, ['r1f', 'r2a']);
    expect(saved['role:r1f']).toEqual({ agent: 'codex', effort: '', fallback: 'none' });
    expect(saved['role:r2a']).toEqual({ agent: '', effort: '', fallback: '' });
    expect(sameRoles(saved, { ...saved, 'role:r1f': { agent: 'codex', effort: 'high', fallback: 'none' } }, ['r1f', 'r2a'])).toBe(false);
    // a role the editor does not know about is not compared
    expect(sameRoles(saved, { ...saved, 'role:r1f': { agent: 'x', effort: '', fallback: '' } })).toBe(true);
  });
  it('knows an edit from the saved one', () => {
    const saved = rolesOrEmpty({ worker: { agent: 'codex', effort: '', fallback: '' } });
    expect(sameRoles(saved, rolesOrEmpty({ worker: { agent: 'codex', effort: '', fallback: '' } }))).toBe(true);
    expect(sameRoles(saved, { ...saved, worker: { agent: 'codex', effort: 'high', fallback: '' } })).toBe(false);
  });
});

describe('the model picker', () => {
  const models = (over: Partial<ModelsData> = {}): ModelsData => ({
    ok: true, program: 'omp', names: [], effort: [], note: '', loading: false, listed: false, suggested: [], providers: [], ...over,
  });

  it('offers the listed models and nothing typed by hand', () => {
    const o = modelOptions('claude', models({ names: ['claude/opus', 'claude/haiku'], listed: true }), 'claude/opus');
    expect(o.map((x) => x.value)).toEqual(['claude/opus', 'claude/haiku']);
  });

  it('offers a program with no list its own default and the models it was used with', () => {
    const o = modelOptions('omp', models({ suggested: ['omp/deepseek/deepseek-v4-flash'] }), '');
    expect(o).toEqual([
      { value: 'omp/deepseek/deepseek-v4-flash', kind: 'model' },
      { value: 'omp', kind: 'default' },
    ]);
    expect(modelOptions('kimi', models(), '').map((x) => x.value)).toEqual(['kimi']);
  });

  it('keeps a value that is set but not offered, at the top; and the program alone first for a chain', () => {
    const o = modelOptions('claude', models({ names: ['claude/opus'], listed: true }), 'claude/old', true);
    expect(o.map((x) => `${x.kind}:${x.value}`)).toEqual(['current:claude/old', 'default:claude', 'model:claude/opus']);
    expect(modelOptions('codex', models({ names: ['codex/gpt'], listed: true }), 'claude/opus').map((x) => x.value)).toEqual(['codex/gpt']);
  });

  it('offers nothing while the answer is not there', () => {
    expect(modelOptions('codex', null, '')).toEqual([]);
  });
});
