//! The request budget of overviews: the near share, 429 pauses, rechecks and exemptions.

use std::time::Duration;

use overseer::map::{Interest, STATS_BATCH_SECTORS, WorldEvent, WorldMap};
use overseer::net::api::WorldOperation;

use super::{
    Fake, advance, answer_pings, drive_for, drive_until, epoch_millis, event, lock, map_stats,
    names_of, quiet, rate_limit, rect, room, rooms_of, settle, stats,
};
use crate::connect;

/// The next pause change of overview loading, skipping terrain events.
async fn paused(world: &mut WorldMap) -> Option<u64> {
    loop {
        match event(world).await {
            WorldEvent::StatsPaused(until) => return until,
            WorldEvent::Terrain(..) | WorldEvent::NoTerrain(..) => {}
            other => panic!("unexpected {other:?}"),
        }
    }
}

#[tokio::test]
async fn far_overviews_leave_the_near_share_of_the_budget_and_a_429_waits_for_the_reset() {
    // 160 × 160 rooms: 256 sectors, the far ones take 6 requests.
    let fake = Fake::limited(160, rect("W79N79", "E79S79"));
    lock(&fake.scripted).extend([
        // Near: 40 of 60 left, the window resets in 1000 s: three far requests go out at once.
        (200, rate_limit(60, 40, Duration::from_secs(1000))),
        // Far: 18 left (other windows share the token), the near share (30 %): no more far
        // requests until the reset.
        (200, rate_limit(60, 18, Duration::from_secs(1000))),
        (200, rate_limit(60, 18, Duration::from_secs(1000))),
        (200, rate_limit(60, 18, Duration::from_secs(1000))),
        // Near again: rejected until the reset 120 s later (beyond the client's own 60 s).
        (429, rate_limit(60, 0, Duration::from_secs(120))),
    ]);
    let (server, overseer, websocket) = connect(fake.routes()).await;
    answer_pings(websocket);
    let mut world = overseer.map().world(None).await.unwrap();
    tokio::time::pause();

    // Near: the minimap's sector and those around it inside the world (W79N79..W60N60), the
    // rooms in view.
    world.set_interest(&Interest {
        stats: Some(rect("W79N79", "W60N60")),
        minimaps: vec![room("W75N75")],
        ..Interest::default()
    });
    let near = stats(&mut world).await;
    assert_eq!(
        rooms_of(near.iter().map(|(room, _)| room)),
        rooms_of(&rect("W79N79", "W60N60").iter().collect::<Vec<_>>())
    );
    for _ in 0..3 {
        assert_eq!(stats(&mut world).await.len(), STATS_BATCH_SECTORS * 100);
    }

    // The far rooms left wait for the window's reset.
    advance(&mut world, Duration::from_secs(300)).await;
    quiet(&mut world).await;
    assert_eq!(map_stats(&server).len(), 4);
    assert!(world.progress().stats_paused_until.is_some());

    // New near rooms still go out: the 429 holds them until its reset.
    world.set_interest(&Interest {
        stats: Some(rect("E60S60", "E79S79")),
        minimaps: vec![room("E75S75")],
        ..Interest::default()
    });
    settle(&mut world, || map_stats(&server).len() == 5).await;
    let corner = rooms_of(&rect("E60S60", "E79S79").iter().collect::<Vec<_>>());
    assert!(names_of(&map_stats(&server)[4]).is_subset(&corner));
    advance(&mut world, Duration::from_secs(60)).await;
    quiet(&mut world).await;
    assert_eq!(map_stats(&server).len(), 5);
    assert!(world.progress().stats_paused_until.is_some());
    advance(&mut world, Duration::from_secs(65)).await;
    let near = stats(&mut world).await;
    assert!(rooms_of(near.iter().map(|(room, _)| room)).is_subset(&corner));
    assert_eq!(map_stats(&server).len(), 6);

    // After the reset the far rooms resume (none left in the window: one at a time), until
    // every room is known.
    while !world.progress().is_complete() {
        stats(&mut world).await;
    }
    assert_eq!(world.progress().stats_known, 25_600);
    assert!(map_stats(&server).len() <= 9);
    quiet(&mut world).await;
    assert_eq!(world.progress().stats_paused_until, None);
}

/// The reset of `X-RateLimit-*` `headers` as [`WorldProgress::stats_paused_until`] reports
/// it: milliseconds since the Unix epoch, a second after the reset (clock skew).
///
/// [`WorldProgress::stats_paused_until`]: overseer::map::WorldProgress::stats_paused_until
fn paused_until(headers: &[(&'static str, String)]) -> u64 {
    let (_, reset) = headers
        .iter()
        .find(|(name, _)| *name == "X-RateLimit-Reset")
        .unwrap();
    (reset.parse::<u64>().unwrap() + 1) * 1000
}

#[tokio::test]
async fn a_paused_overview_load_says_until_when_and_a_recheck_after_a_reset_resumes() {
    // 80 × 80 rooms, all far (no minimap): two requests of the nearest sectors.
    let fake = Fake::limited(80, rect("W39N39", "E39S39"));
    // 18 of 60 left after the first: only the near share, so far rooms wait for the reset in
    // half an hour.
    let exhausted = rate_limit(60, 18, Duration::from_mins(30));
    let until = paused_until(&exhausted);
    lock(&fake.scripted).push_back((200, exhausted));
    let (server, overseer, websocket) = connect(fake.routes()).await;
    answer_pings(websocket);
    let mut world = overseer.map().world(None).await.unwrap();
    tokio::time::pause();

    world.set_interest(&Interest {
        stats: Some(world.bounds()),
        ..Interest::default()
    });
    assert_eq!(world.progress().stats_paused_until, None);
    assert_eq!(stats(&mut world).await.len(), STATS_BATCH_SECTORS * 100);
    // Paused, and until when: the reset as the server's clock says it.
    assert_eq!(paused(&mut world).await, Some(until));
    let progress = world.progress();
    assert_eq!(progress.stats_paused_until, Some(until));
    assert!(!progress.is_complete());
    let left = until - epoch_millis();
    assert!((1_790_000..=1_802_000).contains(&left), "{left} ms left");
    advance(&mut world, Duration::from_secs(600)).await;
    quiet(&mut world).await;
    assert_eq!(map_stats(&server).len(), 1);

    // A recheck while the limit still holds: one request, refused, and paused again.
    lock(&fake.scripted).push_back((429, rate_limit(60, 0, Duration::from_mins(20))));
    assert!(world.recheck_rate_limit());
    assert_eq!(paused(&mut world).await, None);
    let again = paused(&mut world).await;
    assert!(again.is_some_and(|again| again < until), "{again:?}");
    assert_eq!(world.progress().stats_paused_until, again);
    assert_eq!(map_stats(&server).len(), 2);
    advance(&mut world, Duration::from_secs(60)).await;
    quiet(&mut world).await;
    assert_eq!(map_stats(&server).len(), 2);

    // The user lifted the limit: the recheck's request goes out at once, its headers record
    // the real limit, and loading resumes until every room is known.
    lock(&fake.scripted).push_back((200, rate_limit(60, 59, Duration::from_secs(3600))));
    assert!(world.recheck_rate_limit());
    assert_eq!(paused(&mut world).await, None);
    assert_eq!(stats(&mut world).await.len(), 1400);
    assert_eq!(map_stats(&server).len(), 3);
    assert_eq!(
        WorldOperation::MapStats
            .rate_limit(overseer.client())
            .map(|limit| limit.remaining),
        Some(59)
    );
    let progress = world.progress();
    assert!(progress.is_complete());
    assert_eq!(progress.stats_paused_until, None);
}

#[tokio::test]
async fn a_recheck_while_not_paused_sends_nothing() {
    let fake = Fake::limited(40, rect("W19N19", "E19S19"));
    // Few requests left, but nothing waits for them.
    lock(&fake.scripted).extend([
        (200, rate_limit(60, 40, Duration::from_mins(30))),
        (200, rate_limit(60, 10, Duration::from_mins(30))),
    ]);
    let (server, overseer, websocket) = connect(fake.routes()).await;
    answer_pings(websocket);
    let mut world = overseer.map().world(None).await.unwrap();
    tokio::time::pause();

    // Before any request.
    assert!(!world.recheck_rate_limit());
    // The view over the near rooms; the far rooms follow in a second request.
    world.set_interest(&Interest {
        stats: Some(rect("W19N19", "W0N0")),
        minimaps: vec![room("W15N15")],
        ..Interest::default()
    });
    // Between two requests, while pacing.
    assert_eq!(stats(&mut world).await.len(), 400);
    assert!(!world.recheck_rate_limit());
    assert_eq!(stats(&mut world).await.len(), 1200);
    // Every room known and fresh: 10 left is no pause.
    quiet(&mut world).await;
    assert_eq!(world.progress().stats_paused_until, None);
    assert!(!world.recheck_rate_limit());
    advance(&mut world, Duration::from_secs(60)).await;
    quiet(&mut world).await;
    assert_eq!(map_stats(&server).len(), 2);
    // The limit seen is kept.
    assert_eq!(
        WorldOperation::MapStats
            .rate_limit(overseer.client())
            .map(|limit| limit.remaining),
        Some(10)
    );
}

#[tokio::test]
async fn an_exempt_token_ignores_the_budget_but_not_a_429() {
    // 160 × 160 rooms, all far (no minimap): six requests of the nearest sectors.
    let fake = Fake::limited(160, rect("W79N79", "E79S79"));
    let exhausted = rate_limit(60, 18, Duration::from_mins(30));
    let until = paused_until(&exhausted);
    lock(&fake.scripted).extend([
        // Down to the near share: far rooms wait for the reset.
        (200, exhausted),
        // Refused (another client used the window) until the reset 120 s later.
        (429, rate_limit(60, 0, Duration::from_secs(120))),
        // 5 left: the budget would spread them over the window.
        (200, rate_limit(60, 5, Duration::from_mins(30))),
    ]);
    let (server, overseer, websocket) = connect(fake.routes()).await;
    answer_pings(websocket);
    let mut world = overseer.map().world(None).await.unwrap();
    tokio::time::pause();

    world.set_interest(&Interest {
        stats: Some(world.bounds()),
        ..Interest::default()
    });
    assert_eq!(stats(&mut world).await.len(), STATS_BATCH_SECTORS * 100);
    assert_eq!(paused(&mut world).await, Some(until));

    // Lifted for two hours: the pause ends and the rest goes out at once, three at a time,
    // without a gap.
    world.set_rate_limit_exempt_until(Some(epoch_millis() + 7_200_000));
    assert_eq!(paused(&mut world).await, None);
    // Two requests go through; the 429 holds every request until its reset.
    drive_until(&mut world, |world| {
        map_stats(&server).len() == 4
            && world.progress().stats_known == 3 * STATS_BATCH_SECTORS * 100
    })
    .await;
    drive_for(&mut world).await;
    tokio::time::advance(Duration::from_secs(60)).await;
    drive_for(&mut world).await;
    assert_eq!(map_stats(&server).len(), 4);
    assert_eq!(world.progress().stats_paused_until, None);
    advance(&mut world, Duration::from_secs(65)).await;
    while !world.progress().is_complete() {
        event(&mut world).await;
    }
    // The refused rooms and the rest: three more requests.
    assert_eq!(map_stats(&server).len(), 7);
    assert_eq!(world.progress().stats_paused_until, None);
}

#[tokio::test]
async fn the_budget_applies_again_when_the_exemption_ends() {
    // 80 × 80 rooms, all far (no minimap): two requests of the nearest sectors.
    let fake = Fake::limited(80, rect("W39N39", "E39S39"));
    let exhausted = rate_limit(60, 18, Duration::from_mins(30));
    let until = paused_until(&exhausted);
    lock(&fake.scripted).extend([(200, exhausted.clone()), (200, exhausted)]);
    let (server, overseer, websocket) = connect(fake.routes()).await;
    answer_pings(websocket);
    let mut world = overseer.map().world(None).await.unwrap();
    tokio::time::pause();

    // Exempt for 3 s: both requests go out at once despite the budget.
    world.set_rate_limit_exempt_until(Some(epoch_millis() + 3_000));
    world.set_interest(&Interest {
        stats: Some(world.bounds()),
        ..Interest::default()
    });
    while !world.progress().is_complete() {
        event(&mut world).await;
    }
    assert_eq!(map_stats(&server).len(), 2);
    assert_eq!(world.progress().stats_paused_until, None);

    // A past exemption changes nothing; once the exemption ended the budget applies again:
    // the exhausted window holds the far rooms when they go stale.
    world.set_rate_limit_exempt_until(Some(epoch_millis() - 1));
    advance(&mut world, Duration::from_secs(4)).await;
    quiet(&mut world).await;
    assert_eq!(map_stats(&server).len(), 2);
    let _ = until;
}
