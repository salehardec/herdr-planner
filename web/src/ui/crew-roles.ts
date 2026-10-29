/* What a task's row says about the crew's own roles on it (PLN-90): the short mark and the words for where it came from.
   No DOM here, so it is tested on its own. */
import { t } from '../i18n';
import type { RoleMark, Task } from '../types';

/** The marks that show on a row: a role on the task, and the extra check that runs after it (its own task shows `followup`). */
export const marksOf = (task: Pick<Task, 'crew_roles'>): RoleMark[] => task.crew_roles ?? [];

/** The mark's short words: «Дизайнер», «Ревьюер безопасности · после», «Ревьюер безопасности · проверка PLN-12». */
export function markLabel(m: RoleMark, tagOf: (id: string) => string = (id) => id): string {
  if (m.how === 'followup') return t('cr.mark.followup', { name: m.name, task: tagOf(m.of) });
  if (m.how === 'after') return t('cr.mark.after', { name: m.name });
  if (m.how === 'instead') return t('cr.mark.instead', { name: m.name });
  return m.name;
}

/** Where the role came from: chosen by hand, or by which rule. */
export function markFrom(m: RoleMark): string {
  return m.by === 'rule' ? t('cr.mark.byRule', { rule: m.rule || t('cr.mark.ruleGone') }) : t('cr.mark.chosen');
}

/** The tooltip of a mark: the role, how it went on, where it came from. */
export function markTip(m: RoleMark, tagOf: (id: string) => string = (id) => id): string {
  return `${markLabel(m, tagOf)} · ${t(`cr.way.${m.how === 'followup' ? 'after' : m.how}`)} · ${markFrom(m)}`;
}
