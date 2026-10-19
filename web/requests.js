/* Запросы к человеку: один список из снимка доски, details и Координатора. */

const ORDER = { waiting: 0, failed: 1, ready: 2, plan: 3, review: 4, working: 5, queued: 6, done: 7 };
const ATTN = new Set(['waiting', 'failed', 'ready', 'plan']);

export const PERMISSION_ALLOW = 'Да, разрешаю.';
export const PERMISSION_DENY = 'Нет, не делай этого.';

export function firstSentence(text, max = 150) {
  const s = String(text ?? '').replace(/\s+/g, ' ').trim();
  if (!s) return '';
  const m = s.match(/^.{1,}?[.!?…](?=\s|$)/);
  const out = m ? m[0] : s;
  return out.length > max ? out.slice(0, max - 1).trimEnd() + '…' : out;
}

/** Варианты ответа в словах Координатора: нумерованный/буквенный список ПОСЛЕ вопроса.
   Обычные маркированные списки и объяснения — не варианты. */
export function parseOptions(reply) {
  const lines = String(reply ?? '').split('\n');
  let at = -1;
  for (let i = 0; i < lines.length; i++) if (lines[i].includes('?')) at = i;
  if (at < 0) return [];
  const out = [];
  const re = /^\s*(?:(\d+)[).:]|([a-zA-Z])[).:])\s+(.+?)\s*$/;
  for (let i = at + 1; i < lines.length; i++) {
    const line = lines[i];
    if (!line.trim()) {
      if (out.length) break;
      continue;
    }
    const m = line.match(re);
    if (!m) {
      if (out.length) break;
      continue;
    }
    const label = m[1] ? `${m[1]})` : `${m[2].toUpperCase()})`;
    const value = m[3].replace(/\*\*/g, '').replace(/`/g, '').trim();
    if (value.length > 1 && value.length <= 300 && !out.some((o) => o.value === value)) {
      out.push({ label, value });
    }
  }
  return out.length >= 2 ? out.slice(0, 9) : [];
}

/** What a waiting task asks, in the board's own vocabulary. */
export function requestType(t) {
  if (t.stage === 'pick') return 'pick';
  if (t.stage === 'stopped' && (t.state === 'blocked' || t.s === 'waiting')) return 'limit';
  if (t.stage === 'reviewneeds') return 'reviewneeds';
  if (t.s === 'plan') return 'plan';
  if (t.s === 'ready') return 'approval';
  if (t.s === 'failed') return 'failed';
  if (t.kind === 'permission') return 'permission';
  if (t.kind === 'auth') return 'auth';
  return 'question';
}

const TITLES = {
  pick: 'Выбери попытку',
  limit: 'Остановилась на лимите',
  reviewneeds: 'Ревьюеру нужен ты',
  plan: 'План готов',
  approval: 'К одобрению',
  failed: 'Упала: что делать',
  permission: 'Запрос разрешения',
  auth: 'Нужен вход',
  question: 'Вопрос агента',
  'coordinator-dialog': 'Координатор спрашивает',
  'coordinator-options': 'Вопрос Координатора',
};

export function collectRequests({ tasks, coordinator }) {
  const out = [];
  for (const t of tasks) {
    if (!ATTN.has(t.s)) continue;
    const type = requestType(t);
    out.push({
      key: `${type}:${t.id}`,
      type,
      title: TITLES[type] || 'Запрос',
      id: t.id,
      tag: t.tag,
      repo: t.repo,
      msg: t.msg || t.board_label || '',
      text: firstSentence(t.msg || t.board_label || ''),
      task: t,
      urgency: ORDER[t.s] ?? 9,
    });
  }
  if (coordinator?.dialog) {
    out.push({
      key: 'coordinator-dialog',
      type: 'coordinator-dialog',
      title: TITLES['coordinator-dialog'],
      id: 'coordinator',
      tag: 'Координатор',
      repo: 'coordinator',
      msg: coordinator.reply || '',
      text: firstSentence(coordinator.reply || ''),
      urgency: -1,
    });
  } else {
    const options = parseOptions(coordinator?.reply);
    if (options.length) {
      out.push({
        key: 'coordinator-options',
        type: 'coordinator-options',
        title: TITLES['coordinator-options'],
        id: 'coordinator',
        tag: 'Координатор',
        repo: 'coordinator',
        msg: coordinator.reply || '',
        text: firstSentence(coordinator.reply || ''),
        options,
        urgency: 0.5,
      });
    }
  }
  return out.sort((a, b) => a.urgency - b.urgency || (a.task?.n || 0) - (b.task?.n || 0));
}
