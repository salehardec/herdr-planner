'use strict';
// The sign-in page of the gate. It holds no secret of the planner. The key of the link comes in the part after the
// `#`, which no server sees: it is read here, wiped from the address bar at once, and sent once to /g/redeem.
(() => {
  const T = {
    ru: {
      checking: ['Проверяем ссылку…', 'Ключ из адреса отправляется один раз и тут же стирается из строки браузера.'],
      pending: ['Подтвердите на компьютере', 'В планировщике на {computer} появилась просьба пустить «{label}». Нажмите «Пустить» — страница откроется сама.'],
      denied: ['Компьютер не пустил это устройство', 'Если это были вы, откройте «Шлюз» на компьютере, нажмите «Новая ссылка» и отсканируйте новый код.'],
      stale: ['Ссылка больше не работает', 'Она одноразовая и живёт 10 минут. На компьютере откройте «Шлюз» → «Новая ссылка» и наведите камеру на новый код.'],
      closed: ['Доступ закрыт', 'Хозяин закрыл шлюз или планировщик перезапустился. Нужна новая ссылка.'],
      gone: ['Доступ закончился', 'Сессия закрыта: вышел срок, не было действий или устройство отключили на компьютере. Нужна новая ссылка.'],
      slow: ['Слишком много попыток', 'Подождите {s} с и попробуйте снова.'],
      full: ['Уже подключено пять устройств', 'Отключите ненужное в «Шлюзе» на компьютере и откройте ссылку снова.'],
      nokey: ['Нужна ссылка', 'Откройте ссылку или отсканируйте QR-код из «Шлюза» на компьютере. Можно вставить ссылку или ключ сюда.'],
      arrived: ['Готово', 'Открываем…'],
      error: ['Не получилось', 'Нет связи или сервер ответил не так. Попробуйте ещё раз.'],
      enter: 'Войти', again: 'Повторить', cancel: 'Отмена', field: 'Ссылка или ключ',
    },
    en: {
      checking: ['Checking the link…', 'The key in the address is sent once and wiped from the browser’s address bar at once.'],
      pending: ['Confirm on the computer', 'The planner on {computer} is asking to let “{label}” in. Press “Let in” there — this page opens by itself.'],
      denied: ['The computer did not let this device in', 'If it was you, open “Gate” on the computer, press “New link” and scan the new code.'],
      stale: ['This link no longer works', 'It is single-use and lives 10 minutes. On the computer open “Gate” → “New link” and point the camera at the new code.'],
      closed: ['Access closed', 'The owner closed the gate or the planner restarted. A new link is needed.'],
      gone: ['Access has ended', 'The session was closed: it ran out, nothing happened for a while, or the device was disconnected on the computer. A new link is needed.'],
      slow: ['Too many attempts', 'Wait {s} s and try again.'],
      full: ['Five devices are already connected', 'Disconnect one in “Gate” on the computer and open the link again.'],
      nokey: ['A link is needed', 'Open the link or scan the QR code from “Gate” on the computer. You can paste the link or the key here.'],
      arrived: ['Done', 'Opening…'],
      error: ['That did not work', 'No connection, or the server answered unexpectedly. Try again.'],
      enter: 'Sign in', again: 'Try again', cancel: 'Cancel', field: 'Link or key',
    },
  };
  const lang = /^ru\b/i.test(navigator.language || '') ? 'ru' : 'en';
  const t = T[lang];
  document.documentElement.lang = lang;
  const $ = (id) => document.getElementById(id);
  const box = $('box'), head = $('h'), text = $('p'), form = $('f'), field = $('k'), again = $('again'), cancel = $('cancel');
  field.setAttribute('aria-label', t.field);
  field.placeholder = t.field;
  $('enter').textContent = t.enter;
  again.textContent = t.again;
  cancel.textContent = t.cancel;
  document.title = 'Herdr Planner';

  let last = null; // what the ask-again button repeats
  let timer = null;
  let who = { computer: '', label: '' };

  function show(state, vars) {
    box.dataset.s = state;
    const [h, p] = t[state];
    const v = Object.assign({}, who, vars || {});
    head.textContent = h;
    text.textContent = p.replace(/\{(\w+)\}/g, (_, k) => (v[k] === undefined ? '' : String(v[k])));
    form.hidden = state !== 'nokey';
    cancel.hidden = state !== 'pending';
    again.hidden = !(state === 'error' || state === 'slow');
    if (state === 'nokey') field.focus();
  }

  async function ask(method, path, body) {
    const res = await fetch(path, {
      method,
      credentials: 'same-origin',
      cache: 'no-store',
      headers: body ? { 'Content-Type': 'application/json' } : {},
      body: body ? JSON.stringify(body) : undefined,
    });
    let data = {};
    try { data = await res.json(); } catch (_) { /* a page, not JSON */ }
    return { code: res.status, data };
  }

  function arrive(next) {
    show('arrived');
    location.replace(next && next[0] === '/' ? next : '/m');
  }

  function settle(r) {
    if (r.code === 503) return show('closed');
    const s = r.data.state;
    if (r.data.computer) who = { computer: r.data.computer, label: r.data.label || who.label };
    if (s === 'active') return arrive(r.data.next);
    if (s === 'pending') { show('pending'); return wait(); }
    if (s === 'gone') return show(r.data.reason === 'denied' || r.data.reason === 'unanswered' ? 'denied' : 'gone');
    return show('stale');
  }

  function wait() {
    clearTimeout(timer);
    timer = setTimeout(async () => {
      try { settle(await ask('GET', '/g/status')); } catch (_) { wait(); }
    }, 1500);
  }

  async function redeem(key) {
    last = () => redeem(key);
    show('checking');
    try {
      const r = await ask('POST', '/g/redeem', { key, touch: navigator.maxTouchPoints > 1 && matchMedia('(pointer:coarse)').matches });
      if (r.code === 429) { last = () => redeem(key); return show('slow', { s: r.data.retry_s || 60 }); }
      if (r.code === 409) return show('full');
      if (r.code === 410) return show('stale');
      if (r.code !== 200 && r.code !== 503) return show('error');
      settle(r);
    } catch (_) {
      show('error');
    }
  }

  async function status() {
    last = status;
    show('checking');
    try {
      const r = await ask('GET', '/g/status');
      if (r.code === 200 && r.data.state === 'none') return show('nokey');
      settle(r);
    } catch (_) {
      show('error');
    }
  }

  function keyOf(raw) {
    const m = /(?:^|[#&?])k=([A-Za-z0-9_-]{16,64})/.exec(raw) || /^\s*([A-Za-z0-9_-]{16,64})\s*$/.exec(raw);
    return m ? m[1] : null;
  }

  form.addEventListener('submit', (e) => {
    e.preventDefault();
    const key = keyOf(field.value);
    field.value = '';
    if (key) redeem(key); else show('stale');
  });
  again.addEventListener('click', () => (last || status)());
  cancel.addEventListener('click', async () => {
    clearTimeout(timer);
    try { await ask('POST', '/g/cancel', {}); } catch (_) { /* the request lapses by itself */ }
    show('nokey');
  });

  const key = keyOf(location.hash);
  if (location.hash) history.replaceState(null, '', location.pathname + location.search);
  if (key) redeem(key); else status();
})();
