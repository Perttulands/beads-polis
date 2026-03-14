//! JSONL event log — append-only, flock-protected, fsync-durable.

use crate::event::Event;
use fs2::FileExt;
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

pub struct EventLog {
    path: PathBuf,
    lock_path: PathBuf,
    watermark_path: PathBuf,
}

impl EventLog {
    /// Open (or create) the JSONL event log at the given path.
    pub fn open(path: &Path) -> io::Result<Self> {
        // Ensure parent directory exists
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        // Touch the file so it exists
        OpenOptions::new().create(true).append(true).open(path)?;

        let lock_path = path.with_extension("jsonl.lock");
        let watermark_path = path.with_file_name("index.watermark");

        Ok(Self {
            path: path.to_path_buf(),
            lock_path,
            watermark_path,
        })
    }

    /// Append an event: acquire flock, write JSON line, fsync, update watermark, release.
    pub fn append(&self, event: &Event) -> io::Result<()> {
        let lock_file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&self.lock_path)?;
        lock_file.lock_exclusive()?;

        let result = self.append_inner(event);

        lock_file.unlock()?;
        result
    }

    fn append_inner(&self, event: &Event) -> io::Result<()> {
        let mut file = OpenOptions::new().create(true).append(true).open(&self.path)?;
        let line = serde_json::to_string(event)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        writeln!(file, "{}", line)?;
        file.sync_all()?;

        // Update watermark
        let count = self.count_lines()?;
        self.write_watermark(count)?;

        Ok(())
    }

    /// Read all events, skipping bad lines in the middle and discarding a truncated last line.
    pub fn read_all(&self) -> io::Result<Vec<Event>> {
        let file = match File::open(&self.path) {
            Ok(f) => f,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(e),
        };
        let reader = BufReader::new(file);
        let lines: Vec<String> = reader.lines().collect::<io::Result<Vec<_>>>()?;
        let mut events = Vec::new();

        for (i, line) in lines.iter().enumerate() {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            match serde_json::from_str::<Event>(trimmed) {
                Ok(event) => events.push(event),
                Err(_) => {
                    let is_last = i == lines.len() - 1;
                    if is_last {
                        // Truncated last line — discard it (PRD Layer 1 resilience)
                        eprintln!("beads: discarding truncated last line in event log");
                    } else {
                        // Bad line in the middle — skip it, don't break
                        eprintln!("beads: skipping unparseable line {} in event log", i + 1);
                    }
                }
            }
        }
        Ok(events)
    }

    /// Count lines in the JSONL file.
    pub fn line_count(&self) -> io::Result<usize> {
        self.count_lines()
    }

    /// Read the stored watermark (last-processed line count).
    pub fn read_watermark(&self) -> io::Result<usize> {
        match fs::read_to_string(&self.watermark_path) {
            Ok(s) => s
                .trim()
                .parse::<usize>()
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e)),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(0),
            Err(e) => Err(e),
        }
    }

    /// Check if the index is stale (watermark < actual line count).
    pub fn is_stale(&self) -> io::Result<bool> {
        let watermark = self.read_watermark()?;
        let actual = self.count_lines()?;
        Ok(watermark < actual)
    }

    /// Path to the JSONL file.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Path to the lock file.
    pub fn lock_path(&self) -> &Path {
        &self.lock_path
    }

    /// Acquire the flock and return the lock file (caller must hold it).
    pub fn acquire_lock(&self) -> io::Result<File> {
        let lock_file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&self.lock_path)?;
        lock_file.lock_exclusive()?;
        Ok(lock_file)
    }

    fn count_lines(&self) -> io::Result<usize> {
        let file = match File::open(&self.path) {
            Ok(f) => f,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(0),
            Err(e) => return Err(e),
        };
        let reader = BufReader::new(file);
        let mut count = 0;
        for line in reader.lines() {
            let l = line?;
            if !l.trim().is_empty() {
                count += 1;
            }
        }
        Ok(count)
    }

    fn write_watermark(&self, count: usize) -> io::Result<()> {
        let mut f = File::create(&self.watermark_path)?;
        write!(f, "{}", count)?;
        f.sync_all()?;
        Ok(())
    }
}
