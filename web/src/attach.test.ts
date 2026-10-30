import { readdirSync, readFileSync } from 'node:fs';
import { join } from 'node:path';
import { beforeEach, expect, it, vi } from 'vitest';

vi.mock('./toast', () => ({ toast: vi.fn() }));
vi.mock('./i18n', () => ({ t: (k: string) => k }));

const png = (name: string): File => new File([Uint8Array.from([137, 80, 78, 71])], name, { type: 'image/png' });

beforeEach(() => {
  let n = 0;
  vi.stubGlobal('URL', { createObjectURL: () => `blob:x/${++n}`, revokeObjectURL: vi.fn() });
  vi.stubGlobal('fetch', vi.fn(async () => ({ status: 200, ok: true, json: async () => ({ attachment: { id: `id${++n}.png`, name: 'a.png' } }) })));
});

it('a pasted or picked image joins the shelf and can be taken off again', async () => {
  const { makeShelf } = await import('./attach');
  const shelf = makeShelf();
  shelf.add([png('paste.png'), png('two.png')]);
  expect(shelf.files.value.map((f) => f.name)).toEqual(['paste.png', 'two.png']);
  await shelf.settled();
  const [first, second] = shelf.files.value;
  shelf.remove(first.key);
  expect(shelf.files.value.map((f) => f.key)).toEqual([second.key]);
  expect(URL.revokeObjectURL).toHaveBeenCalledWith(first.local);
  shelf.clear();
  expect(shelf.files.value).toEqual([]);
});

it('Backspace in an empty field takes the last file, one by one', async () => {
  const { makeShelf, backspaceTakesFile } = await import('./attach');
  const shelf = makeShelf();
  shelf.add([png('a.png'), png('b.png')]);
  const key = (k = 'Backspace', extra: object = {}) => ({ key: k, preventDefault: vi.fn(), ...extra });
  const keyDown = key();
  expect(backspaceTakesFile(keyDown, { value: '' }, shelf)).toBe(true);
  expect(keyDown.preventDefault).toHaveBeenCalled();
  expect(shelf.files.value.map((f) => f.name)).toEqual(['a.png']);
  // text in the field: Backspace is for the text; other keys and chords leave the files alone
  expect(backspaceTakesFile(key(), { value: 'x' }, shelf)).toBe(false);
  expect(backspaceTakesFile(key('Delete'), { value: '' }, shelf)).toBe(false);
  expect(backspaceTakesFile(key('Backspace', { ctrlKey: true }), { value: '' }, shelf)).toBe(false);
  expect(backspaceTakesFile(key('Backspace', { isComposing: true }), { value: '' }, shelf)).toBe(false);
  expect(shelf.files.value).toHaveLength(1);
  expect(backspaceTakesFile(key(), { value: '' }, shelf)).toBe(true);
  // nothing left: the key is not taken
  const idle = key();
  expect(backspaceTakesFile(idle, { value: '' }, shelf)).toBe(false);
  expect(idle.preventDefault).not.toHaveBeenCalled();
});

it('the × of a preview is not a class another style sheet hides (.rm was the rest room\'s layer)', () => {
  const dir = join(__dirname);
  const sheets: string[] = [];
  const walk = (d: string): void => {
    for (const e of readdirSync(d, { withFileTypes: true })) {
      if (e.isDirectory()) walk(join(d, e.name));
      else if (e.name.endsWith('.css')) sheets.push(join(d, e.name));
    }
  };
  walk(dir);
  const cls = /class="([\w-]+)" type="button" data-(?:att|file)-rm/g;
  const names = [...readFileSync(join(dir, 'ui/Attached.tsx'), 'utf8').matchAll(cls)].map((m) => m[1]);
  expect(names.length).toBe(2);
  for (const name of names) {
    const owners = sheets.filter((f) => new RegExp(`(^|[},\\s])\\.${name}[\\s,{:.\\[]`, 'm').test(readFileSync(f, 'utf8').replace(/\/\*[\s\S]*?\*\//g, '')));
    expect(owners.map((f) => f.slice(dir.length))).toEqual(['/ui/attach.css']);
  }
});
