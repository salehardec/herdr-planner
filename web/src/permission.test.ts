import { describe, expect, it } from 'vitest';
import { boardPlaceholder, permissionPrompt, questionText, taskPermission } from './permission';
import type { MenuData, Task } from './types';

const menu = (labels: string[], over: Partial<MenuData> = {}): MenuData => ({
  program: 'claude', sig: 's', question: 'Do you want to proceed?', context: '', multi: false, cursor: 0,
  submit_at: null, own_editor: false, step: 0, steps: 0, tabs: [],
  options: labels.map((label, at) => ({ at, label, desc: '', checked: false, own: false })),
  ...over,
});

const task = (over: Partial<Task>): Task => ({
  id: 'a/1', repo: 'a', n: 1, tag: 'A-1', title: 't', state: 'blocked', kind: '', stage: '', msg: '', app: 'claude',
  progress: null, eta: null, pr: '', review: null, round: 0, queue: null, status: 'waiting', label: '', board_label: '',
  review_failed: false, merge_blocked: false, archived: false, actions: [], created: 0, description: '', worker_agent: '',
  model: '', session: '', worktree: '', exited_at: 0, key: 'A', subagents: [], subagents_note: '', ...over,
});

// Claude Code 2.1.309, a file to be created (the PLN-60 case): no «always allow», the session row instead.
const FILE_PROMPT = menu(
  ['Yes', 'Yes, and switch to accept edits (shift+tab) for this session', 'No'],
  { question: 'Do you want to create closing-note2.md?', context: 'Create file\nclosing-note2.md' },
);

describe('permission prompts', () => {
  it('reads the rows of a Claude Code prompt: yes, yes and never ask again, no', () => {
    const p = permissionPrompt(menu(['Yes', 'Yes, and always allow access to /tmp/x from this project', 'Yes, and switch to auto mode', 'No']));
    expect(p).toEqual({ allow: 0, always: 1, deny: 3, question: 'Do you want to proceed?', context: '' });
  });
  it('reads Codex’s words', () => {
    const p = permissionPrompt(menu(['Yes, proceed (y)', "Yes, and don't ask again for commands that start with `rm -rf build` (p)", 'No, and tell Codex what to do differently (esc)']));
    expect(p).toMatchObject({ allow: 0, always: 1, deny: 2 });
  });
  it('reads Claude Code’s session row in a file prompt', () => {
    expect(permissionPrompt(FILE_PROMPT)).toEqual({
      allow: 0, always: 1, deny: 2,
      question: 'Do you want to create closing-note2.md?', context: 'Create file\nclosing-note2.md',
    });
  });
  it('is a permission when the board calls the task a question or gives it no kind at all', () => {
    // The board says `question` with «Needs you» for a turn blocked without text, and nothing at all when herdr says blocked.
    expect(taskPermission(task({ kind: 'question', msg: 'Needs you' }), FILE_PROMPT)).not.toBeNull();
    expect(taskPermission(task({ kind: '', msg: 'Needs you' }), FILE_PROMPT)).not.toBeNull();
    expect(taskPermission(task({ kind: 'permission' }), FILE_PROMPT)).not.toBeNull();
  });
  it('a bare yes and no is a permission only when the question asks to do something', () => {
    const plain = menu(['Yes', 'No'], { question: 'Which schema?' });
    expect(permissionPrompt(plain)).toMatchObject({ allow: 0, deny: 1, always: undefined });
    expect(taskPermission(task({ kind: 'question' }), plain)).toBeNull();
    const asks = menu(['Yes', 'No'], { question: 'May I continue?' });
    expect(taskPermission(task({ kind: '' }), asks)).not.toBeNull();
    const bash = menu(['Yes', 'No'], { question: 'Proceed?', context: 'Bash command\n touch made-by-test.txt' });
    expect(taskPermission(task({ kind: '' }), bash)).not.toBeNull();
    // a menu with boxes is a multiple-choice question, never a permission
    expect(taskPermission(task({ kind: 'permission' }), menu(['Yes', 'No'], { multi: true }))).toBeNull();
  });
});

describe('the question of a task', () => {
  it('the board’s «Needs you» is not the question', () => {
    expect(boardPlaceholder('Needs you')).toBe(true);
    expect(boardPlaceholder('Needs you.')).toBe(true);
    expect(boardPlaceholder('Which schema?')).toBe(false);
    expect(questionText(task({ msg: 'Needs you' }), menu(['Yes', 'No'], { question: 'Do you want to proceed?' }))).toBe('Do you want to proceed?');
    expect(questionText(task({ msg: 'Needs you' }), null)).toBe('');
  });
  it('prefers the board’s own words, then the screen, then nothing', () => {
    const m = menu(['Yes', 'No'], { question: 'Which schema?' });
    expect(questionText(task({ msg: 'Old or new?' }), m)).toBe('Old or new?');
    expect(questionText(task({}), m)).toBe('Which schema?');
    expect(questionText(task({}), null)).toBe('');
  });
});
