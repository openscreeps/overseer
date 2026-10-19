# overseer CLI

The `overseer` command uploads built Screeps code to the branches of Screeps servers. It works
with the official server, the seasonal servers and private servers. A configuration names the
servers and the profiles that upload to them. One command uploads a build to one profile or to
many profiles.

The CLI reads a build directory that any tool makes (Rollup, esbuild, `wasm-pack`, ...). Thus the
build needs no plugin, and the upload needs no Node.js.

`overseer poll` keeps a spawn of the account in the world. It places the first spawn where the
selector of the bot chooses. It resets an account only when the bot proves that nothing of the
account is left.

Before version 0.3.0, the CLI was the separate tool `screepsmanager`. To change a bot project,
see [Migrate from screepsmanager](#migrate-from-screepsmanager).

## Install

The CLI has its own releases. The tags of these releases have the prefix `cli-v`, for example
`cli-v0.3.0`. Each release has one archive for each platform:

- Linux x86_64 and aarch64 (glibc 2.35 or newer): `overseer-cli-v<version>-<target>.tar.gz`
- macOS x86_64 and arm64: `overseer-cli-v<version>-<target>.tar.gz`
- Windows x86_64: `overseer-cli-v<version>-x86_64-pc-windows-msvc.zip`

Each archive contains the `overseer` binary (`overseer.exe` on Windows), this README, and the
license files `LICENSE-MIT` and `LICENSE-APACHE`. A `.sha256` file is attached next to each
archive.

### With mise

Use the [mise GitHub backend](https://mise.jdx.dev/dev-tools/backends/github.html). Set the
`version_prefix` tool option to `cli-v`. mise then lists only the tags that start with `cli-v`,
and shows the tag `cli-v0.3.0` as the version `0.3.0`. The plain `v*` tags are for the desktop
app, and mise ignores them.

Install the latest CLI release for your user:

```sh
mise use -g 'github:openscreeps/overseer[version_prefix=cli-v]'
```

Install one version:

```sh
mise use -g 'github:openscreeps/overseer[version_prefix=cli-v]@0.3.0'
```

Quote the argument, so that the shell does not expand the brackets. mise writes this entry into
`mise.toml`. You can also write it yourself. For example, a bot project can pin the CLI and add
an upload task next to its build:

```toml
[tools]
"github:openscreeps/overseer" = { version = "0.3.0", version_prefix = "cli-v" }

[tasks.deploy]
run = "overseer upload"
```

Do not omit `version_prefix`. Without it, `latest` can select a desktop release.

mise selects the archive for your OS, CPU architecture and C library from the asset names. You
do not need `asset_pattern`. mise checks the GitHub artifact attestation of the archive by
default. `latest` does not select a pre-release (for example `cli-v0.3.0-rc1`). To include
pre-releases, add `prerelease=true` to the options.

### With Cargo, from Git

You need Rust 1.99 or newer.

```sh
cargo install --locked --git https://github.com/openscreeps/overseer --tag cli-v0.3.0 overseer-cli
```

### From a clone

rustup selects the toolchain from `rust-toolchain.toml` in the clone.

```sh
git clone https://github.com/openscreeps/overseer
cd overseer
cargo install --locked --path crates/overseer-cli
```

### Verify a download

CI signs build provenance for each archive. Download an archive, then verify it with the
[GitHub CLI](https://cli.github.com/):

```sh
gh release download cli-v0.3.0 --repo openscreeps/overseer --pattern 'overseer-cli-v0.3.0-x86_64-unknown-linux-gnu.tar.gz*'
gh attestation verify overseer-cli-v0.3.0-x86_64-unknown-linux-gnu.tar.gz --repo openscreeps/overseer
sha256sum -c overseer-cli-v0.3.0-x86_64-unknown-linux-gnu.tar.gz.sha256
```

On macOS, use `shasum -a 256 -c` in place of `sha256sum -c`.

## Usage

```sh
overseer servers                           # list the saved servers (shared with the desktop app)
overseer servers add http://127.0.0.1:21025 --name local   # save a server
overseer login local --email me@example.com               # sign in; the password comes from stdin
overseer login screeps.com --token         # sign in with an API token from stdin
overseer logout local                      # delete the saved sign-in
overseer profiles                          # list the profiles of the configuration
overseer upload                            # upload to the default profiles
overseer upload -p main                    # upload to one profile
overseer upload -p main,season             # upload to many profiles, in this order
overseer upload -p dev --branch auto       # upload to the current git branch
overseer upload -p main --activate world   # also make the branch run in the world
overseer upload --dry-run                  # check everything, but send nothing to the server
overseer poll --once                       # read the world one time and print the plan
overseer poll -p main                      # read the world at each interval and print the plan
overseer poll -p main --execute            # also place the spawn, or reset, when the checks pass
overseer --env-file .env upload -p main    # read the secrets from a dotenv file
```

`upload`, `poll` and `profiles` read the configuration file `overseer.toml`. The CLI looks for
this file in the working directory, then in each parent directory. To use another file, give
`--config FILE` (`-c FILE`) or set `OVERSEER_CONFIG`. `servers`, `login` and `logout` need no
configuration file. See [Saved servers and sign-ins](#saved-servers-and-sign-ins).

`upload [DIR]` reads the modules of the build directory one time. The default build directory is
the `dir` of the configuration. Then, for each profile, `upload` replaces every module of the
branch of the profile. If the account does not have the branch, `upload` creates it. Then
`upload` makes the branch run where the profile and `--activate` tell. `--branch` (`-b`) sets
the branch of every profile.

Without `--profile` (`-p`) or `OVERSEER_PROFILE`, `upload` uses the `default` profiles of the
configuration. If the configuration has only one profile, `upload` uses that profile. The
profiles of one account share one sign-in.

The first failure stops the upload. The CLI writes the error to stderr as `overseer: ...` and
exits with status 1. A bad command line gives status 2.

`--env-file FILE` (or `OVERSEER_ENV_FILE`) reads a dotenv file for the `{ env = "..." }` secrets
of every command. The file can have `NAME=value` lines, quotes, `export`, comments and `${VAR}`.
The CLI reads only the given file, relative to the working directory. It does not search for a
file.

A variable of the process environment wins over the file. In the file, the first definition
wins. A missing or malformed file is an error. The error names the file and the line, but never
a value or the text of a line. The CLI does not export the variables, so the selector does not
see them.

The first `overseer upload -p main,season` with the configuration below prints:

```text
config /home/me/bot/overseer.toml
build /home/me/bot/dist: 3 modules
  main         js                 60 bytes  main.js
  main.js.map  source map         69 bytes  main.js.map
  main_bg      wasm                8 bytes  main_bg.wasm
  skipped assets: not a file
  skipped index.html: not a module file (.js, .js.map or .wasm)
profile main: branch main on https://screeps.com/, run in world
  created branch main
  uploaded 3 modules
  branch main runs in world
profile season: branch default on https://screeps.com/season/
  uploaded 3 modules
```

## Configuration

```toml
dir = "dist"         # the build directory, relative to this file (default: dist)
default = ["main"]   # the profiles of `upload` without --profile

[servers.official]
url = "https://screeps.com"
token = { env = "SCREEPS_TOKEN" }

[servers.season]
url = "https://screeps.com/season"
token = { env = "SCREEPS_TOKEN" }

[servers.local]
url = "http://127.0.0.1:21025"
email = "me@example.com"
password = { env = "SCREEPS_LOCAL_PASSWORD" }

[profiles.main]
server = "official"
branch = "main"
activate = ["world"]

[profiles.sim]
server = "official"
branch = "sim"
activate = ["sim"]

[profiles.season]
server = "season"

[profiles.dev]
server = "local"
branch = "auto"
activate = ["world"]

[profiles.main.spawn]
selector = ["node", "scripts/select-spawn.mjs"]
shard = "auto"
```

| Key                              | Meaning                                                                               |
| -------------------------------- | ------------------------------------------------------------------------------------- |
| `dir`                            | The build directory, relative to the file. Default `dist`                             |
| `default`                        | The profiles of `upload` without `--profile`. Default: the only profile               |
| `servers.NAME.url`               | The URL of the API: `http` or `https`. Add `/season` for the seasonal server          |
| `servers.NAME.token`             | The API token (official server: account settings, auth tokens)                        |
| `servers.NAME.email`             | The account email, with `password`, for a private server with screepsmod-auth         |
| `servers.NAME.password`          | The account password                                                                  |
| `profiles.NAME.server`           | A `[servers.NAME]` table, or else the name or the URL of a saved server               |
| `profiles.NAME.branch`           | The branch of the upload. `auto` is the current git branch. Default `default`         |
| `profiles.NAME.activate`         | Where the upload makes the branch run: `world`, `sim`. Default: nowhere               |
| `profiles.NAME.spawn.selector`   | The program and the arguments of the selector (no shell). It runs next to the file    |
| `profiles.NAME.spawn.shard`      | `auto` (the one shard with CPU), a shard name, or no value on a server without shards |
| `profiles.NAME.spawn.interval`   | The seconds between two polls, 60 to 86400. Default 60                                |
| `profiles.NAME.spawn.radius`     | The distance from the start room to the candidates, 1 to 10 rooms. Default 5          |
| `profiles.NAME.spawn.candidates` | The maximum number of candidates for the selector, 1 to 64. Default 16                |

A server signs in with `token`, with `email` and `password`, or with the saved sign-in of its
URL (no credential keys). A secret is a string, or `{ env = "VARIABLE" }`. The second form reads
the secret from the environment (or the `--env-file`) when a command needs it. Thus you can
commit the configuration. Server and profile names have letters, digits, `-`, `_` and `.`. A key
that the format does not have is an error.

### The server of a profile

The CLI finds the server of `profiles.NAME.server = "X"` in this order:

1. The table `[servers.X]` of the file. If the table has `token`, or `email` and `password`, the
   CLI uses them. Else it uses the saved sign-in of the URL of the table.
2. The saved server with the name `X`. If two saved servers have this name, the CLI stops with
   an error that lists their URLs. Use the URL in place of the name.
3. The saved server whose URL is `X` (after normalization, for example `127.0.0.1:21025` is
   `http://127.0.0.1:21025/`).

The credentials of the file always win over a saved sign-in. Thus a CI job with
`token = { env = "SCREEPS_TOKEN" }` works as before and does not touch the credential store. The
server password of a saved private server comes from the saved secrets.

A server without credentials and without a saved sign-in is not an error for `profiles`. A
command that connects stops with an error that names `overseer login`. `profiles` shows the
source of each sign-in in the `AUTH` column: the file (`token in the file`, `token from $VAR`,
`EMAIL, password ...`), `saved API token`, `saved session token`, or `none`. It shows no secret.

A configuration that uses only saved servers needs no `[servers]` table:

```toml
[profiles.main]
server = "screeps.com"
branch = "main"

[profiles.dev]
server = "local"      # a saved server: overseer servers add http://127.0.0.1:21025 --name local
branch = "auto"
```

### Credentials and safety

- Keep secrets out of the configuration. Use `{ env = "..." }` with the environment or
  `--env-file`, or sign in with `overseer login`. Keep the dotenv file out of version control.
  Errors name the variable, the file and the line, but never a secret.
- The CLI never reads a secret from the command line. `login` reads it from stdin.
- The token goes only to the configured server, because the client follows no redirect. When the
  server sends a new `X-Token`, the client uses it for the next calls of the process. With a
  saved token, the CLI also saves each new token.
- The CLI does not repeat a failed call. The Overseer library sends a request again only after
  HTTP 401 (with a new sign-in) or HTTP 429. The first failure of `upload` stops it. `poll` never
  repeats a call whose outcome is unknown.
- `profiles`, `servers` and `upload --dry-run` send nothing. `poll` only reads. Without
  `--execute`, `poll` refuses every call that changes the world. `upload` replaces the modules of
  the branch of the profile.
- Try a configuration against a private server (`http://127.0.0.1:21025`) before you use it with
  an account on the official server.

## Saved servers and sign-ins

The CLI and the Overseer desktop app share one list of saved servers and one sign-in for each
server. Sign in one time, in the app or with `overseer login`, and both use the sign-in. The web
app in a browser keeps its own list.

### Commands

- `overseer servers` lists the saved servers: the name, the URL, the kind (`official`, `season`,
  `private`), the signed-in account, the sign-in kind (`API token`, `session token`, `token` or
  `none`) and whether a server password is saved. It shows no secret.
- `overseer servers add URL [--name NAME]` saves a server. Without a scheme, the CLI uses
  `http://`. The default name is `Season` for `https://screeps.com/season/`, else the host, the
  port and the path of the URL.
- `overseer servers remove NAME|URL` removes a saved server, its token and its server password.
  The CLI cannot remove screeps.com.
- `overseer login NAME|URL --token` signs in with an API token. The CLI checks the token with a
  connection and saves it.
- `overseer login NAME|URL --email EMAIL` signs in with the email (or the username) and the
  password. Where the server has API tokens, the CLI creates a full API token and saves it. Else,
  or when the server does not create the token (for example HTTP 404), it saves the session
  token and tells why. The CLI never saves the password.
- `--server-password` also reads the server password of a password-protected private server and
  saves it.
- `overseer logout NAME|URL` deletes the saved token and forgets the account. With `--revoke`,
  it first deletes the API token on the server. If the CLI cannot find the token in the token
  list of the server, it says so and signs out all the same. If the server call fails, the CLI
  keeps the sign-in: run `logout` again without `--revoke` to sign out of the CLI only.

`login` saves a URL that is not saved yet (an argument with `://`, a `.`, a `:` or a `/`). An
unknown name without these characters is an error, so a typing error does not save a server.

`login` reads each secret from stdin. On a terminal, it asks for the secret and does not show
what you type. Else it reads one line for each secret: first the token or the password, then the
server password. For example:

```sh
printf '%s\n' "$SCREEPS_PASSWORD" | overseer login local --email me@example.com
```

**Caution: a session token expires.** A private server drops a session token 60 s after it
issued it, and replaces it on each reply. While a session of the CLI runs, the CLI saves each new
token, so the next run finds a valid token. After more than a minute without a session, the
saved session token is not valid. The next command then stops with an error that names
`overseer login`. Use an API token where the server has them. The desktop app and the CLI can
also replace the session token of each other.

### Where the CLI keeps them

The saved servers are in the file `profiles.json` in the configuration directory of the desktop
app (identifier `com.openscreeps.overseer`):

| OS      | File                                                                        |
| ------- | --------------------------------------------------------------------------- |
| Linux   | `$XDG_CONFIG_HOME/com.openscreeps.overseer/profiles.json` (`~/.config/...`) |
| macOS   | `~/Library/Application Support/com.openscreeps.overseer/profiles.json`      |
| Windows | `%APPDATA%\com.openscreeps.overseer\profiles.json`                          |

The file has no secrets. The tokens and the server passwords are in the OS credential store
(Windows Credential Manager, macOS Keychain, the Secret Service on Linux), under the service
`overseer`. The keys are `token:URL` and `server-password:URL`. Each change of the file occurs
under a lock, so the CLI and the app keep the changes of each other.

`OVERSEER_APP_IDENTIFIER` selects the storage of another app identifier: the directory with that
name and the credential service `overseer:IDENTIFIER`. Tests and other builds use it. An
identifier has ASCII letters, digits, `.`, `-` and `_`. The variable does not select another kind
of store.

## Spawning

`poll` polls each profile that has a spawn table. With `--profile`, it polls only the given
profiles.

Profiles of one server can target different explicit shard names, for example `shard3` and
`shard0`. Profiles of one server cannot share one poller when their targets overlap, when one
target is `auto`, or when a target has no shard. Only the first selected profile of each server
manages the account-wide reset. The other profiles of that server manage only the placement.

Each profile signs in one time in each process. Then it reads the account (`auth/me`), the
shards and the world status.

**Caution: the official `user/world-status` is account-wide, also with a shard query.** Thus
`normal` on shard3 does not tell whether shard0 has a colony.

For a named target shard (also the shard that `auto` selects), these rules apply:

- `poll` needs a non-empty account `_id` from the sign-in. It reads
  `user/rooms?id=...&reservation=`.
- The map of owned rooms and the map of reserved rooms must both contain the target shard.
  Missing or malformed evidence fails closed.
- An owned or reserved room on the target shard stops the placement.
- When both lists of the target shard are empty, an account-wide **normal** or **empty** status
  permits one limited first-spawn attempt on that shard. This attempt does not reset another
  shard. For example, `shard = "shard0"` can try a placement while shard3 owns `W8S2`.

On a private server without shards, the world status must be **empty**. There, `auth/me` does
not have to give an official account ID.

After the respawn cooldown of 180 s of the server, `poll` reads the candidates. A candidate is a
room that:

- is within `radius` of the start room of the target (the nearest rooms come first),
- can have a controller (not a highway, not a sector center),
- is not prohibited,
- is `normal`, not owned and not reserved on the map (`map-stats`),
- has one neutral controller that is not reserved, no keeper lair, no invader core and no object
  of another owner.

The selector gets the candidates on stdin and writes its answer to stdout. `poll` makes sure
that the answer names a candidate and a tile. The tile must be inside the edge, not on a wall,
not next to an exit, and free of objects.

Before `poll` places `Spawn1`, it reads these items again: the CPU, the cooldown, the active
branch, the prohibited rooms, the room status and the room objects. It also checks the tile
again. The last read revalidates the owned and reserved lists of the target and the signed-in
identity. On a server without shards, it revalidates the empty world status.

**Caution: these room lists do not prove that the target has no creeps or other objects.** The
server makes the final decision about the placement. A refusal is not permission to reset.
`poll` does not allocate CPU. But when a placement succeeds, the initial spawn rules of the server
can change the CPU allocation. An account-wide **lost** status follows the careful reset path
below, not the placement path for one shard.

Without `--execute`, the execution gate stops all reset and placement calls. `poll` then prints
`would place ...` or `would reset ...`.

With `--execute`, Overseer's `Error::outcome()` tells whether a failed mutation was not applied
or has an unknown outcome. Only "not applied" (`Outcome::NotApplied`) avoids the lockout. The CLI
never uses the HTTP status or the message of the server to decide. A reply of the server, for
example an authentication error or a logical error, is not proof that the mutation was not
applied.

An unknown placement outcome stops all next placement attempts of that profile in this process.
Empty controller lists do not clear this lockout, and a new connection does not clear it.
Examine the world before you start `poll` again.

An unknown reset outcome starts the local respawn cooldown, the same as a confirmed reset. A
plan that placed nothing stays for 10 minutes before `poll` reads the candidates again. The
server permits one `map-stats` call in each minute.

`poll` changes no CPU, no code and no branch. The configured branch must already run in the
world, and the account must have CPU on the shard.

An API failure is an error, never an empty or dead account. `--once` exits with status 1 on the
first failure. Without `--once`, `poll` prints the failure and continues. The client follows no
redirect, so the token goes only to the configured server.

### Selector protocol

stdin gets one JSON record:

```json
{
  "version": 1,
  "shard": "shard3",
  "rooms": [
    {
      "name": "W1N1",
      "terrain": "<2500 digits 0..3, row by row>",
      "objects": [],
      "status": {}
    }
  ]
}
```

`shard` is `null` on a server without shards. `objects` and `status` are the same as in the
`room-objects` and `room-status` replies of the server.

The selector writes one JSON record to stdout and nothing else. It writes diagnostics to stderr,
and `poll` prints them.

```json
{"version": 1, "room": "W1N1", "x": 25, "y": 25, "reason": "..."}
{"version": 1, "room": null, "reason": "no suitable room"}
```

The selector runs for a maximum of 60 s. It writes a maximum of 64 KiB to each stream. A
failure, a timeout or an answer that does not obey the protocol is an error.

### Reset of a lost account

`lost` means no spawn, not no creeps. Thus `poll` resets the account (`user/respawn`) only when
the bot proves that nothing of the account is left. The bot writes this heartbeat in each tick:

```js
Memory.__overseer = {
  version: 1,
  tick: Game.time,
  shard: Game.shard.name,
  rooms: 0,
  creeps: 0,
  spawns: 0,
  sites: 0,
  powerCreeps: 0,
};
```

The counts are the footholds of the bot in the world: owned controllers, creeps, spawns,
construction sites and power creeps.

A reset needs all these conditions:

- On each lost shard, the heartbeat has version 1 and the name of that shard.
- The heartbeat is a maximum of 3 ticks behind `game/time`.
- Each count of the heartbeat is 0.
- These conditions stay true for 180 s without a break.
- No shard is `normal`.
- `poll` reads all of it again immediately before the reset.
- For named shards, the maps of owned and reserved rooms are complete for each listed shard, at
  the start and immediately before the reset. An owned or reserved room on any shard stops the
  account-wide reset.

A missing, old or malformed heartbeat stops the reset and starts the 180 s again. Missing room
evidence and a failed call do the same. `--once` never resets. After a reset, the empty account
gets its spawn after the cooldown.

**Limitation**: on the official server, the world status is of the whole account. Thus a lost
account is lost on each shard, and each shard needs a fresh heartbeat. A shard without CPU runs
no code, so it writes no heartbeat. Thus `poll` never resets an account with CPU on only one of
many shards. `poll` prints the reason. Reset such an account manually.

Private servers with a status for each shard keep the same reset checks on all shards. A
placement on an eligible target needs no heartbeat. But its room-list evidence cannot prove that
the account has no other objects.

## Modules

`overseer::code::read_dir` of the `overseer` library sets the rules for the build directory. A
file directly in the build directory is a module when its name ends in one of these suffixes:

| Suffix    | Module                               | Uploaded as                                       |
| --------- | ------------------------------------ | ------------------------------------------------- |
| `.js`     | the name without `.js` (`main`)      | text                                              |
| `.js.map` | the whole name (`main.js.map`)       | `module.exports = MAP;`, or the text as it is     |
| `.wasm`   | the name without `.wasm` (`main_bg`) | `{"binary": BASE64}`                              |

Thus `require("main.js.map")` gives the source map. A source map text that starts with `{` (a
JSON map) becomes `module.exports = MAP;`. Other text stays as it is. The suffixes are
case-sensitive.

The server drops a module whose name is empty or starts with `$` or `.`. Thus the CLI skips such
a file, for example `.eslintrc.js`. The CLI also skips every other entry. The report of `upload`
shows each skipped entry.

Two files of one module are an error, for example `main.js` and `main.wasm`. A directory without
a module is also an error, because an upload replaces every module of a branch.

## Migrate from screepsmanager

Before version 0.3.0, the CLI was the separate tool `screepsmanager`. The CLI does not read the
old names. To migrate a bot project, do these steps:

1. Rename `screepsmanager.toml` to `overseer.toml`. The format of the file does not change.
2. Rename the environment variables:
   - `SCREEPSMANAGER_CONFIG` to `OVERSEER_CONFIG`
   - `SCREEPSMANAGER_ENV_FILE` to `OVERSEER_ENV_FILE`
   - `SCREEPSMANAGER_PROFILE` to `OVERSEER_PROFILE`
3. Caution: `poll` does not read `Memory.__screepsmanager`. If the bot writes only the old key,
   `poll` finds no heartbeat and never resets the account. Change the bot to write the heartbeat
   to `Memory.__overseer`.
4. Change the mise pin. Replace `"github:openscreeps/screepsmanager" = "0.2.0"` with
   `"github:openscreeps/overseer" = { version = "0.3.0", version_prefix = "cli-v" }`.
5. Change the command in scripts and tasks from `screepsmanager` to `overseer`. Errors now start
   with `overseer:`.
6. Optional: on a workstation, replace the credentials of a `[servers.NAME]` table with a saved
   sign-in. Run `overseer login URL --token` (or `--email EMAIL`), then remove `token`, `email`
   and `password` from the table. Keep the `{ env = "..." }` credentials in CI: they win over a
   saved sign-in, and CI needs no credential store.

## Tests

- Run the unit tests with `cargo test -p overseer-cli`. They keep the saved servers in a
  temporary directory and the secrets in memory, never in the credential store of the user.
- `cargo xtask e2e` also runs the live scenarios of the CLI: the test target `live` of this
  crate. They run the CLI against a disposable world in Docker. The scenario of the saved sign-in
  uses a unique `OVERSEER_APP_IDENTIFIER` and removes its directory and its secrets afterwards.
  Without an OS credential store (for example Linux CI without a Secret Service), it skips with a
  message.

## Release

The CLI has its own version in `crates/overseer-cli/Cargo.toml`. It does not use the workspace
version. CLI release tags have the name `cli-v<version>`. Plain `v*` tags are for the desktop
app.

To cut a CLI release:

1. Change `version` in `crates/overseer-cli/Cargo.toml`.
2. Run `cargo check -p overseer-cli` to update `Cargo.lock`.
3. Commit the change and merge it to `main`.
4. Make a tag with the name `cli-v<version>` on that commit, for example `git tag cli-v0.3.0`.
5. Push the tag: `git push origin cli-v0.3.0`.

The tag push starts CI (`.github/workflows/ci.yml`):

- The `build` job makes sure that the tag matches the version in
  `crates/overseer-cli/Cargo.toml`. If the tag does not match, the job fails.
- The `build` job builds one archive for each of the 5 targets, signs build provenance, and
  uploads the archives as workflow artifacts.
- The other jobs (`check`, `e2e`, `cli`) run the tests.

When all CI jobs pass, `.github/workflows/release.yml` starts. It creates a draft release for the
tag, uploads the archives, and then publishes the release. If a CI job fails, no release occurs.

A tag with a pre-release suffix, for example `cli-v0.3.0-rc1`, becomes a GitHub pre-release. mise
does not select a pre-release for `latest`.

To test the builds without a tag, start CI manually:

```sh
gh workflow run ci.yml --ref <branch>
```

The `build` job then keeps the archives as workflow artifacts. It does not sign them, and no
release occurs.

If `release.yml` fails after CI passed, run it again from the Actions tab. It uses the draft
release again and replaces the assets.

Caution: GitHub runs `release.yml` only from the default branch. A change to `release.yml` has an
effect only after the merge.

## License

The CLI is part of Overseer. It is licensed under the
[Apache License, Version 2.0](https://github.com/openscreeps/overseer/blob/main/LICENSE-APACHE) or
the [MIT license](https://github.com/openscreeps/overseer/blob/main/LICENSE-MIT), at your option.
