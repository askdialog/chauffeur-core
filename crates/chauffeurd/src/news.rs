//! The Office's news kiosk (docs/office.md): Hacker News's top stories and the user's own RSS and
//! Atom feeds (`[news]` of settings.toml), fetched by the daemon since the app's web view reaches
//! no other site. A source is fetched again when the kiosk is read and its last fetch is 15
//! minutes old; what was fetched stays in memory, what the user read in the store. Nothing of it
//! goes to the office server: colleagues never see a member's feeds, reads or unread count.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use chauffeur_protocol::client::{NewsItem, NewsSource, NewsState};
use futures_util::future::join_all;
use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::settings::NewsSettings;
use crate::store::Store;
use crate::{Daemon, Error, Result};

/// A source is fetched at most this often.
const FRESH: Duration = Duration::from_secs(15 * 60);
const TIMEOUT: Duration = Duration::from_secs(10);
/// A feed larger than this is refused rather than read.
const MAX_BYTES: usize = 2 * 1024 * 1024;
/// The newest items kept of a feed.
const MAX_ITEMS: usize = 50;
/// Hacker News's official API: the front page's ids in its order, then each story. One request
/// per story, but the order is the front page's own (Algolia's `front_page` search, one request,
/// sorts by points instead).
const HN_TOP: &str = "https://hacker-news.firebaseio.com/v0/topstories.json";
const HN_ITEM: &str = "https://hacker-news.firebaseio.com/v0/item";
const HN_FRONT: &str = "https://news.ycombinator.com/";
const HN_STORIES: usize = 30;

pub struct News {
    http: reqwest::Client,
    /// By the source's URL (`HN_FRONT` for Hacker News). Held while fetching, so that a second
    /// reader waits for the fetch rather than starting another.
    cache: tokio::sync::Mutex<HashMap<String, Fetched>>,
}

#[derive(Clone)]
struct Fetched {
    at: Instant,
    title: Option<String>,
    items: Vec<NewsItem>,
    error: Option<String>,
}

/// A feed's title, if it has one, and its items.
type Read = (Option<String>, Vec<NewsItem>);

impl Default for News {
    fn default() -> Self {
        Self::new()
    }
}

impl News {
    pub fn new() -> News {
        News {
            http: reqwest::Client::builder()
                .timeout(TIMEOUT)
                .user_agent(concat!("Chauffeur/", env!("CARGO_PKG_VERSION")))
                .build()
                .expect("an HTTP client"),
            cache: tokio::sync::Mutex::new(HashMap::new()),
        }
    }

    /// The kiosk's sources, those last fetched 15 minutes ago or more fetched again first. A
    /// source that fails keeps the items it last had, with the error.
    pub async fn state(&self, settings: &NewsSettings, store: &Store) -> Result<NewsState> {
        let mut cache = self.cache.lock().await;
        cache.retain(|url, _| url == HN_FRONT || settings.feeds.contains(url));
        let stale = |url: &str| cache.get(url).is_none_or(|f| f.at.elapsed() >= FRESH);
        let hn = settings.hacker_news && stale(HN_FRONT);
        let feeds: Vec<&String> = settings.feeds.iter().filter(|url| stale(url)).collect();
        let (hn, fetched) = tokio::join!(
            async {
                if hn {
                    Some(self.hacker_news().await.map(|items| (None, items)))
                } else {
                    None
                }
            },
            join_all(feeds.iter().map(|url| self.feed(url)))
        );
        let now = Instant::now();
        let fetched = hn
            .map(|result| (HN_FRONT, result))
            .into_iter()
            .chain(feeds.iter().map(|url| url.as_str()).zip(fetched));
        for (url, result) in fetched {
            let previous = cache.remove(url);
            let entry = match result {
                Ok((title, items)) => Fetched {
                    at: now,
                    title,
                    items,
                    error: None,
                },
                Err(error) => {
                    tracing::warn!(%error, url, "could not read a news source");
                    Fetched {
                        at: now,
                        error: Some(error),
                        ..previous.unwrap_or(Fetched {
                            at: now,
                            title: None,
                            items: Vec::new(),
                            error: None,
                        })
                    }
                }
            };
            cache.insert(url.to_string(), entry);
        }
        let read = store.news_read()?;
        let source = |url: &str, title: &str| {
            let fetched = cache.get(url);
            NewsSource {
                url: url.to_string(),
                title: fetched
                    .and_then(|f| f.title.clone())
                    .unwrap_or_else(|| title.to_string()),
                items: fetched
                    .map(|f| f.items.clone())
                    .unwrap_or_default()
                    .into_iter()
                    .map(|item| NewsItem {
                        read: read.contains(&item.id),
                        ..item
                    })
                    .collect(),
                error: fetched.and_then(|f| f.error.clone()),
            }
        };
        Ok(NewsState {
            hacker_news: settings
                .hacker_news
                .then(|| source(HN_FRONT, "Hacker News")),
            feeds: settings
                .feeds
                .iter()
                .map(|url| source(url, &host_of(url)))
                .collect(),
        })
    }

    /// The front page's stories; a story that cannot be read is left out.
    async fn hacker_news(&self) -> Result<Vec<NewsItem>, String> {
        let ids: Vec<u64> = serde_json::from_slice(&self.get(HN_TOP).await?)
            .map_err(|_| "Hacker News's API answered something else than its list".to_string())?;
        let stories = join_all(
            ids.iter()
                .take(HN_STORIES)
                .map(|id| async move { self.get(&format!("{HN_ITEM}/{id}.json")).await }),
        )
        .await;
        Ok(stories
            .into_iter()
            .filter_map(|body| hn_story(&body.ok()?))
            .collect())
    }

    async fn feed(&self, url: &str) -> Result<Read, String> {
        parse_feed(url, &self.get(url).await?)
    }

    /// The body at `url`, refused past `MAX_BYTES`.
    async fn get(&self, url: &str) -> Result<Vec<u8>, String> {
        let mut response = self
            .http
            .get(url)
            .send()
            .await
            .and_then(|r| r.error_for_status())
            .map_err(failure)?;
        let mut body = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(failure)? {
            body.extend_from_slice(&chunk);
            if body.len() > MAX_BYTES {
                return Err(format!(
                    "larger than {} MB: not read",
                    MAX_BYTES / 1024 / 1024
                ));
            }
        }
        Ok(body)
    }
}

fn failure(error: reqwest::Error) -> String {
    if error.is_timeout() {
        format!("no answer within {} s", TIMEOUT.as_secs())
    } else if let Some(status) = error.status() {
        format!("the site answered {status}")
    } else {
        format!("could not reach it: {}", error.without_url())
    }
}

/// A story of Hacker News's API (`item/<id>.json`); none for a job ad, a poll, a dead or deleted
/// item.
fn hn_story(body: &[u8]) -> Option<NewsItem> {
    #[derive(Deserialize)]
    struct Story {
        id: u64,
        #[serde(rename = "type")]
        kind: String,
        title: Option<String>,
        url: Option<String>,
        score: Option<u32>,
        descendants: Option<u32>,
        time: Option<i64>,
        #[serde(default)]
        dead: bool,
        #[serde(default)]
        deleted: bool,
    }
    let story: Story = serde_json::from_slice(body).ok()?;
    if story.kind != "story" || story.dead || story.deleted {
        return None;
    }
    let discussion = format!("https://news.ycombinator.com/item?id={}", story.id);
    Some(NewsItem {
        id: format!("hn:{}", story.id),
        title: story.title?,
        url: story
            .url
            .filter(|url| !url.is_empty())
            .unwrap_or_else(|| discussion.clone()),
        published: story
            .time
            .and_then(|t| chrono::DateTime::from_timestamp(t, 0))
            .map(|t| t.to_rfc3339()),
        read: false,
        points: Some(story.score.unwrap_or(0)),
        comments: Some(story.descendants.unwrap_or(0)),
        discussion: Some(discussion),
    })
}

/// An RSS or Atom document's title and its items with a link, newest first, `MAX_ITEMS` at most.
fn parse_feed(url: &str, body: &[u8]) -> Result<Read, String> {
    let feed = feed_rs::parser::Builder::new()
        .base_uri(Some(url))
        .build()
        .parse(body)
        .map_err(|_| "not an RSS or Atom feed".to_string())?;
    let mut entries: Vec<_> = feed
        .entries
        .into_iter()
        .filter_map(|entry| {
            let link = entry
                .links
                .iter()
                .find(|l| l.rel.as_deref().is_none_or(|rel| rel == "alternate"))?
                .href
                .clone();
            let when = entry.published.or(entry.updated);
            let title = entry
                .title
                .map(|t| t.content.trim().to_string())
                .filter(|t| !t.is_empty());
            Some((
                when,
                NewsItem {
                    id: item_id(url, &entry.id),
                    title: title.unwrap_or_else(|| link.clone()),
                    url: link,
                    published: when.map(|t| t.to_rfc3339()),
                    read: false,
                    points: None,
                    comments: None,
                    discussion: None,
                },
            ))
        })
        .collect();
    // Undated items go last.
    entries.sort_by_key(|(when, _)| std::cmp::Reverse(*when));
    entries.truncate(MAX_ITEMS);
    let title = feed
        .title
        .map(|t| t.content.trim().to_string())
        .filter(|t| !t.is_empty());
    Ok((title, entries.into_iter().map(|(_, item)| item).collect()))
}

/// The same entry of the same feed has the same id at every fetch: its guid, else what feed-rs
/// derives from its link and title.
fn item_id(feed: &str, entry: &str) -> String {
    let digest = Sha256::digest(format!("{feed}\n{entry}"));
    let hex: String = digest[..12].iter().map(|b| format!("{b:02x}")).collect();
    format!("feed:{hex}")
}

fn host_of(url: &str) -> String {
    reqwest::Url::parse(url)
        .ok()
        .and_then(|u| {
            u.host_str()
                .map(|h| h.trim_start_matches("www.").to_string())
        })
        .unwrap_or_else(|| url.to_string())
}

/// A feed's address as settings.toml keeps it: `http` or `https`, with a host.
pub fn feed_url(url: &str) -> Result<String> {
    let url = url.trim();
    let refused = || {
        Error::BadRequest(format!(
            "\"{url}\" is not a feed's address: give its full URL, https://…"
        ))
    };
    let parsed = reqwest::Url::parse(url).map_err(|_| refused())?;
    if !matches!(parsed.scheme(), "http" | "https") || parsed.host_str().is_none() {
        return Err(refused());
    }
    Ok(parsed.to_string())
}

pub async fn state(d: &Daemon) -> Result<NewsState> {
    d.news.state(&d.settings.get().news, &d.store).await
}

pub async fn add_feed(d: &Daemon, url: &str) -> Result<NewsState> {
    let url = feed_url(url)?;
    let mut feeds = d.settings.get().news.feeds.clone();
    if feeds.contains(&url) {
        return Err(Error::Conflict(format!(
            "{url} is already one of your feeds"
        )));
    }
    feeds.push(url);
    set_feeds(d, &feeds)?;
    state(d).await
}

pub async fn remove_feed(d: &Daemon, url: &str) -> Result<NewsState> {
    let mut feeds = d.settings.get().news.feeds.clone();
    let before = feeds.len();
    feeds.retain(|feed| feed != url.trim());
    if feeds.len() == before {
        return Err(Error::not_found(format!("the feed {url}")));
    }
    set_feeds(d, &feeds)?;
    state(d).await
}

pub async fn show_hacker_news(d: &Daemon, shown: bool) -> Result<NewsState> {
    d.settings
        .update(|doc| news_table(doc)["hacker_news"] = toml_edit::value(shown))
        .map_err(|e| Error::BadRequest(format!("{e:#}")))?;
    state(d).await
}

fn set_feeds(d: &Daemon, feeds: &[String]) -> Result<()> {
    d.settings
        .update(|doc| {
            news_table(doc)["feeds"] = toml_edit::value(toml_edit::Array::from_iter(
                feeds.iter().map(String::as_str),
            ))
        })
        .map_err(|e| Error::BadRequest(format!("{e:#}")))
}

fn news_table(doc: &mut toml_edit::DocumentMut) -> &mut toml_edit::Item {
    if !doc.contains_key("news") {
        doc["news"] = toml_edit::table();
    }
    &mut doc["news"]
}

#[cfg(test)]
mod tests {
    use super::*;

    const RSS: &str = r#"<?xml version="1.0"?>
<rss version="2.0"><channel>
  <title> Julia Evans </title>
  <link>https://jvns.ca/</link>
  <item><title>Old post</title><link>https://jvns.ca/old</link><guid>old</guid>
    <pubDate>Mon, 01 Sep 2026 08:00:00 GMT</pubDate></item>
  <item><title>No date</title><link>https://jvns.ca/undated</link></item>
  <item><title>New post</title><link>https://jvns.ca/new</link><guid>new</guid>
    <pubDate>Wed, 22 Oct 2026 09:30:00 +0200</pubDate></item>
  <item><title>Without a link</title><guid>nolink</guid></item>
</channel></rss>"#;

    const ATOM: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<feed xmlns="http://www.w3.org/2005/Atom">
  <title>Rust Blog</title>
  <id>https://blog.rust-lang.org/</id>
  <updated>2026-10-20T00:00:00Z</updated>
  <entry>
    <title>Announcing Rust 1.95</title>
    <id>tag:blog.rust-lang.org,2026:rust-1.95</id>
    <link rel="alternate" href="/2026/10/16/Rust-1.95.0/"/>
    <link rel="edit" href="https://example.org/edit"/>
    <updated>2026-10-16T00:00:00Z</updated>
  </entry>
</feed>"#;

    #[test]
    fn an_rss_feed_gives_its_title_and_its_linked_items_newest_first_undated_last() {
        let (title, items) = parse_feed("https://jvns.ca/atom.xml", RSS.as_bytes()).unwrap();
        assert_eq!(title.as_deref(), Some("Julia Evans"));
        let titles: Vec<_> = items.iter().map(|i| i.title.as_str()).collect();
        assert_eq!(titles, ["New post", "Old post", "No date"]);
        assert_eq!(items[0].url, "https://jvns.ca/new");
        assert_eq!(
            items[0].published.as_deref(),
            Some("2026-10-22T07:30:00+00:00")
        );
        assert_eq!(items[2].published, None);
        assert_eq!(items[0].points, None);
    }

    #[test]
    fn an_atom_entry_links_to_its_page_resolved_against_the_feed() {
        let url = "https://blog.rust-lang.org/feed.xml";
        let (title, items) = parse_feed(url, ATOM.as_bytes()).unwrap();
        assert_eq!(title.as_deref(), Some("Rust Blog"));
        assert_eq!(items.len(), 1);
        assert_eq!(
            items[0].url,
            "https://blog.rust-lang.org/2026/10/16/Rust-1.95.0/"
        );
        assert_eq!(
            items[0].published.as_deref(),
            Some("2026-10-16T00:00:00+00:00")
        );
    }

    #[test]
    fn an_item_keeps_its_id_from_fetch_to_fetch_and_differs_between_feeds() {
        let first = parse_feed("https://a.example/feed", RSS.as_bytes())
            .unwrap()
            .1;
        let again = parse_feed("https://a.example/feed", RSS.as_bytes())
            .unwrap()
            .1;
        let other = parse_feed("https://b.example/feed", RSS.as_bytes())
            .unwrap()
            .1;
        assert_eq!(first, again);
        assert_ne!(first[0].id, other[0].id);
    }

    #[test]
    fn a_page_that_is_no_feed_is_refused_with_a_reason() {
        let error = parse_feed("https://example.org/", b"<html><body>hi</body></html>");
        assert_eq!(error.unwrap_err(), "not an RSS or Atom feed");
    }

    #[test]
    fn a_hacker_news_story_gives_its_points_comments_and_discussion() {
        let story = br#"{"by":"pg","descendants":71,"id":8863,"kids":[8952],"score":104,
            "time":1175714200,"title":"My YC app: Dropbox","type":"story",
            "url":"http://www.getdropbox.com/u/2/screencast.html"}"#;
        let item = hn_story(story).unwrap();
        assert_eq!(item.id, "hn:8863");
        assert_eq!(item.url, "http://www.getdropbox.com/u/2/screencast.html");
        assert_eq!((item.points, item.comments), (Some(104), Some(71)));
        assert_eq!(
            item.discussion.as_deref(),
            Some("https://news.ycombinator.com/item?id=8863")
        );
        assert_eq!(item.published.as_deref(), Some("2007-04-04T19:16:40+00:00"));
    }

    #[test]
    fn an_ask_hn_opens_its_discussion_and_job_ads_or_dead_stories_are_left_out() {
        let ask = br#"{"id":121003,"score":25,"time":1203647620,"title":"Ask HN: The Arc Effect","type":"story"}"#;
        let item = hn_story(ask).unwrap();
        assert_eq!(item.url, "https://news.ycombinator.com/item?id=121003");
        assert_eq!(item.comments, Some(0));
        let job = br#"{"id":192327,"score":6,"time":1210981217,"title":"Justin.tv is hiring","type":"job"}"#;
        assert!(hn_story(job).is_none());
        let dead = br#"{"id":1,"dead":true,"title":"x","type":"story"}"#;
        assert!(hn_story(dead).is_none());
    }

    #[test]
    fn a_feed_address_is_a_full_http_url() {
        assert_eq!(
            feed_url("  https://jvns.ca/atom.xml ").unwrap(),
            "https://jvns.ca/atom.xml"
        );
        assert_eq!(
            feed_url("http://example.org").unwrap(),
            "http://example.org/"
        );
        for refused in [
            "jvns.ca/atom.xml",
            "file:///etc/passwd",
            "ftp://x.org/f",
            "",
        ] {
            assert!(
                matches!(feed_url(refused), Err(Error::BadRequest(_))),
                "{refused}"
            );
        }
    }

    #[tokio::test]
    async fn hidden_hacker_news_and_no_feeds_fetch_nothing() {
        let news = News::new();
        let store = Store::open_in_memory().unwrap();
        let settings = NewsSettings {
            hacker_news: false,
            feeds: Vec::new(),
        };
        let state = news.state(&settings, &store).await.unwrap();
        assert!(state.hacker_news.is_none());
        assert!(state.feeds.is_empty());
    }
}
