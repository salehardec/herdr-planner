/* Short, read-only summaries of saved crew presets and own roles. Kept apart from the views so old-file migration is
   tested without a browser. */
import { t } from '../i18n';
import type { CrewRole } from './crew-roles-api';
import type { PresetItem } from './station-api';
import { cleanChain, presetKey, removedAgent, ROLE_IDS } from './crew-view';

/** One line of what a preset gives each role. */
export function presetBrief(p: PresetItem, own: CrewRole[]): string {
  const std = ROLE_IDS.map((role) => {
    const r = p.roles[role];
    return r?.agent && !removedAgent(r.agent) ? `${t(`st.r.${role}`)}: ${r.agent}` : '';
  });
  const mine = own.map((x) => {
    const r = p.roles[presetKey(x.id)];
    return r?.agent && !removedAgent(r.agent) ? `${x.name}: ${r.agent}` : '';
  });
  return [...std, ...mine].filter(Boolean).join(' · ');
}

/** One line of an own role, without agents or fallback items of removed providers. */
export function roleBrief(r: CrewRole): string {
  const agent = removedAgent(r.agent) ? '' : r.agent;
  const fallback = cleanChain(r.fallback);
  const parts = [agent || t('cr.inherit'), r.effort && `${t('st.f.effort')}: ${r.effort}`, fallback && `${t('st.f.fallback')}: ${fallback}`].filter(Boolean);
  return parts.join(' · ');
}
