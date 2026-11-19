//! One bounded request to the paired Spotify renderer. All outcomes are explicit
//! and a command is never sent again after its first delivery.
use super::{BridgeImage, now, random_key, track_id, valid_image};
use serde::{Deserialize, Serialize};
use std::{
    sync::{
        Condvar, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

const DEADLINE: Duration = Duration::from_secs(8);
const POLL_FRESHNESS: Duration = Duration::from_secs(5);
const MAX_POLL_WAIT_MS: u64 = 20_000;
const MAX_RETRY_AFTER_SECONDS: u64 = 31_536_000;
pub(super) const UNAVAILABLE: &str = "The local Spotify connection is unavailable.";

#[derive(Clone, Serialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub(super) enum Action {
    Play { uri: String },
    Search { query: String },
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Command {
    id: String,
    #[serde(flatten)]
    action: Action,
    expires_at: u64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Poll {
    #[serde(deserialize_with = "Deserialize::deserialize")]
    result: Option<Reported>,
    #[serde(default, rename = "waitMs")]
    wait_ms: u64,
}

#[derive(Deserialize)]
#[serde(tag = "status", rename_all = "lowercase", deny_unknown_fields)]
enum Reported {
    Played {
        id: String,
    },
    Rejected {
        id: String,
    },
    Unconfirmed {
        id: String,
    },
    Results {
        id: String,
        data: CatalogueResults,
    },
    #[serde(rename = "rate_limited")]
    RateLimited {
        id: String,
        #[serde(rename = "retryAfterSeconds")]
        retry_after_seconds: u64,
    },
}

impl Reported {
    fn id(&self) -> &str {
        match self {
            Self::Played { id }
            | Self::Rejected { id }
            | Self::Unconfirmed { id }
            | Self::Results { id, .. }
            | Self::RateLimited { id, .. } => id,
        }
    }

    fn outcome(self, action: &Action) -> Option<Outcome> {
        match (self, action) {
            (Self::Played { .. }, Action::Play { .. }) => Some(Outcome::Played),
            (Self::Rejected { .. }, _) => Some(Outcome::Rejected),
            (Self::Unconfirmed { .. }, Action::Play { .. }) => Some(Outcome::Unconfirmed),
            (Self::Results { data, .. }, Action::Search { .. }) => Some(Outcome::Results(data)),
            (
                Self::RateLimited {
                    retry_after_seconds,
                    ..
                },
                _,
            ) => Some(Outcome::RateLimited(retry_after_seconds)),
            _ => None,
        }
    }
}

impl Poll {
    pub(super) fn parse(bytes: &[u8]) -> Result<Self, ()> {
        let poll = serde_json::from_slice::<Self>(bytes).map_err(|_| ())?;
        if poll.wait_ms > MAX_POLL_WAIT_MS {
            return Err(());
        }
        if let Some(result) = &poll.result {
            let id = result.id();
            if id.len() != 64 || !id.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                return Err(());
            }
            if let Reported::Results { data, .. } = result
                && !data.valid()
            {
                return Err(());
            }
            if let Reported::RateLimited {
                retry_after_seconds,
                ..
            } = result
                && !(1..=MAX_RETRY_AFTER_SECONDS).contains(retry_after_seconds)
            {
                return Err(());
            }
        }
        Ok(poll)
    }
}

pub(super) enum Outcome {
    Played,
    Rejected,
    Unconfirmed,
    Results(CatalogueResults),
    RateLimited(u64),
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct CatalogueResults {
    tracks: CatalogueTracks,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct CatalogueTracks {
    items: Vec<CatalogueTrack>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct CatalogueTrack {
    id: String,
    uri: String,
    #[serde(rename = "type")]
    kind: String,
    name: String,
    duration_ms: u64,
    artists: Vec<CatalogueArtist>,
    album: CatalogueAlbum,
    #[serde(default)]
    is_local: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    is_playable: Option<bool>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct CatalogueArtist {
    name: String,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct CatalogueAlbum {
    name: String,
    images: Vec<BridgeImage>,
}

fn bounded_text(text: &str, max_bytes: usize) -> bool {
    text.len() <= max_bytes && !text.chars().any(char::is_control)
}

impl CatalogueResults {
    fn valid(&self) -> bool {
        self.tracks.items.len() <= 10 && self.tracks.items.iter().all(CatalogueTrack::valid)
    }
}

impl CatalogueTrack {
    fn valid(&self) -> bool {
        self.kind == "track"
            && track_id(&self.uri) == Some(self.id.as_str())
            && !self.name.trim().is_empty()
            && bounded_text(&self.name, 2000)
            && (1..=86_400_000).contains(&self.duration_ms)
            && (1..=12).contains(&self.artists.len())
            && self
                .artists
                .iter()
                .all(|artist| !artist.name.trim().is_empty() && bounded_text(&artist.name, 1000))
            && bounded_text(&self.album.name, 2000)
            && self.album.images.len() <= 12
            && self.album.images.iter().all(|image| {
                !image.url.contains('?')
                    && valid_image(&image.url)
                    && image
                        .width
                        .is_none_or(|pixels| (1..=16384).contains(&pixels))
                    && image
                        .height
                        .is_none_or(|pixels| (1..=16384).contains(&pixels))
            })
    }
}

struct Pending {
    command: Command,
    expires_at: Instant,
    delivered: bool,
    outcome: Option<Outcome>,
}

#[derive(Default)]
struct State {
    last_poll: Option<(Instant, u64)>,
    pending: Option<Pending>,
    waiting: bool,
    search_limit: Option<Instant>,
    play_limit: Option<Instant>,
}

impl State {
    fn ready(&self, at: Instant) -> bool {
        self.waiting
            || self
                .last_poll
                .is_some_and(|(last, _)| at.saturating_duration_since(last) <= POLL_FRESHNESS)
    }

    fn retry_after(&self, action: &Action, at: Instant) -> Option<u64> {
        let until = match action {
            Action::Play { .. } => self.play_limit,
            Action::Search { .. } => self.search_limit,
        }?;
        let remaining = until
            .checked_duration_since(at)
            .filter(|wait| !wait.is_zero())?;
        Some(remaining.as_secs() + u64::from(remaining.subsec_nanos() != 0))
    }

    fn enqueue(
        &mut self,
        action: Action,
        at: Instant,
        unix_ms: u64,
        id: impl FnOnce() -> Result<String, String>,
    ) -> Result<Option<Instant>, String> {
        if self.pending.is_some() {
            return Err(
                "Spotify is still handling the previous request. Try again shortly.".into(),
            );
        }
        if !self.ready(at) {
            return Ok(None);
        }
        let expires_at = at + DEADLINE;
        self.pending = Some(Pending {
            command: Command {
                id: id()?,
                action,
                expires_at: unix_ms.saturating_add(DEADLINE.as_millis() as u64),
            },
            expires_at,
            delivered: false,
            outcome: None,
        });
        Ok(Some(expires_at))
    }

    fn poll(&mut self, poll: Poll, at: Instant, unix_ms: u64) -> (Option<Command>, bool) {
        self.last_poll = Some((at, unix_ms));
        if let Some(pending) = self.pending.as_mut()
            && pending.delivered
            && at < pending.expires_at
            && pending.outcome.is_none()
            && let Some(result) = poll.result
            && result.id() == pending.command.id
            && let Some(outcome) = result.outcome(&pending.command.action)
        {
            if let Outcome::RateLimited(seconds) = &outcome {
                let limit = match pending.command.action {
                    Action::Play { .. } => &mut self.play_limit,
                    Action::Search { .. } => &mut self.search_limit,
                };
                let until = at + Duration::from_secs(*seconds);
                *limit = Some(limit.map_or(until, |prior| prior.max(until)));
            }
            pending.outcome = Some(outcome);
            return (None, true);
        }
        (self.deliver(at), false)
    }

    fn deliverable(&self, at: Instant) -> bool {
        self.pending
            .as_ref()
            .is_some_and(|pending| !pending.delivered && at < pending.expires_at)
    }

    fn deliver(&mut self, at: Instant) -> Option<Command> {
        let pending = self
            .pending
            .as_mut()
            .filter(|pending| !pending.delivered && at < pending.expires_at)?;
        pending.delivered = true;
        Some(pending.command.clone())
    }
}

#[derive(Default)]
pub(super) struct Queue {
    state: Mutex<State>,
    changed: Condvar,
}

impl Queue {
    pub(super) fn last_seen(&self) -> Option<(u64, bool)> {
        let state = self.state.lock().ok()?;
        let (_, timestamp) = state.last_poll?;
        Some((timestamp, state.ready(Instant::now())))
    }

    pub(super) fn disable(&self, enabled: &AtomicBool) {
        // Serialize shutdown with a poll or a waiter about to release the state lock.
        let _state = self.state.lock();
        enabled.store(false, Ordering::SeqCst);
        self.changed.notify_all();
    }

    pub(super) fn poll(&self, poll: Poll, enabled: &AtomicBool) -> Result<Option<Command>, ()> {
        let mut state = self.state.lock().map_err(|_| ())?;
        if !enabled.load(Ordering::SeqCst) || state.waiting {
            // Only one held poll per pairing. A newer request cannot take
            // the command out of the renderer's already-waiting response.
            return Err(());
        }
        let wait = Duration::from_millis(poll.wait_ms);
        let (command, changed) = state.poll(poll, Instant::now(), now());
        if changed {
            self.changed.notify_all();
        }
        if command.is_some() || wait.is_zero() {
            return Ok(command);
        }

        state.waiting = true;
        self.changed.notify_all();
        let (mut state, _) = self
            .changed
            .wait_timeout_while(state, wait, |state| {
                enabled.load(Ordering::SeqCst) && !state.deliverable(Instant::now())
            })
            .map_err(|_| ())?;
        state.waiting = false;
        let at = Instant::now();
        state.last_poll = Some((at, now()));
        if !enabled.load(Ordering::SeqCst) {
            return Err(());
        }
        Ok(state.deliver(at))
    }

    pub(super) fn run(
        &self,
        action: Action,
        enabled: &AtomicBool,
    ) -> Result<Option<Outcome>, String> {
        let mut state = self.state.lock().map_err(|_| UNAVAILABLE)?;
        if !enabled.load(Ordering::SeqCst) {
            return Ok(None);
        }
        let at = Instant::now();
        if let Some(retry_after) = state.retry_after(&action, at) {
            return Ok(Some(Outcome::RateLimited(retry_after)));
        }
        let Some(expires_at) = state.enqueue(action, at, now(), || {
            random_key().map_err(|_| "Could not prepare the Spotify request.".into())
        })?
        else {
            return Ok(None);
        };
        self.changed.notify_all();
        let (mut state, _) = self
            .changed
            .wait_timeout_while(
                state,
                expires_at.saturating_duration_since(Instant::now()),
                |state| {
                    enabled.load(Ordering::SeqCst)
                        && state
                            .pending
                            .as_ref()
                            .is_some_and(|pending| pending.outcome.is_none())
                },
            )
            .map_err(|_| UNAVAILABLE)?;
        let outcome = state.pending.take().and_then(|pending| pending.outcome);
        outcome
            .map(Some)
            .ok_or_else(|| "Spotify did not confirm the background request in time.".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};
    use std::{sync::mpsc, thread};

    const TRACK: &str = "spotify:track:3OHfY25tqY28d16oZczHc8";
    const ID: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const OTHER_ID: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

    fn empty() -> Poll {
        Poll::parse(br#"{"result":null}"#).unwrap()
    }

    fn reported(result: Value) -> Poll {
        Poll::parse(&serde_json::to_vec(&json!({"result":result})).unwrap()).unwrap()
    }

    fn play() -> Action {
        Action::Play { uri: TRACK.into() }
    }

    fn search() -> Action {
        Action::Search {
            query: "Fixture".into(),
        }
    }

    fn waiting_poll() -> Poll {
        Poll::parse(br#"{"result":null,"waitMs":4000}"#).unwrap()
    }

    fn wait_for_poll(queue: &Queue) {
        let (_state, timeout) = queue
            .changed
            .wait_timeout_while(
                queue.state.lock().unwrap(),
                Duration::from_secs(2),
                |state| !state.waiting,
            )
            .unwrap();
        assert!(
            !timeout.timed_out(),
            "the renderer poll never started waiting"
        );
    }

    #[test]
    fn polling_wait_is_opt_in_and_bounded() {
        assert_eq!(empty().wait_ms, 0);
        assert_eq!(
            Poll::parse(br#"{"result":null,"waitMs":20000}"#)
                .unwrap()
                .wait_ms,
            20_000
        );
        assert!(Poll::parse(br#"{"result":null,"waitMs":20001}"#).is_err());
        assert!(Poll::parse(br#"{"result":null,"waitMs":-1}"#).is_err());
        assert!(Poll::parse(br#"{"waitMs":5}"#).is_err());
        let queue = Queue::default();
        let enabled = AtomicBool::new(true);
        let short_poll = Poll::parse(br#"{"result":null,"waitMs":1}"#).unwrap();
        assert!(queue.poll(short_poll, &enabled).unwrap().is_none());
        assert!(!queue.state.lock().unwrap().waiting);
        assert!(queue.last_seen().is_some_and(|(_, ready)| ready));
        assert!(queue.poll(empty(), &enabled).unwrap().is_none());
    }

    #[test]
    fn command_wakes_the_single_held_poll_and_is_never_replayed() {
        let queue = Queue::default();
        let enabled = AtomicBool::new(true);
        let (send_command, receive_command) = mpsc::channel();
        thread::scope(|scope| {
            scope.spawn(|| {
                send_command
                    .send(queue.poll(waiting_poll(), &enabled).unwrap())
                    .unwrap();
            });
            wait_for_poll(&queue);
            queue.state.lock().unwrap().last_poll = Some((
                Instant::now() - POLL_FRESHNESS - Duration::from_millis(1),
                100,
            ));
            assert_eq!(queue.last_seen(), Some((100, true)));
            assert!(queue.poll(empty(), &enabled).is_err());
            assert!(queue.poll(waiting_poll(), &enabled).is_err());

            let run = scope.spawn(|| queue.run(play(), &enabled));
            let command = receive_command
                .recv_timeout(Duration::from_secs(2))
                .unwrap()
                .unwrap();
            assert!(queue.poll(empty(), &enabled).unwrap().is_none());
            let result = reported(json!({"id":command.id,"status":"played"}));
            assert!(queue.poll(result, &enabled).unwrap().is_none());
            assert!(matches!(
                run.join().unwrap().unwrap(),
                Some(Outcome::Played)
            ));
        });
        assert!(queue.last_seen().is_some_and(|(_, ready)| ready));
    }

    #[test]
    fn disabling_the_bridge_wakes_its_poll() {
        let queue = Queue::default();
        let enabled = AtomicBool::new(true);
        let (send_result, receive_result) = mpsc::channel();
        thread::scope(|scope| {
            scope.spawn(|| {
                send_result
                    .send(queue.poll(waiting_poll(), &enabled).is_err())
                    .unwrap();
            });
            wait_for_poll(&queue);
            queue.disable(&enabled);
            assert!(receive_result.recv_timeout(Duration::from_secs(2)).unwrap());
        });
        assert!(queue.poll(empty(), &enabled).is_err());
        assert!(queue.run(play(), &enabled).unwrap().is_none());
    }

    #[test]
    fn rate_limits_require_a_live_matching_delivery_and_cool_down_only_its_action() {
        for invalid in [
            json!(0),
            json!(-1),
            json!(1.5),
            json!("30"),
            json!(31_536_001),
        ] {
            let invalid =
                json!({"result":{"id":ID,"status":"rate_limited","retryAfterSeconds":invalid}});
            assert!(Poll::parse(&serde_json::to_vec(&invalid).unwrap()).is_err());
        }
        for invalid in [
            json!({"id":ID,"status":"rate_limited"}),
            json!({"id":ID,"status":"rate_limited","retryAfterSeconds":30,"extra":true}),
        ] {
            assert!(Poll::parse(&serde_json::to_vec(&json!({"result":invalid})).unwrap()).is_err());
        }
        let limited =
            |id| reported(json!({"id":id,"status":"rate_limited","retryAfterSeconds":30}));
        let at = Instant::now();
        let queue = Queue::default();
        let enabled = AtomicBool::new(true);
        {
            let mut state = queue.state.lock().unwrap();
            state.poll(empty(), at, 100);
            let expires_at = state
                .enqueue(search(), at, 100, || Ok(ID.into()))
                .unwrap()
                .unwrap();
            let (command, accepted) = state.poll(limited(ID), at, 100);
            assert!(command.is_some());
            assert!(!accepted);
            assert!(!state.poll(limited(OTHER_ID), at, 100).1);
            assert!(!state.poll(limited(ID), expires_at, 8100).1);
            assert_eq!(state.retry_after(&search(), at), None);

            let report_at = expires_at + Duration::from_millis(1);
            state.pending = None;
            state
                .enqueue(search(), report_at, 8101, || Ok(OTHER_ID.into()))
                .unwrap()
                .unwrap();
            assert!(state.poll(empty(), report_at, 8101).0.is_some());
            assert!(!state.poll(limited(ID), report_at, 8101).1);
            assert!(state.poll(limited(OTHER_ID), report_at, 8101).1);
            let outcome = state.pending.take().unwrap().outcome;
            assert!(matches!(outcome, Some(Outcome::RateLimited(30))));
            assert_eq!(state.retry_after(&search(), report_at), Some(30));
            assert_eq!(state.retry_after(&play(), report_at), None);
            assert_eq!(
                state.retry_after(&search(), report_at + Duration::from_millis(29_500)),
                Some(1)
            );
            assert_eq!(
                state.retry_after(&search(), report_at + Duration::from_secs(30)),
                None
            );
            state.search_limit = Some(Instant::now() + Duration::from_secs(30));
        }
        assert!(matches!(
            queue.run(search(), &enabled).unwrap(),
            Some(Outcome::RateLimited(1..=30))
        ));
        assert!(queue.state.lock().unwrap().pending.is_none());
    }

    #[test]
    fn delivery_requires_a_fresh_poll_and_occurs_only_once() {
        let at = Instant::now();
        let mut state = State::default();
        assert!(
            state
                .enqueue(play(), at, 100, || panic!("no capable poll"))
                .unwrap()
                .is_none()
        );
        state.poll(empty(), at, 100);
        let late = at + POLL_FRESHNESS + Duration::from_millis(1);
        assert!(
            state
                .enqueue(play(), late, 100, || panic!("stale capable poll"))
                .unwrap()
                .is_none()
        );

        state.poll(empty(), late, 100);
        state
            .enqueue(play(), late, 100, || Ok(ID.into()))
            .unwrap()
            .unwrap();
        assert!(
            state
                .enqueue(play(), late, 100, || panic!("one command at a time"))
                .is_err()
        );
        let (command, accepted) =
            state.poll(reported(json!({"id":ID,"status":"played"})), late, 100);
        assert!(!accepted, "a report cannot finish an undelivered command");
        assert_eq!(
            serde_json::to_value(command.unwrap()).unwrap(),
            json!({
                "kind":"play", "id":ID, "uri":TRACK, "expiresAt":8100
            })
        );
        assert!(
            state.poll(empty(), late, 100).0.is_none(),
            "a dropped response is not reissued"
        );
        assert!(
            !state
                .poll(
                    reported(json!({"id":OTHER_ID,"status":"played"})),
                    late,
                    100
                )
                .1
        );
        assert!(
            !state
                .poll(
                    reported(json!({"id":ID,"status":"results","data":{"tracks":{"items":[]}}})),
                    late,
                    100
                )
                .1
        );
        assert!(
            state
                .poll(reported(json!({"id":ID,"status":"played"})), late, 100)
                .1
        );
        assert!(
            !state
                .poll(reported(json!({"id":ID,"status":"rejected"})), late, 100)
                .1
        );
        assert!(matches!(
            state.pending.unwrap().outcome,
            Some(Outcome::Played)
        ));
    }

    #[test]
    fn expired_commands_cannot_be_delivered_or_confirmed() {
        let at = Instant::now();
        for delivered in [false, true] {
            let mut state = State::default();
            state.poll(empty(), at, 100);
            let expires_at = state
                .enqueue(play(), at, 100, || Ok(ID.into()))
                .unwrap()
                .unwrap();
            if delivered {
                assert!(state.poll(empty(), at, 100).0.is_some());
            }
            let (command, accepted) = state.poll(
                reported(json!({"id":ID,"status":"played"})),
                expires_at,
                8100,
            );
            assert!(command.is_none());
            assert!(!accepted);
            assert!(state.pending.unwrap().outcome.is_none());
        }
    }

    #[test]
    fn search_returns_only_bounded_catalogue_fields_and_its_own_outcome() {
        let item = json!({
            "type":"track", "id":TRACK.trim_start_matches("spotify:track:"), "uri":TRACK,
            "name":"Fixture song", "duration_ms":220000, "artists":[{"name":"Fixture artist"}],
            "album":{"name":"Fixture album","images":[{"url":"https://i.scdn.co/image/fixture"}]}
        });
        let invalid_poll = |items: Vec<Value>| {
            let body =
                json!({"result":{"id":ID,"status":"results","data":{"tracks":{"items":items}}}});
            assert!(Poll::parse(&serde_json::to_vec(&body).unwrap()).is_err());
        };
        invalid_poll(vec![item.clone(); 11]);
        let mut unknown = item.clone();
        unknown["album"]["accessToken"] = json!("synthetic-fixture");
        invalid_poll(vec![unknown]);
        let mut invalid_image = item.clone();
        invalid_image["album"]["images"][0]["url"] = json!("https://i.scdn.co/image/fixture?q=one");
        invalid_poll(vec![invalid_image]);

        let at = Instant::now();
        let mut state = State::default();
        state.poll(empty(), at, 100);
        state.enqueue(search(), at, 100, || Ok(ID.into())).unwrap();
        let command = state.poll(empty(), at, 100).0.unwrap();
        assert_eq!(
            serde_json::to_value(command).unwrap(),
            json!({"kind":"search","id":ID,"query":"Fixture","expiresAt":8100})
        );
        assert!(
            !state
                .poll(reported(json!({"id":ID,"status":"played"})), at, 100)
                .1
        );
        assert!(
            state
                .poll(
                    reported(
                        json!({"id":ID,"status":"results","data":{"tracks":{"items":[item]}}})
                    ),
                    at,
                    100
                )
                .1
        );
        let Some(Outcome::Results(data)) = state.pending.unwrap().outcome else {
            panic!("missing catalogue results")
        };
        assert_eq!(
            serde_json::to_value(data).unwrap()["tracks"]["items"][0]["uri"],
            TRACK
        );
    }
}
