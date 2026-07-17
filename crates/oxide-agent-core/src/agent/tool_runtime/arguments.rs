//! Shared deserializers for LLM-produced tool arguments.

use serde::{Deserialize, Deserializer};
use std::fmt::Display;

#[derive(Deserialize)]
#[serde(untagged)]
enum UnsignedInteger {
    Number(u64),
    String(String),
}

impl UnsignedInteger {
    fn into_value<T, E>(self) -> Result<T, E>
    where
        T: TryFrom<u64>,
        T::Error: Display,
        E: serde::de::Error,
    {
        let value = match self {
            Self::Number(value) => value,
            Self::String(value) => value.parse::<u64>().map_err(E::custom)?,
        };

        T::try_from(value).map_err(E::custom)
    }
}

pub(crate) fn deserialize_unsigned<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: TryFrom<u64>,
    T::Error: Display,
{
    UnsignedInteger::deserialize(deserializer)?.into_value::<T, D::Error>()
}

pub(crate) fn deserialize_optional_unsigned<'de, D, T>(
    deserializer: D,
) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: TryFrom<u64>,
    T::Error: Display,
{
    Option::<UnsignedInteger>::deserialize(deserializer)?
        .map(UnsignedInteger::into_value::<T, D::Error>)
        .transpose()
}
