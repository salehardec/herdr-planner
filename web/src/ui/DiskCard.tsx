/* «Уборка диска» of a shipyard: «Посчитать место» counts what the docker of the machine holds that nobody uses, the card
   shows it by kind with a tick per kind and per item (volumes unticked, with a warning), and the cleaning runs only
   the ticked ones after the second press. A running builder and a foreign container are never offered. */
import type { JSX } from 'preact';
import { useEffect, useRef, useState } from 'preact/hooks';
import { armed } from '../store';
import { confirmStep } from '../actions';
import { post } from '../api';
import { prepareAnswer } from '../nav';
import { t, tn } from '../i18n';
import { toast } from '../toast';
import { Gl } from './Gl';

interface Item { key: string; name: string; bytes: number; kind: string; sub: string; on: boolean; locked: string }
interface Part { items: Item[]; kept: number; more: number; bytes: number }
interface Report {
  ok: boolean;
  runner: string;
  token: string;
  disk_size: number;
  disk_avail: number;
  images: { unused: number; total: number; bytes: number };
  cache: { total: number; bytes: number };
  stopped: Part;
  builders: Part;
  volumes: Part;
}
interface Step { kind: string; ok: boolean; count: number; error: string; reclaimed: string }
interface Done { ok: boolean; steps: Step[]; bytes: number; after: Report | null }

/** Docker prints decimal sizes, so the card does too. */
export function size(n: number): string {
  if (n >= 1e9) return `${(n / 1e9).toFixed(n >= 1e11 ? 0 : 1)} ${t('disk.unitGb')}`;
  if (n >= 1e6) return `${(n / 1e6).toFixed(n >= 1e8 ? 0 : 1)} ${t('disk.unitMb')}`;
  return `${Math.round(n / 1e3)} ${t('disk.unitKb')}`;
}

function kindTick(items: Item[], picked: Set<string>): { all: boolean; some: boolean; free: Item[] } {
  const free = items.filter((i) => !i.locked);
  const n = free.filter((i) => picked.has(i.key)).length;
  return { all: free.length > 0 && n === free.length, some: n > 0 && n < free.length, free };
}

function KindBox(props: { id: string; items: Item[]; picked: Set<string>; onToggle: (keys: string[], on: boolean) => void; disabled?: boolean }): JSX.Element {
  const ref = useRef<HTMLInputElement>(null);
  const k = kindTick(props.items, props.picked);
  useEffect(() => { if (ref.current) ref.current.indeterminate = k.some; });
  return (
    <input ref={ref} type="checkbox" data-kind={props.id} checked={k.all} disabled={props.disabled || k.free.length === 0}
      onChange={(e) => props.onToggle(k.free.map((i) => i.key), (e.target as HTMLInputElement).checked)} />
  );
}

function itemNote(i: Item): string {
  if (i.locked === 'running') return t('disk.lockedRunning');
  if (i.kind === 'unsure') return t('runner.unsureShort');
  if (i.kind === 'state') return t('disk.kindState');
  return i.sub;
}

function Rows(props: { items: Item[]; picked: Set<string>; onToggle: (keys: string[], on: boolean) => void }): JSX.Element {
  return (
    <table class="rft dk-items">
      <tbody>
        {props.items.map((i) => (
          <tr key={i.key} class={i.locked ? 'dim' : ''} data-key={i.key} data-locked={i.locked || undefined}>
            <td><input type="checkbox" checked={props.picked.has(i.key)} disabled={!!i.locked} onChange={(e) => props.onToggle([i.key], (e.target as HTMLInputElement).checked)} /></td>
            <td><code>{i.name}</code></td>
            <td><span class="hint">{itemNote(i)}</span></td>
            <td>{i.bytes > 0 ? size(i.bytes) : '—'}</td>
          </tr>
        ))}
      </tbody>
    </table>
  );
}

export function DiskCard(props: { runner: string; onClose?: () => void }): JSX.Element {
  const [rep, setRep] = useState<Report | null>(null);
  const [err, setErr] = useState('');
  const [busy, setBusy] = useState(false);
  const [done, setDone] = useState<Done | null>(null);
  const [picked, setPicked] = useState<Set<string>>(new Set());
  const [images, setImages] = useState(false);
  const [cache, setCache] = useState(false);
  const key = `dk:${props.runner}`;
  const take = (r: Report): void => {
    setRep(r);
    setPicked(new Set([...r.stopped.items, ...r.builders.items, ...r.volumes.items].filter((i) => i.on && !i.locked).map((i) => i.key)));
    setImages(r.images.unused > 0);
    setCache(r.cache.bytes > 0);
  };
  const load = async (): Promise<void> => {
    setRep(null);
    setErr('');
    setDone(null);
    try {
      take(await post<Report>('/api/runner/disk', { id: props.runner }));
    } catch (e) {
      setErr((e as Error).message);
    }
  };
  useEffect(() => { void load(); }, [props.runner]);
  const toggle = (keys: string[], on: boolean): void => {
    const next = new Set(picked);
    for (const k of keys) { if (on) next.add(k); else next.delete(k); }
    setPicked(next);
  };
  const chosen = (part: Part): string[] => part.items.filter((i) => picked.has(i.key) && !i.locked).map((i) => i.key);
  const freed = (r: Report): number =>
    (images ? r.images.bytes : 0) + (cache ? r.cache.bytes : 0)
    + [...r.stopped.items, ...r.builders.items, ...r.volumes.items].filter((i) => picked.has(i.key) && !i.locked).reduce((a, i) => a + i.bytes, 0);
  const nothing = (r: Report): boolean => !images && !cache && chosen(r.stopped).length + chosen(r.builders).length + chosen(r.volumes).length === 0;
  const run = (): void => {
    const r = rep;
    if (!r) return;
    if (nothing(r)) { toast(t('disk.none'), { icon: 'failed' }); return; }
    confirmStep(key, async () => {
      setBusy(true);
      const answered = prepareAnswer(props.runner, 'runner');
      try {
        const res = await post<Done>('/api/runner/disk/clean', {
          runner: props.runner, token: r.token, confirm: true, images, cache,
          stopped: chosen(r.stopped), builders: chosen(r.builders), volumes: chosen(r.volumes),
        });
        setDone(res);
        if (res.after) take(res.after);
        toast(res.ok ? t('disk.done', { size: size(res.bytes) }) : t('disk.partial'), { icon: res.ok ? 'done' : 'failed' });
        void answered();
      } catch (e) {
        toast((e as Error).message, { icon: 'failed' });
      } finally {
        setBusy(false);
      }
    });
  };
  const r = rep;
  const gain = r ? freed(r) : 0;
  const armedNow = armed.value?.key === key;
  return (
    <section class="rf-clean rf-disk" id="rf-disk" data-disk={props.runner}>
      <div class="block-hd">
        <h3>{t('disk.title')}</h3>
        <button class="btn icon sm" type="button" aria-label={t('close')} onClick={() => props.onClose?.()}><Gl n="close" /></button>
      </div>
      {!r && !err && <div class="hint">{t('disk.counting')}</div>}
      {err && <><p class="hint bad">{err}</p><button class="btn sm" type="button" onClick={() => void load()}>{t('runner.retry')}</button></>}
      {done && (
        <ul class="dk-steps" id="dk-steps">
          {done.steps.map((s, i) => (
            <li key={i} class={s.ok ? 'w-ok' : 'bad'}>
              {t(`disk.step.${s.kind}`)}: {s.ok ? (s.reclaimed || t('disk.stepOk')) : s.error.split('\n')[0]}
            </li>
          ))}
        </ul>
      )}
      {r && (
        <>
          <div class="dk-kind" data-part="images">
            <label>
              <input type="checkbox" data-kind="images" checked={images} disabled={r.images.unused === 0} onChange={(e) => setImages((e.target as HTMLInputElement).checked)} />
              <b>{t('disk.images')}</b> <span class="dk-sum">{r.images.unused > 0 ? t('disk.imagesLine', { n: r.images.unused, total: r.images.total, size: size(r.images.bytes) }) : t('disk.nothing')}</span>
            </label>
            <div class="hint">{t('disk.imagesHint')}</div>
          </div>
          <div class="dk-kind" data-part="cache">
            <label>
              <input type="checkbox" data-kind="cache" checked={cache} disabled={r.cache.bytes === 0} onChange={(e) => setCache((e.target as HTMLInputElement).checked)} />
              <b>{t('disk.cache')}</b> <span class="dk-sum">{r.cache.bytes > 0 ? `≈ ${size(r.cache.bytes)}` : t('disk.nothing')}</span>
            </label>
          </div>
          <div class="dk-kind" data-part="stopped">
            <label>
              <KindBox id="stopped" items={r.stopped.items} picked={picked} onToggle={toggle} />
              <b>{t('disk.stopped')}</b> <span class="dk-sum">{r.stopped.items.length > 0 ? tn('disk.stoppedLine', r.stopped.items.length, { size: size(r.stopped.bytes) }) : t('disk.nothing')}</span>
            </label>
            {r.stopped.items.length > 0 && <Rows items={r.stopped.items} picked={picked} onToggle={toggle} />}
            <div class="hint">{t('disk.stoppedHint', { n: r.stopped.kept })}</div>
          </div>
          <div class="dk-kind" data-part="builders">
            <label>
              <KindBox id="builders" items={r.builders.items} picked={picked} onToggle={toggle} />
              <b>{t('disk.builders')}</b> <span class="dk-sum">{r.builders.items.length > 0 ? tn('disk.buildersLine', r.builders.items.length, { size: size(r.builders.bytes) }) : t('disk.nothing')}</span>
            </label>
            {r.builders.items.length > 0 && <Rows items={r.builders.items} picked={picked} onToggle={toggle} />}
            <div class="hint">{t('disk.buildersHint')}</div>
          </div>
          <div class="dk-kind dk-vols" data-part="volumes">
            <label>
              <KindBox id="volumes" items={r.volumes.items} picked={picked} onToggle={toggle} />
              <b>{t('disk.volumes')}</b> <span class="dk-sum">{r.volumes.items.length > 0 ? tn('disk.volumesLine', r.volumes.items.length, { size: size(r.volumes.bytes) }) : t('disk.nothing')}</span>
            </label>
            <div class="hint warn" id="dk-vol-warn">{t('disk.volumesWarn', { n: r.volumes.kept })}</div>
            {r.volumes.items.length > 0 && (
              <details>
                <summary>{t('disk.showVolumes')}</summary>
                <Rows items={r.volumes.items} picked={picked} onToggle={toggle} />
                {r.volumes.more > 0 && <div class="hint">{t('disk.more', { n: r.volumes.more })}</div>}
              </details>
            )}
          </div>
          <p class="rf-was" id="dk-was">
            {r.disk_size > 0
              ? t('disk.wasNow', { a: size(r.disk_avail), b: size(r.disk_avail + gain), g: size(gain) })
              : t('disk.freed', { g: size(gain) })}
          </p>
          <div class="act">
            <button class={`btn danger ${armedNow ? 'arm' : ''}`} type="button" id="dk-go" disabled={busy} onClick={run}>
              {armedNow ? t('disk.goConfirm') : t('disk.go')}
            </button>
            <button class="btn" type="button" id="dk-again" disabled={busy} onClick={() => void load()}>{t('disk.again')}</button>
          </div>
        </>
      )}
    </section>
  );
}
