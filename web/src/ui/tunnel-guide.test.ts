import { describe, expect, it } from 'vitest';
import { absolute, bareHost, guideFor, isUuid, shq, DEFAULT_CONFIG, EXAMPLE_HOST, UUID_PLACEHOLDER } from './tunnel-guide';
import { ru } from '../i18n/ru';
import { en } from '../i18n/en';

const UUID = '6ff42ae2-765d-4adf-8112-31c55c1551ef';
const base = { config: '', host: '', port: 7117, home: '/home/me', uuid: '' };

describe('the gate guide', () => {
  it('shows examples until the owner has typed their own', () => {
    const g = guideFor(base);
    expect(g.config).toBe(DEFAULT_CONFIG);
    expect(g.host).toBe(EXAMPLE_HOST);
    expect(g.uuid).toBe(UUID_PLACEHOLDER);
    expect([g.configGiven, g.hostGiven, g.uuidGiven]).toEqual([false, false, false]);
  });

  it('puts the typed address, config path and id into the commands', () => {
    const g = guideFor({ ...base, config: '~/tunnels/pl.yml', host: 'planner.yanao.su', uuid: UUID });
    expect(g.route).toBe(`cloudflared --config ~/tunnels/pl.yml tunnel route dns --overwrite-dns ${UUID} planner.yanao.su`);
    expect(g.yaml).toContain(`tunnel: ${UUID}`);
    expect(g.yaml).toContain('  - hostname: planner.yanao.su');
    expect([g.configGiven, g.hostGiven, g.uuidGiven]).toEqual([true, true, true]);
  });

  it('always names the config in route dns: without it another tunnel gets the record', () => {
    expect(guideFor(base).route).toMatch(/^cloudflared --config \S+ tunnel route dns --overwrite-dns /);
  });

  it('writes the planner’s real port and a credentials path that needs no shell', () => {
    const g = guideFor({ ...base, port: 8123, uuid: UUID });
    expect(g.yaml).toContain('    service: http://127.0.0.1:8123');
    expect(g.yaml).toContain(`credentials-file: /home/me/.cloudflared/${UUID}.json`);
    expect(g.yaml.trimEnd().endsWith('- service: http_status:404')).toBe(true);
  });

  it('takes a pasted link for an address, and a word that is not an id for none', () => {
    expect(bareHost(' https://Planner.Yanao.su:443/m?x=1 ')).toBe('planner.yanao.su');
    expect(guideFor({ ...base, host: 'https://planner.yanao.su/' }).host).toBe('planner.yanao.su');
    expect(isUuid('herdr-planner')).toBe(false);
    expect(isUuid(UUID)).toBe(true);
    expect(guideFor({ ...base, uuid: 'herdr-planner' }).uuid).toBe(UUID_PLACEHOLDER);
  });

  it('quotes what a shell would split, and leaves ~/ outside the quotes', () => {
    expect(shq('/a/b.yml')).toBe('/a/b.yml');
    expect(shq('/my dir/a.yml')).toBe(`'/my dir/a.yml'`);
    expect(shq('~/my dir/a.yml')).toBe(`~/'my dir/a.yml'`);
    expect(shq(`/it's/a.yml`)).toBe(`'/it'\\''s/a.yml'`);
    expect(guideFor({ ...base, config: '/my dir/a.yml' }).route).toContain(`--config '/my dir/a.yml' tunnel`);
  });

  it('writes ~/ out only for a file no shell reads', () => {
    expect(absolute('~/.cloudflared/x.json', '/home/me/')).toBe('/home/me/.cloudflared/x.json');
    expect(absolute('/etc/x.json', '/home/me')).toBe('/etc/x.json');
    expect(absolute('~/x', '')).toBe('~/x');
  });
});

describe('the words of the gate', () => {
  const ids = ['file', 'hostname', 'tunnel', 'credentials', 'credfile', 'service', 'catchall'];
  it('every line of «Check» has its words in both languages', () => {
    for (const id of ['file', 'hostname', 'tunnel', 'credentials', 'service', 'catch-all', 'credentials-file']) {
      for (const k of [`tun.chk.${id}.ok`, `tun.chk.${id}.bad`]) {
        expect(ru[k], k).toBeTruthy();
        expect(en[k], k).toBeTruthy();
      }
    }
    expect(ids.length).toBeGreaterThan(0);
  });
  it('every state of the chip, every kind of failure and every idle choice has words', () => {
    for (const s of ['closed', 'starting', 'open', 'pending', 'reconnecting', 'failed', 'missing', 'external', 'closing']) {
      for (const k of [`tun.st.${s}`, `tun.sub.${s}`]) { expect(ru[k], k).toBeTruthy(); expect(en[k], k).toBeTruthy(); }
    }
    for (const k of ['config-missing', 'no-hostname', 'hostname-mismatch', 'spawn', 'timeout', 'exited', 'lost']) {
      expect(ru[`tun.err.${k}`], k).toBeTruthy();
      expect(en[`tun.err.${k}`], k).toBeTruthy();
    }
    for (const h of [0, 1, 4, 12]) { expect(ru[`tun.idle.${h}`]).toBeTruthy(); expect(en[`tun.idle.${h}`]).toBeTruthy(); }
  });
});
