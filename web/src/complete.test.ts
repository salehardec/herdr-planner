import { describe, expect, it } from 'vitest';
import {
  applyItem, detect, groupsOf, mentionData, mentionItems, slashItems,
  type FileHit, type MentionData, type SlashCmd, type Trigger,
} from './complete';
import type { Snapshot } from './types';

const cmd = (name: string, desc = '', kind: SlashCmd['kind'] = 'builtin'): SlashCmd => ({ name, desc, kind });

const data = (over: Partial<MentionData> = {}): MentionData => ({
  tasks: [{ tag: 'PLN-25', title: 'Верфь: уборка диска' }],
  universes: [{ name: 'herdr-planner', root: '/home/me/Projects/herdr-planner', key: 'PLN' }],
  host: 'orion',
  runners: [{ id: 'app-semantics', target: 'ssh://app-semantics', ok: true }],
  images: [{ name: 'shot.png', path: '/state/attachments/abc.png' }],
  files: [
    { name: 'Chat.tsx', path: '/home/me/Projects/herdr-planner/web/src/ui/Chat.tsx', rel: 'web/src/ui/Chat.tsx', repo: 'herdr-planner', dir: false },
    { name: 'ui', path: '/home/me/Projects/herdr-planner/web/src/ui', rel: 'web/src/ui', repo: 'herdr-planner', dir: true },
  ],
  ...over,
});

describe('detect: where the symbol being typed is', () => {
  it('reads a command only at the very start of the message', () => {
    expect(detect('/comp', 5)).toEqual({ char: '/', start: 0, end: 5, query: 'comp' });
    expect(detect('/', 1)).toEqual({ char: '/', start: 0, end: 1, query: '' });
    // A space after the slash is already the command's own words: no list.
    expect(detect('/compact now', 12)).toBeNull();
    // The caret may sit inside the word being typed.
    expect(detect('/compact', 4)).toEqual({ char: '/', start: 0, end: 4, query: 'com' });
    // Not at the start: a slash in the middle of a message is a path, not a command.
    expect(detect('смотри /src', 11)).toBeNull();
  });

  it('reads a mention after a word boundary', () => {
    expect(detect('@PL', 3)).toEqual({ char: '@', start: 0, end: 3, query: 'PL' });
    expect(detect('смотри @Cha', 11)).toEqual({ char: '@', start: 7, end: 11, query: 'Cha' });
    expect(detect('(см @ui', 7)).toEqual({ char: '@', start: 4, end: 7, query: 'ui' });
    expect(detect('первая\n@Cha', 11)).toEqual({ char: '@', start: 7, end: 11, query: 'Cha' });
  });

  it('leaves an e-mail and a plain space alone', () => {
    expect(detect('a@b', 3)).toBeNull();
    expect(detect('@ ', 2)).toBeNull();
    expect(detect('', 0)).toBeNull();
    expect(detect('plain words', 11)).toBeNull();
  });

  it('takes the caret past the end as the end', () => {
    expect(detect('@a', 99)).toEqual({ char: '@', start: 0, end: 2, query: 'a' });
  });
});

describe('applyItem: what goes into the message', () => {
  const item = (insert: string) => ({ group: 'file' as const, label: 'x', hint: '', insert, icon: 'file' });

  it('a command is inserted with a space ready for its words', () => {
    const trigger: Trigger = { char: '/', start: 0, end: 5, query: 'comp' };
    expect(applyItem('/comp', trigger, item('/compact'))).toEqual({ text: '/compact ', caret: 9 });
    // A space that is already there is not doubled.
    const withSpace: Trigger = { char: '/', start: 0, end: 5, query: 'comp' };
    expect(applyItem('/comp уж', withSpace, item('/compact')).text).toBe('/compact уж');
  });

  it('a mention replaces the typed word and keeps the rest of the message', () => {
    const trigger: Trigger = { char: '@', start: 4, end: 8, query: 'Cha' };
    const got = applyItem('см. @Cha потом', trigger, item('/p/src/Chat.tsx'));
    expect(got.text).toBe('см. /p/src/Chat.tsx потом');
    expect(got.caret).toBe('см. /p/src/Chat.tsx'.length);
  });

  it('a mention before a word keeps them apart, a mention at the end does not add a space', () => {
    const trigger: Trigger = { char: '@', start: 0, end: 2, query: 'PL' };
    expect(applyItem('@PL-25', { ...trigger, end: 6, query: 'PL-25' }, item('PLN-25')).text).toBe('PLN-25');
    expect(applyItem('@ui.txt', { ...trigger, end: 6, query: 'ui.tx' }, item('/p/ui.txt')).text).toBe('/p/ui.txt t');
    expect(applyItem('@a', { ...trigger, end: 2, query: 'a' }, item('/p/a')).text).toBe('/p/a');
    expect(applyItem('@a и', { ...trigger, end: 2, query: 'a' }, item('/p/a')).text).toBe('/p/a и');
  });
});

describe('slashItems: the list of a program', () => {
  const list = [
    cmd('compact', 'cmd.claude.compact'),
    cmd('clear', 'cmd.claude.clear'),
    cmd('ui-ux-pro-max', 'Designing and reviewing interfaces', 'skill'),
    cmd('review:security', 'Check a diff for leaks', 'command'),
  ];

  it('shows the whole list in the order of the files', () => {
    expect(slashItems(list, '').map((i) => i.label)).toEqual(['/compact', '/clear', '/ui-ux-pro-max', '/review:security']);
    expect(slashItems(list, '')[0].hint).toBe('cmd.claude.compact');
  });

  it('filters by the name first, then by the description', () => {
    expect(slashItems(list, 'cl').map((i) => i.label)).toEqual(['/clear']);
    expect(slashItems(list, 'compact').map((i) => i.label)).toEqual(['/compact']);
    expect(slashItems(list, 'designing').map((i) => i.label)).toEqual(['/ui-ux-pro-max']);
    expect(slashItems(list, 'security').map((i) => i.label)).toEqual(['/review:security']);
    expect(slashItems(list, 'nope')).toEqual([]);
  });

  it('finds a command typed on the other keyboard layout', () => {
    // «сщьзфсе» on the Russian layout is «compact».
    expect(slashItems(list, 'сщьзфсе').map((i) => i.label)).toEqual(['/compact']);
  });

  it('an empty list is no hints at all', () => {
    expect(slashItems([], 'any')).toEqual([]);
  });
});

describe('mentionItems: the list of @', () => {
  it('groups tasks, universes, the galaxy, the shipyards, files and pictures', () => {
    const items = mentionItems('', data());
    expect(groupsOf(items)).toEqual(['task', 'universe', 'galaxy', 'runner', 'file', 'image']);
    const at = (g: string) => items.filter((i) => i.group === g);
    expect(at('task')[0].insert).toBe('PLN-25');
    expect(at('universe')[0].insert).toBe('/home/me/Projects/herdr-planner');
    expect(at('galaxy')[0].insert).toBe('orion');
    expect(at('runner')[0].insert).toBe('app-semantics');
    expect(at('file').map((i) => i.insert)).toEqual([
      '/home/me/Projects/herdr-planner/web/src/ui/Chat.tsx',
      '/home/me/Projects/herdr-planner/web/src/ui',
    ]);
    expect(at('image')[0].insert).toBe('/state/attachments/abc.png');
  });

  it('filters by what is typed: a task tag, a folder, a file, a repository', () => {
    expect(mentionItems('pln-2', data()).map((i) => i.label)).toEqual(['PLN-25']);
    expect(mentionItems('src/ui', data()).map((i) => i.label)).toEqual(['Chat.tsx', 'ui']);
    expect(mentionItems('chat', data()).map((i) => i.label)).toEqual(['Chat.tsx']);
    expect(mentionItems('herdr-planner', data()).map((i) => i.group)).toEqual(['universe', 'file', 'file']);
    expect(mentionItems('orion', data()).map((i) => i.group)).toEqual(['galaxy']);
    expect(mentionItems('nothing-here', data())).toEqual([]);
  });

  it('a repository without a folder in the message goes by its name', () => {
    const d = data({ universes: [{ name: 'plain', root: '', key: 'PL' }] });
    expect(mentionItems('plain', d)[0].insert).toBe('plain');
  });

  it('holds each group to a few rows', () => {
    const many = Array.from({ length: 30 }, (_, i) => ({
      name: `f${i}.ts`, path: `/p/f${i}.ts`, rel: `src/f${i}.ts`, repo: 'r', dir: false,
    }));
    expect(mentionItems('', data({ files: many })).filter((i) => i.group === 'file')).toHaveLength(8);
  });
});

describe('mentionData: what the page already holds', () => {
  it('names the pictures of the chat by their file on the machine with the board', () => {
    const s = {
      tasks: [{ tag: 'A-1', title: 'one' }],
      projects: [{ name: 'p', root: '/p', key: 'A' }],
      host: 'orion',
      runners: [],
      coordinator: {
        history: [{
          text: 'hi',
          at: 1,
          attachments: [
            { id: 'abc', name: 'shot.png', mime: 'image/png', size: 10 },
            { id: 'def', name: 'log.txt', mime: 'text/plain', size: 10 },
            { id: 'abc', name: 'shot.png', mime: 'image/png', size: 10 },
          ],
        }],
      },
      server: { attachments_dir: '/state/attachments' },
    } as unknown as Snapshot;
    const got = mentionData(s, []);
    expect(got.tasks).toEqual([{ tag: 'A-1', title: 'one' }]);
    expect(got.universes).toEqual([{ name: 'p', root: '/p', key: 'A' }]);
    expect(got.images).toEqual([{ name: 'shot.png', path: '/state/attachments/abc' }]);
  });

  it('without a state there is nothing', () => {
    const got = mentionData(null, [] as FileHit[]);
    expect(got).toEqual({ tasks: [], universes: [], host: '', runners: [], images: [], files: [] });
  });
});
