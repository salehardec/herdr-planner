/* Stray processes of finished tasks (PLN-53): what an agent or its helper left running in the working copy of a task
   that is done or archived. «Было → стало» first, the second press stops the ticked ones; the server finds each again
   and refuses what is not the same process or no longer a leftover. The automatic cleaning is off until switched on. */
import type { JSX } from 'preact';
import { useEffect, useState } from 'preact/hooks';
import { armed } from '../store';
import { confirmStep } from '../actions';
import { api, post } from '../api';
import { t } from '../i18n';
import { toast } from '../toast';
import { dayTimeOf } from '../util/dates';
import { duration } from '../util/format';
import { memSize } from './usage-api';

interface Stray {
  pid: number; ppid: number; comm: string; cmd: string; cwd: string; task: string; tag: string;
  why: 'done' | 'archived' | 'gone'; rss: number; age_ms: number; start: number; depth: number; shell: boolean; seen_at: number;
}
interface LogRow { n: number; at: number; who: string; ok: boolean; why: string; data: Record<string, any> }
interface Data {
  ok: boolean; board_ok: boolean; supported: boolean; error: string; at: number; now: number; auto: boolean; grace_ms: number; term_wait_ms: number;
  total_rss: number; strays: Stray[]; journal: LogRow[];
  report?: { items: { result: string }[] }; stopped?: number;
}

const keyOf = (s: Stray): string => `${s.pid}:${s.start}`;

function logText(r: LogRow): string {
  const d = r.data ?? {};
  if (d.action === 'switch') return t(d.on ? 'strays.log.on' : 'strays.log.off');
  const n = (d.term ?? 0) + (d.kill ?? 0);
  const v = { n, total: d.count ?? 0, k: d.kill ?? 0, f: d.failed ?? 0 };
  if (d.failed) return t('strays.log.stopFailed', v);
  return t(d.kill ? 'strays.log.stopKill' : 'strays.log.stop', v);
}

export function Strays(): JSX.Element {
  const [data, setData] = useState<Data | null>(null);
  const [err, setErr] = useState('');
  const [busy, setBusy] = useState(false);
  const [picked, setPicked] = useState<Record<string, boolean>>({});
  const [preview, setPreview] = useState(false);
  const take = (d: Data): void => {
    setData(d);
    setPicked((prev) => {
      const next: Record<string, boolean> = {};
      for (const s of d.strays) next[keyOf(s)] = keyOf(s) in prev ? prev[keyOf(s)] : !s.shell;
      return next;
    });
  };
  const load = async (fresh: boolean): Promise<void> => {
    try {
      take(await api<Data>(`/api/strays${fresh ? '?fresh=1' : ''}`));
      setErr('');
    } catch (e) {
      setErr((e as Error).message);
    }
  };
  useEffect(() => {
    void load(false);
    const id = setInterval(() => { void load(false); }, 20000);
    return () => clearInterval(id);
  }, []);
  if (err && !data) {
    return <section class="st-sec" id="st-strays"><h3>{t('strays.title')}</h3><p class="hint bad">{err}</p><button class="btn sm" type="button" onClick={() => void load(true)}>{t('strays.retry')}</button></section>;
  }
  if (!data) return <section class="st-sec" id="st-strays"><h3>{t('strays.title')}</h3><p class="hint">{t('strays.loading')}</p></section>;
  if (!data.supported) return <section class="st-sec" id="st-strays"><h3>{t('strays.title')}</h3><p class="hint" id="st-strays-unsupported">{t('strays.unsupported')}</p></section>;
  const list = data.strays;
  const ticked = list.filter((s) => picked[keyOf(s)]);
  const freed = ticked.reduce((a, s) => a + s.rss, 0);
  const skew = Math.max(0, data.now - data.at);
  const stopKey = 'strays:stop';
  const autoKey = 'strays:auto';
  const graceMin = Math.max(1, Math.round(data.grace_ms / 60000));
  const stop = (): void => {
    if (!ticked.length) { toast(t('strays.noneChosen'), { icon: 'failed' }); return; }
    confirmStep(stopKey, async () => {
      setBusy(true);
      try {
        const r = await post<Data>('/api/strays/stop', { targets: ticked.map((s) => ({ pid: s.pid, start: s.start })), confirm: true });
        const items = r.report?.items ?? [];
        const killed = items.filter((o) => o.result === 'kill').length;
        const failed = items.filter((o) => o.result === 'failed').length;
        const n = r.stopped ?? 0;
        toast(failed ? t('strays.doneFailed', { f: failed }) : n === 0 ? t('strays.doneNone') : killed ? t('strays.doneKill', { n, k: killed }) : t('strays.done', { n }), { icon: failed ? 'failed' : 'done' });
        take(r);
      } catch (e) {
        toast((e as Error).message, { icon: 'failed' });
      } finally {
        setBusy(false);
      }
    });
  };
  const setAuto = async (on: boolean): Promise<void> => {
    try {
      take(await post<Data>('/api/strays/auto', { enabled: on, confirm: true }));
      setPreview(false);
      toast(t(on ? 'strays.autoEnabled' : 'strays.autoDisabled'));
    } catch (e) {
      toast((e as Error).message, { icon: 'failed' });
    }
  };
  const toggleAuto = (): void => {
    if (data.auto) { void setAuto(false); return; }
    setPreview(true);
    confirmStep(autoKey, () => setAuto(true));
  };
  const cleanable = list.filter((s) => !s.shell).length;
  return (
    <section class="st-sec rf-strays" id="st-strays" data-count={list.length} data-auto={data.auto ? 'on' : 'off'}>
      <h3>{t('strays.title')}</h3>
      <p class="hint">{t('strays.text')}</p>
      {!data.ok && <p class="hint bad" id="st-strays-error">{data.error}</p>}
      {data.ok && !data.board_ok && <p class="hint warn" id="st-strays-board">{t('strays.boardDown')}</p>}
      {data.ok && data.board_ok && list.length === 0 && <p class="hint" id="st-strays-none">{t('strays.none')}</p>}
      {list.length > 0 && (
        <div class="rf-clean rf-strays-list">
          <div class="rft-wrap">
            <table class="rft" id="st-strays-table">
              <thead><tr><th /><th>{t('strays.colTask')}</th><th>{t('strays.colPid')}</th><th>{t('strays.colCmd')}</th><th>{t('strays.colLives')}</th><th>{t('strays.colMemory')}</th></tr></thead>
              <tbody>
                {list.map((s) => (
                  <tr key={keyOf(s)} data-pid={s.pid} data-why={s.why}>
                    <td><input type="checkbox" aria-label={String(s.pid)} checked={!!picked[keyOf(s)]} onChange={(e) => setPicked({ ...picked, [keyOf(s)]: (e.target as HTMLInputElement).checked })} /></td>
                    <td><b>{s.tag || s.task}</b><div class="hint">{t(`strays.why.${s.why}`)}</div></td>
                    <td>{s.pid}</td>
                    <td class="st-cmd" style={`--depth:${s.depth}`}>
                      <code>{s.cmd}</code>
                      <div class="hint"><code>{s.cwd}</code>{s.shell && <> · {t('strays.shell')}</>}</div>
                    </td>
                    <td>{duration(s.age_ms + skew, t)}</td>
                    <td>{memSize(s.rss)}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
          <p class="rf-was" id="st-strays-was">{t('strays.wasNow', { a: list.length, b: list.length - ticked.length, mb: memSize(freed) })}</p>
          <p class="hint">{t('strays.how', { s: Math.max(1, Math.round(data.term_wait_ms / 1000)) })}</p>
        </div>
      )}
      <div class="act">
        {list.length > 0 && (
          <button class={`btn danger ${armed.value?.key === stopKey ? 'arm' : ''}`} type="button" id="st-strays-stop" disabled={busy} onClick={stop}>
            {armed.value?.key === stopKey ? t('strays.stopConfirm') : t('strays.stop')}
          </button>
        )}
        <button class="btn" type="button" id="st-strays-check" disabled={busy} onClick={() => void load(true)}>{t('strays.check')}</button>
        <span class="hint">{t('strays.count', { n: list.length })} · {t('strays.checked', { when: dayTimeOf(data.at) })}</span>
      </div>
      <div class="rf-auto st-strays-auto" id="st-strays-auto" data-auto={data.auto ? 'on' : 'off'}>
        <h3>{t('strays.autoTitle')} <span class={`badge s-${data.auto ? 'working' : 'queued'}`}>{data.auto ? t('strays.autoOn') : t('strays.autoOff')}</span></h3>
        <p class="hint">{t('strays.autoText', { m: graceMin })}</p>
        {!data.auto && preview && armed.value?.key === autoKey && <p class="hint warn" id="st-strays-auto-now">{t('strays.autoNow', { n: cleanable, m: graceMin })}</p>}
        <div class="act">
          <button class={`btn ${data.auto ? '' : 'primary'} ${armed.value?.key === autoKey ? 'arm' : ''}`} type="button" id="st-strays-auto-btn" onClick={toggleAuto}>
            {data.auto ? t('strays.autoTurnOff') : armed.value?.key === autoKey ? t('strays.autoConfirm') : t('strays.autoTurnOn')}
          </button>
        </div>
      </div>
      <details class="rf-block st-strays-log" id="st-strays-log">
        <summary>{t('strays.log')}</summary>
        {data.journal.length === 0 && <p class="hint">{t('strays.logNone')}</p>}
        <ul>
          {data.journal.map((r) => (
            <li key={r.n} class={r.ok ? '' : 'bad'}>
              <span class="hint">{dayTimeOf(r.at)} · {t(r.who === 'human' ? 'strays.log.who.human' : 'strays.log.who.auto')}</span> {logText(r)}
              {r.data?.tags?.length > 0 && <div class="hint">{t('strays.log.tags', { list: (r.data.tags as string[]).join(', ') })}</div>}
              {!r.ok && r.why && <div class="hint bad">{r.why}</div>}
            </li>
          ))}
        </ul>
      </details>
    </section>
  );
}
