use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use chrono::Utc;

/// Rotating JSONL file writer for trajectory collection.
///
/// Creates new files when the current file exceeds `max_file_bytes`.
/// File naming: `traj-YYYY-MM-DD-NNN.jsonl` where NNN is zero-padded sequence.
pub struct RotatingWriter {
    output_dir: PathBuf,
    max_file_bytes: u64,
    current_file: Option<File>,
    current_file_size: u64,
    current_date: String,
    current_seq: u32,
}

impl RotatingWriter {
    /// Create a new rotating writer. The output directory is created lazily on first write.
    pub fn new(output_dir: impl Into<PathBuf>, max_file_bytes: u64) -> Self {
        Self {
            output_dir: output_dir.into(),
            max_file_bytes,
            current_file: None,
            current_file_size: 0,
            current_date: String::new(),
            current_seq: 0,
        }
    }

    /// Write a single JSONL line (appends `\n` terminator).
    pub fn write_line(&mut self, line: &str) -> io::Result<()> {
        let today = Utc::now().format("%Y-%m-%d").to_string();
        let line_bytes = line.len() as u64 + 1; // +1 for newline

        // Rotate if: no file open, date changed, or size would exceed limit
        let needs_rotate = self.current_file.is_none()
            || self.current_date != today
            || (self.current_file_size + line_bytes > self.max_file_bytes
                && self.current_file_size > 0);

        if needs_rotate {
            self.rotate(&today)?;
        }

        let file = self
            .current_file
            .as_mut()
            .expect("rotate ensures file exists");
        file.write_all(line.as_bytes())?;
        file.write_all(b"\n")?;
        self.current_file_size += line_bytes;

        Ok(())
    }

    fn rotate(&mut self, today: &str) -> io::Result<()> {
        // Close current file
        self.current_file = None;

        // Reset sequence if date changed
        if self.current_date != today {
            self.current_date = today.to_string();
            self.current_seq = Self::scan_existing_seq(&self.output_dir, today);
        }

        self.current_seq += 1;
        self.current_file_size = 0;

        // Create output directory if missing
        fs::create_dir_all(&self.output_dir)?;

        let filename = format!("traj-{}-{:03}.jsonl", self.current_date, self.current_seq);
        let path = self.output_dir.join(filename);

        let file = open_with_permissions(&path)?;
        self.current_file = Some(file);

        Ok(())
    }

    /// Scan existing files to find the highest sequence number for the given date.
    fn scan_existing_seq(dir: &Path, date: &str) -> u32 {
        let prefix = format!("traj-{}-", date);
        let Ok(entries) = fs::read_dir(dir) else {
            return 0;
        };

        entries
            .filter_map(|e| e.ok())
            .filter_map(|e| {
                let name = e.file_name().to_string_lossy().into_owned();
                let rest = name.strip_prefix(&prefix)?;
                let seq_str = rest.strip_suffix(".jsonl")?;
                seq_str.parse::<u32>().ok()
            })
            .max()
            .unwrap_or(0)
    }
}

fn open_with_permissions(path: &Path) -> io::Result<File> {
    let file = OpenOptions::new().create(true).append(true).open(path)?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    }

    Ok(file)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn creates_output_directory() {
        let tmp = TempDir::new().unwrap();
        let out_dir = tmp.path().join("nested").join("trajectories");
        let mut writer = RotatingWriter::new(&out_dir, 1024 * 1024);
        writer.write_line(r#"{"id":"test"}"#).unwrap();
        assert!(out_dir.exists());
    }

    #[test]
    fn writes_line_with_newline_terminator() {
        let tmp = TempDir::new().unwrap();
        let mut writer = RotatingWriter::new(tmp.path(), 1024 * 1024);
        writer.write_line("line1").unwrap();
        writer.write_line("line2").unwrap();

        let files = list_jsonl_files(tmp.path());
        assert_eq!(files.len(), 1);

        let content = fs::read_to_string(&files[0]).unwrap();
        assert_eq!(content, "line1\nline2\n");
    }

    #[test]
    fn file_naming_follows_pattern() {
        let tmp = TempDir::new().unwrap();
        let mut writer = RotatingWriter::new(tmp.path(), 1024 * 1024);
        writer.write_line("test").unwrap();

        let files = list_jsonl_files(tmp.path());
        assert_eq!(files.len(), 1);

        let name = files[0].file_name().unwrap().to_string_lossy().to_string();
        let today = Utc::now().format("%Y-%m-%d").to_string();
        assert_eq!(name, format!("traj-{}-001.jsonl", today));
    }

    #[test]
    fn rotates_when_size_exceeded() {
        let tmp = TempDir::new().unwrap();
        // Set max to 20 bytes so rotation triggers quickly
        let mut writer = RotatingWriter::new(tmp.path(), 20);

        // "hello world" = 11 bytes + 1 newline = 12
        writer.write_line("hello world").unwrap();
        // "second line" = 11 bytes + 1 newline = 12; total would be 24 > 20
        writer.write_line("second line").unwrap();

        let files = list_jsonl_files(tmp.path());
        assert_eq!(files.len(), 2);

        // Verify each file has exactly one line
        for f in &files {
            let content = fs::read_to_string(f).unwrap();
            let lines: Vec<&str> = content.lines().collect();
            assert_eq!(lines.len(), 1);
        }
    }

    #[test]
    fn sequence_increments_on_rotation() {
        let tmp = TempDir::new().unwrap();
        let mut writer = RotatingWriter::new(tmp.path(), 6);

        writer.write_line("aaaa").unwrap(); // 5 bytes -> file 001
        writer.write_line("bbbb").unwrap(); // 5+5=10 > 6, rotate -> file 002
        writer.write_line("cccc").unwrap(); // 5+5=10 > 6, rotate -> file 003

        let mut files = list_jsonl_files(tmp.path());
        files.sort();

        let today = Utc::now().format("%Y-%m-%d").to_string();
        let names: Vec<String> = files
            .iter()
            .map(|f| f.file_name().unwrap().to_string_lossy().to_string())
            .collect();

        assert!(names.contains(&format!("traj-{}-001.jsonl", today)));
        assert!(names.contains(&format!("traj-{}-002.jsonl", today)));
        assert!(names.contains(&format!("traj-{}-003.jsonl", today)));
    }

    #[test]
    fn resumes_sequence_from_existing_files() {
        let tmp = TempDir::new().unwrap();
        let today = Utc::now().format("%Y-%m-%d").to_string();

        // Pre-create files to simulate previous runs
        fs::write(
            tmp.path().join(format!("traj-{}-005.jsonl", today)),
            "old\n",
        )
        .unwrap();

        let mut writer = RotatingWriter::new(tmp.path(), 1024 * 1024);
        writer.write_line("new data").unwrap();

        let files = list_jsonl_files(tmp.path());
        let names: Vec<String> = files
            .iter()
            .map(|f| f.file_name().unwrap().to_string_lossy().to_string())
            .collect();

        assert!(names.contains(&format!("traj-{}-006.jsonl", today)));
    }

    #[cfg(unix)]
    #[test]
    fn files_have_0o600_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let tmp = TempDir::new().unwrap();
        let mut writer = RotatingWriter::new(tmp.path(), 1024 * 1024);
        writer.write_line("secret data").unwrap();

        let files = list_jsonl_files(tmp.path());
        let perms = fs::metadata(&files[0]).unwrap().permissions();
        assert_eq!(perms.mode() & 0o777, 0o600);
    }

    #[test]
    fn handles_empty_line() {
        let tmp = TempDir::new().unwrap();
        let mut writer = RotatingWriter::new(tmp.path(), 1024 * 1024);
        writer.write_line("").unwrap();

        let files = list_jsonl_files(tmp.path());
        let content = fs::read_to_string(&files[0]).unwrap();
        assert_eq!(content, "\n");
    }

    #[test]
    fn write_error_on_invalid_path_returns_err() {
        let mut writer = RotatingWriter::new("/dev/null/impossible/path", 1024);
        let result = writer.write_line("should fail");
        assert!(result.is_err());
    }

    #[test]
    fn first_line_not_rotated_even_if_exceeds_max() {
        let tmp = TempDir::new().unwrap();
        // max_file_bytes = 5, but a single line is longer
        let mut writer = RotatingWriter::new(tmp.path(), 5);
        writer
            .write_line("this line is much longer than 5 bytes")
            .unwrap();

        let files = list_jsonl_files(tmp.path());
        // Should still be written to one file (we don't split a single line)
        assert_eq!(files.len(), 1);
        let content = fs::read_to_string(&files[0]).unwrap();
        assert!(content.contains("this line is much longer than 5 bytes"));
    }

    fn list_jsonl_files(dir: &Path) -> Vec<PathBuf> {
        fs::read_dir(dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|ext| ext == "jsonl"))
            .collect()
    }
}
