/* Finds the words the page shows that did not come from the dictionary: text between JSX tags, text in the
   attributes a person reads (title, placeholder, aria-label, alt), and strings handed to toast(). Used by the
   i18n test; the dictionary (ru.ts, en.ts) is the only place for words. */
import ts from 'typescript';

export interface Found { file: string; line: number; text: string; where: string }

/** Attributes whose value a person reads or hears. */
const READ_ATTRS = new Set(['title', 'placeholder', 'aria-label', 'alt', 'aria-description', 'label']);

/** Words that are the same in every language: key names, program names, units, symbols. */
const FIXED = new Set([
  'Ctrl', 'Cmd', 'Alt', 'Shift', 'Esc', 'Enter', 'Tab', 'Space', 'Home', 'Escape', 'Delete',
  'claude', 'codex', 'pi', 'omp', 'herdr', 'herdr-projects', 'herdr-planner', 'Homebrew', 'pacman', 'apt', 'yazi', 'git', 'GitHub', 'GitLab',
  'WASD', 'MB', 'KB', 'B', 'ssh', 'SSH', 'OK',
  // names of git branches and remotes
  'main', 'master', 'origin', 'upstream',
]);

/** Elements whose text is code or a key, the same in every language. */
const CODE_TAGS = new Set(['code', 'kbd', 'pre', 'samp', 'var']);

/** Has a word in it (two or more letters in a row) and is not just a key name or a unit. */
function isWords(s: string): boolean {
  const text = s.replace(/\s+/g, ' ').trim();
  if (!/\p{L}{2,}/u.test(text)) return false;
  if (FIXED.has(text)) return false;
  // an example path or address, a currency or unit code (`~/Projects/…`, `https://…`, `USD`)
  if (/^(~\/|https?:\/\/)/.test(text) || /^[A-Z]{2,5}$/.test(text)) return false;
  // one token that is an address, a path or a flag (`user@host`, `--label`)
  if (!/\s/.test(text) && /^[\w@./:~=-]+$/.test(text) && /[@./:~=_-]/.test(text)) return false;
  // a list of fixed words (`Ctrl + K`, `W A S D`)
  if (text.split(/[\s+/,·|-]+/).every((w) => !w || FIXED.has(w) || !/\p{L}{2,}/u.test(w))) return false;
  return true;
}

/** The string literals a JSX expression can show: its branches and the operands of && || ?? (not call arguments or tests). */
function literalsIn(e: ts.Expression): (ts.StringLiteral | ts.NoSubstitutionTemplateLiteral)[] {
  if (ts.isStringLiteral(e) || ts.isNoSubstitutionTemplateLiteral(e)) return [e];
  if (ts.isParenthesizedExpression(e)) return literalsIn(e.expression);
  if (ts.isConditionalExpression(e)) return [...literalsIn(e.whenTrue), ...literalsIn(e.whenFalse)];
  if (ts.isBinaryExpression(e) && [ts.SyntaxKind.AmpersandAmpersandToken, ts.SyntaxKind.BarBarToken, ts.SyntaxKind.QuestionQuestionToken].includes(e.operatorToken.kind)) {
    return [...literalsIn(e.left), ...literalsIn(e.right)];
  }
  return [];
}

export function scan(file: string, source: string): Found[] {
  const sf = ts.createSourceFile(file, source, ts.ScriptTarget.Latest, true, file.endsWith('x') ? ts.ScriptKind.TSX : ts.ScriptKind.TS);
  const out: Found[] = [];
  const line = (n: ts.Node): number => sf.getLineAndCharacterOfPosition(n.getStart(sf)).line + 1;
  const add = (n: ts.Node, text: string, where: string): void => { out.push({ file, line: line(n), text: text.replace(/\s+/g, ' ').trim(), where }); };
  const inCode = (n: ts.Node): boolean => {
    for (let p: ts.Node | undefined = n.parent; p; p = p.parent) {
      if (ts.isJsxElement(p) && CODE_TAGS.has(p.openingElement.tagName.getText(sf))) return true;
    }
    return false;
  };
  const visit = (n: ts.Node): void => {
    if (ts.isJsxText(n) && isWords(n.text) && !inCode(n)) add(n, n.text, 'text');
    if (ts.isJsxAttribute(n) && READ_ATTRS.has(n.name.getText(sf)) && n.initializer && ts.isStringLiteral(n.initializer) && isWords(n.initializer.text)) {
      add(n, n.initializer.text, n.name.getText(sf));
    }
    // a word in a branch or after a logical operator: {ok ? 'Saved' : 'Failed'}, {ok && 'Saved'}, title={ok ? 'a b' : 'c d'}
    if (ts.isJsxExpression(n) && n.expression && (!ts.isJsxAttribute(n.parent) || READ_ATTRS.has(n.parent.name.getText(sf)))) {
      for (const lit of literalsIn(n.expression)) if (isWords(lit.text)) add(lit, lit.text, ts.isJsxAttribute(n.parent) ? n.parent.name.getText(sf) : 'text');
    }
    // el.textContent = 'Loading data' (and innerText, placeholder, title)
    if (ts.isBinaryExpression(n) && n.operatorToken.kind === ts.SyntaxKind.EqualsToken && ts.isPropertyAccessExpression(n.left)
      && ['textContent', 'innerText', 'placeholder', 'title'].includes(n.left.name.text)
      && (ts.isStringLiteral(n.right) || ts.isNoSubstitutionTemplateLiteral(n.right)) && isWords(n.right.text)) {
      add(n.right, n.right.text, n.left.name.text);
    }
    // { label: 'Open the file' } and { title: …, placeholder: … } handed to a view
    if (ts.isPropertyAssignment(n) && ts.isIdentifier(n.name) && ['label', 'title', 'placeholder', 'tooltip', 'hint'].includes(n.name.text)
      && (ts.isStringLiteral(n.initializer) || ts.isNoSubstitutionTemplateLiteral(n.initializer)) && isWords(n.initializer.text)) {
      add(n.initializer, n.initializer.text, n.name.text);
    }
    if (ts.isCallExpression(n) && ts.isIdentifier(n.expression) && n.expression.text === 'toast') {
      const a = n.arguments[0];
      if (a && (ts.isStringLiteral(a) || ts.isNoSubstitutionTemplateLiteral(a)) && isWords(a.text)) add(a, a.text, 'toast');
      if (a && ts.isTemplateExpression(a) && isWords(a.head.text)) add(a, a.head.text, 'toast');
    }
    ts.forEachChild(n, visit);
  };
  visit(sf);
  return out;
}
