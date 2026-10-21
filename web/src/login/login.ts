/* Posts the login form; the server sets the HttpOnly session cookie. */
import './login.css';
import { lang, t } from '../i18n';

const form = document.getElementById('login') as HTMLFormElement;
const err = document.getElementById('err') as HTMLElement;
const go = document.getElementById('go') as HTMLButtonElement;
const qs = new URLSearchParams(location.search);
const loopback = ['127.0.0.1', 'localhost', '::1', '[::1]'].includes(location.hostname);

// The page's own texts follow the language of the planner (remembered in this browser).
function translate(): void {
  document.documentElement.lang = lang.value;
  document.title = t('login.title');
  for (const el of document.querySelectorAll<HTMLElement>('[data-i18n]')) {
    const text = t(el.dataset.i18n ?? '', { url: el.dataset.url ?? '' });
    if (el.dataset.url) el.innerHTML = text.replace(el.dataset.url, `<code>${el.dataset.url}</code>`);
    else el.textContent = text;
  }
}
translate();

// Only a path on this site: `//host`, `/\host` and whitespace tricks would
// otherwise send the browser to another origin.
function safeNext(raw: string | null): string {
  const next = raw || '/';
  if (!next.startsWith('/') || next.startsWith('//') || /[\\\s]/.test(next)) return '/';
  return next;
}

// Exposed for the checks that the redirect target stays on this site.
(window as unknown as { safeNext: typeof safeNext }).safeNext = safeNext;

form.addEventListener('submit', async (e) => {
  e.preventDefault();
  err.textContent = '';
  // The session cookie is Secure. Over plain HTTP from another machine the
  // server would accept the password but the browser would drop the cookie,
  // so refuse before any credentials leave the machine.
  if (location.protocol === 'http:' && !loopback) {
    err.textContent = t('login.https');
    return;
  }
  go.disabled = true;
  const user = (document.getElementById('user') as HTMLInputElement).value;
  const password = (document.getElementById('pass') as HTMLInputElement).value;
  try {
    const r = await fetch('/api/login', {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      credentials: 'same-origin',
      body: JSON.stringify({ user, password }),
    });
    const data = await r.json().catch(() => ({}));
    if (!r.ok) {
      err.textContent = data.code === 'invalid' ? t('login.invalid')
        : data.code === 'too_many' ? t('login.tooMany', { n: data.retry_after ?? 0 })
        : data.error || t('login.failed');
      go.disabled = false;
      return;
    }
    location.href = safeNext(qs.get('next'));
  } catch (e2) {
    err.textContent = t('login.noServer', { why: (e2 as Error).message });
    go.disabled = false;
  }
});
