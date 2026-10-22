import { describe, expect, it } from 'vitest';
import { ansiSpans } from './ansi';

const text = (src: string): string => ansiSpans(src).map((s) => s.text).join('');

describe('terminal colours', () => {
  it('keeps the words and their colours', () => {
    const spans = ansiSpans('\u001b[1;36m❯\u001b[0m May I continue?\n  \u001b[33m1.\u001b[0m Yes');
    expect(spans.map((s) => s.text)).toEqual(['❯', ' May I continue?\n  ', '1.', ' Yes']);
    expect(spans[0].bold).toBe(true);
    expect(spans[0].fg).toBe('var(--c-cyan)');
    expect(spans[2].fg).toBe('var(--c-yellow)');
    expect(spans[3].fg).toBeUndefined();
  });

  it('drops cursor moves, a title and a sequence cut by the limit', () => {
    expect(text('a\u001b[2Kb\u001b]0;title\u0007c')).toBe('abc');
    expect(text('line\u001b[38;5;')).toBe('line');
    expect(text('\r\none\r\ntwo\rthree')).toBe('\none\ntwo\nthree');
  });

  it('reads the 256-colour palette and true colour', () => {
    expect(ansiSpans('\u001b[38;5;196mred\u001b[0m')[0].fg).toBe('#ff0000');
    expect(ansiSpans('\u001b[38;5;244mgrey\u001b[0m')[0].fg).toBe('#808080');
    expect(ansiSpans('\u001b[38;2;18;52;86mblue\u001b[0m')[0].fg).toBe('#123456');
    expect(ansiSpans('\u001b[38;5;999mbad\u001b[0m')[0].fg).toBeUndefined();
  });

  it('turns emphasis off again', () => {
    const spans = ansiSpans('\u001b[1;4;31mon\u001b[22;24;39moff');
    expect(spans[0]).toMatchObject({ text: 'on', bold: true, underline: true, fg: 'var(--c-red)' });
    expect(spans[1]).toMatchObject({ text: 'off' });
    expect(spans[1].bold).toBeUndefined();
    expect(spans[1].underline).toBeUndefined();
  });

  it('never makes markup of the screen', () => {
    expect(text('<b>not bold</b>')).toBe('<b>not bold</b>');
    expect(ansiSpans('').length).toBe(0);
  });
});
