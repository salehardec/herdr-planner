/* Local receipts are a queue, not conversation history. Only the transcript orders delivered messages. */
import { splitAttachments } from './chat-parse';
import type { TurnData } from './types';

export interface Echo {
  id: string;
  text: string;
  attachments: { id: string; name: string }[];
  base: number;
  at: number;
  /** Keep the receipt so two identical sends cannot consume the same transcript message. */
  delivered?: number;
}

const words = (s: string): string => splitAttachments(s).text.replace(/\s+/g, ' ').trim();

export function matchesEcho(e: Echo, t: TurnData): boolean {
  if (t.from !== 'human' || t.n <= e.base || words(e.text) !== words(t.q)) return false;
  const parsed = splitAttachments(t.q);
  // Claude replaces file paths with image blocks. When paths survive, compare the actual IDs;
  // otherwise require all images, including a message containing only images.
  if (parsed.files.length) return e.attachments.map((f) => f.id).join() === parsed.files.map((f) => f.id).join();
  return e.attachments.length === (t.images?.length ?? 0);
}

/** Some accepted input never becomes a human transcript record (slash commands; non-Claude Coordinators). */
export const ECHO_WAIT_MS = 10 * 60 * 1000;

export function reconcileEchoes(receipts: Echo[], turns: TurnData[], now = Date.now()): Echo[] {
  // Expire only waiting receipts: a delivered one still protects a later identical send from reusing its turn.
  const live = receipts.filter((e) => e.delivered !== undefined || now - e.at <= ECHO_WAIT_MS);
  const used = new Set(live.flatMap((e) => e.delivered === undefined ? [] : [e.delivered]));
  return live.map((e) => {
    if (e.delivered !== undefined) return e;
    const t = turns.find((t) => !used.has(t.n) && matchesEcho(e, t));
    if (!t) return e;
    used.add(t.n);
    return { ...e, delivered: t.n };
  });
}

/** Only our saved receipt shape is restored; malformed browser storage cannot break the chat. */
export function restoreEchoes(raw: string | null): Echo[] {
  try {
    const data: unknown = JSON.parse(raw ?? '[]');
    if (!Array.isArray(data)) return [];
    return data.filter((e): e is Echo => !!e && typeof e.id === 'string' && typeof e.text === 'string'
      && Number.isInteger(e.base) && Number.isFinite(e.at)
      && (e.delivered === undefined || Number.isInteger(e.delivered))
      && Array.isArray(e.attachments) && e.attachments.every((f: Echo['attachments'][number]) => f && typeof f.id === 'string' && typeof f.name === 'string'));
  } catch { return []; }
}
