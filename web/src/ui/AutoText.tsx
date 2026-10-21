import type { JSX } from 'preact';
import { useLayoutEffect, useRef } from 'preact/hooks';

/** The text area grows with its text up to the height its style sheet allows, then scrolls inside. */
export function fitText(el: HTMLTextAreaElement | null): void {
  if (!el) return;
  el.style.height = 'auto';
  const style = getComputedStyle(el);
  const max = parseFloat(style.maxHeight);
  // `scrollHeight` leaves the border out, while `box-sizing: border-box` counts it in the height.
  const border = (parseFloat(style.borderTopWidth) || 0) + (parseFloat(style.borderBottomWidth) || 0);
  const h = el.scrollHeight + border;
  el.style.height = `${Number.isFinite(max) ? Math.min(h, max) : h}px`;
}

interface Props extends Omit<JSX.TextareaHTMLAttributes<HTMLTextAreaElement>, 'rows'> {
  /** The element, for callers that read or set the text themselves (the fields are not all controlled). */
  elRef?: { current: HTMLTextAreaElement | null };
  /** Enter (without Shift) does this; Shift+Enter is always a new line. */
  onSend?: () => void;
}

/**
 * One text area for every long text on the page: the todos, the chat, the reply to an agent and the
 * corrections before a todo leaves. Enter sends, Shift+Enter breaks the line, the height follows the text.
 */
export function AutoTextarea(props: Props): JSX.Element {
  const { elRef, onSend, onInput, onKeyDown, value, ...rest } = props;
  const inner = useRef<HTMLTextAreaElement>(null);
  useLayoutEffect(() => { fitText(inner.current); }, [value]);
  return (
    <textarea
      {...rest}
      rows={1}
      value={value}
      ref={(el) => { inner.current = el; if (elRef) elRef.current = el; }}
      onInput={(e) => { fitText(e.currentTarget); onInput?.(e); }}
      onKeyDown={(e) => {
        onKeyDown?.(e);
        if (e.defaultPrevented) return;
        if (onSend && e.key === 'Enter' && !e.shiftKey && !e.isComposing) {
          e.preventDefault();
          onSend();
        }
      }}
    />
  );
}
