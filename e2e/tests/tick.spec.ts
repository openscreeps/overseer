// The tick duration: the status bar's tick item adds how long a tick of the shard on the map
// takes, with its unit (`tick N · 3.1 s/tick`, `250 ms/tick` on fast servers), and its tooltip
// gives it in seconds and in ticks per second and says where it comes from: the server reports
// it, or the app measures it from the game time.
//
// The `player` fixture's bot keeps the disposable world ticking.

import { expect, test } from './fixtures';
import { status } from './support/room';

test.describe('tick duration', { tag: '@openscreeps' }, () => {
  test('the status bar shows the tick duration and its source', async ({
    signedInPage: page,
    player,
  }) => {
    expect(player.username).toBeTruthy();
    const item = status(page).getByTitle('Game tick');
    // The duration ends the item, with its unit: `250 ms/tick`, `3.1 s/tick` or `12 s/tick`.
    await expect(item).toHaveText(/(^|· )(\d+ ms|\d+(\.\d)? s)\/tick$/, { timeout: 30_000 });
    await expect(item).toHaveAttribute(
      'title',
      /^Game tick and tick duration: \d+\.\d\d s per tick \(\d+\.\d\d ticks per second\), (reported by the server|measured from game time)$/,
    );
  });
});
