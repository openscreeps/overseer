# Overseer

Overseer is a [Screeps](https://screeps.com/) client written in Rust. It has a desktop app, a
web app and a set of crates that you can use to make your own tools.

The same code works with screeps.com, with OpenScreeps servers and with community private
servers. Overseer finds out what each server can do when it connects.

## Supported servers

| Server | Contract | How Overseer knows it |
|---|---|---|
| screeps.com and its Season server | official | the address (also behind a proxy) |
| [OpenScreeps](https://github.com/openscreeps/server) server | OpenScreeps | `GET /api/version` lists the `openscreeps` feature |
| Community private server (`screeps` with `@screeps/backend`, for example through [screeps-launcher](https://github.com/screepers/screeps-launcher)) | community | any other server |

A community server can run plugins. Overseer finds these plugins:

| Plugin | How Overseer finds it |
|---|---|
| [screepsmod-auth](https://github.com/ScreepsMods/screepsmod-auth) | `GET /api/authmod` answers |
| [screepsmod-admin-utils](https://github.com/ScreepsMods/screepsmod-admin-utils) | `GET /api/version` lists its feature |
| A room-history mod, for example [screepsmod-history](https://github.com/ScreepsMods/screepsmod-history) | `GET /room-history` does not give the stock `not implemented` error |

### Subsystems

Servers do not all have the same features. Overseer names each feature that can be missing a
`Subsystem`:

| Subsystem | screeps.com | OpenScreeps | Community server |
|---|---|---|---|
| `PasswordSignIn` | yes | yes | with screepsmod-auth |
| `PasswordAccounts` (registration, password changes) | on the website | yes | with screepsmod-auth |
| `ApiTokens` | yes | no | with screepsmod-auth |
| `RateLimitLift` | yes (web page) | no | with screepsmod-auth (one request) |
| `UsernameLookup` | yes | yes | with screepsmod-auth |
| `AccountWebsite` | yes | no | no |
| `CpuShards` (CPU allocation between shards) | yes | no | no |
| `ShardsInfo` (shards with their CPU limits) | yes | no | with screepsmod-admin-utils |
| `RoomObjects` | yes | yes | with screepsmod-admin-utils |
| `RoomHistory` | yes | yes | with a room-history mod |
| `RoomDecorations` | yes | yes | no |
| `ObjectSearch` | yes | yes | no |
| `NoviceRooms` | yes | yes | no |

Write your code against the subsystems, not against the kind of server:

- Ask `World::supports(Subsystem::…)` before you show or use a feature.
- An operation of a missing subsystem fails with `Error::Unsupported` and sends no request.
- Use `World::connection()` only to show or log the kind of server and its plugins.

To skip the discovery, force the contract with `ClientBuilder::server_kind` and the plugins with
`ClientBuilder::plugins`. `ServerProfile` has the same two fields.

## Workspace

| Path | Contents |
|---|---|
| `crates/overseer-core` | Sans-I/O core: the generated HTTP and socket types and endpoints of each contract, the domain types (`model`) with the game's rules (levels, NPCs, the fields of a new account, the CPU allocation to the shards and the 12-hour cooldown, the 2 MiB Memory capacity, Memory paths, queued Memory changes and the segment limits, the tick duration measured from game ticks), the socket session and the room state |
| `crates/overseer-net` | Async client on `overseer-core`: HTTP, socket, rate limits and the request budget (`Client::wait_for_budget`), discovery and the area traits of `api` |
| `crates/overseer` | Application layer: the session, the account (one session-wide profile published after every read and change: `Account::profile_watch`; the usage of the code shared by every consumer: `Account::usage`), the exemption from rate limits (`Overseer::rate_limits`: checked on a schedule, given to the request budget and followed by every world map), live rooms, the console (and the session-cached names of the user's game for completion), code, Memory (session-shared path snapshots and writes observed until they apply), the map (and the session-shared tick speed of a shard: `Map::tick_speed`), player badges (one session-wide cache and pace for every viewer), the market and the other features of the official client, and a server without an account (`Guest`: discovery, registration) |
| `crates/overseer-view` | World and room viewer: renders rooms with wgpu (WebGPU, else WebGL2) |
| `crates/overseer-proxy` | Same-origin proxy for servers that send no CORS headers |
| `app` | The Overseer app: a Leptos UI for browsers |
| `app/src-tauri` | The desktop shell of the app (Tauri 2) |
| `web/demo` | A small browser demo of `overseer` |
| `xtask` | Repository automation (`cargo xtask …`) |
| `reference` | The API contracts that the code is generated from |
| `e2e` | Playwright UI tests and the test worlds |

## Use the library

```rust
use overseer::prelude::*;

let profile = ServerProfile {
    url: ServerUrl::parse("http://localhost:21025/").expect("a valid URL"),
    auth: Auth::Credentials { email: "player@example.com".into(), password: "secret".into() },
    server_password: None,
    default_shard: None,
    server_kind: None, // discover
    plugins: None,     // discover
};
let overseer = Overseer::connect(profile).await?;
if overseer.world().supports(Subsystem::RoomHistory) {
    // Offer room history.
}
```

Run `cargo doc --workspace --no-deps --open` for the API documentation.

## Build and run

The toolchain is pinned in `rust-toolchain.toml`. The tasks are in `mise.toml`.

- Desktop app, development: `mise run app-dev` (`cargo tauri dev`). Install the Tauri 2 CLI and
  the system libraries that Tauri needs first.
- Desktop app, installers: `mise run app-build` (`cargo tauri build`).
- Web app: `mise run app-web` (Trunk). The output goes to `app/dist/`.
- Serve the web app for a server that sends no CORS headers:
  `cargo run -p overseer-proxy -- --upstream https://screeps.com/ --static app/dist`.

## Contracts and generated code

The contracts come from [openscreeps/openapi](https://github.com/openscreeps/openapi): the
official contract, the OpenScreeps and community profiles and one contract for each plugin. Do
not edit the files in `reference/` or the generated code. To refresh them:

```sh
cargo xtask vendor [<openapi checkout>]   # default: ../openapi, needs node
cargo xtask codegen
```

`cargo xtask codegen --check` fails when the generated files are stale.

## Tests

Run all the checks of CI:

```sh
mise run ci
```

The end-to-end tests run the live Rust tests and the Playwright UI tests against a disposable
world in Docker. They need Docker, Node.js and Trunk. Before the first run, install the
Playwright packages:

```sh
cd e2e && npm ci && npx playwright install chromium
```

| Command | Server under test |
|---|---|
| `cargo xtask e2e` | an OpenScreeps world, built from the latest commit of the server's `main` branch (`OPENSCREEPS_SERVER_COMMIT` builds another commit) |
| `cargo xtask e2e --target community` | a community world: screeps-launcher with screepsmod-auth and screepsmod-admin-utils (`e2e/community/`) |
| `cargo xtask e2e --target official` | screeps.com, read-only, with `SCREEPS_TOKEN` |

Add `--rust-only` or `--ui-only` to run one of the two suites. Put Playwright arguments after
`--`. On Windows, `--desktop` runs the tests of the desktop app.

Each run writes its files to its own directory below `target/e2e/`: the web app bundle, the
proxy binary, the logs, and the Playwright results and report. A run that passes removes its
directory. A run that fails keeps it and prints its path. Runs share only the build in
`target/`, so you can run the `openscreeps` and `community` targets at the same time. Their web
app builds go one at a time, and a build of the live tests waits while another run executes
them. The `official` and `--desktop` runs keep one directory each, and only one run of each
goes at a time.

On Windows, do not change `xtask/` while a run goes on: Windows locks the running `xtask.exe`,
so `cargo xtask` cannot rebuild it.

On `openscreeps`, every UI test runs once with the app served by a static server (`direct`).
The tests tagged `@transport` (sign-in, session, socket) also run with the app served by
`overseer-proxy` (`proxy`). The `community` and `official` runs serve the app through the proxy
for every test. A test that fails is not retried: find its cause. The tests tagged `@alone`
measure how fast the page responds: they run after all other tests, with the machine to
themselves (Playwright project `alone`).
