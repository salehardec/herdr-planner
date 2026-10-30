/* What the phone's «Ждёт тебя» draws for a request, and which answers it offers. Pure: the page reads this, the tests read it too.
   The request itself is the planner's (requests.ts); here it is only decided which parts of the card a phone has.
   The phone's server takes four actions (approve, retry, tell, pick) and answers in agents' menus; nothing else. A card whose
   answer is not among them says so and sends the human to the computer. */
import type { Request, Task } from '../types';

/** What the phone may ask of the board; the same four words as `ACTIONS` in mobile.rs. */
export const PHONE_ACTIONS = ['approve', 'retry', 'tell', 'pick'] as const;

export type Block =
  | 'permission' | 'question' | 'plan' | 'approval' | 'failed' | 'limit' | 'pick' | 'reviewneeds' | 'auth'
  | 'alarm' | 'dialog' | 'options' | 'other';

export interface CardPlan {
  block: Block;
  /** Needs the agent's screen read for a menu (a permission or a question with options). */
  screen: boolean;
  /** `approve` is offered (the plan / the merge), asked twice. */
  approve: boolean;
  retry: boolean;
  /** The human may write to the agent: an answer, remarks, a word of advice. */
  tell: boolean;
  pick: boolean;
  /** The plan can be read in a sheet. */
  plan: boolean;
  /** The pull request address, when there is one. */
  pr: string;
  /** Why the rest is for the computer, when it is (a key into the dictionary), else empty. */
  note: string;
}

const BLOCK: Record<string, Block> = {
  permission: 'permission', question: 'question', plan: 'plan', approval: 'approval', failed: 'failed', limit: 'limit',
  pick: 'pick', reviewneeds: 'reviewneeds', auth: 'auth', 'runner-alert': 'alarm', 'coordinator-dialog': 'dialog',
  'coordinator-options': 'options',
};

export function blockOf(r: Request): Block {
  // the board calls the merge decision of a task whose review failed a «permission» too: it is the merge that is asked
  if (r.type === 'permission' && r.task?.stage === 'approval') return 'approval';
  return BLOCK[r.type] ?? 'other';
}

/** The board lists this action for the task, and a phone may ask it. */
export const can = (task: Task | undefined, action: string): boolean =>
  !!task && (PHONE_ACTIONS as readonly string[]).includes(action) && task.actions.includes(action);

/** An address a phone may follow (the pull request): http(s) only. */
export const safeUrl = (url: string): string => (/^https?:\/\/[^\s]+$/i.test(url.trim()) ? url.trim() : '');

export function planOf(r: Request): CardPlan {
  const block = blockOf(r);
  const task = r.task;
  const plan: CardPlan = {
    block, screen: false, approve: false, retry: false, tell: can(task, 'tell'), pick: false, plan: false, pr: '', note: '',
  };
  switch (block) {
    case 'permission':
    case 'question':
      plan.screen = true;
      break;
    case 'plan':
      plan.plan = true;
      plan.screen = true;
      plan.approve = can(task, 'approve');
      if (!plan.approve) plan.note = 'm.note.plan';
      break;
    case 'approval':
      if (task?.merge_via_mr) {
        plan.pr = safeUrl(task.merge_request ?? '');
        plan.note = 'req.mergeViaMr';
        break;
      }
      plan.approve = can(task, 'approve');
      plan.pr = safeUrl(task?.pr ?? '');
      // A checkout refusal is retried with `approve` after it is fixed: keep the button and explain beside it.
      // Failed reviews and the other merge blocks stay on the computer, with their own reason.
      if (task && /merge needs a clean checkout|your checkout is on another branch|task is on the wrong branch/i.test(task.msg)) {
        plan.note = 'm.note.mergeBlockedCheckout';
      } else if (!plan.approve) {
        plan.note = task?.merge_blocked ? 'm.note.mergeBlocked' : 'm.note.approval';
      }
      break;
    case 'failed':
      plan.retry = can(task, 'retry');
      break;
    case 'limit':
      plan.retry = can(task, 'retry');
      plan.tell = false;
      break;
    case 'pick':
      plan.pick = true;
      plan.tell = false;
      break;
    case 'auth':
      plan.note = 'm.note.auth';
      break;
    case 'reviewneeds':
      plan.note = 'm.note.reviewneeds';
      plan.tell = false;
      break;
    case 'alarm':
      plan.note = 'm.note.alarm';
      plan.tell = false;
      break;
    default:
      plan.tell = false;
  }
  return plan;
}

/** The requests the phone lists. The «В память» proposals are not sent to a phone, but if one came it would not be drawn. */
export const listed = (all: Request[]): Request[] => all.filter((r) => r.type !== 'memory');
