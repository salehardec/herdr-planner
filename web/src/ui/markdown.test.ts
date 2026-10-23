import { describe, expect, it } from 'vitest';
import { parseMarkdown } from './markdown-parse';

describe('parseMarkdown', () => {
  it('a nested list keeps its levels', () => {
    const b = parseMarkdown('- один\n  - полтора\n- два');
    expect(b).toHaveLength(1);
    expect(b[0].kind).toBe('list');
    if (b[0].kind !== 'list') return;
    expect(b[0].list.kind).toBe('ul');
    expect(b[0].list.items.map((i) => i.text)).toEqual(['один', 'два']);
    expect(b[0].list.items[0].children[0].items[0].text).toBe('полтора');
  });
  it('a numbered list is ordered', () => {
    const b = parseMarkdown('1. a\n2. b');
    expect(b[0].kind === 'list' && b[0].list.kind).toBe('ol');
  });
  it('a fence keeps its text and its language', () => {
    const b = parseMarkdown('```sh\nls -la\n  echo **x**\n```');
    expect(b).toEqual([{ kind: 'code', text: 'ls -la\n  echo **x**', lang: 'sh' }]);
    expect(parseMarkdown('```\nx\n```')[0]).toEqual({ kind: 'code', text: 'x', lang: '' });
  });
  it('a table with alignment', () => {
    const b = parseMarkdown('| a | b |\n|:--|--:|\n| 1 | 2 |');
    expect(b[0]).toEqual({ kind: 'table', head: ['a', 'b'], rows: [['1', '2']], align: ['left', 'right'] });
  });
  it('a paragraph after a list is a paragraph', () => {
    const b = parseMarkdown('- a\n\nпотом текст');
    expect(b.map((x) => x.kind)).toEqual(['list', 'p']);
    expect(b[1].kind === 'p' && b[1].text).toBe('потом текст');
  });
  it('headings, quotes and rules', () => {
    const b = parseMarkdown('## Заголовок\n> цитата\n---');
    expect(b.map((x) => x.kind)).toEqual(['h', 'quote', 'hr']);
    expect(b[0].kind === 'h' && b[0].level).toBe(2);
  });
});

/* The inline tokens: the same regex MdInline uses, read back through the parts a text is cut into. */
describe('inline tokens', () => {
  const INLINE = /(`[^`\n]+`|\*\*[^*\n]+\*\*|\*[^*\n]+\*|\[[^\]\n]+\]\(https?:\/\/[^)\s]+\)|https?:\/\/[^\s<>()]+)/g;
  const tokens = (s: string): string[] => [...s.matchAll(INLINE)].map((m) => m[1]);
  it('** inside code is code, not bold', () => {
    expect(tokens('see `a **b** c` here')).toEqual(['`a **b** c`']);
  });
  it('a task key inside code stays code', () => {
    expect(tokens('key `PLN-5` is code')).toEqual(['`PLN-5`']);
  });
  it('a bare address is a link token', () => {
    expect(tokens('see https://example.com/x?y=1 now')).toEqual(['https://example.com/x?y=1']);
  });
});
