//! A deck's downloads (docs/adr/slide-decks.md): a PDF of its slides, one slide as a PNG, or one
//! self-contained HTML page that plays it. Skipped slides are left out. The PDF and the PNG are
//! rendered by the headless Chrome design exports use, from a page served once on the screens'
//! listener; the HTML page is put together here, every file it needs inlined.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use base64::Engine;
use chauffeur_protocol::deck::{
    DeckExportFormat, DeckExportParams, DeckExportResult, DeckParams, DeckSlide,
};

use crate::design_export::{find_chrome, pdf_complete, png_complete, run_chrome};
use crate::{Daemon, Error, Result};

const NO_CHROME: &str = "No Chrome to render the deck with: install Google Chrome (or Chromium), \
or set CHAUFFEUR_CHROME to a Chrome binary, then download again.";
/// Bytes a file inlined into the HTML download may weigh.
const INLINE_MAX: u64 = 25 * 1024 * 1024;
/// How long Chrome lets a slide's scripts and fonts settle before printing.
const SETTLE_MS: u32 = 4_000;

pub async fn export(daemon: &Arc<Daemon>, params: &DeckExportParams) -> Result<DeckExportResult> {
    let deck = crate::deck::find(daemon, &params.workspace, &params.path)?;
    let read = crate::deck::read(
        daemon,
        &DeckParams {
            workspace: params.workspace.clone(),
            path: params.path.clone(),
            version: None,
        },
    )?;
    let shown: Vec<&DeckSlide> = read.slides.iter().filter(|s| !s.skip).collect();
    if shown.is_empty() {
        return Err(Error::BadRequest(
            "every slide is skipped: show one again to download the deck".into(),
        ));
    }
    let name = match crate::slug::slugify(&read.title, 60) {
        slug if slug.is_empty() => "deck".to_string(),
        slug => slug,
    };
    let exports = deck.dir.join(crate::versions::EXPORTS_DIR);
    let (file, out) = match params.format {
        DeckExportFormat::Html => {
            let file = format!("{name}.html");
            let html = player(
                &read.title,
                read.width,
                read.height,
                &shown,
                &deck.dir,
                &deck.folder,
            )?;
            std::fs::create_dir_all(&exports)?;
            std::fs::write(exports.join(&file), html)?;
            (file, None)
        }
        DeckExportFormat::Pdf => {
            let file = format!("{name}.pdf");
            let page = print_page(read.width, read.height, &shown);
            (file.clone(), Some((page, exports.join(file), true)))
        }
        DeckExportFormat::Png => {
            let slide = match params.slide.as_deref() {
                Some(file) => read
                    .slides
                    .iter()
                    .find(|s| s.file == file)
                    .ok_or_else(|| Error::NotFound(format!("no slide {file} in this deck")))?,
                None => shown[0],
            };
            let number = read
                .slides
                .iter()
                .position(|s| s.file == slide.file)
                .unwrap_or(0)
                + 1;
            let file = format!("{name}-slide-{number}.png");
            let page = print_page(read.width, read.height, &[slide]);
            (file.clone(), Some((page, exports.join(file), false)))
        }
    };
    if let Some((page, out, pdf)) = out {
        let chrome = match daemon.designs.fake_chrome() {
            Some(fake) => fake,
            None => find_chrome(daemon.shell_env.vars().get("PATH").map(String::as_str))
                .ok_or_else(|| Error::Unavailable(NO_CHROME.into()))?,
        };
        let token = daemon.designs.exports.add(page);
        let url = daemon.designs.export_url(&token).ok_or_else(|| {
            Error::Unavailable("the slides are not served yet: download again in a moment".into())
        })?;
        let args = if pdf {
            vec![
                format!("--virtual-time-budget={SETTLE_MS}"),
                "--no-pdf-header-footer".to_string(),
                format!("--print-to-pdf={}", out.display()),
                url,
            ]
        } else {
            vec![
                format!("--virtual-time-budget={SETTLE_MS}"),
                format!("--window-size={},{}", read.width, read.height),
                format!("--screenshot={}", out.display()),
                url,
            ]
        };
        let complete = if pdf { pdf_complete } else { png_complete };
        let rendered = run_chrome(&chrome, args, &out, complete).await;
        daemon.designs.exports.forget(&token);
        rendered?;
    }
    Ok(DeckExportResult {
        path: format!("{}/{}/{file}", deck.dir_path, crate::versions::EXPORTS_DIR),
    })
}

/// The presenter's page, served once: the current slide, the next one, the notes and a clock;
/// "Open the slides" opens the window shown to the audience, which this page drives.
pub fn presenter(daemon: &Arc<Daemon>, params: &DeckParams) -> Result<String> {
    let read = crate::deck::read(
        daemon,
        &DeckParams {
            workspace: params.workspace.clone(),
            path: params.path.clone(),
            version: None,
        },
    )?;
    let slides: Vec<serde_json::Value> = read
        .slides
        .iter()
        .filter(|s| !s.skip)
        .map(|s| {
            serde_json::json!({
                "url": s.url.as_deref().unwrap_or_default()
                    .trim_end_matches(&format!("?{}", crate::deck::READER_QUERY)),
                "title": s.title,
                "notes": s.notes,
            })
        })
        .collect();
    if slides.is_empty() {
        return Err(Error::BadRequest(
            "every slide is skipped: show one again to present the deck".into(),
        ));
    }
    let html = presenter_page(&read.title, read.width, read.height, &slides);
    let token = daemon.designs.exports.add(html);
    daemon.designs.export_url(&token).ok_or_else(|| {
        Error::Unavailable("the slides are not served yet: present again in a moment".into())
    })
}

fn presenter_page(title: &str, width: u32, height: u32, slides: &[serde_json::Value]) -> String {
    // `</` cannot end the script from inside the JSON.
    let data = serde_json::to_string(slides)
        .expect("slides serialize")
        .replace("</", "<\\/");
    format!(
        r#"<!doctype html><html><head><meta charset="utf-8"><title>{title} — presenter</title><style>
html,body{{margin:0;height:100%;background:#16171a;color:#e8e8ea;font:15px/1.5 -apple-system,system-ui,sans-serif}}
body{{display:grid;grid-template-columns:3fr 2fr;grid-template-rows:auto 1fr auto;gap:16px;padding:16px;box-sizing:border-box}}
header{{grid-column:1/3;display:flex;gap:12px;align-items:center}}
header h1{{font-size:15px;margin:0;flex:1;white-space:nowrap;overflow:hidden;text-overflow:ellipsis}}
button{{background:#2a2c31;color:#eee;border:1px solid #3a3d44;border-radius:6px;padding:6px 12px;font:inherit;cursor:pointer}}
button.primary{{background:#3b82f6;border-color:#3b82f6}}
.frame{{position:relative;background:#000;border-radius:6px;overflow:hidden}}
.frame iframe{{position:absolute;left:0;top:0;width:{width}px;height:{height}px;border:0;transform-origin:0 0;background:#fff}}
#side{{display:flex;flex-direction:column;gap:12px;min-height:0}}
#next{{aspect-ratio:{width}/{height}}}
#notes{{flex:1;overflow:auto;background:#1e2024;border-radius:6px;padding:12px 14px;white-space:pre-wrap;font-size:18px}}
.label{{font-size:12px;color:#9a9ca3;text-transform:uppercase;letter-spacing:.04em}}
footer{{grid-column:1/3;display:flex;gap:16px;align-items:center;font-variant-numeric:tabular-nums}}
#clock{{font-size:22px}} #elapsed{{font-size:22px;color:#9ad}}
</style></head><body>
<header><h1>{title}</h1><button class="primary" id="open">Open the slides</button><button id="full">Full screen</button></header>
<div class="frame" id="current"></div>
<div id="side"><div class="label">Next</div><div class="frame" id="next"></div><div class="label">Notes</div><div id="notes"></div></div>
<footer><button id="prev">‹ Previous</button><span id="count"></span><button id="nextb">Next ›</button><span style="flex:1"></span><span class="label">Elapsed</span><span id="elapsed">0:00</span><button id="reset">Reset</button><span class="label">Time</span><span id="clock"></span></footer>
<script>
var S={data},W={width},H={height},at=0,start=Date.now(),audience=null;
function frame(box,i){{box.innerHTML='';if(i>=S.length){{box.textContent='';return}}var f=document.createElement('iframe');f.src=S[i].url;f.setAttribute('sandbox','allow-scripts');box.appendChild(f);fit(box)}}
function fit(box){{var f=box.querySelector('iframe');if(!f)return;var s=Math.min(box.clientWidth/W,box.clientHeight/H);f.style.transform='translate('+(box.clientWidth-W*s)/2+'px,'+(box.clientHeight-H*s)/2+'px) scale('+s+')'}}
function show(n){{at=Math.max(0,Math.min(S.length-1,n));frame(document.getElementById('current'),at);frame(document.getElementById('next'),at+1);document.getElementById('notes').textContent=S[at].notes||'';document.getElementById('count').textContent=(at+1)+' / '+S.length;if(audience&&!audience.closed&&audience.show)audience.show(at)}}
function pad(n){{return n<10?'0'+n:''+n}}
function tick(){{var d=new Date();document.getElementById('clock').textContent=pad(d.getHours())+':'+pad(d.getMinutes());var e=Math.floor((Date.now()-start)/1000);document.getElementById('elapsed').textContent=Math.floor(e/60)+':'+pad(e%60)}}
document.getElementById('open').onclick=function(){{
  if(audience&&!audience.closed){{audience.focus();return}}
  audience=window.open('','chauffeur-slides','width=1280,height=760');
  if(!audience)return;
  var d=audience.document;d.open();
  d.write('<!doctype html><html><head><meta charset="utf-8"><title>'+document.title.replace(' — presenter','')+'</title><style>html,body{{margin:0;height:100%;background:#000;overflow:hidden}}#s{{position:absolute;left:50%;top:50%;width:'+W+'px;height:'+H+'px}}iframe{{position:absolute;inset:0;width:100%;height:100%;border:0;background:#fff}}</style></head><body><div id="s"></div></body></html>');
  d.close();
  var stage=d.getElementById('s');
  S.forEach(function(s,i){{var f=d.createElement('iframe');f.src=s.url;f.setAttribute('sandbox','allow-scripts');f.hidden=i!==at;stage.appendChild(f)}});
  function fitA(){{var k=Math.min(audience.innerWidth/W,audience.innerHeight/H);stage.style.transform='translate(-50%,-50%) scale('+k+')'}}
  audience.addEventListener('resize',fitA);fitA();
  audience.show=function(n){{[].forEach.call(stage.children,function(f,i){{f.hidden=i!==n}})}};
  audience.addEventListener('keydown',key);
  audience.addEventListener('dblclick',function(){{audience.document.documentElement.requestFullscreen&&audience.document.documentElement.requestFullscreen()}});
}};
function key(e){{if(['ArrowRight',' ','PageDown'].indexOf(e.key)>=0){{e.preventDefault();show(at+1)}}else if(['ArrowLeft','PageUp'].indexOf(e.key)>=0){{e.preventDefault();show(at-1)}}else if(e.key==='Home')show(0);else if(e.key==='End')show(S.length-1)}}
document.getElementById('prev').onclick=function(){{show(at-1)}};
document.getElementById('nextb').onclick=function(){{show(at+1)}};
document.getElementById('reset').onclick=function(){{start=Date.now();tick()}};
document.getElementById('full').onclick=function(){{document.documentElement.requestFullscreen&&document.documentElement.requestFullscreen()}};
addEventListener('keydown',key);
addEventListener('resize',function(){{fit(document.getElementById('current'));fit(document.getElementById('next'))}});
setInterval(tick,1000);tick();show(0);
</script></body></html>"#,
        title = escape(title),
    )
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// The page Chrome prints or captures: the slides one under the other, a page each.
fn print_page(width: u32, height: u32, slides: &[&DeckSlide]) -> String {
    let mut html = format!(
        "<!doctype html><html><head><meta charset=\"utf-8\"><style>\
@page{{size:{width}px {height}px;margin:0}}\
html,body{{margin:0;padding:0;background:#fff}}\
iframe{{display:block;border:0;width:{width}px;height:{height}px;break-after:page;page-break-after:always}}\
iframe:last-child{{break-after:auto;page-break-after:auto}}\
</style></head><body>"
    );
    for slide in slides {
        // The reader's script stays out of a download.
        let url = slide
            .url
            .as_deref()
            .unwrap_or_default()
            .trim_end_matches(&format!("?{}", crate::deck::READER_QUERY));
        html.push_str(&format!("<iframe src=\"{}\"></iframe>", escape(url)));
    }
    html.push_str("</body></html>");
    html
}

/// The deck as one page that plays it: each slide inlined whole in a frame, arrows and keys
/// between them, scaled to the window.
fn player(
    title: &str,
    width: u32,
    height: u32,
    slides: &[&DeckSlide],
    dir: &Path,
    folder: &Path,
) -> Result<String> {
    let mut frames = String::new();
    for (i, slide) in slides.iter().enumerate() {
        let file = dir.join(&slide.file);
        let source = std::fs::read_to_string(&file)
            .map_err(|error| Error::NotFound(format!("{}: {error}", slide.file)))?;
        let inlined = inline(&source, file.parent().unwrap_or(dir), folder);
        frames.push_str(&format!(
            "<iframe class=\"slide\"{} title=\"{}\" srcdoc=\"{}\"></iframe>",
            if i == 0 { "" } else { " hidden" },
            escape(&slide.title),
            escape(&inlined)
        ));
    }
    Ok(format!(
        r#"<!doctype html><html><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>{title}</title><style>
html,body{{margin:0;height:100%;background:#111;overflow:hidden;font:13px -apple-system,system-ui,sans-serif}}
#stage{{position:absolute;left:50%;top:50%;width:{width}px;height:{height}px;transform-origin:center}}
.slide{{position:absolute;inset:0;width:100%;height:100%;border:0;background:#fff}}
#bar{{position:fixed;bottom:12px;left:50%;transform:translateX(-50%);display:flex;gap:8px;align-items:center;color:#ddd;background:rgba(0,0,0,.55);padding:6px 10px;border-radius:8px;opacity:.85}}
#bar button{{background:none;border:1px solid #666;color:#eee;border-radius:6px;padding:3px 9px;cursor:pointer}}
</style></head><body><div id="stage">{frames}</div>
<div id="bar"><button id="prev" aria-label="Previous slide">‹</button><span id="count"></span><button id="next" aria-label="Next slide">›</button><button id="full">Full screen</button></div>
<script>
var slides=[].slice.call(document.querySelectorAll('.slide')),at=0,stage=document.getElementById('stage');
function fit(){{var s=Math.min(innerWidth/{width},innerHeight/{height});stage.style.transform='translate(-50%,-50%) scale('+s+')'}}
function show(n){{at=Math.max(0,Math.min(slides.length-1,n));slides.forEach(function(f,i){{f.hidden=i!==at}});document.getElementById('count').textContent=(at+1)+' / '+slides.length}}
document.getElementById('prev').onclick=function(){{show(at-1)}};
document.getElementById('next').onclick=function(){{show(at+1)}};
document.getElementById('full').onclick=function(){{document.documentElement.requestFullscreen&&document.documentElement.requestFullscreen()}};
addEventListener('keydown',function(e){{if(e.key==='ArrowRight'||e.key===' '||e.key==='PageDown')show(at+1);else if(e.key==='ArrowLeft'||e.key==='PageUp')show(at-1);else if(e.key==='Home')show(0);else if(e.key==='End')show(slides.length-1)}});
addEventListener('resize',fit);fit();show(0);
</script></body></html>"#,
        title = escape(title),
    ))
}

/// A relative URL a slide loads, resolved under the deliverable's folder: a regular file there,
/// never reached by `..` out of it nor through a symlink.
fn local_file(base: &Path, folder: &Path, url: &str) -> Option<PathBuf> {
    let url = url.trim();
    let lower = url.to_ascii_lowercase();
    if url.is_empty()
        || url.starts_with('#')
        || url.starts_with('/')
        || [
            "http:",
            "https:",
            "data:",
            "blob:",
            "mailto:",
            "javascript:",
            "about:",
        ]
        .iter()
        .any(|scheme| lower.starts_with(scheme))
    {
        return None;
    }
    let path = url.split(['?', '#']).next()?;
    let mut out = base.to_path_buf();
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                out.pop();
            }
            part => out.push(part),
        }
    }
    let meta = std::fs::symlink_metadata(&out).ok()?;
    (out.starts_with(folder) && meta.is_file() && meta.len() <= INLINE_MAX).then_some(out)
}

fn data_uri(path: &Path) -> Option<String> {
    let bytes = std::fs::read(path).ok()?;
    let mime = chauffeur_design::content_type(path).unwrap_or("application/octet-stream");
    let mime = mime.split(';').next().unwrap_or(mime);
    Some(format!(
        "data:{mime};base64,{}",
        base64::engine::general_purpose::STANDARD.encode(bytes)
    ))
}

/// CSS with its relative `url(...)`s as data URIs, resolved from `base`.
fn inline_css(css: &str, base: &Path, folder: &Path) -> String {
    let url = regex::Regex::new(r#"url\(\s*(['"]?)([^'")]+)['"]?\s*\)"#).expect("valid regex");
    url.replace_all(css, |caps: &regex::Captures| {
        match local_file(base, folder, &caps[2]).and_then(|file| data_uri(&file)) {
            Some(uri) => format!("url(\"{uri}\")"),
            None => caps[0].to_string(),
        }
    })
    .into_owned()
}

/// A slide's HTML with what it loads by relative URL inlined: stylesheets as `<style>`, other
/// files (images, scripts, fonts) as data URIs. What cannot be found is left as it was.
pub fn inline(html: &str, base: &Path, folder: &Path) -> String {
    let link = regex::Regex::new(r#"(?is)<link\b[^>]*>"#).expect("valid regex");
    let href =
        regex::Regex::new(r#"(?is)\bhref\s*=\s*(?:"([^"]*)"|'([^']*)')"#).expect("valid regex");
    let html = link.replace_all(html, |caps: &regex::Captures| {
        let tag = &caps[0];
        if !tag.to_ascii_lowercase().contains("stylesheet") {
            return tag.to_string();
        }
        let Some(target) = href
            .captures(tag)
            .and_then(|c| c.get(1).or_else(|| c.get(2)))
            .map(|m| m.as_str().to_string())
        else {
            return tag.to_string();
        };
        match local_file(base, folder, &target) {
            Some(file) => match std::fs::read_to_string(&file) {
                Ok(css) => format!(
                    "<style>{}</style>",
                    inline_css(&css, file.parent().unwrap_or(base), folder)
                ),
                Err(_) => tag.to_string(),
            },
            None => tag.to_string(),
        }
    });
    let src = regex::Regex::new(r#"(?is)(\b(?:src|poster)\s*=\s*)(?:"([^"]*)"|'([^']*)')"#)
        .expect("valid regex");
    let html = src.replace_all(&html, |caps: &regex::Captures| {
        let target = caps
            .get(2)
            .or_else(|| caps.get(3))
            .map_or("", |m| m.as_str());
        match local_file(base, folder, target).and_then(|file| data_uri(&file)) {
            Some(uri) => format!("{}\"{uri}\"", &caps[1]),
            None => caps[0].to_string(),
        }
    });
    inline_css(&html, base, folder)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_download_inlines_the_deck_s_files_and_nothing_outside_its_folder() {
        let tmp = tempfile::tempdir().unwrap();
        let folder = tmp.path().join("pitch");
        std::fs::create_dir_all(folder.join("brand/assets")).unwrap();
        std::fs::write(
            folder.join("brand/brand.css"),
            "h1{background:url(assets/dot.png)}",
        )
        .unwrap();
        std::fs::write(folder.join("brand/assets/dot.png"), [137, 80, 78, 71]).unwrap();
        std::fs::write(tmp.path().join("secret.txt"), "no").unwrap();
        let html = r#"<link rel="stylesheet" href="brand/brand.css"><img src="brand/assets/dot.png"><img src='../secret.txt'><img src="https://cdn.example/x.png">"#;
        let out = inline(html, &folder, &folder);
        assert!(out.starts_with("<style>h1{background:url(\"data:image/png;base64,"));
        assert!(out.contains("<img src=\"data:image/png;base64,iVBORw==\">"));
        assert!(out.contains("src='../secret.txt'"));
        assert!(out.contains("https://cdn.example/x.png"));
    }

    #[test]
    fn the_printed_page_frames_each_slide_without_the_reader_s_script() {
        let slide = DeckSlide {
            file: "01.html".into(),
            title: "Intro".into(),
            notes: String::new(),
            skip: false,
            url: Some("http://127.0.0.1:1/files/s/w/.chauffeur/artifacts/p/01.html?deck=1".into()),
            version: "1".into(),
        };
        let page = print_page(1920, 1080, &[&slide]);
        assert!(page.contains("@page{size:1920px 1080px;margin:0}"));
        assert!(
            page.contains("src=\"http://127.0.0.1:1/files/s/w/.chauffeur/artifacts/p/01.html\"")
        );
    }
}
