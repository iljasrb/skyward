//! The SuperKey a FolkPatch patch is made with: made here from `/dev/urandom`,
//! or taken from the screen and held to the rule APatch states. It is
//! root-equivalent, so it is never logged, never put on a command line, and
//! never written where anything but root can read it.

use anyhow::{Context, Result};
use std::fmt;
use std::fs::File;
use std::io::Read;

/// The shortest key APatch takes, and the longest.
const SHORTEST: usize = 8;
const LONGEST: usize = 63;

/// The length of a key made here.
const MADE: usize = 24;

/// The characters a key is made of, and the highest multiple of them a byte
/// can name: bytes above it are drawn again rather than giving some
/// characters a better chance than others.
const ALPHABET: &[u8; 62] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";
const UNBIASED: u8 = 248;

/// A key the manager takes, which nothing displays by accident.
#[derive(Clone, PartialEq, Eq)]
pub struct SuperKey(String);

impl SuperKey {
    /// The key `text` is, where the manager would take it.
    pub fn validate(text: &str) -> std::result::Result<SuperKey, KeyFault> {
        let mut letters = false;
        let mut digits = false;
        for c in text.chars() {
            match c {
                'a'..='z' | 'A'..='Z' => letters = true,
                '0'..='9' => digits = true,
                _ => return Err(if c.is_ascii() { KeyFault::NotAlphanumeric } else { KeyFault::NotAscii }),
            }
        }
        match text.len() {
            0 => return Err(KeyFault::Empty),
            n if n < SHORTEST => return Err(KeyFault::TooShort),
            n if n > LONGEST => return Err(KeyFault::TooLong),
            _ => {}
        }
        match (letters, digits) {
            (false, _) => Err(KeyFault::NoLetter),
            (_, false) => Err(KeyFault::NoDigit),
            _ => Ok(SuperKey(text.to_string())),
        }
    }

    /// A key made here: `MADE` characters, drawn from `/dev/urandom` without
    /// favouring any of them, and drawn again unless it has a letter and a
    /// digit as the manager asks.
    pub fn generate() -> Result<SuperKey> {
        let mut random = File::open("/dev/urandom").context("open /dev/urandom")?;
        let mut drawn = [0u8; 64];
        loop {
            let mut key = String::with_capacity(MADE);
            while key.len() < MADE {
                random.read_exact(&mut drawn).context("read /dev/urandom")?;
                for byte in drawn {
                    if byte < UNBIASED {
                        key.push(ALPHABET[usize::from(byte % ALPHABET.len() as u8)] as char);
                        if key.len() == MADE {
                            break;
                        }
                    }
                }
            }
            if SuperKey::validate(&key).is_ok() {
                return Ok(SuperKey(key));
            }
        }
    }

    /// The key itself, for the screen to show and for the file only root
    /// reads.
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for SuperKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SuperKey(…)")
    }
}

/// Why the manager would not take a key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyFault {
    Empty,
    TooShort,
    TooLong,
    NotAlphanumeric,
    NotAscii,
    NoLetter,
    NoDigit,
}

impl KeyFault {
    /// What the screen says about it.
    pub fn why(self) -> &'static str {
        match self {
            KeyFault::Empty => "the key is empty",
            KeyFault::TooShort => "the key is shorter than 8 characters",
            KeyFault::TooLong => "the key is longer than 63 characters",
            KeyFault::NotAlphanumeric => "the key holds a character that is neither a letter nor a digit",
            KeyFault::NotAscii => "the key holds a character that is not an English letter or a digit",
            KeyFault::NoLetter => "the key holds no letter",
            KeyFault::NoDigit => "the key holds no digit",
        }
    }
}

impl fmt::Display for KeyFault {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.why())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_key_made_here_is_one_the_manager_takes() {
        for _ in 0..8 {
            let key = SuperKey::generate().unwrap();
            assert_eq!(key.expose().len(), MADE);
            assert_eq!(SuperKey::validate(key.expose()).unwrap(), key, "and it validates");
            assert!(key.expose().chars().any(|c| c.is_ascii_alphabetic()), "{:?}", key);
            assert!(key.expose().chars().any(|c| c.is_ascii_digit()), "{:?}", key);
        }
    }

    #[test]
    fn a_key_of_eight_to_sixty_three_letters_and_digits_is_taken() {
        for text in ["abcd1234", "a1234567b", &"a1".repeat(31), &format!("a1{}", "z".repeat(61))] {
            assert_eq!(SuperKey::validate(text).unwrap().expose(), text);
        }
    }

    #[test]
    fn a_key_the_manager_would_not_take_is_named_for_why() {
        let faults = [
            ("", KeyFault::Empty),
            ("abc123", KeyFault::TooShort),
            (&"a1".repeat(32), KeyFault::TooLong),
            ("abcd 1234", KeyFault::NotAlphanumeric),
            ("abcd.1234", KeyFault::NotAlphanumeric),
            ("abcdé1234", KeyFault::NotAscii),
            ("12345678", KeyFault::NoLetter),
            ("abcdefgh", KeyFault::NoDigit),
        ];
        for (text, fault) in faults {
            assert_eq!(SuperKey::validate(text).unwrap_err(), fault, "{text:?}");
        }
    }

    #[test]
    fn the_key_is_not_in_what_a_line_about_it_says() {
        let key = SuperKey::validate("hidden123").unwrap();
        let line = format!("{key:?}");
        assert!(!line.contains("hidden123"), "{line}");
    }
}
