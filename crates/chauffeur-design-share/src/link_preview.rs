//! What a chat app shows of a design share's link when it is pasted (see
//! docs/adr/design-link-previews.md): OpenGraph tags on the page of
//! `/s/<id>` with the design's title, its pages and screens, and the image the owner's daemon
//! rendered of its first screens. Anyone who has the link sees them, signed in or not: a chat
//! app's crawler never is. A document's share shows none of it, nor does a workspace's.

use chauffeur_protocol::client::CanvasResult;

use crate::Service;

const MAX_DESCRIPTION: usize = 200;

#[derive(Debug, PartialEq)]
pub(crate) struct LinkPreview {
    pub title: String,
    pub description: String,
    pub url: String,
    /// The image's absolute URL, its version in the query so that an app re-reads a new one;
    /// none before the daemon sent one.
    pub image: Option<String>,
}

impl Service {
    /// The preview of a live design share; none for a document's, a stopped or an unknown one.
    pub(crate) fn link_preview(&self, id: &str) -> Option<LinkPreview> {
        let share = self.share(id).ok()?;
        if share.stopped() || share.document.is_some() {
            return None;
        }
        let base = &self.config.public_url;
        let canvas = self.files.canvas(id, &format!("/v1/shares/{id}/files/"));
        Some(LinkPreview {
            title: share.title.clone(),
            description: describe(&canvas, &share.owner.name),
            url: format!("{base}/s/{id}"),
            image: self
                .files
                .preview(id)
                .map(|(_, version)| format!("{base}/s/{id}/preview.png?v={version}")),
        })
    }
}

/// "Onboarding, Settings · 6 screens · shared by Justin on Chauffeur": the pages when there are
/// several, the screens, the owner.
pub(crate) fn describe(canvas: &CanvasResult, owner: &str) -> String {
    let screens = canvas
        .pages
        .iter()
        .flat_map(|page| &page.sections)
        .map(|section| section.screens.len())
        .sum::<usize>()
        + canvas.unsorted.len();
    let mut parts = Vec::new();
    let named: Vec<&str> = canvas
        .pages
        .iter()
        .map(|page| page.name.trim())
        .filter(|name| !name.is_empty())
        .collect();
    if named.len() > 1 {
        parts.push(named.join(", "));
    }
    parts.push(match screens {
        0 => "No screens yet".to_string(),
        1 => "1 screen".to_string(),
        n => format!("{n} screens"),
    });
    parts.push(format!("shared by {owner} on Chauffeur"));
    let description = parts.join(" · ");
    if description.chars().count() <= MAX_DESCRIPTION {
        return description;
    }
    let cut: String = description.chars().take(MAX_DESCRIPTION - 1).collect();
    format!("{}…", cut.trim_end())
}

impl LinkPreview {
    /// The page with the preview's tags in its head and the design's title as its own.
    pub fn into_page(self, html: String) -> String {
        let title = escape(&self.title);
        let mut tags = vec![
            meta("property", "og:type", "website"),
            meta("property", "og:site_name", "Chauffeur"),
            meta("property", "og:title", &self.title),
            meta("property", "og:description", &self.description),
            meta("property", "og:url", &self.url),
            meta("name", "description", &self.description),
            meta("name", "twitter:title", &self.title),
            meta("name", "twitter:description", &self.description),
        ];
        match &self.image {
            Some(image) => {
                tags.push(meta("property", "og:image", image));
                tags.push(meta("property", "og:image:width", "1200"));
                tags.push(meta("property", "og:image:height", "630"));
                tags.push(meta("property", "og:image:alt", &self.title));
                tags.push(meta("name", "twitter:card", "summary_large_image"));
                tags.push(meta("name", "twitter:image", image));
            }
            None => tags.push(meta("name", "twitter:card", "summary")),
        }
        let tags = tags.concat();
        let html = match (html.find("<title>"), html.find("</title>")) {
            (Some(start), Some(end)) if start < end => format!(
                "{}<title>{title} · Chauffeur</title>{}",
                &html[..start],
                &html[end + "</title>".len()..]
            ),
            _ => html,
        };
        match html.find("</head>") {
            Some(at) => format!("{}{tags}{}", &html[..at], &html[at..]),
            None => format!("{tags}{html}"),
        }
    }
}

fn meta(attribute: &str, key: &str, content: &str) -> String {
    format!(
        "<meta {attribute}=\"{key}\" content=\"{}\" />",
        escape(content)
    )
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

#[cfg(test)]
mod tests {
    use super::*;
    use chauffeur_protocol::client::{CanvasPage, CanvasScreen, CanvasSection};

    fn screen(file: &str) -> CanvasScreen {
        serde_json::from_value(serde_json::json!({
            "file": file, "width": 390, "height": 844, "present": true
        }))
        .unwrap()
    }

    fn canvas(pages: &[(&str, usize)]) -> CanvasResult {
        CanvasResult {
            chat: "c".into(),
            dir: String::new(),
            pages: pages
                .iter()
                .map(|(name, n)| CanvasPage {
                    name: (*name).into(),
                    sections: vec![CanvasSection {
                        title: String::new(),
                        screens: (0..*n)
                            .map(|i| screen(&format!("{name}-{i}.html")))
                            .collect(),
                    }],
                })
                .collect(),
            unsorted: Vec::new(),
            error: None,
        }
    }

    #[test]
    fn a_description_names_the_pages_the_screens_and_the_owner() {
        assert_eq!(
            describe(&canvas(&[("Onboarding", 2), ("Settings", 4)]), "Justin"),
            "Onboarding, Settings · 6 screens · shared by Justin on Chauffeur"
        );
        assert_eq!(
            describe(&canvas(&[("Main", 1)]), "Justin"),
            "1 screen · shared by Justin on Chauffeur"
        );
        assert_eq!(
            describe(&canvas(&[]), "Justin"),
            "No screens yet · shared by Justin on Chauffeur"
        );
        let long: Vec<(String, usize)> = (0..40).map(|i| (format!("Page {i}"), 1)).collect();
        let long: Vec<(&str, usize)> = long.iter().map(|(n, c)| (n.as_str(), *c)).collect();
        let description = describe(&canvas(&long), "Justin");
        assert!(description.chars().count() <= MAX_DESCRIPTION && description.ends_with('…'));
    }

    #[test]
    fn the_page_carries_the_tags_escaped_and_the_design_s_title() {
        let html = "<html><head><title>Chauffeur design</title></head><body></body></html>";
        let page = LinkPreview {
            title: "Checkout <v2> & \"more\"".into(),
            description: "3 screens".into(),
            url: "https://design.example/s/ab".into(),
            image: Some("https://design.example/s/ab/preview.png?v=1".into()),
        }
        .into_page(html.into());
        assert!(
            page.contains("<title>Checkout &lt;v2&gt; &amp; &quot;more&quot; · Chauffeur</title>")
        );
        assert!(page.contains(
            "<meta property=\"og:title\" content=\"Checkout &lt;v2&gt; &amp; &quot;more&quot;\" />"
        ));
        assert!(page.contains(
            "<meta property=\"og:image\" content=\"https://design.example/s/ab/preview.png?v=1\" />"
        ));
        assert!(page.contains("summary_large_image"));
        assert!(page.find("og:title").unwrap() < page.find("</head>").unwrap());

        let without = LinkPreview {
            title: "Checkout".into(),
            description: "3 screens".into(),
            url: "https://design.example/s/ab".into(),
            image: None,
        }
        .into_page(html.into());
        assert!(!without.contains("og:image") && without.contains("content=\"summary\""));
    }
}
