/* The light page's own state: the tab, the open sheet, what the server says of this device, the choices kept in the browser. */
import { computed, signal } from '@preact/signals';
import { api } from '../api';
import { persist } from '../store';

export type Tab = 'wait' | 'chat' | 'board';
export type Sheet =
  | { kind: 'task'; id: string }
  | { kind: 'plan'; id: string }
  | { kind: 'limits' }
  | { kind: 'settings' }
  | null;

export const tab = signal<Tab>('wait');
export const sheet = signal<Sheet>(null);

/** A tablet's width and up: the list on the left and the chat always on the right, so the chat has no tab of its own. */
const wideQuery = matchMedia('(min-width: 700px)');
export const wide = signal(wideQuery.matches);
wideQuery.addEventListener('change', () => { wide.value = wideQuery.matches; });
/** The tab whose list is on show (the chat's tab shows the waiting list beside the chat on a wide screen). */
export const listTab = computed<Tab>(() => (wide.value && tab.value === 'chat' ? 'wait' : tab.value));
export const chatShown = computed(() => wide.value || tab.value === 'chat');

const read = (key: string, fallback: string): string => {
  try { return localStorage.getItem(key) ?? fallback; } catch { return fallback; }
};

export type ThemePref = 'system' | 'dark' | 'light';
export const THEME_KEY = 'herdr-planner.m.theme';
export const themePref = signal<ThemePref>((['system', 'dark', 'light'].includes(read(THEME_KEY, 'system')) ? read(THEME_KEY, 'system') : 'system') as ThemePref);
export const setThemePref = (v: ThemePref): void => { themePref.value = v; persist(THEME_KEY, v); };

/** `GET /m/api/session`: who this is and until when. */
export interface Session {
  kind: 'device' | 'local';
  computer: string;
  label?: string;
  scope?: string;
  since_ms?: number;
  ends_ms?: number;
  gate_since_ms?: number;
  gate_closes_ms?: number;
  /** What kind of machine the browser said it is, and whether it can have the full version (`src/version.rs`). */
  form?: 'phone' | 'tablet' | 'desktop';
  full?: 'yes' | 'login' | 'closed' | 'nopassword';
  /** The person's own switch (the cookie `hp_view`). */
  view?: 'lite' | 'full' | null;
}
export const session = signal<Session | null>(null);
export async function loadSession(): Promise<void> {
  try { session.value = await api<Session>('/api/session'); } catch { /* the next try */ }
}

/** A word typed in a card and not sent yet, by card: it survives the list being redrawn by a new snapshot. */
export const drafts = signal<Record<string, string>>({});
export const setDraft = (key: string, text: string): void => { drafts.value = { ...drafts.value, [key]: text }; };
export const dropDraft = (key: string): void => {
  const next = { ...drafts.value };
  delete next[key];
  drafts.value = next;
};
