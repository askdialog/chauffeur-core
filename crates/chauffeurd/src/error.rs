use chauffeur_protocol::rpc::{RpcError, codes};

/// Errors a request can fail with; each maps to a JSON-RPC error code.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    NotFound(String),
    #[error("{0}")]
    Conflict(String),
    /// A conflict with data a client acts on (e.g. the files that keep a promote back).
    #[error("{0}")]
    ConflictWith(String, serde_json::Value),
    #[error("{0}")]
    BadRequest(String),
    /// A bad request with data a client acts on (e.g. the form field at fault).
    #[error("{0}")]
    BadRequestWith(String, serde_json::Value),
    #[error("{0}")]
    Unavailable(String),
    /// The system keyring holds what was asked but stays locked: the user unlocks it and retries.
    #[error("{0}")]
    Locked(String),
    #[error(transparent)]
    Internal(#[from] anyhow::Error),
    /// Another daemon's error, passed on as it gave it (`relay`).
    #[error("{}", .0.message)]
    Remote(RpcError),
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

impl Error {
    pub fn not_found(what: impl std::fmt::Display) -> Self {
        Error::NotFound(format!("{what} not found"))
    }

    pub fn to_rpc(&self) -> RpcError {
        // A typed error raised under `anyhow` keeps its code and data, with the chain's message.
        if let Error::Internal(error) = self
            && let Some(inner) = error.downcast_ref::<Error>()
        {
            return RpcError {
                message: format!("{error:#}"),
                ..inner.to_rpc()
            };
        }
        let code = match self {
            Error::NotFound(_) => codes::NOT_FOUND,
            Error::Conflict(_) | Error::ConflictWith(..) => codes::CONFLICT,
            Error::BadRequest(_) | Error::BadRequestWith(..) => codes::BAD_REQUEST,
            Error::Unavailable(_) => codes::UNAVAILABLE,
            Error::Locked(_) => codes::LOCKED,
            Error::Internal(_) => codes::INTERNAL,
            Error::Remote(error) => return error.clone(),
        };
        let message = match self {
            Error::Internal(error) => format!("{error:#}"),
            other => other.to_string(),
        };
        let data = match self {
            Error::ConflictWith(_, data) | Error::BadRequestWith(_, data) => Some(data.clone()),
            _ => None,
        };
        RpcError {
            code,
            message,
            data,
            failure: None,
        }
    }
}

impl From<rusqlite::Error> for Error {
    fn from(error: rusqlite::Error) -> Self {
        Error::Internal(error.into())
    }
}

impl From<std::io::Error> for Error {
    fn from(error: std::io::Error) -> Self {
        Error::Internal(error.into())
    }
}

impl From<serde_json::Error> for Error {
    fn from(error: serde_json::Error) -> Self {
        Error::Internal(error.into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_typed_error_raised_under_anyhow_keeps_its_code_and_data_with_the_chain_s_message() {
        let typed = Error::ConflictWith("edited".into(), serde_json::json!({ "x": 1 }));
        let rpc = Error::Internal(anyhow::Error::from(typed).context("restoring")).to_rpc();
        assert_eq!(rpc.code, codes::CONFLICT);
        assert_eq!(rpc.data, Some(serde_json::json!({ "x": 1 })));
        assert_eq!(rpc.message, "restoring: edited");
        let plain = Error::Internal(anyhow::anyhow!("disk full")).to_rpc();
        assert_eq!((plain.code, plain.data), (codes::INTERNAL, None));
    }
}
