// The selector of the live test (selector protocol version 1).
//
// Run it as `node selector.js <answer file>`. It reads the candidates from stdin. It chooses the
// free tile nearest the room centre, at least two tiles from the edge, so that no exit is next
// to the tile. It skips rooms without one neutral controller and rooms with a keeper lair, an
// invader core or an object of another owner. It writes the answer to stdout and to the answer
// file, so that the test can compare the placed spawn with the answer.

'use strict';

const fs = require('fs');

const answerFile = process.argv[2];
if (!answerFile) {
  process.stderr.write('usage: node selector.js <answer file>\n');
  process.exit(2);
}

let text = '';
process.stdin.setEncoding('utf8');
process.stdin.on('data', (chunk) => {
  text += chunk;
});
process.stdin.on('end', () => {
  const input = JSON.parse(text);
  let best = null;
  for (const room of input.rooms) {
    const objects = room.objects || [];
    const controllers = objects.filter((object) => object.type === 'controller');
    if (controllers.length !== 1 || controllers[0].user || controllers[0].reservation) {
      continue;
    }
    const blocked = objects.some(
      (object) =>
        object.type === 'keeperLair' ||
        object.type === 'invaderCore' ||
        (object.user && object.type !== 'tombstone' && object.type !== 'ruin'),
    );
    if (blocked) {
      continue;
    }
    for (let y = 2; y < 48; y++) {
      for (let x = 2; x < 48; x++) {
        // Odd terrain digits are walls.
        if (Number(room.terrain[y * 50 + x]) % 2 === 1) {
          continue;
        }
        if (objects.some((object) => object.x === x && object.y === y)) {
          continue;
        }
        const distance = Math.max(Math.abs(x - 25), Math.abs(y - 25));
        if (best === null || distance < best.distance) {
          best = { room: room.name, x, y, distance };
        }
      }
    }
  }
  const answer = JSON.stringify(
    best === null
      ? { version: 1, room: null, reason: 'no free interior tile' }
      : {
          version: 1,
          room: best.room,
          x: best.x,
          y: best.y,
          reason: 'the free interior tile nearest the centre',
        },
  );
  fs.writeFileSync(answerFile, answer);
  process.stdout.write(answer);
});
