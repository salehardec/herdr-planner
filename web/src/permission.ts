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

const ALWAYS = /don'?t ask again|always allow|and always|не спрашив|больше не спрашив/i;
const DENY = /^(no\b|deny\b|reject\b|запрет|нет\b)/i;
const ALLOW = /^(yes\b|allow\b|approve\b|proceed\b|разреш|да\b)/i;
/** A prompt that names a tool or a command is a permission even when the choices are a bare yes and no. */
const COMMAND = /bash command|run the following command|allow access|\$\s|команд|файл/i;

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
 * The prompt to offer as buttons under a task: the board says the task waits for a permission, or the screen makes it
 * plain (a tool or a command is named). A task the board calls a question keeps its own card: its menu is the question.
 */
export function taskPermission(task: Task, menu: MenuData | null | undefined): PermissionPrompt | null {
  const prompt = permissionPrompt(menu);
  if (!prompt) return null;
  if (task.kind === 'permission') return prompt;
  if (task.kind === 'question') return null;
  const says = COMMAND.test(`${prompt.question}\n${prompt.context}`);
  return prompt.always !== undefined || says ? prompt : null;
}

/** The question a task waits with: the board's line, else the screen's question, else nothing (the card says so). */
export function questionText(task: Task, menu: MenuData | null | undefined): string {
  const own = task.msg.trim();
  if (own) return own;
  return menu?.question?.trim() ?? '';
}
