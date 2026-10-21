/* The line icons of the page as one SVG sprite (<use href="#g-name">). */
export const SPRITE = `
<svg width="0" height="0" style="position:absolute" aria-hidden="true"><defs>
<symbol id="g-working" viewBox="0 0 16 16"><circle cx="8" cy="8" r="5.6" opacity=".28"/><path d="M8 2.4a5.6 5.6 0 0 1 5.6 5.6"/></symbol>
<symbol id="g-review" viewBox="0 0 16 16"><circle cx="7" cy="7" r="4.4"/><path d="m10.3 10.3 3.6 3.6"/></symbol>
<symbol id="g-waiting" viewBox="0 0 16 16"><path d="M2.4 3.2h11.2v7.4H7.4l-3 2.7v-2.7H2.4z"/><path d="M8 5.5v2.1"/><circle cx="8" cy="8.9" r=".2" fill="currentColor"/></symbol>
<symbol id="g-plan" viewBox="0 0 16 16"><rect x="3" y="1.8" width="10" height="12.4" rx="1.6"/><path d="M5.6 5.8h4.8M5.6 8.3h4.8M5.6 10.8h2.6"/></symbol>
<symbol id="g-ready" viewBox="0 0 16 16"><circle cx="8" cy="8" r="6.2" fill="currentColor" stroke="none"/><path d="m5 8.2 2.2 2.2L11.2 6" style="stroke:var(--pl-bg)"/></symbol>
<symbol id="g-failed" viewBox="0 0 16 16"><path d="M8 2.2 14.4 13.4H1.6z"/><path d="M8 6.4v3.1"/><circle cx="8" cy="11.3" r=".2" fill="currentColor"/></symbol>
<symbol id="g-queued" viewBox="0 0 16 16"><circle cx="8" cy="8" r="5.8" stroke-dasharray="2.2 2"/><path d="M8 5v3.2l2 1.3"/></symbol>
<symbol id="g-done" viewBox="0 0 16 16"><path d="m3.4 8.6 3.2 3.1 6-6.9"/></symbol>
<symbol id="g-chev" viewBox="0 0 16 16"><path d="m4 6 4 4 4-4"/></symbol>
<symbol id="g-chev-r" viewBox="0 0 16 16"><path d="m6 4 4 4-4 4"/></symbol>
<symbol id="g-folder" viewBox="0 0 16 16"><path d="M1.8 4.4a1 1 0 0 1 1-1h3.2l1.5 1.8h5.7a1 1 0 0 1 1 1v6a1 1 0 0 1-1 1H2.8a1 1 0 0 1-1-1z"/></symbol>
<symbol id="g-ext" viewBox="0 0 16 16"><path d="M9 2.8h4.2V7M13 3 7.4 8.6M11.2 9.6v3.2H3.2v-8h3.2"/></symbol>
<symbol id="g-planner" viewBox="0 0 16 16"><rect x="2" y="2.4" width="5" height="5" rx="1"/><rect x="9" y="2.4" width="5" height="3" rx="1"/><rect x="2" y="9.4" width="5" height="4.2" rx="1"/><rect x="9" y="7.4" width="5" height="6.2" rx="1"/></symbol>
<symbol id="g-archive" viewBox="0 0 16 16"><rect x="2" y="3" width="12" height="3" rx=".8"/><path d="M3 6v6.2a.8.8 0 0 0 .8.8h8.4a.8.8 0 0 0 .8-.8V6M6.2 9h3.6"/></symbol>
<symbol id="g-refresh" viewBox="0 0 16 16"><path d="M13.2 6.4A5.4 5.4 0 1 0 13 9.8"/><path d="M13.4 3.2v3.4H10"/></symbol>
<symbol id="g-logout" viewBox="0 0 16 16"><path d="M9.4 2.8H3.6v10.4h5.8M7.4 8h6.2M11 5.6 13.4 8 11 10.4"/></symbol>
<symbol id="g-help" viewBox="0 0 16 16"><circle cx="8" cy="8" r="5.8"/><path d="M6.6 6.3a1.5 1.5 0 1 1 2.2 1.2c-.5.3-.8.7-.8 1.2"/><circle cx="8" cy="11.1" r=".2" fill="currentColor"/></symbol>
<symbol id="g-send" viewBox="0 0 16 16"><path d="M2.4 8h10M8.6 4.2 12.4 8l-3.8 3.8"/></symbol>
<symbol id="g-close" viewBox="0 0 16 16"><path d="m4 4 8 8M12 4l-8 8"/></symbol>
<symbol id="g-attach" viewBox="0 0 16 16"><path d="M10.8 5.2 6.1 9.9a1.7 1.7 0 0 0 2.4 2.4l5-5a3.1 3.1 0 0 0-4.4-4.4l-5 5a4.5 4.5 0 0 0 6.4 6.4l4.2-4.2"/></symbol>
<symbol id="g-link" viewBox="0 0 16 16"><path d="M6.2 9.8 9.8 6.2M5.4 7.4 3.9 8.9a2.6 2.6 0 0 0 3.7 3.7l1.5-1.5M10.6 8.6l1.5-1.5a2.6 2.6 0 0 0-3.7-3.7L6.9 4.9"/></symbol>
<symbol id="g-bell" viewBox="0 0 16 16"><path d="M8 2.2a4 4 0 0 1 4 4v3l1.2 2H2.8L4 9.2v-3a4 4 0 0 1 4-4zM6.6 13a1.5 1.5 0 0 0 2.8 0"/></symbol>
<symbol id="g-universe" viewBox="0 0 16 16"><circle cx="8" cy="8" r="2"/><ellipse cx="8" cy="8" rx="6.4" ry="2.6" transform="rotate(-24 8 8)"/></symbol>
<symbol id="g-list" viewBox="0 0 16 16"><path d="M5.2 4h8M5.2 8h8M5.2 12h8"/><circle cx="2.8" cy="4" r=".7" fill="currentColor"/><circle cx="2.8" cy="8" r=".7" fill="currentColor"/><circle cx="2.8" cy="12" r=".7" fill="currentColor"/></symbol>
<symbol id="g-screen" viewBox="0 0 16 16"><rect x="1.8" y="2.8" width="12.4" height="8.4" rx="1.2"/><path d="M5.6 14h4.8M8 11.2V14"/></symbol>
<symbol id="g-limit" viewBox="0 0 16 16"><path d="M2.8 12.4a5.4 5.4 0 1 1 10.4 0"/><path d="M8 10.2 10.6 6.4"/><circle cx="8" cy="12.2" r="1"/></symbol>
<symbol id="g-search" viewBox="0 0 16 16"><circle cx="7" cy="7" r="4.4"/><path d="m10.3 10.3 3.6 3.6"/></symbol>
<symbol id="g-quasar" viewBox="0 0 16 16"><circle cx="8" cy="8" r="2.4" fill="currentColor"/><path d="M8 1.5v3M8 11.5v3"/><ellipse cx="8" cy="8" rx="6.3" ry="2.2"/></symbol>
<symbol id="g-galaxy" viewBox="0 0 16 16"><circle cx="8" cy="8" r="1.4" fill="currentColor"/><path d="M8 8c3-4 6-2 5.5 0.5S9 12 6 11.5 2.5 7 5 4.6"/></symbol>
<symbol id="g-moon" viewBox="0 0 16 16"><circle cx="8" cy="8" r="3"/><circle cx="12.6" cy="4" r="1.2" fill="currentColor"/></symbol>
<symbol id="g-chat" viewBox="0 0 16 16"><path d="M2.4 3.2h11.2v7.4H7.4l-3 2.7v-2.7H2.4z"/></symbol>
<symbol id="g-ship" viewBox="0 0 16 16"><path d="M8 1.8 11.4 12H8.8L8 13.6 7.2 12H4.6z"/><path d="M4.6 9.4 2.4 12.6M11.4 9.4l2.2 3.2"/></symbol>
<symbol id="g-plus" viewBox="0 0 16 16"><path d="M8 3v10M3 8h10"/></symbol>
<symbol id="g-up" viewBox="0 0 16 16"><path d="m4 10 4-4 4 4"/></symbol>
<symbol id="g-down" viewBox="0 0 16 16"><path d="m4 6 4 4 4-4"/></symbol>
<symbol id="g-home" viewBox="0 0 16 16"><path d="M2.4 8 8 2.8 13.6 8M4 7v6h3V9.6h2V13h3V7"/></symbol>
<symbol id="g-globe" viewBox="0 0 16 16"><circle cx="8" cy="8" r="5.8"/><path d="M2.4 8h11.2M8 2.2c2 2 2 9.6 0 11.6M8 2.2c-2 2-2 9.6 0 11.6"/></symbol>
</defs></svg>`;
