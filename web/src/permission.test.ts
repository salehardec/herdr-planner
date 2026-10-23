import { describe, expect, it } from 'vitest';
import { permissionPrompt, questionText, taskPermission } from './permission';
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

describe('permission prompts', () => {
  it('reads the rows of a Claude Code prompt: yes, yes and never ask again, no', () => {
    const p = permissionPrompt(menu(['Yes', 'Yes, and always allow access to /tmp/x from this project', 'Yes, and switch to auto mode', 'No']));
    expect(p).toEqual({ allow: 0, always: 1, deny: 3, question: 'Do you want to proceed?', context: '' });
  });
  it('reads Codex’s words', () => {
    const p = permissionPrompt(menu(['Yes, proceed (y)', "Yes, and don't ask again for commands that start with `rm -rf build` (p)", 'No, and tell Codex what to do differently (esc)']));
    expect(p).toMatchObject({ allow: 0, always: 1, deny: 2 });
  });
  it('a bare yes and no is a permission only when the screen names a command', () => {
    const plain = menu(['Yes', 'No'], { question: 'May I continue?' });
    expect(permissionPrompt(plain)).toMatchObject({ allow: 0, deny: 1, always: undefined });
    expect(taskPermission(task({ kind: 'permission' }), plain)).not.toBeNull();
    expect(taskPermission(task({ kind: '' }), plain)).toBeNull();
    const bash = menu(['Yes', 'No'], { context: 'Bash command\n touch made-by-test.txt' });
    expect(taskPermission(task({ kind: '' }), bash)).not.toBeNull();
    // a menu with boxes is a multiple-choice question, never a permission
    expect(taskPermission(task({ kind: 'permission' }), menu(['Yes', 'No'], { multi: true }))).toBeNull();
  });
  it('a question keeps its own card even when its rows read yes and no', () => {
    expect(taskPermission(task({ kind: 'question' }), menu(['Yes', 'No']))).toBeNull();
  });
});

describe('the question of a task', () => {
  it('prefers the board’s line, then the screen, then nothing', () => {
    const m = menu(['Yes', 'No'], { question: 'Which schema?' });
    expect(questionText(task({ msg: 'Old or new?' }), m)).toBe('Old or new?');
    expect(questionText(task({}), m)).toBe('Which schema?');
    expect(questionText(task({}), null)).toBe('');
  });
});
