//! Game packs: a producer DLL read as a pack, from its file and without loading it. Its
//! `MONAKA_CONTRACT` and `MONAKA_MANIFEST` exports ([`monaka_core::pack`]) say which contract it
//! speaks, who made it and which games it serves, with their options; nothing of the pack runs in
//! the launcher.
//!
//! Packs the player adds live in the games folder ([`crate::Paths::packs`]), one folder per pack:
//! its DLL and its licence files. [`scan`] reads them all. [`stage`] unpacks a zip (or takes a
//! DLL) into a hidden folder there and reads it, so the player is shown what it is before
//! [`Staged::install`] moves it into place; [`remove`] deletes one. Monaka never downloads or
//! updates a pack by itself, and vetting a pack is the player's call: the window warns each time.

use monaka_core::manifest::Manifest;
use monaka_core::pack::{self, About, Contract};
use monaka_core::pe;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;

/// The most manifest text a pack may export (bytes).
const MAX_MANIFEST: u32 = 1 << 20;

/// Hidden folders in the games folder that [`stage`] unpacks into.
const STAGING: &str = ".adding-";

/// What a producer DLL declares about itself.
#[derive(Debug)]
pub struct Pack {
    pub dll: PathBuf,
    pub contract: Contract,
    pub about: About,
    pub manifests: Vec<Manifest>,
}

impl Pack {
    /// Whether this Monaka can run the pack, or what the player should do.
    pub fn runs(&self) -> Result<(), String> {
        pack::CONTRACT.runs(self.contract)
    }

    /// The folder it was read from (its licence files sit beside it).
    pub fn folder(&self) -> &Path {
        self.dll.parent().unwrap_or(Path::new("."))
    }

    /// The DLL's file name without `.dll`: what its games name as their producer.
    fn stem(&self) -> String {
        self.dll.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()
    }

    /// The games' names, for messages.
    pub fn names(&self) -> String {
        self.manifests.iter().map(|m| m.name).collect::<Vec<_>>().join(", ")
    }

    /// Whether the pack was made for a newer Monaka than this one.
    pub fn needs_newer_monaka(&self) -> bool {
        pack::CONTRACT.needs_newer_host(self.contract)
    }

    /// What the player knows the pack by: its games, or its DLL when its games cannot be read.
    pub fn label(&self) -> String {
        if self.manifests.is_empty() { self.dll.file_name().unwrap_or_default().to_string_lossy().into_owned() } else { self.names() }
    }

    /// Why the launcher cannot run it from its folder, if it cannot: another contract, or a game
    /// whose producer is another DLL.
    pub(crate) fn unusable(&self) -> Option<String> {
        if let Err(why) = self.runs() {
            return Some(why);
        }
        let stem = self.stem();
        let other = self.manifests.iter().find(|m| !m.producer.eq_ignore_ascii_case(&stem))?;
        Some(format!("{} names its producer {}, but the DLL is {stem}.dll", other.name, other.producer))
    }
}

/// Reads the pack a producer DLL file is, or why it is not one.
pub fn read(dll: &Path) -> Result<Pack, String> {
    probe(dll)?.ok_or_else(|| format!("{} is not a game pack (no MONAKA_CONTRACT export)", dll.display()))
}

/// The pack a DLL is, `None` for a DLL that is no pack (one a pack brings along), or why a pack
/// cannot be read.
fn probe(dll: &Path) -> Result<Option<Pack>, String> {
    let bytes = std::fs::read(dll).map_err(|e| format!("{}: {e}", dll.display()))?;
    from_bytes(dll, &bytes)
}

fn from_bytes(dll: &Path, bytes: &[u8]) -> Result<Option<Pack>, String> {
    let name = dll.display();
    let Some(packed) = pe::file_export_u32(bytes, "MONAKA_CONTRACT").map_err(|e| format!("{name}: {e}"))? else { return Ok(None) };
    let contract = Contract::from_packed(packed);
    // A pack for a newer Monaka is a pack this one cannot run, whatever its text holds (it may
    // write text this Monaka cannot read): the player is asked to update Monaka.
    if pack::CONTRACT.needs_newer_host(contract) {
        let text = pe::file_export_text_up_to(bytes, "MONAKA_MANIFEST", MAX_MANIFEST).ok().flatten();
        let parsed = text.and_then(|t| pack::parse(&t).ok());
        let (about, manifests) = parsed.map(|p| (p.about, p.manifests)).unwrap_or_default();
        return Ok(Some(Pack { dll: dll.to_path_buf(), contract, about, manifests }));
    }
    // A pack of another contract may write text this Monaka cannot read: say which update is
    // needed rather than what failed to parse.
    let wrong_contract = |e: String| match pack::CONTRACT.runs(contract) {
        Err(why) => format!("{name} cannot run here: {why}"),
        Ok(()) => e,
    };
    let text = pe::file_export_text_up_to(bytes, "MONAKA_MANIFEST", MAX_MANIFEST)
        .map_err(|e| format!("{name}: {e}"))?
        .ok_or_else(|| wrong_contract(format!("{name} is not a game pack (no MONAKA_MANIFEST export)")))?;
    let parsed = pack::parse(&text).map_err(|e| wrong_contract(format!("{name}: its manifest: {e}")))?;
    if parsed.contract != contract {
        return Err(format!("{name}: its manifest says contract {}, its export {contract}", parsed.contract));
    }
    for manifest in &parsed.manifests {
        manifest.check().map_err(|e| wrong_contract(format!("{name}: {e}")))?;
    }
    Ok(Some(Pack { dll: dll.to_path_buf(), contract, about: parsed.about, manifests: parsed.manifests }))
}

/// The packs in a games folder, and the folders that hold none the launcher can run.
#[derive(Debug, Default)]
pub struct Scan {
    pub packs: Vec<Pack>,
    /// One line per folder or DLL that is no usable pack, naming it and why.
    pub problems: Vec<String>,
    /// The packs among those made for a newer Monaka, by what the player knows them as.
    pub too_new: Vec<String>,
}

/// Every pack in `games` (one folder each; hidden and staging folders skipped), by folder name.
pub fn scan(games: &Path) -> Scan {
    scan_with(games, probe)
}

fn scan_with(games: &Path, probe: impl Fn(&Path) -> Result<Option<Pack>, String>) -> Scan {
    let mut scan = Scan::default();
    let Ok(entries) = std::fs::read_dir(games) else { return scan };
    let mut folders: Vec<PathBuf> = entries.filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p.is_dir()).collect();
    folders.sort();
    for folder in folders {
        let name = folder.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        if name.starts_with('.') {
            continue;
        }
        let mut found = false;
        for dll in dlls(&folder) {
            match probe(&dll) {
                Ok(None) => {}
                Ok(Some(pack)) => {
                    found = true;
                    match pack.unusable() {
                        Some(why) => {
                            if pack.needs_newer_monaka() {
                                scan.too_new.push(format!("{} (game pack {name})", pack.label()));
                            }
                            scan.problems.push(format!("Game pack {name} ({}): {why}", pack.label()));
                        }
                        None => scan.packs.push(pack),
                    }
                }
                Err(why) => {
                    found = true;
                    scan.problems.push(format!("Game pack {name}: {why}"));
                }
            }
        }
        if !found {
            scan.problems.push(format!("Game pack {name}: no game pack DLL in {}", folder.display()));
        }
    }
    scan
}

/// The `.dll` files directly in `folder`, by name.
fn dlls(folder: &Path) -> Vec<PathBuf> {
    let mut found: Vec<PathBuf> = std::fs::read_dir(folder)
        .into_iter()
        .flatten()
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.is_file() && p.extension().is_some_and(|x| x.eq_ignore_ascii_case("dll")))
        .collect();
    found.sort();
    found
}

/// A pack unpacked into the games folder's staging area and read, not yet in place. Dropping it
/// discards it.
#[derive(Debug)]
pub struct Staged {
    games: PathBuf,
    staging: PathBuf,
    /// The folder inside `staging` holding the pack's DLL (the zip's top folder, or `staging`).
    root: PathBuf,
    pub packs: Vec<Pack>,
}

impl Staged {
    /// The folder name it goes in: the zip's own folder (`dying-light-games-vr`), else its first
    /// DLL's name.
    pub fn name(&self) -> String {
        let folder = self.root.file_name().map(|n| n.to_string_lossy().into_owned()).filter(|n| {
            self.root != self.staging
                && !n.starts_with('.')
                && n.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | ' '))
        });
        folder.unwrap_or_else(|| self.packs[0].stem())
    }

    /// The installed folder it would replace (an update of the same pack).
    pub fn replaces(&self) -> Option<PathBuf> {
        let target = self.games.join(self.name());
        target.is_dir().then_some(target)
    }

    /// Every game it offers.
    pub fn manifests(&self) -> impl Iterator<Item = &Manifest> {
        self.packs.iter().flat_map(|p| p.manifests.iter())
    }

    /// Its games' names, for messages.
    pub fn names(&self) -> String {
        self.packs.iter().map(Pack::names).collect::<Vec<_>>().join(", ")
    }

    /// Who made it and where from, as it says: "by X, from Y", or that it names neither.
    pub fn credit(&self) -> String {
        credit(&self.packs[0].about)
    }

    /// What of it was made for a newer Monaka (the player is asked to update instead of adding it).
    pub fn too_new(&self) -> Option<String> {
        let labels: Vec<String> = self.packs.iter().filter(|p| p.needs_newer_monaka()).map(Pack::label).collect();
        (!labels.is_empty()).then(|| labels.join(", "))
    }

    /// Moves it into the games folder, replacing an installed copy of the same pack; returns its
    /// folder. A pack made for a newer Monaka is not added.
    pub fn install(self) -> Result<PathBuf, String> {
        if let Some(what) = self.too_new() {
            return Err(format!("{what} was made for a newer Monaka: update Monaka"));
        }
        let target = self.games.join(self.name());
        if target.exists() {
            std::fs::remove_dir_all(&target).map_err(|e| format!("cannot remove the installed {}: {e}", target.display()))?;
        }
        std::fs::rename(&self.root, &target).map_err(|e| format!("cannot move the pack into {}: {e}", target.display()))?;
        Ok(target)
        // Drop removes what is left of the staging folder.
    }
}

impl Drop for Staged {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.staging);
    }
}

/// "by X, from Y" for a pack's [`About`], or that it names nobody.
pub fn credit(about: &About) -> String {
    let author = about.author.replace(':', ", ");
    match (author.is_empty(), about.source.is_empty()) {
        (false, false) => format!("by {author}, from {}", about.source),
        (false, true) => format!("by {author}"),
        (true, false) => format!("from {}", about.source),
        (true, true) => "naming no author or source".to_owned(),
    }
}

/// Unpacks a pack (a `.zip`, or a lone producer `.dll`) into a staging folder of `games` and reads
/// it. The zip may hold the pack's files at its top or in one folder; its other files (licences)
/// come along.
pub fn stage(file: &Path, games: &Path) -> Result<Staged, String> {
    stage_with(file, games, probe)
}

fn stage_with(file: &Path, games: &Path, probe: impl Fn(&Path) -> Result<Option<Pack>, String>) -> Result<Staged, String> {
    if !file.is_file() {
        return Err(format!("{} is not a file", file.display()));
    }
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or_default();
    let staging = games.join(format!("{STAGING}{}-{stamp}", std::process::id()));
    std::fs::create_dir_all(&staging).map_err(|e| format!("cannot make {}: {e}", staging.display()))?;
    // From here on the staging folder goes with the value, or is removed on failure.
    let mut staged = Staged { games: games.to_owned(), staging: staging.clone(), root: staging.clone(), packs: Vec::new() };
    let extension = file.extension().map(|x| x.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    match extension.as_str() {
        "zip" => unzip(file, &staging)?,
        "dll" => {
            let name = file.file_name().unwrap_or_default();
            std::fs::copy(file, staging.join(name)).map_err(|e| format!("cannot copy {}: {e}", file.display()))?;
        }
        _ => return Err(format!("{} is not a game pack: expected a .zip or a .dll", file.display())),
    }
    // The folders (the staging folder and two levels under it) holding pack DLLs.
    let mut holders: Vec<(PathBuf, Vec<Pack>)> = Vec::new();
    let mut folders = vec![(staging.clone(), 0)];
    while let Some((folder, depth)) = folders.pop() {
        let mut packs = Vec::new();
        for dll in dlls(&folder) {
            if let Some(pack) = probe(&dll)? {
                packs.push(pack);
            }
        }
        if !packs.is_empty() {
            holders.push((folder.clone(), packs));
        }
        if depth < 2 {
            let mut inner: Vec<PathBuf> = std::fs::read_dir(&folder).into_iter().flatten().filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p.is_dir()).collect();
            inner.sort();
            folders.extend(inner.into_iter().map(|p| (p, depth + 1)));
        }
    }
    let (root, packs) = match holders.len() {
        0 => return Err(format!("{} holds no game pack (no DLL with a MONAKA_CONTRACT export)", file.display())),
        1 => holders.pop().expect("one"),
        n => return Err(format!("{} holds {n} game packs in different folders; add them one at a time", file.display())),
    };
    for pack in &packs {
        // One for a newer Monaka is staged: the player is asked to update ([`Staged::too_new`]).
        if let Some(why) = pack.unusable().filter(|_| !pack.needs_newer_monaka()) {
            return Err(format!("{} ({}) cannot be used: {why}", pack.dll.file_name().unwrap_or_default().to_string_lossy(), pack.names()));
        }
    }
    staged.root = root;
    staged.packs = packs;
    Ok(staged)
}

/// Unpacks `zip` into `into` with Windows' own `tar.exe` (bsdtar), which refuses entries that
/// would land outside it.
fn unzip(zip: &Path, into: &Path) -> Result<(), String> {
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let system = std::env::var_os("SystemRoot").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(r"C:\Windows"));
    let tar = system.join(r"System32\tar.exe");
    let output = Command::new(&tar)
        .arg("-xf")
        .arg(zip)
        .arg("-C")
        .arg(into)
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .map_err(|e| format!("cannot run {}: {e}", tar.display()))?;
    if !output.status.success() {
        let why = String::from_utf8_lossy(&output.stderr);
        return Err(format!("cannot unpack {}: {}", zip.display(), why.trim()));
    }
    Ok(())
}

/// Deletes an installed pack's folder (one directly inside `games`).
pub fn remove(folder: &Path, games: &Path) -> Result<(), String> {
    if folder.parent() != Some(games) || !folder.is_dir() {
        return Err(format!("{} is not a game pack folder in {}", folder.display(), games.display()));
    }
    std::fs::remove_dir_all(folder).map_err(|e| format!("cannot remove {}: {e}", folder.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use monaka_core::manifest::{Launch, Status};

    #[test]
    fn a_dll_without_the_exports_is_not_a_pack() {
        let kernel = std::fs::read(r"C:\Windows\System32\kernel32.dll").unwrap();
        assert!(from_bytes(Path::new("kernel32.dll"), &kernel).unwrap().is_none());
        assert!(read(Path::new(r"C:\Windows\System32\kernel32.dll")).unwrap_err().contains("not a game pack"));
        assert!(from_bytes(Path::new("x.dll"), b"not a PE file").is_err());
    }

    const GAME: Manifest = Manifest {
        id: "t",
        name: "Test",
        status: Status::Experimental,
        executables: &["t.exe"],
        steam_app: 1,
        producer: "monaka_t",
        log: "t.log",
        options_file: "t-options.txt",
        run_prefix: "t",
        launch: Launch::Manual,
        refused_arguments: &[],
        before: "",
        options: &[],
        licences: &[],
        left_hand_aims: false,
        weapon_ring: false,
    };

    /// Stands in for reading a DLL: a file whose text is `pack <contract>` is a pack of [`GAME`]
    /// with that contract; anything else is no pack.
    fn fake(dll: &Path) -> Result<Option<Pack>, String> {
        let text = std::fs::read_to_string(dll).map_err(|e| e.to_string())?;
        let Some(version) = text.strip_prefix("pack ") else { return Ok(None) };
        let (major, minor) = version.trim().split_once('.').unwrap();
        let contract = Contract { major: major.parse().unwrap(), minor: minor.parse().unwrap() };
        Ok(Some(Pack { dll: dll.to_owned(), contract, about: About { author: "A:B".into(), source: "https://x".into() }, manifests: vec![GAME] }))
    }

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("monaka-packs-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write(path: &Path, text: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    #[test]
    fn scanning_lists_packs_and_says_what_is_wrong_with_the_rest() {
        let games = temp("scan");
        let current = format!("pack {}", pack::CONTRACT);
        write(&games.join(r"monaka_t\monaka_t.dll"), &current);
        write(&games.join(r"monaka_t\helper.dll"), "not a pack");
        write(&games.join(r"monaka_t\T-LICENSE.txt"), "licence");
        write(&games.join(r"old\monaka_t.dll"), "pack 0.9");
        write(&games.join(r"renamed\monaka_x.dll"), &current);
        write(&games.join(r"empty\readme.txt"), "");
        write(&games.join(r".adding-1-2\monaka_t.dll"), &current);
        let scan = scan_with(&games, fake);
        assert_eq!(scan.packs.len(), 1);
        assert!(scan.packs[0].dll.ends_with(r"monaka_t\monaka_t.dll"));
        assert_eq!(scan.packs[0].folder(), games.join("monaka_t"));
        assert_eq!(scan.problems.len(), 3, "{:?}", scan.problems);
        assert!(scan.problems[0].starts_with("Game pack empty: no game pack DLL"), "{:?}", scan.problems);
        assert!(scan.problems[1].starts_with("Game pack old (Test)") && scan.problems[1].contains("older Monaka"), "{:?}", scan.problems);
        assert!(scan.problems[2].contains("names its producer monaka_t, but the DLL is monaka_x.dll"), "{:?}", scan.problems);
        let _ = std::fs::remove_dir_all(&games);
    }

    #[test]
    fn a_pack_for_a_newer_monaka_asks_for_an_update() {
        let games = temp("newer");
        let newer = format!("pack {}.{}", pack::CONTRACT.major, pack::CONTRACT.minor + 1);
        write(&games.join(r"newer\monaka_t.dll"), &newer);
        write(&games.join(r"old\monaka_t.dll"), "pack 0.9");
        let scan = scan_with(&games, fake);
        assert!(scan.packs.is_empty());
        assert_eq!(scan.too_new, ["Test (game pack newer)"], "only the newer one asks for an update");
        assert_eq!(scan.problems.len(), 2, "{:?}", scan.problems);
        // Offered for adding, it is staged so the player can be asked, and never installed.
        let source = temp("newer-source").join("monaka_t.dll");
        write(&source, &newer);
        let staged = stage_with(&source, &games, fake).unwrap();
        assert_eq!(staged.too_new().as_deref(), Some("Test"));
        assert!(staged.install().unwrap_err().contains("update Monaka"));
        assert!(!games.join("monaka_t").exists());
        let _ = std::fs::remove_dir_all(&games);
        let _ = std::fs::remove_dir_all(source.parent().unwrap());
    }

    #[test]
    fn a_dll_is_staged_shown_and_installed_or_discarded() {
        let games = temp("dll");
        let source = temp("dll-source").join("monaka_t.dll");
        write(&source, &format!("pack {}", pack::CONTRACT));
        let staged = stage_with(&source, &games, fake).unwrap();
        assert_eq!(staged.name(), "monaka_t");
        assert_eq!(staged.names(), "Test");
        assert_eq!(staged.credit(), "by A, B, from https://x");
        assert!(staged.replaces().is_none());
        // Discarded: nothing is left behind.
        drop(staged);
        assert_eq!(std::fs::read_dir(&games).unwrap().count(), 0);
        // Installed, then again as an update of itself.
        let folder = stage_with(&source, &games, fake).unwrap().install().unwrap();
        assert_eq!(folder, games.join("monaka_t"));
        assert!(folder.join("monaka_t.dll").is_file());
        let again = stage_with(&source, &games, fake).unwrap();
        assert_eq!(again.replaces(), Some(folder.clone()));
        again.install().unwrap();
        assert_eq!(std::fs::read_dir(&games).unwrap().count(), 1, "the staging folder is gone");
        assert_eq!(scan_with(&games, fake).packs.len(), 1);
        // Removed, only from inside the games folder.
        assert!(remove(&source, &games).is_err());
        remove(&folder, &games).unwrap();
        assert!(!folder.exists());
        let _ = std::fs::remove_dir_all(&games);
        let _ = std::fs::remove_dir_all(source.parent().unwrap());
    }

    #[test]
    fn what_cannot_be_used_is_refused_and_cleaned_up() {
        let games = temp("refuse");
        let source = temp("refuse-source");
        let old = source.join("monaka_t.dll");
        write(&old, "pack 0.1");
        assert!(stage_with(&old, &games, fake).unwrap_err().contains("older Monaka"));
        let none = source.join("x.dll");
        write(&none, "nothing");
        assert!(stage_with(&none, &games, fake).unwrap_err().contains("holds no game pack"));
        let text = source.join("notes.txt");
        write(&text, "pack 1.0");
        assert!(stage_with(&text, &games, fake).unwrap_err().contains("expected a .zip or a .dll"));
        assert_eq!(std::fs::read_dir(&games).unwrap().count(), 0, "no staging folder left behind");
        let _ = std::fs::remove_dir_all(&games);
        let _ = std::fs::remove_dir_all(&source);
    }

    #[test]
    fn a_zip_with_a_top_folder_brings_its_licences() {
        let games = temp("zip");
        let source = temp("zip-source");
        write(&source.join(r"Test VR\monaka_t.dll"), &format!("pack {}", pack::CONTRACT));
        write(&source.join(r"Test VR\T-LICENSE.txt"), "licence");
        let zip = source.join("test-vr.zip");
        // Windows' bsdtar writes a zip for a .zip name (`-a`).
        let made = Command::new(r"C:\Windows\System32\tar.exe").arg("-a").arg("-cf").arg(&zip).arg("-C").arg(&source).arg("Test VR").status().unwrap();
        assert!(made.success());
        let folder = stage_with(&zip, &games, fake).unwrap().install().unwrap();
        assert_eq!(folder, games.join("Test VR"), "named after the zip's own folder");
        assert!(folder.join("T-LICENSE.txt").is_file());
        assert_eq!(std::fs::read_dir(&games).unwrap().count(), 1);
        let _ = std::fs::remove_dir_all(&games);
        let _ = std::fs::remove_dir_all(&source);
    }
}
