import { signal } from '@preact/signals';
import type { JSX } from 'preact';
import { post } from '../api';
import { lang, t } from '../i18n';
import { needsTranslation } from '../util/lang';
import { RichText } from './RichText';
import { toast } from '../toast';

const cache = new Map<string, string>();
const busy = signal<Set<string>>(new Set());
const showOriginal = signal<Set<string>>(new Set());

const keyOf = (text: string): string => `${lang.value}|${text.length}|${text.slice(0, 80)}|${text.slice(-40)}`;

/** A text of an agent, a review or a board line, in its own language, with a button that translates it. */
export function Translatable(props: { text: string; class?: string; plain?: boolean }): JSX.Element {
  const text = props.text;
  const k = keyOf(text);
  const target = lang.value;
  const can = needsTranslation(text, target);
  const out = cache.get(k);
  const original = showOriginal.value.has(k);
  const loading = busy.value.has(k);
  const run = async (): Promise<void> => {
    if (out) { const s = new Set(showOriginal.value); s.delete(k); showOriginal.value = s; return; }
    busy.value = new Set(busy.value).add(k);
    try {
      const r = await post<{ ok: boolean; text?: string; error?: string }>('/api/translate', { text, to: target });
      if (r.ok && r.text) cache.set(k, r.text);
      else toast(r.error || t('translate.failed'), { icon: 'failed' });
    } catch (e) {
      toast(t('translate.failedWhy', { why: (e as Error).message }), { icon: 'failed' });
    } finally {
      const b = new Set(busy.value);
      b.delete(k);
      busy.value = b;
    }
  };
  const shown = out && !original ? out : text;
  return (
    <div class={`tr ${props.class ?? ''}`}>
      {props.plain ? <span>{shown}</span> : <RichText text={shown} />}
      {(can || out) && (
        <div class="trbar">
          {out && !original ? (
            <>
              <span class="hint">{t('translate.done')}</span>
              <button type="button" class="btn sm" data-act="translate-orig" onClick={() => { const s = new Set(showOriginal.value); s.add(k); showOriginal.value = s; }}>{t('translate.original')}</button>
            </>
          ) : (
            <button type="button" class="btn sm" data-act="translate" disabled={loading} onClick={() => void run()}>
              {loading ? t('translate.working') : out ? t('translate.show') : t('translate.button')}
            </button>
          )}
        </div>
      )}
    </div>
  );
}
