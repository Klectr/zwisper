//! Plain-file storage strategies.
//!
//! ```text
//! <library>/
//! ├── Meetings/2026-11-17 1030 Weekly sync/
//! │   ├── session.json      source of truth
//! │   ├── transcript.md     rendered for humans
//! │   ├── notes.md          active template's note
//! │   └── audio/{mic,system}.wav
//! └── Dictations/
//!     ├── 2026-11-17.md     human-readable log
//!     └── dictations.jsonl  machine-readable log
//! ```

use chrono::Local;
use parking_lot::Mutex;
use std::collections::HashMap;
use std::fs;
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use zwisper_core::domain::*;
use zwisper_core::ports::*;
use zwisper_core::{Error, Result};

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    let tmp = path.with_extension("tmp");
    {
        let mut f = fs::File::create(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
    }
    fs::rename(tmp, path)?;
    Ok(())
}

fn sanitize(title: &str) -> String {
    let s: String = title
        .chars()
        .map(|c| if c.is_alphanumeric() || " -_.,'()&".contains(c) { c } else { ' ' })
        .collect();
    let s = s.split_whitespace().collect::<Vec<_>>().join(" ");
    let s: String = s.chars().take(60).collect();
    if s.is_empty() { "Meeting".into() } else { s }
}

pub fn render_transcript_md(s: &Session) -> String {
    let mut out = format!(
        "# {}\n\n_{}_\n\n",
        s.title,
        s.created.with_timezone(&Local).format("%A, %B %-d, %Y at %-I:%M %p")
    );
    let mut last: Option<SpeakerId> = None;
    for g in &s.segments {
        if last != Some(g.speaker_id) {
            out.push_str(&format!("\n**{}** · {}\n", s.speaker_name(g.speaker_id), fmt_clock(g.start_ms)));
            last = Some(g.speaker_id);
        }
        out.push_str(g.text.trim());
        out.push('\n');
    }
    out
}

pub fn render_notes_md(s: &Session) -> Option<String> {
    let t = s.active_template.or_else(|| s.notes.keys().next().copied())?;
    let n = s.notes.get(&t)?;
    let mut out = format!("# {}\n\n{}\n", s.title, n.markdown.trim());
    if !s.my_notes.trim().is_empty() {
        out.push_str(&format!("\n## My notes\n\n{}\n", s.my_notes.trim()));
    }
    Some(out)
}

/// Session repository + audio recorder over plain folders.
pub struct FsStore {
    root: PathBuf,
    index: Mutex<HashMap<SessionId, PathBuf>>,
}

impl FsStore {
    pub fn new(root: impl Into<PathBuf>) -> Result<Self> {
        let root = root.into();
        fs::create_dir_all(root.join("Meetings"))?;
        fs::create_dir_all(root.join("Dictations"))?;
        let s = FsStore { root, index: Mutex::default() };
        s.rescan();
        Ok(s)
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn rescan(&self) -> Vec<Session> {
        let mut idx = HashMap::new();
        let mut sessions = Vec::new();
        if let Ok(rd) = fs::read_dir(self.root.join("Meetings")) {
            for e in rd.flatten() {
                let p = e.path().join("session.json");
                match fs::read(&p).map_err(Error::from).and_then(|b| {
                    serde_json::from_slice::<Session>(&b).map_err(|e| Error::Storage(e.to_string()))
                }) {
                    Ok(s) => {
                        idx.insert(s.id, e.path());
                        sessions.push(s);
                    }
                    Err(err) => tracing::debug!(path = %p.display(), %err, "skipping folder"),
                }
            }
        }
        *self.index.lock() = idx;
        sessions
    }

    fn folder_for(&self, s: &Session) -> PathBuf {
        if let Some(p) = self.index.lock().get(&s.id) {
            return p.clone();
        }
        let base = format!(
            "{} {}",
            s.created.with_timezone(&Local).format("%Y-%m-%d %H%M"),
            sanitize(&s.title)
        );
        let meetings = self.root.join("Meetings");
        let mut p = meetings.join(&base);
        let mut n = 2;
        while p.exists() {
            p = meetings.join(format!("{base} {n}"));
            n += 1;
        }
        p
    }

    fn lookup(&self, id: SessionId) -> Option<PathBuf> {
        if let Some(p) = self.index.lock().get(&id) {
            return Some(p.clone());
        }
        self.rescan();
        self.index.lock().get(&id).cloned()
    }

    fn dict_dir(&self) -> PathBuf {
        self.root.join("Dictations")
    }
}

impl SessionRepository for FsStore {
    fn list(&self) -> Result<Vec<SessionSummary>> {
        Ok(self.rescan().iter().map(SessionSummary::from).collect())
    }

    fn load(&self, id: SessionId) -> Result<Session> {
        let dir = self.lookup(id).ok_or_else(|| Error::NotFound(id.to_string()))?;
        let bytes = fs::read(dir.join("session.json"))?;
        serde_json::from_slice(&bytes).map_err(|e| Error::Storage(e.to_string()))
    }

    fn save(&self, s: &Session) -> Result<()> {
        let dir = self.folder_for(s);
        fs::create_dir_all(&dir)?;
        let json = serde_json::to_vec_pretty(s).map_err(|e| Error::Storage(e.to_string()))?;
        atomic_write(&dir.join("session.json"), &json)?;
        atomic_write(&dir.join("transcript.md"), render_transcript_md(s).as_bytes())?;
        if let Some(n) = render_notes_md(s) {
            atomic_write(&dir.join("notes.md"), n.as_bytes())?;
        }
        self.index.lock().insert(s.id, dir);
        Ok(())
    }

    fn delete(&self, id: SessionId) -> Result<()> {
        if let Some(dir) = self.lookup(id) {
            fs::remove_dir_all(dir)?;
        }
        self.index.lock().remove(&id);
        Ok(())
    }

    fn folder(&self, id: SessionId) -> Option<PathBuf> {
        self.lookup(id)
    }

    fn append_dictation(&self, d: &Dictation) -> Result<()> {
        let local = d.at.with_timezone(&Local);
        let md = self.dict_dir().join(format!("{}.md", local.format("%Y-%m-%d")));
        let mut f = fs::OpenOptions::new().create(true).append(true).open(md)?;
        writeln!(f, "- **{}** {}", local.format("%-I:%M %p"), d.text.trim())?;
        let mut j = fs::OpenOptions::new().create(true).append(true).open(self.dict_dir().join("dictations.jsonl"))?;
        let line = serde_json::to_string(d).map_err(|e| Error::Storage(e.to_string()))?;
        writeln!(j, "{line}")?;
        Ok(())
    }

    fn dictations(&self, limit: usize) -> Result<Vec<Dictation>> {
        let p = self.dict_dir().join("dictations.jsonl");
        let f = match fs::File::open(p) {
            Ok(f) => f,
            Err(_) => return Ok(Vec::new()),
        };
        let mut all: Vec<Dictation> = std::io::BufReader::new(f)
            .lines()
            .map_while(|l| l.ok())
            .filter_map(|l| serde_json::from_str(&l).ok())
            .collect();
        all.reverse();
        all.truncate(limit);
        Ok(all)
    }

    fn clear_dictations(&self) -> Result<()> {
        for e in fs::read_dir(self.dict_dir())?.flatten() {
            fs::remove_file(e.path())?;
        }
        Ok(())
    }
}

// ───────────────────────────── audio ─────────────────────────────

struct WavTrack {
    writer: hound::WavWriter<std::io::BufWriter<fs::File>>,
    since_flush: usize,
}

impl TrackWriter for WavTrack {
    fn write(&mut self, frames: &[f32]) -> Result<()> {
        for &s in frames {
            let v = (s.clamp(-1.0, 1.0) * i16::MAX as f32) as i16;
            self.writer.write_sample(v).map_err(|e| Error::Storage(e.to_string()))?;
        }
        self.since_flush += frames.len();
        // Keep the header valid every ~2 s so a crash loses seconds, not the meeting.
        if self.since_flush >= SAMPLE_RATE as usize * 2 {
            self.writer.flush().map_err(|e| Error::Storage(e.to_string()))?;
            self.since_flush = 0;
        }
        Ok(())
    }
    fn finish(self: Box<Self>) -> Result<()> {
        self.writer.finalize().map_err(|e| Error::Storage(e.to_string()))
    }
}

impl AudioRecorder for FsStore {
    fn create(&self, session: SessionId, track: TrackKind) -> Result<Box<dyn TrackWriter>> {
        let dir = self.lookup(session).ok_or_else(|| Error::NotFound(session.to_string()))?.join("audio");
        fs::create_dir_all(&dir)?;
        let spec = hound::WavSpec { channels: 1, sample_rate: SAMPLE_RATE, bits_per_sample: 16, sample_format: hound::SampleFormat::Int };
        let writer = hound::WavWriter::create(dir.join(format!("{}.wav", track.file_stem())), spec)
            .map_err(|e| Error::Storage(e.to_string()))?;
        Ok(Box::new(WavTrack { writer, since_flush: 0 }))
    }

    fn read(&self, session: SessionId, track: TrackKind) -> Result<Vec<f32>> {
        let p = self
            .lookup(session)
            .ok_or_else(|| Error::NotFound(session.to_string()))?
            .join("audio")
            .join(format!("{}.wav", track.file_stem()));
        let mut r = hound::WavReader::open(p).map_err(|e| Error::Storage(e.to_string()))?;
        r.samples::<i16>()
            .map(|s| s.map(|v| v as f32 / i16::MAX as f32).map_err(|e| Error::Storage(e.to_string())))
            .collect()
    }
}

// ───────────────────────────── settings ─────────────────────────────

pub struct TomlSettings {
    path: PathBuf,
    cache: Mutex<Option<Settings>>,
}

impl TomlSettings {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        TomlSettings { path: path.into(), cache: Mutex::default() }
    }
}

impl SettingsStore for TomlSettings {
    fn load(&self) -> Settings {
        let mut c = self.cache.lock();
        if let Some(s) = c.as_ref() {
            return s.clone();
        }
        let s = fs::read_to_string(&self.path)
            .ok()
            .and_then(|t| toml::from_str(&t).ok())
            .unwrap_or_default();
        *c = Some(s);
        c.clone().unwrap()
    }

    fn save(&self, s: &Settings) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)?;
        }
        let text = toml::to_string_pretty(s).map_err(|e| Error::Storage(e.to_string()))?;
        atomic_write(&self.path, text.as_bytes())?;
        *self.cache.lock() = Some(s.clone());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repository_contract() {
        let dir = tempfile::tempdir().unwrap();
        zwisper_core::testkit::repository_contract(&FsStore::new(dir.path()).unwrap());
    }

    #[test]
    fn writes_human_readable_files_and_audio_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let store = FsStore::new(dir.path()).unwrap();
        let mut s = Session::new("Weekly / sync", chrono::Utc::now());
        s.segments.push(TranscriptSegment {
            id: 0, track: TrackKind::Mic, speaker_id: SpeakerId::YOU, start_ms: 6000, end_ms: 7000,
            text: "Ship Thursday.".into(), words: vec![], edited: false,
        });
        store.save(&s).unwrap();
        let folder = store.folder(s.id).unwrap();
        assert!(folder.file_name().unwrap().to_string_lossy().ends_with("Weekly sync"));
        let md = fs::read_to_string(folder.join("transcript.md")).unwrap();
        assert!(md.contains("**You** · 0:06\nShip Thursday."));

        let mut w = store.create(s.id, TrackKind::System).unwrap();
        w.write(&[0.0, 0.5, -0.5, 1.0]).unwrap();
        w.finish().unwrap();
        let back = store.read(s.id, TrackKind::System).unwrap();
        assert_eq!(back.len(), 4);
        assert!((back[1] - 0.5).abs() < 1e-3 && (back[3] - 1.0).abs() < 1e-3);

        // Renaming keeps the folder stable.
        s.title = "Renamed".into();
        store.save(&s).unwrap();
        assert_eq!(store.folder(s.id).unwrap(), folder);
        // A fresh store finds it again by scanning.
        let again = FsStore::new(dir.path()).unwrap();
        assert_eq!(again.load(s.id).unwrap().title, "Renamed");
    }

    #[test]
    fn settings_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("cfg/config.toml");
        let st = TomlSettings::new(&p);
        assert_eq!(st.load(), Settings::default());
        let mut s = Settings::default();
        s.tier = Some(Tier::Lite);
        s.dictate_shortcut = "Ctrl+Space".into();
        st.save(&s).unwrap();
        assert_eq!(TomlSettings::new(&p).load(), s);
    }
}
