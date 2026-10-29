//! Finding rekordbox tracks whose files moved.
//!
//! rekordbox keeps the absolute path it imported a file from. When a music
//! folder moves (another drive, a sync folder like Dropbox / Synology Drive),
//! every `FolderPath` under it dangles and the import skips those tracks. The
//! relocator indexes the audio files under folders the user picks and matches
//! a missing path by file name, preferring the candidate that shares the most
//! trailing path components (`Artist/Album/01 Track.aiff`). A tie is left
//! unresolved rather than guessed: importing the wrong file would attach the
//! track's cues and beat grid to different audio.

use std::collections::{HashMap, HashSet};
use std::path::{Component, Path, PathBuf};

use unicode_normalization::UnicodeNormalization;
use walkdir::WalkDir;

use crate::models::RelocationGroup;
use crate::scanner::is_audio_path;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Relocation {
    Found(PathBuf),
    /// Several files fit equally well (or several tracks want the same file).
    Ambiguous,
    /// Only the file name matches and the size can't confirm it.
    Weak,
    NotFound,
}

pub(super) struct TrackRelocator {
    by_name: HashMap<String, Vec<PathBuf>>,
    indexed_files: usize,
}

/// NFC, then lowercased: a sync folder may store `Ö` decomposed where
/// rekordbox kept it composed, and a case-only rename should still match
/// (APFS and most sync folders are case-insensitive anyway).
fn name_key(name: &str) -> String {
    name.nfc().collect::<String>().to_lowercase()
}

fn components_lower(path: &Path) -> Vec<String> {
    path.components()
        .filter_map(|c| match c {
            Component::Normal(s) => Some(name_key(&s.to_string_lossy())),
            _ => None,
        })
        .collect()
}

/// How many path components, counted from the file name, `a` and `b` share.
fn shared_tail(a: &[String], b: &[String]) -> usize {
    a.iter()
        .rev()
        .zip(b.iter().rev())
        .take_while(|(x, y)| x == y)
        .count()
}

impl TrackRelocator {
    /// Index the audio files under `roots`. Hidden directories (`.Trash`,
    /// `.sync`, ...) are skipped; symlinks are not followed so a link back up
    /// the tree can't loop. A file under two overlapping roots counts once.
    pub(super) fn index(roots: &[String]) -> Self {
        let mut by_name: HashMap<String, Vec<PathBuf>> = HashMap::new();
        let mut seen: HashSet<PathBuf> = HashSet::new();
        for root in roots.iter().map(|r| r.trim()).filter(|r| !r.is_empty()) {
            let walker = WalkDir::new(root).into_iter().filter_entry(|e| {
                e.depth() == 0 || !e.file_name().to_string_lossy().starts_with('.')
            });
            for entry in walker.filter_map(Result::ok) {
                if !entry.file_type().is_file() || !is_audio_path(entry.path()) {
                    continue;
                }
                if !seen.insert(entry.path().to_path_buf()) {
                    continue;
                }
                let key = name_key(&entry.file_name().to_string_lossy());
                by_name.entry(key).or_default().push(entry.into_path());
            }
        }
        Self {
            by_name,
            indexed_files: seen.len(),
        }
    }

    pub(super) fn indexed_files(&self) -> usize {
        self.indexed_files
    }

    /// The file `missing` moved to. `size` is rekordbox's `FileSize`;
    /// `claimed` are files other tracks still point at, never a target.
    ///
    /// A candidate sharing at least the parent folder is taken; one sharing
    /// only the file name (`01 Intro.mp3` is everywhere) needs the same size.
    /// Equally good candidates are told apart by size, else ambiguous.
    pub(super) fn resolve(
        &self,
        missing: &Path,
        size: Option<u64>,
        claimed: &HashSet<PathBuf>,
    ) -> Relocation {
        let Some(name) = missing.file_name() else {
            return Relocation::NotFound;
        };
        let Some(candidates) = self.by_name.get(&name_key(&name.to_string_lossy())) else {
            return Relocation::NotFound;
        };
        let wanted = components_lower(missing);
        let scored: Vec<(&PathBuf, usize)> = candidates
            .iter()
            .filter(|c| !claimed.contains(*c))
            .map(|c| (c, shared_tail(&wanted, &components_lower(c))))
            .collect();
        let Some(top) = scored.iter().map(|(_, s)| *s).max() else {
            return Relocation::NotFound;
        };
        let best: Vec<&PathBuf> = scored
            .iter()
            .filter(|(_, s)| *s == top)
            .map(|(c, _)| *c)
            .collect();
        let same_size = |c: &&PathBuf| {
            size.is_some_and(|want| std::fs::metadata(c).is_ok_and(|m| m.len() == want))
        };
        let sized: Vec<&PathBuf> = best.iter().copied().filter(same_size).collect();
        match (best.as_slice(), sized.as_slice()) {
            (_, [only]) => Relocation::Found((*only).clone()),
            ([only], []) if top >= 2 => Relocation::Found((*only).clone()),
            ([_], []) => Relocation::Weak,
            _ => Relocation::Ambiguous,
        }
    }
}

/// How many trailing path components `old` and `new` share (file name = 1).
pub(super) fn match_score(old: &Path, new: &Path) -> usize {
    shared_tail(&components_lower(old), &components_lower(new))
}

/// The folder that moved: `old` and `new` with their shared tail removed.
pub(super) fn moved_prefixes(old: &Path, new: &Path) -> (String, String) {
    let old_parts: Vec<_> = old.components().collect();
    let new_parts: Vec<_> = new.components().collect();
    let shared = match_score(old, new);
    let prefix = |parts: &[Component], keep: usize| -> String {
        parts[..keep].iter().collect::<PathBuf>().to_string_lossy().into_owned()
    };
    (
        prefix(&old_parts, old_parts.len().saturating_sub(shared)),
        prefix(&new_parts, new_parts.len().saturating_sub(shared)),
    )
}

/// Counts relocations per moved folder, largest first.
#[derive(Default)]
pub(super) struct RelocationGroups(HashMap<(String, String), usize>);

impl RelocationGroups {
    pub(super) fn add(&mut self, old: &Path, new: &Path) {
        *self.0.entry(moved_prefixes(old, new)).or_default() += 1;
    }

    pub(super) fn into_sorted(self) -> Vec<RelocationGroup> {
        let mut groups: Vec<_> = self
            .0
            .into_iter()
            .map(|((from_prefix, to_prefix), count)| RelocationGroup {
                from_prefix,
                to_prefix,
                count,
            })
            .collect();
        groups.sort_by(|a, b| b.count.cmp(&a.count).then(a.from_prefix.cmp(&b.from_prefix)));
        groups
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn touch(path: &Path) {
        write(path, b"x");
    }

    fn write(path: &Path, bytes: &[u8]) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, bytes).unwrap();
    }

    fn none() -> HashSet<PathBuf> {
        HashSet::new()
    }

    fn index(dir: &Path) -> TrackRelocator {
        TrackRelocator::index(&[dir.to_string_lossy().into()])
    }

    #[test]
    fn a_name_only_match_needs_the_same_size() {
        let dir = tempfile::tempdir().unwrap();
        let other_album = dir.path().join("Album B/01 Intro.mp3");
        write(&other_album, b"12345");
        let relocator = index(dir.path());
        let missing = Path::new("/Old/Album A/01 Intro.mp3");
        assert_eq!(relocator.resolve(missing, None, &none()), Relocation::Weak);
        assert_eq!(relocator.resolve(missing, Some(9), &none()), Relocation::Weak);
        assert_eq!(
            relocator.resolve(missing, Some(5), &none()),
            Relocation::Found(other_album)
        );
    }

    #[test]
    fn size_tells_equally_good_matches_apart() {
        let dir = tempfile::tempdir().unwrap();
        let small = dir.path().join("a/Track.mp3");
        write(&small, b"12");
        write(&dir.path().join("b/Track.mp3"), b"1234");
        let relocator = index(dir.path());
        assert_eq!(
            relocator.resolve(Path::new("/old/c/Track.mp3"), Some(2), &none()),
            Relocation::Found(small)
        );
    }

    #[test]
    fn never_takes_a_file_another_track_still_uses() {
        let dir = tempfile::tempdir().unwrap();
        let used = dir.path().join("Music/A/01.mp3");
        touch(&used);
        let relocator = index(dir.path());
        let missing = Path::new("/Old/Music/A/01.mp3");
        assert_eq!(
            relocator.resolve(missing, None, &none()),
            Relocation::Found(used.clone())
        );
        let claimed: HashSet<PathBuf> = [used].into_iter().collect();
        assert_eq!(relocator.resolve(missing, None, &claimed), Relocation::NotFound);
    }

    #[test]
    fn overlapping_roots_index_a_file_once() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("Sync/DJSET/Artist/Track.mp3");
        touch(&file);
        let relocator = TrackRelocator::index(&[
            dir.path().join("Sync").to_string_lossy().into(),
            dir.path().join("Sync/DJSET").to_string_lossy().into(),
        ]);
        assert_eq!(relocator.indexed_files(), 1);
        assert_eq!(
            relocator.resolve(Path::new("/Old/Artist/Track.mp3"), None, &none()),
            Relocation::Found(file)
        );
    }

    #[test]
    fn finds_a_moved_file_and_prefers_the_longest_shared_tail() {
        let dir = tempfile::tempdir().unwrap();
        let synced = dir.path().join("Sync/DJSET/Music/Artist/Album/01 Track.aiff");
        let other = dir.path().join("Sync/DJSET/Dupes/01 Track.aiff");
        touch(&synced);
        touch(&other);
        let relocator = TrackRelocator::index(&[dir.path().join("Sync").to_string_lossy().into()]);
        assert_eq!(relocator.indexed_files(), 2);

        let missing = Path::new("/DJSET/Music/Artist/Album/01 Track.aiff");
        assert_eq!(relocator.resolve(missing, None, &none()), Relocation::Found(synced));
    }

    #[test]
    fn equally_good_matches_are_ambiguous_not_guessed() {
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("a/Track.mp3"));
        touch(&dir.path().join("b/Track.mp3"));
        let relocator = TrackRelocator::index(&[dir.path().to_string_lossy().into()]);
        assert_eq!(
            relocator.resolve(Path::new("/old/c/Track.mp3"), None, &none()),
            Relocation::Ambiguous
        );
    }

    #[test]
    fn skips_hidden_folders_non_audio_and_unknown_names() {
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join(".Trash/Gone.mp3"));
        touch(&dir.path().join("notes/Gone.txt"));
        let relocator = TrackRelocator::index(&[dir.path().to_string_lossy().into()]);
        assert_eq!(relocator.indexed_files(), 0);
        assert_eq!(
            relocator.resolve(Path::new("/old/Gone.mp3"), None, &none()),
            Relocation::NotFound
        );
    }

    #[test]
    fn matches_names_case_insensitively() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("Music/track.MP3");
        touch(&file);
        let relocator = TrackRelocator::index(&[dir.path().to_string_lossy().into()]);
        assert_eq!(
            relocator.resolve(Path::new("/Old/Music/Track.mp3"), None, &none()),
            Relocation::Found(file)
        );
    }

    #[test]
    fn matches_composed_and_decomposed_accents() {
        let dir = tempfile::tempdir().unwrap();
        // Decomposed: O + U+0308 COMBINING DIAERESIS, as some sync folders store it.
        let file = dir.path().join("Ru\u{308}fu\u{308}s/Feat. O\u{308}hrn.aiff");
        touch(&file);
        let relocator = TrackRelocator::index(&[dir.path().to_string_lossy().into()]);
        let found = relocator.resolve(Path::new("/Old/R\u{fc}f\u{fc}s/Feat. \u{d6}hrn.aiff"), None, &none());
        assert_eq!(found, Relocation::Found(file.clone()));
        let (from, to) = moved_prefixes(
            Path::new("/Old/R\u{fc}f\u{fc}s/Feat. \u{d6}hrn.aiff"),
            &file,
        );
        assert_eq!(from, "/Old");
        assert_eq!(to, dir.path().to_string_lossy());
    }

    #[test]
    fn groups_by_the_folder_that_moved() {
        let mut groups = RelocationGroups::default();
        groups.add(
            Path::new("/DJSET/Music/A/1.aiff"),
            Path::new("/Users/me/Sync/DJSET/Music/A/1.aiff"),
        );
        groups.add(
            Path::new("/DJSET/Music/B/2.aiff"),
            Path::new("/Users/me/Sync/DJSET/Music/B/2.aiff"),
        );
        groups.add(Path::new("/Downloads/x.mp3"), Path::new("/Users/me/Sync/new/x.mp3"));
        assert_eq!(
            groups.into_sorted(),
            vec![
                RelocationGroup {
                    from_prefix: "/".into(),
                    to_prefix: "/Users/me/Sync".into(),
                    count: 2,
                },
                RelocationGroup {
                    from_prefix: "/Downloads".into(),
                    to_prefix: "/Users/me/Sync/new".into(),
                    count: 1,
                },
            ]
        );
    }
}
