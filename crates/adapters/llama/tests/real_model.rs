//! Real-model tests. They download the Notes pack into `/home/user/zwisper-models`
//! (override with `ZWISPER_MODELS`) on first run. Run with
//! `cargo test --release -p zwisper-llama -- --ignored --nocapture --test-threads=1`.

use std::path::PathBuf;
use std::time::Instant;
use zwisper_core::domain::{Pack, Template, Tier};
use zwisper_core::ports::{ModelPaths, ModelStore, NoteInput, NoteWriter, TemplateCatalog};
use zwisper_core::testkit::note_writer_contract;
use zwisper_core::CancelToken;
use zwisper_llama::{LlamaNoteWriter, Variant};
use zwisper_templates::BuiltinTemplates;

fn notes_pack(tier: Tier) -> ModelPaths {
    let root = std::env::var_os("ZWISPER_MODELS").map(PathBuf::from).unwrap_or_else(|| "/home/user/zwisper-models".into());
    let store = zwisper_models_http::HttpModelStore::new(root);
    if store.paths(Pack::Notes, tier).is_err() {
        store
            .download(Pack::Notes, tier, &mut |_, _| {}, &CancelToken::new())
            .expect("download notes pack");
    }
    store.paths(Pack::Notes, tier).expect("pack paths")
}

const TRANSCRIPT: &str = "\
[0:02] You: Okay, let's get started. This is the launch check-in for the Zwisper beta.
[0:07] Priya: Sure. Quick status from design: the onboarding screens are done, but the model download dialog still needs a progress bar and an error state.
[0:18] Marcus: On the engineering side, transcription is solid. Diarization works, but long meetings over an hour are slow, around four minutes of processing.
[0:29] You: Is that a blocker for the beta?
[0:31] Marcus: I don't think so. Most meetings are under an hour. I'd ship it and optimize after.
[0:37] Priya: Agreed, as long as we show a progress indicator so people know it's working.
[0:42] You: Okay, decision: we ship the beta with the current diarization speed and add a progress indicator. What about the date?
[0:49] Marcus: If Priya gets me the dialog designs by Wednesday, I can have the build ready Friday.
[0:55] Priya: Wednesday works. I'll send the designs and the error copy.
[1:00] You: Great. So beta goes out next Monday, the 24th. I'll write the announcement email and invite the first fifty testers.
[1:08] Marcus: I'll also set up crash reporting before Friday.
[1:12] You: Perfect. Anything else? No? Thanks everyone.
";

fn report(name: &str, w: &LlamaNoteWriter) {
    let s = w.last_stats();
    println!(
        "{name}: prompt {} tok in {:.2}s ({:.0} tok/s), generated {} tok in {:.2}s ({:.1} tok/s), ctx {}",
        s.prompt_tokens,
        s.prompt_secs,
        s.prompt_tokens_per_sec(),
        s.generated_tokens,
        s.generate_secs,
        s.tokens_per_sec(),
        s.context_tokens
    );
}

fn run(variant: Variant, tier: Tier) {
    let paths = notes_pack(tier);
    let t0 = Instant::now();
    let mut w = LlamaNoteWriter::load(&paths, variant).expect("load");
    println!("{}: loaded in {:.2?}", w.id(), t0.elapsed());
    assert_eq!(w.id(), variant.engine_id());
    assert_eq!(w.input_budget_tokens(), variant.input_budget_tokens());
    let n = w.count_tokens("Hello world, this is a test.");
    assert!((5..=10).contains(&n), "token count {n}");

    note_writer_contract(&mut w);
    report("contract", &w);

    let input = NoteInput {
        title: "Beta launch check-in".into(),
        date: "Tuesday 18 November 2025".into(),
        transcript: TRANSCRIPT.into(),
        my_notes: "beta = Monday 24th?\nask about crash reporting".into(),
        speakers: vec!["You".into(), "Priya".into(), "Marcus".into()],
    };
    let prompt = BuiltinTemplates.prompt(Template::Summary, &input);
    let mut streamed = String::new();
    let mut deltas = 0;
    let out = w
        .generate(&prompt, &mut |d| {
            deltas += 1;
            streamed.push_str(d)
        }, &CancelToken::new())
        .expect("generate summary");
    report("summary", &w);
    println!("----- {} Summary ({deltas} deltas) -----\n{out}\n-----", w.id());
    assert_eq!(streamed.trim(), out);
    assert!(!out.contains("<think>"), "thinking leaked into the output");
    BuiltinTemplates.validate(Template::Summary, &out).expect("summary validates");

    // Cancelling mid-stream stops promptly.
    let cancel = CancelToken::new();
    let mut n = 0;
    let r = w.generate(&prompt, &mut |_| {
        n += 1;
        if n == 3 {
            cancel.cancel();
        }
    }, &cancel);
    assert!(matches!(r, Err(zwisper_core::Error::Cancelled)), "{r:?}");
    assert!(n <= 4);
}

#[test]
#[ignore = "downloads the Lite notes model (~1.1 GB)"]
fn lite_qwen3_1_7b() {
    run(Variant::Lite, Tier::Lite);
}

#[test]
#[ignore = "downloads the Standard notes model (~2.5 GB)"]
fn standard_qwen3_4b_2507() {
    run(Variant::Standard, Tier::Standard);
}
