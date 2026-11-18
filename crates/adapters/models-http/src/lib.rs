//! `HttpModelStore`: the `ModelStore` strategy that downloads the curated
//! manifest over HTTPS with resume + SHA-256 verification.
//!
//! Layout: `<root>/<pack>/<version>/<file>` plus a `<file>.ok` marker written
//! only after the hash matched.

pub mod manifest;

use manifest::{spec, FileSpec, PackSpec};
use sha2::{Digest, Sha256};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use zwisper_core::domain::{Pack, PackStatus, Tier};
use zwisper_core::ports::{ModelPaths, ModelStore, ProgressFn};
use zwisper_core::{CancelToken, Error, Result};

pub struct HttpModelStore {
    root: PathBuf,
    agent: ureq::Agent,
    /// Overridable for tests.
    packs: &'static [PackSpec],
}

impl HttpModelStore {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        let mut builder = ureq::AgentBuilder::new()
            .timeout_connect(std::time::Duration::from_secs(20))
            .timeout_read(std::time::Duration::from_secs(60))
            .user_agent(concat!("zwisper/", env!("CARGO_PKG_VERSION")));
        // Respect a configured HTTPS proxy (corporate networks).
        if let Some(p) = ["HTTPS_PROXY", "https_proxy", "ALL_PROXY"].iter().find_map(|k| std::env::var(k).ok()) {
            if let Ok(proxy) = ureq::Proxy::new(p) {
                builder = builder.proxy(proxy);
            }
        }
        HttpModelStore {
            root: root.into(),
            agent: builder.build(),
            packs: manifest::PACKS,
        }
    }

    /// Use a custom manifest (tests).
    pub fn with_manifest(mut self, packs: &'static [PackSpec]) -> Self {
        self.packs = packs;
        self
    }

    fn spec(&self, pack: Pack, tier: Tier) -> &'static PackSpec {
        if std::ptr::eq(self.packs, manifest::PACKS) {
            return spec(pack, tier);
        }
        self.packs
            .iter()
            .find(|p| p.pack == pack && p.tier.map_or(true, |t| t == tier))
            .expect("pack in manifest")
    }

    fn dir(&self, s: &PackSpec) -> PathBuf {
        self.root.join(format!("{:?}", s.pack).to_lowercase()).join(s.version)
    }

    fn is_ok(&self, s: &PackSpec, f: &FileSpec) -> bool {
        let dir = self.dir(s);
        dir.join(format!("{}.ok", f.file)).exists()
            && fs::metadata(dir.join(f.file)).map(|m| m.len() == f.bytes).unwrap_or(false)
    }

    fn fetch(&self, url: &str, dest: &Path, f: &FileSpec, base: u64, total: u64, progress: ProgressFn, cancel: &CancelToken) -> Result<()> {
        let part = dest.with_extension("part");
        let mut have = fs::metadata(&part).map(|m| m.len()).unwrap_or(0);
        if have > f.bytes {
            fs::remove_file(&part)?;
            have = 0;
        }
        if have < f.bytes {
            let mut req = self.agent.get(url);
            if have > 0 {
                req = req.set("Range", &format!("bytes={have}-"));
            }
            let resp = req.call().map_err(|e| Error::Network(e.to_string()))?;
            let resumed = resp.status() == 206;
            let mut out = OpenOptions::new().create(true).write(true).open(&part)?;
            if resumed {
                out.seek(SeekFrom::End(0))?;
            } else {
                out.set_len(0)?;
                have = 0;
            }
            let mut reader = resp.into_reader();
            let mut buf = vec![0u8; 1 << 16];
            loop {
                cancel.check()?;
                let n = reader.read(&mut buf).map_err(|e| Error::Network(e.to_string()))?;
                if n == 0 {
                    break;
                }
                out.write_all(&buf[..n])?;
                have += n as u64;
                progress(base + have, total);
            }
            out.flush()?;
        }
        // Verify the complete file.
        let mut hasher = Sha256::new();
        let mut file = File::open(&part)?;
        std::io::copy(&mut file, &mut hasher)?;
        let got = hex::encode(hasher.finalize());
        if got != f.sha256 {
            let _ = fs::remove_file(&part);
            return Err(Error::Network(format!("checksum mismatch for {} (got {got})", f.file)));
        }
        fs::rename(&part, dest)?;
        File::create(dest.with_file_name(format!("{}.ok", f.file)))?;
        Ok(())
    }
}

impl ModelStore for HttpModelStore {
    fn status(&self, pack: Pack, tier: Tier) -> PackStatus {
        let s = self.spec(pack, tier);
        if s.files.iter().all(|f| self.is_ok(s, f)) {
            PackStatus::Ready { bytes: s.bytes() }
        } else {
            PackStatus::NotDownloaded { bytes: s.bytes() }
        }
    }

    fn paths(&self, pack: Pack, tier: Tier) -> Result<ModelPaths> {
        let s = self.spec(pack, tier);
        if !s.files.iter().all(|f| self.is_ok(s, f)) {
            return Err(Error::ModelMissing(pack));
        }
        let dir = self.dir(s);
        Ok(ModelPaths { files: s.files.iter().map(|f| (f.name.to_string(), dir.join(f.file))).collect() })
    }

    fn download(&self, pack: Pack, tier: Tier, progress: ProgressFn, cancel: &CancelToken) -> Result<()> {
        let s = self.spec(pack, tier);
        let dir = self.dir(s);
        fs::create_dir_all(&dir)?;
        let total = s.bytes();
        let mut base = 0;
        for f in s.files {
            if !self.is_ok(s, f) {
                tracing::info!(file = f.file, "downloading model file");
                self.fetch(f.url, &dir.join(f.file), f, base, total, progress, cancel)?;
            }
            base += f.bytes;
            progress(base, total);
        }
        Ok(())
    }

    fn remove(&self, pack: Pack, tier: Tier) -> Result<()> {
        let dir = self.dir(self.spec(pack, tier));
        if dir.exists() {
            fs::remove_dir_all(dir)?;
        }
        Ok(())
    }

    fn acknowledgements(&self) -> Vec<String> {
        manifest::ACKNOWLEDGEMENTS.iter().map(|s| s.to_string()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_covers_every_pack_and_tier() {
        for pack in Pack::ALL {
            for tier in [Tier::Standard, Tier::Lite] {
                let s = spec(pack, tier);
                assert!(!s.files.is_empty());
                for f in s.files {
                    assert_eq!(f.sha256.len(), 64, "{}", f.file);
                    assert!(f.url.starts_with("https://") && f.url.ends_with(f.file.rsplit('/').next().unwrap()) || f.name == "segmentation");
                }
            }
        }
        assert_eq!(spec(Pack::Notes, Tier::Standard).files[0].name, "gguf");
        assert!(spec(Pack::Notes, Tier::Lite).bytes() < spec(Pack::Notes, Tier::Standard).bytes());
    }

    static TINY: &[PackSpec] = &[PackSpec {
        pack: Pack::Speech,
        tier: None,
        version: "test",
        files: &[FileSpec {
            name: "tokens",
            file: "tokens.txt",
            url: "https://huggingface.co/csukuangfj/sherpa-onnx-nemo-parakeet-tdt-0.6b-v2-int8/resolve/84b478d541cdbf0c296a7ab5f1e20a6f8f998114/tokens.txt",
            sha256: "ec182b70dd42113aff6c5372c75cac58c952443eb22322f57bbd7f53977d497d",
            bytes: 9_384,
        }],
    }];

    /// Network test: run with `cargo test -p zwisper-models-http -- --ignored`.
    #[test]
    #[ignore]
    fn downloads_verifies_and_removes() {
        let dir = tempfile::tempdir().unwrap();
        let store = HttpModelStore::new(dir.path()).with_manifest(TINY);
        assert!(matches!(store.status(Pack::Speech, Tier::Lite), PackStatus::NotDownloaded { .. }));
        let mut last = 0;
        store.download(Pack::Speech, Tier::Lite, &mut |d, _| last = d, &CancelToken::new()).unwrap();
        assert_eq!(last, 9_384);
        assert!(matches!(store.status(Pack::Speech, Tier::Lite), PackStatus::Ready { .. }));
        assert!(store.paths(Pack::Speech, Tier::Lite).unwrap().get("tokens").unwrap().exists());
        store.remove(Pack::Speech, Tier::Lite).unwrap();
        assert!(store.paths(Pack::Speech, Tier::Lite).is_err());
    }

    #[test]
    fn cancelled_download_stops() {
        let dir = tempfile::tempdir().unwrap();
        let store = HttpModelStore::new(dir.path()).with_manifest(TINY);
        let c = CancelToken::new();
        c.cancel();
        // Either cancelled before any bytes, or network error offline: never Ready.
        let _ = store.download(Pack::Speech, Tier::Lite, &mut |_, _| {}, &c);
        assert!(store.paths(Pack::Speech, Tier::Lite).is_err());
    }
}
