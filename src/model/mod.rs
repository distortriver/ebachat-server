//! API structs and table rows. Go's JSON rules: zero values for missing or `null` fields, `omitempty`, map keys
//! sorted (`BTreeMap`, and `serde_json`'s default sorted `Map`).

mod post;
mod types;
mod user;

pub use post::Post;
pub use types::{JsonText, StringArray, StringInterface, StringMap};
pub use user::User;

use serde::{Deserialize, Deserializer};

/// Go's `omitempty`.
pub fn is_zero<T: Default + PartialEq>(v: &T) -> bool {
    *v == T::default()
}

/// Go's decoder: `null` leaves the zero value.
pub fn null_default<'de, D: Deserializer<'de>, T: Deserialize<'de> + Default>(d: D) -> Result<T, D::Error> {
    Ok(Option::<T>::deserialize(d)?.unwrap_or_default())
}
