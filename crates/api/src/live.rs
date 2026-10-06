//! The show hub (docs/PLAN.md, redesign, S5): one live room per open show. Browsers
//! connect to `/api/shows/{id}/live` over a websocket; the first message carries the
//! access token (browsers can't set headers on a websocket). The server holds the one
//! true playback state, `LiveState`: the intents of anyone in the show change it (decision
//! 56), every change goes out to everyone, and each browser plays to it on its own clock
//! (decision 34). Everyone starts together because play is scheduled a moment ahead
//! (`lead`).
//!
//! The state is saved on every change so a deploy restart resumes the show; the hub runs
//! in this one api instance (one App Service instance, see the plan).
//!
//! When the server ends a connection for good it sends `{"type":"error"}` saying why (or
//! `{"type":"showOver"}` when the show ends), then closes with one of the `CLOSE_*` codes
//! below, which tell the client whether to reconnect.

use std::{
    collections::{HashMap, HashSet},
    sync::{
        Arc, Mutex, PoisonError,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use axum::{
    extract::{
        Path, State,
        ws::{CloseFrame, Message, WebSocket, WebSocketUpgrade},
    },
    response::Response,
};
use clipos_core::{
    clips::{self, ClipStatus},
    shows::{self, Show, ShowStatus},
    users::{self, User, UserStatus},
};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use tokio::sync::{broadcast, oneshot};
use uuid::Uuid;

use crate::{AppState, error::ApiError, extract::authenticate, routes::shows::shows_open_to};

/// Bad or expired token: the client gets a fresh one and tries once more.
pub const CLOSE_BAD_TOKEN: u16 = 4001;
/// Not allowed (shows aren't open to you, or your account is disabled): the client stops.
pub const CLOSE_NOT_ALLOWED: u16 = 4003;
/// No such show, or it's over: the client stops.
pub const CLOSE_SHOW_OVER: u16 = 4004;
/// Nothing heard for too long: the client reconnects as usual.
pub const CLOSE_IDLE: u16 = 4008;
/// Something went wrong on the server (the standard code): the client reconnects.
const CLOSE_INTERNAL: u16 = 1011;

/// Server pings keep idle connections (and App Service's proxy) alive.
const KEEPALIVE: Duration = Duration::from_secs(25);
/// After closing, how long to wait for the browser's own close before dropping the socket.
const CLOSE_GRACE: Duration = Duration::from_secs(1);
/// The largest message (and frame) a client may send; a hello with its token is ~2 KiB.
const MAX_MESSAGE: usize = 16 * 1024;
/// Steering, reactions and ready a connection may send: this many a second, in bursts of
/// up to `BURST`. More are dropped.
const RATE_PER_S: f64 = 10.0;
const BURST: f64 = 20.0;
/// Play starts this far ahead at least, so every browser can start together.
const MIN_LEAD_MS: f64 = 300.0;
const MAX_LEAD_MS: f64 = 2_000.0;
/// The furthest ahead anyone may schedule a start (`load` with `startAt`).
const MAX_START_AHEAD_MS: f64 = 5_000.0;

/// How long things may take on a live connection. `Default` is what runs in production;
/// tests shorten it.
#[derive(Debug, Clone, Copy)]
pub struct Timing {
    /// A new connection has this long to say hello.
    pub hello: Duration,
    /// A connection that sends nothing for this long is closed (`CLOSE_IDLE`). Clients ping
    /// every 30 s and browsers answer the server's keepalive pings by themselves, so only a
    /// dead connection goes this quiet.
    pub idle: Duration,
    /// The host may be taken over this long after leaving, once the clip has ended.
    pub takeover_after: Duration,
    /// A show everyone has left ends by itself after this long (decision 33). `Hub::tick`
    /// says when this clock runs.
    pub abandon_after: Duration,
}

impl Default for Timing {
    fn default() -> Self {
        Self {
            hello: Duration::from_secs(5),
            idle: Duration::from_secs(75),
            takeover_after: Duration::from_secs(60),
            abandon_after: Duration::from_secs(15 * 60),
        }
    }
}

/// Milliseconds since the Unix epoch on the server's clock; clients work out their offset
/// to it with pings.
pub fn now_ms() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64() * 1000.0)
        .unwrap_or_default()
}

/// What every player in the show should be doing: at server time `at_server_ms` the clip
/// is at `position_ms`, and moves at `rate` while `playing`. A play scheduled in the
/// future holds `position_ms` until then.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LiveState {
    /// Goes up on every change; clients ignore anything older than what they have. One
    /// seq is one state: the room only takes a state once it's final (see `commit`), so a
    /// welcome, a broadcast and the saved copy with the same seq are the same.
    pub seq: u64,
    pub clip_id: Option<Uuid>,
    pub playing: bool,
    pub position_ms: f64,
    pub at_server_ms: f64,
    pub rate: f64,
    /// The clip's length, when the server knows it.
    pub duration_ms: Option<f64>,
}

impl Default for LiveState {
    fn default() -> Self {
        Self {
            seq: 0,
            clip_id: None,
            playing: false,
            position_ms: 0.0,
            at_server_ms: now_ms(),
            rate: 1.0,
            duration_ms: None,
        }
    }
}

impl LiveState {
    /// Where the clip is at server time `t`.
    pub fn position_at(&self, t: f64) -> f64 {
        let p = if self.playing {
            self.position_ms + (t - self.at_server_ms).max(0.0) * self.rate
        } else {
            self.position_ms
        };
        match self.duration_ms {
            Some(d) => p.min(d),
            None => p,
        }
    }

    fn ended_at(&self, t: f64) -> bool {
        self.duration_ms.is_some_and(|d| self.position_at(t) >= d)
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum ClientMsg {
    /// First message: who you are.
    Hello {
        token: String,
    },
    /// Clock sync: echoed back with the server's time. `rtt` is the client's latest
    /// measured round trip, which sets how far ahead play is scheduled.
    Ping {
        client_ms: f64,
        rtt_ms: Option<f64>,
    },
    /// Put a clip on, paused at 0, or playing from server time `start_at` (at most 5 s
    /// ahead; sooner than the lead play needs is moved to then). Anyone in the show steers
    /// (decision 57).
    Load {
        clip_id: Uuid,
        start_at: Option<f64>,
    },
    Play,
    Pause,
    Seek {
        position_ms: f64,
    },
    /// A tap in the reaction dock (six reactions or 🍌), at a moment in the clip.
    React {
        clip_id: Uuid,
        emoji: String,
        at_ms: i32,
    },
    /// "I'm ready, sound on".
    Ready {
        ready: bool,
    },
    /// "Take over as host", once the host has been gone a minute and the clip ended.
    TakeOver,
}

impl ClientMsg {
    /// Messages that reach everyone (or the database): rate-limited.
    fn is_limited(&self) -> bool {
        matches!(
            self,
            Self::Load { .. }
                | Self::Play
                | Self::Pause
                | Self::Seek { .. }
                | Self::React { .. }
                | Self::Ready { .. }
        )
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum ServerMsg {
    Welcome {
        user_id: Uuid,
        server_ms: f64,
        state: LiveState,
        presence: Presence,
    },
    State {
        state: LiveState,
    },
    Pong {
        client_ms: f64,
        server_ms: f64,
    },
    Presence {
        presence: Presence,
    },
    Reaction {
        user_id: Uuid,
        clip_id: Uuid,
        emoji: String,
        at_ms: i32,
    },
    /// Something in the stored show changed (lineup, who joined, votes, status): fetch
    /// `GET /api/shows/{id}` again.
    ShowChanged,
    /// The show ended or was abandoned; the server closes with `CLOSE_SHOW_OVER` next.
    ShowOver,
    Error {
        message: String,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Presence {
    pub host_id: Uuid,
    /// Everyone connected right now.
    pub online: Vec<Uuid>,
    /// When the host's last connection closed (server ms), while they're away.
    pub host_away_since: Option<f64>,
}

/// Why the hub ends a connection from outside the connection's own loop.
#[derive(Debug, Clone, Copy)]
enum Kick {
    ShowOver,
    NotAllowed,
}

struct Conn {
    user: Uuid,
    rtt_ms: f64,
    /// Ends this connection; taken when used.
    kick: Option<oneshot::Sender<Kick>>,
}

struct RoomState {
    live: LiveState,
    host: Uuid,
    conns: HashMap<u64, Conn>,
    empty_since: Option<Instant>,
    /// Someone has connected since the room was made, or a tick found the show started
    /// (see `Hub::tick`).
    had_people: bool,
    host_away_since: Option<f64>,
    /// The show is over: everyone has been sent away and nobody new gets in.
    over: bool,
}

impl RoomState {
    fn presence(&self) -> Presence {
        let mut online: Vec<Uuid> = self
            .conns
            .values()
            .map(|c| c.user)
            .collect::<HashSet<_>>()
            .into_iter()
            .collect();
        online.sort();
        Presence {
            host_id: self.host,
            online,
            host_away_since: self.host_away_since,
        }
    }

    fn host_online(&self) -> bool {
        self.conns.values().any(|c| c.user == self.host)
    }

    /// How far ahead to schedule play: twice the slowest round trip, within bounds.
    fn lead_ms(&self) -> f64 {
        let slowest = self.conns.values().map(|c| c.rtt_ms).fold(0.0, f64::max);
        (2.0 * slowest).clamp(MIN_LEAD_MS, MAX_LEAD_MS)
    }

    /// Lets a connection in, with what it starts from: the live state and who's here.
    /// `None` once the show is over.
    fn join(
        &mut self,
        conn: u64,
        user: Uuid,
        kick: oneshot::Sender<Kick>,
    ) -> Option<(LiveState, Presence)> {
        if self.over {
            return None;
        }
        self.conns.insert(
            conn,
            Conn {
                user,
                rtt_ms: 100.0,
                kick: Some(kick),
            },
        );
        self.empty_since = None;
        self.had_people = true;
        if user == self.host {
            self.host_away_since = None;
        } else if !self.host_online() && self.host_away_since.is_none() {
            // The host isn't here and nobody saw them go: the takeover clock starts now.
            self.host_away_since = Some(now_ms());
        }
        Some((self.live.clone(), self.presence()))
    }

    fn kick(conn: &mut Conn, why: Kick) {
        if let Some(kick) = conn.kick.take() {
            let _ = kick.send(why);
        }
    }
}

pub struct Room {
    id: Uuid,
    tx: broadcast::Sender<ServerMsg>,
    state: Mutex<RoomState>,
    /// Whose turn it is to change the live state (see `steer_turn`).
    steering: tokio::sync::Mutex<()>,
}

impl Room {
    fn lock(&self) -> std::sync::MutexGuard<'_, RoomState> {
        self.state.lock().expect("room lock poisoned")
    }

    fn send(&self, msg: ServerMsg) {
        // No receivers is fine: nobody's connected.
        let _ = self.tx.send(msg);
    }

    /// The show is over: everyone gets `showOver` and is closed, and nobody new gets in.
    fn finish(&self) {
        let mut s = self.lock();
        s.over = true;
        for conn in s.conns.values_mut() {
            RoomState::kick(conn, Kick::ShowOver);
        }
    }
}

type Rooms = Mutex<HashMap<Uuid, Arc<Room>>>;

/// Forgets a show's room and sends everyone in it away.
fn close_room(rooms: &Rooms, show: Uuid) {
    let room = rooms.lock().expect("hub lock").remove(&show);
    if let Some(room) = room {
        tracing::info!(show = %show, "show over: closing its room");
        room.finish();
    }
}

/// Every open show's live room, in this api process.
pub struct Hub {
    pool: PgPool,
    rooms: Arc<Rooms>,
    next_conn: AtomicU64,
    timing: Timing,
}

impl Hub {
    pub fn new(pool: PgPool) -> Self {
        Self::with_timing(pool, Timing::default())
    }

    pub fn with_timing(pool: PgPool, timing: Timing) -> Self {
        Self {
            pool,
            rooms: Arc::default(),
            next_conn: AtomicU64::new(1),
            timing,
        }
    }

    /// The show's room, made (and its saved state loaded) on first use.
    async fn room(&self, show: &Show) -> sqlx::Result<Arc<Room>> {
        if let Some(room) = self.rooms.lock().expect("hub lock").get(&show.id) {
            return Ok(room.clone());
        }
        let saved = shows::live_state(&self.pool, show.id)
            .await?
            .and_then(|v| serde_json::from_value::<LiveState>(v).ok())
            .unwrap_or_default();
        let mut rooms = self.rooms.lock().expect("hub lock");
        Ok(rooms
            .entry(show.id)
            .or_insert_with(|| {
                Arc::new(Room {
                    id: show.id,
                    tx: broadcast::channel(256).0,
                    state: Mutex::new(RoomState {
                        live: saved,
                        host: show.host_id,
                        conns: HashMap::new(),
                        empty_since: Some(Instant::now()),
                        had_people: false,
                        // Nobody's connected yet, the host included (after a restart they
                        // may never come back): the takeover clock starts now.
                        host_away_since: Some(now_ms()),
                        over: false,
                    }),
                    steering: tokio::sync::Mutex::new(()),
                })
            })
            .clone())
    }

    /// The stored show changed through the REST API: tell everyone to fetch it again. If
    /// the change ended the show, everyone is sent away and the room goes; if it dropped
    /// the clip that's on, the clip comes off for everyone.
    pub fn show_changed(&self, show: Uuid) {
        let Some(room) = self.rooms.lock().expect("hub lock").get(&show).cloned() else {
            return;
        };
        room.send(ServerMsg::ShowChanged);
        let (pool, rooms) = (self.pool.clone(), self.rooms.clone());
        tokio::spawn(async move {
            match shows::get(&pool, show).await {
                Ok(Some(s)) if s.status.is_open() => unload_if_gone(&pool, &room).await,
                Ok(_) => close_room(&rooms, show),
                Err(e) => tracing::warn!(show = %show, error = %e, "checking the show's status"),
            }
        });
    }

    /// Housekeeping, every few seconds: ends a show everyone has left, closes rooms of
    /// shows that are over, and sends away anyone who's no longer allowed in.
    ///
    /// Everyone has left once the room has had people and has been empty for
    /// `Timing::abandon_after`. A started show counts as having had people, also when
    /// nobody has connected to it (since a restart, or since it started from a lobby
    /// nobody connected to): its clock then starts at the first tick that finds it
    /// started. A lobby only counts once someone has connected to it: until then people
    /// may be in it over the REST API alone, and it stays open for them.
    pub async fn tick(&self, state: &AppState) -> anyhow::Result<()> {
        let open = shows::open(&self.pool).await?;
        let stale: Vec<Uuid> = {
            let rooms = self.rooms.lock().expect("hub lock");
            rooms
                .keys()
                .filter(|id| open.as_ref().map(|s| s.id) != Some(**id))
                .copied()
                .collect()
        };
        for id in stale {
            close_room(&self.rooms, id);
        }
        let Some(show) = open else {
            return Ok(());
        };
        let room = self.room(&show).await?;
        let abandon = {
            let mut s = room.lock();
            if !s.had_people {
                // Nobody has left a room nobody has connected to: a lobby's clock doesn't
                // run yet, and a started show's starts now. Not at the lobby's last tick,
                // which may have been a while before the show started.
                s.empty_since = Some(Instant::now());
                s.had_people = show.status != ShowStatus::Lobby;
            }
            s.conns.is_empty()
                && s.empty_since
                    .is_some_and(|t| t.elapsed() >= self.timing.abandon_after)
        };
        if abandon {
            tracing::info!(show = %show.id, "nobody's there: show abandoned");
            shows::abandon(&self.pool, show.id).await?;
            room.send(ServerMsg::ShowChanged);
            close_room(&self.rooms, show.id);
            return Ok(());
        }
        self.recheck(state, &room).await
    }

    /// Sign-in is checked at hello; someone disabled since, or no longer let into shows,
    /// is sent away here.
    async fn recheck(&self, state: &AppState, room: &Room) -> anyhow::Result<()> {
        if room.lock().conns.is_empty() {
            return Ok(());
        }
        let users: HashMap<Uuid, User> = users::list(&self.pool)
            .await?
            .into_iter()
            .map(|u| (u.id, u))
            .collect();
        let mut s = room.lock();
        for conn in s.conns.values_mut() {
            let allowed = users
                .get(&conn.user)
                .is_some_and(|u| u.status == UserStatus::Active && shows_open_to(state, u));
            if !allowed {
                tracing::info!(show = %room.id, user = %conn.user, "no longer allowed in the show");
                RoomState::kick(conn, Kick::NotAllowed);
            }
        }
        Ok(())
    }

    /// Runs `tick` forever (spawned by main).
    pub async fn run(state: AppState) {
        let mut every = tokio::time::interval(Duration::from_secs(15));
        loop {
            every.tick().await;
            if let Err(e) = state.hub.tick(&state).await {
                tracing::warn!(error = ?e, "show hub tick failed");
            }
        }
    }
}

/// `GET /api/shows/{id}/live`: the websocket.
pub async fn connect(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Response {
    ws.max_message_size(MAX_MESSAGE)
        .max_frame_size(MAX_MESSAGE)
        .on_upgrade(move |socket| async move {
            if let Err(e) = serve(state, id, socket).await {
                tracing::debug!(show = %id, error = %e, "live connection ended");
            }
        })
}

async fn send(socket: &mut WebSocket, msg: &ServerMsg) -> anyhow::Result<()> {
    let text = serde_json::to_string(msg)?;
    socket.send(Message::Text(text.into())).await?;
    Ok(())
}

/// Closes with `code` and a short reason, then waits a moment for the browser's own close,
/// so the code reaches it before the connection drops.
async fn close(socket: &mut WebSocket, code: u16, reason: &str) -> anyhow::Result<()> {
    socket
        .send(Message::Close(Some(CloseFrame {
            code,
            reason: reason.into(),
        })))
        .await?;
    let _ = tokio::time::timeout(CLOSE_GRACE, async {
        while let Some(Ok(msg)) = socket.recv().await {
            if matches!(msg, Message::Close(_)) {
                break;
            }
        }
    })
    .await;
    Ok(())
}

/// Ends the connection for good: an error saying why, then the close.
async fn refuse(socket: &mut WebSocket, code: u16, message: &str) -> anyhow::Result<()> {
    send(
        socket,
        &ServerMsg::Error {
            message: message.into(),
        },
    )
    .await?;
    close(socket, code, message).await
}

/// Takes a connection out of its room when it goes, however it goes: an error, a failed
/// send, a panic.
struct Leave<'a> {
    room: &'a Room,
    conn: u64,
}

impl Drop for Leave<'_> {
    fn drop(&mut self) {
        let presence = {
            // A panic may have poisoned the lock; the room is still worth tidying.
            let mut s = self
                .room
                .state
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            let Some(gone) = s.conns.remove(&self.conn) else {
                return;
            };
            if s.conns.is_empty() {
                s.empty_since = Some(Instant::now());
            }
            if gone.user == s.host && !s.host_online() {
                s.host_away_since = Some(now_ms());
            }
            s.presence()
        };
        self.room.send(ServerMsg::Presence { presence });
    }
}

async fn serve(state: AppState, id: Uuid, mut socket: WebSocket) -> anyhow::Result<()> {
    let socket = &mut socket;

    // Hello first, with the token.
    let hello = tokio::time::timeout(state.hub.timing.hello, socket.recv()).await;
    let token = match hello {
        Ok(Some(Ok(Message::Text(text)))) => match serde_json::from_str(&text) {
            Ok(ClientMsg::Hello { token }) => token,
            _ => return refuse(socket, CLOSE_BAD_TOKEN, "say hello first").await,
        },
        Ok(Some(Ok(_))) => return refuse(socket, CLOSE_BAD_TOKEN, "say hello first").await,
        Ok(None | Some(Err(_))) => return Ok(()),
        Err(_) => return refuse(socket, CLOSE_IDLE, "no hello in time").await,
    };
    let (user, expires) = match authenticate(&state, &token).await {
        Ok((user, expires)) if shows_open_to(&state, &user) => (user, expires),
        Ok(_) => return refuse(socket, CLOSE_NOT_ALLOWED, "shows aren't open to you yet").await,
        Err(ApiError::Unauthorized(message)) => {
            return refuse(socket, CLOSE_BAD_TOKEN, &message).await;
        }
        Err(ApiError::Internal(e)) => {
            tracing::error!(error = ?e, "live connection sign-in");
            return refuse(socket, CLOSE_INTERNAL, "something went wrong").await;
        }
        // Not invited, disabled, or a token without an email.
        Err(e) => return refuse(socket, CLOSE_NOT_ALLOWED, &e.to_string()).await,
    };
    let show = match shows::get(&state.pool, id).await? {
        Some(show) if show.status.is_open() => show,
        _ => return refuse(socket, CLOSE_SHOW_OVER, "no such show, or it's over").await,
    };
    // Connecting joins (late joiners included).
    shows::join(&state.pool, id, user.id).await?;

    let room = state.hub.room(&show).await?;
    let mut rx = room.tx.subscribe();
    let conn = state.hub.next_conn.fetch_add(1, Ordering::Relaxed);
    let (kick, kicked) = oneshot::channel();
    let joined = room.lock().join(conn, user.id, kick);
    let Some((live, presence)) = joined else {
        return refuse(socket, CLOSE_SHOW_OVER, "no such show, or it's over").await;
    };
    // From here on, the connection leaves the room however this ends.
    let _leave = Leave { room: &room, conn };

    room.send(ServerMsg::Presence {
        presence: presence.clone(),
    });
    send(
        socket,
        &ServerMsg::Welcome {
            user_id: user.id,
            server_ms: now_ms(),
            state: live,
            presence,
        },
    )
    .await?;
    state.hub.show_changed(id); // someone joined

    // The connection lasts as long as the token: at its expiry the client reconnects with
    // a fresh one. Capped at a day so a far-off expiry can't overflow the clock.
    let left_ms = (expires as f64 * 1000.0 - now_ms()).clamp(0.0, 86_400_000.0);
    let ends = Ends {
        kicked,
        token_expires: tokio::time::Instant::now() + Duration::from_millis(left_ms as u64),
    };
    run(&state, &room, conn, &user, socket, &mut rx, ends).await
}

/// What ends a connection from outside: the hub's kick, or the token running out.
struct Ends {
    kicked: oneshot::Receiver<Kick>,
    token_expires: tokio::time::Instant,
}

/// Reactions, ready and replay requests per connection: a token bucket, `RATE_PER_S`
/// refilling up to `BURST`.
struct Bucket {
    tokens: f64,
    at: Instant,
    /// When the sender was last told to slow down.
    warned: Option<Instant>,
}

impl Bucket {
    fn new() -> Self {
        Self {
            tokens: BURST,
            at: Instant::now(),
            warned: None,
        }
    }

    /// Takes a token, if there's one left.
    fn take(&mut self) -> bool {
        let now = Instant::now();
        let refill = now.duration_since(self.at).as_secs_f64() * RATE_PER_S;
        self.tokens = (self.tokens + refill).min(BURST);
        self.at = now;
        if self.tokens < 1.0 {
            return false;
        }
        self.tokens -= 1.0;
        true
    }

    /// The error for a dropped message, at most once a second.
    fn slow_down(&mut self) -> Option<ServerMsg> {
        if self
            .warned
            .is_some_and(|t| t.elapsed() < Duration::from_secs(1))
        {
            return None;
        }
        self.warned = Some(Instant::now());
        error("slow down")
    }
}

async fn run(
    state: &AppState,
    room: &Room,
    conn: u64,
    user: &User,
    socket: &mut WebSocket,
    rx: &mut broadcast::Receiver<ServerMsg>,
    ends: Ends,
) -> anyhow::Result<()> {
    let Ends {
        mut kicked,
        token_expires,
    } = ends;
    let idle = state.hub.timing.idle;
    let mut last_heard = tokio::time::Instant::now();
    let mut bucket = Bucket::new();
    let mut keepalive = tokio::time::interval(KEEPALIVE);
    keepalive.tick().await;
    loop {
        tokio::select! {
            incoming = socket.recv() => {
                let Some(incoming) = incoming else { return Ok(()) };
                // Too big, or broken: the connection ends.
                let incoming = incoming?;
                // Any frame counts as a sign of life, pongs to our keepalive included.
                last_heard = tokio::time::Instant::now();
                match incoming {
                    Message::Text(text) => {
                        let reply = match serde_json::from_str::<ClientMsg>(&text) {
                            Ok(msg) if msg.is_limited() && !bucket.take() => bucket.slow_down(),
                            Ok(msg) => handle(state, room, conn, user, msg).await,
                            Err(_) => error("unknown message"),
                        };
                        if let Some(reply) = reply {
                            send(socket, &reply).await?;
                        }
                    }
                    Message::Close(_) => return Ok(()),
                    _ => {}
                }
            }
            out = rx.recv() => match out {
                Ok(msg) => send(socket, &msg).await?,
                // Fell behind, and the oldest messages waiting were lost (the rest still
                // follow): send the current state and presence now, and have the show
                // fetched again in case a change was among them. Older states still in the
                // backlog are ignored for their seq.
                Err(broadcast::error::RecvError::Lagged(_)) => {
                    let (live, presence) = {
                        let s = room.lock();
                        (s.live.clone(), s.presence())
                    };
                    send(socket, &ServerMsg::State { state: live }).await?;
                    send(socket, &ServerMsg::Presence { presence }).await?;
                    send(socket, &ServerMsg::ShowChanged).await?;
                }
                Err(broadcast::error::RecvError::Closed) => return Ok(()),
            },
            kick = &mut kicked => return match kick {
                Ok(Kick::ShowOver) => {
                    send(socket, &ServerMsg::ShowOver).await?;
                    close(socket, CLOSE_SHOW_OVER, "the show is over").await
                }
                Ok(Kick::NotAllowed) => refuse(socket, CLOSE_NOT_ALLOWED, "not allowed").await,
                // The room let go of this connection without saying why: just go.
                Err(_) => Ok(()),
            },
            _ = tokio::time::sleep_until(token_expires) => {
                return refuse(socket, CLOSE_BAD_TOKEN, "token expired").await;
            }
            _ = tokio::time::sleep_until(last_heard + idle) => {
                return refuse(socket, CLOSE_IDLE, "nothing heard for too long").await;
            }
            _ = keepalive.tick() => socket.send(Message::Ping(Default::default())).await?,
        }
    }
}

fn error(message: impl Into<String>) -> Option<ServerMsg> {
    Some(ServerMsg::Error {
        message: message.into(),
    })
}

/// One message from one connection. Returns a reply for that connection only; anything
/// for everyone goes out on the room's channel.
async fn handle(
    state: &AppState,
    room: &Room,
    conn: u64,
    user: &User,
    msg: ClientMsg,
) -> Option<ServerMsg> {
    let pool = &state.pool;
    match msg {
        ClientMsg::Hello { .. } => error("already said hello"),
        ClientMsg::Ping { client_ms, rtt_ms } => {
            if let Some(rtt) = rtt_ms.filter(|r| r.is_finite() && *r >= 0.0)
                && let Some(c) = room.lock().conns.get_mut(&conn)
            {
                c.rtt_ms = rtt;
            }
            Some(ServerMsg::Pong {
                client_ms,
                server_ms: now_ms(),
            })
        }
        ClientMsg::Load { clip_id, start_at } => {
            let _turn = match steer_turn(pool, room).await {
                Ok(turn) => turn,
                Err(why) => return error(why),
            };
            let duration = match playable(pool, room.id, clip_id).await {
                Ok(Ok(duration)) => duration,
                Ok(Err(why)) => return error(why),
                Err(e) => return internal(e),
            };
            let live = change(room, |live, lead| {
                let now = now_ms();
                // A start further ahead is a browser whose clock is off (or a bad client): the
                // clip would count as played long before anyone sees it, and not end in
                // time for a takeover.
                let at = match start_at {
                    None => now,
                    Some(t) if t.is_finite() && t <= now + MAX_START_AHEAD_MS => t.max(now + lead),
                    Some(_) => return Err("start it at most 5 s ahead"),
                };
                live.clip_id = Some(clip_id);
                live.duration_ms = Some(duration);
                live.position_ms = 0.0;
                live.playing = start_at.is_some();
                live.at_server_ms = at;
                Ok(())
            });
            commit(pool, room, user.id, live).await
        }
        ClientMsg::Play => {
            let _turn = match steer_turn(pool, room).await {
                Ok(turn) => turn,
                Err(why) => return error(why),
            };
            let (clip, playing) = {
                let s = room.lock();
                (s.live.clip_id, s.live.playing)
            };
            let Some(clip) = clip else {
                return error("no clip on");
            };
            // Already playing: starting it again would make everyone wait for nothing.
            if playing {
                return None;
            }
            // It may have been dropped, put in the trash or deleted since it was loaded:
            // then it comes off for everyone instead.
            match playable(pool, room.id, clip).await {
                Ok(Ok(_)) => {}
                Ok(Err(why)) => {
                    unload(pool, room).await;
                    return error(why);
                }
                Err(e) => return internal(e),
            }
            let live = change(room, |live, lead| {
                let now = now_ms();
                live.position_ms = live.position_at(now);
                live.playing = true;
                live.at_server_ms = now + lead;
                Ok(())
            });
            commit(pool, room, user.id, live).await
        }
        ClientMsg::Pause => {
            steer(pool, room, user, |live, _| {
                let now = now_ms();
                live.position_ms = live.position_at(now);
                live.playing = false;
                live.at_server_ms = now;
                Ok(())
            })
            .await
        }
        ClientMsg::Seek { position_ms } => {
            steer(pool, room, user, |live, lead| {
                if !position_ms.is_finite() {
                    return Err("bad position");
                }
                let now = now_ms();
                let max = live.duration_ms.unwrap_or(f64::MAX);
                live.position_ms = position_ms.clamp(0.0, max);
                live.at_server_ms = if live.playing { now + lead } else { now };
                Ok(())
            })
            .await
        }
        ClientMsg::React {
            clip_id,
            emoji,
            at_ms,
        } => match shows::react(pool, room.id, user.id, clip_id, &emoji, at_ms).await {
            Ok(()) => {
                room.send(ServerMsg::Reaction {
                    user_id: user.id,
                    clip_id,
                    emoji,
                    at_ms,
                });
                None
            }
            Err(e) => error(e.to_string()),
        },
        ClientMsg::Ready { ready } => match shows::set_ready(pool, room.id, user.id, ready).await {
            Ok(()) => {
                room.send(ServerMsg::ShowChanged);
                None
            }
            Err(e) => error(e.to_string()),
        },
        ClientMsg::TakeOver => {
            let takeover_after_ms = state.hub.timing.takeover_after.as_secs_f64() * 1000.0;
            let (allowed, old_host) = {
                let s = room.lock();
                let now = now_ms();
                let away_long_enough = !s.host_online()
                    && s.host_away_since
                        .is_some_and(|t| now - t >= takeover_after_ms);
                // A clip of unknown length can't be seen to end: the minute away is enough.
                let clip_done =
                    !s.live.playing || s.live.duration_ms.is_none() || s.live.ended_at(now);
                (s.host != user.id && away_long_enough && clip_done, s.host)
            };
            if !allowed {
                return error(
                    "the host can only be taken over a minute after they leave, once the clip ends",
                );
            }
            // Only from the host we saw leave, so of two people taking over at once, only
            // the first wins (in the database, and below in memory).
            match shows::set_host(pool, room.id, old_host, user.id).await {
                Ok(true) => {}
                Ok(false) => return error("someone else took over first"),
                Err(e) => return internal(e),
            }
            let presence = {
                let mut s = room.lock();
                if s.host != old_host {
                    return error("someone else took over first");
                }
                s.host = user.id;
                s.host_away_since = None;
                s.presence()
            };
            tracing::info!(show = %room.id, host = %user.id, "host taken over");
            room.send(ServerMsg::Presence { presence });
            room.send(ServerMsg::ShowChanged);
            None
        }
    }
}

fn internal(e: impl std::fmt::Display) -> Option<ServerMsg> {
    tracing::error!(error = %e, "show hub");
    error("something went wrong")
}

/// A turn to change the live state, or why not: anyone in the room steers (decision 57;
/// connecting joins the show), but only while the show is live (not in the lobby or the
/// finale).
///
/// Changes go one at a time, each from its checks to its broadcast, so nothing lands
/// between a change's checks and the state it sends out. This is the one lock held across
/// awaits; the room's own lock never is.
async fn steer_turn<'a>(
    pool: &PgPool,
    room: &'a Room,
) -> Result<tokio::sync::MutexGuard<'a, ()>, &'static str> {
    // The status is checked once it's this change's turn: the show may move on to the
    // finale while it waits.
    let turn = room.steering.lock().await;
    match shows::get(pool, room.id).await {
        Ok(Some(show)) if show.status == ShowStatus::Live => Ok(turn),
        Ok(_) => Err("only while the show is live"),
        Err(e) => {
            tracing::error!(error = %e, "show hub");
            Err("something went wrong")
        }
    }
}

/// The next live state: `f` changes a copy of the current one (given the lead play needs)
/// and `seq` goes up. The room keeps the current state until `commit` puts the new one
/// out.
fn change(
    room: &Room,
    f: impl FnOnce(&mut LiveState, f64) -> Result<(), &'static str>,
) -> Result<LiveState, &'static str> {
    let s = room.lock();
    let mut live = s.live.clone();
    f(&mut live, s.lead_ms())?;
    live.seq += 1;
    Ok(live)
}

async fn steer(
    pool: &PgPool,
    room: &Room,
    user: &User,
    f: impl FnOnce(&mut LiveState, f64) -> Result<(), &'static str>,
) -> Option<ServerMsg> {
    let _turn = match steer_turn(pool, room).await {
        Ok(turn) => turn,
        Err(why) => return error(why),
    };
    let live = change(room, f);
    commit(pool, room, user.id, live).await
}

/// Puts a new state out, with the steering turn held: counts the clip as played once it
/// plays for everyone, stamps the time, saves it, and only then makes it the room's state
/// and sends it to everyone. So the welcome, the saved copy and the broadcast only ever
/// carry it in its final form. If the clip can't be counted nothing changes, and whoever
/// asked gets an error.
async fn commit(
    pool: &PgPool,
    room: &Room,
    by: Uuid,
    live: Result<LiveState, &'static str>,
) -> Option<ServerMsg> {
    let mut live = match live {
        Ok(live) => live,
        Err(message) => return error(message),
    };
    if live.playing
        && let Some(clip) = live.clip_id
        && let Err(e) = shows::mark_played(pool, room.id, by, clip).await
    {
        tracing::warn!(show = %room.id, clip = %clip, error = %e, "marking the clip played");
        return error("couldn't count the clip as played, so it didn't start");
    }
    // Stamped after counting it, so that write's time doesn't eat into the lead everyone
    // needs to start together. The save that follows is one quick write.
    let now = now_ms();
    live.at_server_ms = if live.playing {
        live.at_server_ms.max(now + room.lock().lead_ms())
    } else {
        now
    };
    // Saved before it goes out, so a restart resumes from what everyone last saw. A save
    // never replaces a newer one.
    if let Ok(json) = serde_json::to_value(&live)
        && let Err(e) = shows::save_newer_live_state(pool, room.id, live.seq as i64, &json).await
    {
        tracing::warn!(error = %e, "saving the show's live state");
    }
    room.lock().live = live.clone();
    room.send(ServerMsg::State { state: live });
    None
}

/// The clip's length if it can go on: in the show's lineup and not dropped, ready, and
/// not in the trash. Otherwise why not.
async fn playable(
    pool: &PgPool,
    show: Uuid,
    clip: Uuid,
) -> sqlx::Result<Result<f64, &'static str>> {
    let lineup = shows::lineup(pool, show).await?;
    if !lineup.iter().any(|c| c.clip_id == clip && !c.dropped) {
        return Ok(Err("that clip isn't in the lineup"));
    }
    // `get` skips clips in the trash.
    let Some(c) = clips::get(pool, clip).await? else {
        return Ok(Err("that clip is in the trash"));
    };
    if c.status != ClipStatus::Ready {
        return Ok(Err("that clip isn't ready to play"));
    }
    // A ready clip always has its length (the transcode stores it); without it nobody
    // could tell when the clip ends.
    Ok(c.duration_ms
        .map(f64::from)
        .ok_or("that clip's length is unknown"))
}

/// Takes the clip off for everyone (`clipId: null`, paused), with the steering turn held.
async fn unload(pool: &PgPool, room: &Room) {
    let host = room.lock().host;
    let live = change(room, |live, _| {
        live.clip_id = None;
        live.duration_ms = None;
        live.position_ms = 0.0;
        live.playing = false;
        live.at_server_ms = now_ms();
        Ok(())
    });
    commit(pool, room, host, live).await;
}

/// Takes the clip that's on off for everyone if it can't play any more: dropped from the
/// lineup, put in the trash or deleted.
async fn unload_if_gone(pool: &PgPool, room: &Room) {
    if room.lock().live.clip_id.is_none() {
        return;
    }
    let _turn = room.steering.lock().await;
    let Some(clip) = room.lock().live.clip_id else {
        return;
    };
    match playable(pool, room.id, clip).await {
        Ok(Ok(_)) => {}
        Ok(Err(why)) => {
            tracing::info!(show = %room.id, clip = %clip, why, "the clip on can't play any more");
            unload(pool, room).await;
        }
        Err(e) => tracing::warn!(show = %room.id, error = %e, "checking the clip on"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn position_follows_the_clock_from_a_scheduled_start() {
        let s = LiveState {
            seq: 1,
            clip_id: None,
            playing: true,
            position_ms: 1_000.0,
            at_server_ms: 10_000.0,
            rate: 1.0,
            duration_ms: Some(5_000.0),
        };
        // Before the scheduled start it holds; after, it runs; it stops at the end.
        assert_eq!(s.position_at(9_000.0), 1_000.0);
        assert_eq!(s.position_at(12_000.0), 3_000.0);
        assert_eq!(s.position_at(30_000.0), 5_000.0);
        assert!(s.ended_at(30_000.0));
        let paused = LiveState {
            playing: false,
            ..s
        };
        assert_eq!(paused.position_at(30_000.0), 1_000.0);
    }

    #[test]
    fn messages_in_the_wire_format() {
        let msg: ClientMsg = serde_json::from_str(r#"{"type":"seek","positionMs":1500}"#).unwrap();
        assert!(matches!(msg, ClientMsg::Seek { position_ms } if position_ms == 1500.0));
        let msg: ClientMsg = serde_json::from_str(r#"{"type":"play"}"#).unwrap();
        assert!(matches!(msg, ClientMsg::Play));
        let out = serde_json::to_value(ServerMsg::Reaction {
            user_id: Uuid::nil(),
            clip_id: Uuid::nil(),
            emoji: "🔥".into(),
            at_ms: 1200,
        })
        .unwrap();
        assert_eq!(out["type"], "reaction");
        assert_eq!(out["userId"], Uuid::nil().to_string());
        assert_eq!(out["atMs"], 1200);
        let out = serde_json::to_value(ServerMsg::ShowOver).unwrap();
        assert_eq!(out, serde_json::json!({ "type": "showOver" }));
    }

    #[test]
    fn a_connection_leaves_its_room_even_on_a_panic() {
        let host = Uuid::new_v4();
        let room = Room {
            id: Uuid::new_v4(),
            tx: broadcast::channel(8).0,
            state: Mutex::new(RoomState {
                live: LiveState::default(),
                host,
                conns: HashMap::from([(
                    1,
                    Conn {
                        user: host,
                        rtt_ms: 100.0,
                        kick: None,
                    },
                )]),
                empty_since: None,
                had_people: true,
                host_away_since: None,
                over: false,
            }),
            steering: tokio::sync::Mutex::new(()),
        };
        let mut rx = room.tx.subscribe();
        let panicked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _leave = Leave {
                room: &room,
                conn: 1,
            };
            // Poisons the lock on the way out, too.
            let _s = room.lock();
            panic!("boom");
        }));
        assert!(panicked.is_err());
        let s = room.state.lock().unwrap_or_else(PoisonError::into_inner);
        assert!(s.conns.is_empty());
        assert!(s.empty_since.is_some() && s.host_away_since.is_some());
        assert!(
            matches!(rx.try_recv(), Ok(ServerMsg::Presence { presence }) if presence.online.is_empty())
        );
    }

    /// An empty room for a show `host` is hosting.
    fn room(host: Uuid) -> Room {
        Room {
            id: Uuid::new_v4(),
            tx: broadcast::channel(8).0,
            state: Mutex::new(RoomState {
                live: LiveState::default(),
                host,
                conns: HashMap::new(),
                empty_since: Some(Instant::now()),
                had_people: false,
                host_away_since: None,
                over: false,
            }),
            steering: tokio::sync::Mutex::new(()),
        }
    }

    #[test]
    fn a_clip_of_unknown_length_plays_on() {
        let s = LiveState {
            playing: true,
            position_ms: 500.0,
            at_server_ms: 1_000.0,
            duration_ms: None,
            ..LiveState::default()
        };
        assert_eq!(s.position_at(3_600_000.0), 3_599_500.0);
        assert!(!s.ended_at(3_600_000.0));
    }

    #[test]
    fn joining_starts_the_takeover_clock_when_nobody_saw_the_host_go() {
        let host = Uuid::new_v4();
        let room = room(host);
        let mut s = room.lock();
        // The host isn't here, and nobody saw them leave: a friend's arrival starts it.
        let (_, presence) = s.join(1, Uuid::new_v4(), oneshot::channel().0).unwrap();
        let since = presence.host_away_since;
        assert!(since.is_some());
        assert!(s.had_people && s.empty_since.is_none());
        // The next friend doesn't restart it.
        let (_, presence) = s.join(2, Uuid::new_v4(), oneshot::channel().0).unwrap();
        assert_eq!(presence.host_away_since, since);
        // The host's arrival stops it.
        let (_, presence) = s.join(3, host, oneshot::channel().0).unwrap();
        assert_eq!(presence.host_away_since, None);
        assert_eq!(presence.online.len(), 3);
    }

    #[test]
    fn nobody_joins_a_show_thats_over() {
        let host = Uuid::new_v4();
        let room = room(host);
        room.finish();
        let (kick, mut kicked) = oneshot::channel();
        assert!(room.lock().join(1, host, kick).is_none());
        assert!(room.lock().conns.is_empty());
        // Nothing holds on to the connection.
        assert!(matches!(
            kicked.try_recv(),
            Err(oneshot::error::TryRecvError::Closed)
        ));
    }

    #[test]
    fn closing_a_room_sends_everyone_in_it_home() {
        let host = Uuid::new_v4();
        let open = Arc::new(room(host));
        let (kick, mut kicked) = oneshot::channel();
        open.lock().join(1, host, kick).unwrap();
        let rooms: Rooms = Mutex::new(HashMap::from([(open.id, open.clone())]));

        // Another show (or one already closed): nothing happens.
        close_room(&rooms, Uuid::new_v4());
        assert!(rooms.lock().unwrap().contains_key(&open.id));
        assert!(kicked.try_recv().is_err());
        assert!(!open.lock().over);

        close_room(&rooms, open.id);
        assert!(rooms.lock().unwrap().is_empty());
        assert!(matches!(kicked.try_recv(), Ok(Kick::ShowOver)));
        assert!(open.lock().over);
    }

    #[test]
    fn a_connection_that_already_left_changes_nothing() {
        let host = Uuid::new_v4();
        let room = room(host);
        room.lock().join(1, host, oneshot::channel().0).unwrap();
        let mut rx = room.tx.subscribe();
        drop(Leave {
            room: &room,
            conn: 2,
        });
        assert!(rx.try_recv().is_err(), "no presence sent");
        let s = room.lock();
        assert_eq!(s.conns.len(), 1);
        assert_eq!(s.host_away_since, None);
    }

    #[tokio::test]
    async fn a_hub_runs_on_the_production_timing() {
        let pool = PgPool::connect_lazy("postgres://localhost/unused").unwrap();
        let hub = Hub::new(pool);
        let t = hub.timing;
        assert_eq!(
            (t.hello, t.idle, t.takeover_after, t.abandon_after),
            (
                Duration::from_secs(5),
                Duration::from_secs(75),
                Duration::from_secs(60),
                Duration::from_secs(15 * 60)
            )
        );
        assert!(hub.rooms.lock().unwrap().is_empty());
    }

    #[test]
    fn the_bucket_allows_a_burst_then_the_rate() {
        let mut bucket = Bucket::new();
        let taken = (0..30).filter(|_| bucket.take()).count();
        assert_eq!(taken, BURST as usize);
        // Only one "slow down" a second.
        assert!(bucket.slow_down().is_some());
        assert!(bucket.slow_down().is_none());
        // A tenth of a second later, about one more.
        bucket.at -= Duration::from_millis(100);
        assert!(bucket.take());
        assert!(!bucket.take());
    }
}
