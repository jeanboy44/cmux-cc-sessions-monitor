pub mod cmux;
pub mod claude_sessions;
pub mod context;
pub mod hook_sessions;
pub mod session_meta;
pub mod subagents;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Deserializer};

/// Deserializes a field leniently: a value of the wrong type becomes `None`
/// instead of failing the whole record.
pub(crate) fn lenient<'de, D, T>(d: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: DeserializeOwned,
{
    let v = serde_json::Value::deserialize(d)?;
    Ok(serde_json::from_value(v).ok())
}
