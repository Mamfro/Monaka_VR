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
mod tests;
