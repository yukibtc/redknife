use std::fmt;
use std::hash::{Hash, Hasher};
use std::str::{self, FromStr};

use thiserror::Error;

const LABEL_MAX_LENGTH: usize = 64;

#[derive(Debug, PartialEq, Eq, Error)]
pub enum LabelError {
    #[error("Label is too big: {LABEL_MAX_LENGTH} <= {0}")]
    Length(usize),
    #[error("Label is empty")]
    Empty,
    #[error("Label is not valid UTF-8")]
    Encoding,
}

#[derive(Debug, Clone)]
pub(super) struct Label(String);

impl Label {
    pub(super) fn from_bytes(bytes: &[u8]) -> Result<Self, LabelError> {
        let label: &str = str::from_utf8(bytes).map_err(|_| LabelError::Encoding)?;
        check_label(label)?;
        Ok(Label(label.to_owned()))
    }

    #[inline]
    pub(super) fn len(&self) -> u8 {
        let len: usize = self.0.len();
        assert!(len < LABEL_MAX_LENGTH);
        len as u8
    }

    #[inline]
    pub(super) fn as_bytes(&self) -> &[u8] {
        self.0.as_bytes()
    }
}

impl fmt::Display for Label {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl Eq for Label {}

impl PartialEq for Label {
    #[inline]
    fn eq(&self, other: &Label) -> bool {
        self.0.eq_ignore_ascii_case(&other.0)
    }
}

impl PartialEq<&str> for Label {
    fn eq(&self, other: &&str) -> bool {
        self.0.eq_ignore_ascii_case(other)
    }
}

impl Hash for Label {
    fn hash<H>(&self, state: &mut H)
    where
        H: Hasher,
    {
        self.0.to_lowercase().hash(state);
    }
}

impl FromStr for Label {
    type Err = LabelError;

    fn from_str(label: &str) -> Result<Self, Self::Err> {
        check_label(label)?;
        Ok(Label(label.to_owned()))
    }
}

#[inline]
fn check_label(label: &str) -> Result<(), LabelError> {
    let len: usize = label.len();

    if len == 0 {
        Err(LabelError::Empty)
    } else if len < LABEL_MAX_LENGTH {
        Ok(())
    } else {
        Err(LabelError::Length(len))
    }
}
