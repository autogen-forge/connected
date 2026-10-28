use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::error::{NetError, Result};
use crate::fingerprint::Fingerprint;

/// A device this one has been paired with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrustedPeer {
    pub name: String,
    pub fingerprint: Fingerprint,
}

/// The devices allowed to connect, kept in a text file with one `<fingerprint> <name>` per
/// line. Lines starting with `#` are comments.
#[derive(Debug, Default)]
pub struct TrustStore {
    path: Option<PathBuf>,
    peers: Vec<TrustedPeer>,
    /// What the file looked like when `peers` was last in step with it.
    stamp: Option<FileStamp>,
}

/// Modification time and size: enough to notice that another process rewrote the file.
type FileStamp = (SystemTime, u64);

fn stamp_of(path: &Path) -> Option<FileStamp> {
    let metadata = fs::metadata(path).ok()?;
    Some((metadata.modified().ok()?, metadata.len()))
}

impl TrustStore {
    /// A store that is never written to disk.
    pub fn in_memory() -> Self {
        Self::default()
    }

    /// Reads the store at `path`; a missing file is an empty store. Changes are saved there.
    pub fn load(path: &Path) -> Result<Self> {
        let peers = match fs::read_to_string(path) {
            Ok(text) => parse(&text)?,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(err) => return Err(err.into()),
        };
        Ok(Self { path: Some(path.to_owned()), peers, stamp: stamp_of(path) })
    }

    /// Picks up changes that other processes made to the file, such as `connected forget`
    /// run while a server is up. A file that has become unreadable or malformed trusts
    /// nobody, since the safe answer to "who is allowed?" is "no one" when that is unknown.
    pub fn refresh(&mut self) {
        let Some(path) = &self.path else {
            return;
        };
        let current = stamp_of(path);
        if current == self.stamp {
            return;
        }
        self.peers = fs::read_to_string(path)
            .ok()
            .and_then(|text| parse(&text).ok())
            .unwrap_or_default();
        self.stamp = current;
    }

    pub fn contains(&self, fingerprint: &Fingerprint) -> bool {
        self.peers.iter().any(|peer| peer.fingerprint == *fingerprint)
    }

    pub fn peers(&self) -> &[TrustedPeer] {
        &self.peers
    }

    /// Trusts `peer`. A device already present under the same fingerprint is renamed.
    pub fn add(&mut self, peer: TrustedPeer) -> Result<()> {
        self.refresh();
        let peer = TrustedPeer { name: sanitize(&peer.name), ..peer };
        match self.peers.iter_mut().find(|p| p.fingerprint == peer.fingerprint) {
            Some(existing) => *existing = peer,
            None => self.peers.push(peer),
        }
        self.save()
    }

    /// Stops trusting the devices that `selector` names: a name, or the start of a
    /// fingerprint. Returns who was removed.
    pub fn remove(&mut self, selector: &str) -> Result<Vec<TrustedPeer>> {
        self.refresh();
        let selector = selector.trim();
        let by_prefix = selector.len() >= 4 && selector.bytes().all(|b| b.is_ascii_hexdigit());
        let (removed, kept): (Vec<_>, Vec<_>) = std::mem::take(&mut self.peers)
            .into_iter()
            .partition(|peer| {
                peer.name == selector
                    || (by_prefix
                        && peer.fingerprint.to_hex().starts_with(&selector.to_ascii_lowercase()))
            });
        self.peers = kept;
        if !removed.is_empty() {
            self.save()?;
        }
        Ok(removed)
    }

    fn save(&mut self) -> Result<()> {
        let Some(path) = self.path.clone() else {
            return Ok(());
        };
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)?;
        }
        let mut text = String::from("# Devices allowed to connect: <sha-256 fingerprint> <name>\n");
        for peer in &self.peers {
            text.push_str(&format!("{} {}\n", peer.fingerprint, peer.name));
        }
        // Write beside the file and rename, so a crash never leaves a half-written list.
        let temp = path.with_extension("tmp");
        fs::write(&temp, text)?;
        fs::rename(&temp, &path)?;
        self.stamp = stamp_of(&path);
        Ok(())
    }
}

fn parse(text: &str) -> Result<Vec<TrustedPeer>> {
    let mut peers = Vec::new();
    for (index, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let bad_line = |why: &str| NetError::TrustFile(format!("line {}: {why}", index + 1));
        let (fingerprint, name) = line
            .split_once(char::is_whitespace)
            .ok_or_else(|| bad_line("expected `<fingerprint> <name>`"))?;
        let fingerprint = fingerprint.parse().map_err(|_| bad_line("not a valid fingerprint"))?;
        peers.push(TrustedPeer { name: name.trim().to_owned(), fingerprint });
    }
    Ok(peers)
}

/// Names go on one line of a text file, so control characters become spaces.
fn sanitize(name: &str) -> String {
    let cleaned: String = name.chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
    let cleaned = cleaned.trim();
    if cleaned.is_empty() { "unnamed".to_owned() } else { cleaned.to_owned() }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fp(seed: u8) -> Fingerprint {
        Fingerprint::of_certificate(&[seed; 8])
    }

    fn peer(name: &str, seed: u8) -> TrustedPeer {
        TrustedPeer { name: name.into(), fingerprint: fp(seed) }
    }

    fn temp_file(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "connected-trust-{label}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = fs::remove_dir_all(&dir);
        dir.join("trusted_peers")
    }

    #[test]
    fn missing_file_is_an_empty_store() {
        let store = TrustStore::load(&temp_file("missing")).unwrap();
        assert!(store.peers().is_empty());
    }

    #[test]
    fn added_peers_are_saved_and_reloaded() {
        let path = temp_file("persist");
        let mut store = TrustStore::load(&path).unwrap();
        store.add(peer("windows pc", 1)).unwrap();
        store.add(peer("linux box", 2)).unwrap();

        let reloaded = TrustStore::load(&path).unwrap();
        assert_eq!(reloaded.peers(), store.peers());
        assert!(reloaded.contains(&fp(1)));
        assert!(!reloaded.contains(&fp(3)));
        let _ = fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn adding_the_same_fingerprint_renames_instead_of_duplicating() {
        let mut store = TrustStore::in_memory();
        store.add(peer("old name", 1)).unwrap();
        store.add(peer("new name", 1)).unwrap();
        assert_eq!(store.peers().len(), 1);
        assert_eq!(store.peers()[0].name, "new name");
    }

    #[test]
    fn peers_are_removed_by_name_or_fingerprint_prefix() {
        let mut store = TrustStore::in_memory();
        store.add(peer("a", 1)).unwrap();
        store.add(peer("b", 2)).unwrap();
        store.add(peer("c", 3)).unwrap();

        assert_eq!(store.remove("a").unwrap().len(), 1);
        let prefix = &fp(2).to_hex()[..8];
        assert_eq!(store.remove(prefix).unwrap()[0].name, "b");
        assert!(store.remove("nobody").unwrap().is_empty());
        assert_eq!(store.peers().len(), 1);
    }

    #[test]
    fn changes_made_through_another_handle_are_picked_up() {
        let path = temp_file("refresh");
        let mut writer = TrustStore::load(&path).unwrap();
        let mut reader = TrustStore::load(&path).unwrap();
        assert!(!reader.contains(&fp(1)));

        writer.add(peer("laptop", 1)).unwrap();
        reader.refresh();
        assert!(reader.contains(&fp(1)));

        writer.remove("laptop").unwrap();
        reader.refresh();
        assert!(!reader.contains(&fp(1)));
        let _ = fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn a_file_that_turns_unreadable_trusts_nobody() {
        let path = temp_file("corrupt");
        let mut store = TrustStore::load(&path).unwrap();
        store.add(peer("laptop", 1)).unwrap();

        fs::write(&path, "this is not a trust file at all\n").unwrap();
        store.refresh();
        assert!(!store.contains(&fp(1)));
        let _ = fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn adding_keeps_entries_written_by_someone_else() {
        let path = temp_file("merge");
        let mut first = TrustStore::load(&path).unwrap();
        let mut second = TrustStore::load(&path).unwrap();

        first.add(peer("one", 1)).unwrap();
        second.add(peer("two", 2)).unwrap();

        let all = TrustStore::load(&path).unwrap();
        assert!(all.contains(&fp(1)) && all.contains(&fp(2)));
        let _ = fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn a_short_selector_never_matches_by_prefix() {
        let mut store = TrustStore::in_memory();
        store.add(peer("a", 1)).unwrap();
        let first_char = &fp(1).to_hex()[..1];
        assert!(store.remove(first_char).unwrap().is_empty());
    }

    #[test]
    fn names_with_newlines_cannot_forge_entries() {
        let mut store = TrustStore::in_memory();
        store.add(peer("evil\n# comment\nabcd", 1)).unwrap();
        assert!(!store.peers()[0].name.contains('\n'));
    }

    #[test]
    fn comments_and_blank_lines_are_ignored_and_bad_lines_rejected() {
        let good = format!("# note\n\n{} my laptop\n", fp(1));
        let peers = parse(&good).unwrap();
        assert_eq!(peers, vec![peer("my laptop", 1)]);

        assert!(matches!(parse("nonsense\n"), Err(NetError::TrustFile(_))));
        assert!(matches!(parse("zz name\n"), Err(NetError::TrustFile(_))));
    }
}
