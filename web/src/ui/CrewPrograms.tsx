/* The top of the crew screen: the one-line summary (who leads each role, which subscriptions are down) and the card of
   every program installed here: sign-in and account, whether it can take work now and until when, the providers inside,
   who uses it, and the models it takes. What cannot be known is said so, with the reason. */
import type { JSX } from 'preact';
import { useState } from 'preact/hooks';
import { t, tn } from '../i18n';
import { Down, momentLabel, stateOf, summary, untilLabel } from './crew-view';
import { useModels } from './CrewPick';
import type { ProgramState, StationData, StationProgram } from './station-api';

const roleTitle = (role: string): string => t(`st.r.${role}`);

function downText(d: Down): string {
  if (d.kind === 'signed_out') return t('st.sumSignedOut', { p: d.program });
  const until = untilLabel({ until: d.until, until_ms: d.untilMs });
  return until ? t('st.sumLimited', { p: d.program, until }) : t('st.sumLimitedNoTime', { p: d.program });
}

/** One line: who leads each role now, and which subscriptions are unavailable. */
export function CrewSummary({ data }: { data: StationData }): JSX.Element {
  const s = summary(data);
  return (
    <div class="st-sum" id="st-summary" role="status">
      <span class="sum-leads">
        {s.leads.map((l) => (
          <span class="lead" key={l.role} data-lead={l.role}>
            <span class="kl">{roleTitle(l.role)}</span> <b>{l.agent || t('st.sumNobody')}</b>
          </span>
        ))}
      </span>
      <span class={`sum-down ${s.down.length ? 'bad' : 'ok'}`} id="st-summary-down" data-down={s.down.length}>
        {s.down.length === 0
          ? t('st.sumAllUp')
          : <>{t('st.sumDown')} {s.down.map((d, i) => <span key={d.program} data-down-program={d.program}>{i > 0 ? '; ' : ''}{downText(d)}</span>)}</>}
      </span>
    </div>
  );
}

const SOURCE: Record<string, string> = { board: 'st.src.board', omarchy: 'st.src.omarchy', doctor: 'st.src.doctor' };

function Account({ s }: { s: ProgramState }): JSX.Element {
  if (s.account) return <div class="pr-line" data-account><span class="kl">{t('st.account')}</span> <b>{s.account}</b></div>;
  return <div class="pr-line" data-account="unknown"><span class="kl">{t('st.account')}</span> <span class="unk">{t('st.unknownBecause', { why: s.unknown_why || t('st.noReason') })}</span></div>;
}

function Windows({ s }: { s: ProgramState }): JSX.Element | null {
  if (!s.windows.length) return <div class="pr-line" data-limits="unknown"><span class="kl">{t('st.limits')}</span> <span class="unk">{t('st.unknownBecause', { why: s.limit_why || t('st.noReason') })}</span></div>;
  return (
    <div class="pr-line" data-limits>
      <span class="kl">{t('st.limits')}</span>
      <ul class="wins">
        {s.windows.map((w) => (
          <li key={w.label} class={w.severity}>
            <span>{w.label}</span> <b>{t('st.windowUsed', { n: Math.round(w.percent * 100) })}</b>
            {w.resets_at_ms > 0 && <span class="m"> · {t('st.windowResets', { at: momentLabel(w.resets_at_ms) })}</span>}
          </li>
        ))}
      </ul>
      {s.stale && <span class="m warn">{t('st.windowsStale')}</span>}
    </div>
  );
}

function ProgramModels({ p }: { p: StationProgram }): JSX.Element {
  const { data, loading, error } = useModels(p.name);
  if (loading) return <div class="hint">{t('st.modelsLoading')}</div>;
  if (error) return <div class="hint bad">{error}</div>;
  if (!data) return <div class="hint">{t('st.noModels', { p: p.name })}</div>;
  const names = data.names.length ? data.names : data.suggested;
  return (
    <div class="pr-models" data-models-list={p.name}>
      {data.note && <div class="hint">{data.note}</div>}
      {!data.names.length && <div class="hint">{data.suggested.length ? t('st.modelsFromUse', { p: p.name, n: data.suggested.length }) : t('st.noModels', { p: p.name })}</div>}
      {names.length > 0 && <ul class="chips">{names.map((n) => <li key={n}><code>{n}</code></li>)}</ul>}
      {data.effort.length > 0 && <div class="hint">{t('st.effortTakes', { list: data.effort.join(', ') })}</div>}
    </div>
  );
}

function usedBy(p: StationProgram): JSX.Element {
  const roles = p.roles.filter((r) => r.field === 'agent').map((r) => roleTitle(r.role));
  const fb = p.roles.filter((r) => r.field === 'fallback').map((r) => roleTitle(r.role));
  if (!roles.length && !fb.length && !p.tasks.length) return <span class="unk">{t('st.usedNone')}</span>;
  return (
    <>
      {roles.length > 0 && <span data-used-roles>{t('st.usedRoles', { list: roles.join(', ') })}</span>}
      {fb.length > 0 && <span data-used-fallback> {t('st.usedFallback', { list: fb.join(', ') })}</span>}
      {p.tasks.length > 0 && (
        <span data-used-tasks> {tn('st.usedTasks', p.tasks.length)}: {p.tasks.slice(0, 6).map((x, i) => <span key={x.id} class="tg">{i > 0 ? ', ' : ''}{x.id}</span>)}{p.tasks.length > 6 ? ` +${p.tasks.length - 6}` : ''}</span>
      )}
    </>
  );
}

function ProgramCard({ p }: { p: StationProgram }): JSX.Element {
  const [open, setOpen] = useState(false);
  const st = p.state;
  const kind = stateOf(p);
  const until = untilLabel(st.avail);
  const chip = kind === 'ok' ? t('st.avail.ok')
    : kind === 'limited' ? (until ? t('st.avail.limitedUntil', { until }) : t('st.avail.limited'))
    : kind === 'signed_out' ? t('st.avail.signedOut')
    : t('st.avail.unknown');
  const sign = st.signed_in === true ? t('st.signedIn') : st.signed_in === false ? t('st.notSignedIn') : t('st.noSignIn');
  const m = p.models;
  const modelsLabel = m.listed ? tn('st.modelsOf', m.count) : m.loading ? t('st.modelsLoadingShort') : m.suggested > 0 ? tn('st.modelsUsed', m.suggested) : t('st.modelsNoList');
  return (
    <li class={`pcard ${kind}`} data-program={p.name} data-state={kind}>
      <div class="pr-head">
        <b>{p.name}</b> <span class="m">{p.version}</span>
        <span class="sig" data-sign={st.signed_in === null ? 'unknown' : st.signed_in ? 'in' : 'out'}>{sign}</span>
        <span class={`chip ${kind}`} data-avail={kind}>{chip}</span>
      </div>
      {st.avail.reason && kind !== 'ok' && kind !== 'unknown' && (
        <div class="hint" data-reason>{st.avail.reason}{st.avail.source ? ` — ${t(SOURCE[st.avail.source])}` : ''}</div>
      )}
      {kind === 'ok' && st.avail.reason && <div class="hint warn" data-reason>{st.avail.reason}</div>}
      <Account s={st} />
      <Windows s={st} />
      {p.providers.length > 0 && (
        <div class="pr-line" data-providers>
          <span class="kl">{t('st.providers')}</span>
          <ul class="chips">
            {p.providers.map((x) => <li key={x.name} data-provider={x.name} title={x.source === 'usage' ? t('st.providerGuess') : t('st.providerListed')}>{x.name}{x.source === 'usage' ? '*' : ''}</li>)}
          </ul>
          {p.providers.some((x) => x.source === 'usage') && <span class="m">{t('st.providerGuess')}</span>}
        </div>
      )}
      {['omp', 'pi', 'opencode'].includes(p.name) && p.providers.length === 0 && (
        <div class="pr-line" data-providers="unknown"><span class="kl">{t('st.providers')}</span> <span class="unk">{t('st.unknownBecause', { why: t('st.providersWhy', { p: p.name }) })}</span></div>
      )}
      <div class="pr-line" data-used><span class="kl">{t('st.usedBy')}</span> <span>{usedBy(p)}</span></div>
      <details class="pr-m" data-models-for={p.name} open={open} onToggle={(e) => setOpen((e.target as HTMLDetailsElement).open)}>
        <summary>{t('st.models')}: {modelsLabel}</summary>
        {open && <ProgramModels p={p} />}
      </details>
    </li>
  );
}

export function Programs({ programs }: { programs: StationProgram[] }): JSX.Element {
  const inst = programs.filter((p) => p.installed);
  const missing = programs.filter((p) => !p.installed).map((p) => p.name);
  return (
    <section class="st-sec" id="st-programs">
      <h3>{t('st.programs')}</h3>
      <p class="hint">{t('st.programsHint')}</p>
      {inst.length === 0 ? <div class="hint">{t('st.noPrograms')}</div> : <ul class="progs">{inst.map((p) => <ProgramCard key={p.name} p={p} />)}</ul>}
      {missing.length > 0 && <div class="hint">{t('st.notInstalled', { list: missing.join(', ') })}</div>}
    </section>
  );
}
