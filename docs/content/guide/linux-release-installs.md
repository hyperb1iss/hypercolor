+++
title = "How Linux release installs are managed"
description = "Where the Linux release installer puts releases and update state, how it adopts older installs, what uninstall removes and keeps, and which directory states it refuses."
weight = 35
+++

The [prebuilt one-liner](@/guide/choose-your-install.md#prebuilt-linux) installs
Hypercolor per user, without `sudo`. This page explains where that install
lives, how upgrades and recovery stay safe, and why the installer sometimes
refuses to continue. It applies to the Linux release tarball only; `.deb`,
AUR, Nix and source installs are managed by their own tools and the installer
leaves them alone.

## Where the install lives

The first install records its locations once and every later run follows that
record.

| What | Default location | Notes |
| --- | --- | --- |
| Releases | `${XDG_DATA_HOME:-~/.local/share}/hypercolor/releases` | The running release and the one before it, each in its own read-only directory, plus an `active` link to the running one |
| Update state | `${XDG_STATE_HOME:-~/.local/state}/hypercolor/update` | Transaction journal, lock and installation record, private to you |
| Locator | `~/.local/lib/hypercolor/install-journal.json` | Points every installer at the recorded locations |
| Upgrade target | `~/.local/lib/hypercolor/managed-adoption.json` | Written once an upgrade from an older install has proven its locations, before it copies or prepares anything there |
| Commands | `~/.local/bin/hypercolor` and friends | Links into the active release |
| Service | `~/.config/systemd/user/hypercolor.service` | Generated user unit that starts the active release |
| Your data | `${XDG_DATA_HOME:-~/.local/share}/hypercolor` | Scenes, layouts, credentials, user effects |
| Your runtime state | `${XDG_STATE_HOME:-~/.local/state}/hypercolor` | Daemon session and device identity, beside `update` |
| Your configuration | `${XDG_CONFIG_HOME:-~/.config}/hypercolor` | Daemon and CLI configuration |

Every install ends by removing releases nothing needs any more. It keeps the
running release, the release it replaced, and after a rollback the release
that failed (the next install checks its own starting point against that
record). Older releases go, along with anything an interrupted copy or
removal left behind.

The installer sets exact permissions on every directory it creates, whatever
your umask is: `0755` for the data and releases directories, and `0700` for the
state and configuration directories. It never changes the permissions of a
directory that already exists.

### The recorded locations win

`XDG_DATA_HOME`, `XDG_STATE_HOME` and `XDG_CONFIG_HOME` are read once, when an
install is first set up. Upgrades, recovery after an interruption and uninstall
all follow the recorded locations, even if those variables change later. To
move an install, uninstall it and install again with the new values.

Each recorded location must be an absolute path of at most 512 bytes, and your
home directory at most 256 bytes. Both may contain only letters, digits, `/`,
`.`, `_` and `-`, because they are written verbatim into the generated systemd
unit. Other paths are refused before anything is written.

## How the service starts

The generated `hypercolor.service` names one release's directory: its daemon,
and the UI and bundled effects of that same release. Every install writes a
new unit for the release it installs, as part of the same transaction, and a
rollback puts the previous unit back byte for byte. Switching the `active`
link never changes what the service starts, so a restart can never run one
release's code with another's UI or effects. The unit's first line names the
service contract it was written under, so an installer never misjudges a unit
written by a build it does not know.

The service runs sandboxed. The whole system is read-only to it except your
configuration, data and state directories (the recorded ones above), and it
gets a private `/tmp`. The releases directory and the update state stay
read-only even though they sit inside those directories, and `~/.local/bin`
and `~/.local/lib` are never writable. The unit sets the daemon's XDG
directories to the recorded roots, and caches go to
`${XDG_STATE_HOME:-~/.local/state}/hypercolor/cache`. Before the sandbox is
built, the service recreates the configuration directory if you deleted it,
so removing `~/.config/hypercolor` to reset your settings still lets the
daemon start. The data and state directories hold the releases and the update
records, so deleting either one removes the installation; install again after
that.

## Upgrades from older installs

Releases before this layout kept everything under `~/.local/lib/hypercolor`.
The next install adopts that setup in place:

1. Any interrupted transaction from the old installer is finished first, with
   the old rules.
2. The running release is copied into the new releases directory and verified.
   The old directory and the releases in it stay where they are.
3. The new transaction is written to the update state, and only then does the
   locator switch to the new layout. That switch is the single commit point.
4. The service is moved to the new release and proven healthy. If that fails,
   the original service, launcher, command links and running daemon are
   restored exactly.

If the process dies at any step, run the installer again. Before the locator
switches, the old install is still the one in charge and nothing visible has
changed. The rerun keeps the locations recorded when the upgrade started, even
if your `XDG_*` variables changed since, and prepares the upgrade again if the
daemon restarted, a different release is being installed, or an older
installer touched the old install in the meantime. After the locator switches,
recovery always continues in the new layout.

Once an install has switched, older copies of the installer stop before they
change the service, launcher, command links or locator, instead of managing a
second, independent copy. Use a current installer from then on.

## A new release must stay up before it commits

After the installer starts a new release and proves it healthy (the service
runs that release's own daemon, and the daemon's local API answers with the
release's version), it keeps watching the service for 90 seconds before it
commits the upgrade. If the daemon crashes, restarts, stops, or is killed by
its watchdog in that window, the upgrade rolls back at once and the previous
release runs again. At the end of the window the installer checks the daemon
and its API again, which catches a daemon that still runs but no longer
answers. The watch follows systemd's own change notifications; it does not
poll the daemon.

After a rollback the installer names the release that runs again, with its
process ID and systemd invocation, so you can match it against
`systemctl --user status hypercolor`.

If the installer itself is interrupted during the window, run it again: the
new release is watched for a whole window again and then committed. If the
machine loses power during the window, the new release is rolled back,
because the release systemd starts at boot is not the one that was being
watched.

## When an upgrade is interrupted

Run the installer again. It finishes the interrupted upgrade or rolls it back
before it does anything else, and it handles what the system did in the
meantime:

- **A start or stop still in progress.** The installer first waits for any
  start or stop systemd already has queued or running for the service. Waits
  follow the service's own `TimeoutStartSec` and `TimeoutStopSec`, up to three
  minutes each, so a slow start is never mistaken for a failure.
- **A service systemd started on its own.** After a power loss or a logout,
  systemd starts the service again at the next login or boot, from the
  release the installer last switched to. When the upgrade expects the
  service stopped at that point, the installer proves the running service is
  exactly that installed release and stops it, then continues. It never stops
  a process it cannot identify.
- **A new release that fails after it started.** A new release that restarts,
  keeps crashing, stops, or fails before it proves itself healthy is rolled
  back, and the previous release runs again. If it crashed often enough to hit
  systemd's start limit, the failure is cleared before the previous release
  starts.
- **An upgrade that never got going.** If the running release restarted
  before the upgrade stopped it, the upgrade can no longer prove what it was
  about to stop. It ends without changing anything and says so; run the
  installer again to upgrade.

## What a release says about your data

Every Linux release tarball ships `share/hypercolor/durable-stores.json`, a
list of every durable store the release reads or writes: your configuration,
scenes, layouts, library, device settings and the rest. For each store it
names the schema the release writes and the oldest and newest it can read.
Stores without a version field on disk declare schema `0`, their only shape so
far. The values come from each store's own code, and a test keeps the list
equal to it.

The installer does not read this file and never refuses a release without it;
older releases do not have one. It is there for tools that decide whether one
release can take over another's data.

## Directory permissions and private groups

Another account must not be able to change the directories the installer
trusts. The rules are:

- Directories the installer owns (releases, update state and
  `~/.local/lib/hypercolor`) must be owned by you and writable only by you.
- Directories it does not own (your home, `~/.local`, `~/.local/share`,
  `~/.local/state`, `~/.config`, and the data, state and configuration
  directories the daemon shares) must be owned by you and not writable by
  everyone. They may also be writable by their group, but only when that group
  is your private group.
- Every existing directory above those, up to `/`, must be owned by root and
  writable by nobody else, or owned by you under the same rules. The service
  reaches the daemon by path, so another account must not be able to rename
  anything on the way.
- The same rule covers your home directory and the public directories the
  installer writes into, such as `~/.local/bin` and `~/.config/systemd/user`,
  because systemd and your shell run what they contain.

Many distributions give each user a private group and a umask of `002`, so
`~/.local` and the daemon's own directories often end up as `0775`. That is
accepted when the installer can prove the group is private:

- it is your primary group;
- the group lists no member other than you;
- no other account uses it as its primary group;
- the system user and group databases can be listed completely, and the
  listing includes you and the group;
- the directory carries no extended access ACL.

The check reads the user and group databases through `getent`. It trusts the
listing only when `/etc/nsswitch.conf` uses `files`, `systemd` or `altfiles`
for users and groups. Directory services such as SSSD, LDAP, winbind or NIS can
hide accounts from a listing, so they refuse the group exception.

## What refuses, and why

| Refusal | Why | What to do |
| --- | --- | --- |
| Directory is group-writable and the group is not proven private | Other group members could replace installed files | `chmod g-w` the named directory |
| Directory is writable by everyone, or sticky like `/tmp` | Any account could replace installed files | Use a directory only you can write |
| A directory above a location, or a public directory such as `~/.local/bin`, is writable by another account | That account could rename the path the service runs from or replace a command | Fix its permissions |
| Directory is owned by another account | That account controls what the installer trusts | Choose a location you own, before one is recorded |
| Group-writable directory carries an extended ACL | An ACL entry can grant another account write access | `chmod g-w` it, or remove the ACL with `setfacl -b` |
| Group lookup fails, or a directory service is configured | The installer cannot prove no one else is in the group | `chmod g-w` the named directory |
| Location or home path is too long | The transaction record could not carry it | Use shorter locations |
| Location or home path has other characters | systemd would read the generated unit differently | Use a path of letters, digits, `/`, `.`, `_` and `-` |
| `XDG_RUNTIME_DIR` has no `systemd/private` socket you own | The installer drives the service through your user manager's private socket, never the session bus | Run it in your own login session, or keep the user manager running with `loginctl enable-linger` |
| The service is still starting, stopping or restarting after its own timeout | A service that keeps changing state is not a safe starting point | Wait for it to settle, or stop it with `systemctl --user stop hypercolor.service`, then rerun |
| Locator from an unknown or newer installer | Guessing would risk managing the wrong install | Use the current installer |
| An XDG base directory would put a writable root at your home, `~/.local/bin` or `~/.local/lib`, or one inside another | The service sandbox could not keep the daemon out of your commands and libraries | Point that variable at another directory and rerun |
| A pending install was prepared under a service contract this installer does not know | It cannot check a unit another build wrote | Finish it with the installer that started it |
| Uninstall finds a service, unit or link this installer did not generate | It belongs to a package, another install or a local edit | Remove it with its owner, then rerun |
| Another install or uninstall is running | Two writers would corrupt the journal | Wait for it to finish |

Every refusal happens before the service, launcher or command links change.

Choosing another location only helps before one is recorded. After the first
install, or once an upgrade has written its target, the installer ignores new
`XDG_*` values, so fix the named directory instead.

## Uninstall

```bash
curl -fsSL https://raw.githubusercontent.com/hyperb1iss/hypercolor/main/scripts/install-release.sh \
  | bash -s -- --uninstall
```

The script runs the installed `hypercolor __uninstall-release`, which follows
the recorded locations:

- it finishes any interrupted transaction first, and if that cannot finish it
  says so and removes the install anyway;
- it stops and disables the service and removes the generated unit;
- it removes only the command links, desktop entry, icons and completions this
  installer generated;
- it removes the releases directory, the update state and
  `~/.local/lib/hypercolor`, including locations an interrupted upgrade had
  prepared.

It keeps your data directory (everything except `releases`), your runtime
state beside `update`, and your configuration directory. If the service,
launcher or a link was not generated by this installer, uninstall stops
without changing anything and names what it found. An interrupted uninstall
continues where it stopped when you run it again. The empty `~/.hypercolor-release-install.lock` bootstrap lock file is
left in your home directory.
