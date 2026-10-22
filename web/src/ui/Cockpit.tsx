import type { JSX } from 'preact';
import { useEffect, useState } from 'preact/hooks';
import { mode, selected, speed, taskOf } from '../store';
import { getWorld } from '../nav';
import { t } from '../i18n';
import './cockpit.css';

function targetName(): string {
  const e = selected.value;
  if (!e) return '';
  if (e.kind === 'planet') return taskOf(e.id)?.tag ?? e.id;
  if (e.kind === 'quasar') return t('quasar.label');
  if (e.kind === 'moon') {
    const [taskId, sid] = e.id.split('#');
    const sa = taskOf(taskId)?.subagents.find((x) => x.id === sid);
    return sa?.role || sa?.kind || sid;
  }
  return e.id;
}

/** The cabin: a frame around the view while the operator is «inside the ship» (something is selected, or the ship is flown by hand),
 *  with the target and the distance to it. */
export function Cockpit(): JSX.Element {
  const sel = selected.value;
  const inside = !!sel || mode.value === 'manual';
  const [dist, setDist] = useState<number | null>(null);
  useEffect(() => {
    if (!sel) { setDist(null); return; }
    const tick = (): void => setDist(getWorld()?.distanceTo(sel) ?? null);
    tick();
    const id = window.setInterval(tick, 400);
    return () => window.clearInterval(id);
  }, [sel?.kind, sel?.id]);
  return (
    <div class={`cockpit ${inside ? 'on' : ''}`} id="cockpit" aria-hidden="true">
      <i class="c tl" /><i class="c tr" /><i class="c bl" /><i class="c br" />
      <div class="strut l" /><div class="strut r" />
      {sel && (
        <div class="readout" id="readout">
          <span class="lbl">{t('cockpit.target')}</span> <b>{targetName()}</b>
          {dist !== null && <span class="dim"> · {t('cockpit.distance', { n: Math.round(dist) })}</span>}
          {mode.value === 'manual' && <span class="dim"> · {t('cockpit.speed', { n: Math.round(speed.value.now) })}</span>}
        </div>
      )}
    </div>
  );
}
