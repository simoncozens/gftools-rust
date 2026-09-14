use babelfont::Font;
// use fontations::read::FontRef;
use std::sync::{Arc, Mutex, MutexGuard};
use tempfile::NamedTempFile;

use crate::error::ApplicationError;

/// An output from an operation
///
/// gftools-builder is essentially a build system like `make` or `ninja`. In these kinds of build systems,
/// the fundamental unit of operation is a process, and processes communicate their outputs via file names.
/// In some cases, the file names aren't important; they are just a way to pass data around between processes.
/// So you could imagine a hypothetical extension to Makefile which does something like:
///
/// ```makefile
/// <temporary file 1>: Foo.glyphs
///     fontc $< -o $@
/// Foo.ttf: <temporary file 1>
///     gftools fix $< -o $@
/// ```
///
/// We're saying "we don't care what the file is called; when you need a name for the purposes of the process,
/// come up with a temporary file name and use it". Now let's go even further. In gftools-builder, the fundamental
/// unit of operation is a Rust thread running a function. If we're just passing data from thread to thread, we don't
/// even need our data to hit the disk at all. We can just pass around `Vec<u8>`s in memory between our operations.
/// (The data is stored in the operations graph as edges between the operations.)
/// Some operations, however, do call external processes and need to write and read files, so we need to be able to
/// convert between `Vec<u8>`s and file names. So there are three kinds of thing that
/// we pass around:
///
/// 1. Named files: these are files with a known name on disk; usually the start and end points
///    of the build process.
/// 2. Temporary files: these are files which have a name on disk, but we don't care what the
///    name is; usually intermediate files in the build process passed to external processes.
/// 3. In-memory bytes: these are just `Vec<u8>`s stored in memory; usually intermediate files
///    in the build process passed to internal Rust functions.
#[derive(Debug)]
pub enum RawOperationOutput {
    NamedFile(String),
    TemporaryFile(Option<NamedTempFile>),
    InMemoryBytes(Vec<u8>),
    SourceFont(Box<babelfont::Font>),
}

impl PartialEq for RawOperationOutput {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (RawOperationOutput::NamedFile(a), RawOperationOutput::NamedFile(b)) => a == b,
            (RawOperationOutput::TemporaryFile(a), RawOperationOutput::TemporaryFile(b)) => {
                a.as_ref().map(|f| f.path()) == b.as_ref().map(|f| f.path())
            }
            (RawOperationOutput::InMemoryBytes(a), RawOperationOutput::InMemoryBytes(b)) => a == b,
            (RawOperationOutput::SourceFont(a), RawOperationOutput::SourceFont(b)) => {
                std::ptr::eq(a, b) // This is probably wrong and definitely evil.
            }
            _ => false,
        }
    }
}

impl RawOperationOutput {
    pub fn from_str(s: &str) -> Self {
        Self::NamedFile(s.to_string())
    }
}

impl From<&str> for RawOperationOutput {
    fn from(s: &str) -> Self {
        Self::from_str(s)
    }
}

impl From<Font> for RawOperationOutput {
    fn from(font: Font) -> Self {
        Self::SourceFont(Box::new(font))
    }
}

/// A thread-safe, reference-counted wrapper around [RawOperationOutput]
#[derive(Clone)]
pub struct OperationOutput(Arc<Mutex<RawOperationOutput>>);

impl From<RawOperationOutput> for OperationOutput {
    fn from(output: RawOperationOutput) -> Self {
        Self(Arc::new(Mutex::new(output)))
    }
}

impl PartialEq for OperationOutput {
    fn eq(&self, other: &Self) -> bool {
        // unwraps here are horrible but this is only used during graph creation
        let self_lock = self.lock().unwrap();
        let other_lock = other.lock().unwrap();
        *self_lock == *other_lock
    }
}
impl OperationOutput {
    pub fn lock(&self) -> Result<MutexGuard<'_, RawOperationOutput>, ApplicationError> {
        self.0.lock().map_err(|_| ApplicationError::MutexPoisoned)
    }
}

impl std::fmt::Display for OperationOutput {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let raw_output = self.lock().map_err(|_| std::fmt::Error)?;
        match &*raw_output {
            RawOperationOutput::NamedFile(name) => write!(f, "{name}"),
            RawOperationOutput::TemporaryFile(_) => write!(f, "<temporary file>"),
            RawOperationOutput::InMemoryBytes(_) => write!(f, "<in-memory bytes>"),
            RawOperationOutput::SourceFont(font) => write!(
                f,
                "<{} source>",
                font.names
                    .family_name
                    .get_default()
                    .unwrap_or(&"unknown".to_string())
            ),
        }
    }
}

impl std::fmt::Debug for OperationOutput {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let raw_output = self.lock().map_err(|_| std::fmt::Error)?;
        match &*raw_output {
            RawOperationOutput::NamedFile(name) => write!(f, "NamedFile({name})"),
            RawOperationOutput::TemporaryFile(None) => write!(f, "UnnamedTemporaryFile"),
            RawOperationOutput::TemporaryFile(Some(x)) => {
                write!(f, "NamedTemporaryFile({})", x.path().to_string_lossy())
            }
            RawOperationOutput::InMemoryBytes(_) => write!(f, "InMemoryBytes"),
            RawOperationOutput::SourceFont(font) => {
                write!(
                    f,
                    "FontSource({})",
                    font.names
                        .family_name
                        .get_default()
                        .unwrap_or(&"unknown".to_string())
                )
            }
        }
    }
}

impl OperationOutput {
    /// Convert the OperationOutput to a filename on disk.
    ///
    /// Use this when you are passing the output to an external process that needs a file name.
    ///
    /// If the output is already a named file, returns that name.
    /// If the output is a temporary file, returns the temporary file name, creating the temp file if necessary.
    /// If the output is in-memory bytes, writes the bytes to a temporary file and returns the temp file name.
    pub fn to_filename(&self, suffix: Option<&str>) -> Result<String, ApplicationError> {
        let mut f = self.lock().map_err(|_| ApplicationError::MutexPoisoned)?;
        match &mut *f {
            RawOperationOutput::NamedFile(name) => Ok(name.to_string()),
            RawOperationOutput::TemporaryFile(x) => {
                // if it's none, make one and set it to some
                if let Some(temp_file) = x {
                    Ok(temp_file.path().to_string_lossy().to_string())
                } else {
                    let temp_file = if let Some(suffix) = suffix {
                        NamedTempFile::with_suffix(suffix)
                    } else {
                        NamedTempFile::new()
                    }
                    .map_err(|e| ApplicationError::Other(e.to_string()))?;
                    *x = Some(temp_file);
                    Ok(x.as_ref().unwrap().path().to_string_lossy().to_string())
                }
            }
            RawOperationOutput::InMemoryBytes(bytes) => {
                // Convert in-memory bytes to a temp file by writing it
                let temp_file = if let Some(suffix) = suffix {
                    NamedTempFile::with_suffix(suffix)
                } else {
                    NamedTempFile::new()
                }
                .map_err(|e| ApplicationError::Other(e.to_string()))?;
                // write
                let temp_path = temp_file.path();
                let temp_path_string = temp_path.to_string_lossy().to_string();
                std::fs::write(temp_path, bytes)
                    .map_err(|e| ApplicationError::Other(e.to_string()))?;
                *f = RawOperationOutput::TemporaryFile(Some(temp_file));
                Ok(temp_path_string)
            }
            RawOperationOutput::SourceFont(font) => {
                // Convert in-memory bytes to a temp file by writing it in Glyphs format
                let temp_file = NamedTempFile::with_suffix(".glyphs")
                    .map_err(|e| ApplicationError::Other(e.to_string()))?;
                // write
                let temp_path = temp_file.path();
                let temp_path_string = temp_path.to_string_lossy().to_string();

                font.save(temp_path)
                    .map_err(|e| ApplicationError::Other(e.to_string()))?;
                *f = RawOperationOutput::TemporaryFile(Some(temp_file));
                Ok(temp_path_string)
            }
        }
    }

    /// Set the contents of the OperationOutput to the given bytes.
    ///
    /// Use this when you have completed an operation and want to store the output bytes.
    /// This differs from `set_contents` in that it always sets the output to in-memory bytes,
    /// whereas `set_contents` will write to a named file if the output is a named file.
    pub fn set_bytes(&self, bytes: Vec<u8>) -> Result<(), ApplicationError> {
        let mut f = self.lock().map_err(|_| ApplicationError::MutexPoisoned)?;
        match &mut *f {
            RawOperationOutput::NamedFile(name) => {
                std::fs::write(&*name, bytes)
                    .map_err(|e| ApplicationError::Other(e.to_string()))?;
            }
            RawOperationOutput::TemporaryFile(Some(temp_file)) => {
                std::fs::write(temp_file.path(), bytes)
                    .map_err(|e| ApplicationError::Other(e.to_string()))?;
            }
            _ => {
                *f = RawOperationOutput::InMemoryBytes(bytes);
            }
        }
        Ok(())
    }

    /// Returns true if the OperationOutput is a named file.
    pub fn is_named_file(&self) -> bool {
        let f = self.lock().unwrap();
        matches!(&*f, RawOperationOutput::NamedFile(_))
    }

    /// Gets the contents of the OperationOutput as bytes.
    ///
    /// Use this when you need to read the output of an operation as bytes.
    /// If the output is a named file or temporary file, reads the file contents.
    /// If the output is in-memory bytes, returns the bytes directly.
    pub fn to_bytes(&self) -> Result<Vec<u8>, ApplicationError> {
        let f = self.lock().map_err(|_| ApplicationError::MutexPoisoned)?;
        match &*f {
            RawOperationOutput::NamedFile(name) => {
                // Read the file contents
                let bytes =
                    std::fs::read(name).map_err(|e| ApplicationError::Other(e.to_string()))?;
                Ok(bytes)
            }
            RawOperationOutput::TemporaryFile(Some(temp_file)) => {
                // Read the temp file contents
                let bytes = std::fs::read(temp_file.path())
                    .map_err(|e| ApplicationError::Other(e.to_string()))?;
                Ok(bytes)
            }
            RawOperationOutput::TemporaryFile(None) => Err(ApplicationError::Other(
                "Temporary file is not set".to_string(),
            )),
            RawOperationOutput::InMemoryBytes(bytes) => Ok(bytes.clone()),
            RawOperationOutput::SourceFont(_font) => {
                // Convert in-memory bytes to a temp file by writing it in Glyphs format
                // Unfortunately this currently requires a temp file on disk
                let temp_file = self.to_filename(Some(".glyphs"))?;
                // Noe read the bytes back
                let buffer =
                    std::fs::read(temp_file).map_err(|e| ApplicationError::Other(e.to_string()))?;
                Ok(buffer)
            }
        }
    }

    /// Set the contents of the OperationOutput to the given bytes.
    ///
    /// If the output is a named file, writes the bytes to the file.
    pub fn set_contents(&self, bytes: Vec<u8>) -> Result<(), ApplicationError> {
        if self.is_named_file() {
            // OK, we write it
            let output_path = self.to_filename(None)?;
            Ok(std::fs::write(output_path, bytes)?)
        } else {
            self.set_bytes(bytes)
        }
    }

    /// Convert the OperationOutput to a babelfont Font object.
    ///
    /// Use this when you need to work with the font source data directly.
    /// If the output is already a SourceFont, returns a clone of the font.
    /// If the output is a named file or temporary file, loads the font from disk.
    /// If the output is in-memory bytes, writes to a temp file and loads from there.
    pub fn to_font_source(&self) -> Result<Box<babelfont::Font>, ApplicationError> {
        let f = self.lock().map_err(|_| ApplicationError::MutexPoisoned)?;
        match &*f {
            RawOperationOutput::SourceFont(font) => Ok(font.clone()),
            RawOperationOutput::NamedFile(name) => {
                let font = babelfont::load(name).map_err(|e| {
                    ApplicationError::Other(format!("Failed to load font '{}': {}", name, e))
                })?;
                Ok(Box::new(font))
            }
            RawOperationOutput::TemporaryFile(Some(temp_file)) => {
                let path = temp_file.path().to_string_lossy().to_string();
                let font = babelfont::load(&path).map_err(|e| {
                    ApplicationError::Other(format!("Failed to load font from temp file: {}", e))
                })?;
                Ok(Box::new(font))
            }
            RawOperationOutput::TemporaryFile(None) => Err(ApplicationError::Other(
                "Temporary file is not set".to_string(),
            )),
            RawOperationOutput::InMemoryBytes(_) => {
                // Need to write to temp file first, then load
                drop(f); // Release the lock before calling to_filename which needs it
                let temp_filename = self.to_filename(None)?;
                let font = babelfont::load(&temp_filename).map_err(|e| {
                    ApplicationError::Other(format!("Failed to load font from bytes: {}", e))
                })?;
                Ok(Box::new(font))
            }
        }
    }

    /// Set the OperationOutput to contain a SourceFont.
    pub fn set_font_source(&self, font: Box<babelfont::Font>) -> Result<(), ApplicationError> {
        let mut f = self.lock().map_err(|_| ApplicationError::MutexPoisoned)?;
        match &mut *f {
            RawOperationOutput::NamedFile(name) => {
                font.save(&*name)
                    .map_err(|e| ApplicationError::Other(e.to_string()))?;
            }
            RawOperationOutput::TemporaryFile(Some(temp_file)) => {
                font.save(temp_file.path())
                    .map_err(|e| ApplicationError::Other(e.to_string()))?;
            }
            _ => {
                *f = RawOperationOutput::SourceFont(font);
            }
        }
        Ok(())
    }
}
