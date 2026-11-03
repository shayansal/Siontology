# Run it on a server

On your computer, the agents work while it is on: they stop when it
sleeps. A server of yours keeps them at work while it is off. `sion agent
up` runs everything in one process and keeps it running, and `sion
deploy` puts sion and your agents on a server over SSH.

## Everything in one process

`sion agent up` runs a list of sion commands, each in a process of its
own, and starts each again when it stops: after a second, then 2, 4… up
to a minute, the pause starting over once it has run steadily for a
minute. What each says goes to `$SION_HOME/logs/<name>.log` (a log past
10 MiB is set aside as `<name>.log.1` and started over, looked at every
half minute), and `$SION_HOME/up.status.json` says
what runs, since when, and how often it was started again. When `sion
agent up` stops, however it stops (Ctrl-C, its service stopped, or killed
outright), what it ran stops too: each command is given a pipe from it,
and stops when the pipe closes.

The list is `$SION_HOME/up.json`:

- `sion agent up init` fills it from what `sion setup` made: the chain
  (if setup made one here), the scheduler, the work runner for the
  profile (`--profile`; with `--webhooks`, taking GitHub's webhooks), and
  a bridge for each messenger setup connected.
- `sion agent up add <name> -- <command>` adds any other sion command
  (`sion agent up add helper -- agent work run helper`, the work runner
  of a second profile), and `remove <name>` takes one off.
- `sion agent up list` shows the list and how each is doing.

To keep it running across restarts, install it as a service of yours:
`sion agent service install up --start -- agent up`.

## On a server of yours

`sion deploy user@host` does it all over SSH (`ssh`, or the program
`SION_SSH` names; `--port` and `--identity` as SSH takes them):

1. It asks the server what it is (`uname -sm`) and where its home is.
2. It copies this `sion` there as `~/.local/bin/sion`, when the server is
   the same kind of machine (Linux on the same processor). Otherwise it
   says to install a release there first (`scripts/install.sh`), then
   deploy again.
3. It sets up an agent of the server's own, running `sion setup` there in
   your terminal; or, with `--copy-home`, copies this machine's
   `$SION_HOME` there (wallets and their keys, profiles, kept
   credentials, the chain's state) and rewrites the paths in it to the
   server's home (`sion agent up rehome`).
4. It fills the server's list (`sion agent up init`, with `--profile` and
   `--webhooks` passed on) and installs `sion agent up` as a service of
   the user's there, started now (unless `--no-start`) and at boot. With
   lingering on (`loginctl enable-linger`), the service runs with no one
   logged in; where the administrator has not allowed it, it says so.

Copying a home moves its keys to the server: whoever controls the server
controls the agents' wallets (`deploy:copied-keys`). Stop the agents on
your computer afterwards, so that one copy answers the messengers and
works the watches.

## In a container

`deploy/compose.yaml` runs `sion agent up` in a container, its home
(wallets and their keys, profiles, the chain's state) in a volume. Its
comments give the steps: the chain added to the list, the container
started, then `sion setup` and `sion agent up init` run in it. The
release workflow builds its image, `ghcr.io/<owner>/sion`, for amd64 and
arm64.

## How this was tested

The tests run `sion agent up` for real: it starts its list, starts again
what stops, with the pause doubling, and what it ran stops when it is
killed outright. `sion deploy` is tested against a stand-in for SSH that
runs the server's commands in a home of their own on the same machine:
sion is copied and runs there, the home is copied with its paths
rewritten, the list is filled and the service's unit written. No service
manager is reached in the tests, and the container image and the compose
file have not been built or run: there is no Docker here, and no release
yet.

## What this does not do yet

- sion is copied only to Linux on the same processor as this one, and
  the service is a systemd user unit or a launchd agent; there is no
  Windows service.
- One server runs the agents: there is no failing over to a second.
