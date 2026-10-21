import type { JSX } from 'preact';
import { classOfSaved, fileUrl, type PendingFile, type SavedFile } from '../attach';
import { t } from '../i18n';
import { Gl } from './Gl';
import './attach.css';

const sizeText = (n: number): string => (n >= 1048576 ? `${(n / 1048576).toFixed(1)} MB` : n >= 1024 ? `${Math.round(n / 1024)} KB` : `${n} B`);

/** Files that are about to be saved (a message, a todo item): a preview of each, and a way to take it away. */
export function PendingFiles(props: { files: PendingFile[]; onRemove: (key: number) => void; id?: string }): JSX.Element | null {
  if (!props.files.length) return null;
  return (
    <div class="attach" id={props.id}>
      {props.files.map((p) => (
        <div class={`att ${p.cls}`} data-state={p.state} data-cls={p.cls} key={p.key} title={p.error ? `${p.name} — ${p.error}` : `${p.name} · ${sizeText(p.size)}`}>
          {p.cls === 'image' && <img src={p.local} alt={p.name} />}
          {p.cls === 'audio' && (
            <>
              <span class="nm"><Gl n="sound" />{p.name}</span>
              <audio controls preload="metadata" src={p.local} />
            </>
          )}
          {p.cls === 'text' && (
            <>
              <span class="nm"><Gl n="file" />{p.name}</span>
              <span class="ex">{p.excerpt}</span>
            </>
          )}
          {p.state === 'uploading' && <span class="wait">{t('chat.uploading')}</span>}
          <button class="rm" type="button" data-att-rm={p.key} aria-label={`${t('chat.remove')} ${p.name}`} onClick={() => props.onRemove(p.key)}>×</button>
        </div>
      ))}
    </div>
  );
}

/** Files already kept on the server: thumbnails for images, a player for sound, a link for text. */
export function SavedFiles(props: { list: SavedFile[]; onRemove?: (id: string) => void; class?: string }): JSX.Element | null {
  const items = props.list.filter((a) => a && a.id);
  if (!items.length) return null;
  return (
    <div class={`saved ${props.class ?? ''}`}>
      {items.map((a) => {
        const cls = classOfSaved(a);
        const rm = props.onRemove && <button class="rm" type="button" data-file-rm={a.id} aria-label={`${t('chat.remove')} ${a.name}`} onClick={() => props.onRemove?.(a.id)}>×</button>;
        if (cls === 'image') {
          return (
            <span class="sv img" key={a.id}>
              <a class="shot" href={fileUrl(a)} target="_blank" rel="noopener"><img src={fileUrl(a)} alt={a.name || t('chat.image')} title={a.name} loading="lazy" /></a>
              {rm}
            </span>
          );
        }
        if (cls === 'audio') {
          return (
            <span class="sv aud" key={a.id}>
              <span class="nm"><Gl n="sound" />{a.name}</span>
              <audio controls preload="none" src={fileUrl(a)} />
              {rm}
            </span>
          );
        }
        return (
          <span class="sv txt" key={a.id}>
            <a href={fileUrl(a)} target="_blank" rel="noopener" title={a.name}><Gl n="file" />{a.name}</a>
            {rm}
          </span>
        );
      })}
    </div>
  );
}
