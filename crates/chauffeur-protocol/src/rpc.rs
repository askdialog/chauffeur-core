//! JSON-RPC 2.0 envelopes. Requests are `{"jsonrpc":"2.0","id":…,"method":…,"params":…}`;
//! the method enums of [`crate::client`] and [`crate::sidecar`] serialize to the
//! `method`/`params` pair and are flattened into [`Request`].

use serde::{Deserialize, Serialize};
use serde_json::Value;
use ts_rs::TS;

pub const JSONRPC: &str = "2.0";

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Request<M> {
    pub jsonrpc: String,
    pub id: Value,
    #[serde(flatten)]
    pub call: M,
}

impl<M> Request<M> {
    pub fn new(id: impl Into<Value>, call: M) -> Self {
        Self {
            jsonrpc: JSONRPC.to_string(),
            id: id.into(),
            call,
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Notification<M> {
    pub jsonrpc: String,
    #[serde(flatten)]
    pub call: M,
}

impl<M> Notification<M> {
    pub fn new(call: M) -> Self {
        Self {
            jsonrpc: JSONRPC.to_string(),
            call,
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Response {
    pub jsonrpc: String,
    pub id: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<RpcError>,
}

impl Response {
    pub fn ok(id: Value, result: Value) -> Self {
        Self {
            jsonrpc: JSONRPC.to_string(),
            id,
            result: Some(result),
            error: None,
        }
    }

    pub fn err(id: Value, error: RpcError) -> Self {
        Self {
            jsonrpc: JSONRPC.to_string(),
            id,
            result: None,
            error: Some(error),
        }
    }
}

#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq)]
#[ts(export)]
pub struct RpcError {
    #[ts(type = "number")]
    pub code: i64,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
    /// The daemon's id of this failure, which `errors.report` sends with the app's error toast.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure: Option<String>,
}

/// Declares the error codes as constants and lists them in `codes::ALL`, which
/// [`crate::generate`] writes to TypeScript.
macro_rules! codes {
    ($($(#[doc = $doc:literal])* $name:ident = $value:expr,)*) => {
        /// Error codes. Negative ones are JSON-RPC's own.
        pub mod codes {
            $($(#[doc = $doc])* pub const $name: i64 = $value;)*

            /// Every code, by name.
            pub const ALL: &[(&str, i64)] = &[$((stringify!($name), $name),)*];
        }
    };
}

codes! {
    PARSE_ERROR = -32700,
    INVALID_REQUEST = -32600,
    METHOD_NOT_FOUND = -32601,
    INVALID_PARAMS = -32602,
    INTERNAL = -32603,
    NOT_FOUND = 404,
    CONFLICT = 409,
    BAD_REQUEST = 400,
    UNAUTHORIZED = 401,
    UNAVAILABLE = 503,
    /// The system keyring holding the asked secret is locked: unlocking it lets a retry succeed.
    LOCKED = 423,
}

/// A message read off the wire before it is known to be a request, a notification or a response.
#[derive(Deserialize, Debug)]
pub struct Incoming {
    #[serde(default)]
    pub id: Option<Value>,
    #[serde(default)]
    pub method: Option<String>,
    #[serde(default)]
    pub params: Option<Value>,
    #[serde(default)]
    pub result: Option<Value>,
    #[serde(default)]
    pub error: Option<RpcError>,
}

impl Incoming {
    pub fn is_response(&self) -> bool {
        self.method.is_none() && self.id.is_some()
    }

    /// Rebuilds the `{method, params}` object that the method enums deserialize from.
    pub fn call_value(&self) -> Value {
        let mut call = serde_json::Map::new();
        if let Some(method) = &self.method {
            call.insert("method".into(), Value::String(method.clone()));
        }
        call.insert(
            "params".into(),
            self.params
                .clone()
                .unwrap_or(Value::Object(Default::default())),
        );
        Value::Object(call)
    }
}
