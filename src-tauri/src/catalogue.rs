//! Only main/*.iwd is indexed. Loose assets, aliases and FastFiles are outside v1.
use crate::sound::{self, Sound, MAX_SOUND_BYTES};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, VecDeque},
    fs::{self, File},
    io::{BufReader, Read, Write},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

const INDEX_LIMIT: u64 = 24 * 1024 * 1024;
const CACHE_VERSION: &str = "sounds-iwd-1";

pub fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Source {
    pub archive: String,
    pub member: usize,
    pub path: String,
    pub bytes: u64,
    pub crc: u32,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Entry {
    pub id: String,
    pub name: String,
    pub folder: String,
    pub sources: Vec<Source>,
}

#[derive(Clone, Default, Serialize, Deserialize)]
pub struct Index {
    pub entries: Vec<Entry>,
    pub archives: Vec<String>,
    pub warnings: Vec<String>,
    pub members: usize,
    pub cached: bool,
}

#[derive(Serialize)]
pub struct Progress {
    pub archive: String,
    pub completed: usize,
    pub total: usize,
    pub sounds: usize,
}

#[derive(Serialize)]
pub struct CacheInfo {
    pub path: String,
    pub bytes: u64,
}

pub struct Catalogue {
    pub root: PathBuf,
    pub cache: PathBuf,
    pub index: Index,
    locations: BTreeMap<String, usize>,
    open_archives: VecDeque<OpenArchive>,
}

struct OpenArchive {
    name: String,
    length: u64,
    modified: SystemTime,
    reader: zip::ZipArchive<BufReader<File>>,
}

fn archive_names(root: &Path) -> Result<Vec<String>, String> {
    let mut names = Vec::new();
    for item in
        fs::read_dir(root.join("main")).map_err(|error| format!("Cannot read main: {error}"))?
    {
        let item = item.map_err(|error| error.to_string())?;
        if item
            .file_type()
            .map_err(|error| error.to_string())?
            .is_file()
            && item
                .path()
                .extension()
                .is_some_and(|value| value.eq_ignore_ascii_case("iwd"))
        {
            names.push(item.file_name().to_string_lossy().to_string());
        }
    }
    names.sort();
    if names.is_empty() {
        return Err("No IWD archives found in the game's main folder.".into());
    }
    Ok(names)
}

fn valid_archive(name: &str) -> bool {
    !name.is_empty() && !name.contains(['/', '\\', ':']) && name.to_lowercase().ends_with(".iwd")
}

fn sound_name(path: &str) -> Option<String> {
    let name = path.replace('\\', "/");
    let lower = name.to_lowercase();
    if !lower.starts_with("sound/")
        || !lower.ends_with(".wav")
        || name.split('/').any(|part| part == ".." || part.is_empty())
    {
        return None;
    }
    Some(name[6..].into())
}

impl Catalogue {
    pub fn new(root: PathBuf, cache: PathBuf) -> Self {
        Self {
            root,
            cache,
            index: Index::default(),
            locations: BTreeMap::new(),
            open_archives: VecDeque::new(),
        }
    }

    fn install(&mut self, index: Index) -> Index {
        self.locations = index
            .entries
            .iter()
            .enumerate()
            .map(|(position, entry)| (entry.id.clone(), position))
            .collect();
        self.index = index;
        self.index.clone()
    }

    pub fn scan(
        &mut self,
        force: bool,
        mut progress: impl FnMut(Progress),
    ) -> Result<Index, String> {
        self.open_archives.clear();
        let names = archive_names(&self.root)?;
        let mut stamps = format!("{CACHE_VERSION}|{}", self.root.display());
        for name in &names {
            let meta = fs::metadata(self.root.join("main").join(name))
                .map_err(|error| error.to_string())?;
            let modified = meta
                .modified()
                .map_err(|error| error.to_string())?
                .duration_since(UNIX_EPOCH)
                .map_err(|error| error.to_string())?
                .as_nanos();
            stamps.push_str(&format!("|{name}:{}:{modified}", meta.len()));
        }
        let key = digest(stamps.as_bytes());
        let cache_path = self.cache.join("index.json");
        if !force {
            if let Ok(file) = File::open(&cache_path) {
                if let Ok((saved_key, mut index)) = serde_json::from_reader::<_, (String, Index)>(
                    BufReader::new(file.take(INDEX_LIMIT)),
                ) {
                    if saved_key == key
                        && index.archives == names
                        && index.entries.iter().all(|entry| {
                            !entry.sources.is_empty()
                                && entry.sources.iter().all(|source| {
                                    valid_archive(&source.archive)
                                        && names.contains(&source.archive)
                                        && sound_name(&source.path).is_some()
                                })
                        })
                    {
                        index.cached = true;
                        return Ok(self.install(index));
                    }
                }
            }
        }
        let mut grouped = BTreeMap::<String, Entry>::new();
        let mut index = Index {
            archives: names.clone(),
            ..Index::default()
        };
        for (position, name) in names.iter().enumerate() {
            progress(Progress {
                archive: name.clone(),
                completed: position,
                total: names.len(),
                sounds: index.members,
            });
            let result = (|| -> Result<(), String> {
                let file = File::open(self.root.join("main").join(name))
                    .map_err(|error| error.to_string())?;
                let mut archive = zip::ZipArchive::new(BufReader::new(file))
                    .map_err(|error| error.to_string())?;
                for member in 0..archive.len() {
                    let item = archive
                        .by_index(member)
                        .map_err(|error| error.to_string())?;
                    if item.is_dir() {
                        continue;
                    }
                    let Some(path) = sound_name(item.name()) else {
                        continue;
                    };
                    let lower = path.to_lowercase();
                    let source = Source {
                        archive: name.clone(),
                        member,
                        path: item.name().into(),
                        bytes: item.size(),
                        crc: item.crc32(),
                    };
                    let entry = grouped.entry(lower.clone()).or_insert_with(|| Entry {
                        id: digest(lower.as_bytes()),
                        folder: path
                            .rsplit_once('/')
                            .map_or("(root)", |(folder, _)| folder)
                            .into(),
                        name: path,
                        sources: Vec::new(),
                    });
                    entry.sources.push(source);
                    index.members += 1;
                }
                Ok(())
            })();
            if let Err(error) = result {
                index.warnings.push(format!("{name}: {error}"));
            }
        }
        progress(Progress {
            archive: String::new(),
            completed: names.len(),
            total: names.len(),
            sounds: index.members,
        });
        index.entries = grouped.into_values().collect();
        if index.warnings.is_empty() {
            // A failed or interrupted cache write must not publish half an index.
            let result = (|| -> Result<(), String> {
                fs::create_dir_all(&self.cache).map_err(|error| error.to_string())?;
                let mut file = tempfile::NamedTempFile::new_in(&self.cache)
                    .map_err(|error| error.to_string())?;
                serde_json::to_writer(&mut file, &(key, &index))
                    .map_err(|error| error.to_string())?;
                file.flush().map_err(|error| error.to_string())?;
                file.persist(&cache_path)
                    .map_err(|error| error.to_string())?;
                Ok(())
            })();
            if let Err(error) = result {
                index.warnings.push(format!(
                    "Catalogue loaded; cache could not be saved: {error}"
                ));
            }
        }
        Ok(self.install(index))
    }

    pub fn load(&mut self, id: &str, source_index: usize) -> Result<Sound, String> {
        let position = self
            .locations
            .get(id)
            .ok_or("Sound is no longer in the catalogue. Rescan and try again.")?;
        let source = self.index.entries[*position]
            .sources
            .get(source_index)
            .ok_or("Unknown sound source.")?
            .clone();
        if !valid_archive(&source.archive) || source.bytes > MAX_SOUND_BYTES {
            return Err(
                "The sound exceeds the 128 MiB preview limit or its archive name is invalid."
                    .into(),
            );
        }
        let path = self.root.join("main").join(&source.archive);
        let metadata = fs::metadata(&path).map_err(|error| error.to_string())?;
        let modified = metadata.modified().map_err(|error| error.to_string())?;
        let existing = self
            .open_archives
            .iter()
            .position(|archive| archive.name == source.archive);
        if let Some(position) = existing {
            let archive = self
                .open_archives
                .remove(position)
                .ok_or("Archive cache changed.")?;
            if archive.length == metadata.len() && archive.modified == modified {
                self.open_archives.push_back(archive);
            }
        }
        if self
            .open_archives
            .back()
            .is_none_or(|archive| archive.name != source.archive)
        {
            let file = File::open(&path).map_err(|error| error.to_string())?;
            let reader = zip::ZipArchive::new(BufReader::with_capacity(128 * 1024, file))
                .map_err(|error| error.to_string())?;
            self.open_archives.push_back(OpenArchive {
                name: source.archive.clone(),
                length: metadata.len(),
                modified,
                reader,
            });
        }
        // Keep only two directory readers, never a library of decoded clips.
        while self.open_archives.len() > 2 {
            self.open_archives.pop_front();
        }
        let archive = self
            .open_archives
            .back_mut()
            .ok_or("Cannot open the sound archive.")?;
        let mut member = archive
            .reader
            .by_index(source.member)
            .map_err(|error| error.to_string())?;
        if member.name() != source.path
            || member.size() != source.bytes
            || member.crc32() != source.crc
        {
            return Err("This archive changed. Click Rescan before playing the sound.".into());
        }
        let mut bytes = Vec::with_capacity(source.bytes as usize);
        member
            .by_ref()
            .take(MAX_SOUND_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|error| error.to_string())?;
        sound::parse(bytes)
    }

    pub fn cache_info(&self) -> CacheInfo {
        CacheInfo {
            path: self.cache.display().to_string(),
            bytes: fs::metadata(self.cache.join("index.json")).map_or(0, |value| value.len()),
        }
    }

    pub fn clear_cache(&self) -> Result<(), String> {
        let path = self.cache.join("index.json");
        match fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error.to_string()),
        }
    }
}
