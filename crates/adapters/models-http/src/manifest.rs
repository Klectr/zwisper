//! The curated model manifest. This IS the product's model list: editing it
//! means shipping a new app version. URLs are pinned to exact revisions and
//! every file is verified against its SHA-256.
//!
//! Logical file names (`name`) are the contract with the engine adapters:
//! * Speech:   "encoder", "decoder", "joiner", "tokens", "vad"
//! * Speakers: "segmentation", "embedding"
//! * Notes:    "gguf"

use zwisper_core::domain::{Pack, Tier};

#[derive(Debug, Clone, Copy)]
pub struct FileSpec {
    pub name: &'static str,
    /// File name on disk.
    pub file: &'static str,
    pub url: &'static str,
    pub sha256: &'static str,
    pub bytes: u64,
}

#[derive(Debug, Clone, Copy)]
pub struct PackSpec {
    pub pack: Pack,
    /// `None` = same files for every tier.
    pub tier: Option<Tier>,
    /// Folder name under the models dir; bump to force a re-download.
    pub version: &'static str,
    pub files: &'static [FileSpec],
}

impl PackSpec {
    pub fn bytes(&self) -> u64 {
        self.files.iter().map(|f| f.bytes).sum()
    }
}

pub static PACKS: &[PackSpec] = &[
    PackSpec {
        pack: Pack::Speech,
        tier: None,
        version: "parakeet-tdt-0.6b-v2-int8+silero-v5.1.2",
        files: &[
            FileSpec {
                name: "encoder",
                file: "encoder.int8.onnx",
                url: "https://huggingface.co/csukuangfj/sherpa-onnx-nemo-parakeet-tdt-0.6b-v2-int8/resolve/84b478d541cdbf0c296a7ab5f1e20a6f8f998114/encoder.int8.onnx",
                sha256: "92a10bd9c23936ea0566c29f963bc57c2a5df4261b6a8921c39abea66557ca8b",
                bytes: 652_282_309,
            },
            FileSpec {
                name: "decoder",
                file: "decoder.int8.onnx",
                url: "https://huggingface.co/csukuangfj/sherpa-onnx-nemo-parakeet-tdt-0.6b-v2-int8/resolve/84b478d541cdbf0c296a7ab5f1e20a6f8f998114/decoder.int8.onnx",
                sha256: "4d097bae3bac5c96018d5293ba39f1b657296102b142affda40378f38b020f41",
                bytes: 7_257_753,
            },
            FileSpec {
                name: "joiner",
                file: "joiner.int8.onnx",
                url: "https://huggingface.co/csukuangfj/sherpa-onnx-nemo-parakeet-tdt-0.6b-v2-int8/resolve/84b478d541cdbf0c296a7ab5f1e20a6f8f998114/joiner.int8.onnx",
                sha256: "77a29f3e5ac7d37074e0af465391b80f314015d5b97f0f90c6c7767f1d56f428",
                bytes: 1_739_080,
            },
            FileSpec {
                name: "tokens",
                file: "tokens.txt",
                url: "https://huggingface.co/csukuangfj/sherpa-onnx-nemo-parakeet-tdt-0.6b-v2-int8/resolve/84b478d541cdbf0c296a7ab5f1e20a6f8f998114/tokens.txt",
                sha256: "ec182b70dd42113aff6c5372c75cac58c952443eb22322f57bbd7f53977d497d",
                bytes: 9_384,
            },
            FileSpec {
                name: "vad",
                file: "silero_vad.onnx",
                url: "https://raw.githubusercontent.com/snakers4/silero-vad/v5.1.2/src/silero_vad/data/silero_vad.onnx",
                sha256: "2623a2953f6ff3d2c1e61740c6cdb7168133479b267dfef114a4a3cc5bdd788f",
                bytes: 2_327_524,
            },
        ],
    },
    PackSpec {
        pack: Pack::Speakers,
        tier: None,
        version: "pyannote-seg-3.0+wespeaker-resnet34-lm",
        files: &[
            FileSpec {
                name: "segmentation",
                file: "segmentation.onnx",
                url: "https://huggingface.co/csukuangfj/sherpa-onnx-pyannote-segmentation-3-0/resolve/9403a6902bb58e3d5ae8c7e77c3422de279db2e0/model.onnx",
                sha256: "220ad67ca923bef2fa91f2390c786097bf305bceb5e261d4af67b38e938e1079",
                bytes: 5_992_913,
            },
            FileSpec {
                name: "embedding",
                file: "wespeaker_en_voxceleb_resnet34_LM.onnx",
                url: "https://huggingface.co/csukuangfj/speaker-embedding-models/resolve/0743f301363dec56491a490f6d6cbc9d67f9a3bf/wespeaker_en_voxceleb_resnet34_LM.onnx",
                sha256: "e9848563da86f263117134dfd7ad63c92355b37de492b55e325400c9d9c39012",
                bytes: 26_530_550,
            },
        ],
    },
    PackSpec {
        pack: Pack::Notes,
        tier: Some(Tier::Standard),
        version: "qwen3-4b-instruct-2507-q4km",
        files: &[FileSpec {
            name: "gguf",
            file: "Qwen3-4B-Instruct-2507-Q4_K_M.gguf",
            url: "https://huggingface.co/unsloth/Qwen3-4B-Instruct-2507-GGUF/resolve/a06e946bb6b655725eafa393f4a9745d460374c9/Qwen3-4B-Instruct-2507-Q4_K_M.gguf",
            sha256: "3605803b982cb64aead44f6c1b2ae36e3acdb41d8e46c8a94c6533bc4c67e597",
            bytes: 2_497_281_120,
        }],
    },
    PackSpec {
        pack: Pack::Notes,
        tier: Some(Tier::Lite),
        version: "qwen3-1.7b-q4km",
        files: &[FileSpec {
            name: "gguf",
            file: "Qwen3-1.7B-Q4_K_M.gguf",
            url: "https://huggingface.co/unsloth/Qwen3-1.7B-GGUF/resolve/d7f544eead698dbd1f15126ef60b45a1e1933222/Qwen3-1.7B-Q4_K_M.gguf",
            sha256: "b139949c5bd74937ad8ed8c8cf3d9ffb1e99c866c823204dc42c0d91fa181897",
            bytes: 1_107_409_472,
        }],
    },
];

pub fn spec(pack: Pack, tier: Tier) -> &'static PackSpec {
    PACKS
        .iter()
        .find(|p| p.pack == pack && p.tier.map_or(true, |t| t == tier))
        .expect("every pack has a spec for every tier")
}

pub const ACKNOWLEDGEMENTS: &[&str] = &[
    "Speech recognition: NVIDIA Parakeet TDT 0.6B v2 — © NVIDIA, CC-BY-4.0 (huggingface.co/nvidia/parakeet-tdt-0.6b-v2)",
    "Voice activity detection: Silero VAD — © Silero Team, MIT",
    "Speaker segmentation: pyannote/segmentation-3.0 — © pyannote, MIT",
    "Speaker embeddings: WeSpeaker ResNet34 (VoxCeleb) — © WeNet community, Apache-2.0",
    "Notes: Qwen3-4B-Instruct-2507 and Qwen3-1.7B — © Alibaba Cloud, Apache-2.0",
    "Runtimes: sherpa-onnx (Apache-2.0), ONNX Runtime (MIT), llama.cpp (MIT)",
];
