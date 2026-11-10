//! The Office (docs/office.md): what the app asks its daemon (`office.*` methods, `office` events)
//! and what a daemon and the office server say to each other over WebSocket.
//!
//! Only presence crosses the network: a member's name, office, position, and for each of their
//! agents its kind, status and task. Never a conversation, a prompt, code or a file name.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::model::{AgentKind, ChatStatus};

/// Bumped on any breaking change between daemons and the office server.
pub const OFFICE_PROTOCOL_VERSION: u32 = 1;

/// How long a piece of scenery someone cleared (R) takes to grow back.
pub const OFFICE_REGROW_MS: u64 = 20 * 60 * 1000;

/// How long a member who just took a place in the Office (opened their 3D view, woke up) can
/// neither be struck nor strike.
pub const OFFICE_WAKE_GRACE_MS: u64 = 5000;

/// What each point of attack speed adds to how often a character strikes, in percent of its
/// class's pace: never capped, the time between blows only ever shrinks towards nothing.
pub const OFFICE_SPEED_STEP: u64 = 5;

/// How long an agent idle with nothing new stays awake: past it, it rests, and past a quarter's
/// twelve houses, those whose agents all rest give way to the busy ones.
pub const OFFICE_REST_MS: u64 = 30 * 60 * 1000;

/// The last level of a character's weapon and of its armour.
pub const OFFICE_GEAR_MAX: u32 = 10;

/// A piece of scenery's id, as app/src/lib/officeWorld.ts names it (`n:3:12`, `lantern:1`): a short
/// word of letters, digits and colons, nothing else.
pub fn valid_scenery(id: &str) -> bool {
    !id.is_empty() && id.len() <= 32 && id.chars().all(|c| c.is_ascii_alphanumeric() || c == ':')
}

/// What the plaza's jukebox may play: the https address of an audio file or stream, or of a page
/// yt-dlp reads (a YouTube video…), in ASCII (the
/// app sends a URL as `URL.href` writes it), no longer than a long link. Cleartext http is refused:
/// the app's content security policy lets media come over https only.
pub fn valid_jukebox_url(url: &str) -> bool {
    url.len() <= 2048
        && url.chars().all(|c| c.is_ascii_graphic())
        && url
            .strip_prefix("https://")
            .is_some_and(|rest| !rest.is_empty() && !rest.starts_with('/'))
}

/// Who sees the user in the Office.
#[derive(Serialize, Deserialize, TS, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum OfficeVisibility {
    /// The user sees the others; nobody sees them.
    #[default]
    Invisible,
    /// Their character only.
    Present,
    /// Their character and their agents, with each agent's task.
    WithAgents,
}

/// A member's looping dance, carried with their position for everyone who sees them.
#[derive(Serialize, Deserialize, TS, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum OfficeDance {
    Floss,
    OrangeJustice,
    SixSeven,
}

/// A character on the floor, in metres (x to the east, z to the south) and radians.
#[derive(Serialize, Deserialize, TS, Clone, Copy, Debug, PartialEq)]
#[ts(export)]
pub struct OfficePosition {
    pub x: f32,
    pub z: f32,
    /// Height off the floor, while jumping.
    #[serde(default)]
    pub y: f32,
    pub heading: f32,
    pub walking: bool,
    /// Standing in their own quarter: they can neither strike nor be struck there.
    #[serde(default)]
    pub safe: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub dance: Option<OfficeDance>,
}

/// A workspace's house in the village, from its pull requests: foundations while none is open,
/// walls once one is, the roof once every open one's checks pass, finished once all are merged.
#[derive(Serialize, Deserialize, TS, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum HouseStage {
    Foundations,
    Walls,
    Roof,
    Finished,
}

impl HouseStage {
    /// From a workspace's PRs: their `state` (`OPEN`, `MERGED`, `CLOSED`) and combined `checks`.
    pub fn of<'a>(prs: impl IntoIterator<Item = (&'a str, Option<&'a str>)>) -> HouseStage {
        let known: Vec<_> = prs
            .into_iter()
            .filter(|(state, _)| *state != "CLOSED")
            .collect();
        if known.is_empty() {
            return HouseStage::Foundations;
        }
        let open: Vec<_> = known.iter().filter(|(state, _)| *state == "OPEN").collect();
        if open.is_empty() {
            HouseStage::Finished
        } else if open.iter().all(|(_, checks)| *checks == Some("SUCCESS")) {
            HouseStage::Roof
        } else {
            HouseStage::Walls
        }
    }
}

/// One of a member's agents, as published: the chat id is hashed with a salt of that member's
/// daemon, so that it cannot be matched with anything else.
#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq)]
#[ts(export)]
pub struct OfficeAgent {
    pub id: String,
    pub agent: AgentKind,
    pub status: ChatStatus,
    /// The workspace's name, with its first Linear ticket's id.
    pub task: String,
    /// How far its workspace's pull requests are, as the member's daemon last read them: the
    /// house's stage in the village. None before it read them, or from an older daemon.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub stage: Option<HouseStage>,
    /// How far the agent is through its plan, numbers only (never the steps' text).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub progress: Option<crate::model::ChatProgress>,
    /// Its workspace's island of desks: the workspace id hashed with the same salt, shared by the
    /// workspace's chats. None from a daemon that predates islands.
    #[serde(default)]
    pub island: Option<String>,
    /// The island of its workspace's parent, when that parent shows too: its workshop then has a
    /// hut attached. None from a daemon that predates it.
    #[serde(default)]
    pub parent: Option<String>,
    /// Idle with nothing new for `OFFICE_REST_MS` at least. False from a daemon that predates it.
    #[serde(default)]
    pub resting: bool,
}

/// A member of the Office as the server knows them.
#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq)]
#[ts(export)]
pub struct OfficeMember {
    pub email: String,
    /// First name, on their nameplate and above their character.
    pub name: String,
    /// Their office's place in the world, from 0.
    pub office: u32,
    /// None until they first moved, and for a member who is invisible.
    #[serde(default)]
    pub position: Option<OfficePosition>,
    #[serde(default)]
    pub agents: Vec<OfficeAgent>,
    /// They open their microphone and camera to every colleague in the Office who does too, and see theirs.
    #[serde(default)]
    pub media: bool,
    /// The villager outfit they chose; None for the default one, and from older daemons or servers.
    #[serde(default)]
    pub skin: Option<String>,
    /// Their level and fighting stats; None from older daemons or servers, who cannot be hit.
    #[serde(default)]
    pub stats: Option<OfficeStats>,
}

/// A character's class, chosen once: it gives their weapon, the name of their attack stat, how far
/// and how hard they strike, and how often.
#[derive(Serialize, Deserialize, TS, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum OfficeClass {
    /// A sword, strength: heavy blows at arm's length.
    Warrior,
    /// A staff, intelligence: the heaviest blows, from afar, but slow to cast.
    Mage,
    /// A bow, agility: light blows, from the farthest.
    Archer,
}

impl OfficeClass {
    /// How far its blows reach, in metres.
    pub fn reach(self) -> f32 {
        match self {
            OfficeClass::Warrior => 2.4,
            OfficeClass::Mage => 7.5,
            OfficeClass::Archer => 9.5,
        }
    }

    /// What its blows take, times the attack stat less half the defense.
    pub fn might(self) -> f32 {
        match self {
            OfficeClass::Warrior => 1.3,
            OfficeClass::Mage => 1.6,
            OfficeClass::Archer => 0.7,
        }
    }

    /// The least time between two of its blows, in milliseconds.
    pub fn interval_ms(self) -> u64 {
        match self {
            OfficeClass::Warrior => 400,
            OfficeClass::Mage => 850,
            OfficeClass::Archer => 450,
        }
    }

    /// The least time between two of its blows with `speed` points of attack speed: each point
    /// strikes `OFFICE_SPEED_STEP` percent more often.
    pub fn pace_ms(self, speed: u32) -> u64 {
        self.interval_ms() * 100 / (100 + OFFICE_SPEED_STEP * u64::from(speed))
    }
}

/// A character's level and what it fights with, as colleagues see it.
#[derive(Serialize, Deserialize, TS, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[ts(export)]
pub struct OfficeStats {
    pub level: u32,
    /// Health points left, out of `max_hp`.
    pub hp: u32,
    pub max_hp: u32,
    /// The attack stat: the warrior's strength, the mage's intelligence, the archer's agility.
    pub power: u32,
    pub defense: u32,
    /// The points spent on attack speed (`OfficeClass::pace_ms`); None from daemons that predate
    /// it: none.
    #[serde(default)]
    pub speed: Option<u32>,
    /// None until the user chooses, and from daemons that predate classes: a warrior then.
    #[serde(default)]
    pub class: Option<OfficeClass>,
    /// The gold the character has, all of which goes to who knocks it out. None from daemons
    /// that predate it: nothing to take.
    #[serde(default)]
    #[ts(type = "number | null")]
    pub gold: Option<u64>,
    /// The levels of its weapon and armour, which a colleague sees by clicking the character.
    /// None from daemons that predate it.
    #[serde(default)]
    pub weapon: Option<u32>,
    #[serde(default)]
    pub armour: Option<u32>,
    /// The colleagues it knocked out, the times it was knocked out, and the gold it took from those
    /// it knocked out, since the character was first made (a knock-out keeps them): the
    /// leaderboard's columns. None from daemons that predate it.
    #[serde(default)]
    pub kills: Option<u32>,
    #[serde(default)]
    pub deaths: Option<u32>,
    #[serde(default)]
    #[ts(type = "number | null")]
    pub looted: Option<u64>,
}

/// The user's own character: its stats, the experience its agents earned it, and the points
/// they have to spend since they last levelled up.
#[derive(Serialize, Deserialize, TS, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[ts(export)]
pub struct OfficeHero {
    pub stats: OfficeStats,
    #[ts(type = "number")]
    pub xp: u64,
    /// The experience the current level started at, and the one the next one starts at.
    #[ts(type = "number")]
    pub level_xp: u64,
    #[ts(type = "number")]
    pub next_xp: u64,
    /// Stat points earned by levelling up and not spent yet.
    pub points: u32,
    /// When health starts coming back, in milliseconds since the Unix epoch on this Mac's clock:
    /// five minutes after the last blow taken or landed. None at full health.
    #[serde(default)]
    #[ts(type = "number | null")]
    pub healing_at: Option<u64>,
    /// Gold earned by agents and dancing, plus loot from colleagues, not spent yet.
    #[serde(default)]
    #[ts(type = "number")]
    pub gold: u64,
    /// Its weapon, which adds to the attack stat, and its armour, which adds to defense.
    #[serde(default)]
    pub weapon: OfficeGear,
    #[serde(default)]
    pub armour: OfficeGear,
}

/// A piece of the character's equipment: its level, bought with gold, and what it adds to its
/// stat (the weapon to the attack stat, the armour to defense).
#[derive(Serialize, Deserialize, TS, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[ts(export)]
pub struct OfficeGear {
    pub level: u32,
    pub bonus: u32,
    /// What the next level costs and adds in all; None at the last level.
    pub next: Option<OfficeGearLevel>,
}

#[derive(Serialize, Deserialize, TS, Clone, Copy, Debug, PartialEq, Eq)]
#[ts(export)]
pub struct OfficeGearLevel {
    #[ts(type = "number")]
    pub price: u64,
    pub bonus: u32,
}

/// Which piece of equipment the user upgrades.
#[derive(Serialize, Deserialize, TS, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum OfficeGearKind {
    Weapon,
    Armour,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq, Default)]
#[serde(tag = "state", rename_all = "snake_case")]
#[ts(export)]
pub enum OfficeConnection {
    /// No Office view is open, or the user has not signed in.
    #[default]
    Offline,
    Connecting,
    Online,
    /// The last attempt failed; the daemon tries again while an Office view is open.
    Failed {
        message: String,
    },
}

/// What the Office view shows of the network side.
#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq, Default)]
#[ts(export)]
pub struct OfficeState {
    /// An office server is configured in `settings.toml` (`office_url`).
    pub configured: bool,
    /// The account the user signed in with, if any.
    pub email: Option<String>,
    pub visibility: OfficeVisibility,
    /// Microphone and camera open to the colleagues in the Office, if they chose so too.
    #[serde(default)]
    pub media: bool,
    /// The villager outfit the user chose for their character; None for the default one.
    #[serde(default)]
    pub skin: Option<String>,
    pub connection: OfficeConnection,
    /// The user as the server placed them, once online.
    pub me: Option<OfficeMember>,
    /// The other members online and visible.
    pub members: Vec<OfficeMember>,
    /// The relays a call may go through when the two Macs cannot reach each other directly, as
    /// the office server gave them (short-lived credentials).
    #[serde(default)]
    pub ice_servers: Vec<IceServer>,
    /// The scenery cleared by the user or a colleague, until it grows back.
    #[serde(default)]
    pub cleared: Vec<ClearedScenery>,
    /// The user's character's level and stats.
    #[serde(default)]
    pub hero: OfficeHero,
    /// What the plaza's jukebox plays, while online and something does.
    #[serde(default)]
    pub jukebox: Option<Jukebox>,
}

/// What the plaza's jukebox plays, for everyone in the Office: each Mac that listens fetches and
/// plays it itself, at the same place in it.
#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq)]
#[ts(export)]
pub struct Jukebox {
    pub url: String,
    /// Playing, or paused at `position_ms`.
    pub playing: bool,
    /// How far into it the jukebox was at `at`.
    #[ts(type = "number")]
    pub position_ms: u64,
    /// When, in milliseconds since the Unix epoch, on this Mac's clock.
    #[ts(type = "number")]
    pub at: u64,
    /// The first name of the member who chose it.
    pub set_by: String,
}

/// What the office server says of the jukebox: Macs' clocks differ, so it gives how far into it
/// the jukebox is as it says so rather than since when it plays.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct OfficeJukebox {
    pub url: String,
    pub playing: bool,
    pub position_ms: u64,
    pub set_by: String,
}

/// What a member does at the jukebox.
#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq)]
#[serde(tag = "action", rename_all = "snake_case")]
#[ts(export)]
pub enum JukeboxCommand {
    /// Plays `url` from its start (`valid_jukebox_url`), or without one goes on with what is paused.
    Play {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        url: Option<String>,
    },
    Pause,
    /// Nothing plays any more.
    Stop,
}

/// An audio format the app's web view plays, for the daemon to pick a page's stream in.
#[derive(Serialize, Deserialize, TS, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum JukeboxFormat {
    /// AAC in MP4 (`audio/mp4; codecs="mp4a.40.2"`).
    M4a,
    /// Opus in WebM (`audio/webm; codecs="opus"`).
    Opus,
    /// MP3 (`audio/mpeg`), as Bandcamp has it.
    Mp3,
    /// AAC over HLS (`application/vnd.apple.mpegurl`), as SoundCloud has it: WebKit on macOS plays
    /// it, WebKitGTK when GStreamer's HLS plugin is installed.
    Hls,
}

/// A page the jukebox plays (a YouTube video, a SoundCloud track…), for this Mac's daemon to
/// find its audio stream with yt-dlp.
#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
pub struct JukeboxResolveParams {
    pub url: String,
    /// The formats the web view plays, the preferred first.
    pub formats: Vec<JukeboxFormat>,
}

/// A page's audio stream, found on this Mac.
#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq)]
#[ts(export)]
pub struct JukeboxStream {
    /// Valid for this Mac only (the site binds it to the address that asked) and for a few hours.
    pub url: String,
    /// The page's title, when yt-dlp gives one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub title: Option<String>,
}

/// A tree cut down, a rock rolled away…: gone from every Mac until `regrows_at`.
#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq)]
#[ts(export)]
pub struct ClearedScenery {
    /// Its id in the world (app/src/lib/officeWorld.ts).
    pub scenery: String,
    /// When it grows back, in milliseconds since the Unix epoch, on this Mac's clock.
    #[ts(type = "number")]
    pub regrows_at: u64,
}

/// What the office server says of a cleared piece of scenery: Macs' clocks differ, so it gives
/// how long until it grows back rather than when.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct OfficeCleared {
    pub scenery: String,
    pub regrows_in_ms: u64,
}

/// A STUN or TURN server for WebRTC, as `RTCIceServer` takes it.
#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq)]
#[ts(export)]
pub struct IceServer {
    pub urls: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub username: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub credential: Option<String>,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
pub struct OfficeVisibilityParams {
    pub visibility: OfficeVisibility,
}

/// A one-time link to the Office in the user's browser, which only works once and for a minute.
#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
pub struct OfficeBrowserLink {
    pub url: String,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
pub struct OfficeMediaParams {
    pub enabled: bool,
}

/// A few words for the colleagues near the user's character.
#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
pub struct OfficeSayParams {
    pub text: String,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
pub struct OfficeSkinParams {
    /// One of the outfits of app/src/lib/officeAvatars.ts; None goes back to the default one.
    pub skin: Option<String>,
}

/// A piece of scenery the user clears, standing next to it.
#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
pub struct OfficeClearParams {
    pub scenery: String,
}

/// Stat points the user spends, each on one stat: health, power, defense or attack speed.
#[derive(Serialize, Deserialize, TS, Clone, Copy, Debug, Default)]
#[ts(export)]
pub struct OfficeSpendParams {
    #[serde(default)]
    pub health: u32,
    #[serde(default)]
    pub power: u32,
    #[serde(default)]
    pub defense: u32,
    #[serde(default)]
    pub speed: u32,
}

/// A piece of equipment the user buys the next level of with their gold.
#[derive(Serialize, Deserialize, TS, Clone, Copy, Debug)]
#[ts(export)]
pub struct OfficeUpgradeParams {
    pub gear: OfficeGearKind,
}

#[derive(Serialize, Deserialize, TS, Clone, Copy, Debug)]
#[ts(export)]
pub struct OfficeClassParams {
    pub class: OfficeClass,
}

/// The colleague the user strikes, within their weapon's reach.
#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
pub struct OfficeAttackParams {
    pub email: String,
}

/// A sword's blow that landed, as the office server rolled it.
#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq)]
#[ts(export)]
pub struct OfficeHit {
    pub from: String,
    pub to: String,
    pub damage: u32,
    pub critical: bool,
    /// It took the last of `to`'s health: they start again at level 1, healed, at the plaza.
    pub ko: bool,
    /// With a knock-out, the gold `from` takes from `to`: all `to` had, as the office server last
    /// heard. 0 otherwise, and from a server that predates it.
    #[serde(default)]
    #[ts(type = "number")]
    pub loot: u64,
}

/// A blow the user struck at a training dummy, as their app rolled it: the colleagues near see it.
#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
pub struct OfficeStrikeDummyParams {
    /// Its id in the world (app/src/lib/officeWorld.ts), `dummy:3`.
    pub dummy: String,
    pub damage: u32,
    pub critical: bool,
}

/// A WebRTC message (offer, answer, ICE candidate) for one colleague, opaque to the daemon and
/// the office server: the call itself goes from one Mac to the other.
#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq)]
#[ts(export)]
pub struct OfficeSignalParams {
    pub to: String,
    pub signal: String,
}

/// The `office` scope of the event log: ephemeral, delivered only to subscriptions that ask for it.
#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
#[ts(export)]
pub enum OfficeEvent {
    /// Anything but a move: the whole state again.
    StateChanged { state: Box<OfficeState> },
    MemberMoved {
        email: String,
        position: OfficePosition,
    },
    /// A colleague's WebRTC message for this user.
    Signal { from: String, signal: String },
    /// A colleague near the user said something: a bubble above their character, for a moment.
    Said { email: String, text: String },
    /// A blow struck near the user, maybe by or at them: the swing and the damage show.
    Hit { hit: OfficeHit },
    /// A browser exchanged a link's code (`office.browser_link`): the Office opened there.
    BrowserJoined,
    /// Someone knocked someone out, anywhere in the Office: everyone reads it for a moment.
    KnockedOut { hit: OfficeHit },
    /// A colleague near the user struck a training dummy (`hit.to` is its id).
    DummyHit { hit: OfficeHit },
}

/// From a daemon to the office server.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ToOffice {
    /// First message of a connection.
    Hello {
        protocol: u32,
        /// A Google ID token of an account of the configured Workspace.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id_token: Option<String>,
        /// Accepted only by a server started with `--insecure-dev`, on loopback.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        dev_email: Option<String>,
        visibility: OfficeVisibility,
    },
    Visibility {
        visibility: OfficeVisibility,
    },
    Move {
        position: OfficePosition,
    },
    /// The member's 3D view closed: they stand nowhere, asleep by the fire for the others, until
    /// they move again; a server that predates it ignores it.
    Asleep,
    /// The full list of the member's published agents, sent when it changes.
    Agents {
        agents: Vec<OfficeAgent>,
    },
    /// Whether the member opens their microphone and camera to who comes near.
    Media {
        enabled: bool,
    },
    /// A WebRTC message for `to`, relayed only while both have media on.
    Signal {
        to: String,
        signal: String,
    },
    /// A few words for the members near the user's character (a speech bubble); not stored.
    Say {
        text: String,
    },
    /// The member's villager outfit; a server that predates skins ignores it.
    Skin {
        #[serde(default)]
        skin: Option<String>,
    },
    /// The member cleared a piece of scenery; a server that predates it ignores it.
    Clear {
        scenery: String,
    },
    /// The member's level and stats, sent when they change; a server that predates them ignores it.
    Stats {
        stats: OfficeStats,
        /// How many of the blows the server told this connection the member took (`Hit` at them)
        /// these stats count. The server keeps the health it took itself from blows they do not
        /// count yet; from daemons that predate it, the stats are taken as they come.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        blows: Option<u64>,
    },
    /// The member swings their sword at the colleague `target` (an e-mail) next to them.
    Attack {
        target: String,
    },
    /// The member struck a training dummy, their app rolling the blow; a server that predates it
    /// ignores it.
    StrikeDummy {
        dummy: String,
        damage: u32,
        critical: bool,
    },
    /// The member plays, pauses or stops the jukebox; a server that predates it ignores it.
    Jukebox {
        command: JukeboxCommand,
    },
}

/// From the office server to a daemon.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum FromOffice {
    /// The answer to `hello`: who the server says the user is, and who else is there.
    Welcome {
        you: OfficeMember,
        members: Vec<OfficeMember>,
        /// The server's TURN relay with credentials for this member, when it has one.
        #[serde(default)]
        ice_servers: Vec<IceServer>,
        /// The scenery cleared and not grown back yet.
        #[serde(default)]
        cleared: Vec<OfficeCleared>,
        /// What the jukebox plays, if anything.
        #[serde(default)]
        jukebox: Option<OfficeJukebox>,
    },
    /// A member became visible, or changed what they show.
    Member {
        member: OfficeMember,
    },
    Left {
        email: String,
    },
    Moved {
        email: String,
        position: OfficePosition,
    },
    /// The connection is refused; the server closes it after this.
    Refused {
        message: String,
    },
    Signal {
        from: String,
        signal: String,
    },
    /// What a member near this one said.
    Said {
        email: String,
        text: String,
    },
    /// Someone cleared a piece of scenery.
    Cleared(OfficeCleared),
    /// A blow landed near this member, maybe by or at them.
    Hit(OfficeHit),
    /// A member knocked another out, told to every member connected.
    KnockedOut(OfficeHit),
    /// A member near this one struck a training dummy (`to` is its id).
    DummyHit(OfficeHit),
    /// Someone played, paused or stopped the jukebox: what it plays now, None once stopped.
    Jukebox {
        #[serde(default)]
        jukebox: Option<OfficeJukebox>,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn positions_without_a_dance_still_read_and_dances_round_trip() {
        let old = r#"{"x":0,"z":0,"heading":0,"walking":false}"#;
        let mut position: OfficePosition = serde_json::from_str(old).unwrap();
        assert_eq!(position.dance, None);
        assert!(
            serde_json::to_value(position)
                .unwrap()
                .get("dance")
                .is_none()
        );
        position.dance = Some(OfficeDance::SixSeven);
        let json = serde_json::to_value(position).unwrap();
        assert_eq!(json["dance"], "six_seven");
        assert_eq!(
            serde_json::from_value::<OfficePosition>(json).unwrap(),
            position
        );
    }

    #[test]
    fn each_point_of_attack_speed_strikes_more_often_without_a_cap() {
        assert_eq!(OfficeClass::Warrior.pace_ms(0), 400);
        assert_eq!(
            OfficeClass::Warrior.pace_ms(10),
            266,
            "half as many blows again"
        );
        assert_eq!(OfficeClass::Warrior.pace_ms(20), 200, "twice as many");
        assert!(OfficeClass::Mage.pace_ms(100) < OfficeClass::Mage.pace_ms(99));
        assert!(OfficeClass::Mage.pace_ms(1000) > 0);
    }

    #[test]
    fn a_house_grows_with_its_pull_requests() {
        assert_eq!(HouseStage::of([]), HouseStage::Foundations);
        assert_eq!(HouseStage::of([("CLOSED", None)]), HouseStage::Foundations);
        assert_eq!(
            HouseStage::of([("OPEN", Some("PENDING"))]),
            HouseStage::Walls
        );
        assert_eq!(
            HouseStage::of([("OPEN", Some("SUCCESS")), ("MERGED", None)]),
            HouseStage::Roof
        );
        assert_eq!(
            HouseStage::of([("MERGED", None), ("CLOSED", None)]),
            HouseStage::Finished
        );
    }

    #[test]
    fn the_jukebox_takes_an_https_address_and_nothing_else() {
        assert!(valid_jukebox_url(
            "https://ice.somafm.com/groovesalad-128-mp3"
        ));
        assert!(valid_jukebox_url("https://example.com/a%20podcast.mp3?t=1"));
        for refused in [
            "http://radio.example.com/stream",
            "https://",
            "https:///path",
            "javascript:alert(1)",
            "file:///etc/passwd",
            "https://example.com/a podcast.mp3",
            "https://exämple.com/a.mp3",
        ] {
            assert!(!valid_jukebox_url(refused), "{refused}");
        }
        assert!(!valid_jukebox_url(&format!(
            "https://e.com/{}",
            "a".repeat(2048)
        )));
    }

    #[test]
    fn a_jukebox_command_is_tagged_by_its_action() {
        let play = serde_json::to_value(JukeboxCommand::Play {
            url: Some("https://e.com/a.mp3".into()),
        })
        .unwrap();
        assert_eq!(play["action"], "play");
        let resume: JukeboxCommand = serde_json::from_str(r#"{"action":"play"}"#).unwrap();
        assert_eq!(resume, JukeboxCommand::Play { url: None });
    }

    #[test]
    fn office_messages_are_tagged_by_type_in_snake_case() {
        let hello = ToOffice::Hello {
            protocol: OFFICE_PROTOCOL_VERSION,
            id_token: None,
            dev_email: Some("carol@example.com".into()),
            visibility: OfficeVisibility::WithAgents,
        };
        let json = serde_json::to_value(&hello).unwrap();
        assert_eq!(json["type"], "hello");
        assert_eq!(json["visibility"], "with_agents");
        assert!(json.get("id_token").is_none());
        let back: ToOffice = serde_json::from_value(json).unwrap();
        assert_eq!(back, hello);
    }
}
