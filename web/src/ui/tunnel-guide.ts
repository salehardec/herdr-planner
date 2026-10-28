/* The commands and the config the step-by-step guide of the gate shows, with the owner's own address, config path, port
   and tunnel id put in. Pure: the guide only draws what this returns (tunnel-guide.test.ts). */

export const DEFAULT_CONFIG = '~/.cloudflared/herdr-planner.yml';
export const EXAMPLE_HOST = 'planner.example.com';
export const UUID_PLACEHOLDER = '<UUID>';
export const RELEASES_URL = 'https://github.com/cloudflare/cloudflared/releases/latest';
export const PACKAGES_URL = 'https://pkg.cloudflare.com/';
export const DASH_URL = 'https://dash.cloudflare.com/';
export const ACCESS_URL = 'https://one.dash.cloudflare.com/';

export interface GuideInput {
  /** What is typed in the planner's «Config» and «Address» fields. */
  config: string;
  host: string;
  /** The planner's port and the user's home folder (the server tells both). */
  port: number;
  home: string;
  /** The tunnel's id, from the config or typed in the guide. */
  uuid: string;
}

export interface Guide {
  config: string;
  host: string;
  uuid: string;
  /** Whether the values come from the owner's fields (the guide says «your values are in») or are examples. */
  hostGiven: boolean;
  configGiven: boolean;
  uuidGiven: boolean;
  install: { pacman: string; brew: string; apt: string };
  login: string;
  certCheck: string;
  create: string;
  /** The config file's text, and the command that writes it. */
  yaml: string;
  route: string;
  /** The line for the config of a network where QUIC does not get through. */
  http2: string;
  /** The same line for a quick tunnel's empty config. */
  quickConfig: string;
}

export const isUuid = (s: string): boolean => /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i.test(s.trim());

/** A word for a shell: left alone when it is plain, else in single quotes (a leading `~/` stays outside, the shell expands it there). */
export function shq(s: string): string {
  if (/^[\w@%+=:,./~-]+$/.test(s)) return s;
  const home = s.startsWith('~/');
  const rest = home ? s.slice(2) : s;
  return (home ? '~/' : '') + `'${rest.replace(/'/g, `'\\''`)}'`;
}

/** The host as a bare DNS name: what a person pastes may be a whole link. */
export function bareHost(s: string): string {
  return s.trim().replace(/^[a-z]+:\/\//i, '').replace(/[/?#].*$/, '').replace(/:\d+$/, '').toLowerCase();
}

/** `~/…` written out for a file that is not read by a shell (cloudflared's config does not expand it). */
export function absolute(path: string, home: string): string {
  return path.startsWith('~/') && home ? `${home.replace(/\/+$/, '')}/${path.slice(2)}` : path;
}

export function guideFor(i: GuideInput): Guide {
  const configGiven = i.config.trim() !== '';
  const hostGiven = bareHost(i.host) !== '';
  const uuidGiven = isUuid(i.uuid);
  const config = configGiven ? i.config.trim() : DEFAULT_CONFIG;
  const host = hostGiven ? bareHost(i.host) : EXAMPLE_HOST;
  const uuid = uuidGiven ? i.uuid.trim().toLowerCase() : UUID_PLACEHOLDER;
  const port = i.port > 0 ? i.port : 7117;
  const credentials = absolute(`~/.cloudflared/${uuid}.json`, i.home);
  const yaml = [
    `tunnel: ${uuid}`,
    `credentials-file: ${credentials}`,
    '',
    'ingress:',
    `  - hostname: ${host}`,
    `    service: http://127.0.0.1:${port}`,
    '  - service: http_status:404',
    '',
  ].join('\n');
  return {
    config, host, uuid, hostGiven, configGiven, uuidGiven,
    install: { pacman: 'sudo pacman -S cloudflared', brew: 'brew install cloudflared', apt: 'sudo apt install cloudflared' },
    login: 'cloudflared tunnel login',
    certCheck: 'ls ~/.cloudflared/cert.pem',
    create: 'cloudflared tunnel create herdr-planner',
    yaml,
    // --config on purpose: without it cloudflared takes the tunnel from ~/.cloudflared/config.yml, which may be another one
    route: `cloudflared --config ${shq(config)} tunnel route dns --overwrite-dns ${uuid} ${host}`,
    http2: 'protocol: http2',
    quickConfig: '~/.cloudflared/quick-empty.yml',
  };
}
