import { describe, expect, it } from 'vitest';
import type { AgentUsage, LimitWindow } from '../types';
import { ago, barAgents, countdown, expectedLeft, leftPct, limitSummary, paceLine, tabAgents, tightest, usedPct } from './limits';

const win = (percent: number, extra: Partial<LimitWindow> = {}): LimitWindow => ({ label: 'w', percent, severity: 'ok', resets_at_ms: 1, ...extra });
const agent = (id: string, limits: LimitWindow[], extra: Partial<AgentUsage> = {}): AgentUsage => ({ id, name: id, limits, ...extra });

describe('left and used', () => {
  it('rounds the share and keeps it between 0 and 100', () => {
    expect(usedPct(win(0.07))).toBe(7);
    expect(leftPct(win(0.07))).toBe(93);
    expect(leftPct(win(1.4))).toBe(0);
    expect(leftPct(win(-1))).toBe(100);
  });
});

describe('the countdown', () => {
  it('names the coarsest two units, rounded up to the minute', () => {
    expect(countdown(31 * 60000)).toEqual({ d: 0, h: 0, m: 31 });
    expect(countdown((60 + 33) * 60000 - 1000)).toEqual({ d: 0, h: 1, m: 33 });
    expect(countdown((5 * 1440 + 60) * 60000)).toEqual({ d: 5, h: 1, m: 0 });
    expect(countdown(-5)).toEqual({ d: 0, h: 0, m: 0 });
  });
});

describe('the pace line, as in the card of the screenshot', () => {
  it('62 % in reserve and it lasts until the reset', () => {
    expect(paceLine(win(0.07, { reserve: 62.4 }))).toEqual({ standing: 'reserve', n: 62, runsOutMs: null });
  });
  it('a deficit, with the time it runs out', () => {
    expect(paceLine(win(0.6, { reserve: -40, runs_out_ms: 2400000 }))).toEqual({ standing: 'deficit', n: 40, runsOutMs: 2400000 });
  });
  it('on pace within half a point either way', () => {
    expect(paceLine(win(0.3, { reserve: 0.4 }))?.standing).toBe('pace');
    expect(paceLine(win(0.3, { reserve: -0.4 }))?.standing).toBe('pace');
    expect(paceLine(win(0.3, { reserve: 0.5 }))?.standing).toBe('reserve');
  });
  it('a spent window ran out, whatever the margin says', () => {
    expect(paceLine(win(1, { reserve: -3 }))?.standing).toBe('ran_out');
  });
  it('is nothing when the pace could not be judged', () => {
    expect(paceLine(win(0.1))).toBeNull();
    expect(paceLine(win(0.1, { reserve: null }))).toBeNull();
  });
  it('puts the tick where an even spend would have left it', () => {
    // 7 % used, 62 % in reserve: an even spend would have used 69 %, so 31 % would be left
    expect(expectedLeft(win(0.07, { reserve: 62 }))).toBe(31);
    expect(expectedLeft(win(0.07))).toBeNull();
  });
});

describe('what the bar shows', () => {
  it('the two tightest windows, in the provider\'s own order', () => {
    const ws = [win(0.05, { label: 'session' }), win(0.12, { label: 'weekly' }), win(0.05, { label: 'fable' })];
    expect(tightest(ws).map((w) => w.label)).toEqual(['session', 'weekly']);
    const kimi = [win(0.35, { label: 'session' }), win(0.04, { label: 'weekly' }), win(0.21, { label: 'total' })];
    expect(tightest(kimi).map((w) => w.label)).toEqual(['session', 'total']);
    expect(tightest(kimi, 1).map((w) => w.label)).toEqual(['session']);
  });

  it('lists the providers by how close they are to a limit, the picked one first, and leaves out those without numbers', () => {
    const list = [
      agent('codex', [win(0)]),
      agent('claude', [win(0.07)]),
      agent('kimi', [win(0.35)]),
      agent('deepseek', [], { balance: { remaining: 11.92, currency: 'USD' } }),
      agent('omp', []),
      agent('grok', [], { note: 'нет данных' }),
    ];
    expect(barAgents(list, '').map((a) => a.id)).toEqual(['kimi', 'claude', 'codex', 'deepseek']);
    expect(barAgents(list, 'deepseek').map((a) => a.id)).toEqual(['deepseek', 'kimi', 'claude', 'codex']);
  });

  it('puts the tabs in the registry\'s order and keeps a provider that is signed in but gave nothing', () => {
    const list = [
      agent('deepseek', [], { balance: { remaining: 1, currency: 'USD' } }),
      agent('kimi', [], { why: 'rejected' }),
      agent('pi', []),
      agent('claude', [win(0.1)]),
      agent('codex', [win(0.1)]),
      agent('omp', [], { source: 'omarchy' }),
    ];
    expect(tabAgents(list).map((a) => a.id)).toEqual(['codex', 'claude', 'kimi', 'deepseek']);
  });
});

describe('how long ago', () => {
  it('says now under a minute, then minutes, hours, days', () => {
    expect(ago(20000)).toEqual({ n: 0, unit: 'now' });
    expect(ago(5 * 60000)).toEqual({ n: 5, unit: 'min' });
    expect(ago(3 * 3600000)).toEqual({ n: 3, unit: 'h' });
    expect(ago(50 * 3600000)).toEqual({ n: 2, unit: 'd' });
  });
});

describe('compact subscription summary', () => {
  it('uses the worst live window across providers and ignores expired windows and stats', () => {
    const list = [agent('claude', [win(1), win(.81, { resets_at_ms: 200 })]), agent('codex', [win(.96, { resets_at_ms: 200 })]), agent('pi', [win(1, { resets_at_ms: 200 })])];
    expect(limitSummary(list, 100)).toEqual({ percent: 96, level: 'crit', unavailable: false });
  });
  it('warns about a signed-in provider without readings alongside a healthy one', () => {
    expect(limitSummary([agent('claude', [win(.1, { resets_at_ms: 200 })]), agent('codex', [], { why: 'unauthorized' })], 100)).toEqual({ percent: 10, level: 'warn', unavailable: true });
  });
  it('warns on stale readings, but does not flag stats or unconnected providers', () => {
    expect(limitSummary([agent('claude', [win(.1, { resets_at_ms: 200 })], { stale: true }), agent('omp', [], { source: 'omarchy' })], 100).unavailable).toBe(true);
    expect(limitSummary([agent('claude', []), agent('omp', [], { source: 'omarchy' })], 100)).toEqual({ percent: null, level: 'ok', unavailable: false });
  });
  it('shows no invented percentage for balances and flags an exhausted balance', () => {
    expect(limitSummary([agent('deepseek', [], { balance: { remaining: 0, currency: 'USD' } })], 100)).toEqual({ percent: null, level: 'crit', unavailable: false });
  });
});
