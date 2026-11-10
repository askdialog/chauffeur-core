//! Workspace slugs (folder names) and branch names.

const ADJECTIVES: &[&str] = &[
    "amber", "brisk", "calm", "deft", "eager", "fleet", "gentle", "hardy", "keen", "lucid",
    "mellow", "nimble", "polar", "quiet", "rapid", "steady", "swift", "tidy", "vivid", "witty",
];
const NOUNS: &[&str] = &[
    "badger", "comet", "delta", "ember", "falcon", "glacier", "harbor", "island", "juniper",
    "kestrel", "lagoon", "meadow", "nebula", "orchard", "pebble", "quarry", "river", "summit",
    "tundra", "willow",
];

/// Lowercase, ASCII, words joined by `-`, at most `max` characters.
pub fn slugify(text: &str, max: usize) -> String {
    let mut out = String::new();
    let mut dash = false;
    for c in text.chars() {
        let c = deunicode_char(c);
        if c.is_ascii_alphanumeric() {
            if dash && !out.is_empty() {
                out.push('-');
            }
            out.push(c.to_ascii_lowercase());
            dash = false;
        } else {
            dash = true;
        }
    }
    if out.len() > max {
        out.truncate(max);
        while out.ends_with('-') {
            out.pop();
        }
    }
    out
}

fn deunicode_char(c: char) -> char {
    match c {
        'à' | 'â' | 'ä' | 'á' => 'a',
        'é' | 'è' | 'ê' | 'ë' => 'e',
        'î' | 'ï' | 'í' => 'i',
        'ô' | 'ö' | 'ó' => 'o',
        'ù' | 'û' | 'ü' | 'ú' => 'u',
        'ç' => 'c',
        other => other,
    }
}

/// A slug for a workspace: `<ticket>-<title>` for one ticket, else an explicit name, else a
/// generated `adjective-noun`.
pub fn workspace_slug(name: Option<&str>, ticket: Option<(&str, Option<&str>)>) -> String {
    if let Some(name) = name.filter(|n| !n.trim().is_empty()) {
        return slugify(name, 60);
    }
    if let Some((identifier, title)) = ticket {
        let title = title.map(|t| slugify(t, 40)).unwrap_or_default();
        let id = slugify(identifier, 20);
        return if title.is_empty() {
            id
        } else {
            format!("{id}-{title}")
        };
    }
    let seed = uuid::Uuid::now_v7().as_u128();
    let adjective = ADJECTIVES[(seed % ADJECTIVES.len() as u128) as usize];
    let noun = NOUNS[((seed / 7) % NOUNS.len() as u128) as usize];
    format!("{adjective}-{noun}")
}

/// Makes `slug` unique with `exists`, by appending `-2`, `-3`…
pub fn unique(slug: &str, mut exists: impl FnMut(&str) -> bool) -> String {
    if !exists(slug) {
        return slug.to_string();
    }
    (2..)
        .map(|n| format!("{slug}-{n}"))
        .find(|candidate| !exists(candidate))
        .expect("an unused suffix exists")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_ticket_slug_joins_the_identifier_and_the_title() {
        assert_eq!(
            workspace_slug(
                None,
                Some(("XYZ-1374", Some("Facet filters: côté widget!")))
            ),
            "xyz-1374-facet-filters-cote-widget"
        );
    }

    #[test]
    fn slugs_are_made_unique() {
        let taken = ["calm-river", "calm-river-2"];
        assert_eq!(unique("calm-river", |s| taken.contains(&s)), "calm-river-3");
    }

    #[test]
    fn long_titles_are_cut_on_a_word() {
        let slug = slugify("a very long title that keeps going and going forever", 20);
        assert!(slug.len() <= 20);
        assert!(!slug.ends_with('-'));
    }
}
