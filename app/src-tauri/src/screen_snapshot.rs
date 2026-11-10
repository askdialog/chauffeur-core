//! Sharp pictures of a design chat's screens for the canvas (story 47), on macOS.
//!
//! Under the canvas's zoom, WebKit draws a framed screen whose page holds a composited layer
//! (`position: fixed`, `will-change`, `backdrop-filter`, a transform animation, a 3D transform)
//! at 100% and lets the GPU shrink that bitmap, which blurs it. A WKWebView snapshot paints the
//! page again at the size asked, so the canvas lays these pictures over its live frames. The
//! pages load in a few offscreen web views, reused because each one costs a WebContent process,
//! with `snapshot=1` so that the daemon sandboxes them as the canvas's frames are. Elsewhere the
//! commands refuse and the canvas keeps its live frames.

#[cfg(target_os = "macos")]
use std::time::Duration;

/// How long a screen may take to load and be pictured.
#[cfg(target_os = "macos")]
const TIMEOUT: Duration = Duration::from_secs(15);
/// The largest picture: past it the canvas asks for the part of the screen it shows.
const MAX_PIXELS: f64 = 40_000_000.0;
const MAX_SIDE: f64 = 20_000.0;

/// A part of a screen, in its CSS pixels.
#[derive(serde::Deserialize, Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
}

/// What to picture: `rect` of the screen at `url` laid out at `width` × `height`, `pixels` wide.
/// Elsewhere than on macOS the request is still checked, then refused: nothing reads it.
#[derive(Debug)]
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
struct Job {
    url: String,
    width: f64,
    height: f64,
    rect: Rect,
    pixels: f64,
}

/// A PNG of `rect` of the screen at `url` (the daemon's, as the canvas frames it), `pixels` wide.
#[tauri::command]
pub async fn screen_snapshot(
    window: tauri::WebviewWindow,
    url: String,
    width: f64,
    height: f64,
    rect: Rect,
    pixels: f64,
) -> Result<tauri::ipc::Response, String> {
    if window.label() != "main" {
        return Err("only the main Chauffeur window pictures screens".into());
    }
    let job = job(&url, width, height, rect, pixels)?;
    picture(&window, job).await.map(tauri::ipc::Response::new)
}

/// Closes the offscreen web views (the canvas is gone): their processes end.
#[tauri::command]
pub fn screen_snapshots_release(window: tauri::WebviewWindow) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    return window
        .run_on_main_thread(macos::release)
        .map_err(|error| error.to_string());
    #[cfg(not(target_os = "macos"))]
    {
        let _ = window;
        Ok(())
    }
}

fn job(url: &str, width: f64, height: f64, rect: Rect, pixels: f64) -> Result<Job, String> {
    // Only the daemon's screens: nothing else loads in these hidden views. Parsed, so that a
    // `127.0.0.1:1@elsewhere` user part cannot pass for the host.
    let ours = url::Url::parse(url).is_ok_and(|parsed| {
        parsed.scheme() == "http"
            && parsed.host_str() == Some("127.0.0.1")
            && parsed.port().is_some()
            && parsed.username().is_empty()
            && parsed.password().is_none()
    });
    if !ours {
        return Err(format!(
            "refusing to picture {url}: not a screen of the daemon"
        ));
    }
    let side = |v: f64| v.is_finite() && v > 0.0 && v <= MAX_SIDE;
    if !(side(width) && side(height) && side(rect.width) && side(rect.height) && side(pixels)) {
        return Err(format!("refusing to picture {url}: invalid size"));
    }
    if rect.x < 0.0
        || rect.y < 0.0
        || rect.x + rect.width > width + 0.5
        || rect.y + rect.height > height + 0.5
    {
        return Err(format!(
            "refusing to picture {url}: the part asked is out of the screen"
        ));
    }
    if pixels * pixels * rect.height / rect.width > MAX_PIXELS {
        return Err(format!("refusing to picture {url}: too many pixels"));
    }
    let separator = if url.contains('?') { '&' } else { '?' };
    Ok(Job {
        url: format!("{url}{separator}snapshot=1"),
        width,
        height,
        rect,
        pixels,
    })
}

#[cfg(target_os = "macos")]
async fn picture(window: &tauri::WebviewWindow, job: Job) -> Result<Vec<u8>, String> {
    let (reply, answer) = tokio::sync::oneshot::channel();
    let url = job.url.clone();
    window
        .run_on_main_thread(move || macos::start(job, reply))
        .map_err(|error| error.to_string())?;
    let image = match tokio::time::timeout(TIMEOUT, answer).await {
        Ok(Ok(image)) => image?,
        Ok(Err(_)) => return Err(format!("{url} was not pictured: its view was reused")),
        Err(_) => return Err(format!("{url} did not load within {} s", TIMEOUT.as_secs())),
    };
    // Off the main thread: a large PNG takes a tenth of a second to encode.
    tokio::task::spawn_blocking(move || macos::png(&image))
        .await
        .map_err(|error| error.to_string())?
}

#[cfg(not(target_os = "macos"))]
async fn picture(_window: &tauri::WebviewWindow, _job: Job) -> Result<Vec<u8>, String> {
    Err("screens are pictured on macOS only".into())
}

#[cfg(target_os = "macos")]
mod macos {
    use super::{Job, TIMEOUT};
    use block2::RcBlock;
    use objc2::rc::Retained;
    use objc2::runtime::{AnyObject, NSObject, NSObjectProtocol, ProtocolObject};
    use objc2::{
        AllocAnyThread, MainThreadMarker, MainThreadOnly, Message, define_class, msg_send,
    };
    use objc2_app_kit::{NSBitmapImageFileType, NSBitmapImageRep, NSImage};
    use objc2_core_foundation::{CGPoint, CGRect, CGSize};
    use objc2_core_graphics::CGImage;
    use objc2_foundation::{NSDictionary, NSError, NSNumber, NSString, NSURL, NSURLRequest};
    use objc2_web_kit::{
        WKAudiovisualMediaTypes, WKContentWorld, WKNavigation, WKNavigationDelegate,
        WKSnapshotConfiguration, WKWebView, WKWebViewConfiguration, WKWebsiteDataStore,
    };
    use std::cell::{Cell, RefCell};
    use std::time::Instant;

    /// The offscreen web views kept.
    const VIEWS: usize = 4;
    /// What a loaded page waits for before it is pictured: its fonts, then a moment for its
    /// scripts' first render. Answers its device pixel ratio, which sizes the pictures.
    const SETTLE: &str = "await document.fonts.ready; await new Promise((r) => setTimeout(r, 80)); return window.devicePixelRatio;";

    pub type Reply = tokio::sync::oneshot::Sender<Result<Retained<CGImage>, String>>;
    type Ready = Box<dyn FnOnce(Result<(Retained<WKWebView>, f64), String>)>;

    struct View {
        web: Retained<WKWebView>,
        url: String,
        size: (f64, f64),
        /// The load under way or done; another one's callbacks are ignored.
        navigation: Option<Retained<WKNavigation>>,
        /// The page's device pixel ratio once it settled.
        ratio: Option<f64>,
        waiting: Vec<Ready>,
        started: Instant,
        used: u64,
    }

    #[derive(Default)]
    struct Pool {
        views: Vec<View>,
        loader: Option<Retained<Loader>>,
        clock: u64,
    }

    thread_local! {
        static POOL: RefCell<Pool> = RefCell::default();
    }

    define_class!(
        #[unsafe(super(NSObject))]
        #[thread_kind = MainThreadOnly]
        #[name = "ChauffeurScreenSnapshotLoader"]
        struct Loader;

        unsafe impl NSObjectProtocol for Loader {}

        unsafe impl WKNavigationDelegate for Loader {
            #[unsafe(method(webView:didFinishNavigation:))]
            fn did_finish(&self, web: &WKWebView, navigation: Option<&WKNavigation>) {
                loaded(web, navigation, None);
            }

            #[unsafe(method(webView:didFailNavigation:withError:))]
            fn did_fail(
                &self,
                web: &WKWebView,
                navigation: Option<&WKNavigation>,
                error: &NSError,
            ) {
                loaded(web, navigation, Some(error));
            }

            #[unsafe(method(webView:didFailProvisionalNavigation:withError:))]
            fn did_fail_provisional(
                &self,
                web: &WKWebView,
                navigation: Option<&WKNavigation>,
                error: &NSError,
            ) {
                loaded(web, navigation, Some(error));
            }

            #[unsafe(method(webViewWebContentProcessDidTerminate:))]
            fn terminated(&self, web: &WKWebView) {
                // Its page is gone: the next picture of that screen loads it again.
                POOL.with_borrow_mut(|pool| {
                    pool.views.retain(|view| !std::ptr::eq(&*view.web, web))
                });
            }
        }
    );

    impl Loader {
        fn new(mtm: MainThreadMarker) -> Retained<Self> {
            let this = Self::alloc(mtm).set_ivars(());
            unsafe { msg_send![super(this), init] }
        }
    }

    pub fn release() {
        POOL.with_borrow_mut(|pool| pool.views.clear());
    }

    /// Pictures `job` once its screen is loaded and settled in a view: one already holding it,
    /// else a new one, else the one used longest ago.
    pub fn start(job: Job, reply: Reply) {
        let Some(mtm) = MainThreadMarker::new() else {
            let _ = reply.send(Err("not on the main thread".into()));
            return;
        };
        let reply = Cell::new(Some(reply));
        let rect = job.rect;
        let pixels = job.pixels;
        let ready: Ready = Box::new(move |result| {
            let Some(reply) = reply.take() else { return };
            match result {
                Ok((web, ratio)) => snapshot(&web, mtm, rect, pixels / ratio, reply),
                Err(error) => {
                    let _ = reply.send(Err(error));
                }
            }
        });
        let now_ready = POOL.with_borrow_mut(|pool| {
            pool.clock += 1;
            let clock = pool.clock;
            let size = (job.width, job.height);
            if let Some(view) = pool
                .views
                .iter_mut()
                // A view whose load failed holds nothing: it is loaded again below.
                .find(|v| {
                    v.url == job.url
                        && v.size == size
                        && (v.ratio.is_some() || v.navigation.is_some())
                })
            {
                view.used = clock;
                if let Some(ratio) = view.ratio {
                    return Some((ready, Ok((view.web.clone(), ratio))));
                }
                view.waiting.push(ready);
                return None;
            }
            let loader = pool.loader.get_or_insert_with(|| Loader::new(mtm)).clone();
            let index = if pool.views.len() < VIEWS {
                pool.views.push(View {
                    web: new_view(mtm, &loader),
                    url: String::new(),
                    size,
                    navigation: None,
                    ratio: None,
                    waiting: Vec::new(),
                    started: Instant::now(),
                    used: clock,
                });
                pool.views.len() - 1
            } else {
                // A view still loading is taken only once its own request gave up.
                let free = |v: &View| {
                    v.ratio.is_some() || v.navigation.is_none() || v.started.elapsed() > TIMEOUT
                };
                match (0..pool.views.len())
                    .filter(|&i| free(&pool.views[i]))
                    .min_by_key(|&i| pool.views[i].used)
                {
                    Some(index) => index,
                    None => {
                        return Some((
                            ready,
                            Err("every offscreen view is loading a screen".into()),
                        ));
                    }
                }
            };
            let view = &mut pool.views[index];
            view.url = job.url.clone();
            view.size = size;
            view.ratio = None;
            view.waiting = vec![ready];
            view.started = Instant::now();
            view.used = clock;
            view.web.setFrame(CGRect::new(
                CGPoint::new(0.0, 0.0),
                CGSize::new(job.width, job.height),
            ));
            let request = NSURL::URLWithString(&NSString::from_str(&job.url))
                .map(|url| NSURLRequest::requestWithURL(&url));
            view.navigation = request.and_then(|request| unsafe { view.web.loadRequest(&request) });
            if view.navigation.is_none() {
                let waiting = std::mem::take(&mut view.waiting);
                return waiting
                    .into_iter()
                    .next()
                    .map(|ready| (ready, Err(format!("could not load {}", job.url))));
            }
            None
        });
        if let Some((ready, result)) = now_ready {
            ready(result);
        }
    }

    fn new_view(mtm: MainThreadMarker, loader: &Loader) -> Retained<WKWebView> {
        unsafe {
            let config = WKWebViewConfiguration::new(mtm);
            config.setWebsiteDataStore(&WKWebsiteDataStore::nonPersistentDataStore(mtm));
            config.setMediaTypesRequiringUserActionForPlayback(WKAudiovisualMediaTypes::All);
            let frame = CGRect::new(CGPoint::new(0.0, 0.0), CGSize::new(390.0, 844.0));
            let web = WKWebView::initWithFrame_configuration(WKWebView::alloc(mtm), frame, &config);
            web.setNavigationDelegate(Some(ProtocolObject::from_ref(loader)));
            web
        }
    }

    /// The end of a load: settle the page, then hand it to whoever waits for it.
    fn loaded(web: &WKWebView, navigation: Option<&WKNavigation>, error: Option<&NSError>) {
        let ours = |view: &View| {
            std::ptr::eq(&*view.web, web)
                && matches!((&view.navigation, navigation), (Some(a), Some(b)) if std::ptr::eq(&**a, b))
        };
        if let Some(error) = error {
            let waiting = POOL.with_borrow_mut(|pool| {
                let view = pool.views.iter_mut().find(|v| ours(v))?;
                view.navigation = None;
                Some((view.url.clone(), std::mem::take(&mut view.waiting)))
            });
            if let Some((url, waiting)) = waiting {
                let message = format!("could not load {url}: {}", error.localizedDescription());
                for ready in waiting {
                    ready(Err(message.clone()));
                }
            }
            return;
        }
        let Some(navigation) = POOL.with_borrow(|pool| {
            pool.views
                .iter()
                .find(|v| ours(v))
                .and_then(|v| v.navigation.clone())
        }) else {
            return;
        };
        let Some(mtm) = MainThreadMarker::new() else {
            return;
        };
        let page = web.retain();
        let settled = RcBlock::new(move |value: *mut AnyObject, _error: *mut NSError| {
            let ratio = unsafe { value.as_ref() }
                .and_then(|value| value.downcast_ref::<NSNumber>())
                .map(|number| number.doubleValue())
                .filter(|ratio| ratio.is_finite() && *ratio > 0.0)
                .unwrap_or(1.0);
            let waiting = POOL.with_borrow_mut(|pool| {
                let view = pool.views.iter_mut().find(|v| {
                    std::ptr::eq(&*v.web, &*page)
                        && v.navigation
                            .as_ref()
                            .is_some_and(|n| std::ptr::eq(&**n, &*navigation))
                })?;
                view.ratio = Some(ratio);
                Some(std::mem::take(&mut view.waiting))
            });
            for ready in waiting.unwrap_or_default() {
                ready(Ok((page.clone(), ratio)));
            }
        });
        unsafe {
            web.callAsyncJavaScript_arguments_inFrame_inContentWorld_completionHandler(
                &NSString::from_str(SETTLE),
                None,
                None,
                &WKContentWorld::defaultClientWorld(mtm),
                Some(&settled),
            );
        }
    }

    fn snapshot(
        web: &WKWebView,
        mtm: MainThreadMarker,
        rect: super::Rect,
        points: f64,
        reply: Reply,
    ) {
        let reply = Cell::new(Some(reply));
        let done = RcBlock::new(move |image: *mut NSImage, error: *mut NSError| {
            let Some(reply) = reply.take() else { return };
            let result = match unsafe { image.as_ref() } {
                Some(image) => unsafe {
                    image.CGImageForProposedRect_context_hints(std::ptr::null_mut(), None, None)
                }
                .ok_or_else(|| "the snapshot has no bitmap".to_string()),
                None => Err(match unsafe { error.as_ref() } {
                    Some(error) => format!("the snapshot failed: {}", error.localizedDescription()),
                    None => "the snapshot failed".into(),
                }),
            };
            let _ = reply.send(result);
        });
        unsafe {
            let config = WKSnapshotConfiguration::new(mtm);
            config.setRect(CGRect::new(
                CGPoint::new(rect.x, rect.y),
                CGSize::new(rect.width, rect.height),
            ));
            config.setSnapshotWidth(Some(&NSNumber::numberWithDouble(points)));
            web.takeSnapshotWithConfiguration_completionHandler(Some(&config), &done);
        }
    }

    pub fn png(image: &CGImage) -> Result<Vec<u8>, String> {
        let rep = NSBitmapImageRep::initWithCGImage(NSBitmapImageRep::alloc(), image);
        unsafe {
            rep.representationUsingType_properties(NSBitmapImageFileType::PNG, &NSDictionary::new())
        }
        .map(|data| data.to_vec())
        .ok_or_else(|| "could not encode the snapshot as PNG".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const URL: &str = "http://127.0.0.1:4100/s3cret/ws/chat/sign%20in/empty.html?v=12";

    fn whole(width: f64, height: f64) -> Rect {
        Rect {
            x: 0.0,
            y: 0.0,
            width,
            height,
        }
    }

    #[test]
    fn a_screen_is_loaded_sandboxed_for_its_picture() {
        let framed = job(URL, 1440.0, 900.0, whole(1440.0, 900.0), 864.0).unwrap();
        assert_eq!(framed.url, format!("{URL}&snapshot=1"));
        let bare = job(
            "http://127.0.0.1:4100/a.html",
            390.0,
            844.0,
            whole(390.0, 844.0),
            390.0,
        )
        .unwrap();
        assert_eq!(bare.url, "http://127.0.0.1:4100/a.html?snapshot=1");
    }

    #[test]
    fn only_the_daemons_screens_at_a_sane_size_are_pictured() {
        assert!(
            job(
                "https://example.com/a.html",
                390.0,
                844.0,
                whole(390.0, 844.0),
                390.0
            )
            .is_err()
        );
        assert!(
            job(
                "file:///etc/passwd",
                390.0,
                844.0,
                whole(390.0, 844.0),
                390.0
            )
            .is_err()
        );
        assert!(job(URL, 0.0, 844.0, whole(390.0, 844.0), 390.0).is_err());
        assert!(job(URL, f64::NAN, 844.0, whole(390.0, 844.0), 390.0).is_err());
        assert!(
            job(
                URL,
                390.0,
                844.0,
                Rect {
                    x: 200.0,
                    y: 0.0,
                    width: 300.0,
                    height: 100.0
                },
                300.0
            )
            .is_err()
        );
        // 20 000 × 12 500 pixels: the canvas asks for the part on screen instead.
        assert!(job(URL, 1440.0, 900.0, whole(1440.0, 900.0), 20_000.0).is_err());
        assert!(
            job(
                URL,
                1440.0,
                900.0,
                Rect {
                    x: 100.0,
                    y: 50.0,
                    width: 400.0,
                    height: 300.0
                },
                3200.0
            )
            .is_ok()
        );
    }
}
