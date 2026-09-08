use std::{path::PathBuf, sync::Arc, time::Instant};

use contracts::{TokenId, TokenV1};
use dictionary::{EnrichedDictionary, LexicalMetadata, SqliteDictionary};
use ports::DictionaryPort;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .ok_or("usage: lookup_probe <jmdict.sqlite> [iterations] [lexical-metadata.sqlite]")?;
    let iterations = std::env::args()
        .nth(2)
        .map(|value| value.parse::<usize>())
        .transpose()?
        .unwrap_or(100);

    let open_started = Instant::now();
    let base: Arc<dyn DictionaryPort> = Arc::new(SqliteDictionary::open(&path)?);
    let dictionary: Arc<dyn DictionaryPort> = match std::env::args_os().nth(3) {
        Some(metadata_path) => Arc::new(EnrichedDictionary::new(
            base,
            Some(LexicalMetadata::open(&PathBuf::from(metadata_path))?),
        )),
        None => base,
    };
    let open_elapsed = open_started.elapsed();
    let tokens = [
        token("航空", "航空", "こうくう"),
        token("自衛隊", "自衛隊", "じえいたい"),
        token("パイロット", "パイロット", "ぱいろっと"),
        token("一緒", "一緒", "いっしょ"),
        token("飛ん", "飛ぶ", "とん"),
        token("見る", "見る", "みる"),
        token("空", "空", "そら"),
        token("の", "の", "の"),
        token("は", "は", "は"),
        token("なる", "なる", "なる"),
    ];

    let lookup_started = Instant::now();
    let mut result_count = 0_usize;
    let mut pitch_count = 0_usize;
    let mut jlpt_count = 0_usize;
    for _ in 0..iterations {
        for token in &tokens {
            let entries = dictionary.lookup(token)?;
            result_count += entries.len();
            pitch_count += entries
                .iter()
                .map(|entry| entry.pitch_accents.len())
                .sum::<usize>();
            jlpt_count += entries
                .iter()
                .filter(|entry| entry.jlpt_level.is_some())
                .count();
        }
    }
    let lookup_elapsed = lookup_started.elapsed();
    let lookup_count = iterations * tokens.len();

    println!(
        "dictionary_open_ms={:.3}",
        open_elapsed.as_secs_f64() * 1_000.0
    );
    println!("lookup_count={lookup_count}");
    println!(
        "lookup_total_ms={:.3}",
        lookup_elapsed.as_secs_f64() * 1_000.0
    );
    println!(
        "lookup_mean_us={:.3}",
        lookup_elapsed.as_secs_f64() * 1_000_000.0 / lookup_count as f64
    );
    println!("result_count={result_count}");
    println!("pitch_pattern_count={pitch_count}");
    println!("jlpt_result_count={jlpt_count}");
    Ok(())
}

fn token(surface: &str, lemma: &str, reading: &str) -> TokenV1 {
    TokenV1 {
        token_id: TokenId::new(format!("probe-{surface}")),
        surface: surface.into(),
        byte_start: 0,
        byte_end: u32::try_from(surface.len()).unwrap_or(u32::MAX),
        lemma: lemma.into(),
        reading: reading.into(),
        pronunciation: None,
        part_of_speech: Vec::new(),
        lookup_candidate: true,
    }
}
