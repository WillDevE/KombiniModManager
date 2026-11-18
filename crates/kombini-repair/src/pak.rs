//! The small slice of the `valve_pak` API the ported repair code uses,
//! implemented on [`kombini_vpk`], plus the vpkmerge-core helpers `pack`,
//! `read_vpk_entry` and `embed_metadata` (as [`repack`]).

use anyhow::{bail, Context, Result};
use kombini_vpk::{write_vpk, EntrySource, VpkArchive};
use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use std::sync::Arc;

/// An opened VPK with its entry paths listed once.
pub struct Pak {
    archive: Arc<VpkArchive>,
    paths: Vec<String>,
    index: HashMap<String, usize>,
}

/// One entry, read on demand.
pub struct PakFile<'a> {
    pak: &'a Pak,
    index: usize,
}

pub fn open<P: AsRef<Path>>(path: P) -> Result<Pak> {
    let path = path.as_ref();
    let archive = VpkArchive::open(path).with_context(|| format!("opening {}", path.display()))?;
    let paths: Vec<String> = archive
        .file_paths()
        .into_iter()
        .map(str::to_owned)
        .collect();
    let mut index = HashMap::with_capacity(archive.entries().len());
    for (i, e) in archive.entries().iter().enumerate() {
        // Split files list several entries for one path; the first wins.
        index.entry(e.path.clone()).or_insert(i);
    }
    Ok(Pak {
        archive: Arc::new(archive),
        paths,
        index,
    })
}

impl Pak {
    pub fn file_paths(&self) -> std::slice::Iter<'_, String> {
        self.paths.iter()
    }

    pub fn get_file(&self, path: &str) -> Result<PakFile<'_>> {
        match self.index.get(path) {
            Some(&index) => Ok(PakFile { pak: self, index }),
            None => bail!("no entry {path:?} in {}", self.archive.path().display()),
        }
    }

    pub fn archive(&self) -> &Arc<VpkArchive> {
        &self.archive
    }
}

impl PakFile<'_> {
    pub fn read_all(&mut self) -> Result<Vec<u8>> {
        let entry = &self.pak.archive.entries()[self.index];
        Ok(self.pak.archive.read_file(&entry.path)?)
    }
}

/// Reads one entry from the VPK at `path`.
pub fn read_entry<P: AsRef<Path>>(path: P, entry: &str) -> Result<Vec<u8>> {
    open(path)?.get_file(entry)?.read_all()
}

/// Writes a new single-file VPK holding `files`.
pub fn pack<O: AsRef<Path>>(files: &[(&str, &[u8])], output: O) -> Result<()> {
    let entries = files
        .iter()
        .map(|(p, b)| ((*p).to_owned(), EntrySource::Bytes(b.to_vec())))
        .collect();
    write_vpk(output.as_ref(), entries, &|| false)?;
    Ok(())
}

/// Copies every entry of `input` into a new VPK at `output`, replacing (or
/// adding) the entries in `replace`. `output` must not be `input`.
pub fn repack<I: AsRef<Path>, O: AsRef<Path>>(
    input: I,
    replace: &[(&str, &[u8])],
    output: O,
) -> Result<()> {
    let (input, output) = (input.as_ref(), output.as_ref());
    if input == output {
        bail!("Repack output must differ from its input");
    }
    let pak = open(input)?;
    let replace: BTreeMap<&str, &[u8]> = replace.iter().copied().collect();
    let mut entries: Vec<(String, EntrySource)> = Vec::with_capacity(pak.paths.len());
    for path in &pak.paths {
        let source = match replace.get(path.as_str()) {
            Some(bytes) => EntrySource::Bytes(bytes.to_vec()),
            None => EntrySource::FromVpk {
                archive: pak.archive.clone(),
                path: path.clone(),
            },
        };
        entries.push((path.clone(), source));
    }
    for (path, bytes) in &replace {
        if !pak.index.contains_key(*path) {
            entries.push(((*path).to_owned(), EntrySource::Bytes(bytes.to_vec())));
        }
    }
    write_vpk(output, entries, &|| false)?;
    Ok(())
}
