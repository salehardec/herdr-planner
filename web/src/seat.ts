/* Who is on a task now (the reviewer under review, the working agent), in words. */
import { t } from './i18n';
import type { Seat } from './types';

/** `pi/k3.1-pro`: the program and the last part of its model (the provider in front of it stays in the tooltip). */
export function seatName(s: Seat): string {
  const model = s.model.split('/').pop() ?? '';
  return s.program ? (model ? `${s.program}/${model}` : s.program) : t('seat.nobody');
}

/** `pi/kimi-coding/k3.1-pro`, as the board writes the agent. */
export const seatSpec = (s: Seat): string => (s.program ? [s.program, s.model].filter(Boolean).join('/') : t('seat.nobody'));

/** The short line after a task's name: `ревью · pi/k3.1-pro`; for the working agent the name alone (the glyph before the task says it works). */
export const seatLabel = (s: Seat): string => (s.role === 'reviewer' ? `${t('seat.reviewer')} · ${seatName(s)}` : seatName(s));

/** The tooltip: who it is, in full, and what a click does. */
export function seatTip(s: Seat): string {
  const who = `${t(`seat.who.${s.role}`)}: ${seatSpec(s)}${s.took_over ? ` (${t('seat.tookOver')})` : ''}`;
  return `${who}\n${t('seat.open')}`;
}
