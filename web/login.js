'use strict';
/* Posts the login form; the server sets the HttpOnly session cookie. */
const form = document.getElementById('login');
const err = document.getElementById('err');
const go = document.getElementById('go');
const qs = new URLSearchParams(location.search);
const loopback = ['127.0.0.1', 'localhost', '::1', '[::1]'].includes(location.hostname);

// Only a path on this site: `//host`, `/\host` and whitespace tricks would
// otherwise send the browser to another origin.
function safeNext(raw) {
  const next = raw || '/';
  if (!next.startsWith('/') || next.startsWith('//') || /[\\\s]/.test(next)) return '/';
  return next;
}

form.addEventListener('submit', async (e) => {
  e.preventDefault();
  err.textContent = '';
  // The session cookie is Secure. Over plain HTTP from another machine the
  // server would accept the password but the browser would drop the cookie,
  // so refuse before any credentials leave the machine.
  if (location.protocol === 'http:' && !loopback) {
    err.textContent = 'Удалённый вход работает только по HTTPS: включите TLS или прокси.';
    return;
  }
  go.disabled = true;
  const user = document.getElementById('user').value;
  const password = document.getElementById('pass').value;
  try {
    const r = await fetch('/api/login', {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      credentials: 'same-origin',
      body: JSON.stringify({ user, password }),
    });
    const data = await r.json().catch(() => ({}));
    if (!r.ok) {
      err.textContent = data.error || 'Не удалось войти';
      go.disabled = false;
      return;
    }
    location.href = safeNext(qs.get('next'));
  } catch (e2) {
    err.textContent = 'Сервер не отвечает: ' + e2.message;
    go.disabled = false;
  }
});
