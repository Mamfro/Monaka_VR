//! Takes the player's own folder names out of text they may share (logs, reports): their profile
//! folder (`USERPROFILE`, either slash, any case) becomes `%USERPROFILE%`.

const PLACEHOLDER: &[u8] = b"%USERPROFILE%";

pub struct Scrub {
    /// The profile folder, lowercase with `\` separators, without a trailing one.
    profile: Option<Vec<u8>>,
}

impl Scrub {
    /// For this process's user (`USERPROFILE`).
    pub fn from_env() -> Self {
        Self::for_profile(std::env::var("USERPROFILE").ok().as_deref())
    }

    /// For the profile folder `profile`; none, or a bare drive, scrubs nothing.
    pub fn for_profile(profile: Option<&str>) -> Self {
        let profile = profile.map(|p| p.trim_end_matches(['\\', '/']).bytes().map(fold).collect::<Vec<u8>>()).filter(|p| p.len() > 3);
        Self { profile }
    }

    /// Appends `text` to `out` with the profile folder replaced.
    pub fn bytes(&self, text: &[u8], out: &mut Vec<u8>) {
        let Some(profile) = &self.profile else {
            out.extend_from_slice(text);
            return;
        };
        let (mut at, mut copied) = (0, 0);
        while at + profile.len() <= text.len() {
            let candidate = &text[at..at + profile.len()];
            if candidate.iter().zip(profile).all(|(&c, &p)| fold(c) == p) && text.get(at + profile.len()).is_none_or(|&next| !in_name(next)) {
                out.extend_from_slice(&text[copied..at]);
                out.extend_from_slice(PLACEHOLDER);
                at += profile.len();
                copied = at;
            } else {
                at += 1;
            }
        }
        out.extend_from_slice(&text[copied..]);
    }

    /// `text` with the profile folder replaced.
    pub fn text(&self, text: &str) -> String {
        let mut out = Vec::with_capacity(text.len());
        self.bytes(text.as_bytes(), &mut out);
        // Only whole ASCII runs are replaced, by ASCII, so the text stays UTF-8.
        String::from_utf8(out).unwrap_or_else(|e| String::from_utf8_lossy(e.as_bytes()).into_owned())
    }
}

/// Lowercase, with `/` read as `\`.
fn fold(byte: u8) -> u8 {
    if byte == b'/' { b'\\' } else { byte.to_ascii_lowercase() }
}

/// Whether `byte` would continue a folder name (so a profile `..\bob` does not match `..\bobby`).
fn in_name(byte: u8) -> bool {
    !matches!(byte, b'\\' | b'/' | b'"' | b'\'' | b' ' | b'\t' | b'\r' | b'\n' | b';' | b',' | b')' | b']' | b'>' | b':')
}

#[cfg(test)]
mod tests;
