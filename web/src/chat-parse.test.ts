import { beforeEach, describe, expect, it } from 'vitest';
import { eventText, noticeStatus, parseEvent, parseNotice, splitAttachments, splitEvents, splitRefs } from './chat-parse';
import { lang } from './i18n';

describe('board events at the end of an answer', () => {
  beforeEach(() => { lang.value = 'ru'; });
  const lines: [string, string[], string][] = [
    ['Added APC-2', ['APC-2'], 'Добавлена APC-2'],
    ['Approved APC-2', ['APC-2'], 'Одобрена APC-2'],
    ['Approved APC-11 and pushed it to GitLab', ['APC-11'], 'Одобрена APC-11 и отправлена в GitLab'],
    ['Started an agent on APC-2', ['APC-2'], 'Агент запущен на APC-2'],
    ['Added PLN-11 and started an agent on it', ['PLN-11'], 'Добавлена PLN-11, агент запущен'],
    ['Added APC-5 and started an agent on it (worker.agent omp/deepseek/deepseek-v4-flash)', ['APC-5'], 'Добавлена APC-5, агент запущен (worker.agent omp/deepseek/deepseek-v4-flash)'],
    ['Updated PLN-7 (description)', ['PLN-7'], 'Изменена PLN-7 (описание)'],
    ['Updated PLN-10 (sent the agent your feedback)', ['PLN-10'], 'Изменена PLN-10 (sent the agent your feedback)'],
    ['Updated APC-2 (settings: merge.by itself)', ['APC-2'], 'Изменена APC-2 (settings: merge.by itself)'],
    ['Added PLN-28 (queued after PLN-25)', ['PLN-28'], 'Добавлена PLN-28 (в очереди после PLN-25)'],
    ['Removed BOT-1, OMRC-1, …', ['BOT-1', 'OMRC-1'], 'Убраны BOT-1, OMRC-1'],
    ['Sent PLN-23 the CI failure to fix', ['PLN-23'], 'Агенту PLN-23 передано падение CI'],
    ['Updated worker.instructions and reviewer.instructions (the yard rule)', [], 'Изменено worker.instructions and reviewer.instructions (the yard rule)'],
  ];
  for (const [raw, keys, ru] of lines) {
    it(`reads «${raw}»`, () => {
      const ev = parseEvent(raw);
      expect(ev, raw).not.toBeNull();
      expect(ev!.keys).toEqual(keys);
      expect(eventText(ev!)).toBe(ru);
    });
  }
  it('says the same in English', () => {
    lang.value = 'en';
    expect(eventText(parseEvent('Added PLN-11 and started an agent on it')!)).toBe('Added PLN-11 and started an agent on it');
    expect(eventText(parseEvent('Updated PLN-7 (description)')!)).toBe('Updated PLN-7 (description)');
    expect(eventText(parseEvent('Removed BOT-1, OMRC-1')!)).toBe('Removed BOT-1, OMRC-1');
  });
  it('the forms Claude Code uses most: parentheses after the keys, «;», a plan', () => {
    expect(eventText(parseEvent('Added PLN-12 (queued after PLN-9) and started an agent on it')!)).toBe('Добавлена PLN-12, агент запущен (в очереди после PLN-9)');
    expect(eventText(parseEvent('Updated APC-3 (description) and started an agent on it')!)).toBe('Изменена APC-3, агент запущен (описание)');
    expect(eventText(parseEvent("Approved PLN-7's plan")!)).toBe('Одобрен план PLN-7');
    const r = splitEvents('Approved SRV-3; updated ANS-2 (description)');
    expect(r.events.map((e) => eventText(e))).toEqual(['Одобрена SRV-3', 'Изменена ANS-2 (описание)']);
    const two = splitEvents('Added ANS-4 and started an agent on it; stopped SRV-5, added SRV-6 and started an agent on it');
    expect(two.events.map((e) => e.verb)).toEqual(['added', 'stopped']);
  });
  it('an unknown wording stays as it was, still as an event', () => {
    const ev = parseEvent('Approved APC-3 and told the reviewer');
    expect(ev?.keys).toEqual(['APC-3']);
    expect(eventText(ev!)).toBe('Approved APC-3 and told the reviewer');
  });
  it('a Russian last paragraph is not an event', () => {
    expect(parseEvent('Added задача PLN-1')).toBeNull();
    const r = splitEvents('Сделал.\n\nДобавил PLN-5 и запустил агента.');
    expect(r.events).toEqual([]);
    expect(r.body).toBe('Сделал.\n\nДобавил PLN-5 и запустил агента.');
  });
  it('a paragraph that is only partly an event stays text', () => {
    const r = splitEvents('Готово.\n\nAdded PLN-5. The rest is up to you.');
    expect(r.events).toEqual([]);
    expect(r.body).toContain('Added PLN-5');
    expect(parseEvent('Added')).toBeNull();
    expect(parseEvent('Sent you the log')).toBeNull();
  });
  it('splits the last paragraph with several sentences', () => {
    const r = splitEvents('Сделал оба.\n\nUpdated PLN-9 (description). Updated PLN-10 (sent the agent your feedback)');
    expect(r.body).toBe('Сделал оба.');
    expect(r.events.map((e) => e.keys[0])).toEqual(['PLN-9', 'PLN-10']);
    const two = splitEvents('Updated APC-2 (settings: merge.by itself). Added PLN-1 and started an agent on it');
    expect(two.body).toBe('');
    expect(two.events.map((e) => eventText(e))).toEqual(['Изменена APC-2 (settings: merge.by itself)', 'Добавлена PLN-1, агент запущен']);
  });
  it('an answer that is one event is all event', () => {
    const r = splitEvents('Approved APC-2');
    expect(r.body).toBe('');
    expect(r.events).toHaveLength(1);
  });
  it('an answer without events is untouched', () => {
    const a = 'Ответ 9: ALP-9 идёт по плану.';
    expect(splitEvents(a)).toEqual({ body: a, events: [] });
  });
});

describe('board notices', () => {
  const states: [string, string][] = [['ждёт тебя (needs you)', 'waiting'], ['готова к одобрению (ready to approve)', 'ready'], ['упала (failed)', 'failed'], ['план готов (plan ready)', 'plan']];
  for (const [st, status] of states) {
    it(`reads «${st}»`, () => {
      const n = parseNotice(`[Уведомление доски] apicatalog/2 «Живой счётчик запросов»: ${st}. Посмотри board details apicatalog/2 и коротко доложи итог.`);
      expect(n).not.toBeNull();
      expect(n!.id).toBe('apicatalog/2');
      expect(n!.title).toBe('Живой счётчик запросов');
      expect(n!.stateRu).toBe(st.replace(/\s*\(.*$/, ''));
      expect(n!.stateEn).toBe(/\((.*)\)/.exec(st)![1]);
      expect(n!.note).toBe('');
      expect(noticeStatus(n!)).toBe(status);
    });
  }
  it('keeps other words of the notice, drops the instruction', () => {
    const n = parseNotice('[Уведомление доски] alpha/7 «Кэш»: ждёт тебя (needs you). Агент спрашивает про формат. Посмотри board details alpha/7 и доложи.');
    expect(n!.note).toBe('Агент спрашивает про формат.');
  });
  it('a title with quotes of its own runs to the state', () => {
    const n = parseNotice('[Уведомление доски] alpha/3 «Мастер «Подключение системы»: проще и понятнее»: ждёт тебя (needs you). Посмотри board details alpha/3 и коротко доложи итог.');
    expect(n!.title).toBe('Мастер «Подключение системы»: проще и понятнее');
    expect(n!.stateRu).toBe('ждёт тебя');
    expect(n!.stateEn).toBe('needs you');
    expect(n!.note).toBe('');
  });
  it('a short notice without a title', () => {
    const n = parseNotice('[Уведомление доски] apicache/2 ждёт тебя');
    expect(n).toEqual({ id: 'apicache/2', title: '', stateRu: 'ждёт тебя', stateEn: '', note: '' });
  });
  it('a human message is not a notice', () => {
    expect(parseNotice('Как дела по PLN-5?')).toBeNull();
  });
});

describe('attachments of a human message', () => {
  const A = '0123456789abcdef0123456789abcdef.png';
  const B = 'fedcba9876543210fedcba9876543210.jpg';
  it('one image', () => {
    const r = splitAttachments(`вот скриншот\n\nКартинки (файлы на машине с доской):\n/home/me/.local/state/herdr-planner/attachments/${A}`);
    expect(r.text).toBe('вот скриншот');
    expect(r.files).toEqual([{ id: A, name: A }]);
    expect(r.pasted).toBe(0);
  });
  it('several files', () => {
    const r = splitAttachments(`смотри\n\nВложения (файлы на машине с доской):\n/x/${A}\n/x/${B}\n/x/0123456789abcdef0123456789abcdef.txt`);
    expect(r.files.map((f) => f.id)).toEqual([A, B, '0123456789abcdef0123456789abcdef.txt']);
  });
  it('the header alone (Claude Code kept the files as image blocks) is taken off the text', () => {
    expect(splitAttachments('вот снимок\nКартинки (файлы на машине с доской):')).toEqual({ text: 'вот снимок', files: [], pasted: 0 });
    expect(splitAttachments('смотри\n\nВложения (файлы на машине с доской):  ')).toEqual({ text: 'смотри', files: [], pasted: 0 });
  });
  it('a path that is not a saved file keeps the text as it is', () => {
    const q = 'смотри\n\nКартинки (файлы на машине с доской):\n/etc/passwd';
    expect(splitAttachments(q)).toEqual({ text: q, files: [], pasted: 0 });
  });
  it('images pasted into the terminal are counted', () => {
    expect(splitAttachments('[Image #2] переделай')).toEqual({ text: 'переделай', files: [], pasted: 1 });
    expect(splitAttachments('[Image: source: /tmp/x.png] и [Image #3] вот')).toEqual({ text: 'и вот', files: [], pasted: 2 });
  });
  it('text without files is unchanged', () => {
    expect(splitAttachments('просто слова')).toEqual({ text: 'просто слова', files: [], pasted: 0 });
  });
});

describe('task references', () => {
  it('keys become references, repo/n only when known', () => {
    expect(splitRefs('PLN-9 и src/1 и alpha/7', (id) => id === 'alpha/7')).toEqual([{ ref: 'PLN-9' }, ' и src/1 и ', { ref: 'alpha/7' }]);
    expect(splitRefs('src/1')).toEqual(['src/1']);
    expect(splitRefs('')).toEqual([]);
  });
});
