/* The edge of a panel that can be resized: dragged with the mouse, moved with the arrow keys, a double click for the default.
   One handle for every panel — the navigator, the focus card, «Ждёт тебя», the chat (see panel-width.ts). */
import type { JSX } from 'preact';
import { t } from '../i18n';
import { PANELS, dragPanel, maxWidth, panelWidths, resetWidth, stepWidth, widthNow, type PanelId } from '../panel-width';
import { scaleTick } from '../scale';

/** The name of a panel for the label of its handle: «Ширина панели «Фокус»». */
const NAME: Record<PanelId, string> = { nav: 'nav.title', focus: 'focus.title', wait: 'wait.title', chat: 'chat.button' };

/** `side` is the edge of the panel the handle stands on: the left one moves to the left as the panel grows. */
export function Grip({ panel, side }: { panel: PanelId; side: 'left' | 'right' }): JSX.Element {
  // the arrow that makes the panel bigger is the one its edge would move: right for an edge on the right, left otherwise
  const grow = side === 'right' ? 'ArrowRight' : 'ArrowLeft';
  const shrink = side === 'right' ? 'ArrowLeft' : 'ArrowRight';
  void panelWidths.value; // a new width (a drag, a key) says the figure again
  void scaleTick.value; // the limits follow the size of the window and the scale of the interface
  const key = (e: KeyboardEvent): void => {
    if (e.key !== grow && e.key !== shrink && e.key !== 'Home' && e.key !== 'Enter') return;
    e.preventDefault();
    e.stopPropagation();
    if (e.key === 'Home' || e.key === 'Enter') resetWidth(panel);
    else stepWidth(panel, e.key === grow, e.shiftKey ? 4 : 1);
  };
  return (
    <div
      class={`panel-grip ${side}`} role="separator" aria-orientation="vertical" tabIndex={0}
      aria-label={t('grip.label', { panel: t(NAME[panel]) })} title={t('grip.tip')}
      aria-valuemin={PANELS[panel].min} aria-valuemax={Math.round(maxWidth(panel))} aria-valuenow={Math.round(widthNow(panel))}
      onPointerDown={(e) => dragPanel(panel, e as unknown as PointerEvent)} onDblClick={() => resetWidth(panel)} onKeyDown={key}
    />
  );
}
