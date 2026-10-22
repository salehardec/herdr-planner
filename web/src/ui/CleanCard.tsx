/* «Было → стало»: the card that removes forgotten containers of a shipyard. It shows the plan first, asks for the
   second press, and sends only the exact ids of that plan; the server removes nothing else. */
import type { JSX } from 'preact';
import { useEffect, useState } from 'preact/hooks';
import { armed } from '../store';
import { confirmStep } from '../actions';
import { api, post } from '../api';
import { t, tn } from '../i18n';
import { toast } from '../toast';
import { duration } from '../util/format';
import { Gl } from './Gl';

interface Item { id: string; name: string; task: string; status: string; state: string; age_ms: number; mem_bytes: number; size_bytes: number }
interface Counts { containers: number; networks: number; volumes: number; bytes: number }
interface Preview {
  ok: boolean;
  runner: string;
  containers: Item[];
  networks: Item[];
  volumes: Item[];
  was: Counts;
  now: Counts;
  unsure: string[];
  token: string;
  expires_ms: number;
}

const mb = (n: number): string => `${(n / (1024 * 1024)).toFixed(n >= 100 * 1024 * 1024 ? 0 : 1)} MB`;

export function CleanCard(props: { runner: string; ids?: string[]; onDone?: () => void; onClose?: () => void }): JSX.Element {
  const [plan, setPlan] = useState<Preview | null>(null);
  const [err, setErr] = useState('');
  const [busy, setBusy] = useState(false);
  const [picked, setPicked] = useState<Record<string, boolean>>({});
  const key = `rc:${props.runner}`;
  const load = async (): Promise<void> => {
    setPlan(null);
    setErr('');
    try {
      const ids = props.ids && props.ids.length ? props.ids : undefined;
      const p = await post<Preview>('/api/runner/clean/preview', { runner: props.runner, ids });
      setPlan(p);
      const next: Record<string, boolean> = {};
      for (const c of p.containers) next[c.id] = !p.unsure.includes(c.id);
      setPicked(next);
    } catch (e) {
      setErr((e as Error).message);
    }
  };
  useEffect(() => { void load(); }, [props.runner, (props.ids ?? []).join(',')]);
  const chosen = (): string[] => Object.entries(picked).filter(([, v]) => v).map(([id]) => id);
  const run = (): void => {
    const p = plan;
    if (!p) return;
    const ids = chosen();
    if (!ids.length) { toast(t('runner.cleanNone'), { icon: 'failed' }); return; }
    confirmStep(key, async () => {
      setBusy(true);
      try {
        const r = await post<{ ok: boolean; removed: { containers: number; networks: number; volumes: number }; failed: { id: string; error: string }[] }>(
          '/api/runner/clean', { runner: props.runner, token: p.token, ids, confirm: true },
        );
        toast(t('runner.cleanDone', { n: r.removed.containers }), { icon: r.ok ? 'done' : 'failed' });
        props.onDone?.();
        props.onClose?.();
      } catch (e) {
        toast((e as Error).message, { icon: 'failed' });
      } finally {
        setBusy(false);
      }
    });
  };
  const p = plan;
  return (
    <section class="rf-clean" data-clean={props.runner}>
      <div class="block-hd">
        <h3>{t('runner.cleanTitle')}</h3>
        <button class="btn icon sm" type="button" aria-label={t('close')} onClick={() => props.onClose?.()}><Gl n="close" /></button>
      </div>
      {!p && !err && <div class="hint">{t('runner.cleanLoading')}</div>}
      {err && <><p class="hint bad">{err}</p><button class="btn sm" type="button" onClick={() => void load()}>{t('runner.retry')}</button></>}
      {p && (
        <>
          <table class="rft">
            <thead><tr><th /><th>{t('runner.colContainer')}</th><th>{t('runner.colTask')}</th><th>{t('runner.colLives')}</th><th>{t('runner.colMemory')}</th></tr></thead>
            <tbody>
              {p.containers.map((c) => (
                <tr key={c.id}>
                  <td><input type="checkbox" checked={!!picked[c.id]} onChange={(e) => { setPicked({ ...picked, [c.id]: (e.target as HTMLInputElement).checked }); }} /></td>
                  <td><code>{c.name}</code>{p.unsure.includes(c.id) && <span class="hint"> · {t('runner.unsureShort')}</span>}</td>
                  <td>{c.task}</td>
                  <td>{duration(c.age_ms, t)}</td>
                  <td>{mb(c.mem_bytes)}</td>
                </tr>
              ))}
            </tbody>
          </table>
          <p class="rf-was" id="rf-was">
            {t('runner.wasNow', {
              a: p.was.containers, b: p.now.containers, na: p.was.networks, nb: p.now.networks,
              va: p.was.volumes, vb: p.now.volumes, mb: mb(p.was.bytes),
            })}
          </p>
          {(p.networks.length > 0 || p.volumes.length > 0) && (
            <div class="hint">{tn('runner.cleanUnits', p.networks.length + p.volumes.length, {
              nets: p.networks.map((n) => n.name).join(', '), vols: p.volumes.map((v) => v.name).join(', '),
            })}</div>
          )}
          {p.unsure.length > 0 && <div class="hint">{t('runner.unsureHint')}</div>}
          <div class="act">
            <button class={`btn danger ${armed.value?.key === key ? 'arm' : ''}`} type="button" id="rf-clean-go" disabled={busy} onClick={run}>
              {armed.value?.key === key ? t('runner.cleanConfirm') : t('runner.cleanGo')}
            </button>
          </div>
        </>
      )}
    </section>
  );
}

void api;
