//! Token buckets for the public `/s/*` routes: requests per client network on the share
//! page, and on the media, which can stream whole files, bytes per client and per link,
//! with only so many streams per client at once.

use std::{
    collections::HashMap,
    net::{IpAddr, Ipv6Addr, SocketAddr},
    sync::{Arc, Mutex},
    time::Duration,
};

use axum::{
    body::Body,
    extract::{ConnectInfo, Path, Request, State},
    http::{HeaderValue, StatusCode, header},
    middleware::Next,
    response::{IntoResponse, Response},
};
use futures_util::StreamExt;
// tokio's clock rather than the system's, so tests can stop it.
use tokio::time::Instant;

use crate::AppState;

/// Keys a limiter remembers per generation (see `Buckets`), so at most twice this.
const CAPACITY: usize = 10_000;

/// The media budgets count MiB.
const MIB: usize = 1024 * 1024;

pub struct RateLimiter {
    /// Tokens refilled per second.
    rate: f64,
    burst: f64,
    /// How long an untouched bucket takes to fill up again, after which forgetting it
    /// loses nothing.
    idle: Duration,
    capacity: usize,
    buckets: Mutex<Buckets>,
}

/// The buckets in two generations: those touched since `started`, and those touched in
/// the generation before and not since. A new generation starts once the current one is
/// full or `idle` old, and the one before it is dropped whole, so the map stays bounded
/// without ever being scanned. A client that keeps coming back moves into each new
/// generation; only one that stayed away for a whole generation is forgotten.
struct Buckets {
    current: HashMap<String, Bucket>,
    previous: HashMap<String, Bucket>,
    started: Instant,
}

struct Bucket {
    tokens: f64,
    last: Instant,
}

impl RateLimiter {
    pub fn new(per_minute: u32, burst: u32) -> Self {
        Self::with_capacity(per_minute, burst, CAPACITY)
    }

    fn with_capacity(per_minute: u32, burst: u32, capacity: usize) -> Self {
        let rate = f64::from(per_minute) / 60.0;
        let burst = f64::from(burst);
        Self {
            rate,
            burst,
            // A bucket that never refills is never full again: keep it for good.
            idle: Duration::try_from_secs_f64(burst / rate).unwrap_or(Duration::MAX),
            capacity,
            buckets: Mutex::new(Buckets {
                current: HashMap::new(),
                previous: HashMap::new(),
                started: Instant::now(),
            }),
        }
    }

    /// Takes one token for `key`; false when the bucket is empty.
    pub fn allow(&self, key: String) -> bool {
        self.allow_at(key, Instant::now())
    }

    fn allow_at(&self, key: String, now: Instant) -> bool {
        self.with_tokens(key, now, |tokens| {
            let allowed = *tokens >= 1.0;
            if allowed {
                *tokens -= 1.0;
            }
            allowed
        })
    }

    /// Takes `n` tokens for `key` even when it hasn't got them, for something already
    /// used, and says how long until it has paid them back. Media owe at most about a
    /// chunk per open stream, so forgetting a bucket that still owes forgives next to
    /// nothing.
    pub fn charge(&self, key: &str, n: f64) -> Duration {
        self.charge_at(key, n, Instant::now())
    }

    fn charge_at(&self, key: &str, n: f64, now: Instant) -> Duration {
        self.with_tokens(key.to_owned(), now, |tokens| {
            *tokens -= n;
            if *tokens >= 0.0 {
                Duration::ZERO
            } else {
                // Never, when the bucket doesn't refill.
                Duration::try_from_secs_f64(-*tokens / self.rate).unwrap_or(Duration::MAX)
            }
        })
    }

    /// Runs `take` on `key`'s tokens, refilled up to `now`.
    fn with_tokens<T>(&self, key: String, now: Instant, take: impl FnOnce(&mut f64) -> T) -> T {
        let mut guard = self.buckets.lock().expect("rate limiter lock");
        let buckets = &mut *guard;
        if buckets.current.len() >= self.capacity
            || now.duration_since(buckets.started) >= self.idle
        {
            buckets.previous = std::mem::take(&mut buckets.current);
            buckets.started = now;
        }
        let previous = &mut buckets.previous;
        let bucket = buckets.current.entry(key).or_insert_with_key(|key| {
            previous.remove(key).unwrap_or(Bucket {
                tokens: self.burst,
                last: now,
            })
        });
        bucket.tokens = (bucket.tokens + now.duration_since(bucket.last).as_secs_f64() * self.rate)
            .min(self.burst);
        bucket.last = now;
        take(&mut bucket.tokens)
    }
}

/// The limits on public share links.
pub struct ShareLimits {
    /// Requests per client, on the share page and its `clip.json`.
    pub pages: RateLimiter,
    /// MiB per client, streamed from the video and poster.
    pub media: RateLimiter,
    /// MiB per link, streamed from its video and poster to everyone together.
    pub link_media: RateLimiter,
    /// Videos and posters each client may be streaming at once.
    pub streams: StreamLimit,
}

impl ShareLimits {
    /// `pages` in requests and the media in MiB, each per minute with bursts up to half
    /// of it, and `streams` at once per client.
    pub fn new(pages: u32, media_mib: u32, link_media_mib: u32, streams: usize) -> Self {
        let limiter = |per_minute: u32| RateLimiter::new(per_minute, (per_minute / 2).max(1));
        Self {
            pages: limiter(pages),
            media: limiter(media_mib),
            link_media: limiter(link_media_mib),
            streams: StreamLimit::new(streams),
        }
    }
}

/// How many media responses each client is streaming, up to a cap. Only clients with one
/// open are remembered.
pub struct StreamLimit {
    per_client: usize,
    open: Mutex<HashMap<String, usize>>,
}

impl StreamLimit {
    pub fn new(per_client: usize) -> Self {
        Self {
            per_client,
            open: Mutex::default(),
        }
    }

    /// Counts one more stream for `key`; false when it already has as many as it may.
    fn open(&self, key: &str) -> bool {
        let mut open = self.open.lock().expect("stream limit lock");
        let count = open.get(key).copied().unwrap_or(0);
        if count >= self.per_client {
            return false;
        }
        open.insert(key.to_owned(), count + 1);
        true
    }

    /// One of `key`'s streams has ended.
    fn close(&self, key: &str) {
        let mut open = self.open.lock().expect("stream limit lock");
        if let Some(count) = open.get_mut(key) {
            *count -= 1;
            if *count == 0 {
                open.remove(key);
            }
        }
    }
}

/// One media response's place among its client's streams, and its bill. The first MiB is
/// paid when the request is let in; past that, the body's bytes go on the client's and
/// the link's budgets as they are sent, and each chunk waits until both have paid for it.
/// So a client gets its budget's speed and no more however many requests it makes, and a
/// player that seeks pays only for what it read before it let go. Dropping the meter (the
/// body was sent, or the client went away) frees its place.
struct Meter {
    limits: Arc<ShareLimits>,
    client: String,
    link: String,
    /// Bytes still covered by the MiB paid up front.
    prepaid: usize,
}

impl Meter {
    /// A place among `client`'s streams; `None` when it has none free.
    fn open(limits: Arc<ShareLimits>, client: String, link: String) -> Option<Self> {
        if !limits.streams.open(&client) {
            return None;
        }
        Some(Self {
            limits,
            client,
            link,
            prepaid: MIB,
        })
    }

    /// Puts `bytes` on the bill, and says how long to hold them back.
    fn charge(&mut self, bytes: usize) -> Duration {
        let prepaid = bytes.min(self.prepaid);
        self.prepaid -= prepaid;
        if bytes == prepaid {
            return Duration::ZERO;
        }
        let mib = (bytes - prepaid) as f64 / MIB as f64;
        let client = self.limits.media.charge(&self.client, mib);
        let link = self.limits.link_media.charge(&self.link, mib);
        client.max(link)
    }

    /// `body`, sent at the pace the budgets allow.
    fn pace(self, body: Body) -> Body {
        Body::from_stream(futures_util::stream::unfold(
            (body.into_data_stream(), self),
            |(mut chunks, mut meter)| async move {
                let chunk = chunks.next().await?;
                if let Ok(bytes) = &chunk {
                    let wait = meter.charge(bytes.len());
                    if !wait.is_zero() {
                        tokio::time::sleep(wait).await;
                    }
                }
                Some((chunk, (chunks, meter)))
            },
        ))
    }
}

impl Drop for Meter {
    fn drop(&mut self) {
        self.limits.streams.close(&self.client);
    }
}

/// Who a request counts against: the client's IP, or its /64 for IPv6, since one home or
/// phone is usually handed a whole /64. Behind App Service the front end appends the
/// address to `X-Forwarded-For` (as `ip:port`), so the last entry is the one we can
/// trust; locally it's the socket.
fn client_key(req: &Request) -> String {
    let forwarded = req
        .headers()
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.rsplit(',').next())
        .map(str::trim)
        .filter(|v| !v.is_empty());
    if let Some(entry) = forwarded {
        let ip = entry
            .parse::<SocketAddr>()
            .map(|addr| addr.ip())
            .or_else(|_| entry.parse::<IpAddr>());
        return match ip {
            Ok(ip) => network(ip).to_string(),
            Err(_) => entry.to_owned(),
        };
    }
    req.extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|ConnectInfo(addr)| network(addr.ip()).to_string())
        .unwrap_or_else(|| "unknown".into())
}

/// An IPv4 address as it is (also when written as IPv6), an IPv6 one cut to its /64.
fn network(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
            Some(v4) => IpAddr::V4(v4),
            None => IpAddr::V6(Ipv6Addr::from(u128::from(v6) & (u128::MAX << 64))),
        },
        v4 => v4,
    }
}

fn slow_down() -> Response {
    let mut res = (StatusCode::TOO_MANY_REQUESTS, "slow down").into_response();
    res.headers_mut()
        .insert(header::RETRY_AFTER, HeaderValue::from_static("30"));
    res
}

/// The share page and its `clip.json`.
pub async fn limit_pages(State(state): State<AppState>, req: Request, next: Next) -> Response {
    if state.share_limits.pages.allow(client_key(&req)) {
        return next.run(req).await;
    }
    slow_down()
}

/// A link's video and poster. Each request can stream a whole file, so they are paid for
/// by the byte (see `Meter`), out of a budget per client and one per link so many clients
/// together can't pull it without end either, and a client has only so many streams at
/// once. A client over its own limits doesn't use up the link's.
pub async fn limit_media(
    State(state): State<AppState>,
    Path(token): Path<String>,
    req: Request,
    next: Next,
) -> Response {
    let Some(meter) = Meter::open(state.share_limits, client_key(&req), token) else {
        return slow_down();
    };
    let limits = &meter.limits;
    if limits.media.allow(meter.client.clone()) && limits.link_media.allow(meter.link.clone()) {
        return next.run(req).await.map(|body| meter.pace(body));
    }
    slow_down()
}

#[cfg(test)]
mod tests {
    use http_body_util::BodyExt;

    use super::*;

    fn remembered(limiter: &RateLimiter) -> usize {
        let buckets = limiter.buckets.lock().unwrap();
        buckets.current.len() + buckets.previous.len()
    }

    #[test]
    fn refills_over_time() {
        let limiter = RateLimiter::new(60, 3);
        let t0 = Instant::now();
        assert!((0..3).all(|_| limiter.allow_at("a".into(), t0)));
        assert!(!limiter.allow_at("a".into(), t0), "burst used up");
        assert!(limiter.allow_at("b".into(), t0), "per client");
        assert!(
            limiter.allow_at("a".into(), t0 + Duration::from_secs(1)),
            "1 token per second"
        );
        assert!(!limiter.allow_at("a".into(), t0 + Duration::from_secs(1)));
    }

    #[test]
    fn keys_on_the_last_forwarded_address() {
        let key = |forwarded: &str| {
            client_key(
                &Request::builder()
                    .header("x-forwarded-for", forwarded)
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
        };
        assert_eq!(key("6.6.6.6, 203.0.113.7:51234"), "203.0.113.7");
        // A bare address (no port) is taken as it is.
        assert_eq!(key("6.6.6.6, 198.51.100.4"), "198.51.100.4");
        // Something else isn't an address, but still tells clients apart.
        assert_eq!(key("6.6.6.6, somewhere"), "somewhere");
        // No header and no socket: everyone shares one bucket.
        let req = Request::builder().body(axum::body::Body::empty()).unwrap();
        assert_eq!(client_key(&req), "unknown");
        // Locally it's the socket.
        let mut req = Request::builder().body(axum::body::Body::empty()).unwrap();
        req.extensions_mut()
            .insert(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 5173))));
        assert_eq!(client_key(&req), "127.0.0.1");
    }

    #[test]
    fn groups_ipv6_clients_by_their_64() {
        let key = |forwarded: &str| {
            client_key(
                &Request::builder()
                    .header("x-forwarded-for", forwarded)
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
        };
        // Every address in one /64 is one client, with or without a port.
        assert_eq!(key("2001:db8:1:2::7"), "2001:db8:1:2::");
        assert_eq!(
            key("[2001:db8:1:2:aaaa:bbbb:cccc:dddd]:443"),
            "2001:db8:1:2::"
        );
        // The next /64 over is someone else.
        assert_eq!(key("2001:db8:1:3::7"), "2001:db8:1:3::");
        // An IPv4 address written as IPv6 is the IPv4 client.
        assert_eq!(key("[::ffff:203.0.113.7]:51234"), "203.0.113.7");

        let mut req = Request::builder().body(axum::body::Body::empty()).unwrap();
        req.extensions_mut().insert(ConnectInfo(SocketAddr::from((
            "2001:db8::1:2:3:4".parse::<IpAddr>().unwrap(),
            443,
        ))));
        assert_eq!(client_key(&req), "2001:db8::");
    }

    #[test]
    fn remembers_a_bounded_number_of_clients() {
        // A full bucket of 3 at 1 a second: idle for 3 s, a client has nothing to remember.
        let limiter = RateLimiter::with_capacity(60, 3, 100);
        let t0 = Instant::now();
        assert!((0..3).all(|_| limiter.allow_at("busy".into(), t0)));
        // Many more clients than it keeps, all at once: never more than two generations.
        for i in 0..1_000 {
            limiter.allow_at(format!("client {i}"), t0);
            assert!(remembered(&limiter) <= 200);
        }
        // "busy" was forgotten in the crowd, and starts again with a full bucket.
        assert!(limiter.allow_at("busy".into(), t0));
    }

    #[test]
    fn keeps_clients_that_come_back() {
        let limiter = RateLimiter::with_capacity(60, 3, 100);
        let t0 = Instant::now();
        assert!((0..3).all(|_| limiter.allow_at("busy".into(), t0)));
        // A client seen in every generation is carried along: its bucket stays empty.
        for i in 0..1_000 {
            limiter.allow_at(format!("client {i}"), t0);
            if i % 50 == 0 {
                assert!(!limiter.allow_at("busy".into(), t0), "after {i}");
            }
        }
    }

    #[test]
    fn forgets_clients_once_their_buckets_are_full_again() {
        let limiter = RateLimiter::with_capacity(60, 3, 100);
        let t0 = Instant::now();
        assert!(limiter.allow_at("old".into(), t0));
        assert!(limiter.allow_at("recent".into(), t0 + Duration::from_secs(2)));
        // 3 s on, a new generation starts; both are still remembered.
        assert!(limiter.allow_at("new".into(), t0 + Duration::from_secs(3)));
        assert_eq!(remembered(&limiter), 3);
        // Another 3 s on, the generation before goes: "old" and "recent" (idle 4 s or
        // more, so full again) are forgotten, "new" is kept.
        assert!(limiter.allow_at("newer".into(), t0 + Duration::from_secs(6)));
        let buckets = limiter.buckets.lock().unwrap();
        assert!(buckets.previous.contains_key("new"));
        assert!(!buckets.previous.contains_key("old") && !buckets.previous.contains_key("recent"));
        assert_eq!(buckets.current.len() + buckets.previous.len(), 2);
    }

    #[test]
    fn a_limit_of_zero_never_refills() {
        let limiter = RateLimiter::new(0, 1);
        let t0 = Instant::now();
        assert!(limiter.allow_at("a".into(), t0));
        assert!(!limiter.allow_at("a".into(), t0 + Duration::from_secs(3600)));
    }

    #[test]
    fn share_limits_burst_half_the_rate() {
        let limits = ShareLimits::new(4, 2, 1, 1);
        let t0 = Instant::now();
        let used =
            |limiter: &RateLimiter| (0..10).filter(|_| limiter.allow_at("a".into(), t0)).count();
        assert_eq!(used(&limits.pages), 2);
        assert_eq!(used(&limits.media), 1);
        assert_eq!(used(&limits.link_media), 1, "at least one");
    }

    #[test]
    fn a_charge_can_leave_a_bucket_owing() {
        let limiter = RateLimiter::new(60, 3);
        let t0 = Instant::now();
        assert_eq!(
            limiter.charge_at("a", 2.5, t0),
            Duration::ZERO,
            "it had them"
        );
        assert_eq!(
            limiter.charge_at("a", 2.5, t0),
            Duration::from_secs(2),
            "owes 2, at 1 a second"
        );
        // Nothing more until it has paid them back and refilled a whole token.
        assert!(!limiter.allow_at("a".into(), t0 + Duration::from_secs(2)));
        assert!(limiter.allow_at("a".into(), t0 + Duration::from_secs(3)));
        // A bucket that never refills never pays back.
        let limiter = RateLimiter::new(0, 1);
        assert_eq!(limiter.charge_at("a", 2.0, t0), Duration::MAX);
    }

    #[test]
    fn caps_the_streams_each_client_has_open() {
        let streams = StreamLimit::new(2);
        assert!(streams.open("a") && streams.open("a"));
        assert!(!streams.open("a"), "two at once");
        assert!(streams.open("b"), "per client");
        streams.close("a");
        assert!(streams.open("a"), "one of them ended");
        for key in ["a", "a", "b"] {
            streams.close(key);
        }
        assert!(
            streams.open.lock().unwrap().is_empty(),
            "clients with nothing open are forgotten"
        );
    }

    /// Media limits with room for one stream per client, in MiB a minute.
    fn media_limits(
        client: u32,
        client_burst: u32,
        link: u32,
        link_burst: u32,
    ) -> Arc<ShareLimits> {
        Arc::new(ShareLimits {
            pages: RateLimiter::new(60, 1),
            media: RateLimiter::new(client, client_burst),
            link_media: RateLimiter::new(link, link_burst),
            streams: StreamLimit::new(1),
        })
    }

    /// `client` streams a body of `mib` chunks of 1 MiB from `link`; how long it took.
    async fn stream(limits: &Arc<ShareLimits>, client: &str, link: &str, mib: usize) -> Duration {
        let meter = Meter::open(Arc::clone(limits), client.into(), link.into()).unwrap();
        let chunks = (0..mib).map(|_| Ok::<_, std::io::Error>(vec![7u8; MIB]));
        let body = Body::from_stream(futures_util::stream::iter(chunks));
        let start = Instant::now();
        let sent = meter.pace(body).collect().await.unwrap().to_bytes();
        assert_eq!(sent.len(), mib * MIB);
        start.elapsed()
    }

    /// Whether `elapsed` is `secs`, give or take the timer's rounding.
    fn about(elapsed: Duration, secs: u64) -> bool {
        elapsed.abs_diff(Duration::from_secs(secs)) < Duration::from_millis(10)
    }

    #[tokio::test(start_paused = true)]
    async fn media_streams_at_the_pace_of_the_clients_budget() {
        // 1 MiB a second after a burst of 2 for the client; the link has plenty.
        let limits = media_limits(60, 2, 6000, 100);
        let meter = Meter::open(Arc::clone(&limits), "a".into(), "link".into()).unwrap();
        assert!(
            Meter::open(Arc::clone(&limits), "a".into(), "other".into()).is_none(),
            "one stream at once"
        );
        drop(meter);
        // The first MiB counts as paid when the request was let in, the next two come out
        // of the burst, and the last two at 1 MiB a second.
        let elapsed = stream(&limits, "a", "link", 5).await;
        assert!(about(elapsed, 2), "{elapsed:?}");
        // Done, so its place is free again. Another client starts with a burst of its own.
        assert!(about(stream(&limits, "b", "link", 3).await, 0));
        assert!(about(stream(&limits, "a", "link", 2).await, 1));
    }

    #[tokio::test(start_paused = true)]
    async fn a_links_media_streams_at_the_pace_of_its_budget_for_everyone() {
        // 1 MiB a second after a burst of 2 for the link; each client has plenty.
        let limits = media_limits(6000, 100, 60, 2);
        assert!(about(stream(&limits, "a", "link", 3).await, 0));
        // The link's burst is used up: the next client waits for it, whoever it is.
        assert!(about(stream(&limits, "b", "link", 3).await, 2));
        assert!(about(stream(&limits, "b", "other link", 3).await, 0));
        // Within the MiB paid up front, nothing waits.
        assert!(about(stream(&limits, "c", "link", 1).await, 0));
    }

    #[tokio::test]
    async fn a_failed_read_is_passed_on_and_frees_its_place() {
        let limits = media_limits(60, 1, 60, 1);
        let meter = Meter::open(Arc::clone(&limits), "a".into(), "link".into()).unwrap();
        let chunks = [
            Ok(vec![7u8; 10]),
            Err(std::io::Error::other("storage went away")),
        ];
        let body = meter.pace(Body::from_stream(futures_util::stream::iter(chunks)));
        assert!(body.collect().await.is_err());
        assert!(limits.streams.open.lock().unwrap().is_empty());
    }
}
