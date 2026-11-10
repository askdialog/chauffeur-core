//! How each embedding model wants its text, and how far a match may be. Measured on a synthetic
//! French, English and code sample (docs/adr/local-hybrid-search.md): a relevant passage scored
//! at least 0.32 with embeddinggemma, an unrelated one at most 0.25.

/// The prompts and the floor of one model, chosen by its name.
pub struct Embedding<'a> {
    model: &'a str,
}

impl<'a> Embedding<'a> {
    pub fn new(model: &'a str) -> Self {
        Embedding { model }
    }

    fn family(&self) -> Family {
        let name = self.model.rsplit('/').next().unwrap_or(self.model);
        if name.starts_with("embeddinggemma") {
            Family::Gemma
        } else if name.starts_with("qwen3-embedding") {
            Family::Qwen3
        } else {
            Family::Other
        }
    }

    pub fn document(&self, title: &str, text: &str) -> String {
        match self.family() {
            Family::Gemma => {
                let title = if title.trim().is_empty() {
                    "none"
                } else {
                    title.trim()
                };
                format!("title: {title} | text: {text}")
            }
            Family::Qwen3 | Family::Other => text.to_string(),
        }
    }

    pub fn query(&self, query: &str) -> String {
        match self.family() {
            Family::Gemma => format!("task: search result | query: {query}"),
            Family::Qwen3 => format!(
                "Instruct: Given a search query, retrieve the chat message, note or title that answers it\nQuery: {query}"
            ),
            Family::Other => query.to_string(),
        }
    }

    /// The largest cosine distance (1 − similarity) of a match by meaning: farther is noise.
    pub fn max_distance(&self) -> f32 {
        match self.family() {
            Family::Gemma => 0.70,
            Family::Qwen3 => 0.64,
            Family::Other => 1.0,
        }
    }
}

impl Embedding<'_> {
    /// The least cosine similarity of two messages asking for the same work, worded differently
    /// (Find routines). With embeddinggemma, such pairs measured 0.57 to 0.80, French against
    /// English included, and unrelated requests of the same developer 0.45 at most.
    pub fn same_request(&self) -> f32 {
        match self.family() {
            Family::Gemma => 0.55,
            // Its unrelated passages already score up to 0.44 against a query.
            Family::Qwen3 => 0.65,
            Family::Other => 0.8,
        }
    }
}

enum Family {
    Gemma,
    Qwen3,
    /// A model we did not measure: plain text, no floor.
    Other,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embeddinggemma_gets_its_task_prompts_and_others_plain_text() {
        let gemma = Embedding::new("embeddinggemma:latest");
        assert_eq!(
            gemma.query("facture"),
            "task: search result | query: facture"
        );
        assert_eq!(gemma.document("", "x"), "title: none | text: x");
        assert_eq!(gemma.document("plan.md", "x"), "title: plan.md | text: x");
        let qwen = Embedding::new("qwen3-embedding:0.6b");
        assert!(qwen.query("facture").ends_with("\nQuery: facture"));
        assert_eq!(qwen.document("t", "x"), "x");
        let other = Embedding::new("nomic-embed-text");
        assert_eq!(other.query("facture"), "facture");
        assert_eq!(other.max_distance(), 1.0);
    }
}
