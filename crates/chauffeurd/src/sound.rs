//! The sound played when a chat finishes or needs the user (`sound` in `[notifications]`): one of
//! the app's own, or an audio file of the user's, which the daemon serves to the app (`/sound`).

/// The app's own sounds, made by the app itself: no file, no licence.
pub const BUILT_IN: [&str; 3] = ["chime", "bell", "station"];
/// Longer than any notification sound.
pub const MAX_BYTES: u64 = 5 << 20;

/// What `sound` names: nothing, one of the app's own, or a file.
#[derive(Debug, PartialEq, Eq)]
pub enum Sound<'a> {
    None,
    BuiltIn(&'a str),
    File(&'a str),
}

pub fn kind(sound: &str) -> Sound<'_> {
    match sound.trim() {
        "" | "none" => Sound::None,
        name if BUILT_IN.contains(&name) => Sound::BuiltIn(name),
        path => Sound::File(path),
    }
}

/// `~/` expanded.
pub fn expand(path: &str) -> std::path::PathBuf {
    match path.strip_prefix("~/") {
        Some(rest) => std::env::var_os("HOME")
            .map(|home| std::path::Path::new(&home).join(rest))
            .unwrap_or_else(|| path.into()),
        None => path.into(),
    }
}

/// The user's sound file and its media type, or why it cannot be played.
pub fn read(path: &str) -> Result<(Vec<u8>, &'static str), String> {
    let path = expand(path);
    let meta = std::fs::metadata(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    if !meta.is_file() || meta.len() > MAX_BYTES {
        return Err(format!(
            "{} is not a sound file of at most {} MB",
            path.display(),
            MAX_BYTES >> 20
        ));
    }
    let bytes = std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let media_type = sniff(&bytes).ok_or_else(|| {
        format!(
            "{} is not an audio file (MP3, WAV, AIFF, M4A, OGG or FLAC)",
            path.display()
        )
    })?;
    Ok((bytes, media_type))
}

/// The audio type from the first bytes; never trust a name for that.
pub fn sniff(bytes: &[u8]) -> Option<&'static str> {
    let at = |range: std::ops::Range<usize>| bytes.get(range);
    if at(0..4) == Some(b"RIFF") && at(8..12) == Some(b"WAVE") {
        Some("audio/wav")
    } else if at(0..4) == Some(b"FORM") && matches!(at(8..12), Some(b"AIFF") | Some(b"AIFC")) {
        Some("audio/aiff")
    } else if at(4..8) == Some(b"ftyp") {
        Some("audio/mp4")
    } else if at(0..4) == Some(b"OggS") {
        Some("audio/ogg")
    } else if at(0..4) == Some(b"fLaC") {
        Some("audio/flac")
    } else if at(0..3) == Some(b"ID3")
        || (bytes.len() > 1 && bytes[0] == 0xFF && bytes[1] & 0xE0 == 0xE0)
    {
        Some("audio/mpeg")
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_sound_is_nothing_one_of_the_apps_or_a_file() {
        assert_eq!(kind(""), Sound::None);
        assert_eq!(kind("none"), Sound::None);
        assert_eq!(kind(" station "), Sound::BuiltIn("station"));
        assert_eq!(kind("~/sncf.mp3"), Sound::File("~/sncf.mp3"));
    }

    #[test]
    fn audio_is_known_by_its_bytes() {
        assert_eq!(sniff(b"RIFF\0\0\0\0WAVEfmt "), Some("audio/wav"));
        assert_eq!(sniff(b"ID3\x04\0\0"), Some("audio/mpeg"));
        assert_eq!(sniff(&[0xFF, 0xFB, 0x90, 0x00]), Some("audio/mpeg"));
        assert_eq!(sniff(b"\0\0\0\x20ftypM4A "), Some("audio/mp4"));
        assert_eq!(sniff(b"FORM\0\0\0\0AIFF"), Some("audio/aiff"));
        assert_eq!(sniff(b"OggS\0"), Some("audio/ogg"));
        assert_eq!(sniff(b"fLaC"), Some("audio/flac"));
        assert_eq!(sniff(b"<html>"), None);
        assert_eq!(sniff(b"\x89PNG\r\n\x1a\n"), None);
    }

    #[test]
    fn a_file_that_is_not_audio_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let text = dir.path().join("notes.mp3");
        std::fs::write(&text, "not a sound").unwrap();
        assert!(
            read(text.to_str().unwrap())
                .unwrap_err()
                .contains("not an audio file")
        );
        let wav = dir.path().join("ding.wav");
        std::fs::write(&wav, b"RIFF\0\0\0\0WAVEfmt ").unwrap();
        assert_eq!(read(wav.to_str().unwrap()).unwrap().1, "audio/wav");
        assert!(read("/no/such/file.mp3").is_err());
    }
}
