import type { JSX } from 'preact';
import { useEffect, useRef } from 'preact/hooks';
import { fileUrl } from '../attach';
import { t } from '../i18n';
import { closeLightbox, lightbox, stepLightbox } from '../lightbox';
import { Gl } from './Gl';
import './lightbox.css';

/** The picture of a message, large, over the page: ←/→ between the pictures of the message, Esc closes. */
export function Lightbox(): JSX.Element | null {
  const s = lightbox.value;
  const box = useRef<HTMLDivElement>(null);
  const closeBtn = useRef<HTMLButtonElement>(null);
  useEffect(() => {
    if (!s) return;
    closeBtn.current?.focus();
    const onKey = (e: KeyboardEvent): void => {
      if (e.key === 'ArrowLeft') { stepLightbox(-1); e.preventDefault(); e.stopPropagation(); }
      else if (e.key === 'ArrowRight') { stepLightbox(1); e.preventDefault(); e.stopPropagation(); }
      else if (e.key === 'Tab') {
        // the focus stays inside the window
        const items = Array.from(box.current?.querySelectorAll<HTMLElement>('button,a[href]') ?? []).filter((el) => !el.hidden);
        if (!items.length) return;
        const first = items[0];
        const last = items[items.length - 1];
        if (e.shiftKey && document.activeElement === first) { last.focus(); e.preventDefault(); }
        else if (!e.shiftKey && document.activeElement === last) { first.focus(); e.preventDefault(); }
      }
    };
    document.addEventListener('keydown', onKey, true);
    return () => document.removeEventListener('keydown', onKey, true);
  }, [!!s]);
  if (!s) return null;
  const file = s.list[s.at];
  const many = s.list.length > 1;
  return (
    <div class="lightbox" id="lightbox" ref={box} role="dialog" aria-modal="true" aria-label={file.name || t('chat.image')} onClick={(e) => { if (e.target === e.currentTarget) closeLightbox(); }}>
      <div class="lbbar">
        <span class="nm">{file.name}</span>
        <a class="btn sm" href={fileUrl(file)} target="_blank" rel="noopener noreferrer"><Gl n="ext" />{t('chat.lightbox.open')}</a>
        <button class="btn sm" type="button" id="lbclose" ref={closeBtn} onClick={closeLightbox}><Gl n="close" />{t('chat.lightbox.close')}</button>
      </div>
      {many && <button class="nav prev" type="button" aria-label={t('chat.lightbox.prev')} title={t('chat.lightbox.prev')} onClick={() => stepLightbox(-1)}><Gl n="chev" /></button>}
      <img src={fileUrl(file)} alt={file.name || t('chat.image')} />
      {many && <button class="nav next" type="button" aria-label={t('chat.lightbox.next')} title={t('chat.lightbox.next')} onClick={() => stepLightbox(1)}><Gl n="chev" /></button>}
      {many && <div class="cnt">{s.at + 1} / {s.list.length}</div>}
    </div>
  );
}
