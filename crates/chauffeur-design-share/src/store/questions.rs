use axum::http::StatusCode;
use chauffeur_protocol::design_share::{
    DesignQuestion, DesignQuestionResult, DesignQuestionVote, DesignVoteParams,
};
use rusqlite::{Connection, OptionalExtension, params};

use super::Store;
use crate::{Identity, Reject, internal};

pub(super) const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS design_questions (
    seq INTEGER PRIMARY KEY AUTOINCREMENT,
    share_id TEXT NOT NULL REFERENCES shares(id),
    id TEXT NOT NULL,
    json TEXT NOT NULL,
    UNIQUE (share_id, id)
);
CREATE TABLE IF NOT EXISTS design_question_votes (
    seq INTEGER PRIMARY KEY AUTOINCREMENT,
    question INTEGER NOT NULL REFERENCES design_questions(seq) ON DELETE CASCADE,
    email TEXT NOT NULL,
    name TEXT NOT NULL,
    file TEXT NOT NULL,
    note TEXT NOT NULL,
    UNIQUE (question, email)
);
";

impl Store {
    /// Authorize and apply the operation under the same lock as membership and share changes.
    pub(crate) fn design_questions(
        &self,
        share_id: &str,
        who: &Identity,
        change: Option<QuestionChange<'_>>,
    ) -> Result<Vec<DesignQuestionResult>, Reject> {
        let conn = self.conn.lock().unwrap();
        let share = Self::share_in(&conn, share_id)
            .map_err(internal)?
            .ok_or_else(|| reject(StatusCode::NOT_FOUND, "There is no such share"))?;
        if share.stopped() {
            return Err(reject(StatusCode::GONE, "This share was stopped"));
        }
        if !share.can_view(who) {
            return Err(reject(
                StatusCode::FORBIDDEN,
                "This design was not shared with you",
            ));
        }
        if share.document.is_some() {
            return Err(reject(
                StatusCode::NOT_FOUND,
                "Questions require a design canvas",
            ));
        }
        let owner = who.email == share.owner.email;
        match change {
            Some(QuestionChange::Publish(question, validate)) => {
                if !owner {
                    return Err(reject(
                        StatusCode::FORBIDDEN,
                        "Only the owner can publish questions",
                    ));
                }
                if let Some((_, existing)) =
                    find(&conn, share_id, &question.id).map_err(internal)?
                {
                    if existing != *question {
                        return Err(reject(
                            StatusCode::CONFLICT,
                            "This question was already published",
                        ));
                    }
                } else {
                    validate(question)?;
                    let count: u32 = conn
                        .query_row(
                            "SELECT count(*) FROM design_questions WHERE share_id = ?1",
                            [share_id],
                            |row| row.get(0),
                        )
                        .map_err(internal)?;
                    if count >= 50 {
                        return Err(reject(
                            StatusCode::CONFLICT,
                            "This design already has 50 questions",
                        ));
                    }
                    conn.execute(
                        "INSERT INTO design_questions (share_id, id, json) VALUES (?1, ?2, ?3)",
                        params![
                            share_id,
                            question.id,
                            serde_json::to_string(question).map_err(internal)?
                        ],
                    )
                    .map_err(internal)?;
                }
            }
            Some(QuestionChange::Vote(id, answer)) => {
                let (seq, question) = find(&conn, share_id, id)
                    .map_err(internal)?
                    .ok_or_else(|| reject(StatusCode::NOT_FOUND, "There is no such question"))?;
                if !question
                    .options
                    .iter()
                    .any(|option| option.file == answer.file)
                {
                    return Err(reject(
                        StatusCode::BAD_REQUEST,
                        "Choose a variant from this question",
                    ));
                }
                let changed = conn
                    .execute(
                        "INSERT INTO design_question_votes (question, email, name, file, note)
                     VALUES (?1, ?2, ?3, ?4, ?5) ON CONFLICT (question, email) DO NOTHING",
                        params![seq, who.email, who.name, answer.file, answer.note],
                    )
                    .map_err(internal)?;
                if changed == 0 {
                    let same = conn
                        .query_row(
                            "SELECT file = ?3 AND note = ?4 FROM design_question_votes
                         WHERE question = ?1 AND email = ?2",
                            params![seq, who.email, answer.file, answer.note],
                            |row| row.get::<_, bool>(0),
                        )
                        .map_err(internal)?;
                    if !same {
                        return Err(reject(
                            StatusCode::CONFLICT,
                            "You already voted on this question",
                        ));
                    }
                }
            }
            None => {}
        }
        read(&conn, share_id, who, owner).map_err(internal)
    }
}

pub(crate) enum QuestionChange<'a> {
    Publish(
        &'a DesignQuestion,
        &'a dyn Fn(&DesignQuestion) -> Result<(), Reject>,
    ),
    Vote(&'a str, &'a DesignVoteParams),
}

fn reject(status: StatusCode, reason: &str) -> Reject {
    Reject(status, reason.into())
}

fn find(conn: &Connection, share: &str, id: &str) -> anyhow::Result<Option<(i64, DesignQuestion)>> {
    let row: Option<(i64, String)> = conn
        .query_row(
            "SELECT seq, json FROM design_questions WHERE share_id = ?1 AND id = ?2",
            params![share, id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    match row {
        Some((seq, json)) => Ok(Some((seq, serde_json::from_str(&json)?))),
        None => Ok(None),
    }
}

fn read(
    conn: &Connection,
    share: &str,
    who: &Identity,
    owner: bool,
) -> anyhow::Result<Vec<DesignQuestionResult>> {
    let mut questions =
        conn.prepare("SELECT seq, json FROM design_questions WHERE share_id = ?1 ORDER BY seq")?;
    let mut votes = conn.prepare(
        "SELECT email, name, file, note FROM design_question_votes
         WHERE question = ?1 AND (?2 OR EXISTS (
            SELECT 1 FROM design_question_votes WHERE question = ?1 AND email = ?3
         )) ORDER BY seq",
    )?;
    let mut results = Vec::new();
    let mut rows = questions.query([share])?;
    while let Some(row) = rows.next()? {
        let seq: i64 = row.get(0)?;
        let json: String = row.get(1)?;
        let votes = votes
            .query_map(params![seq, owner, who.email], |row| {
                Ok(DesignQuestionVote {
                    mine: row.get::<_, String>(0)? == who.email,
                    name: row.get(1)?,
                    file: row.get(2)?,
                    note: row.get(3)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        results.push(DesignQuestionResult {
            question: serde_json::from_str(&json)?,
            votes: (owner || !votes.is_empty()).then_some(votes),
        });
    }
    Ok(results)
}
