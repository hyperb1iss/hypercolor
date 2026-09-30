import assert from 'node:assert/strict';
import { execFile } from 'node:child_process';
import { randomBytes, randomUUID } from 'node:crypto';
import { createSocket } from 'node:dgram';
import { createServer } from 'node:http';
import { setTimeout as delay } from 'node:timers/promises';
import { promisify } from 'node:util';

const execute = promisify(execFile);

// The emulator runs inside its own bridge network. No host lighting is scanned.
async function emulateWled() {
  let frames = 0;
  const colors = new Set();
  const socket = createSocket('udp4');
  socket.on('message', (packet) => {
    if (packet.length === 58 && packet[0] === 0x41 && packet[2] === 0x0b
      && packet[3] === 1 && packet.readUInt32BE(4) === 0 && packet.readUInt16BE(8) === 48) {
      frames += 1;
      colors.add(packet.subarray(10).toString('hex'));
    }
  });
  socket.bind(4048);
  createServer((request, response) => {
    response.setHeader('content-type', 'application/json');
    if (request.url === '/proof') {
      response.end(JSON.stringify({ frames, colors: colors.size, nonzero: [...colors].some((c) => /[1-9a-f]/.test(c)) }));
    } else if (request.url === '/json/info') {
      response.end(JSON.stringify({
        ver: '0.15.0', name: 'Docker WLED proof', mac: '020000000001', arch: 'esp32',
        leds: { count: 16, rgbw: false, maxseg: 1, fps: 60 },
      }));
    } else if (request.url === '/json/state') {
      request.resume();
      response.end(JSON.stringify({ on: true, live: true, seg: [{ id: 0, start: 0, stop: 16, lc: 1 }] }));
    } else if (request.url === '/json/cfg') {
      response.end('{}');
    } else {
      response.writeHead(404).end('{}');
    }
  }).listen(80, '0.0.0.0');
}

async function smoke(image) {
  const engine = process.env.CONTAINER_ENGINE ?? 'docker';
  const prefix = `hypercolor-proof-${randomUUID().slice(0, 8)}`;
  const network = `${prefix}-net`;
  const volume = `${prefix}-data`;
  const daemon = `${prefix}-daemon`;
  const emulator = `${prefix}-wled`;
  const key = randomBytes(32).toString('hex');
  const owned = [];
  const run = async (...args) => (await execute(engine, args, { maxBuffer: 10 * 1024 * 1024 })).stdout.trim();
  const waitFor = async (label, probe) => {
    const deadline = Date.now() + 60_000;
    let lastError;
    while (Date.now() < deadline) {
      try {
        const value = await probe();
        if (value) return value;
      } catch (error) {
        lastError = error;
      }
      await delay(1000);
    }
    throw new Error(`Timed out waiting for ${label}`, { cause: lastError });
  };
  const origin = async (name) => {
    const binding = await run('port', name);
    const port = binding.match(/127\.0\.0\.1:(\d+)/)?.[1];
    assert.ok(port, `missing loopback port binding: ${binding}`);
    return `http://127.0.0.1:${port}`;
  };

  try {
    await run('network', 'create', network);
    owned.push(['network', 'rm', network]);
    await run('volume', 'create', volume);
    owned.push(['volume', 'rm', volume]);
    await run('run', '--rm', '--entrypoint', '/bin/sh', '-v', `${volume}:/var/lib/hypercolor`, image, '-ec',
      'for dir in /var/lib/hypercolor /var/lib/hypercolor/config /var/lib/hypercolor/data /var/lib/hypercolor/state /var/lib/hypercolor/cache; do test -w "$dir"; done');
    await run('create', '--name', emulator, '--network', network, '-p', '127.0.0.1::80',
      'docker.io/library/node:24-bookworm-slim', 'node', '/proof.mjs', '--wled-emulator');
    owned.push(['rm', '--force', emulator]);
    await run('cp', import.meta.filename, `${emulator}:/proof.mjs`);
    await run('start', emulator);
    const wledOrigin = await origin(emulator);
    await waitFor('WLED emulator', async () => (await fetch(`${wledOrigin}/json/info`, { signal: AbortSignal.timeout(5000) })).ok);
    const wledIp = await run('inspect', '--format', '{{range .NetworkSettings.Networks}}{{.IPAddress}}{{end}}', emulator);
    assert.match(wledIp, /^\d+\.\d+\.\d+\.\d+$/);
    const config = `schema_version = 5
[audio]
enabled = false
[capture]
enabled = false
[session]
enabled = false
[discovery]
background_enabled = false
mdns_enabled = false
blocks_scan = false
[network]
mdns_publish = false
[drivers.wled]
enabled = true
known_ips = ["${wledIp}"]
`;
    await run('run', '--rm', '--user', '0', '--entrypoint', '/bin/sh', '-v', `${volume}:/var/lib/hypercolor`,
      '-e', `HYPERCOLOR_TEST_CONFIG=${config}`, image, '-ec',
      'install -d -o 10001 -g 10001 /var/lib/hypercolor/config/hypercolor; printf "%s" "$HYPERCOLOR_TEST_CONFIG" > /var/lib/hypercolor/config/hypercolor/hypercolor.toml; chown 10001:10001 /var/lib/hypercolor/config/hypercolor/hypercolor.toml');

    const launch = async () => {
      await run('run', '--detach', '--name', daemon, '--network', network,
        '-p', '127.0.0.1::9420', '-v', `${volume}:/var/lib/hypercolor`, '-e', `HYPERCOLOR_API_KEY=${key}`, image);
      if (!owned.some((args) => args.at(-1) === daemon)) owned.push(['rm', '--force', daemon]);
      const apiOrigin = await origin(daemon);
      await waitFor('healthy daemon', async () => (await fetch(`${apiOrigin}/health`, { signal: AbortSignal.timeout(5000) })).ok);
      return apiOrigin;
    };
    let apiOrigin = await launch();
    const api = async (route, method = 'GET', body) => {
      const response = await fetch(`${apiOrigin}/api/v1${route}`, {
        method, headers: { authorization: `Bearer ${key}`, 'content-type': 'application/json' },
        body: body === undefined ? undefined : JSON.stringify(body),
        signal: AbortSignal.timeout(15_000),
      });
      const payload = await response.json();
      assert.ok(response.ok, `${method} ${route}: ${response.status} ${JSON.stringify(payload)}`);
      return payload.data;
    };
    const user = await run('exec', daemon, 'id', '-u');
    assert.equal(user, '10001', 'daemon must run unprivileged');
    const unauthenticated = await fetch(`${apiOrigin}/api/v1/effects`);
    assert.equal(unauthenticated.status, 401, 'remote API must require its key');
    const page = await fetch(apiOrigin);
    assert.ok(page.ok, 'web UI must be served');
    assert.match(await page.text(), /<html/i);
    const effects = await api('/effects');
    const html = effects.items.find((effect) => effect.source === 'html' && effect.runnable && effect.name === 'Gradient');
    assert.ok(html, 'packaged Gradient HTML effect must be runnable');
    const discovery = await api('/devices/discover', 'POST', { targets: ['wled'], wait: true, timeout_ms: 1000 });
    assert.ok(discovery.result?.scanners?.some((scanner) => scanner.discovered > 0), `WLED emulator must be discovered: ${JSON.stringify(discovery)}`);
    const device = (await api('/devices')).items.find((item) => item.name === 'Docker WLED proof');
    assert.ok(device, 'discovered WLED device must be listed');
    const layout = await api('/layouts', 'POST', { name: 'Docker WLED layout' });
    await api(`/layouts/${layout.id}`, 'PUT', { zones: [{
      id: 'proof-strip', name: 'WLED strip', device_id: device.layout_device_id,
      position: { x: 0.5, y: 0.5 }, size: { x: 0.8, y: 0.2 }, rotation: 0,
      topology: { type: 'strip', count: 16, direction: 'left_to_right' },
    }] });
    await api(`/layouts/${layout.id}/apply`, 'POST', {});
    const applied = await api(`/effects/${html.id}/apply`, 'POST', {});
    await api(`/scene/zones/${applied.zone.id}/members`, 'POST', { device_id: device.layout_device_id });
    const health = await waitFor('Servo rendered frames', async () => {
      const system = await api('/system');
      const health = system.status?.effect_health;
      return Number(health?.servo_render_cpu_frames_total) > 0 && health;
    });
    assert.equal(Number(health.servo_page_load_failures_total), 0, 'HTML effect must load successfully');
    const delivery = await waitFor('animated DDP pixel delivery', async () => {
      const proof = await (await fetch(`${wledOrigin}/proof`, { signal: AbortSignal.timeout(5000) })).json();
      return proof.frames >= 3 && proof.colors > 1 && proof.nonzero && proof;
    });
    await api('/output', 'PATCH', { brightness: 0.73 });
    const snapshot = await api('/scenes/snapshot', 'POST', { name: 'Docker persistence proof' });
    await api(`/scenes/${snapshot.id}/activate`, 'POST', {});
    const scene = await api('/scene');
    assert.ok(scene.zones.some((zone) => zone.layers.some((layer) => layer.source.effect_id === html.id)));
    await run('stop', '--time', '30', daemon);
    assert.equal(await run('inspect', '--format', '{{.State.ExitCode}}', daemon), '0', 'SIGTERM must finish cleanly');
    await run('rm', daemon);
    const beforeReplacement = await (await fetch(`${wledOrigin}/proof`, { signal: AbortSignal.timeout(5000) })).json();
    apiOrigin = await launch();
    const scenes = await api('/scenes');
    assert.ok(scenes.items.some((item) => item.id === snapshot.id && item.name === 'Docker persistence proof'));
    const restored = await api('/scene');
    assert.equal(restored.id, scene.id, 'active scene must survive container replacement');
    assert.ok(restored.zones.some((zone) => zone.layers.some((layer) => layer.source.effect_id === html.id)));
    const output = await api('/output');
    assert.ok(Math.abs(output.brightness - 0.73) < 0.001, 'output state must survive container replacement');
    assert.match(await run('exec', daemon, 'cat', '/var/lib/hypercolor/config/hypercolor/hypercolor.toml'), /known_ips/);
    // This fixture disables background discovery, so rediscover explicitly.
    await api('/devices/discover', 'POST', { targets: ['wled'], wait: true, timeout_ms: 1000 });
    await waitFor('restored HTML rendering and WLED output', async () => {
      const system = await api('/system');
      const proof = await (await fetch(`${wledOrigin}/proof`, { signal: AbortSignal.timeout(5000) })).json();
      return Number(system.status?.effect_health?.servo_render_cpu_frames_total) > 0
        && proof.frames >= beforeReplacement.frames + 3;
    });
    await run('exec', daemon, 'curl', '--fail', '--silent', 'http://127.0.0.1:9420/health');
    await run('stop', '--time', '30', daemon);
    assert.equal(await run('inspect', '--format', '{{.State.ExitCode}}', daemon), '0');
    console.log(`PASS: ${image}: nonroot startup, authenticated API, UI, headless HTML rendering, ${delivery.frames} DDP frames, persisted config/scene/output, clean SIGTERM`);
  } catch (error) {
    for (const name of [daemon, emulator]) {
      try { console.error(await run('logs', name)); } catch (logError) { console.error(logError.message); }
    }
    throw error;
  } finally {
    for (const args of owned.reverse()) {
      try { await run(...args); } catch (error) { console.error(`cleanup ${args.join(' ')}: ${error.message}`); }
    }
  }
}

if (process.argv[2] === '--wled-emulator') {
  await emulateWled();
} else {
  assert.ok(process.argv[2], 'usage: node scripts/tests/docker-smoke.mjs IMAGE');
  await smoke(process.argv[2]);
}
