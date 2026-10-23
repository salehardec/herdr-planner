/* The agent's closing note is Markdown. A small renderer for the blocks it actually uses (headings, lists
   with nesting, tables, quotes, fences, rules, paragraphs) and for inline code, emphasis and http(s) links.
   Everything becomes Preact nodes, so the text of an agent is never read as markup. */
import type { JSX } from 'preact';
import { t } from '../i18n';
import { toast } from '../toast';
import { RefText } from './TaskChip';
import { parseMarkdown, type ListNode } from './markdown-parse';

export { parseMarkdown, type MdBlock } from './markdown-parse';

const INLINE = /(`[^`\n]+`|\*\*[^*\n]+\*\*|\*[^*\n]+\*|\[[^\]\n]+\]\(https?:\/\/[^)\s]+\)|https?:\/\/[^\s<>()]+)/g;

/** A bare address ends before the punctuation that closes the sentence around it. */
function trimUrl(url: string): [string, string] {
  const m = /[.,;:!?»'"]+$/.exec(url);
  return m ? [url.slice(0, m.index), url.slice(m.index)] : [url, ''];
}

/** Inline code, **bold**, *italic*, http(s) links; all other text stays text (with task chips when `chips`). */
export function MdInline(props: { text: string; chips?: boolean }): JSX.Element {
  const text = props.text;
  const parts: (string | JSX.Element)[] = [];
  const plain = (s: string, k: number): void => { parts.push(props.chips ? <RefText key={`t${k}`} text={s} /> : s); };
  let last = 0;
  for (const m of text.matchAll(INLINE)) {
    const at = m.index ?? 0;
    if (at > last) plain(text.slice(last, at), at);
    const tok = m[1];
    if (tok.startsWith('http')) {
      const [url, tail] = trimUrl(tok);
      parts.push(<a key={at} href={url} target="_blank" rel="noopener noreferrer">{url}</a>);
      if (tail) plain(tail, at + url.length);
    } else if (tok.startsWith('`')) {
      parts.push(<code key={at}>{tok.slice(1, -1)}</code>);
    } else if (tok.startsWith('**')) {
      parts.push(<strong key={at}><MdInline text={tok.slice(2, -2)} chips={props.chips} /></strong>);
    } else if (tok.startsWith('*')) {
      parts.push(<em key={at}><MdInline text={tok.slice(1, -1)} chips={props.chips} /></em>);
    } else {
      const link = /^\[([^\]]+)\]\(([^)]+)\)$/.exec(tok);
      parts.push(link
        ? <a key={at} href={link[2]} target="_blank" rel="noopener noreferrer">{link[1]}</a>
        : tok);
    }
    last = at + tok.length;
  }
  if (last < text.length) plain(text.slice(last), last);
  return <>{parts}</>;
}

function ListView(props: { list: ListNode; chips?: boolean }): JSX.Element {
  const Tag = props.list.kind;
  return (
    <Tag>
      {props.list.items.map((it, j) => (
        <li key={j}>
          <MdInline text={it.text} chips={props.chips} />
          {it.children.map((child, k) => <ListView key={k} list={child} chips={props.chips} />)}
        </li>
      ))}
    </Tag>
  );
}

/** A fenced block with a button that copies it. */
function CodeBlock(props: { text: string; lang: string }): JSX.Element {
  return (
    <div class="pre" data-lang={props.lang || undefined}>
      <button
        class="copy" type="button" data-act="copy-code" title={t('chat.code.copy')} aria-label={t('chat.code.copy')}
        onClick={() => { navigator.clipboard?.writeText(props.text).catch(() => {}); toast(t('toast.copied')); }}
      >{t('chat.code.copy')}</button>
      <pre><code>{props.text}</code></pre>
    </div>
  );
}

/**
 * The blocks of a Markdown text as nodes. `chips`: task keys in the text are chips; `copy`: a fenced block has a
 * button that copies it (and a table scrolls sideways in its own box). Without them the markup is the plain one
 * the agent's note uses.
 */
export function Markdown(props: { text: string; class?: string; chips?: boolean; copy?: boolean }): JSX.Element {
  const blocks = parseMarkdown(props.text);
  const chips = props.chips;
  return (
    <div class={`md ${props.class ?? ''}`}>
      {blocks.map((b, i) => {
        switch (b.kind) {
          case 'h': {
            const Tag = b.level === 1 ? 'h3' : b.level === 2 ? 'h4' : 'h5';
            return <Tag key={i}><MdInline text={b.text} chips={chips} /></Tag>;
          }
          case 'list':
            return <ListView key={i} list={b.list} chips={chips} />;
          case 'code':
            return props.copy ? <CodeBlock key={i} text={b.text} lang={b.lang} /> : <pre key={i}><code>{b.text}</code></pre>;
          case 'quote':
            return <blockquote key={i}>{b.lines.map((l, j) => <p key={j}><MdInline text={l} chips={chips} /></p>)}</blockquote>;
          case 'table': {
            const table = (
              <table key={i}>
                <thead>
                  <tr>
                    {b.head.map((c, j) => <th key={j} style={b.align[j] ? { textAlign: b.align[j] } : undefined}><MdInline text={c} chips={chips} /></th>)}
                  </tr>
                </thead>
                <tbody>
                  {b.rows.map((row, r) => (
                    <tr key={r}>
                      {b.head.map((_, c) => <td key={c} style={b.align[c] ? { textAlign: b.align[c] } : undefined}><MdInline text={row[c] ?? ''} chips={chips} /></td>)}
                    </tr>
                  ))}
                </tbody>
              </table>
            );
            return props.copy ? <div key={i} class="tbl">{table}</div> : table;
          }
          case 'hr':
            return <hr key={i} />;
          default:
            return <p key={i}><MdInline text={b.text} chips={chips} /></p>;
        }
      })}
    </div>
  );
}
