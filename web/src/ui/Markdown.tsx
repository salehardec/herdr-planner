/* The agent's closing note is Markdown. A small renderer for the blocks it actually uses (headings, lists
   with nesting, tables, quotes, fences, rules, paragraphs) and for inline code, emphasis and http(s) links.
   Everything becomes Preact nodes, so the text of an agent is never read as markup. */
import type { JSX } from 'preact';

type Heading = 1 | 2 | 3 | 4 | 5 | 6;
type Align = '' | 'left' | 'center' | 'right';

interface MdItem { text: string; children: ListNode[] }
interface ListNode { kind: 'ul' | 'ol'; items: MdItem[] }

export type MdBlock =
  | { kind: 'h'; level: Heading; text: string }
  | { kind: 'p'; text: string }
  | { kind: 'list'; list: ListNode }
  | { kind: 'code'; text: string }
  | { kind: 'quote'; lines: string[] }
  | { kind: 'table'; head: string[]; rows: string[][]; align: Align[] }
  | { kind: 'hr' };

const HEADING = /^\s{0,3}(#{1,6})\s+(.*?)\s*#*\s*$/;
const ITEM = /^(\s*)([-*+]|\d+[.)])\s+(.*)$/;
const QUOTE = /^\s{0,3}>\s?(.*)$/;
const FENCE = /^\s{0,3}(`{3,}|~{3,})/;
const HR = /^\s{0,3}([-*_])(?:\s*\1){2,}\s*$/;
const TABLE_SEP = /^\s*\|?\s*:?-{3,}:?\s*(\|\s*:?-{3,}:?\s*)+\|?\s*$/;

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
      out.push({ kind: 'code', text: body.join('\n') });
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

const INLINE = /(`[^`\n]+`|\*\*[^*\n]+\*\*|\*[^*\n]+\*|\[[^\]\n]+\]\(https?:\/\/[^)\s]+\))/g;

/** Inline code, **bold**, *italic* and http(s) links; all other text stays text. */
export function MdInline(props: { text: string }): JSX.Element {
  const text = props.text;
  const parts: (string | JSX.Element)[] = [];
  let last = 0;
  for (const m of text.matchAll(INLINE)) {
    const at = m.index ?? 0;
    if (at > last) parts.push(text.slice(last, at));
    const tok = m[1];
    if (tok.startsWith('`')) {
      parts.push(<code key={at}>{tok.slice(1, -1)}</code>);
    } else if (tok.startsWith('**')) {
      parts.push(<strong key={at}>{tok.slice(2, -2)}</strong>);
    } else if (tok.startsWith('*')) {
      parts.push(<em key={at}>{tok.slice(1, -1)}</em>);
    } else {
      const link = /^\[([^\]]+)\]\(([^)]+)\)$/.exec(tok);
      parts.push(link
        ? <a key={at} href={link[2]} target="_blank" rel="noopener noreferrer">{link[1]}</a>
        : tok);
    }
    last = at + tok.length;
  }
  if (last < text.length) parts.push(text.slice(last));
  return <>{parts}</>;
}

function ListView(props: { list: ListNode }): JSX.Element {
  const Tag = props.list.kind;
  return (
    <Tag>
      {props.list.items.map((it, j) => (
        <li key={j}>
          <MdInline text={it.text} />
          {it.children.map((child, k) => <ListView key={k} list={child} />)}
        </li>
      ))}
    </Tag>
  );
}

export function Markdown(props: { text: string }): JSX.Element {
  const blocks = parseMarkdown(props.text);
  return (
    <div class="md">
      {blocks.map((b, i) => {
        switch (b.kind) {
          case 'h': {
            const Tag = b.level === 1 ? 'h3' : b.level === 2 ? 'h4' : 'h5';
            return <Tag key={i}><MdInline text={b.text} /></Tag>;
          }
          case 'list':
            return <ListView key={i} list={b.list} />;
          case 'code':
            return <pre key={i}><code>{b.text}</code></pre>;
          case 'quote':
            return <blockquote key={i}>{b.lines.map((l, j) => <p key={j}><MdInline text={l} /></p>)}</blockquote>;
          case 'table':
            return (
              <table key={i}>
                <thead>
                  <tr>
                    {b.head.map((c, j) => <th key={j} style={b.align[j] ? { textAlign: b.align[j] } : undefined}><MdInline text={c} /></th>)}
                  </tr>
                </thead>
                <tbody>
                  {b.rows.map((row, r) => (
                    <tr key={r}>
                      {b.head.map((_, c) => <td key={c} style={b.align[c] ? { textAlign: b.align[c] } : undefined}><MdInline text={row[c] ?? ''} /></td>)}
                    </tr>
                  ))}
                </tbody>
              </table>
            );
          case 'hr':
            return <hr key={i} />;
          default:
            return <p key={i}><MdInline text={b.text} /></p>;
        }
      })}
    </div>
  );
}
