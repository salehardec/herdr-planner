/* The rest room («Комната отдыха»): a wallpaper, a console with two games, the notices of «Ждёт тебя» and the way back.
   The room is a layer over the world, kept in the page all the time: leaving it only hides it, so the game waits on pause. */
import type { JSX } from 'preact';
import { useEffect, useRef, useState } from 'preact/hooks';
import { requests } from '../store';
import { t } from '../i18n';
import { toast } from '../toast';
import { effectiveWidth } from '../scale';
import { backLabel, irisText } from '../restroom/labels';
import { bindPad } from '../restroom/touch';
import { drawScreen } from '../restroom/draw';
import { H, W, type Pad } from '../restroom/game';
import { readWallpaper } from '../restroom/image';
import { freshNotes } from '../restroom/notes';
import { WALLS, wallCss } from '../restroom/walls';
import {
  customWall, game, iris, leave, noteLater, noteBase, origin, roomOpen, setCustomWall, setWall, touch, wallId,
} from '../restroom/state';
import './restroom.css';

/** The buttons on the shell of the console: the same buttons as on the keyboard. */
const SHELL: [string, Pad, string][] = [['u', 'up', 'restroom.pad.up'], ['l', 'left', 'restroom.pad.left'], ['r', 'right', 'restroom.pad.right'], ['d', 'down', 'restroom.pad.down']];

function PadButton(props: { pad: Pad; label: string; cls?: string }): JSX.Element {
  const ref = useRef<HTMLButtonElement>(null);
  useEffect(() => ref.current ? bindPad(ref.current, props.pad, touch) : undefined, [props.pad]);
  return <button ref={ref} type="button" class={props.cls} tabIndex={-1} data-pad={props.pad} aria-label={props.label} />;
}

function Screen(props: { mobile?: boolean }): JSX.Element {
  const ref = useRef<HTMLCanvasElement>(null);
  const open = roomOpen.value;
  useEffect(() => {
    const g = ref.current?.getContext('2d');
    if (!g) return;
    g.imageSmoothingEnabled = false;
    drawScreen(g, game, performance.now(), props.mobile);
    if (!open) return;
    let last = performance.now();
    let id = requestAnimationFrame(function frame(now) {
      id = requestAnimationFrame(frame);
      touch.advance(now - last);
      game.advance(now - last);
      last = now;
      drawScreen(g, game, now, props.mobile);
    });
    return () => cancelAnimationFrame(id);
  }, [open, props.mobile]);
  return <canvas id="rm-screen" ref={ref} width={W} height={H} role="img" aria-label={t('restroom.screen')} />;
}

function Wallpapers(): JSX.Element {
  // on a narrow window the panel starts folded, so that it does not cover the console
  const [min, setMin] = useState(() => effectiveWidth() < 1500);
  const file = useRef<HTMLInputElement>(null);
  const custom = customWall.value;
  const onFile = async (e: Event): Promise<void> => {
    const input = e.target as HTMLInputElement;
    const f = input.files?.[0];
    input.value = '';
    if (!f) return;
    try {
      if (!setCustomWall(await readWallpaper(f))) toast(t('restroom.wall.notKept'), { icon: 'failed' });
    } catch {
      toast(t('restroom.wall.notImage'), { icon: 'failed' });
    }
  };
  return (
    <div id="rm-wp" class={`rm-panel ${min ? 'min' : ''}`}>
      <header>
        <b>{t('restroom.wall.title')}</b>
        <button
          class="btn sm" id="rm-wp-min" type="button" aria-expanded={!min} aria-label={t(min ? 'restroom.wall.show' : 'restroom.wall.hide')}
          onClick={() => setMin(!min)}
        >{min ? '+' : '–'}</button>
      </header>
      <div class="thumbs" id="rm-thumbs">
        {WALLS.map((w) => (
          <button key={w.id} class="rm-thumb" type="button" data-wall={w.id} aria-pressed={wallId.value === w.id} onClick={() => setWall(w.id)}>
            <span class="im" style={`background-image:${wallCss(w)}`} /><span>{t(w.name)}</span>
          </button>
        ))}
        {custom && (
          <button class="rm-thumb" type="button" data-wall="custom" aria-pressed={wallId.value === 'custom'} onClick={() => setWall('custom')}>
            <span class="im" style={`background-image:url("${custom}")`} /><span>{t('restroom.wall.own')}</span>
          </button>
        )}
        <button class="rm-thumb browse" id="rm-browse" type="button" onClick={() => file.current?.click()}>
          <span class="im">+</span><span>{t('restroom.wall.browse')}</span>
        </button>
      </div>
      <input ref={file} id="rm-file" type="file" accept="image/*" hidden onChange={(e) => void onFile(e)} />
    </div>
  );
}

function Notes(): JSX.Element {
  const list = roomOpen.value ? freshNotes(noteBase.value, noteLater.value, requests.value).slice(0, 3) : [];
  return (
    <div id="rm-notes" aria-live="polite">
      {list.map((r) => (
        <div key={r.key} class={`rm-note rm-panel s-${r.task?.status ?? 'waiting'}`} data-note={r.id}>
          <div class="k"><i /><b>{r.why ? t('restroom.note.waits', { why: r.why }) : r.title}</b><span>· {r.tag}</span></div>
          <p>{r.text || r.title}</p>
          <div class="row">
            <button class="btn rm-btn" type="button" data-act="later" onClick={() => { noteLater.value = new Set([...noteLater.value, r.key]); }}>{t('restroom.note.later')}</button>
            <button class="btn rm-btn pri" type="button" data-act="fly" onClick={() => leave({ type: r.type, id: r.id, tag: r.tag })}>{t('restroom.note.fly')}</button>
          </div>
        </div>
      ))}
    </div>
  );
}

/** While the room is open nothing behind it takes the focus or is read out. */
function useInertBehind(room: { current: HTMLElement | null }, on: boolean): void {
  useEffect(() => {
    const win = document.getElementById('win') ?? document.getElementById('app');
    if (!win || !room.current) return;
    const others = [...win.children].flatMap((c) => (c.id === 'app' ? [...c.children] : [c])).filter((c) => c !== room.current && c.id !== 'rm-iris') as HTMLElement[];
    others.forEach((c) => { c.inert = on; });
    return () => others.forEach((c) => { c.inert = false; });
  }, [on]);
}

export function Restroom(props: { mobile?: boolean } = {}): JSX.Element {
  const room = useRef<HTMLElement>(null);
  const open = roomOpen.value;
  const ir = iris.value;
  useInertBehind(room, open);
  // a narrow window scrolls the room: it opens from the top
  useEffect(() => { if (open && room.current) { room.current.scrollTop = 0; room.current.focus({ preventScroll: true }); } }, [open]);
  const wall = wallId.value === 'custom' && customWall.value ? `url("${customWall.value}")` : wallCss(WALLS.find((w) => w.id === wallId.value) ?? WALLS[0]);
  return (
    <>
      <section id="rm" class={`rm ${props.mobile ? 'rm-mobile' : ''} ${open ? 'on' : ''}`} ref={room} tabIndex={-1} inert={!open} aria-label={t('restroom.title')} aria-hidden={!open}>
        <div class="rm-wall"><div id="rm-wall-img" key={wallId.value} style={`background-image:${wall}`} data-wall={wallId.value} /></div>
        <div class="rm-vig" />
        <div id="rm-exit">
          <button class="btn rm-btn" id="rm-back" type="button" onClick={() => leave()}>← <span>{backLabel(origin.value)}</span> <kbd>Esc</kbd></button>
        </div>
        <Notes />
        <div id="rm-console">
          <div class="rm-shell">
            <div class="rm-brand"><span>{t('restroom.brand')}</span><small>{t('restroom.brand.tag')}</small></div>
            <div class="rm-bezel">
              <div class="rm-led"><i class={open ? 'on' : ''} />{t('restroom.power')}</div>
              <Screen mobile={props.mobile} />
            </div>
            <div class="rm-ctl">
              <div class="rm-dpad">{SHELL.map(([c, pad, label]) => <PadButton key={pad} pad={pad} cls={c} label={t(label)} />)}</div>
              <div class="rm-mid">
                <div class="rm-pill"><PadButton pad="select" label={t('restroom.pad.select')} />{t('restroom.pad.menu')}</div>
                <div class="rm-pill"><PadButton pad="start" label={t('restroom.pad.start')} />{t('restroom.pad.pause')}</div>
              </div>
              <div class="rm-ab"><div><PadButton pad="b" label="B" />B</div><div><PadButton pad="a" label={t('restroom.pad.a')} />A</div></div>
            </div>
            <div class="rm-speaker"><i /><i /><i /><i /><i /></div>
          </div>
          <div class="rm-touch-hint">{t('restroom.hint.touch')}</div>
          <div class="rm-hint">
            <kbd>←</kbd><kbd>→</kbd> {t('restroom.hint.move')} · <kbd>{t('restroom.key.space')}</kbd> {t('restroom.hint.fire')} · <kbd>P</kbd> {t('restroom.hint.pause')} · <kbd>M</kbd> {t('restroom.hint.menu')} · <kbd>Esc</kbd> {t('restroom.hint.exit')}
          </div>
        </div>
        <Wallpapers />
      </section>
      <div id="rm-iris" class={`${ir ? 'on' : ''} ${ir?.shut ? 'shut' : ''}`} aria-hidden="true">
        <div class="l" /><div class="r" /><div class="lbl">{ir ? irisText(ir.label) : ''}</div>
      </div>
    </>
  );
}
