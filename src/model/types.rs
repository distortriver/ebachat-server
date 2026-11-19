// Based on Mattermost server/public/model/utils.go (StringMap, StringArray, StringInterface),
// Copyright (c) 2015-present Mattermost, Inc. Licensed under AGPL-3.0.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize, de::DeserializeOwned};
use sqlx::encode::IsNull;
use sqlx::error::BoxDynError;
use sqlx::postgres::{PgArgumentBuffer, PgTypeInfo, PgValueRef};
use sqlx::types::Json;
use sqlx::{Decode, Encode, Postgres, Type};

/// `map[string]string` in a jsonb column.
pub type StringMap = Json<BTreeMap<String, String>>;
/// `map[string]any` in a jsonb column.
pub type StringInterface = Json<serde_json::Map<String, serde_json::Value>>;
/// `[]string` stored as JSON text in a varchar column (`Posts.FileIds`, `Posts.Filenames`).
pub type StringArray = JsonText<Vec<String>>;

/// A value stored as JSON in a text column, as Go's `driver.Valuer` on these types writes it.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct JsonText<T>(pub T);

impl<T> Type<Postgres> for JsonText<T> {
    fn type_info() -> PgTypeInfo {
        <String as Type<Postgres>>::type_info()
    }
    fn compatible(ty: &PgTypeInfo) -> bool {
        <String as Type<Postgres>>::compatible(ty)
    }
}

impl<T: Serialize> Encode<'_, Postgres> for JsonText<T> {
    fn encode_by_ref(&self, buf: &mut PgArgumentBuffer) -> Result<IsNull, BoxDynError> {
        <String as Encode<Postgres>>::encode(serde_json::to_string(&self.0)?, buf)
    }
}

impl<'r, T: DeserializeOwned + Default> Decode<'r, Postgres> for JsonText<T> {
    fn decode(value: PgValueRef<'r>) -> Result<Self, BoxDynError> {
        let s = <&str as Decode<Postgres>>::decode(value)?;
        // Go's StringArray.Scan of an empty string is an empty array.
        Ok(JsonText(if s.is_empty() { T::default() } else { serde_json::from_str(s)? }))
    }
}
