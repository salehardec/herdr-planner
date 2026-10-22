/* A shipyard's journal (PLN-47): who did what and when — container actions, cleanings, the agents' rule block and the
   periods the yard did not answer. The rows are numbers and kinds from the server; the page says them in the language
   it is read in. Filter and search run on the server, so a word finds old rows too, not only the first page. */
import type { JSX } from 'preact';
import { useEffect, useState } from 'preact/hooks';
import { api } from '../api';
import { t } from '../i18n';
import { dayTimeOf } from '../util/dates';
import { duration } from '../util/format';
import { memSize } from './usage-api';
import type { RunnerLogData, RunnerLogEntry } from '../types';

/** The filter values are groups of kinds the server accepts (`kind=a,b`). */
const FILTERS = ['stop', 'restart', 'delete', 'clean,disk', 'auto', 'rules', 'down', 'remove'];

function tagOf(e: RunnerLogEntry): string {
  return e.tag || t('runner.log.noTag');
}

function whoText(e: RunnerLogEntry): string {
  if (e.who === 'human') return t('runner.log.who.human');
  if (e.who === 'planner') return t('runner.log.who.planner');
  return t('runner.log.who.auto');
}

function whatText(e: RunnerLogEntry): string {
  const d = e.data ?? {};
  const c = d.containers ?? 0;
  const n = d.networks ?? 0;
  const v = d.volumes ?? 0;
  switch (e.kind) {
    case 'stop': return t('runner.log.stop', { name: e.name, tag: tagOf(e) });
    case 'restart': return t('runner.log.restart', { name: e.name, tag: tagOf(e) });
    case 'delete': return t('runner.log.delete', { name: e.name, tag: tagOf(e) });
    case 'clean': return t('runner.log.clean', { c, n, v });
    case 'disk': return (d.bytes ?? 0) > 0 ? t('runner.log.disk', { size: memSize(d.bytes ?? 0) }) : t('runner.log.diskNothing');
    case 'auto':
      return d.action === 'switch'
        ? (d.on ? t('runner.log.autoOn', { h: d.hours ?? 0 }) : t('runner.log.autoOff'))
        : t('runner.log.auto', { c, n, v });
    case 'rules': return d.on ? t('runner.log.rulesOn') : t('runner.log.rulesOff');
    case 'down':
      return e.until
        ? t('runner.log.down', { dur: duration(e.until - e.at, t) })
        : t('runner.log.downNow', { when: dayTimeOf(e.at) });
    case 'remove': return t('runner.log.remove');
    default: return e.kind;
  }
}

/** The second line of a row: what exactly went, in the words of the kind. */
function detailText(e: RunnerLogEntry): string {
  if (e.kind === 'clean') {
    const list = (e.data?.names ?? []).join(', ');
    return list ? t('runner.log.names', { list }) : '';
  }
  if (e.kind === 'auto' && e.data?.action !== 'switch') {
    const list = (e.data?.tags ?? []).join(', ');
    return list ? t('runner.log.tags', { list }) : '';
  }
  if (e.kind === 'disk') {
    const d = e.data ?? {};
    const parts: string[] = [];
    if (d.stopped) parts.push(t('runner.log.diskStopped', { n: d.stopped }));
    if (d.builders) parts.push(t('runner.log.diskBuilders', { n: d.builders }));
    if (d.volumes) parts.push(t('runner.log.diskVolumes', { n: d.volumes }));
    if (d.images) parts.push(t('runner.log.diskImages'));
    if (d.cache) parts.push(t('runner.log.diskCache'));
    return parts.join(' · ');
  }
  if (e.kind === 'remove') {
    const list = (e.data?.warned ?? []).join(', ');
    return list ? t('runner.log.warned', { list }) : '';
  }
  return '';
}

export function RunnerLog(props: { runner: string; stamp: string }): JSX.Element {
  const [kind, setKind] = useState('');
  const [word, setWord] = useState('');
  const [query, setQuery] = useState('');
  const [limit, setLimit] = useState(100);
  const [data, setData] = useState<RunnerLogData | null>(null);
  const [error, setError] = useState('');
  const [loading, setLoading] = useState(false);

  // Typing is cheap for the page and not for the server: the word is asked for after a pause.
  useEffect(() => {
    const id = setTimeout(() => {
      setQuery(word);
      setLimit(100);
    }, 300);
    return () => clearTimeout(id);
  }, [word]);

  useEffect(() => {
    let alive = true;
    const params = new URLSearchParams({ id: props.runner, limit: String(limit) });
    if (kind) params.set('kind', kind);
    if (query.trim()) params.set('q', query.trim());
    setLoading(true);
    api<RunnerLogData>(`/api/runner/log?${params.toString()}`)
      .then((d) => {
        if (!alive) return;
        setData(d);
        setError('');
      })
      .catch((e: Error) => {
        if (alive) setError(e.message);
      })
      .finally(() => {
        if (alive) setLoading(false);
      });
    return () => { alive = false; };
  }, [props.runner, props.stamp, kind, query, limit]);

  const filtered = !!kind || !!query.trim();
  const entries = data?.entries ?? [];
  return (
    <section class="rf-log" id="rf-log" data-kind={kind} data-rows={entries.length}>
      <h3>{t('runner.log')} <span class="cnt">{entries.length}</span></h3>
      {data && <p class="hint" id="rf-log-hint">{t('runner.logHint', { days: data.keep_days })}</p>}
      <div class="rf-log-tools">
        <select
          class="field" id="rf-log-kind" aria-label={t('runner.log.filter')} value={kind}
          onChange={(ev) => { setKind((ev.target as HTMLSelectElement).value); setLimit(100); }}
        >
          <option value="">{t('runner.log.all')}</option>
          {FILTERS.map((f) => <option key={f} value={f}>{t(`runner.log.kind.${f}`)}</option>)}
        </select>
        <input
          class="field" id="rf-log-q" type="search" aria-label={t('runner.log.search')}
          placeholder={t('runner.log.search')} value={word}
          onInput={(ev) => setWord((ev.target as HTMLInputElement).value)}
        />
      </div>
      {error && <p class="hint bad" id="rf-log-error">{t('runner.logFailed', { why: error })}</p>}
      {loading && !data && <div class="hint" id="rf-log-loading">{t('runner.logLoading')}</div>}
      {!error && entries.length === 0 && !loading && (
        <div class="hint" id="rf-log-empty">{filtered ? t('runner.logNoMatch') : t('runner.logNone')}</div>
      )}
      {entries.length > 0 && (
        <table class="rft rf-log-table" id="rf-log-table">
          <thead><tr><th>{t('runner.log.when')}</th><th>{t('runner.log.who')}</th><th>{t('runner.log.what')}</th></tr></thead>
          <tbody>
            {entries.map((e) => (
              <tr key={e.n} data-kind={e.kind} data-ok={e.ok ? '1' : '0'} data-who={e.who}>
                <td class="rf-log-when">
                  <div>{dayTimeOf(e.at)}</div>
                  {e.kind === 'down' && e.until > 0 && <div class="hint">{dayTimeOf(e.until)}</div>}
                </td>
                <td><span class={`chip rf-log-who ${e.who}`}>{whoText(e)}</span></td>
                <td>
                  <div class="rf-log-what">{whatText(e)}</div>
                  {detailText(e) && <div class="hint">{detailText(e)}</div>}
                  {e.why && (e.ok
                    ? <div class="hint">{t('runner.log.why', { why: e.why })}</div>
                    : <div class="hint bad">{e.kind === 'down' ? t('runner.log.why', { why: e.why }) : t('runner.log.failed', { why: e.why })}</div>)}
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
      {data?.more && (
        <div class="act">
          <button class="btn sm" type="button" id="rf-log-more" onClick={() => setLimit((n) => Math.min(n + 100, 500))}>{t('runner.log.more')}</button>
        </div>
      )}
    </section>
  );
}
