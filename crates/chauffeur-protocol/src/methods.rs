//! How a method enum is declared: one variant per JSON-RPC method, with its name, params and
//! result in one place. [`methods!`] derives from it the enum serde reads and writes, a fieldless
//! twin naming its methods, and what [`crate::generate`] writes to TypeScript.

use ts_rs::{Config, TypeVisitor};

/// A method enum's fieldless twin: what each method is called, takes and returns.
pub trait Method: Copy + Eq + std::fmt::Debug + 'static {
    /// Every method, in declaration order.
    const ALL: &'static [Self];
    fn name(self) -> &'static str;
    /// Its params' TypeScript type.
    fn params(self, cfg: &Config) -> String;
    /// Its result's TypeScript type; `None` for a notification, which gets no answer.
    fn result(self, cfg: &Config) -> Option<String>;
    /// Its params' TypeScript type written out, fields and their presence included.
    fn params_inline(self, cfg: &Config) -> String;
    /// Visits its params' and result's types.
    fn visit_types(self, visitor: &mut impl TypeVisitor);

    fn from_name(name: &str) -> Option<Self> {
        Self::ALL
            .iter()
            .copied()
            .find(|method| method.name() == name)
    }
}

/// Declares a method enum `$call` and its fieldless twin `$method`. Each variant reads
/// `#[serde(rename = "<method>")] Variant(Params) -> Result,`; a notification has no `-> Result`.
///
/// A client enum also classifies every variant, after its name: `#[access(Scope, flags..)]` says
/// who may call it (`crate::access::Scope`; the flags `phone` and `office` open it to the phone
/// and to the Office in the browser), `#[relay(Relay)]` what becomes of it when its workspace runs
/// on another machine (`crate::access::Relay`). One variant without them and the enum does not
/// compile ("no rules expected `#`").
macro_rules! methods {
    (
        $(#[doc = $enum_doc:literal])*
        pub enum $call:ident / $method:ident {
            $(
                $(#[doc = $doc:literal])*
                #[serde(rename = $name:literal)]
                $variant:ident($params:ty) $(-> $result:ty)?,
            )*
        }
    ) => {
        $(#[doc = $enum_doc])*
        #[derive(serde::Serialize, serde::Deserialize, ts_rs::TS, Clone, Debug)]
        #[serde(tag = "method", content = "params")]
        #[ts(export)]
        pub enum $call {
            $(
                $(#[doc = $doc])*
                #[serde(rename = $name)]
                $variant($params),
            )*
        }

        #[doc = concat!("The methods of [`", stringify!($call), "`], without their params.")]
        #[derive(Clone, Copy, PartialEq, Eq, Debug)]
        pub enum $method {
            $($variant,)*
        }

        impl $call {
            pub fn method(&self) -> $method {
                match self {
                    $($call::$variant(_) => $method::$variant,)*
                }
            }
        }

        impl $crate::methods::Method for $method {
            const ALL: &'static [Self] = &[$($method::$variant,)*];

            fn name(self) -> &'static str {
                match self {
                    $($method::$variant => $name,)*
                }
            }

            fn params(self, cfg: &ts_rs::Config) -> String {
                match self {
                    $($method::$variant => <$params as ts_rs::TS>::name(cfg),)*
                }
            }

            // `cfg` goes unused in an enum of notifications only.
            #[allow(unused_variables)]
            fn result(self, cfg: &ts_rs::Config) -> Option<String> {
                match self {
                    $($method::$variant => None::<String>$(.or(Some(<$result as ts_rs::TS>::name(cfg))))?,)*
                }
            }

            fn params_inline(self, cfg: &ts_rs::Config) -> String {
                match self {
                    $($method::$variant => <$params as ts_rs::TS>::inline(cfg),)*
                }
            }

            fn visit_types(self, visitor: &mut impl ts_rs::TypeVisitor) {
                match self {
                    $($method::$variant => {
                        visitor.visit::<$params>();
                        $(visitor.visit::<$result>();)?
                    })*
                }
            }
        }
    };
    (
        $(#[doc = $enum_doc:literal])*
        pub enum $call:ident / $method:ident {
            $(
                $(#[doc = $doc:literal])*
                #[serde(rename = $name:literal)]
                #[access($scope:ident $(, $flag:ident)*)]
                #[relay($relay:ident)]
                $variant:ident($params:ty) $(-> $result:ty)?,
            )*
        }
    ) => {
        methods! {
            $(#[doc = $enum_doc])*
            pub enum $call / $method {
                $(
                    $(#[doc = $doc])*
                    #[serde(rename = $name)]
                    $variant($params) $(-> $result)?,
                )*
            }
        }

        impl $method {
            /// Who may call it; `crate::access::scope` narrows it by a call's params.
            pub fn scope(self) -> $crate::access::Scope {
                match self {
                    $($method::$variant => $crate::access::Scope::$scope,)*
                }
            }

            /// What becomes of it when its workspace runs on another machine.
            pub fn relay(self) -> $crate::access::Relay {
                match self {
                    $($method::$variant => $crate::access::Relay::$relay,)*
                }
            }

            /// Whether the phone may call it through the mobile host.
            pub fn phone(self) -> bool {
                match self {
                    $($method::$variant => methods!(@flag phone $($flag)*),)*
                }
            }

            /// Whether the Office in the browser may call it.
            pub fn office(self) -> bool {
                match self {
                    $($method::$variant => methods!(@flag office $($flag)*),)*
                }
            }
        }
    };
    (@flag phone phone $($rest:ident)*) => { true };
    (@flag office office $($rest:ident)*) => { true };
    // Any other flag has no rule: a misspelt one does not compile.
    (@flag $wanted:ident phone $($rest:ident)*) => { methods!(@flag $wanted $($rest)*) };
    (@flag $wanted:ident office $($rest:ident)*) => { methods!(@flag $wanted $($rest)*) };
    (@flag $wanted:ident) => { false };
}

/// What a client relies on in `M`: each method's name and its params' fields with their
/// presence (`name?` when optional), one method per line in name order. Neither types nor docs:
/// they change nothing a client sends.
pub fn surface<M: Method>() -> String {
    let cfg = Config::new();
    let mut lines: Vec<String> = M::ALL
        .iter()
        .map(|method| format!("{} {}", method.name(), fields(&method.params_inline(&cfg))))
        .collect();
    lines.sort();
    lines.join("\n")
}

/// [`surface`] hashed (FNV-1a, stable across Rust versions), as `protocol.lock` keeps it.
pub fn fingerprint<M: Method>() -> String {
    let hash = surface::<M>()
        .bytes()
        .fold(0xcbf29ce484222325u64, |hash, byte| {
            (hash ^ u64::from(byte)).wrapping_mul(0x100000001b3)
        });
    format!("{hash:016x}")
}

/// The field names of an inline TypeScript type, `{a, b?}`, one object per alternative of a union;
/// anything else as it is.
fn fields(inline: &str) -> String {
    let text = without_comments(inline);
    if !text.trim_start().starts_with('{') {
        return text.trim().to_string();
    }
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::new();
    let mut depth = 0;
    let mut expecting_key = false;
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        match c {
            '{' | '[' | '(' | '<' => {
                depth += 1;
                if c == '{' && depth == 1 {
                    out.push('{');
                    expecting_key = true;
                }
            }
            '}' | ']' | ')' | '>' => {
                if c == '}' && depth == 1 {
                    out.push('}');
                }
                depth -= 1;
            }
            ',' if depth == 1 => expecting_key = true,
            '|' if depth == 0 => out.push('|'),
            _ if depth == 1 && expecting_key && !c.is_whitespace() => {
                let start = i;
                while i < chars.len() && chars[i] != ':' {
                    i += 1;
                }
                let key: String = chars[start..i].iter().collect();
                if !out.ends_with('{') {
                    out.push(',');
                }
                out.push_str(&key.trim().replace('"', ""));
                expecting_key = false;
            }
            _ => {}
        }
        i += 1;
    }
    out
}

fn without_comments(text: &str) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(start) = rest.find("/*") {
        out.push_str(&rest[..start]);
        rest = rest[start..]
            .find("*/")
            .map_or("", |end| &rest[start + end + 2..]);
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_params_type_reads_as_its_field_names_and_their_presence() {
        assert_eq!(
            fields("{ chat: string, /** Its text. */\ntext?: Array<{ a: number }>, }"),
            "{chat,text?}"
        );
        assert_eq!(
            fields("{ \"action\": \"play\", url?: string, } | { \"action\": \"stop\" }"),
            "{action,url?}|{action}"
        );
        assert_eq!(fields("Record<string, never>"), "Record<string, never>");
    }
}
