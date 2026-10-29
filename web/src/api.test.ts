import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { Snapshot } from './types';

const store = vi.hoisted(() => ({
  snap: { value: null as Snapshot | null }, chat: { value: null }, details: { value: {} },
  gate: { value: null }, lastError: { value: '' }, live: { value: true }, setLoad: vi.fn(), GATE_POLL_MS: 3000,
}));
vi.mock('./store', () => store);
vi.mock('./i18n', () => ({ t: (key: string) => key }));
vi.mock('./toast', () => ({ toast: vi.fn() }));

class Stream extends EventTarget {
  static current: Stream;
  constructor(_url: string) { super(); Stream.current = this; }
  close() {}
  state(s: Snapshot) { this.dispatchEvent(Object.assign(new Event('state'), { data: JSON.stringify(s) })); }
}
const snapshot = (seq: number, at: number, archived = false): Snapshot => ({
  seq, at, chat_rev: seq,
  links: archived ? [{ from: 'gamma/2', to: 'alpha/11', kind: 'far', label: 'GAM-2' }] : [],
} as Snapshot);
const response = (data: unknown): Response => ({ ok: true, status: 200, json: async () => data } as Response);

beforeEach(() => {
  vi.resetModules();
  vi.unstubAllGlobals();
  store.snap.value = null;
  store.gate.value = null;
  vi.stubGlobal('document', Object.assign(new EventTarget(), { hidden: false }));
  vi.stubGlobal('EventSource', Stream);
});

describe('state reads interleaved with the event stream', () => {
  for (const sameTime of [false, true]) {
    it(`keeps an archived wait when an older GET arrives after a newer event${sameTime ? ' within the same millisecond' : ''}`, async () => {
      let finish!: (r: Response) => void;
      vi.stubGlobal('fetch', vi.fn((path: string) => path === '/api/load'
        ? Promise.resolve(response({})) : new Promise<Response>((r) => { finish = r; })));
      const api = await import('./api');
      const onState = vi.fn(), onChat = vi.fn();
      api.onState(onState); api.onChatEvent(onChat); api.connect();
      const old = snapshot(1, 1000);
      const pending = api.refreshNow();
      const latest = snapshot(2, sameTime ? 1000 : 2000, true);
      Stream.current.state(latest);
      finish(response(old)); await pending;
      expect(store.snap.value).toEqual(latest);
      expect(onState).toHaveBeenCalledTimes(1);
      expect(onChat).not.toHaveBeenCalled();
    });
  }

  it('lets the snapshot counter restart when the server has a later reading time', async () => {
    const api = await import('./api'); api.connect();
    Stream.current.state(snapshot(100, 1000));
    const restarted = snapshot(1, 2000, true);
    Stream.current.state(restarted);
    expect(store.snap.value).toEqual(restarted);
  });

  it('accepts updates to the same snapshot (such as chat or todo changes)', async () => {
    const api = await import('./api'); api.connect();
    const first = snapshot(2, 2000);
    Stream.current.state(first);
    const updated = { ...first, chat_rev: 3, todo: [] };
    Stream.current.state(updated);
    expect(store.snap.value).toEqual(updated);
  });

  it('makes every caller of refreshNow wait for the shared read to finish', async () => {
    let finish!: (r: Response) => void;
    const fetch = vi.fn((path: string) => path === '/api/load'
      ? Promise.resolve(response({})) : new Promise<Response>((r) => { finish = r; }));
    vi.stubGlobal('fetch', fetch);
    const api = await import('./api');
    const first = api.refreshNow();
    let secondDone = false;
    const second = api.refreshNow().then(() => { secondDone = true; });
    await Promise.resolve(); await Promise.resolve();
    expect(secondDone).toBe(false);
    const latest = snapshot(2, 2000, true);
    finish(response(latest)); await Promise.all([first, second]);
    expect(store.snap.value).toEqual(latest);
    expect(fetch.mock.calls.filter(([path]) => path === '/api/state')).toHaveLength(1);
  });
});
