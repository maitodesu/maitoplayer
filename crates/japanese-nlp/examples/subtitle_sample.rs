use std::{env, fs, process};

use japanese_nlp::LinderaTokenizer;
use ports::TokenizerPort;

const DEFAULT_SAMPLE_SIZE: usize = 40;

fn main() {
    if let Err(error) = run() {
        eprintln!("{error}");
        process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let path = env::args()
        .nth(1)
        .ok_or("usage: subtitle_sample <Japanese.srt> [sample-size]")?;
    let sample_size = env::args()
        .nth(2)
        .map_or(Ok(DEFAULT_SAMPLE_SIZE), |value| value.parse())?;
    let source = fs::read_to_string(path)?.replace("\r\n", "\n");
    let cues = source
        .split("\n\n")
        .filter_map(parse_eligible_cue)
        .collect::<Vec<_>>();
    if sample_size < 2 || sample_size > cues.len() {
        return Err("sample size must be between 2 and the eligible cue count".into());
    }

    let tokenizer = LinderaTokenizer::embedded_ipadic()?;
    println!("eligible cues: {}", cues.len());
    println!("sample size: {sample_size}");
    for sample_index in 0..sample_size {
        let cue_position = sample_index * (cues.len() - 1) / (sample_size - 1);
        let (cue_id, text) = &cues[cue_position];
        println!("\n## cue {cue_id}: {text}");
        for token in tokenizer.tokenize(text)? {
            println!(
                "{}\t{}\t{}\t{}",
                token.surface,
                token.lemma,
                token
                    .part_of_speech
                    .first()
                    .map_or("unknown", String::as_str),
                token.reading
            );
        }
    }
    Ok(())
}

fn parse_eligible_cue(block: &str) -> Option<(u32, String)> {
    let mut lines = block.lines();
    let cue_id = lines
        .next()?
        .trim()
        .trim_start_matches('\u{feff}')
        .parse()
        .ok()?;
    if !lines.next()?.contains("-->") {
        return None;
    }
    let text = lines.map(str::trim).collect::<Vec<_>>().join("\n");
    text.chars()
        .any(is_japanese_lexical_character)
        .then_some((cue_id, text))
}

fn is_japanese_lexical_character(character: char) -> bool {
    matches!(character,
        '\u{3040}'..='\u{30ff}' |
        '\u{3400}'..='\u{4dbf}' |
        '\u{4e00}'..='\u{9fff}' |
        '\u{f900}'..='\u{faff}'
    )
}
