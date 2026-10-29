/* The «Экипаж» window: the one place where the operator steers the crew and the presets. The standard roles
   (Координатор, Планировщик, Ревьюер, Тяжёлые, Watcher, Лёгкие), the crew's own roles and the rules that put them on tasks, the
   subscriptions and their limits beside them, and the presets: switching one shows «было → стало» here, over all of
   them. Every change goes through the board's own commands after a confirmation (Station.tsx has the machinery).
   The first-run wizard («Экипаж корабля») is this window with the intro and «Готово». */
import type { JSX } from 'preact';
import { useEffect, useState } from 'preact/hooks';
import { t } from '../i18n';
import { Gl } from './Gl';
import { CrewSummary, Programs } from './CrewPrograms';
import { Presets } from './CrewPresets';
import { OwnRoles, Rules, closePut, useRoles } from './CrewRoles';
import { Crew, ErrorBlock, LevelBar, suggestCrew, useStationData } from './Station';
import { closeCrew, crewDlg, openStation } from './station-api';
import './station.css';

const SECTIONS = ['st-presets', 'st-crew', 'st-roles', 'st-rules', 'st-programs'] as const;
const SECTION_NAMES: Record<typeof SECTIONS[number], string> = {
  'st-presets': 'cr.nav.presets', 'st-crew': 'cr.nav.std', 'st-roles': 'cr.nav.own', 'st-rules': 'cr.nav.rules', 'st-programs': 'cr.nav.subs',
};

function CrewBody(props: { wizard: boolean }): JSX.Element {
  const { wizard } = props;
  const st = useStationData();
  const roles = useRoles();
  const { data, err, loading, scope } = st;
  const [suggest, setSuggest] = useState<Record<string, string>>({});
  const [sugDone, setSugDone] = useState(false);
  // a role changed, or a preset applied: the station's answer (the presets' differences) and the roles are read again
  const reloadAll = (): void => { st.reload(); roles.reload(); };

  useEffect(() => () => closePut(), []);
  useEffect(() => {
    if (!wizard || !data) return;
    let alive = true;
    suggestCrew(data)
      .then((m) => { if (alive) { setSuggest(m); setSugDone(true); } })
      .catch(() => { if (alive) setSugDone(true); });
    return () => { alive = false; };
  }, [wizard, data]);

  const jump = (id: string): void => { document.getElementById(id)?.scrollIntoView({ block: 'start' }); };

  return (
    <div class="bd st-bd">
      {wizard && <p class="hint" id="st-intro">{t('st.wizIntro')}</p>}
      {!wizard && <LevelBar st={st} />}
      {loading && !data && <div class="hint" id="st-loading">{t('st.loading')}</div>}
      {err && <ErrorBlock err={err} onRetry={st.reload} />}
      {data && (
        <>
          <CrewSummary data={data} />
          {!wizard && (
            <nav class="cr-nav" id="cr-nav" aria-label={t('cr.nav.aria')}>
              {SECTIONS.map((id) => <button key={id} class="btn sm" type="button" data-jump={id} onClick={() => jump(id)}>{t(SECTION_NAMES[id])}</button>)}
            </nav>
          )}
          {!wizard && <Presets data={data} repo={scope} own={roles.data?.roles ?? []} reload={reloadAll} />}
          <Crew repo={scope} data={data} reload={reloadAll} wizard={wizard} suggest={suggest} />
          {wizard && sugDone && Object.keys(suggest).length === 0 && <div class="hint bad">{t('st.wizNone')}</div>}
          {!wizard && <OwnRoles programs={data.programs} roles={roles} reload={st.reload} />}
          {!wizard && <Rules roles={roles} reload={st.reload} />}
          <Programs programs={data.programs} />
          {wizard && (
            <div class="act">
              <button class="btn primary" type="button" id="st-done" onClick={closeCrew}>{t('st.done')}</button>
              <button class="btn" type="button" id="st-skip" onClick={closeCrew}>{t('st.skip')}</button>
              <button class="btn" type="button" id="st-all" onClick={() => { closeCrew(); openStation(); }}>{t('st.allSettings')}</button>
            </div>
          )}
          {!wizard && (
            <p class="hint" id="cr-station-link">
              {t('cr.stationHint')} <button class="btn sm" type="button" id="cr-open-station" onClick={() => { closeCrew(); openStation(); }}>{t('cr.openStation')}</button>
            </p>
          )}
        </>
      )}
    </div>
  );
}

/** The crew window. Opened by `openCrew(wizard?)`. */
export function CrewWindow(): JSX.Element | null {
  const d = crewDlg.value;
  if (!d) return null;
  const title = d.wizard ? t('st.wizTitle') : t('cr.window');
  return (
    <div class="scrim" onMouseDown={(e) => { if (e.target === e.currentTarget) closeCrew(); }}>
      <section class={`panel dlg wide station crew ${d.wizard ? 'wizard' : ''}`} id="crew" role="dialog" aria-label={title}>
        <header><b>{title}</b><button class="btn icon sm" type="button" aria-label={t('close')} onClick={closeCrew}><Gl n="close" /></button></header>
        <CrewBody key={d.wizard ? 'w' : 'n'} wizard={!!d.wizard} />
      </section>
    </div>
  );
}
