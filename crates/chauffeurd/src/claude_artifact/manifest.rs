use std::collections::{BTreeMap, HashSet};

use serde::Deserialize;
use serde_json::{Value, json};

use crate::{Error, Result};

#[derive(Deserialize)]
pub struct Manifest {
    pub title: String,
    pages: Vec<Page>,
    boards: BTreeMap<String, Board>,
    #[serde(default)]
    order: Vec<String>,
}
#[derive(Deserialize)]
struct Page {
    id: String,
    name: String,
}
#[derive(Deserialize)]
struct Board {
    title: String,
    page: String,
    w: u32,
    h: u32,
}

pub struct Canvas {
    pub title: String,
    pub pages: Vec<String>,
    pub files: Vec<String>,
    pub manifest: Value,
}

impl Manifest {
    pub fn canvas(self) -> Result<Canvas> {
        if self.boards.is_empty() || self.boards.len() > 100 || self.pages.len() > 100 {
            return Err(invalid("a design must contain between 1 and 100 screens"));
        }
        let mut seen = HashSet::new();
        let files: Vec<String> = self
            .order
            .into_iter()
            .chain(self.boards.keys().cloned())
            .filter(|file| self.boards.contains_key(file) && seen.insert(file.clone()))
            .collect();
        let page_ids: HashSet<_> = self.pages.iter().map(|p| &p.id).collect();
        if page_ids.len() != self.pages.len() {
            return Err(invalid("duplicate pages"));
        }
        for (file, board) in &self.boards {
            if chauffeur_design::safe_relative(file).is_none()
                || !file.ends_with(".html")
                || file.contains(['?', '#', '%'])
                || !(1..=10_000).contains(&board.w)
                || !(1..=10_000).contains(&board.h)
                || !page_ids.contains(&board.page)
            {
                return Err(invalid("a screen has an invalid path, page or size"));
            }
        }
        let pages =
            self.pages
                .iter()
                .map(|page| {
                    let screens: Vec<Value> =
                        files
                            .iter()
                            .filter_map(|file| {
                                let board = &self.boards[file];
                                (board.page == page.id).then(|| json!({
                    "file":file,"label":board.title,"width":board.w,"height":board.h
                }))
                            })
                            .collect();
                    json!({"name":page.name,"sections":[{"title":"","screens":screens}]})
                })
                .collect::<Vec<_>>();
        Ok(Canvas {
            title: self.title,
            pages: self.pages.into_iter().map(|p| p.name).collect(),
            files,
            manifest: json!({"pages":pages}),
        })
    }
}

fn invalid(reason: &str) -> Error {
    Error::BadRequest(format!(
        "This Claude design cannot be imported ({reason}): export its HTML instead."
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source(file: &str) -> Value {
        json!({"title":"Demo","pages":[{"id":"p1","name":"Checkout"}],
            "boards":{file:{"title":"Empty","page":"p1","w":1440,"h":900}},"order":[file]})
    }

    #[test]
    fn pages_screen_names_dimensions_and_order_survive_conversion() {
        let mut value = source("empty.dc.html");
        value["boards"]["ready.dc.html"] = json!({"title":"Ready","page":"p1","w":390,"h":844});
        value["order"] = json!(["ready.dc.html", "empty.dc.html"]);
        let canvas = serde_json::from_value::<Manifest>(value)
            .unwrap()
            .canvas()
            .unwrap();
        assert_eq!(canvas.files, ["ready.dc.html", "empty.dc.html"]);
        assert_eq!(canvas.manifest["pages"][0]["name"], "Checkout");
        let screens = &canvas.manifest["pages"][0]["sections"][0]["screens"];
        assert_eq!(screens[0]["label"], "Ready");
        assert_eq!(screens[1]["width"], 1440);
    }

    #[test]
    fn screen_paths_cannot_escape_or_replace_the_import_manifest() {
        for file in [
            "../secret.html",
            "/etc/secret.html",
            ".hidden/a.html",
            "a%2fsecret.html",
            "a.html?x=1",
            "canvas.json",
        ] {
            assert!(
                serde_json::from_value::<Manifest>(source(file))
                    .unwrap()
                    .canvas()
                    .is_err(),
                "{file}"
            );
        }
    }
}
