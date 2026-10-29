/* The words for places and for the airlock. */
import { t } from '../i18n';
import type { IrisLabel, Origin } from './state';

/** «Назад в навигатор» — the button that leaves the room names the place it leads to. */
export function backLabel(o: Origin): string {
  switch (o.kind) {
    case 'nav': return t('restroom.back.nav');
    case 'quasar': return t('restroom.back.quasar');
    case 'task': return t('restroom.back.task', { tag: o.tag });
    case 'universe': return t('restroom.back.universe', { name: o.name });
    default: return t('restroom.back.galaxy');
  }
}

function where(o: Origin): string {
  switch (o.kind) {
    case 'nav': return t('restroom.where.nav');
    case 'quasar': return t('restroom.where.quasar');
    case 'task': return o.tag;
    case 'universe': return o.name;
    default: return t('restroom.where.galaxy');
  }
}

export function irisText(l: IrisLabel): string {
  if (l.kind === 'fly') return t('restroom.gate.fly', { tag: l.tag });
  if (l.kind === 'back') return t('restroom.gate.back', { where: where(l.from).toUpperCase() });
  return t('restroom.gate.in');
}
