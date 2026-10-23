/* The blocks of a Markdown text: headings, lists with nesting, tables, quotes, fences, rules, paragraphs.
   Pure (no DOM): Markdown.tsx draws the blocks, the tests read them. */
type Heading = 1 | 2 | 3 | 4 | 5 | 6;
type Align = '' | 'left' | 'center' | 'right';

export interface MdItem { text: string; children: ListNode[] }
export interface ListNode { kind: 'ul' | 'ol'; items: MdItem[] }

export type MdBlock =
  | { kind: 'h'; level: Heading; text: string }
  | { kind: 'p'; text: string }
  | { kind: 'list'; list: ListNode }
  | { kind: 'code'; text: string; lang: string }
  | { kind: 'quote'; lines: string[] }
  | { kind: 'table'; head: string[]; rows: string[][]; align: Align[] }
  | { kind: 'hr' };

const HEADING = /^\s{0,3}(#{1,6})\s+(.*?)\s*#*\s*$/;
const ITEM = /^(\s*)([-*+]|\d+[.)])\s+(.*)$/;
const QUOTE = /^\s{0,3}>\s?(.*)$/;
const FENCE = /^\s{0,3}(`{3,}|~{3,})\s*([\w+#.-]*)/;
const HR = /^\s{0,3}([-*_])(?:\s*\1){2,}\s*$/;
const TABLE_SEP = /^\s*\|?\s*:?-+:?\s*(\|\s*:?-+:?\s*)+\|?\s*$/;

const indentOf = (space: string): number => space.replace(/\t/g, '  ').length;

const cells = (line: string): string[] => {
  let text = line.trim();
  if (text.startsWith('|')) text = text.slice(1);
  if (text.endsWith('|')) text = text.slice(0, -1);
  return text.split('|').map((c) => c.trim());
};

const isTableStart = (lines: string[], i: number): boolean =>
  i + 1 < lines.length && lines[i].includes('|') && TABLE_SEP.test(lines[i + 1])
  && cells(lines[i]).length === cells(lines[i + 1]).length;

const alignOf = (sep: string): Align[] => cells(sep).map((c) => {
  const left = c.startsWith(':');
  const right = c.endsWith(':');
  return left && right ? 'center' : right ? 'right' : left ? 'left' : '';
});

const startsBlockAt = (lines: string[], i: number): boolean => {
  const line = lines[i];
  return HEADING.test(line) || ITEM.test(line) || QUOTE.test(line) || FENCE.test(line) || HR.test(line) || isTableStart(lines, i);
};

/** A list at one indent and its sub-lists, which keep their own indent. */
function parseList(lines: string[], start: number, indent: number, ordered: boolean): [ListNode, number] {
  const items: MdItem[] = [];
  let i = start;
  while (i < lines.length) {
    const m = ITEM.exec(lines[i]);
    if (!m) break;
    const here = indentOf(m[1]);
    const isOrdered = /^\d/.test(m[2]);
    if (here < indent) break;
    if (here > indent) {
      if (!items.length) break;
      const [child, next] = parseList(lines, i, here, isOrdered);
      items[items.length - 1].children.push(child);
      i = next;
      continue;
    }
    if (isOrdered !== ordered) break;
    const item: MdItem = { text: m[3].trim(), children: [] };
    items.push(item);
    i++;
    while (i < lines.length) {
      const nm = ITEM.exec(lines[i]);
      if (nm && indentOf(nm[1]) > indent) {
        const [child, next] = parseList(lines, i, indentOf(nm[1]), /^\d/.test(nm[2]));
        item.children.push(child);
        i = next;
        continue;
      }
      if (!lines[i].trim() || nm) break;
      const lead = /^\s*/.exec(lines[i])?.[0] ?? '';
      if (indentOf(lead) > indent && !startsBlockAt(lines, i)) { item.text += ' ' + lines[i].trim(); i++; continue; }
      break;
    }
  }
  return [{ kind: ordered ? 'ol' : 'ul', items }, i];
}

export function parseMarkdown(text: string): MdBlock[] {
  const lines = (text ?? '').replace(/\r\n?/g, '\n').split('\n');
  const out: MdBlock[] = [];
  let i = 0;
  while (i < lines.length) {
    const line = lines[i];
    if (!line.trim()) { i++; continue; }
    const fence = FENCE.exec(line);
    if (fence) {
      const mark = fence[1][0];
      const close = new RegExp('^\\s{0,3}' + (mark === '`' ? '`' : '~') + '{3,}\\s*$');
      const body: string[] = [];
      i++;
      while (i < lines.length && !close.test(lines[i])) { body.push(lines[i]); i++; }
      if (i < lines.length) i++;
      out.push({ kind: 'code', text: body.join('\n'), lang: fence[2] ?? '' });
      continue;
    }
    const h = HEADING.exec(line);
    if (h) {
      out.push({ kind: 'h', level: Math.min(6, h[1].length) as Heading, text: h[2] });
      i++;
      continue;
    }
    if (HR.test(line)) { out.push({ kind: 'hr' }); i++; continue; }
    if (isTableStart(lines, i)) {
      const head = cells(line);
      const align = alignOf(lines[i + 1]);
      i += 2;
      const rows: string[][] = [];
      while (i < lines.length && lines[i].trim() && lines[i].includes('|') && !FENCE.test(lines[i])) {
        rows.push(cells(lines[i]));
        i++;
      }
      out.push({ kind: 'table', head, rows, align });
      continue;
    }
    const item = ITEM.exec(line);
    if (item) {
      const [list, next] = parseList(lines, i, indentOf(item[1]), /^\d/.test(item[2]));
      out.push({ kind: 'list', list });
      i = next;
      continue;
    }
    if (QUOTE.test(line)) {
      const quoted: string[] = [];
      while (i < lines.length) {
        const m = QUOTE.exec(lines[i]);
        if (!m) break;
        quoted.push(m[1]);
        i++;
      }
      out.push({ kind: 'quote', lines: quoted });
      continue;
    }
    const para = [line.trim()];
    i++;
    while (i < lines.length && lines[i].trim() && !startsBlockAt(lines, i)) { para.push(lines[i].trim()); i++; }
    out.push({ kind: 'p', text: para.join(' ') });
  }
  return out;
}
