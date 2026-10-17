# UI reference screenshots

Taken by `e2e/tests/screens.spec.ts` (tag `@screens`, excluded from normal runs):
`cargo xtask e2e --screens[=<set>]` (before that flag exists:
`OVERSEER_E2E_SCREENS=<set> cargo xtask e2e --ui-only -- --grep @screens --project proxy`).
Each set has `desktop/` (1440×900) and `small/` (390×844). They show the UI for people comparing
changes; they are not assertions (the canvases depend on the GPU, accounts and rooms are random).

Sets:
- `before/`: the hand-written stylesheets (`style.css`, `drawer.css`) at 632bc23, before the
  switch to Tailwind.
- `after/`: the same screens after the switch to Tailwind v4 (on top of 3aa81b2: the room tab and
  the inspector also show the controller's upgrade progress, and the console keeps following).

Differences between the two, apart from the random rooms and accounts and those fixes: none
visible in the page chrome (servers, dialog, sign-in, register, profile, menus, panels, drawer,
console, windows match within a pixel or two).

| Image | Shows |
| --- | --- |
| `01-servers` | The servers screen, signed out: the built-in screeps.com entry. |
| `02-add-server` | The "Add server" dialog, filled in with a private server. |
| `03-toast` | The servers list with the new server and the "Added …" toast. |
| `04-sign-in` | The sign-in card of a private server (Sign in tab). |
| `05-register` | The same card on the Register tab. |
| `06-map-world` | The world map zoomed out as far as it goes (Escape), drawer open. |
| `07-map-sectors` | The world map wheeled in to the "Rooms" level (a sector of rooms). |
| `08-map-room-tooltip` | The player's room framed whole by the room search, the hovered room's tooltip. |
| `09-drawer-room` | The drawer's Room tab: the live room control and the room's facts (`small/`: the bottom sheet with the facts only; the map over it shows the room). |
| `10-drawer-objects-inspector` | The Objects tab with the spawn selected in the list and the inspector. |
| `11-drawer-console` | The Console tab with the bot's output. |
| `12-account-menu` | The account menu open. |
| `13-rate-limits` | The Rate limits panel (no limit reported yet on a private server). |
| `14-profile` | The profile page of the player. |
| `15-room-window` | A room popped out into its own window. |
| `16-room-close` | The player's room at the closest zoom over the spawn: the creeps carry the player's badge (set by the test) inside their ring of body parts. |
| `17-room-close-3d` | The same in the perspective view. |
