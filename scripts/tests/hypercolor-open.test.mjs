// The opener hands a person their own Hypercolor only. Every installation
// serves on 127.0.0.1:9420, so on a shared Linux box whatever answers
// there may be someone else's, and opening it would show that person's
// lighting and settings.
import assert from 'node:assert/strict'
import { spawnSync } from 'node:child_process'
import { chmodSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync, existsSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { test } from 'node:test'
import { fileURLToPath } from 'node:url'

const opener = fileURLToPath(new URL('../../packaging/bin/hypercolor-open', import.meta.url))
const ME = 1000
const HEADER = '  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode\n'

/** One /proc/net/tcp row: `local` is the kernel's hex address:port. */
const row = (local, uid, state = '0A') =>
  `   0: ${local} 00000000:0000 ${state} 00000000:00000000 00:00000000 00000000  ${uid}        0 4242 1 0000000000000000 100 0 0 10 0\n`
const LOOPBACK = '0100007F:24CC'
const WILDCARD = '00000000:24CC'
const MAPPED_LOOPBACK = '0000000000000000FFFF00000100007F:24CC'
const ANY_V6 = '00000000000000000000000000000000:24CC'

function executable(path, text) {
  writeFileSync(path, text)
  chmodSync(path, 0o755)
}

/**
 * Runs the opener with `tcp` and `tcp6` as the socket tables, stub
 * systemctl, curl, id and getent, and an xdg-open that records the URL.
 * `serviceStarts` says whether this person's own service start succeeds.
 */
function open(t, { tcp = '', tcp6 = '', serviceStarts = true, healthy = true } = {}) {
  const work = mkdtempSync(join(tmpdir(), 'hypercolor-open-'))
  t.after(() => rmSync(work, { recursive: true, force: true }))
  const bin = join(work, 'bin')
  const net = join(work, 'net')
  mkdirSync(bin)
  mkdirSync(net)
  writeFileSync(join(net, 'tcp'), HEADER + tcp)
  writeFileSync(join(net, 'tcp6'), HEADER + tcp6)
  executable(join(bin, 'systemctl'), `#!/bin/sh\nexit ${serviceStarts ? 0 : 1}\n`)
  executable(join(bin, 'curl'), `#!/bin/sh\nexit ${healthy ? 0 : 7}\n`)
  executable(join(bin, 'id'), `#!/bin/sh\necho ${ME}\n`)
  executable(
    join(bin, 'getent'),
    '#!/bin/sh\ncase "$2" in 1000) echo "me:x:1000:1000::/home/me:/bin/sh" ;; 4242) echo "ana:x:4242:4242::/home/ana:/bin/sh" ;; *) exit 2 ;; esac\n',
  )
  executable(join(bin, 'xdg-open'), `#!/bin/sh\nprintf '%s\\n' "$1" >"${join(work, 'opened')}"\n`)
  const result = spawnSync('sh', [opener], {
    env: { PATH: `${bin}:/usr/bin:/bin`, HYPERCOLOR_OPEN_NET_TABLES: net },
    encoding: 'utf8',
    timeout: 30_000,
  })
  const opened = existsSync(join(work, 'opened')) ? readFileSync(join(work, 'opened'), 'utf8').trim() : null
  return { ...result, opened }
}

const HELD_BY_ANA =
  'Hypercolor is already running for ana, and only one installation can run at a time. Ask ana to stop it, then try again.\n'

test("this person's own listener is opened", (t) => {
  for (const tables of [{ tcp: row(LOOPBACK, ME) }, { tcp: row(WILDCARD, ME) }, { tcp6: row(MAPPED_LOOPBACK, ME) }]) {
    const run = open(t, tables)
    assert.equal(run.status, 0, run.stderr)
    assert.equal(run.opened, 'http://127.0.0.1:9420/')
  }
})

test("someone else's listener is never opened, even when it answers health", (t) => {
  // The race the deb launcher's preflight cannot close: nothing listened
  // when it checked, then the other person's Hypercolor bound first, this
  // person's own service start failed, and that Hypercolor answers health.
  for (const tables of [
    { tcp: row(LOOPBACK, 4242) },
    { tcp: row(WILDCARD, 4242) },
    { tcp6: row(MAPPED_LOOPBACK, 4242) },
    { tcp6: row(ANY_V6, 4242) },
  ]) {
    const run = open(t, { ...tables, serviceStarts: false, healthy: true })
    assert.equal(run.status, 75)
    assert.equal(run.opened, null, 'nothing was opened')
    assert.equal(run.stdout, '')
    assert.equal(run.stderr, HELD_BY_ANA)
  }
})

test('an owner the system cannot name is still refused', (t) => {
  const run = open(t, { tcp: row(LOOPBACK, 5555) })
  assert.equal(run.status, 75)
  assert.equal(run.opened, null)
  assert.equal(
    run.stderr,
    'Hypercolor is already running for another user, and only one installation can run at a time. Ask them to stop it, then try again.\n',
  )
})

test('sockets on other ports or in other states do not count', (t) => {
  const run = open(t, { tcp: row('0100007F:24CD', 4242) + row(LOOPBACK, 4242, '01') + row(LOOPBACK, ME) })
  assert.equal(run.status, 0, run.stderr)
  assert.equal(run.opened, 'http://127.0.0.1:9420/')
})

test('with nothing listening yet the page still opens and waits for it', (t) => {
  const run = open(t, { healthy: false })
  assert.equal(run.status, 0, run.stderr)
  assert.equal(run.opened, 'http://127.0.0.1:9420/')
})
