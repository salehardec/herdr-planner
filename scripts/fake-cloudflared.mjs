#!/usr/bin/env node
// A stand-in for `cloudflared`, for the tunnel's tests (src/tunnel.rs, tests/tunnel_shutdown.rs). It takes the same
// arguments the planner gives the real one and behaves as the real one was seen to on a machine (PLN-71):
//
//   quick tunnel   fake-cloudflared --config F tunnel --no-autoupdate --metrics 127.0.0.1:P --url http://127.0.0.1:N
//   named tunnel   fake-cloudflared --config F tunnel --no-autoupdate --metrics 127.0.0.1:P run
//
// It serves the two metrics endpoints the planner reads (`/ready`, and `/quicktunnel` in the quick mode), writes
// cloudflared-looking lines to stderr and leaves when it gets SIGTERM. It never opens the config file, and so never
// the credentials file the config names. What it does is chosen by FAKE_CF, a comma-separated list:
//
//   ready=MS       how long after the start the tunnel becomes ready (default 150)
//   never=1        never ready
//   host=NAME      the quick tunnel's address (default fake-quick-tunnel.trycloudflare.com)
//   noapi=1        no /quicktunnel: the address is only in the frame written to stderr
//   die=MS         exit with code 1 this long after the start
//   fail=N         the first N starts (counted in the file FAKE_CF_STATE) fail with «failed to request quick Tunnel»
//   blip=A:B       not ready between A and B ms after becoming ready, ready again after
//   lost=MS        not ready from MS after becoming ready, for good
//   ignoreterm=1   SIGTERM changes nothing (the planner has to use SIGKILL)
//   leak=1         writes a line that names a credentials file (the planner must not show it)
//
// FAKE_CF_ARGS names a file that gets the arguments (JSON), FAKE_CF_PIDFILE one that gets the pid.
import fs from 'node:fs';
import http from 'node:http';

const opts = Object.fromEntries(
  (process.env.FAKE_CF || '')
    .split(',')
    .filter(Boolean)
    .map((kv) => {
      const [k, ...v] = kv.split('=');
      return [k, v.join('=')];
    }),
);
const num = (k, d) => (opts[k] === undefined ? d : Number(opts[k]));
const args = process.argv.slice(2);
if (args.includes('--version')) {
  console.log('cloudflared version 2026.8.3 (fake, for the planner\'s tests)');
  process.exit(0);
}
const after = (flag) => {
  const i = args.indexOf(flag);
  return i >= 0 ? args[i + 1] : undefined;
};
if (process.env.FAKE_CF_ARGS) fs.writeFileSync(process.env.FAKE_CF_ARGS, JSON.stringify(args));
if (process.env.FAKE_CF_PIDFILE) fs.writeFileSync(process.env.FAKE_CF_PIDFILE, String(process.pid));

const log = (level, text) => process.stderr.write(`${new Date().toISOString().replace(/\.\d+Z$/, 'Z')} ${level} ${text}\n`);
const named = args.includes('run');
const host = opts.host || 'fake-quick-tunnel.trycloudflare.com';

log('INF', 'Starting tunnel');
if (!named) log('ERR', 'Configuration file ' + (after('--config') || '') + ' was empty');
if (opts.leak) log('INF', 'Settings: map[credentials-file:/home/me/.cloudflared/secret.json no-autoupdate:true]');

if (opts.fail) {
  const stateFile = process.env.FAKE_CF_STATE;
  let n = 0;
  try {
    n = Number(fs.readFileSync(stateFile, 'utf8')) || 0;
  } catch {}
  fs.writeFileSync(stateFile, String(n + 1));
  if (n < Number(opts.fail)) {
    log('ERR', 'failed to request quick Tunnel: Post "https://api.trycloudflare.com/tunnel": 429 Too Many Requests');
    process.exit(1);
  }
}

const started = Date.now();
const readyAt = started + num('ready', 150);
let readySince = 0;
const isReady = () => {
  if (opts.never || Date.now() < readyAt) return false;
  if (!readySince) readySince = Date.now();
  const t = Date.now() - readyAt;
  if (opts.lost !== undefined && t >= Number(opts.lost)) return false;
  if (opts.blip) {
    const [a, b] = opts.blip.split(':').map(Number);
    if (t >= a && t < b) return false;
  }
  return true;
};

const server = http.createServer((req, res) => {
  const json = (code, body) => {
    res.writeHead(code, { 'content-type': 'application/json' });
    res.end(JSON.stringify(body));
  };
  if (req.url === '/ready') {
    return isReady()
      ? json(200, { status: 200, readyConnections: 1, connectorId: '00000000-0000-0000-0000-000000000000' })
      : json(503, { status: 503, readyConnections: 0 });
  }
  if (req.url === '/quicktunnel' && !named && !opts.noapi) return json(200, { hostname: host });
  json(404, {});
});
const metrics = after('--metrics') || '127.0.0.1:0';
const at = metrics.lastIndexOf(':');
server.listen(Number(metrics.slice(at + 1)), metrics.slice(0, at), () => {
  if (!named) {
    log('INF', 'Requesting new quick Tunnel on trycloudflare.com...');
    setTimeout(() => {
      log('INF', 'Your quick Tunnel has been created! Visit it at (it may take some time to be reachable):');
      log('INF', '+--------------------------------------------------------------------------------------------+');
      log('INF', `|  https://${host}                                                    |`);
      log('INF', '+--------------------------------------------------------------------------------------------+');
    }, Math.min(80, num('ready', 150)));
  }
  setTimeout(() => {
    if (!opts.never) log('INF', 'Registered tunnel connection connIndex=0 location=fra01 protocol=quic');
  }, num('ready', 150));
});
if (opts.die) setTimeout(() => {
  log('ERR', 'Unable to establish connection with Cloudflare edge');
  process.exit(1);
}, Number(opts.die));

if (!opts.ignoreterm) {
  process.on('SIGTERM', () => {
    log('INF', 'Initiating graceful shutdown due to signal terminated ...');
    log('INF', 'Tunnel server stopped');
    process.exit(0);
  });
} else {
  process.on('SIGTERM', () => log('INF', 'SIGTERM ignored'));
}
