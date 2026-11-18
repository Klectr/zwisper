//! Hear the dictation muffle without installing the app.
//!
//!   cargo run -p zwisper-audio-macos --example muffle_preview -- in.wav out.wav [ON_SECS [OFF_SECS]]
//!
//! `in.wav` must be 32-bit float PCM, mono or stereo, e.g. made with:
//!   say -o speech.aiff "Some sentence"
//!   afconvert -f WAVE -d LEF32@48000 speech.aiff in.wav
//!
//! The output starts open, muffles at ON_SECS (default 1) and opens again at
//! OFF_SECS (default: 1 s before the end), so you hear the glide both ways.

use zwisper_audio_macos::muffle::Muffler;

fn read_f32_wav(bytes: &[u8]) -> (u32, u16, Vec<f32>) {
    assert!(&bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WAVE", "not a WAV file");
    let (mut pos, mut rate, mut channels, mut data) = (12, 0, 0, Vec::new());
    while pos + 8 <= bytes.len() {
        let id = &bytes[pos..pos + 4];
        let len = u32::from_le_bytes(bytes[pos + 4..pos + 8].try_into().unwrap()) as usize;
        let body = &bytes[pos + 8..(pos + 8 + len).min(bytes.len())];
        match id {
            b"fmt " => {
                let format = u16::from_le_bytes([body[0], body[1]]);
                channels = u16::from_le_bytes([body[2], body[3]]);
                rate = u32::from_le_bytes(body[4..8].try_into().unwrap());
                let bits = u16::from_le_bytes([body[14], body[15]]);
                assert!(bits == 32 && (format == 3 || format == 0xFFFE), "need 32-bit float PCM (see usage)");
            }
            b"data" => data = body.chunks_exact(4).map(|b| f32::from_le_bytes(b.try_into().unwrap())).collect(),
            _ => {}
        }
        pos += 8 + len + (len & 1);
    }
    (rate, channels, data)
}

fn write_f32_wav(path: &str, rate: u32, l: &[f32], r: &[f32]) {
    let data_len = (l.len() * 8) as u32;
    let mut out = Vec::with_capacity(44 + data_len as usize);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&3u16.to_le_bytes()); // IEEE float
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&rate.to_le_bytes());
    out.extend_from_slice(&(rate * 8).to_le_bytes());
    out.extend_from_slice(&8u16.to_le_bytes());
    out.extend_from_slice(&32u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for (a, b) in l.iter().zip(r) {
        out.extend_from_slice(&a.to_le_bytes());
        out.extend_from_slice(&b.to_le_bytes());
    }
    std::fs::write(path, out).expect("write output");
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let (input, output) = match args.as_slice() {
        [_, i, o, ..] if args.len() <= 5 => (i, o),
        _ => {
            eprintln!("usage: muffle_preview in.wav out.wav [ON_SECS [OFF_SECS]]");
            std::process::exit(2);
        }
    };
    let secs = |i: usize| args.get(i).map(|s| s.parse::<f32>().expect("seconds must be a number"));
    let (rate, channels, samples) = read_f32_wav(&std::fs::read(input).expect("read input"));
    let ch = channels.max(1) as usize;
    let mut l: Vec<f32> = samples.chunks(ch).map(|f| f[0]).collect();
    let mut r: Vec<f32> = samples.chunks(ch).map(|f| f[f.len().min(2) - 1]).collect();

    let mut m = Muffler::new(rate as f32);
    let block = 512;
    let at = |s: f32| (s * rate as f32) as usize;
    let on = secs(3).map(at).unwrap_or(rate as usize);
    let off = secs(4).map(at).unwrap_or(l.len().saturating_sub(rate as usize));
    for start in (0..l.len()).step_by(block) {
        let end = (start + block).min(l.len());
        let target = if start >= on && start < off { 1.0 } else { 0.0 };
        m.process(&mut l[start..end], &mut r[start..end], target);
    }
    write_f32_wav(output, rate, &l, &r);
    println!("wrote {output} ({:.1} s)", l.len() as f32 / rate as f32);
}
