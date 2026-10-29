import { beforeEach, describe, expect, it, vi } from 'vitest';

/** A page's storage that is kept between loads of the module. */
function storage(init: Record<string, string> = {}): Storage {
  const m = new Map(Object.entries(init));
  return {
    getItem: (k: string) => m.get(k) ?? null, setItem: (k: string, v: string) => { m.set(k, v); }, removeItem: (k: string) => { m.delete(k); },
    clear: () => m.clear(), key: (i: number) => [...m.keys()][i] ?? null, get length() { return m.size; },
  };
}

async function load(init: Record<string, string> = {}, store = storage(init)) {
  vi.resetModules();
  vi.stubGlobal('localStorage', store);
  return { s: await import('./state'), store };
}

beforeEach(() => { vi.unstubAllGlobals(); });

describe('entering and leaving', () => {
  it('opens on entering, remembers where from, and closes on leaving', async () => {
    const { s } = await load();
    expect(s.roomOpen.value).toBe(false);
    s.enter({ kind: 'task', tag: 'PLN-85' });
    expect(s.roomOpen.value).toBe(true);
    expect(s.origin.value).toEqual({ kind: 'task', tag: 'PLN-85' });
    expect(s.game.mode).toBe('menu');
    s.leave();
    expect(s.roomOpen.value).toBe(false);
  });

  it('keeps the game: left on pause, found on pause with the same score', async () => {
    const { s } = await load();
    s.enter({ kind: 'galaxy' });
    s.game.press('a');
    s.game.s!.score = 340;
    s.leave();
    expect(s.game.paused).toBe(true);
    s.enter({ kind: 'nav' });
    expect(s.game.mode).toBe('shoot');
    expect(s.game.s!.score).toBe(340);
    expect(s.game.paused).toBe(true);
    expect(s.origin.value).toEqual({ kind: 'nav' });
  });

  it('lets go of the held buttons when it is left', async () => {
    const { s } = await load();
    s.enter({ kind: 'galaxy' });
    s.game.press('a');
    s.game.down('left');
    s.leave();
    expect(s.game.held.size).toBe(0);
  });

  it('«Лететь» leaves the room and hands the thing to the page', async () => {
    const { s } = await load();
    const fly = vi.fn();
    s.hooks.fly = fly;
    s.enter({ kind: 'galaxy' });
    s.leave({ type: 'question', id: 'alpha/3', tag: 'ALP-3' });
    expect(s.roomOpen.value).toBe(false);
    expect(fly).toHaveBeenCalledWith({ type: 'question', id: 'alpha/3', tag: 'ALP-3' });
  });

  it('gives the focus back to the control that opened it, if that is still on the page', async () => {
    const { s } = await load();
    const focus = vi.fn();
    s.enter({ kind: 'galaxy' }, { isConnected: true, focus } as unknown as HTMLElement);
    s.leave();
    expect(focus).toHaveBeenCalled();
  });

  it('does nothing twice: a second entry or a leave of a closed room', async () => {
    const { s } = await load();
    s.leave();
    s.enter({ kind: 'galaxy' });
    s.enter({ kind: 'nav' });
    expect(s.origin.value).toEqual({ kind: 'galaxy' });
  });

  it('runs the airlock when motion is allowed: shut with a label, the room opens behind it', async () => {
    vi.useFakeTimers();
    const { s } = await load();
    s.hooks.calm = () => false;
    s.enter({ kind: 'galaxy' });
    expect(s.iris.value).toMatchObject({ shut: true, label: { kind: 'in' } });
    expect(s.roomOpen.value).toBe(false);
    s.enter({ kind: 'nav' }); // pressed again meanwhile
    vi.advanceTimersByTime(500);
    expect(s.roomOpen.value).toBe(true);
    expect(s.origin.value).toEqual({ kind: 'galaxy' });
    vi.advanceTimersByTime(300);
    expect(s.iris.value?.shut).toBe(false);
    s.leave();
    expect(s.iris.value).toMatchObject({ shut: true, label: { kind: 'back', from: { kind: 'galaxy' } } });
    vi.advanceTimersByTime(1000);
    expect(s.roomOpen.value).toBe(false);
    vi.useRealTimers();
  });
});

describe('the wallpaper', () => {
  it('is the Earth and the garden at first', async () => {
    const { s } = await load();
    expect(s.wallId.value).toBe('earth');
  });

  it('is remembered, a picture of the person\'s own too', async () => {
    const a = await load();
    a.s.setWall('sea');
    expect(a.store.getItem('herdr-planner.restroom.wall')).toBe('sea');
    expect(a.s.setCustomWall('data:image/png;base64,AAAA')).toBe(true);
    expect(a.s.wallId.value).toBe('custom');
    const b = await load({}, a.store); // the page is opened again
    expect(b.s.wallId.value).toBe('custom');
    expect(b.s.customWall.value).toBe('data:image/png;base64,AAAA');
  });

  it('falls back to the default for a picture that is gone or an id nobody knows', async () => {
    const { s } = await load({ 'herdr-planner.restroom.wall': 'custom' });
    expect(s.wallId.value).toBe('earth');
    s.setWall('nonsense');
    expect(s.wallId.value).toBe('earth');
  });

  it('says so when the browser cannot keep a picture, and still shows it', async () => {
    const store = storage();
    store.setItem = () => { throw new Error('quota'); };
    const { s } = await load({}, store);
    expect(s.setCustomWall('data:image/png;base64,BBBB')).toBe(false);
    expect(s.wallId.value).toBe('custom');
  });
});
