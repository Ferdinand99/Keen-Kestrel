//! Debug files on disk: chunks, assembling a file from its chunks, unpacking
//! zip archives (bounded), and storing each object of a file with the
//! SymCache made from it. Everything here blocks; callers run it in
//! `spawn_blocking`.

use std::collections::HashMap;
use std::fs::File;
use std::io::{BufWriter, Read, Write};
use std::path::{Path, PathBuf};

use symbolic_common::{ByteView, DebugId};
use symbolic_debuginfo::{Archive, FileFormat, Object};
use symbolic_symcache::{SymCache, SymCacheConverter};

/// sentry-cli's chunk size (and the largest chunk accepted).
pub const CHUNK_SIZE: usize = 8 * 1024 * 1024;
/// Chunks per chunk-upload request.
pub const CHUNKS_PER_REQUEST: usize = 64;
/// Largest chunk-upload request body.
pub const MAX_REQUEST: usize = 32 * 1024 * 1024;
/// Largest debug file (assembled, or one file in a zip).
pub const MAX_FILE: u64 = 1024 * 1024 * 1024;
/// Largest zip upload (as sent).
pub const MAX_ZIP: u64 = 1024 * 1024 * 1024;
/// Most bytes unpacked from one zip.
pub const MAX_UNPACKED: u64 = 2 * 1024 * 1024 * 1024;
/// Most entries a zip may have.
pub const MAX_ZIP_ENTRIES: usize = 10_000;
/// Largest ratio of unpacked to packed size for an entry over 16 MB: DWARF
/// packs four- to eightfold, a zip bomb by thousands.
pub const MAX_RATIO: u64 = 100;
/// Most files in one assemble request.
pub const MAX_ASSEMBLE_FILES: usize = 1000;

/// Why a debug file was refused.
#[derive(Debug)]
pub enum DifError {
    /// Not an object file symbolic can read.
    NotAnObject,
    /// An object file without debug information or a symbol table.
    NoDebugInfo,
    TooLarge(&'static str),
    Invalid(String),
    Io(std::io::Error),
}

impl std::fmt::Display for DifError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DifError::NotAnObject => f.write_str("not a debug information file"),
            DifError::NoDebugInfo => {
                f.write_str("the file has neither debug information nor symbols")
            }
            DifError::TooLarge(what) => write!(f, "{what} is too large"),
            DifError::Invalid(why) => f.write_str(why),
            DifError::Io(e) => write!(f, "can't store the file: {e}"),
        }
    }
}

impl std::error::Error for DifError {}

impl From<std::io::Error> for DifError {
    fn from(e: std::io::Error) -> Self {
        DifError::Io(e)
    }
}

/// A 40-character hex SHA-1.
pub fn is_sha1(s: &str) -> bool {
    s.len() == 40 && s.bytes().all(|b| b.is_ascii_hexdigit())
}

pub fn sha1_hex(data: &[u8]) -> String {
    sha1_smol::Sha1::from(data).digest().to_string()
}

/// Stores one uploaded chunk after checking its SHA-1.
pub fn store_chunk(chunk_dir: &Path, checksum: &str, data: &[u8]) -> Result<(), DifError> {
    if !is_sha1(checksum) {
        return Err(DifError::Invalid(format!(
            "the chunk name {checksum:?} is not a SHA-1"
        )));
    }
    if data.len() > CHUNK_SIZE {
        return Err(DifError::TooLarge("the chunk"));
    }
    let checksum = checksum.to_ascii_lowercase();
    let actual = sha1_hex(data);
    if actual != checksum {
        return Err(DifError::Invalid(format!(
            "the chunk's SHA-1 is {actual}, not {checksum}"
        )));
    }
    let path = chunk_dir.join(&checksum);
    if path.is_file() {
        return Ok(());
    }
    write_atomic(&path, |w| w.write_all(data))?;
    Ok(())
}

pub fn chunk_path(chunk_dir: &Path, checksum: &str) -> PathBuf {
    chunk_dir.join(checksum.to_ascii_lowercase())
}

/// Joins `chunks` into a file in `tmp_dir`, checking the file's SHA-1.
/// Returns the file's path; the caller removes it.
pub fn assemble(
    chunk_dir: &Path,
    tmp_dir: &Path,
    checksum: &str,
    chunks: &[String],
) -> Result<PathBuf, DifError> {
    let mut total = 0u64;
    for c in chunks {
        total += std::fs::metadata(chunk_path(chunk_dir, c))?.len();
    }
    if total > MAX_FILE {
        return Err(DifError::TooLarge("the file"));
    }
    let path = tmp_dir.join(format!("assemble-{}", checksum.to_ascii_lowercase()));
    let mut out = BufWriter::new(File::create(&path)?);
    let mut sha = sha1_smol::Sha1::new();
    for c in chunks {
        let data = std::fs::read(chunk_path(chunk_dir, c))?;
        sha.update(&data);
        out.write_all(&data)?;
    }
    out.flush()?;
    drop(out);
    let actual = sha.digest().to_string();
    if !actual.eq_ignore_ascii_case(checksum) {
        let _ = std::fs::remove_file(&path);
        return Err(DifError::Invalid(format!(
            "the assembled file's SHA-1 is {actual}, not {checksum}"
        )));
    }
    Ok(path)
}

/// What became of one object of an uploaded file.
#[derive(Debug, Clone)]
pub struct Stored {
    pub debug_id: DebugId,
    pub code_id: Option<String>,
    pub arch: String,
    pub kind: String,
    pub features: Vec<String>,
    pub size: u64,
    pub symcache_size: u64,
    /// Kept the existing object instead (it has debug information, this one
    /// only symbols).
    pub kept_existing: bool,
}

fn features(o: &Object<'_>) -> Vec<String> {
    let mut f = Vec::new();
    if o.has_debug_info() {
        f.push("debug".to_string());
    }
    if o.has_symbols() {
        f.push("symtab".to_string());
    }
    if o.has_unwind_info() {
        f.push("unwind".to_string());
    }
    if o.has_sources() {
        f.push("sources".to_string());
    }
    f
}

/// Writes `path` through a temporary file next to it, then renames it.
fn write_atomic(
    path: &Path,
    write: impl FnOnce(&mut BufWriter<File>) -> std::io::Result<()>,
) -> std::io::Result<()> {
    let tmp = path.with_extension("part");
    {
        let mut w = BufWriter::new(File::create(&tmp)?);
        write(&mut w)?;
        w.flush()?;
    }
    std::fs::rename(&tmp, path)
}

/// Converts one object to a SymCache in memory.
fn convert(o: &Object<'_>) -> Result<Vec<u8>, DifError> {
    let mut conv = SymCacheConverter::new();
    conv.process_object(o)
        .map_err(|e| DifError::Invalid(format!("can't read the debug information: {e}")))?;
    let mut bytes = Vec::new();
    conv.serialize(&mut std::io::Cursor::new(&mut bytes))?;
    Ok(bytes)
}

/// Stores every object in the file at `path` that has debug information or
/// symbols (a fat Mach-O has one per architecture): the object and its
/// SymCache under `project_dir/<debug_id>/`. An object that only has
/// symbols never replaces one with debug information (`existing` says which
/// debug ids have that).
pub fn store_objects(
    path: &Path,
    project_dir: &Path,
    existing: &HashMap<DebugId, bool>,
) -> Result<Vec<Stored>, DifError> {
    let view = ByteView::open(path)?;
    if Archive::peek(&view) == FileFormat::Unknown {
        return Err(DifError::NotAnObject);
    }
    let archive = Archive::parse(&view).map_err(|_| DifError::NotAnObject)?;
    let mut out = Vec::new();
    for object in archive.objects() {
        let Ok(object) = object else {
            continue;
        };
        if !object.has_debug_info() && !object.has_symbols() {
            continue;
        }
        let debug_id = object.debug_id();
        if debug_id.is_nil() {
            continue;
        }
        let features = features(&object);
        let base = Stored {
            debug_id,
            code_id: object.code_id().map(|c| c.to_string()),
            arch: object.arch().name().to_string(),
            kind: object.file_format().name().to_string(),
            size: object.data().len() as u64,
            symcache_size: 0,
            features,
            kept_existing: false,
        };
        if !object.has_debug_info() && existing.get(&debug_id) == Some(&true) {
            out.push(Stored {
                kept_existing: true,
                ..base
            });
            continue;
        }
        let symcache = convert(&object)?;
        let dir = project_dir.join(debug_id.to_string());
        std::fs::create_dir_all(&dir)?;
        write_atomic(&dir.join("object"), |w| w.write_all(object.data()))?;
        write_atomic(&dir.join("symcache"), |w| w.write_all(&symcache))?;
        out.push(Stored {
            symcache_size: symcache.len() as u64,
            ..base
        });
    }
    if out.is_empty() {
        return Err(DifError::NoDebugInfo);
    }
    Ok(out)
}

/// Opens a stored SymCache, checking that symbolic reads it.
pub fn open_symcache(dir: &Path) -> Result<ByteView<'static>, DifError> {
    let view = ByteView::open(dir.join("symcache"))?;
    SymCache::parse(&view).map_err(|e| DifError::Invalid(format!("unreadable SymCache: {e}")))?;
    Ok(view)
}

/// Makes the SymCache of `debug_id` again from its stored object.
pub fn rebuild_symcache(dir: &Path, debug_id: &DebugId) -> Result<ByteView<'static>, DifError> {
    let view = ByteView::open(dir.join("object"))?;
    let archive = Archive::parse(&view).map_err(|_| DifError::NotAnObject)?;
    let object = archive
        .objects()
        .flatten()
        .find(|o| o.debug_id() == *debug_id)
        .ok_or(DifError::NotAnObject)?;
    let symcache = convert(&object)?;
    write_atomic(&dir.join("symcache"), |w| w.write_all(&symcache))?;
    open_symcache(dir)
}

/// One object file found in a zip, unpacked into `tmp`.
#[derive(Debug)]
pub struct Unpacked {
    /// The entry's path in the zip.
    pub name: String,
    pub path: PathBuf,
}

/// Bytes read to tell whether an entry is an object file.
const PEEK: usize = 4096;

/// Unpacks the object files of the zip at `zip` into `tmp` (a directory the
/// caller removes), within the limits: entries, bytes per file and in all,
/// and the packing ratio. Other files (Info.plist, Swift interfaces, …) are
/// skipped and listed.
pub fn unpack_zip(zip: &Path, tmp: &Path) -> Result<(Vec<Unpacked>, Vec<String>), DifError> {
    let file = File::open(zip)?;
    let mut archive = zip::ZipArchive::new(std::io::BufReader::new(file))
        .map_err(|e| DifError::Invalid(format!("not a zip archive: {e}")))?;
    if archive.len() > MAX_ZIP_ENTRIES {
        return Err(DifError::TooLarge("the number of files in the zip"));
    }
    std::fs::create_dir_all(tmp)?;
    let mut out = Vec::new();
    let mut skipped = Vec::new();
    let mut total = 0u64;
    for i in 0..archive.len() {
        let mut entry = archive
            .by_index(i)
            .map_err(|e| DifError::Invalid(format!("a broken zip entry: {e}")))?;
        if !entry.is_file() {
            continue;
        }
        let name = entry
            .name()
            .map_err(|e| DifError::Invalid(format!("a broken zip entry name: {e}")))?
            .to_string();
        let base = name.rsplit('/').next().unwrap_or(&name);
        if name.starts_with("__MACOSX/") || base.starts_with("._") {
            continue;
        }
        if entry.size() > MAX_FILE {
            return Err(DifError::TooLarge("a file in the zip"));
        }
        let packed = entry.compressed_size();
        let mut head = Vec::with_capacity(PEEK);
        (&mut entry)
            .take(PEEK as u64)
            .read_to_end(&mut head)
            .map_err(|e| DifError::Invalid(format!("a broken zip entry: {e}")))?;
        if Archive::peek(&head) == FileFormat::Unknown {
            skipped.push(name);
            continue;
        }
        let path = tmp.join(format!("entry-{i}"));
        let mut w = BufWriter::new(File::create(&path)?);
        w.write_all(&head)?;
        let mut written = head.len() as u64;
        let mut buf = vec![0u8; 256 * 1024];
        loop {
            let n = entry
                .read(&mut buf)
                .map_err(|e| DifError::Invalid(format!("a broken zip entry: {e}")))?;
            if n == 0 {
                break;
            }
            written += n as u64;
            if written > MAX_FILE {
                return Err(DifError::TooLarge("a file in the zip"));
            }
            if total + written > MAX_UNPACKED {
                return Err(DifError::TooLarge("the unpacked zip"));
            }
            if written > 16 * 1024 * 1024 && written > packed.saturating_mul(MAX_RATIO) {
                return Err(DifError::Invalid(
                    "a file in the zip unpacks to far more than it packs (zip bomb?)".into(),
                ));
            }
            w.write_all(&buf[..n])?;
        }
        w.flush()?;
        total += written;
        out.push(Unpacked { name, path });
    }
    Ok((out, skipped))
}

/// The object's name from a path: `App.dSYM/Contents/Resources/DWARF/App`
/// becomes `App`.
pub fn object_name(path: &str) -> String {
    let name = path
        .trim_end_matches(['/', '\\'])
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(path)
        .trim();
    let name: String = name.chars().filter(|c| !c.is_control()).take(200).collect();
    if name.is_empty() {
        "unnamed".to_string()
    } else {
        name
    }
}

/// Removes chunks older than `max_age` (left by uploads that never
/// assembled). Returns how many.
pub fn prune_chunks(chunk_dir: &Path, max_age: std::time::Duration) -> usize {
    let Ok(entries) = std::fs::read_dir(chunk_dir) else {
        return 0;
    };
    let mut n = 0;
    for e in entries.flatten() {
        let old = e
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.elapsed().ok())
            .is_some_and(|age| age > max_age);
        if old && std::fs::remove_file(e.path()).is_ok() {
            n += 1;
        }
    }
    n
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("kk-difs-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn chunks_are_checked_and_assembled() {
        let d = dir("chunks");
        let (a, b) = (b"first chunk ".to_vec(), b"second chunk".to_vec());
        let whole = [a.clone(), b.clone()].concat();
        let (ca, cb, cw) = (sha1_hex(&a), sha1_hex(&b), sha1_hex(&whole));
        assert!(store_chunk(&d, &ca, &b).is_err(), "wrong checksum");
        assert!(store_chunk(&d, "../../etc/passwd", &a).is_err());
        store_chunk(&d, &ca.to_uppercase(), &a).unwrap();
        store_chunk(&d, &cb, &b).unwrap();
        let path = assemble(&d, &d, &cw, &[ca.clone(), cb.clone()]).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), whole);
        let bad = assemble(&d, &d, &sha1_hex(b"other"), &[ca, cb]).unwrap_err();
        assert!(bad.to_string().contains("SHA-1"), "{bad}");
        // Not an object file.
        let err = store_objects(&path, &d.join("p"), &HashMap::new()).unwrap_err();
        assert!(matches!(err, DifError::NotAnObject), "{err}");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn object_names_come_from_paths() {
        assert_eq!(
            object_name("App.app.dSYM/Contents/Resources/DWARF/App"),
            "App"
        );
        assert_eq!(object_name("C:\\x\\Lib.dll"), "Lib.dll");
        assert_eq!(object_name(""), "unnamed");
    }
}
