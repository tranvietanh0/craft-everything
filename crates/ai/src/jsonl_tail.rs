//! Reading a JSON-lines file as it grows: each read returns only the whole lines written since the
//! last one, keeping a line that is still being written for the next read. Claude Code's
//! transcripts are read this way, so a large one is read once and then only its new end.

use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

pub struct JsonlTail {
    path: PathBuf,
    offset: u64,
    partial: String,
}

impl JsonlTail {
    /// Read `path` from `offset` bytes in (0 for the whole file).
    pub fn new(path: PathBuf, offset: u64) -> Self {
        JsonlTail { path, offset, partial: String::new() }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The whole lines written since the last read. A file that isn't there yet has none; any other
    /// failure to read it is returned.
    pub fn read_lines(&mut self) -> std::io::Result<Vec<String>> {
        let mut f = match std::fs::File::open(&self.path) {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(e),
        };
        let mut bytes = Vec::new();
        f.seek(SeekFrom::Start(self.offset))?;
        f.read_to_end(&mut bytes)?;
        self.offset += bytes.len() as u64;
        self.partial.push_str(&String::from_utf8_lossy(&bytes));
        let mut lines = Vec::new();
        while let Some(end) = self.partial.find('\n') {
            lines.push(self.partial.drain(..=end).collect());
        }
        Ok(lines)
    }
}
