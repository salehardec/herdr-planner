import { readFileSync, readdirSync, statSync } from 'node:fs';
import { join } from 'node:path';
import { describe, expect, it } from 'vitest';
import { opensRoom, padOf, ROOM_KEY, swallowed } from './keymap';

const SRC = join(__dirname, '..');
const files = (dir: string): string[] => readdirSync(dir).flatMap((f) => {
  const p = join(dir, f);
  return statSync(p).isDirectory() ? files(p) : /\.(ts|tsx)$/.test(f) && !f.endsWith('.test.ts') ? [p] : [];
});

describe('the key of the rest room', () => {
  it('is O, read by the physical key: Щ of the Russian layout opens it too', () => {
    expect(ROOM_KEY).toBe('KeyO');
    expect(opensRoom({ code: 'KeyO' })).toBe(true);
  });

  it('is not O with Ctrl, Cmd or Alt, nor a key held down', () => {
    expect(opensRoom({ code: 'KeyO', ctrlKey: true })).toBe(false);
    expect(opensRoom({ code: 'KeyO', metaKey: true })).toBe(false);
    expect(opensRoom({ code: 'KeyO', altKey: true })).toBe(false);
    expect(opensRoom({ code: 'KeyO', repeat: true })).toBe(false);
    expect(opensRoom({ code: 'KeyP' })).toBe(false);
  });

  it('is taken by nothing else in the page: no other KeyO, and the letter is not a key of the world either', () => {
    const users = files(SRC).filter((f) => !f.includes('/restroom/')).flatMap((f) => (readFileSync(f, 'utf8').match(/'KeyO'|"KeyO"/g) ?? []).map(() => f.slice(SRC.length + 1)));
    expect(users).toEqual([]);
    const keys = readFileSync(join(SRC, 'keys.ts'), 'utf8');
    expect(keys.match(/case ROOM_KEY:/g)?.length).toBe(1);
    // the other letters of the page's keyboard, each in its own case: none is the room's
    const taken = [...keys.matchAll(/case '(Key[A-Z])'/g)].map((m) => m[1]);
    expect(taken.length).toBeGreaterThan(8);
    expect(taken).not.toContain(ROOM_KEY);
    const world = readFileSync(join(SRC, 'world', 'index.ts'), 'utf8');
    expect([...world.matchAll(/'(Key[A-Z])'/g)].map((m) => m[1])).not.toContain(ROOM_KEY);
  });

  it('is in the help of the keys', () => {
    expect(readFileSync(join(SRC, 'ui', 'Help.tsx'), 'utf8')).toMatch(/\['O', 'help\.rest'\]/);
  });
});

describe('the buttons of the console', () => {
  it('are the arrows and A D, Space, X, Enter, P and M', () => {
    expect(padOf('ArrowLeft')).toBe('left');
    expect(padOf('KeyD')).toBe('right');
    expect(padOf('Space')).toBe('a');
    expect(padOf('KeyP')).toBe('start');
    expect(padOf('KeyM')).toBe('select');
    expect(padOf('Enter')).toBe('ok');
    expect(padOf('KeyO')).toBeNull();
    expect(padOf('KeyQ')).toBeNull();
  });

  it('keep the page from scrolling or clicking with them', () => {
    expect(swallowed('Space')).toBe(true);
    expect(swallowed('ArrowDown')).toBe(true);
    expect(swallowed('Tab')).toBe(false);
  });
});
