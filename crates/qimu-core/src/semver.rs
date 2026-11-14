use serde::{Deserialize, Serialize};
use std::fmt;

/// Semantic version X.Y.Z used for content-block rendition versions and
/// index release tags (spec §3.5 / §3.10). "Latest" is the maximum by this
/// ordering. Nobody types numbers in the product UI; versions are produced
/// by clicking one of three increments on the publish confirm card.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct SemVer {
    pub major: u64,
    pub minor: u64,
    pub patch: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Increment {
    /// 大版本: X+1.0.0
    Major,
    /// 小版本: X.Y+1.0
    Minor,
    /// 文字调整: X.Y.Z+1
    Patch,
}

impl SemVer {
    pub fn new(major: u64, minor: u64, patch: u64) -> Self {
        Self { major, minor, patch }
    }

    pub fn parse(s: &str) -> Result<Self, String> {
        let s = s.strip_prefix('V').or_else(|| s.strip_prefix('v')).unwrap_or(s);
        let mut it = s.split('.');
        let mut next = |what: &str| -> Result<u64, String> {
            it.next()
                .ok_or_else(|| format!("missing {what} in version '{s}'"))?
                .parse::<u64>()
                .map_err(|_| format!("invalid {what} in version '{s}'"))
        };
        let v = SemVer::new(next("major")?, next("minor")?, next("patch")?);
        if it.next().is_some() {
            return Err(format!("too many components in version '{s}'"));
        }
        Ok(v)
    }

    pub fn bumped(self, inc: Increment) -> Self {
        match inc {
            Increment::Major => SemVer::new(self.major + 1, 0, 0),
            Increment::Minor => SemVer::new(self.major, self.minor + 1, 0),
            Increment::Patch => SemVer::new(self.major, self.minor, self.patch + 1),
        }
    }
}

impl fmt::Display for SemVer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_and_order() {
        assert_eq!(SemVer::parse("3.2.0").unwrap(), SemVer::new(3, 2, 0));
        assert_eq!(SemVer::parse("V1.10.0").unwrap(), SemVer::new(1, 10, 0));
        assert!(SemVer::new(3, 2, 0) > SemVer::new(3, 1, 9));
        assert!(SemVer::parse("1.0").is_err());
    }

    #[test]
    fn bump() {
        let v = SemVer::new(2, 3, 1);
        assert_eq!(v.bumped(Increment::Major), SemVer::new(3, 0, 0));
        assert_eq!(v.bumped(Increment::Minor), SemVer::new(2, 4, 0));
        assert_eq!(v.bumped(Increment::Patch), SemVer::new(2, 3, 2));
    }
}
