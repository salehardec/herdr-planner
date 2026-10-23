/* A permission prompt on an agent's screen: which rows say «yes», «yes and never ask again» and «no». The rows are the
   program's own words (Claude Code, Codex, omp), so they are read by meaning, and a menu without a plain yes and no is
   not a permission prompt at all (it is a question, and the MenuCard answers it). */
import type { MenuData, Task } from './types';

export interface PermissionPrompt {
  /** The row of «yes» (an index into `menu.options`). */
  allow: number;
  /** The row of «yes, and do not ask again» — not every program draws one. */
  always?: number;
  /** The row of «no». */
  deny: number;
  question: string;
  /** What stands around the question: the tool, the command, the file. */
  context: string;
}

/** What the human did, as the server names it for the Coordinator. */
export type PermissionNote = 'allow' | 'always' | 'deny';

/** «Yes, and always allow…», «…don't ask again», Claude Code's «…switch to accept edits … for this session». */
const ALWAYS = /don'?t ask again|always allow|and always|for this session|switch to accept edits|accept edits|не спрашив|больше не спрашив|в этой сессии/i;
const DENY = /^(no\b|deny\b|reject\b|запрет|нет\b)/i;
const ALLOW = /^(yes\b|allow\b|approve\b|proceed\b|разреш|да\b)/i;
/** What a program asks when it wants to do something: the question itself names the deed. */
const PERMISSION_QUESTION = /^(do you want\b|would you like\b|may i\b|shall i\b|allow\b|approve\b|proceed\b|разрешить\b|хотите\b)/i;
/** A prompt that names a tool or a command is a permission even when the choices are a bare yes and no. */
const COMMAND = /bash command|run the following command|allow access|create file|write file|edit file|\$\s|команд|файл/i;

/** The permission prompt a menu draws, or null when the menu is a question. */
export function permissionPrompt(menu: MenuData | null | undefined): PermissionPrompt | null {
  if (!menu || menu.multi) return null;
  let allow: number | undefined;
  let always: number | undefined;
  let deny: number | undefined;
  for (const o of menu.options) {
    if (o.own) continue;
    const label = o.label.trim();
    if (ALWAYS.test(label)) always ??= o.at;
    else if (DENY.test(label)) deny ??= o.at;
    else if (ALLOW.test(label)) allow ??= o.at;
  }
  if (allow === undefined || deny === undefined) return null;
  return { allow, always, deny, question: menu.question, context: menu.context };
}

/**
 * The prompt to offer as buttons under a task. The board is not a reliable witness here: an agent that stops on a dialog
 * comes as `kind: question` (a turn blocked without text) or with no kind at all (herdr says blocked), and its `msg` is
 * the board's own «Needs you». The screen is the witness: a yes/and-never-ask/no list, or a question that asks to do
 * something, is a permission. A menu that is only a question (no permission words) keeps the question card.
 */
export function taskPermission(task: Task, menu: MenuData | null | undefined): PermissionPrompt | null {
  const prompt = permissionPrompt(menu);
  if (!prompt) return null;
  if (task.kind === 'permission') return prompt;
  const says = COMMAND.test(`${prompt.question}\n${prompt.context}`);
  return prompt.always !== undefined || PERMISSION_QUESTION.test(prompt.question.trim()) || says ? prompt : null;
}

/** The board's own words when the agent left no text: «Needs you» is not a question. */
export function boardPlaceholder(text: string): boolean {
  return /^\s*needs you[.!…]?\s*$/i.test(text);
}

/** The question a task waits with: the board's line (when it has one), else the screen's question, else nothing. */
export function questionText(task: Task, menu: MenuData | null | undefined): string {
  const own = task.msg.trim();
  if (own && !boardPlaceholder(own)) return own;
  return menu?.question?.trim() ?? '';
}
