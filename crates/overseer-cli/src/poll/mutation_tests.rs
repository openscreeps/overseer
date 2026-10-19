//! Tests how the CLI records the result of a placement or a reset. The tests
//! use no HTTP fixtures.

use std::path::PathBuf;
use std::time::UNIX_EPOCH;

use overseer::core::api::ApiError;

use super::*;

/// Runs `test` on the watch of a test profile. The watch opens no session.
fn with_watch(test: impl FnOnce(&mut Watch<'_>)) {
    let config = Config::parse(
        r#"
        [servers.local]
        url = "http://127.0.0.1:21025/"
        token = "unused-test-token"
        [profiles.local]
        server = "local"
        [profiles.local.spawn]
        selector = ["unused-selector"]
        "#,
        PathBuf::from("overseer.toml"),
    )
    .expect("valid configuration");
    let targets = config.select_spawn(&[]).expect("spawn profile");
    let target = *targets.first().expect("one profile");
    let spawn = target.profile.spawn.as_ref().expect("spawn configuration");
    test(&mut Watch::new(target, spawn, Instant::now()));
}

/// A server error gives an unknown outcome. The unknown outcome blocks all
/// later placements, not only the chosen room. A request that the CLI did
/// not send does not clear the block.
#[test]
fn uncertain_placement_latches_profile_lockout() {
    with_watch(|watch| {
        let planned = Instant::now();
        watch.planned = Some(planned);
        let mut output = Vec::new();
        assert!(watch.finish_placement(Ok(()), &mut output).is_ok());
        assert_eq!(watch.planned, None);
        watch.planned = Some(planned);
        assert!(
            watch
                .finish_placement(Err(overseer::Error::Closed), &mut output)
                .is_err()
        );
        assert!(!watch.placement_unknown);
        assert_eq!(watch.planned, Some(planned));
        assert!(
            watch
                .finish_placement(
                    Err(overseer::Error::Net(net::Error::Api(ApiError::Server(
                        "room busy".to_owned(),
                    )))),
                    &mut output,
                )
                .is_err()
        );
        assert!(watch.placement_unknown);
        assert!(
            watch
                .finish_placement(Err(overseer::Error::Closed), &mut output)
                .is_err()
        );
        assert!(
            watch.placement_unknown,
            "an unsent operation cannot resolve an earlier placement"
        );
    });
}

/// A reset with an unknown outcome starts the same cooldown as a confirmed
/// reset. A reset that the CLI did not send does not start a cooldown. It
/// also does not clear an earlier cooldown.
#[test]
fn reset_certainty_controls_local_cooldown() {
    with_watch(|watch| {
        let now = UNIX_EPOCH + Duration::from_secs(1000);
        let later = now + Duration::from_secs(60);
        let mut output = Vec::new();
        assert!(
            watch
                .finish_reset(Err(overseer::Error::Closed), now, &mut output)
                .is_err()
        );
        assert_eq!(watch.respawned, None);
        assert!(watch.finish_reset(Ok(()), now, &mut output).is_ok());
        assert_eq!(watch.respawned, Some(now));
        assert!(
            watch
                .finish_reset(
                    Err(overseer::Error::Net(net::Error::Api(ApiError::Server(
                        "failed".to_owned()
                    )))),
                    later,
                    &mut output,
                )
                .is_err()
        );
        assert_eq!(watch.respawned, Some(later));
        assert_eq!(
            cooldown(None, watch.respawned, later),
            Some(Duration::from_secs(190))
        );
        assert!(
            watch
                .finish_reset(
                    Err(overseer::Error::Closed),
                    later + Duration::from_secs(60),
                    &mut output
                )
                .is_err()
        );
        assert_eq!(watch.respawned, Some(later));
    });
}
