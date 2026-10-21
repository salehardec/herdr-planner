/* Landing in a far galaxy is not silent: a card says that the galaxy has its own station and who runs its crew.
   The flight line («Устанавливаю связь…») says it too, and the top bar always shows the galaxy and its station. */
import { signal } from '@preact/signals';
import type { JSX } from 'preact';
import { useEffect } from 'preact/hooks';
import { galaxyId, snap } from '../store';
import { t } from '../i18n';
import { Gl } from './Gl';
import { arrival, arrivalHidden, callError, CallError, hideArrival, openStation, stationGet, StationData } from './station-api';
import './arrive.css';

interface Crew { label: string; data?: StationData; err?: CallError }
const crew = signal<Crew | null>(null);

const agentOf = (d: StationData, name: string): string => d.settings.find((s) => s.name === name)?.value || '—';

/** Lands the notice on a far galaxy that has not been hidden for good; leaving the galaxy clears it. */
export function Arrival(): JSX.Element | null {
  const g = galaxyId.value;
  useEffect(() => {
    arrival.value = g && !arrivalHidden(g) ? { label: g } : null;
  }, [g]);
  const a = arrival.value;
  useEffect(() => {
    if (!a) { crew.value = null; return; }
    let alive = true;
    crew.value = { label: a.label };
    stationGet(a.label)
      .then((d) => { if (alive) crew.value = { label: a.label, data: d }; })
      .catch((e) => { if (alive) crew.value = { label: a.label, err: callError(e) }; });
    return () => { alive = false; };
  }, [a?.label]);
  if (!a) return null;
  const c = crew.value?.label === a.label ? crew.value : null;
  const d = c?.data;
  const text = d
    ? t('arr.text', {
      g: a.label,
      coord: agentOf(d, 'coordinator.agent'),
      rev: agentOf(d, 'reviewer.agent'),
      work: agentOf(d, 'worker.agent'),
    })
    : c?.err ? t('arr.noCrew', { g: a.label, why: c.err.message }) : t('arr.loading', { g: a.label });
  return (
    <section class="panel arrive" id="arrive" role="dialog" aria-label={t('arr.title', { g: a.label })} data-galaxy={a.label}>
      <header><b><Gl n="galaxy" />{t('arr.title', { g: a.label })}</b><button class="btn icon sm" type="button" aria-label={t('close')} onClick={() => { arrival.value = null; }}><Gl n="close" /></button></header>
      <div class="bd">
        <p id="arrive-text">{text}</p>
        <div class="act">
          <button class="btn primary sm" type="button" id="arrive-ok" onClick={() => { arrival.value = null; }}>{t('arr.ok')}</button>
          <button class="btn sm" type="button" id="arrive-hide" onClick={() => { hideArrival(a.label); arrival.value = null; }}>{t('arr.hide')}</button>
          <button class="btn sm" type="button" id="arrive-station" onClick={() => openStation(a.label)}>⚙ {t('arr.station')}</button>
        </div>
      </div>
    </section>
  );
}

/** The line of the flight and the connection: this galaxy has its own station. */
export function FlightStationLine({ label }: { label: string }): JSX.Element {
  return <span class="st-line" id="link-station">{t('arr.flightLine', { g: label })}</span>;
}

/** The top bar's station: which galaxy the ship is in, whose station it is, and the ⚙ that opens it. */
export function StationChip(): JSX.Element {
  const g = galaxyId.value;
  const host = snap.value?.host ?? '';
  const name = g === '' ? host : g;
  return (
    <div class={`stchip ${g === '' ? 'home' : 'far'}`} id="station-chip" data-galaxy={g}>
      <span class="long">{g === '' ? t('chip.own', { host }) : t('chip.far', { g })}</span>
      <span class="short">{name}</span>
      <button class="btn icon sm stgear" type="button" id="station-open" aria-label={g === '' ? t('chip.openHome') : t('chip.openFar', { g })} title={g === '' ? t('chip.openHome') : t('chip.openFar', { g })} onClick={() => openStation(g)}>⚙</button>
    </div>
  );
}
