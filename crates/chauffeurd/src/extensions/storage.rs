//! Portable extension storage. A remote runtime sends statements to Core; only Core opens the
//! extension's private SQLite file. An in-process adapter keeps the existing integration tests.
use crate::{Error, Result, store::Store};
use chauffeur_extension_api::Host;
pub use chauffeur_protocol::extension_host::ExtensionStatement as Statement;
use rusqlite::types::{Value as SqlValue, ValueRef};
use serde_json::Value;

#[derive(Clone)]
pub enum Storage {
    Local(Store),
    Remote(Host),
}

pub fn failure(error: chauffeur_extension_api::Failure) -> Error {
    Error::Remote(chauffeur_protocol::rpc::RpcError {
        code: error.code,
        message: error.message,
        data: error.data,
        failure: None,
    })
}

pub fn parameters(values: &[Value]) -> Result<Vec<SqlValue>> {
    values
        .iter()
        .map(|value| {
            Ok(match value {
                Value::Null => SqlValue::Null,
                Value::Bool(value) => SqlValue::Integer(i64::from(*value)),
                Value::String(value) => SqlValue::Text(value.clone()),
                Value::Number(value) => {
                    if let Some(value) = value.as_i64() {
                        SqlValue::Integer(value)
                    } else if let Some(value) = value.as_f64() {
                        SqlValue::Real(value)
                    } else {
                        return Err(Error::BadRequest(
                            "This SQLite value is too large; use a string".into(),
                        ));
                    }
                }
                _ => {
                    return Err(Error::BadRequest(
                        "SQLite parameters must be scalar values; encode the object as JSON".into(),
                    ));
                }
            })
        })
        .collect()
}

pub fn query(
    connection: &rusqlite::Connection,
    sql: &str,
    values: &[Value],
) -> Result<Vec<Vec<Value>>> {
    let params = parameters(values)?;
    let mut statement = connection.prepare(sql)?;
    if !statement.readonly() {
        return Err(Error::BadRequest(
            "This storage request changes data; send it as a transaction".into(),
        ));
    }
    let columns = statement.column_count();
    let mut rows = statement.query(rusqlite::params_from_iter(params))?;
    let mut result = Vec::new();
    while let Some(row) = rows.next()? {
        let mut values = Vec::with_capacity(columns);
        for column in 0..columns {
            values.push(match row.get_ref(column)? {
                ValueRef::Null => Value::Null,
                ValueRef::Integer(value) => value.into(),
                ValueRef::Real(value) => {
                    serde_json::Number::from_f64(value).map_or(Value::Null, Value::Number)
                }
                ValueRef::Text(value) => Value::String(
                    String::from_utf8(value.to_vec())
                        .map_err(|error| Error::Internal(error.into()))?,
                ),
                ValueRef::Blob(_) => {
                    return Err(Error::BadRequest(
                        "This storage value is binary; encode it before reading it".into(),
                    ));
                }
            });
        }
        result.push(values);
    }
    Ok(result)
}

pub fn batch(connection: &rusqlite::Connection, statements: &[Statement]) -> Result<()> {
    let transaction = connection.unchecked_transaction()?;
    for statement in statements {
        transaction.execute(
            &statement.sql,
            rusqlite::params_from_iter(parameters(&statement.params)?),
        )?;
    }
    transaction.commit()?;
    Ok(())
}

impl Storage {
    pub fn query(&self, sql: &str, params: Vec<Value>) -> Result<Vec<Vec<Value>>> {
        match self {
            Self::Local(store) => store.with(|connection| Ok(query(connection, sql, &params)))?,
            Self::Remote(host) => host
                .call(
                    "storage.query",
                    Statement {
                        sql: sql.into(),
                        params,
                    },
                )
                .map_err(failure),
        }
    }

    pub fn batch(&self, statements: Vec<Statement>) -> Result<()> {
        match self {
            Self::Local(store) => store.with(|connection| Ok(batch(connection, &statements)))?,
            Self::Remote(host) => host.call("storage.batch", statements).map_err(failure),
        }
    }

    pub fn execute(&self, sql: &str, params: Vec<Value>) -> Result<()> {
        self.batch(vec![Statement {
            sql: sql.into(),
            params,
        }])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn a_failed_storage_transaction_keeps_the_previous_values() {
        let store = Store::open_in_memory().unwrap();
        let storage = Storage::Local(store);
        storage
            .execute(
                "INSERT INTO kv(key,value) VALUES(?1,?2)",
                vec!["test".into(), "before".into()],
            )
            .unwrap();
        assert!(
            storage
                .batch(vec![
                    Statement {
                        sql: "UPDATE kv SET value=?1 WHERE key=?2".into(),
                        params: vec!["after".into(), "test".into()]
                    },
                    Statement {
                        sql: "INSERT INTO missing_table VALUES(1)".into(),
                        params: vec![]
                    },
                ])
                .is_err()
        );
        assert_eq!(
            storage
                .query("SELECT value FROM kv WHERE key=?1", vec!["test".into()])
                .unwrap(),
            vec![vec![Value::String("before".into())]]
        );
    }
}
