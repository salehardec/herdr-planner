/* Auto-clean of a shipyard: the switch, the time after a task is finished, and the counter of what it has removed.
   Off by default; the switch is the consent, so switching on asks twice. */
import type { JSX } from 'preact';
import { useEffect, useState } from 'preact/hooks';
import { armed } from '../store';
import { confirmStep } from '../actions';
import { post } from '../api';
import { t } from '../i18n';
import { toast } from '../toast';
import { dayTimeOf } from '../util/dates';
import type { Runner } from '../types';

interface Preview { hours: number; containers: string[]; networks: string[]; volumes: string[]; counts: { containers: number; networks: number; volumes: number } }

export function RunnerAuto(props: { runner: Runner }): JSX.Element {
  const r = props.runner;
  const auto = r.auto ?? { enabled: false, hours: 6 };
  const stats = r.auto_stats ?? { containers: 0, networks: 0, volumes: 0, runs: 0, last_ms: 0, last_line: '' };
  const [hours, setHours] = useState(String(auto.hours));
  const [err, setErr] = useState('');
  const [preview, setPreview] = useState<Preview | null>(null);
  useEffect(() => { setHours(String(auto.hours)); }, [auto.hours, r.id]);
  const send = async (body: { enabled?: boolean; hours?: number }, done: string): Promise<void> => {
    setErr('');
    try {
      await post('/api/runner/auto', { id: r.id, confirm: true, ...body });
      toast(done);
    } catch (e) {
      setErr((e as Error).message);
    }
  };
  const key = `rau:${r.id}`;
  const keyHours = `rauh:${r.id}`;
  /* Switching on or shortening the time removes a batch at once: the first press shows what would go now, the second
     one confirms. */
  const ask = async (k: string, h: number, fire: () => Promise<void>): Promise<void> => {
    if (armed.value?.key !== k) {
      try { setPreview(await post<Preview>('/api/runner/auto/preview', { id: r.id, hours: h })); } catch { setPreview(null); }
    }
    confirmStep(k, async () => { setPreview(null); await fire(); });
  };
  const toggle = (on: boolean): void => {
    if (!on) { void send({ enabled: false }, t('auto.disabled')); return; }
    void ask(key, auto.hours, () => send({ enabled: true }, t('auto.enabled')));
  };
  const save = (): void => {
    const n = Number(hours);
    if (!Number.isInteger(n) || n < 1) { setErr(t('auto.badHours')); return; }
    if (auto.enabled && n < auto.hours) { void ask(keyHours, n, () => send({ hours: n }, t('auto.saved'))); return; }
    void send({ hours: n }, t('auto.saved'));
  };
  const armedHere = armed.value?.key === key || armed.value?.key === keyHours;
  const total = stats.containers + stats.networks + stats.volumes;
  return (
    <section class="rf-auto" id="rf-auto" data-auto={auto.enabled ? 'on' : 'off'}>
      <h3>{t('auto.title')} <span class={`badge s-${auto.enabled ? 'working' : 'queued'}`}>{auto.enabled ? t('auto.stateOn') : t('auto.stateOff')}</span></h3>
      <p class="hint">{t('auto.text')}</p>
      <div class="act">
        <button class={`btn ${auto.enabled ? '' : 'primary'} ${armed.value?.key === key ? 'arm' : ''}`} type="button" id="rf-auto-toggle" onClick={() => toggle(!auto.enabled)}>
          {auto.enabled ? t('auto.turnOff') : armed.value?.key === key ? t('auto.enableConfirm') : t('auto.turnOn')}
        </button>
      </div>
      {armedHere && preview && (
        <div class="rf-auto-preview" id="rf-auto-preview">
          {preview.counts.containers + preview.counts.networks + preview.counts.volumes === 0
            ? <p class="hint">{t('auto.previewNone', { h: preview.hours })}</p>
            : (
              <>
                <p class="hint warn">{t('auto.previewNow', { h: preview.hours, c: preview.counts.containers, n: preview.counts.networks, v: preview.counts.volumes })}</p>
                <ul>
                  {preview.containers.map((x) => <li key={`c${x}`}><code>{x}</code> <span class="hint">{t('auto.kindContainer')}</span></li>)}
                  {preview.networks.map((x) => <li key={`n${x}`}><code>{x}</code> <span class="hint">{t('auto.kindNetwork')}</span></li>)}
                  {preview.volumes.map((x) => <li key={`v${x}`}><code>{x}</code> <span class="hint">{t('auto.kindVolume')}</span></li>)}
                </ul>
              </>
            )}
        </div>
      )}
      <div class="rf-auto-time">
        <span>{t('auto.after')}</span>
        <input class="field" id="rf-auto-hours" type="number" min="1" max="2160" value={hours} onInput={(e) => setHours((e.target as HTMLInputElement).value)} />
        <span>{t('auto.hoursUnit')}</span>
        <button class={`btn sm ${armed.value?.key === keyHours ? 'arm' : ''}`} type="button" id="rf-auto-save" disabled={Number(hours) === auto.hours} onClick={save}>{armed.value?.key === keyHours ? t('auto.saveConfirm') : t('auto.save')}</button>
      </div>
      {err && <p class="hint bad">{err}</p>}
      <p class="rf-auto-count" id="rf-auto-count" data-total={total}>
        {t('auto.count', { c: stats.containers, n: stats.networks, v: stats.volumes })}
        {' · '}
        {stats.last_ms > 0 ? t('auto.last', { when: dayTimeOf(stats.last_ms) }) : t('auto.never')}
      </p>
    </section>
  );
}
