//! Delivery of queued messages (ADR 0133): one loop and one retry schedule for every queue
//! of messages the panel keeps.
//!
//! An owner keeps its messages in its own table and implements [`Queue`] for it: it claims
//! due messages so that no other worker sends them, sends one, and records how the send
//! ended. [`deliver`] runs one pass over a queue — claim, send, record — and works out from
//! the queue's [`Policy`] when a failed message is due again and whether it is given up.
//! Which messages are due, which are held back and what is recorded stay with the owner;
//! the platform names no queue. Only [`deliver`] makes a [`Claim`] or an [`Outcome`], so no
//! queue is driven by a loop of its own.

use super::Failure;
use crate::ports::BoxFuture;

/// Characters of a failure that an outcome keeps.
const ERROR_CHARS: usize = 500;

/// How one queue claims, retries and gives up its messages.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Policy {
    /// The table that keeps the queue, as its owner names it.
    pub table: &'static str,
    /// Claims in one pass; the pass ends early when a claim finds nothing due.
    pub claims: usize,
    /// How a claim keeps its messages from other workers while they are sent.
    pub claiming: Claiming,
    /// Sends a message gets: when the last of them fails, the message is given up.
    pub attempts: i32,
    /// The wait before a message is due again after a send.
    pub backoff: Backoff,
    /// Seconds before a message that its owner holds back (during a maintenance window) is
    /// due again; `None` leaves such a message due, for the next claim to check again.
    pub held: Option<i64>,
}

/// How a claim keeps its messages from other workers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Claiming {
    /// One message, locked by the claiming transaction, which stays open while the message
    /// is sent and commits with its outcome.
    Locked,
    /// Up to `batch` messages, leased for `seconds`: the claim commits before they are
    /// sent, so a send that never finishes leaves its message to a claim after the lease.
    Leased { seconds: i64, batch: i64 },
}

/// The wait before a message is due again.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Backoff {
    /// `first` seconds after a message's first send, doubling after each later one, at most
    /// `doublings` times. A wait the channel asks for replaces it; either is kept within
    /// `min..=max` seconds.
    Doubling {
        first: i64,
        doublings: u32,
        min: i64,
        max: i64,
    },
    /// The same wait after every send, whatever the channel asks.
    Fixed(i64),
}

impl Backoff {
    /// The wait after a send that followed `previous` sends of the same message, when the
    /// channel asked for `asked` seconds.
    fn wait(self, previous: i32, asked: Option<i64>) -> i64 {
        match self {
            Self::Fixed(seconds) => seconds,
            Self::Doubling {
                first,
                doublings,
                min,
                max,
            } => {
                let doublings = u32::try_from(previous).unwrap_or(0).min(doublings).min(62);
                asked
                    .unwrap_or_else(|| first.saturating_mul(1_i64 << doublings))
                    .clamp(min, max)
            }
        }
    }
}

/// What one claim may take. Only [`deliver`] makes one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct Claim {
    /// The time of the claim: messages due by then are due.
    pub now: i64,
    /// The most messages the claim takes.
    pub limit: i64,
    /// When the lease of a leased claim ends; `now` for a locked one.
    pub lease_until: i64,
    /// When a message held back is due again; `now` when the policy leaves it due.
    pub held_until: i64,
}

/// A claimed message and the sends it had before this claim.
pub struct Claimed<M> {
    pub message: M,
    pub attempts: i32,
}

/// What a claim returns: the claimed messages, or the owner's error.
pub type Claims<'a, M, E> = BoxFuture<'a, Result<Vec<Claimed<M>>, E>>;

/// How one send ended, worked out from the queue's policy. Only [`deliver`] makes one.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct Outcome {
    /// When the message was claimed.
    pub attempted_at: i64,
    /// When its send ended; never before the claim.
    pub finished_at: i64,
    /// Why the send failed, at most 500 characters; `None` when the message was sent.
    pub error: Option<String>,
    /// When the message is due again: the end of the send plus the policy's wait.
    pub retry_at: i64,
    /// Whether this was the last send the policy allows.
    pub last_attempt: bool,
}

impl Outcome {
    pub fn sent(&self) -> bool {
        self.error.is_none()
    }
}

impl Policy {
    fn claim(&self, now: i64) -> Claim {
        let (limit, lease) = match self.claiming {
            Claiming::Locked => (1, 0),
            Claiming::Leased { seconds, batch } => (batch, seconds),
        };
        Claim {
            now,
            limit,
            lease_until: now.saturating_add(lease),
            held_until: now.saturating_add(self.held.unwrap_or(0)),
        }
    }

    fn outcome(
        &self,
        previous: i32,
        result: Result<(), Failure>,
        attempted_at: i64,
        now: i64,
    ) -> Outcome {
        let finished_at = now.max(attempted_at);
        let (error, asked) = match result {
            Ok(()) => (None, None),
            Err(failure) => (
                Some(failure.message.chars().take(ERROR_CHARS).collect()),
                failure.retry_after,
            ),
        };
        Outcome {
            attempted_at,
            finished_at,
            error,
            retry_at: finished_at.saturating_add(self.backoff.wait(previous, asked)),
            last_attempt: previous.saturating_add(1) >= self.attempts,
        }
    }
}

/// A queue of messages in its owner's table.
pub trait Queue: Sync {
    /// How this queue claims, retries and gives up its messages.
    const POLICY: Policy;
    /// A claimed message, with what the owner needs to send it and record the outcome.
    type Message: Send;
    type Error: Send;

    /// The time a pass uses; the wall clock unless the owner fixes it for the pass.
    fn now(&self) -> i64 {
        apsp_protocol::now_timestamp()
    }

    /// Claims the messages due at `claim.now`, at most `claim.limit`, as the policy's
    /// claiming says; returns none when nothing is due.
    fn claim(&self, claim: Claim) -> Claims<'_, Self::Message, Self::Error>;

    /// Sends one claimed message.
    fn send<'a>(&'a self, message: &'a Self::Message) -> BoxFuture<'a, Result<(), Failure>>;

    /// Records how the send of a claimed message ended.
    fn settle(
        &self,
        message: Self::Message,
        outcome: Outcome,
    ) -> BoxFuture<'_, Result<(), Self::Error>>;
}

/// One pass over a queue: up to the policy's number of claims, each claimed message sent
/// and its outcome recorded before the next one. The pass ends when a claim finds nothing.
pub async fn deliver<Q: Queue>(queue: &Q) -> Result<(), Q::Error> {
    let policy = Q::POLICY;
    for _ in 0..policy.claims {
        let claim = policy.claim(queue.now());
        let claimed = queue.claim(claim).await?;
        if claimed.is_empty() {
            break;
        }
        for Claimed { message, attempts } in claimed {
            let result = queue.send(&message).await;
            let outcome = policy.outcome(attempts, result, claim.now, queue.now());
            queue.settle(message, outcome).await?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    const DOUBLING: Policy = Policy {
        table: "fixture",
        claims: 3,
        claiming: Claiming::Locked,
        attempts: 8,
        backoff: Backoff::Doubling {
            first: 30,
            doublings: 7,
            min: 1,
            max: 86_400,
        },
        held: None,
    };

    fn failure(retry_after: Option<i64>) -> Result<(), Failure> {
        Err(Failure {
            message: "failed".into(),
            retry_after,
        })
    }

    #[test]
    fn the_wait_doubles_up_to_its_cap_and_a_requested_wait_stays_within_bounds() {
        let waits: Vec<i64> = (0..10)
            .map(|previous| DOUBLING.backoff.wait(previous, None))
            .collect();
        assert_eq!(waits, [30, 60, 120, 240, 480, 960, 1920, 3840, 3840, 3840]);
        assert_eq!(DOUBLING.backoff.wait(-1, None), 30);
        assert_eq!(DOUBLING.backoff.wait(3, Some(123)), 123);
        assert_eq!(DOUBLING.backoff.wait(3, Some(0)), 1);
        assert_eq!(DOUBLING.backoff.wait(3, Some(999_999)), 86_400);
        let bounded = Backoff::Doubling {
            first: 30,
            doublings: 4,
            min: 30,
            max: 86_400,
        };
        assert_eq!(bounded.wait(0, Some(5)), 30);
        assert_eq!(bounded.wait(4, None), 480);
        assert_eq!(bounded.wait(9, None), 480);
        assert_eq!(Backoff::Fixed(300).wait(4, Some(5)), 300);
    }

    #[test]
    fn outcomes_count_attempts_bound_errors_and_never_finish_before_the_claim() {
        let sent = DOUBLING.outcome(0, Ok(()), 100, 108);
        assert!(sent.sent());
        assert_eq!((sent.finished_at, sent.retry_at), (108, 138));
        assert!(!sent.last_attempt);
        let retried = DOUBLING.outcome(6, failure(Some(123)), 100, 90);
        assert_eq!(retried.finished_at, 100);
        assert_eq!(retried.retry_at, 223);
        assert!(!retried.last_attempt);
        assert!(DOUBLING.outcome(7, failure(None), 100, 100).last_attempt);
        let long = DOUBLING.outcome(
            0,
            Err(Failure {
                message: "错".repeat(600),
                retry_after: None,
            }),
            1,
            1,
        );
        assert_eq!(long.error.unwrap().chars().count(), 500);
        let leased = Policy {
            claiming: Claiming::Leased {
                seconds: 300,
                batch: 4,
            },
            held: Some(60),
            ..DOUBLING
        };
        assert_eq!(
            leased.claim(1000),
            Claim {
                now: 1000,
                limit: 4,
                lease_until: 1300,
                held_until: 1060
            }
        );
        assert_eq!(
            DOUBLING.claim(1000),
            Claim {
                now: 1000,
                limit: 1,
                lease_until: 1000,
                held_until: 1000
            }
        );
    }

    /// Messages are numbers; the batches stand for what each claim finds due.
    struct Fixture {
        batches: Mutex<Vec<Vec<(u32, i32)>>>,
        failing: u32,
        settled: Mutex<Vec<(u32, Outcome)>>,
    }

    impl Queue for Fixture {
        const POLICY: Policy = DOUBLING;
        type Message = u32;
        type Error = ();

        fn now(&self) -> i64 {
            50
        }

        fn claim(&self, claim: Claim) -> Claims<'_, u32, ()> {
            assert_eq!(claim.now, 50);
            let mut batches = self.batches.lock().unwrap();
            let batch = if batches.is_empty() {
                Vec::new()
            } else {
                batches.remove(0)
            };
            Box::pin(async move {
                Ok(batch
                    .into_iter()
                    .map(|(message, attempts)| Claimed { message, attempts })
                    .collect())
            })
        }

        fn send<'a>(&'a self, message: &'a u32) -> BoxFuture<'a, Result<(), Failure>> {
            let result = if *message == self.failing {
                failure(None)
            } else {
                Ok(())
            };
            Box::pin(async move { result })
        }

        fn settle(&self, message: u32, outcome: Outcome) -> BoxFuture<'_, Result<(), ()>> {
            self.settled.lock().unwrap().push((message, outcome));
            Box::pin(async { Ok(()) })
        }
    }

    #[tokio::test]
    async fn a_pass_settles_every_claimed_message_and_stops_at_an_empty_claim() {
        let queue = Fixture {
            batches: Mutex::new(vec![vec![(1, 0), (2, 3)], vec![], vec![(3, 0)]]),
            failing: 2,
            settled: Mutex::new(Vec::new()),
        };
        deliver(&queue).await.unwrap();
        let settled = queue.settled.into_inner().unwrap();
        let summary: Vec<_> = settled
            .iter()
            .map(|(message, outcome)| (*message, outcome.sent(), outcome.retry_at))
            .collect();
        assert_eq!(summary, [(1, true, 80), (2, false, 290)]);
        assert_eq!(queue.batches.into_inner().unwrap(), [vec![(3, 0)]]);
    }
}
