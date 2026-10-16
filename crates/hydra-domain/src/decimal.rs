//! Canonical decimal strings avoid JavaScript's 53-bit integer limit.
use serde::{
    Deserializer, Serializer,
    de::{self, Visitor},
};
use std::{
    fmt::{self, Display},
    marker::PhantomData,
    str::FromStr,
};
pub fn serialize<T: Display, S: Serializer>(value: &T, serializer: S) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(&value.to_string())
}
pub fn deserialize<'de, T: FromStr, D: Deserializer<'de>>(deserializer: D) -> Result<T, D::Error> {
    struct Decimal<T>(PhantomData<T>);
    impl<'de, T: FromStr> Visitor<'de> for Decimal<T> {
        type Value = T;
        fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("a bounded decimal integer string or integer")
        }
        fn visit_str<E: de::Error>(self, value: &str) -> Result<T, E> {
            value
                .parse()
                .map_err(|_| E::custom("invalid decimal integer"))
        }
        fn visit_i64<E: de::Error>(self, value: i64) -> Result<T, E> {
            self.visit_str(&value.to_string())
        }
        fn visit_u64<E: de::Error>(self, value: u64) -> Result<T, E> {
            self.visit_str(&value.to_string())
        }
    }
    deserializer.deserialize_any(Decimal(PhantomData))
}
#[cfg(test)]
mod tests {
    use crate::FieldValue;
    #[test]
    fn preserves_extreme_integers_and_reads_legacy_numbers() {
        for value in [FieldValue::Uint(u64::MAX), FieldValue::Int(i64::MIN)] {
            let json = serde_json::to_string(&value).unwrap();
            assert!(json.contains("\"value\":\""));
            assert_eq!(serde_json::from_str::<FieldValue>(&json).unwrap(), value);
        }
        assert_eq!(
            serde_json::from_str::<FieldValue>(r#"{"type":"uint","value":18446744073709551615}"#)
                .unwrap(),
            FieldValue::Uint(u64::MAX)
        );
        for json in [
            r#"{"type":"uint","value":"-1"}"#,
            r#"{"type":"int","value":"9223372036854775808"}"#,
            r#"{"type":"uint","value":1.5}"#,
        ] {
            assert!(serde_json::from_str::<FieldValue>(json).is_err());
        }
    }
}
